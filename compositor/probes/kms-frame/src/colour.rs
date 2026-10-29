//! The panels' factory calibration (the sfpd partition's Qualcomm display
//! calibration files), as item-recovery reads and applies it (itemgrid,
//! crates/recovery/src/colour.rs, where it is told in full): into linear
//! light (IGC), a matrix (PCC), back out (GC) - a copy for the probe.

/// A mode of one panel's calibration: the way into linear light (3
/// channels of 256 points, 12 bits), the matrix (3 rows of c, r, g, b, rg,
/// gb, rb, rgb, rr, gg, bb) and the gamma (3 channels of 1024 points, 10
/// bits), as the file has them.
#[derive(Clone, Debug)]
pub struct Calib {
    pub igc: [Vec<u32>; 3],
    pub pcc: [[f64; 11]; 3],
    pub gc: [Vec<u32>; 3],
}

/// The calibration of the panel `which` ("c3", "r2") in `mode` ("srgb",
/// "vivid"), from the sfpd partition as the Android side mounts it.
pub fn load(which: &str, mode: &str) -> Result<Calib, String> {
    let dir = "/android/vendor/sfpd/display";
    let file = std::fs::read_dir(dir)
        .map_err(|e| format!("{dir}: {e}"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).is_some_and(|n| n.starts_with("disp_user_calib_data") && n.ends_with(&format!("_{which}.xml"))))
        .ok_or_else(|| format!("no calibration for panel {which} in {dir}"))?;
    let xml = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    parse(&xml, mode)
}

/// A mode (by its name, any case) of a calibration file.
pub fn parse(xml: &str, mode: &str) -> Result<Calib, String> {
    let mut rest = xml;
    while let Some(i) = rest.find("<Mode ") {
        let block_end = rest[i..].find("</Mode>").map(|j| i + j).ok_or("a mode not closed")?;
        let block = &rest[i..block_end];
        rest = &rest[block_end..];
        let name = attr(block, "Name").unwrap_or_default();
        if !name.eq_ignore_ascii_case(mode) {
            continue;
        }
        let feature = |ty: &str| -> Option<Vec<u8>> {
            let at = block.find(&format!("FeatureType=\"{ty}\""))?;
            let start = block[at..].find('>')? + at + 1;
            let end = block[start..].find('<')? + start;
            hexes(block[start..end].trim())
        };
        let pcc_raw = feature("2").ok_or("no colour matrix (PCC) in the mode")?;
        if pcc_raw.len() < 8 + 33 * 8 {
            return Err("the colour matrix is short".into());
        }
        let mut pcc = [[0f64; 11]; 3];
        for (k, v) in pcc.iter_mut().flatten().enumerate() {
            *v = f64::from_le_bytes(pcc_raw[8 + 8 * k..16 + 8 * k].try_into().unwrap());
        }
        // The gamma: 12 bytes of header, then three channels of 1024.
        let gc_raw = feature("8").ok_or("no gamma (GC) in the mode")?;
        if gc_raw.len() < 12 + 3 * 1024 * 4 {
            return Err("the gamma is short".into());
        }
        let ch = |raw: &[u8], c: usize, n: usize| (0..n).map(|i| u32::from_le_bytes(raw[12 + 4 * (c * 1024 + i)..16 + 4 * (c * 1024 + i)].try_into().unwrap())).collect::<Vec<_>>();
        // Into linear light: the same 12 bytes and three slots of 1024, of
        // which the first 256 hold the table (its header says 256).
        let igc_raw = feature("7").ok_or("no way into linear light (IGC) in the mode")?;
        if igc_raw.len() < 12 + 3 * 1024 * 4 {
            return Err("the way into linear light is short".into());
        }
        let igc = [ch(&igc_raw, 0, 256), ch(&igc_raw, 1, 256), ch(&igc_raw, 2, 256)];
        return Ok(Calib { igc, pcc, gc: [ch(&gc_raw, 0, 1024), ch(&gc_raw, 1, 1024), ch(&gc_raw, 2, 1024)] });
    }
    Err(format!("no mode {mode}"))
}

fn attr(block: &str, name: &str) -> Option<String> {
    let at = block.find(&format!(" {name}=\""))? + name.len() + 3;
    let end = block[at..].find('"')? + at;
    Some(block[at..end].to_owned())
}

fn hexes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// The matrix as struct drm_msm_pcc: flags (u64), then r, g, b each c, r,
/// g, b, rg, gb, rb, rgb, then r_rr r_gg r_bb g_rr ... b_bb - each a
/// signed fixed point with 15 bits of fraction (1.0 is 0x8000).
pub fn pcc_blob(c: &Calib) -> Vec<u8> {
    let fx = |v: f64| ((v * 32768.0).round() as i32) as u32;
    let mut out = Vec::with_capacity(8 + 4 * 33);
    out.extend_from_slice(&0u64.to_le_bytes());
    for row in &c.pcc {
        for v in &row[..8] {
            out.extend_from_slice(&fx(*v).to_le_bytes());
        }
    }
    for row in &c.pcc {
        for v in &row[8..11] {
            out.extend_from_slice(&fx(*v).to_le_bytes());
        }
    }
    // The struct padded to its u64's alignment: 144 bytes, the kernel
    // checks the length to the byte (140 was refused).
    out.resize(out.len().next_multiple_of(8), 0);
    out
}

/// Which of r, g, b `order` makes c0, c1, c2.
fn channels(order: &str) -> Vec<usize> {
    order
        .chars()
        .take(3)
        .map(|ch| match ch {
            'r' => 0,
            'g' => 1,
            _ => 2,
        })
        .collect()
}

/// The way into linear light as struct drm_msm_igc_lut: flags (u64), then
/// c0, c1, c2 of 256 points (12 bits), the channels in `order`, a strength
/// (u32, unused) and each channel's end point (c0_last..c2_last: full) -
/// 3096 bytes (without the ends, 3088, was refused).
pub fn igc_blob(c: &Calib, order: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 3 * 256 * 4 + 16);
    out.extend_from_slice(&0u64.to_le_bytes());
    for ch in channels(order) {
        for v in &c.igc[ch] {
            out.extend_from_slice(&(v & 0xfff).to_le_bytes());
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..3 {
        out.extend_from_slice(&0xfffu32.to_le_bytes());
    }
    out
}

/// The gamma as struct drm_msm_pgc_lut: flags (u64), then c0, c1, c2 of
/// 512 words, two 10-bit points a word (the even in the low bits, the odd
/// from bit 16), the channels in `order` (which of r, g, b is c0, c1, c2).
pub fn gc_blob(c: &Calib, order: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 3 * 512 * 4);
    out.extend_from_slice(&0u64.to_le_bytes());
    for ch in channels(order) {
        let v = &c.gc[ch];
        for i in 0..512 {
            let w = (v[2 * i] & 0x3ff) | ((v[2 * i + 1] & 0x3ff) << 16);
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
    out
}
