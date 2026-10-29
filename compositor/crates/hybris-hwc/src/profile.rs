//! Which device item runs on, and what its own display output is to know
//! of it: each Duo its panels' order and gap, its colour calibration and
//! hardware, its display's votes for clock and bandwidth. Found from the
//! device tree at start; a device not known here keeps hwcomposer.

/// A device's display, as the own output drives it.
#[derive(Debug, Clone)]
pub struct Profile {
    pub name: &'static str,
    /// The frame's columns between the panels (the hinge): the frame stays
    /// as wide as hwcomposer's was, so nothing above the output changes.
    pub gap: u32,
    /// The calibration files' panel names, left panel first.
    pub calib: [&'static str; 2],
    /// The colour hardware's properties on a CRTC: into linear light, the
    /// matrix, back out.
    pub igc: &'static str,
    pub pcc: &'static str,
    pub gc: &'static str,
    /// The CRTCs' votes, each: (property, value). hwcomposer's own in
    /// normal running; what it leaves when it stops (the bus's maximum on
    /// each) refuses every commit.
    pub votes: &'static [(&'static str, u64)],
}

/// The Surface Duo (SM8150, msm 4.14): two 1350x1800 panels at 60 Hz, the
/// hinge 84 columns (vendor.display.bezel_size); R2's calibration the
/// left panel's (seen so on the owner's Duo, 2026-10-29); hwcomposer's
/// votes read under item the same day.
const DUO: Profile = Profile {
    name: "Surface Duo",
    gap: 84,
    calib: ["r2", "c3"],
    igc: "SDE_DSPP_IGC_V3",
    pcc: "SDE_DSPP_PCC_V4",
    gc: "SDE_DSPP_GC_V1",
    votes: &[
        ("core_clk", 187_920_000),
        ("core_ab", 1_164_041_856),
        ("core_ib", 2_500_000_000),
        ("llcc_ab", 1_164_041_856),
        ("llcc_ib", 0),
        ("dram_ab", 1_164_041_856),
        ("dram_ib", 1_600_000_000),
    ],
};

/// The device's profile from the device tree's model and compatible
/// ("Surface Duo DV_A", qcom,sm8150 on the owner's), or none. The Surface
/// Duo 2 (SM8350) is not yet here: its panels, rates and votes are to be
/// read off it first.
pub fn detect() -> Option<Profile> {
    let read = |p: &str| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).replace('\0', " ")).unwrap_or_default();
    let model = read("/proc/device-tree/model");
    let compatible = read("/proc/device-tree/compatible");
    if model.starts_with("Surface Duo") && !model.starts_with("Surface Duo 2") && compatible.contains("qcom,sm8150") {
        return Some(DUO);
    }
    None
}
