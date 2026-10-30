//! The kernel's DRM by hand (no libdrm), what item's own display output
//! needs of it: the card's connectors, CRTCs and planes, properties by
//! name, gralloc buffers as framebuffers (PRIME, ADDFB2), the first mode
//! set (SETCRTC), atomic commits after, vblank and flip events.
//! As item-recovery's screen (itemgrid, crates/recovery/src/drm.rs) and
//! probes/kms-frame, where each step was first seen to work on the Duo.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;

const fn ioc(dir: u32, ty: u8, nr: u32, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((ty as u32) << 8) | nr) as libc::c_ulong
}
const fn iowr(nr: u32, size: usize) -> libc::c_ulong {
    ioc(3, b'd', nr, size)
}

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    pub htotal: u16,
    hskew: u16,
    pub vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    pub vtotal: u16,
    vscan: u16,
    pub vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
}

impl ModeInfo {
    /// The time between frames, from the mode's refresh rate (a
    /// command-mode panel's timings are not its own: the Duo's add up to
    /// 45.6 ms a frame at 60 Hz); from the timings only without one.
    pub fn period_ns(&self) -> u64 {
        let pixels = self.htotal as u64 * self.vtotal as u64;
        if self.vrefresh > 0 {
            1_000_000_000 / self.vrefresh as u64
        } else if self.clock > 0 && pixels > 0 {
            pixels * 1_000_000 / self.clock as u64
        } else {
            16_666_667
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct GetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
struct Crtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
#[derive(Default)]
struct PlaneRes {
    planes_ptr: u64,
    count_planes: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetPlane {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    possible_crtcs: u32,
    gamma_size: u32,
    count_format_types: u32,
    format_type_ptr: u64,
}

#[repr(C)]
#[derive(Default)]
struct PrimeHandle {
    handle: u32,
    flags: u32,
    fd: i32,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Default)]
struct CreateBlob {
    data: u64,
    length: u32,
    blob_id: u32,
}

#[repr(C)]
#[derive(Default)]
struct ObjSetProperty {
    value: u64,
    prop_id: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
#[derive(Default)]
struct ObjProps {
    props_ptr: u64,
    values_ptr: u64,
    count_props: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
struct GetProperty {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Default)]
struct Atomic {
    flags: u32,
    count_objs: u32,
    objs_ptr: u64,
    count_props_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    reserved: u64,
    user_data: u64,
}

#[repr(C)]
#[derive(Default)]
struct ClientCap {
    capability: u64,
    value: u64,
}

#[repr(C)]
#[derive(Default)]
struct WaitVblank {
    kind: u32,
    sequence: u32,
    signal: u64,
}

#[repr(C)]
struct SyncMerge {
    name: [u8; 32],
    fd2: i32,
    fence: i32,
    flags: u32,
    pad: u32,
}

const SET_MASTER: libc::c_ulong = ioc(0, b'd', 0x1e, 0);
const SET_CLIENT_CAP: libc::c_ulong = ioc(1, b'd', 0x0d, std::mem::size_of::<ClientCap>());
const WAIT_VBLANK: libc::c_ulong = iowr(0x3a, std::mem::size_of::<WaitVblank>());
const PRIME_FD_TO_HANDLE: libc::c_ulong = iowr(0x2e, std::mem::size_of::<PrimeHandle>());
const GETRESOURCES: libc::c_ulong = iowr(0xA0, std::mem::size_of::<CardRes>());
const SETCRTC: libc::c_ulong = iowr(0xA2, std::mem::size_of::<Crtc>());
const GETENCODER: libc::c_ulong = iowr(0xA6, std::mem::size_of::<GetEncoder>());
const GETCONNECTOR: libc::c_ulong = iowr(0xA7, std::mem::size_of::<GetConnector>());
const GETPROPERTY: libc::c_ulong = iowr(0xAA, std::mem::size_of::<GetProperty>());
const GETPLANERESOURCES: libc::c_ulong = iowr(0xB5, std::mem::size_of::<PlaneRes>());
const GETPLANE: libc::c_ulong = iowr(0xB6, std::mem::size_of::<GetPlane>());
const ADDFB2: libc::c_ulong = iowr(0xB8, std::mem::size_of::<FbCmd2>());
const OBJ_GETPROPERTIES: libc::c_ulong = iowr(0xB9, std::mem::size_of::<ObjProps>());
const OBJ_SETPROPERTY: libc::c_ulong = iowr(0xBA, std::mem::size_of::<ObjSetProperty>());
const ATOMIC: libc::c_ulong = iowr(0xBC, std::mem::size_of::<Atomic>());
const CREATEPROPBLOB: libc::c_ulong = iowr(0xBD, std::mem::size_of::<CreateBlob>());
const DESTROYPROPBLOB: libc::c_ulong = iowr(0xBE, 4);
const SYNC_IOC_MERGE: libc::c_ulong = ioc(3, b'>', 3, std::mem::size_of::<SyncMerge>());

pub const OBJ_CRTC: u32 = 0xcccccccc;
pub const OBJ_PLANE: u32 = 0xeeeeeeee;

/// DRM_FORMAT_ABGR8888: gralloc's RGBA_8888, bytes R, G, B, A.
pub const ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
/// DRM_FORMAT_MOD_QCOM_COMPRESSED: fourcc_mod_code(QCOM, 1) - UBWC.
pub const MOD_QCOM_COMPRESSED: u64 = (5 << 56) | 1;
const FB_MODIFIERS: u32 = 2;

pub const ATOMIC_PAGE_FLIP_EVENT: u32 = 0x01;
pub const ATOMIC_TEST_ONLY: u32 = 0x0100;
pub const ATOMIC_NONBLOCK: u32 = 0x0200;
pub const ATOMIC_ALLOW_MODESET: u32 = 0x0400;

/// A DRM event: a vblank (1) or a flip done (2), and its time.
pub struct Event {
    pub kind: u32,
    pub time_ns: u64,
}

pub fn ioctl<T>(fd: i32, req: libc::c_ulong, arg: &mut T) -> std::io::Result<()> {
    loop {
        // SAFETY: `arg` is the request's own struct, laid out as the kernel's.
        let r = unsafe { libc::ioctl(fd, req as _, arg as *mut T) };
        if r >= 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// A connected DSI connector, its mode and the CRTC it is given.
pub struct Connector {
    pub id: u32,
    pub mode: ModeInfo,
    pub crtc: u32,
    /// The CRTC's place among the card's (for vblank requests).
    pub crtc_index: u32,
}

pub struct Card {
    file: File,
    /// Property ids by object and name.
    props: std::sync::Mutex<HashMap<(u32, String), u32>>,
}

impl Card {
    /// The card, as its master if it can be (the first to open it is, root
    /// or not), with atomic commits and all planes listed.
    pub fn open() -> Result<Card, String> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open("/dev/dri/card0").map_err(|e| format!("/dev/dri/card0: {e}"))?;
        let fd = file.as_raw_fd();
        // SAFETY: SET_MASTER takes no argument. Refused to a user before
        // 5.8 even when it is the master already: only noted.
        if unsafe { libc::ioctl(fd, SET_MASTER as _, 0) } < 0 {
            eprintln!("hybris-hwc: kms: SET_MASTER: {} (the first to open the card is its master anyway)", std::io::Error::last_os_error());
        }
        for cap in [2u64, 3] {
            ioctl(fd, SET_CLIENT_CAP, &mut ClientCap { capability: cap, value: 1 }).map_err(|e| format!("client cap {cap}: {e}"))?;
        }
        Ok(Card { file, props: Default::default() })
    }

    pub fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    /// The connected DSI connectors (the panels), in the card's order, each
    /// with its preferred mode and a CRTC of its own.
    pub fn panels(&self) -> Result<Vec<Connector>, String> {
        let fd = self.fd();
        let mut res = CardRes::default();
        ioctl(fd, GETRESOURCES, &mut res).map_err(|e| format!("resources: {e}"))?;
        let mut crtcs = vec![0u32; res.count_crtcs as usize];
        let mut conns = vec![0u32; res.count_connectors as usize];
        let mut encs = vec![0u32; res.count_encoders as usize];
        let mut fbs = vec![0u32; res.count_fbs as usize];
        res.crtc_id_ptr = crtcs.as_mut_ptr() as u64;
        res.connector_id_ptr = conns.as_mut_ptr() as u64;
        res.encoder_id_ptr = encs.as_mut_ptr() as u64;
        res.fb_id_ptr = fbs.as_mut_ptr() as u64;
        ioctl(fd, GETRESOURCES, &mut res).map_err(|e| format!("resources: {e}"))?;
        let mut out: Vec<Connector> = Vec::new();
        for &id in &conns {
            let mut c = GetConnector { connector_id: id, ..Default::default() };
            ioctl(fd, GETCONNECTOR, &mut c).map_err(|e| e.to_string())?;
            // DRM_MODE_CONNECTOR_DSI, connected.
            if c.connection != 1 || c.count_modes == 0 || c.connector_type != 16 {
                continue;
            }
            // SAFETY: zeroed ModeInfo is a valid value of plain integers.
            let mut modes = vec![unsafe { std::mem::zeroed::<ModeInfo>() }; c.count_modes as usize];
            let mut cencs = vec![0u32; c.count_encoders as usize];
            let mut props = vec![0u32; c.count_props as usize];
            let mut values = vec![0u64; c.count_props as usize];
            c.modes_ptr = modes.as_mut_ptr() as u64;
            c.encoders_ptr = cencs.as_mut_ptr() as u64;
            c.props_ptr = props.as_mut_ptr() as u64;
            c.prop_values_ptr = values.as_mut_ptr() as u64;
            ioctl(fd, GETCONNECTOR, &mut c).map_err(|e| e.to_string())?;
            let mode = *modes.iter().find(|m| m.kind & 8 != 0).unwrap_or(&modes[0]);
            let used: Vec<u32> = out.iter().map(|p| p.crtc).collect();
            let mut crtc = None;
            for &e in cencs.iter().chain([c.encoder_id].iter()).filter(|&&e| e != 0) {
                let mut enc = GetEncoder { encoder_id: e, ..Default::default() };
                if ioctl(fd, GETENCODER, &mut enc).is_err() {
                    continue;
                }
                if enc.crtc_id != 0 && !used.contains(&enc.crtc_id) {
                    crtc = crtcs.iter().position(|&x| x == enc.crtc_id).map(|i| (enc.crtc_id, i as u32));
                    break;
                }
                if let Some((i, &cid)) = crtcs.iter().enumerate().find(|(i, cid)| enc.possible_crtcs & (1 << i) != 0 && !used.contains(cid)) {
                    crtc = Some((cid, i as u32));
                    break;
                }
            }
            let (crtc, crtc_index) = crtc.ok_or_else(|| format!("no CRTC for connector {id}"))?;
            out.push(Connector { id, mode, crtc, crtc_index });
        }
        Ok(out)
    }

    /// The plane showing on `crtc` now (after its mode is set).
    pub fn plane_on(&self, crtc: u32) -> Result<u32, String> {
        let fd = self.fd();
        let mut pr = PlaneRes::default();
        ioctl(fd, GETPLANERESOURCES, &mut pr).map_err(|e| format!("planes: {e}"))?;
        let mut planes = vec![0u32; pr.count_planes as usize];
        pr.planes_ptr = planes.as_mut_ptr() as u64;
        ioctl(fd, GETPLANERESOURCES, &mut pr).map_err(|e| format!("planes: {e}"))?;
        for id in planes {
            let mut gp = GetPlane { plane_id: id, ..Default::default() };
            if ioctl(fd, GETPLANE, &mut gp).is_ok() && gp.crtc_id == crtc {
                return Ok(id);
            }
        }
        Err(format!("no plane on crtc {crtc}"))
    }

    /// A dmabuf (`fd`) of `width` x `height`, `pitch` bytes a row, as a
    /// framebuffer: plain, or UBWC-compressed (`modifier`).
    pub fn framebuffer(&self, fd: i32, width: u32, height: u32, pitch: u32, modifier: Option<u64>) -> Result<u32, String> {
        let mut ph = PrimeHandle { fd, ..Default::default() };
        ioctl(self.fd(), PRIME_FD_TO_HANDLE, &mut ph).map_err(|e| format!("importing the dmabuf: {e}"))?;
        let mut fb = FbCmd2 { width, height, pixel_format: ABGR8888, ..Default::default() };
        fb.handles[0] = ph.handle;
        fb.pitches[0] = pitch;
        if let Some(m) = modifier {
            fb.flags = FB_MODIFIERS;
            fb.modifier[0] = m;
        }
        ioctl(self.fd(), ADDFB2, &mut fb).map_err(|e| format!("a framebuffer ({width}x{height}, pitch {pitch}, modifier {modifier:?}): {e}"))?;
        Ok(fb.fb_id)
    }

    /// `fb` from `x` on `crtc` driving `connector` in `mode`: the legacy
    /// way, blocking - the first frame's, which sets the mode.
    pub fn set_crtc(&self, crtc: u32, connector: u32, mode: ModeInfo, fb: u32, x: u32) -> Result<(), String> {
        let mut conn = connector;
        let mut set = Crtc { set_connectors_ptr: &mut conn as *mut u32 as u64, count_connectors: 1, crtc_id: crtc, fb_id: fb, x, y: 0, gamma_size: 0, mode_valid: 1, mode };
        ioctl(self.fd(), SETCRTC, &mut set).map_err(|e| format!("setting crtc {crtc}: {e}"))
    }

    /// `crtc` off the legacy way: no mode, no connector, no framebuffer -
    /// the next set_crtc sets the mode afresh.
    pub fn crtc_off(&self, crtc: u32) -> Result<(), String> {
        let mut set = Crtc { set_connectors_ptr: 0, count_connectors: 0, crtc_id: crtc, fb_id: 0, x: 0, y: 0, gamma_size: 0, mode_valid: 0, mode: ModeInfo::default() };
        ioctl(self.fd(), SETCRTC, &mut set).map_err(|e| format!("crtc {crtc} off: {e}"))
    }

    /// A property's id on an object, by its name (looked up once).
    pub fn prop(&self, obj: u32, ty: u32, name: &str) -> Result<u32, String> {
        let key = (obj, name.to_owned());
        if let Some(&p) = self.props.lock().unwrap().get(&key) {
            return Ok(p);
        }
        let fd = self.fd();
        let mut op = ObjProps { obj_id: obj, obj_type: ty, ..Default::default() };
        ioctl(fd, OBJ_GETPROPERTIES, &mut op).map_err(|e| e.to_string())?;
        let mut props = vec![0u32; op.count_props as usize];
        let mut values = vec![0u64; op.count_props as usize];
        op.props_ptr = props.as_mut_ptr() as u64;
        op.values_ptr = values.as_mut_ptr() as u64;
        ioctl(fd, OBJ_GETPROPERTIES, &mut op).map_err(|e| e.to_string())?;
        let mut cache = self.props.lock().unwrap();
        let mut found = None;
        for p in props {
            let mut gp = GetProperty { values_ptr: 0, enum_blob_ptr: 0, prop_id: p, flags: 0, name: [0; 32], count_values: 0, count_enum_blobs: 0 };
            if ioctl(fd, GETPROPERTY, &mut gp).is_ok() {
                let n = String::from_utf8_lossy(&gp.name).trim_end_matches('\0').to_owned();
                if n == name {
                    found = Some(p);
                }
                cache.insert((obj, n), p);
            }
        }
        found.ok_or_else(|| format!("no {name} on object {obj}"))
    }

    /// A blob property of `crtc` set to `data`, or cleared with none.
    pub fn set_blob(&self, crtc: u32, name: &str, data: Option<&[u8]>) -> Result<(), String> {
        let fd = self.fd();
        let prop = self.prop(crtc, OBJ_CRTC, name)?;
        let mut blob = CreateBlob::default();
        if let Some(d) = data {
            blob = CreateBlob { data: d.as_ptr() as u64, length: d.len() as u32, blob_id: 0 };
            ioctl(fd, CREATEPROPBLOB, &mut blob).map_err(|e| format!("{name}'s blob: {e}"))?;
        }
        let mut set = ObjSetProperty { value: blob.blob_id as u64, prop_id: prop, obj_id: crtc, obj_type: OBJ_CRTC };
        let r = ioctl(fd, OBJ_SETPROPERTY, &mut set).map_err(|e| format!("{name} on crtc {crtc}: {e}"));
        if blob.blob_id != 0 {
            let _ = ioctl(fd, DESTROYPROPBLOB, &mut blob.blob_id);
        }
        r
    }

    /// One atomic commit of `changes` (object, its type, property name,
    /// value), with `flags`.
    pub fn commit(&self, changes: &[(u32, u32, &str, u64)], flags: u32) -> std::io::Result<()> {
        let mut objs: Vec<u32> = Vec::new();
        let mut counts: Vec<u32> = Vec::new();
        let mut props: Vec<u32> = Vec::new();
        let mut values: Vec<u64> = Vec::new();
        // Grouped by object, in the order the objects first come.
        let mut order: Vec<u32> = Vec::new();
        for c in changes {
            if !order.contains(&c.0) {
                order.push(c.0);
            }
        }
        for obj in order {
            let mut n = 0;
            for &(o, ty, name, v) in changes.iter().filter(|c| c.0 == obj) {
                let p = self.prop(o, ty, name).map_err(std::io::Error::other)?;
                props.push(p);
                values.push(v);
                n += 1;
            }
            objs.push(obj);
            counts.push(n);
        }
        let mut a = Atomic {
            flags,
            count_objs: objs.len() as u32,
            objs_ptr: objs.as_ptr() as u64,
            count_props_ptr: counts.as_ptr() as u64,
            props_ptr: props.as_ptr() as u64,
            prop_values_ptr: values.as_ptr() as u64,
            ..Default::default()
        };
        ioctl(self.fd(), ATOMIC, &mut a)
    }

    /// An event at the next vblank of the CRTC at `index` (relative 1).
    pub fn request_vblank(&self, index: u32, user_data: u64) -> std::io::Result<()> {
        // _DRM_VBLANK_RELATIVE | _DRM_VBLANK_EVENT, the CRTC's index in
        // the high bits (_DRM_VBLANK_HIGH_CRTC_SHIFT 1) past the first two.
        let kind = 0x1 | 0x0400_0000 | if index == 1 { 0x2000_0000 } else if index > 1 { (index << 1) & 0x3e } else { 0 };
        let mut w = WaitVblank { kind, sequence: 1, signal: user_data };
        ioctl(self.fd(), WAIT_VBLANK, &mut w)
    }

    /// The events the card has now (waiting up to `timeout_ms`).
    pub fn events(&self, timeout_ms: i32) -> Vec<Event> {
        let fd = self.fd();
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, ours.
        if unsafe { libc::poll(&mut pfd, 1, timeout_ms) } <= 0 {
            return Vec::new();
        }
        let mut buf = [0u8; 1024];
        // SAFETY: reading into our buffer, its length.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        // The events' times are CLOCK_BOOTTIME's on the Duo's kernel
        // (ahead of CLOCK_MONOTONIC by the time the phone has slept: 254 s
        // on 2026-10-29, and every vsync seemed that old to the pacing):
        // brought to CLOCK_MONOTONIC, the clock of everything else here.
        let clock = |id| {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // SAFETY: a timespec of ours.
            unsafe { libc::clock_gettime(id, &mut ts) };
            ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
        };
        let slept = clock(libc::CLOCK_BOOTTIME).saturating_sub(clock(libc::CLOCK_MONOTONIC));
        let mut out = Vec::new();
        let mut at = 0usize;
        let u32_at = |i: usize| u32::from_ne_bytes(buf[i..i + 4].try_into().unwrap());
        while n > 0 && at + 8 <= n as usize {
            let (kind, len) = (u32_at(at), u32_at(at + 4) as usize);
            // struct drm_event_vblank: base, user_data (u64), tv_sec,
            // tv_usec, sequence, crtc_id.
            if (kind == 1 || kind == 2) && len >= 32 && at + 32 <= n as usize {
                let time_ns = (u32_at(at + 16) as u64 * 1_000_000_000 + u32_at(at + 20) as u64 * 1000).saturating_sub(slept);
                out.push(Event { kind, time_ns });
            }
            at += len.max(8);
        }
        out
    }
}

/// Two sync fences as one that signals when both have (`a` and `b` stay
/// the caller's to close).
pub fn merge_fences(a: i32, b: i32) -> i32 {
    if a < 0 || b < 0 {
        // SAFETY: dup of an fd we were given, or -1 passed on.
        return unsafe { libc::dup(a.max(b)) };
    }
    let mut m = SyncMerge { name: [0; 32], fd2: b, fence: -1, flags: 0, pad: 0 };
    m.name[..10].copy_from_slice(b"item-panel");
    match ioctl(a, SYNC_IOC_MERGE, &mut m) {
        Ok(()) => m.fence,
        // SAFETY: as above.
        Err(_) => unsafe { libc::dup(a) },
    }
}
