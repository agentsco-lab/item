//! item's own display output (ITEM_OUTPUT=kms): the window's buffers
//! straight to the panels through the kernel's DRM, no hwcomposer
//! (notes/drafts/51-own-display.md). The GPU's side is as with
//! hwcomposer: EGL draws into the same native window, and only its
//! present callback differs.
//!
//! - the first frame sets each panel's mode (SETCRTC, its half of the frame);
//! - every frame after goes to a committer thread, `present` waiting for
//!   nothing: one atomic commit for both panels as soon as the last one's
//!   flips are done (a command-mode panel takes a frame ~11 ms), the newest
//!   frame if one came meanwhile (the one passed over is free at once);
//! - the GPU's fence goes to the kernel with the buffer (SDE's input_fence),
//!   which sends it once drawn; the commit's release fences (SDE's
//!   output_fence, one a panel, merged) go onto the buffer itself, freeing it
//!   once it has gone into the panels.
//!
//! The vsync comes from the first panel's vblank events, read on a thread of
//! the output's own with the flips' events; synthesised from the period
//! should the kernel give none. The panels' calibration and the CRTCs'
//! votes for clock and bandwidth come from the device's profile.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::drm::{self, Card, ModeInfo, OBJ_CRTC, OBJ_PLANE};
use crate::profile::Profile;
use crate::{colour, now_ns, NativeBase, Output, PresentDetail, Win};

/// A panel as the output drives it.
struct Panel {
    connector: u32,
    crtc: u32,
    crtc_index: u32,
    mode: ModeInfo,
    /// Its half's first column in the frame.
    x: u32,
    /// The plane showing it (known once the mode is set).
    plane: u32,
}

#[derive(Default)]
struct Stats {
    frames: u32,
    long: u32,
    sum_ms: f64,
    max_ms: f64,
    errors: u32,
    /// Frames passed over for a newer one before their commit.
    passed: u32,
    commits: u32,
    commit_ns: u64,
}

struct Kms {
    card: Arc<Card>,
    profile: Profile,
    win: Win,
    panels: Vec<Panel>,
    /// A framebuffer a window buffer (by its address): imported once.
    fbs: HashMap<usize, u32>,
    /// The modes are set (the first frame shown).
    started: bool,
    /// The last framebuffer shown: the panels on again show it.
    shown_fb: u32,
    last: *mut c_void,
    last_ns: u64,
    stats: Stats,
}

// SAFETY: the pointers are the window's buffers, which belong to no thread;
// the state is only reached under KMS's mutex.
unsafe impl Send for Kms {}

static KMS: Mutex<Option<Kms>> = Mutex::new(None);
/// A commit on its way (its flips not all done), and its waiters.
static PENDING: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static FLIPS_LEFT: AtomicU64 = AtomicU64::new(0);
static POWER_ON: AtomicBool = AtomicBool::new(true);
/// The last flip's time: the synthesised vsync's phase.
static LAST_FLIP_NS: AtomicU64 = AtomicU64::new(0);
/// When the panels came on, until the first flip after (for the log).
static FIRST_FLIP_AFTER_ON: AtomicU64 = AtomicU64::new(0);

const GRALLOC_MAGIC: i32 = 0x676d_736d;
const PRIV_FLAGS_UBWC_ALIGNED: i32 = 0x0800_0000;

/// ANativeWindowBuffer as libhybris lays it out: the base, then width,
/// height, stride (pixels), format, usage; two reserved words; the
/// gralloc handle.
#[repr(C)]
struct WindowBuffer {
    base: NativeBase,
    width: i32,
    height: i32,
    stride: i32,
    format: i32,
    usage: i32,
    reserved: [*mut c_void; 2],
    handle: *const NativeHandle,
}

/// native_handle_t, then `num_fds` fds and `num_ints` ints: Qualcomm's
/// gralloc puts the buffer's fd first, and the magic ('gmsm') and the flags
/// (UBWC 0x08000000) first of the ints.
#[repr(C)]
struct NativeHandle {
    version: i32,
    num_fds: i32,
    num_ints: i32,
    data: [i32; 0],
}

