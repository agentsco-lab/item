//! The panels through the kernel's DRM, by hand (no libdrm), as
//! item-recovery's screen drives them (itemgrid, crates/recovery/src/drm.rs),
//! but with the GPU's buffers: a gralloc buffer's dmabuf imported (PRIME),
//! made a framebuffer (ADDFB2, UBWC-compressed when gralloc made it so) and
//! flipped onto both panels' CRTCs, each its half.

use std::fs::File;
use std::os::fd::AsRawFd;

const fn ioc(dir: u32, nr: u32, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | (('d' as u32) << 8) | nr) as libc::c_ulong
}
const fn iowr(nr: u32, size: usize) -> libc::c_ulong {
    ioc(3, nr, size)
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
#[derive(Clone, Copy)]
pub struct ModeInfo {
    clock: u32,
    pub hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    pub vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    pub vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
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
struct PageFlip {
    crtc_id: u32,
    fb_id: u32,
    flags: u32,
    reserved: u32,
    user_data: u64,
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

const SET_MASTER: libc::c_ulong = ioc(0, 0x1e, 0);
const PRIME_FD_TO_HANDLE: libc::c_ulong = iowr(0x2e, std::mem::size_of::<PrimeHandle>());
const GETRESOURCES: libc::c_ulong = iowr(0xA0, std::mem::size_of::<CardRes>());
const SETCRTC: libc::c_ulong = iowr(0xA2, std::mem::size_of::<Crtc>());
const GETENCODER: libc::c_ulong = iowr(0xA6, std::mem::size_of::<GetEncoder>());
const GETCONNECTOR: libc::c_ulong = iowr(0xA7, std::mem::size_of::<GetConnector>());
const GETPROPERTY: libc::c_ulong = iowr(0xAA, std::mem::size_of::<GetProperty>());
const PAGE_FLIP: libc::c_ulong = iowr(0xB0, std::mem::size_of::<PageFlip>());
const ADDFB2: libc::c_ulong = iowr(0xB8, std::mem::size_of::<FbCmd2>());
const OBJ_GETPROPERTIES: libc::c_ulong = iowr(0xB9, std::mem::size_of::<ObjProps>());
const OBJ_SETPROPERTY: libc::c_ulong = iowr(0xBA, std::mem::size_of::<ObjSetProperty>());
const CREATEPROPBLOB: libc::c_ulong = iowr(0xBD, std::mem::size_of::<CreateBlob>());
const DESTROYPROPBLOB: libc::c_ulong = iowr(0xBE, 4);
const OBJ_CRTC: u32 = 0xcccccccc;

/// DRM_FORMAT_ABGR8888: gralloc's RGBA_8888, bytes R, G, B, A.
pub const ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
/// DRM_FORMAT_MOD_QCOM_COMPRESSED: fourcc_mod_code(QCOM, 1) - UBWC.
pub const MOD_QCOM_COMPRESSED: u64 = (5 << 56) | 1;
const FB_MODIFIERS: u32 = 2;
const PAGE_FLIP_EVENT: u32 = 1;

fn ioctl<T>(fd: i32, req: libc::c_ulong, arg: &mut T) -> std::io::Result<()> {
    // SAFETY: `arg` is the request's own struct, laid out as the kernel's.
    let r = unsafe { libc::ioctl(fd, req as _, arg as *mut T) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A panel: its connector, CRTC and mode, and where in the frame its half
/// begins.
pub struct Panel {
    pub connector: u32,
    pub crtc: u32,
    pub mode: ModeInfo,
    pub x: u32,
}

pub struct Card {
    file: File,
    pub panels: Vec<Panel>,
    /// The frame has been set on the CRTCs (SETCRTC) once: flips after.
    set: bool,
}

impl Card {
    /// The card as its master, its DSI panels (connected), each with a
    /// CRTC, left to right in the connectors' order.
    pub fn open() -> Result<Card, String> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open("/dev/dri/card0").map_err(|e| format!("/dev/dri/card0: {e}"))?;
        let fd = file.as_raw_fd();
        // SAFETY: SET_MASTER takes no argument.
        if unsafe { libc::ioctl(fd, SET_MASTER as _, 0) } < 0 {
            return Err(format!("not the card's master (is hwcomposer still up?): {}", std::io::Error::last_os_error()));
        }
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
        let mut panels: Vec<Panel> = Vec::new();
        let mut x = 0;
        for &id in &conns {
            let mut c = GetConnector { connector_id: id, ..Default::default() };
            ioctl(fd, GETCONNECTOR, &mut c).map_err(|e| e.to_string())?;
            // DRM_MODE_CONNECTOR_DSI, connected: the panels.
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
            let used: Vec<u32> = panels.iter().map(|p| p.crtc).collect();
            let mut crtc = 0;
            for &e in cencs.iter().chain([c.encoder_id].iter()).filter(|&&e| e != 0) {
                let mut enc = GetEncoder { encoder_id: e, ..Default::default() };
                if ioctl(fd, GETENCODER, &mut enc).is_err() {
                    continue;
                }
                if enc.crtc_id != 0 && !used.contains(&enc.crtc_id) {
                    crtc = enc.crtc_id;
                    break;
                }
                if let Some((_, &cid)) = crtcs.iter().enumerate().find(|(i, cid)| enc.possible_crtcs & (1 << i) != 0 && !used.contains(cid)) {
                    crtc = cid;
                    break;
                }
            }
            if crtc == 0 {
                return Err(format!("no CRTC for connector {id}"));
            }
            panels.push(Panel { connector: id, crtc, mode, x });
            x += mode.hdisplay as u32;
        }
        if panels.is_empty() {
            return Err("no panel connected".into());
        }
        Ok(Card { file, panels, set: false })
    }

    pub fn fd(&self) -> i32 {
        self.file.as_raw_fd()
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
        ioctl(self.fd(), ADDFB2, &mut fb).map_err(|e| format!("its framebuffer ({width}x{height}, pitch {pitch}, modifier {modifier:?}): {e}"))?;
        Ok(fb.fb_id)
    }

    /// `fb` on every panel, each its half: set on the CRTCs the first time
    /// (or always, `setcrtc`), flipped after - and waited for, the flips
    /// done. The time spent waiting, in ns.
    pub fn show(&mut self, fb: u32, setcrtc: bool) -> Result<u64, String> {
        let fd = self.fd();
        let t = crate::now_ns();
        if !self.set || setcrtc {
            for p in &self.panels {
                let mut conn = p.connector;
                let mut set = Crtc { set_connectors_ptr: &mut conn as *mut u32 as u64, count_connectors: 1, crtc_id: p.crtc, fb_id: fb, x: p.x, y: 0, gamma_size: 0, mode_valid: 1, mode: p.mode };
                ioctl(fd, SETCRTC, &mut set).map_err(|e| format!("setting crtc {}: {e}", p.crtc))?;
            }
            self.set = true;
            return Ok(crate::now_ns() - t);
        }
        for p in &self.panels {
            let mut flip = PageFlip { crtc_id: p.crtc, fb_id: fb, flags: PAGE_FLIP_EVENT, user_data: p.crtc as u64, ..Default::default() };
            ioctl(fd, PAGE_FLIP, &mut flip).map_err(|e| format!("flipping crtc {}: {e}", p.crtc))?;
        }
        // The flips' events (struct drm_event_vblank, type 2: a flip done).
        let mut done = 0;
        let mut buf = [0u8; 256];
        while done < self.panels.len() {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd, ours.
            if unsafe { libc::poll(&mut pfd, 1, 500) } <= 0 {
                return Err(format!("no flip done in 500 ms ({done} of {})", self.panels.len()));
            }
            // SAFETY: reading into our buffer, its length.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            let mut at = 0usize;
            while n > 0 && at + 8 <= n as usize {
                let ty = u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap());
                let len = u32::from_ne_bytes(buf[at + 4..at + 8].try_into().unwrap()) as usize;
                if ty == 2 {
                    done += 1;
                }
                at += len.max(8);
            }
        }
        Ok(crate::now_ns() - t)
    }

    /// A property of `crtc` by its name: its value now.
    pub fn get(&self, crtc: u32, name: &str) -> Result<u64, String> {
        let fd = self.fd();
        let prop = prop_id(fd, crtc, OBJ_CRTC, name)?;
        let mut op = ObjProps { obj_id: crtc, obj_type: OBJ_CRTC, ..Default::default() };
        ioctl(fd, OBJ_GETPROPERTIES, &mut op).map_err(|e| e.to_string())?;
        let mut props = vec![0u32; op.count_props as usize];
        let mut values = vec![0u64; op.count_props as usize];
        op.props_ptr = props.as_mut_ptr() as u64;
        op.values_ptr = values.as_mut_ptr() as u64;
        ioctl(fd, OBJ_GETPROPERTIES, &mut op).map_err(|e| e.to_string())?;
        props.iter().zip(&values).find(|(p, _)| **p == prop).map(|(_, v)| *v).ok_or_else(|| format!("{name}: no value"))
    }

    /// A property of `crtc` by its name set to `value`.
    pub fn set(&self, crtc: u32, name: &str, value: u64) -> Result<(), String> {
        let fd = self.fd();
        let prop = prop_id(fd, crtc, OBJ_CRTC, name)?;
        let mut set = ObjSetProperty { value, prop_id: prop, obj_id: crtc, obj_type: OBJ_CRTC };
        ioctl(fd, OBJ_SETPROPERTY, &mut set).map_err(|e| format!("{name} on crtc {crtc}: {e}"))
    }

    /// A blob property of `crtc` by its name (SDE_DSPP_PCC_V4...) set to
    /// `data`, or cleared with none.
    pub fn set_blob(&self, crtc: u32, name: &str, data: Option<&[u8]>) -> Result<(), String> {
        let fd = self.fd();
        let prop = prop_id(fd, crtc, OBJ_CRTC, name)?;
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
}

fn prop_id(fd: i32, obj: u32, ty: u32, want: &str) -> Result<u32, String> {
    let mut op = ObjProps { obj_id: obj, obj_type: ty, ..Default::default() };
    ioctl(fd, OBJ_GETPROPERTIES, &mut op).map_err(|e| e.to_string())?;
    let mut props = vec![0u32; op.count_props as usize];
    let mut values = vec![0u64; op.count_props as usize];
    op.props_ptr = props.as_mut_ptr() as u64;
    op.values_ptr = values.as_mut_ptr() as u64;
    ioctl(fd, OBJ_GETPROPERTIES, &mut op).map_err(|e| e.to_string())?;
    for p in props {
        let mut gp = GetProperty { values_ptr: 0, enum_blob_ptr: 0, prop_id: p, flags: 0, name: [0; 32], count_values: 0, count_enum_blobs: 0 };
        if ioctl(fd, GETPROPERTY, &mut gp).is_ok() && String::from_utf8_lossy(&gp.name).trim_end_matches('\0') == want {
            return Ok(p);
        }
    }
    Err(format!("no {want} on object {obj}"))
}
