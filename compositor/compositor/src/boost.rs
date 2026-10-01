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
//! The big cores' minimum clocks go up with it (to the step at three
//! quarters of their top): schedutil raises them only after a frame has
//! run late on them, a frame too late.
//!
//! It writes `/sys/class/kgsl/kgsl-3d0/devfreq/min_freq` and the big
//! clusters' `scaling_min_freq`, which are root's: `tools/session-run.sh`
//! hands them to the session's user for a run and back after. Not
//! writable, or `GPU_BOOST=0` / `CPU_BOOST=0`: no boost there.

use std::io::Write;

const DEVFREQ: &str = "/sys/class/kgsl/kgsl-3d0/devfreq";
/// The big cores' clusters (the A76s and the prime): their minimum clock.
const CPU_POLICIES: [&str; 2] = ["/sys/devices/system/cpu/cpufreq/policy4", "/sys/devices/system/cpu/cpufreq/policy7"];
/// How high the big cores' minimum goes: of their top clock, the nearest
/// step at or above this share (the top itself would only burn more).
const CPU_SHARE: f64 = 0.75;
/// How long the clocks stay up after the last touch or motion.
const HOLD_NS: u64 = 1_500_000_000;

/// One clock's minimum: its file, what it is raised to and what it was.
struct Knob {
    name: &'static str,
    file: std::fs::File,
    high: String,
    floor: String,
}

pub struct GpuBoost {
    knobs: Vec<Knob>,
    boosted: bool,
    until_ns: u64,
    /// Time spent boosted, for the report (ns).
    pub boosted_ns: u64,
    since_ns: u64,
}

impl GpuBoost {
    pub fn new() -> GpuBoost {
        let read = |p: &str| std::fs::read_to_string(p).map(|s| s.trim().to_owned()).unwrap_or_default();
        let open = |p: &str| std::fs::OpenOptions::new().write(true).open(p);
        let mut knobs = Vec::new();
        if std::env::var("GPU_BOOST").map(|v| v != "0").unwrap_or(true) {
            match open(&format!("{DEVFREQ}/min_freq")) {
                Ok(file) => knobs.push(Knob { name: "GPU", file, high: read(&format!("{DEVFREQ}/max_freq")), floor: read(&format!("{DEVFREQ}/min_freq")) }),
                Err(e) => tracing::warn!("boost: {DEVFREQ}/min_freq: {e}; no GPU boost"),
            }
        }
        if std::env::var("CPU_BOOST").map(|v| v != "0").unwrap_or(true) {
            for (policy, name) in CPU_POLICIES.iter().zip(["big cores", "prime core"]) {
                let steps: Vec<u64> = read(&format!("{policy}/scaling_available_frequencies")).split_whitespace().filter_map(|f| f.parse().ok()).collect();
                let top = read(&format!("{policy}/cpuinfo_max_freq")).parse::<u64>().unwrap_or(0);
                let want = (top as f64 * CPU_SHARE) as u64;
                let high = steps.iter().copied().filter(|&f| f >= want).min().unwrap_or(top);
                match open(&format!("{policy}/scaling_min_freq")) {
                    Ok(file) if high > 0 => knobs.push(Knob { name, file, high: high.to_string(), floor: read(&format!("{policy}/scaling_min_freq")) }),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("boost: {policy}/scaling_min_freq: {e}; no {name} boost"),
                }
            }
        }
        for k in &knobs {
            tracing::info!("boost: {} min {} -> {} while touched or moving, {:.1} s after", k.name, k.floor, k.high, HOLD_NS as f64 / 1e9);
        }
        GpuBoost { knobs, boosted: false, until_ns: 0, boosted_ns: 0, since_ns: 0 }
    }

    fn set(&mut self, high: bool) {
        for k in &mut self.knobs {
            let value = if high { &k.high } else { &k.floor };
            if let Err(e) = k.file.write_all(value.as_bytes()) {
                tracing::warn!("boost: {}: {e}", k.name);
            }
        }
    }

    /// A touch, or the shell moving or about to: the clocks up, and held
    /// HOLD_NS.
    pub fn kick(&mut self, now_ns: u64) {
        if self.knobs.is_empty() {
            return;
        }
        self.until_ns = now_ns + HOLD_NS;
        if !self.boosted {
            self.set(true);
            self.boosted = true;
            self.since_ns = now_ns;
        }
    }

    /// Now and then: the clocks back down once the hold is over.
    pub fn tick(&mut self, now_ns: u64) {
        if self.boosted && now_ns >= self.until_ns {
            self.set(false);
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
            self.set(false);
        }
    }
}