fn wait_idle(timeout_ms: u64) -> bool {
    let (lock, cv) = &PENDING;
    let guard = lock.lock().unwrap();
    let (guard, _) = cv.wait_timeout_while(guard, std::time::Duration::from_millis(timeout_ms), |p| *p).unwrap();
    !*guard
}

fn set_pending(p: bool) {
    let (lock, cv) = &PENDING;
    *lock.lock().unwrap() = p;
    if !p {
        cv.notify_all();
    }
}

/// The own output on this device, if it is one item knows and the card is
/// ours (hwcomposer stopped): its window, `buffers` deep.
pub(crate) fn open(buffers: i32, win: Win) -> Result<Output, String> {
    let profile = crate::profile::detect().ok_or("a device item's own output does not know")?;
    let (card, conns) = master()?;
    let card = Arc::new(card);
    if conns.len() != 2 {
        return Err(format!("{} panels connected, not 2", conns.len()));
    }
    let mut panels = Vec::new();
    let mut x = 0;
    for c in conns {
        panels.push(Panel { connector: c.id, crtc: c.crtc, crtc_index: c.crtc_index, mode: c.mode, x, plane: 0 });
        x += c.mode.hdisplay as u32 + profile.gap;
    }
    let width = panels.iter().map(|p| p.mode.hdisplay as u32).sum::<u32>() + profile.gap;
    let height = panels.iter().map(|p| p.mode.vdisplay as u32).max().unwrap_or(0);
    let period = panels[0].mode.period_ns();

    // The votes first: hwcomposer, stopped, left the bus's maximum on each.
    for p in &panels {
        for &(name, v) in profile.votes {
            if let Err(e) = card.commit(&[(p.crtc, OBJ_CRTC, name, v)], 0) {
                eprintln!("hybris-hwc: kms: {name} on crtc {}: {e}", p.crtc);
            }
        }
    }
    apply_colour(&card, &profile, &panels, &std::env::var("ITEM_COLOUR").unwrap_or_else(|_| "vivid".into()));
    println!(
        "hybris-hwc: kms: {} - {} panels, frame {width}x{height}, {:.3} ms a frame",
        profile.name,
        panels.len(),
        period as f64 / 1e6
    );
    for (i, p) in panels.iter().enumerate() {
        println!("hybris-hwc: kms: panel {i}: connector {}, crtc {}, {}x{}, from x {}", p.connector, p.crtc, p.mode.hdisplay, p.mode.vdisplay, p.x);
    }

    let vblank_crtc = panels[0].crtc_index;
    *KMS.lock().unwrap() = Some(Kms { card: card.clone(), profile, win, panels, fbs: HashMap::new(), started: false, shown_fb: 0, last: null_mut(), last_ns: 0, stats: Stats::default() });
    crate::KMS_ON.store(true, Ordering::Relaxed);
    std::thread::Builder::new().name("kms-events".into()).spawn(move || events(card, vblank_crtc, period)).map_err(|e| e.to_string())?;
    std::thread::Builder::new().name("kms-commit".into()).spawn(committer).map_err(|e| e.to_string())?;

    let window = (win.create)(width, height, crate::HAL_PIXEL_FORMAT_RGBA_8888, present, null_mut());
    if window.is_null() {
        return Err("HWCNativeWindowCreate failed".into());
    }
    (win.set_buffer_count)(window, buffers);
    Ok(Output { width: width as i32, height: height as i32, vsync_period_ns: period as i64, window })
}

