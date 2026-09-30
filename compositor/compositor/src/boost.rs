//! The GPU's clock while something moves, as Android's input boost.
//!
//! The Adreno 640's governor (`msm-adreno-tz`) leaves the GPU at its lowest
//! clock, 257 of 585 MHz, while it is half busy: every frame of a client
//! and of ours is drawn slowly, and a GTK app scrolled at 43 fps. With the
//! minimum raised to the top it scrolled at 60 (log/2026-09-30-step-7-clocks.md).
//! The compositor sees every touch and every animation of the shell first:
//! on either, the GPU's minimum clock goes to its maximum, and back 1.5 s
//! after the last (a kinetic scroll outlasts the finger).
//!
//! It writes `/sys/class/kgsl/kgsl-3d0/devfreq/min_freq`, which is root's:
//! `tools/session-run.sh` hands it to the session's user for a run and back
//! after. Not writable, or `GPU_BOOST=0`: no boost.

use std::io::Write;

const DEVFREQ: &str = "/sys/class/kgsl/kgsl-3d0/devfreq";
/// How long the clock stays up after the last touch or motion.
const HOLD_NS: u64 = 1_500_000_000;

pub struct GpuBoost {
    file: Option<std::fs::File>,
    /// The clocks written: the top, and the minimum found at start.
    max: String,
    floor: String,
    boosted: bool,
    until_ns: u64,
    /// Time spent boosted, for the report (ns).
    pub boosted_ns: u64,
    since_ns: u64,
}

impl GpuBoost {
    pub fn new() -> GpuBoost {
        let read = |f: &str| std::fs::read_to_string(format!("{DEVFREQ}/{f}")).map(|s| s.trim().to_owned()).unwrap_or_default();
        let (max, floor) = (read("max_freq"), read("min_freq"));
        let enabled = std::env::var("GPU_BOOST").map(|v| v != "0").unwrap_or(true);
        let file = if enabled {
            match std::fs::OpenOptions::new().write(true).open(format!("{DEVFREQ}/min_freq")) {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!("GPU boost: {DEVFREQ}/min_freq: {e}; no boost");
                    None
                }
            }
        } else {
            None
        };
        if file.is_some() {
            tracing::info!("GPU boost: min {floor} -> {max} Hz while touched or moving, {:.1} s after", HOLD_NS as f64 / 1e9);
        }
        GpuBoost { file, max, floor, boosted: false, until_ns: 0, boosted_ns: 0, since_ns: 0 }
    }

    fn write(&mut self, value: &str) {
        if let Some(f) = &mut self.file {
            if let Err(e) = f.write_all(value.as_bytes()) {
                tracing::warn!("GPU boost: {e}");
            }
        }
    }

    /// A touch, or the shell moving: the clock up, and held HOLD_NS.
    pub fn kick(&mut self, now_ns: u64) {
        if self.file.is_none() {
            return;
        }
        self.until_ns = now_ns + HOLD_NS;
        if !self.boosted {
            let max = self.max.clone();
            self.write(&max);
            self.boosted = true;
            self.since_ns = now_ns;
        }
    }

    /// Now and then: the clock back down once the hold is over.
    pub fn tick(&mut self, now_ns: u64) {
        if self.boosted && now_ns >= self.until_ns {
            let floor = self.floor.clone();
            self.write(&floor);
            self.boosted = false;
            self.boosted_ns += now_ns - self.since_ns;
        }
    }

    /// The share of the time since the last call spent boosted, and resets.
    pub fn take_share(&mut self, now_ns: u64, period_ns: u64) -> f64 {
        let mut t = std::mem::take(&mut self.boosted_ns);
        if self.boosted {
            t += now_ns - self.since_ns.max(now_ns.saturating_sub(period_ns));
            self.since_ns = now_ns;
        }
        (t as f64 / period_ns as f64 * 100.0).min(100.0)
    }
}

impl Drop for GpuBoost {
    fn drop(&mut self) {
        if self.boosted {
            let floor = self.floor.clone();
            self.write(&floor);
        }
    }
}