/// The card as its master, and its panels. hwcomposer gone, the kernel
/// may hold its mastership a moment yet (2026-10-29: a start in that moment
/// had every commit refused, the panels dark): opened again until a test
/// commit is allowed, for up to 5 s.
fn master() -> Result<(Card, Vec<drm::Connector>), String> {
    let until = now_ns() + 5_000_000_000;
    loop {
        let card = Card::open()?;
        let conns = card.panels()?;
        let probe = conns.first().map(|c| card.commit(&[(c.crtc, OBJ_CRTC, "ACTIVE", 1)], drm::ATOMIC_TEST_ONLY | drm::ATOMIC_ALLOW_MODESET));
        match probe {
            Some(Err(e)) if e.raw_os_error() == Some(libc::EACCES) => {
                if now_ns() > until {
                    return Err("the card has another master (hwcomposer still up?)".into());
                }
                drop(card);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => return Ok((card, conns)),
        }
    }
}

/// Each panel's factory calibration in `mode` (vivid, srgb), or none (off).
fn apply_colour(card: &Card, profile: &Profile, panels: &[Panel], mode: &str) {
    for (i, p) in panels.iter().enumerate() {
        let calib = match mode {
            "off" => None,
            _ => colour::load(profile.calib[i.min(1)], mode).map_err(|e| eprintln!("hybris-hwc: kms: colour: {e}")).ok(),
        };
        let blobs = [
            (profile.igc, calib.as_ref().map(|c| colour::igc_blob(c, "rgb"))),
            (profile.pcc, calib.as_ref().map(colour::pcc_blob)),
            (profile.gc, calib.as_ref().map(|c| colour::gc_blob(c, "rgb"))),
        ];
        let said: Vec<String> = blobs
            .iter()
            .map(|(name, b)| match card.set_blob(p.crtc, name, b.as_deref()) {
                Ok(()) => format!("{name} {}", if b.is_some() { "on" } else { "off" }),
                Err(e) => e,
            })
            .collect();
        println!("hybris-hwc: kms: panel {i}: colour {mode}: {}", said.join(", "));
    }
}

/// The card's events: the first panel's vblanks (the vsync, asked for again
/// after each) and the flips done (a commit's last frees the next).
fn events(card: Arc<Card>, crtc_index: u32, period: u64) {
    let mut vblank_ok = card.request_vblank(crtc_index, 1).is_ok();
    if !vblank_ok {
        eprintln!("hybris-hwc: kms: no vblank events: the vsync synthesised from the flips");
    }
    let mut next_synth = now_ns() + period;
    let mut was_on = true;
    loop {
        let on = POWER_ON.load(Ordering::Relaxed);
        if on && !was_on {
            // The panels on again: their vblanks asked for anew.
            vblank_ok = card.request_vblank(crtc_index, 1).is_ok();
            next_synth = now_ns() + period;
        }
        was_on = on;
        // Dark, no vsync at all (as hwcomposer's): nothing wakes the
        // compositor for frames no one sees.
        let wait = if vblank_ok || !on { 100 } else { ((next_synth.saturating_sub(now_ns())) / 1_000_000).max(1) as i32 };
        for e in card.events(wait) {
            match e.kind {
                1 => {
                    crate::vsync(e.time_ns as i64);
                    if card.request_vblank(crtc_index, 1).is_err() {
                        vblank_ok = false;
                    }
                }
                2 => {
                    LAST_FLIP_NS.store(e.time_ns, Ordering::Relaxed);
                    let on_at = FIRST_FLIP_AFTER_ON.swap(0, Ordering::Relaxed);
                    if on_at != 0 {
                        println!("hybris-hwc: kms: the first flip after the panels came on, {:.1} ms after", (now_ns() - on_at) as f64 / 1e6);
                    }
                    if FLIPS_LEFT.fetch_sub(1, Ordering::AcqRel) <= 1 {
                        FLIPS_LEFT.store(0, Ordering::Release);
                        set_pending(false);
                    }
                }
                _ => {}
            }
        }
        if !vblank_ok && on {
            let now = now_ns();
            if now >= next_synth {
                crate::vsync(next_synth as i64);
                // In phase with the last flip, a period on.
                let flip = LAST_FLIP_NS.load(Ordering::Relaxed);
                next_synth = if flip > 0 && flip + period > now { flip + period } else { next_synth + period };
                while next_synth <= now {
                    next_synth += period;
                }
            }
            // Asked for again now and then: they may come yet.
            if now % 1_000_000_000 < period {
                vblank_ok = card.request_vblank(crtc_index, 1).is_ok();
            }
        }
    }
}

/// The buffer's framebuffer: imported the first time it is seen.
fn framebuffer(k: &mut Kms, buffer: *mut c_void) -> Result<u32, String> {
    if let Some(&fb) = k.fbs.get(&(buffer as usize)) {
        return Ok(fb);
    }
    // SAFETY: the window's buffers are ANativeWindowBuffers whose handle is
    // gralloc's native_handle_t, `num_fds + num_ints` ints after its head.
    let (w, h, stride, fd, ints) = unsafe {
        let b = &*(buffer as *const WindowBuffer);
        let nh = &*b.handle;
        let n = (nh.num_fds + nh.num_ints) as usize;
        let data = std::slice::from_raw_parts(nh.data.as_ptr(), n);
        (b.width as u32, b.height as u32, b.stride as u32, data[0], data[nh.num_fds as usize..].to_vec())
    };
    let ubwc = ints.first() == Some(&GRALLOC_MAGIC) && ints.get(1).is_some_and(|f| f & PRIV_FLAGS_UBWC_ALIGNED != 0);
    let fb = k.card.framebuffer(fd, w, h, stride * 4, ubwc.then_some(drm::MOD_QCOM_COMPRESSED))?;
    println!("hybris-hwc: kms: buffer {:#x}: {w}x{h} (stride {stride}), UBWC {ubwc}: framebuffer {fb}", buffer as usize);
    k.fbs.insert(buffer as usize, fb);
    Ok(fb)
}

/// The first frame: each panel's mode set with its half (the GPU waited for
/// here, once), and the plane that shows it found.
fn start(k: &mut Kms, fb: u32, acquire: i32) -> Result<(), String> {
    wait_fence(acquire);
    for p in &mut k.panels {
        k.card.set_crtc(p.crtc, p.connector, p.mode, fb, p.x)?;
        p.plane = k.card.plane_on(p.crtc)?;
    }
    k.started = true;
    Ok(())
}

fn wait_fence(fd: i32) {
    if fd >= 0 {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, ours.
        unsafe { libc::poll(&mut pfd, 1, 1000) };
    }
}

/// A frame for both panels: `fb`, once `acquire` has signalled. The
/// panels' release fences, merged (-1 on none): SDE's `output_fence`, which
/// signals once the frame has gone into the panels (~8 ms after the
/// transfer begins, 2026-10-29) - a command-mode panel keeps the picture in
/// its own memory after, and the buffer is the GPU's again.
fn commit(k: &Kms, fb: u32, acquire: i32) -> Result<i32, String> {
    let mut release = vec![-1i32; k.panels.len()];
    let mut changes: Vec<(u32, u32, &str, u64)> = Vec::new();
    for (i, p) in k.panels.iter().enumerate() {
        let (w, h) = (p.mode.hdisplay as u64, p.mode.vdisplay as u64);
        changes.extend_from_slice(&[
            (p.plane, OBJ_PLANE, "FB_ID", fb as u64),
            (p.plane, OBJ_PLANE, "CRTC_ID", p.crtc as u64),
            (p.plane, OBJ_PLANE, "SRC_X", (p.x as u64) << 16),
            (p.plane, OBJ_PLANE, "SRC_Y", 0),
            (p.plane, OBJ_PLANE, "SRC_W", w << 16),
            (p.plane, OBJ_PLANE, "SRC_H", h << 16),
            (p.plane, OBJ_PLANE, "CRTC_X", 0),
            (p.plane, OBJ_PLANE, "CRTC_Y", 0),
            (p.plane, OBJ_PLANE, "CRTC_W", w),
            (p.plane, OBJ_PLANE, "CRTC_H", h),
            // The GPU's fence: SDE's own property (it waits on this one;
            // the standard IN_FENCE_FD it leaves unwaited).
            (p.plane, OBJ_PLANE, "input_fence", acquire.max(0) as u64),
            // Opaque, as hwcomposer's client layer was (blend NONE).
            (p.plane, OBJ_PLANE, "blend_op", 1),
            (p.crtc, OBJ_CRTC, "output_fence", &mut release[i] as *mut i32 as u64),
        ]);
        for &(name, v) in k.profile.votes {
            changes.push((p.crtc, OBJ_CRTC, name, v));
        }
    }
    FLIPS_LEFT.store(k.panels.len() as u64, Ordering::Release);
    set_pending(true);
    let flags = drm::ATOMIC_NONBLOCK | drm::ATOMIC_PAGE_FLIP_EVENT;
    let mut r = k.card.commit(&changes, flags);
    if r.as_ref().is_err_and(|e| e.raw_os_error() == Some(libc::EBUSY)) {
        // The last commit still on its way after all: once it is done, again.
        wait_idle(50);
        FLIPS_LEFT.store(k.panels.len() as u64, Ordering::Release);
        set_pending(true);
        r = k.card.commit(&changes, flags);
    }
    if let Err(e) = r {
        FLIPS_LEFT.store(0, Ordering::Release);
        set_pending(false);
        return Err(format!("the frame's commit: {e}"));
    }
    let merged = if release.len() == 2 { drm::merge_fences(release[0], release[1]) } else { release[0] };
    if release.len() == 2 {
        for f in release {
            if f >= 0 {
                // SAFETY: the kernel's fence fds, ours; merged already.
                unsafe { libc::close(f) };
            }
        }
    }
    Ok(merged)
}

/// A frame handed over by the window and not yet committed.
struct Job {
    buffer: *mut c_void,
    fb: u32,
    acquire: i32,
}

// SAFETY: the buffer is the window's (no thread's); the job is only reached
// under MAILBOX's mutex, then by the committer alone.
unsafe impl Send for Job {}

/// The next frame to commit: one only, the newest - a frame the last
/// commit kept waiting is passed over for the one after it.
static MAILBOX: (Mutex<Option<Job>>, Condvar) = (Mutex::new(None), Condvar::new());

fn hold(buffer: *mut c_void) {
    // SAFETY: the window's ANativeWindowBuffer, NativeBase first.
    unsafe {
        let base = buffer as *mut NativeBase;
        ((*base).inc_ref)(base);
    }
}

fn let_go(buffer: *mut c_void) {
    // SAFETY: as hold's; a buffer held before.
    unsafe {
        let base = buffer as *mut NativeBase;
        ((*base).dec_ref)(base);
    }
}

/// A job dropped: its fence closed, its buffer let go (never shown, it is
/// the GPU's again at once).
fn drop_job(job: Job) {
    if job.acquire >= 0 {
        // SAFETY: the window's fence, handed to us.
        unsafe { libc::close(job.acquire) };
    }
    let_go(job.buffer);
}

/// The committer's thread: the newest frame committed as soon as the last
/// commit's flips are done; the buffer it shows given its release fence and
/// kept until the next one shows.
fn committer() {
    loop {
        {
            let (lock, cv) = &MAILBOX;
            let mut g = lock.lock().unwrap();
            while g.is_none() {
                g = cv.wait(g).unwrap();
            }
        }
        wait_idle(100);
        let mut guard = KMS.lock().unwrap();
        let k = guard.as_mut().expect("kms output");
        let Some(job) = MAILBOX.0.lock().unwrap().take() else { continue };
        if !POWER_ON.load(Ordering::Relaxed) {
            drop_job(job);
            continue;
        }
        let t = now_ns();
        match commit(k, job.fb, job.acquire) {
            Ok(release) => {
                (k.win.set_fence)(job.buffer, release);
                if !k.last.is_null() {
                    let_go(k.last);
                }
                k.last = job.buffer;
                k.shown_fb = job.fb;
                if job.acquire >= 0 {
                    // SAFETY: the window's fence; the kernel took its own.
                    unsafe { libc::close(job.acquire) };
                }
            }
            Err(e) => {
                if k.stats.errors < 5 {
                    eprintln!("hybris-hwc: kms: {e}");
                }
                k.stats.errors += 1;
                drop_job(job);
            }
        }
        k.stats.commit_ns += now_ns() - t;
        k.stats.commits += 1;
    }
}

/// The window's present callback, for the own output: the frame handed to
/// the committer, nothing waited for (the first frame excepted, which sets
/// the modes).
pub(crate) extern "C" fn present(_: *mut c_void, _window: *mut c_void, buffer: *mut c_void) {
    let t_start = now_ns();
    let mut guard = KMS.lock().unwrap();
    let k = guard.as_mut().expect("kms output");
    crate::note_presented(buffer);
    let acquire = (k.win.get_fence)(buffer);
    // The window's fence is ours now; the buffer gets its release fence
    // from its commit (or none, passed over).
    (k.win.set_fence)(buffer, -1);
    let mut detail = PresentDetail { gpu_pending: crate::fence_pending(acquire), ..Default::default() };

    let t = now_ns();
    let fb = if POWER_ON.load(Ordering::Relaxed) { framebuffer(k, buffer) } else { Err(String::new()) };
    match fb {
        Ok(fb) if !k.started => {
            if let Err(e) = start(k, fb, acquire) {
                eprintln!("hybris-hwc: kms: {e}");
                k.stats.errors += 1;
            } else {
                hold(buffer);
                k.last = buffer;
                k.shown_fb = fb;
            }
            if acquire >= 0 {
                // SAFETY: the window's fence, waited for.
                unsafe { libc::close(acquire) };
            }
        }
        Ok(fb) => {
            hold(buffer);
            let (lock, cv) = &MAILBOX;
            let passed = lock.lock().unwrap().replace(Job { buffer, fb, acquire });
            cv.notify_one();
            if let Some(old) = passed {
                drop_job(old);
                k.stats.passed += 1;
            }
        }
        Err(e) => {
            if !e.is_empty() {
                if k.stats.errors < 5 {
                    eprintln!("hybris-hwc: kms: {e}");
                }
                k.stats.errors += 1;
            }
            if acquire >= 0 {
                // SAFETY: the window's fence, handed to us.
                unsafe { libc::close(acquire) };
            }
        }
    }
    detail.present_ns = now_ns() - t;

    let now = now_ns();
    crate::note_present(now, now - t_start);
    detail.since_last_ns = if k.last_ns != 0 { now - k.last_ns } else { 0 };
    crate::note_detail(detail);
    if k.last_ns != 0 {
        let ms = (now - k.last_ns) as f64 / 1e6;
        k.stats.frames += 1;
        k.stats.sum_ms += ms;
        k.stats.max_ms = k.stats.max_ms.max(ms);
        if ms > 20.0 {
            k.stats.long += 1;
        }
    }
    k.last_ns = now;
}

/// The panels on or off (each CRTC's ACTIVE: off, the panel and its light
/// go dark); frames are not shown while they are off.
pub(crate) fn set_power(on: bool) -> bool {
    let guard = KMS.lock().unwrap();
    let Some(k) = guard.as_ref() else { return false };
    wait_idle(100);
    POWER_ON.store(on, Ordering::Relaxed);
    if !k.started {
        return true;
    }
    let changes: Vec<(u32, u32, &str, u64)> = k.panels.iter().map(|p| (p.crtc, OBJ_CRTC, "ACTIVE", on as u64)).collect();
    let t = now_ns();
    match k.card.commit(&changes, drm::ATOMIC_ALLOW_MODESET) {
        Ok(()) => {
            println!("hybris-hwc: kms: the panels {} in {:.1} ms", if on { "on" } else { "off" }, (now_ns() - t) as f64 / 1e6);
            if on {
                FIRST_FLIP_AFTER_ON.store(now_ns(), Ordering::Relaxed);
            }
            true
        }
        Err(e) => {
            eprintln!("hybris-hwc: kms: the panels {}: {e}", if on { "on" } else { "off" });
            false
        }
    }
}

pub(crate) fn take_stats() -> crate::FrameStats {
    let mut guard = KMS.lock().unwrap();
    let st = std::mem::take(&mut guard.as_mut().expect("kms output").stats);
    let f = st.frames.max(1) as f64;
    if std::env::var_os("KMS_STATS").is_some() && st.commits > 0 {
        println!("hybris-hwc: kms: {} commits ({:.2} ms each), {} frames passed over", st.commits, st.commit_ns as f64 / 1e6 / st.commits as f64, st.passed);
    }
    crate::FrameStats { fast: 0, skipped: 0, frames: st.frames, over_20ms: st.long, mean_ms: st.sum_ms / f, max_ms: st.max_ms, errors: st.errors }
}
