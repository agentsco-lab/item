//! The hinge's angle and which way the Duo is held, from the sensors read
//! in-process (sensors.rs: the hinge, gravity, the roll from the gyroscope,
//! Moving, Still): 180 flat, less folded like a book, more folded back;
//! gravity from the right, camera half's accelerometer, x y z in g.
//!
//! item's `Orientation` is decided here from the angle, gravity and the
//! roll, as measured in hand on 2026-10-22 (notes/drafts/40, 41): x runs
//! from the hinge to the outer edge, y along the long side (up when the
//! book stands), z out of the screen. Thresholds carry 0.1 g (5°) of
//! hysteresis; a candidate is taken once the readings have been still for a
//! while, never while the hinge moves, never while a finger is down or
//! within 300 ms of its lift, never with the display dark. The frame turns
//! with it (layout.rs, output.rs's turn through blur, draft 44).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;

/// Folded back: the left panel's glass from here (degrees) to here.
const FOLD_FROM: f64 = 190.0;
const FOLD_TO: f64 = 330.0;

/// Which way the Duo is held. `0` is the left panel, `1` the right.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Orientation {
    /// Shut.
    Closed,
    /// Both panels to the user, the hinge vertical, or flat on the table:
    /// the layout as it is. Upside down when the hinge's top is below.
    Book { upside_down: bool },
    /// The hinge horizontal, one half on the table and the other raised:
    /// `lid` is the raised panel.
    Laptop { lid: usize },
    /// Open, held with the hinge horizontal, both panels to the user one
    /// above the other: `up` is the upper panel. The laptop's layout in hand.
    Stacked { up: usize },
    /// Folded past flat, standing on both edges with the hinge up.
    Tent,
    /// Folded past flat, one half lying face down and the other raised with
    /// its screen out: `up` is the raised panel.
    Stand { up: usize },
    /// Folded back, one panel to the user. Landscape when the hinge is
    /// above or below (`hinge_down`).
    Single { panel: usize, upside_down: bool, landscape: Option<bool> },
}

impl Orientation {
    pub fn name(self) -> String {
        let side = |p: usize| if p == 0 { "left" } else { "right" };
        match self {
            Orientation::Closed => "closed".into(),
            Orientation::Book { upside_down: false } => "book".into(),
            Orientation::Book { upside_down: true } => "book, upside down".into(),
            Orientation::Laptop { lid } => format!("laptop, {} lid", side(lid)),
            Orientation::Stacked { up } => format!("stacked, {} up", side(up)),
            Orientation::Tent => "tent".into(),
            Orientation::Stand { up } => format!("stand, {} up", side(up)),
            Orientation::Single { panel, upside_down, landscape } => {
                let how = match (landscape, upside_down) {
                    (Some(true), _) => ", hinge down",
                    (Some(false), _) => ", hinge up",
                    (None, true) => ", upside down",
                    (None, false) => "",
                };
                format!("{} panel{how}", side(panel))
            }
        }
    }
}

/// The orientation last decided, for threads without the state (the
/// system screen's facts).
static CURRENT: Mutex<Orientation> = Mutex::new(Orientation::Book { upside_down: false });

pub fn current() -> Orientation {
    *CURRENT.lock().unwrap()
}

/// What the bus told last.
#[derive(Clone, Copy, Default)]
pub struct Told {
    pub angle: f64,
    pub gravity: [f64; 3],
    pub moving: bool,
    /// The roll: how far turned about the screen's normal from the book's
    /// upright, degrees, positive the right panel going up.
    pub roll: f64,
    /// Still: the gyroscope quiet for a while.
    pub still: bool,
}

/// What sensors.rs says on its channel: a reading, or the display dark
/// (nothing is decided) and lit again - said once gravity has been read
/// afresh, so the reading after is never stale gravity with a new angle.
pub enum Said {
    Told(Told),
    Dark(bool),
}

/// `value` past `on` turns it on; on, it stays until `value` drops under `off`.
fn above(value: f64, on: f64, off: f64, was: bool) -> bool {
    if was { value > off } else { value > on }
}

/// The orientation the readings say, or None when nothing dominates (the
/// last one stays): the thresholds measured in hand, 0.1 g apart for the
/// orientation held.
fn decide(t: Told, cur: Orientation) -> Option<Orientation> {
    let [x, y, z] = t.gravity;
    let a = t.angle;
    let is = |o: Orientation| cur == o;
    // The angle's bands, 5° of hysteresis each.
    let closed = above(-a, -10.0, -15.0, is(Orientation::Closed));
    if closed {
        return Some(Orientation::Closed);
    }
    let single = above(a, 340.0, 335.0, matches!(cur, Orientation::Single { .. }));
    if single {
        let (cur_panel, cur_up, cur_land) = match cur {
            Orientation::Single { panel, upside_down, landscape } => (Some(panel), upside_down, landscape),
            _ => (None, false, None),
        };
        let panel = if above(z, 0.3, 0.2, cur_panel == Some(1)) {
            1
        } else if above(-z, 0.3, 0.2, cur_panel == Some(0)) {
            0
        } else {
            cur_panel?
        };
        let landscape = if above(x, 0.6, 0.5, cur_land == Some(true)) {
            Some(true)
        } else if above(-x, 0.6, 0.5, cur_land == Some(false)) {
            Some(false)
        } else {
            None
        };
        let upside_down = landscape.is_none() && above(-y, 0.5, 0.4, cur_up);
        return Some(Orientation::Single { panel, upside_down, landscape });
    }
    let back = above(a, 210.0, 205.0, matches!(cur, Orientation::Tent | Orientation::Stand { .. }));
    if back {
        return if above(-x, 0.6, 0.5, is(Orientation::Tent)) {
            Some(Orientation::Tent)
        } else if above(x, 0.6, 0.5, is(Orientation::Stand { up: 1 })) {
            Some(Orientation::Stand { up: 1 })
        } else if above(-z, 0.8, 0.7, is(Orientation::Stand { up: 0 })) {
            Some(Orientation::Stand { up: 0 })
        } else {
            matches!(cur, Orientation::Tent | Orientation::Stand { .. }).then_some(cur)
        };
    }
    let was_book = matches!(cur, Orientation::Book { .. });
    let book_down = above(-y, 0.5, 0.4, is(Orientation::Book { upside_down: true }));
    // Open, the hinge horizontal: the right panel above the left (its outer
    // edge up) or below.
    // By the roll about the screen's normal (the daemon's, gyroscope and
    // gravity), meaningful while gravity lies in the screen's plane, with
    // Android's 45° gap: in past 55°, out under 35° (draft 44).
    let was_stacked = matches!(cur, Orientation::Stacked { .. });
    let stacked = if z.abs() < 0.85 && above(t.roll.abs(), 55.0, 35.0, was_stacked) {
        Some(Orientation::Stacked { up: if t.roll > 0.0 { 1 } else { 0 } })
    } else {
        None
    };
    let flat = above(a, 150.0, 145.0, was_book || matches!(cur, Orientation::Stacked { .. }));
    if flat {
        return Some(stacked.unwrap_or(Orientation::Book { upside_down: book_down }));
    }
    // 10 to 150: a book standing, a laptop with one half on the table, or
    // the book held with the hinge horizontal.
    if above(y.abs(), 0.6, 0.5, was_book) {
        Some(Orientation::Book { upside_down: book_down })
    } else if above(x, 0.6, 0.5, is(Orientation::Laptop { lid: 1 })) {
        Some(Orientation::Laptop { lid: 1 })
    } else if above(z, 0.95, 0.9, is(Orientation::Laptop { lid: 0 })) && y.abs() < 0.2 {
        Some(Orientation::Laptop { lid: 0 })
    } else if let Some(o @ Orientation::Stacked { up: 0 }) = stacked {
        Some(o)
    } else {
        (was_book || matches!(cur, Orientation::Laptop { .. } | Orientation::Stacked { .. })).then_some(cur)
    }
}

/// A reading held this long, still, before the orientation it says is taken.
/// (The daemon's Still already took 200 ms of a quiet gyroscope.)
const STEADY_MS: u64 = 60;
/// Gravity's magnitude at rest (the sensor reads 1.02 g), and how far from
/// it a reading is the hand moving, not a posture.
const G_AT_REST: f64 = 1.02;
const G_SLACK: f64 = 0.12;
/// The hinge turning this much while a reading is held starts the hold over.
const HINGE_STILL: f64 = 5.0;

/// The screen touched: no decision while a finger is down or within
/// TOUCH_GAP_NS of its lift (Android's PROPOSAL_MIN_TIME_SINCE_TOUCH_END).
static TOUCH_END_NS: AtomicU64 = AtomicU64::new(0);
const TOUCH_GAP_NS: u64 = 300_000_000;

/// input.rs tells of fingers lifted: a decision waits 300 ms after one.
/// A finger resting (a thumb holding the Duo open) does not hold it - the
/// owner's word, 2026-10-22: the turn must come while the phone is held.
pub fn touched(down: bool, now_ns: u64) {
    if !down {
        TOUCH_END_NS.store(now_ns, Ordering::Relaxed);
    }
}

/// A finger lifted just now (as Android holds its proposals).
fn touched_lately(now_ns: u64) -> bool {
    now_ns.saturating_sub(TOUCH_END_NS.load(Ordering::Relaxed)) < TOUCH_GAP_NS
}

/// Takes a new orientation only from readings that held: the hinge not
/// turning, gravity's magnitude that of rest, the same answer for STEADY_MS.
#[derive(Default)]
struct Decider {
    /// What the readings have been saying, since when, at what angle.
    candidate: Option<(Orientation, std::time::Instant, f64)>,
    /// The quarter second the holding gate was last logged in.
    said: u64,
}

impl Decider {
    fn consider(&mut self, t: Told, now: std::time::Instant) -> Option<Orientation> {
        let [x, y, z] = t.gravity;
        let g = (x * x + y * y + z * z).sqrt();
        let cur = current();
        let held = t.moving || !t.still || (g - G_AT_REST).abs() > G_SLACK || touched_lately(hybris_hwc::now_ns());
        // While the readings say another orientation, which gate holds it
        // (at most four times a second).
        if held && decide(t, cur).is_some_and(|o| o != cur) {
            let slot = hybris_hwc::now_ns() / 250_000_000;
            if self.said != slot {
                self.said = slot;
                tracing::debug!("posture: held by{}{}{}{} (roll {:.0}, g {g:.2})", if t.moving { " hinge" } else { "" }, if !t.still { " gyro" } else { "" }, if (g - G_AT_REST).abs() > G_SLACK { " |g|" } else { "" }, if touched_lately(hybris_hwc::now_ns()) { " touch" } else { "" }, t.roll);
            }
        }
        if held {
            self.candidate = None;
            return None;
        }
        let Some(next) = decide(t, cur) else { return None };
        if next == cur {
            self.candidate = None;
            return None;
        }
        match self.candidate {
            Some((o, since, at)) if o == next && (t.angle - at).abs() <= HINGE_STILL => {
                if now.duration_since(since).as_millis() as u64 >= STEADY_MS {
                    self.candidate = None;
                    Some(next)
                } else {
                    None
                }
            }
            _ => {
                self.candidate = Some((next, now, t.angle));
                None
            }
        }
    }
}

/// Gravity as last read (x, y, z in g), while the sensors stream it.
static GRAVITY: std::sync::Mutex<Option<[f64; 3]>> = std::sync::Mutex::new(None);

/// Gravity as last read, if it is read (the first setup's drop leans with
/// it).
pub fn gravity() -> Option<[f64; 3]> {
    *GRAVITY.lock().unwrap()
}

pub struct Posture {
    angle: Arc<AtomicU64>,
    changed: Arc<AtomicBool>,
    orientation_changed: Arc<AtomicBool>,
    /// The readings say another orientation than the one held: the screen
    /// is veiled until they settle (or come back).
    turning: Arc<AtomicBool>,
    /// The sensors (sensors.rs), to stop and start with the display.
    sensors: crate::sensors::Sensors,
}

impl Posture {
    pub fn new(wake: Ping) -> Posture {
        let angle = Arc::new(AtomicU64::new(180f64.to_bits()));
        let changed: Arc<AtomicBool> = Default::default();
        let orientation_changed: Arc<AtomicBool> = Default::default();
        let turning: Arc<AtomicBool> = Default::default();
        let (a, c, oc, tr) = (angle.clone(), changed.clone(), orientation_changed.clone(), turning.clone());
        // The sensors read by item itself (sensors.rs): what they say comes
        // on a channel, and the decider takes its time (a reading held
        // STEADY_MS) on a thread of its own.
        let (sensors, rx) = crate::sensors::Sensors::start(wake.clone());
        let _ = std::thread::Builder::new().name("posture decide".into()).spawn(move || {
            let mut told: Option<Told> = None;
            let mut dark = false;
            let mut decider = Decider::default();
            loop {
                // Woken by what the sensors say; by the clock only while a
                // candidate is maturing, which the clock alone ripens. Dark,
                // or with nothing maturing, the thread sleeps.
                let said = if decider.candidate.is_some() && !dark {
                    match rx.recv_timeout(std::time::Duration::from_millis(STEADY_MS / 2)) {
                        Ok(s) => Some(s),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                } else {
                    match rx.recv() {
                        Ok(s) => Some(s),
                        Err(_) => return,
                    }
                };
                match said {
                    Some(Said::Told(t)) => {
                        if t.angle.is_finite() && t.angle.to_bits() != a.swap(t.angle.to_bits(), Ordering::Relaxed) {
                            c.store(true, Ordering::Relaxed);
                            wake.ping();
                        }
                        // Dark, the angle is still passed on (the hinge read
                        // as the display lit) but nothing is decided from it.
                        if !dark {
                            told = Some(t);
                            *GRAVITY.lock().unwrap() = Some(t.gravity);
                        }
                    }
                    Some(Said::Dark(d)) => {
                        // Dark: no stale reading to keep judging, no turn with
                        // the screen off. Lit: decided afresh from the next.
                        dark = d;
                        told = None;
                        decider.candidate = None;
                    }
                    None => {}
                }
                let Some(t) = told else { continue };
                let says_other = decide(t, current()).is_some_and(|o| o != current());
                if says_other != tr.swap(says_other, Ordering::Relaxed) {
                    wake.ping();
                }
                if let Some(next) = decider.consider(t, std::time::Instant::now()) {
                    *CURRENT.lock().unwrap() = next;
                    let [x, y, z] = t.gravity;
                    tracing::info!("posture: {} (angle {:.0}°, gravity {x:.2} {y:.2} {z:.2})", next.name(), t.angle);
                    oc.store(true, Ordering::Relaxed);
                    tr.store(false, Ordering::Relaxed);
                    wake.ping();
                }
            }
        });
        Posture { angle, changed, orientation_changed, turning, sensors }
    }

    /// The display dark or lit: the sensors stop and start (sensors.rs).
    pub fn dark(&self, dark: bool) {
        self.sensors.dark(dark);
    }

    /// The turn handed to the sensors (gravity and the gyroscope read) or
    /// taken back (the hinge alone).
    pub fn by_sensors(&self, on: bool) {
        self.sensors.streams(on);
    }

    pub fn angle(&self) -> f64 {
        f64::from_bits(self.angle.load(Ordering::Relaxed))
    }

    /// Whether the angle changed since last asked.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }

    /// Which way the Duo is held, as last decided.
    pub fn orientation(&self) -> Orientation {
        current()
    }

    /// Whether the orientation changed since last asked.
    pub fn take_orientation_changed(&self) -> bool {
        self.orientation_changed.swap(false, Ordering::Relaxed)
    }

    /// Whether the readings say another orientation than the one held.
    pub fn turning(&self) -> bool {
        self.turning.load(Ordering::Relaxed)
    }

    /// How far folded back past flat (#117): 0 up to FOLD_FROM degrees, 1
    /// at FOLD_TO and beyond, eased between.
    pub fn fold_back(&self) -> f64 {
        let t = ((self.angle() - FOLD_FROM) / (FOLD_TO - FOLD_FROM)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// How far folded like a book, for the dock: 0 flat (and folded back),
    /// 1 at 90 degrees and below, eased between.
    pub fn book(&self) -> f64 {
        let t = ((180.0 - self.angle()) / 90.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(angle: f64, gravity: [f64; 3]) -> Told {
        Told { angle, gravity, moving: false, roll: 0.0, still: true }
    }
    const BOOK: Orientation = Orientation::Book { upside_down: false };

    /// The nine positions measured in hand on 2026-10-22.
    #[test]
    fn the_measured_positions_are_named() {
        let cases = [
            (120.0, [0.08, 0.96, 0.32], "book"),
            (127.0, [0.19, -0.94, 0.36], "book, upside down"),
            (180.0, [0.01, -0.01, 1.02], "book"),
            (114.0, [0.93, -0.01, 0.42], "laptop, right lid"),
            (114.0, [0.01, -0.02, 1.02], "laptop, left lid"),
            (319.0, [-0.95, -0.01, 0.38], "tent"),
            (360.0, [-0.01, 0.73, 0.71], "right panel"),
            (360.0, [0.00, 0.77, -0.66], "left panel"),
            (360.0, [0.10, -0.71, 0.73], "right panel, upside down"),
            (0.0, [0.0, 0.0, 1.02], "closed"),
        ];
        for (angle, g, name) in cases {
            assert_eq!(decide(told(angle, g), BOOK).map(|o| o.name()).as_deref(), Some(name), "{angle} {g:?}");
        }
        // Open, held with the hinge horizontal: the roll says so, past 55°.
        let rolled = |angle: f64, g: [f64; 3], roll: f64| Told { roll, ..told(angle, g) };
        assert_eq!(decide(rolled(180.0, [0.9, 0.05, 0.42], 88.0), BOOK).map(|o| o.name()).as_deref(), Some("stacked, right up"));
        assert_eq!(decide(rolled(175.0, [-0.9, 0.05, 0.42], -87.0), BOOK).map(|o| o.name()).as_deref(), Some("stacked, left up"));
        assert_eq!(decide(rolled(120.0, [-0.9, 0.05, 0.42], -80.0), BOOK).map(|o| o.name()).as_deref(), Some("stacked, left up"));
        // Not yet past the gap from the book; held, it stays stacked to 35°.
        assert_eq!(decide(rolled(180.0, [0.7, 0.7, 0.14], 45.0), BOOK), Some(BOOK));
        let up = Orientation::Stacked { up: 1 };
        assert_eq!(decide(rolled(180.0, [0.7, 0.7, 0.14], 45.0), up), Some(up));
        assert_eq!(decide(rolled(180.0, [0.5, 0.86, 0.1], 30.0), up), Some(BOOK));
    }

    #[test]
    fn nothing_dominant_keeps_the_last() {
        // A book leaned back 65°: y 0.42, z 0.9 - neither a book nor a laptop.
        assert_eq!(decide(told(120.0, [0.05, 0.42, 0.9]), BOOK), Some(BOOK));
        let lap = Orientation::Laptop { lid: 1 };
        assert_eq!(decide(told(120.0, [0.55, 0.3, 0.78]), lap), Some(lap));
        // From closed, the same reading says nothing.
        assert_eq!(decide(told(120.0, [0.05, 0.42, 0.9]), Orientation::Closed), None);
    }

    #[test]
    fn a_reading_must_hold_still_and_long() {
        use std::time::{Duration, Instant};
        let mut d = Decider::default();
        let t0 = Instant::now();
        let lap = told(114.0, [0.93, -0.01, 0.42]);
        // Book now; the laptop reading is taken only after STEADY_MS.
        *CURRENT.lock().unwrap() = BOOK;
        assert_eq!(d.consider(lap, t0), None);
        assert_eq!(d.consider(lap, t0 + Duration::from_millis(STEADY_MS - 1)), None);
        assert_eq!(d.consider(lap, t0 + Duration::from_millis(STEADY_MS)), Some(Orientation::Laptop { lid: 1 }));
        // A hand in motion (gravity off 1.02 g) starts the hold over.
        let ms = |n: u64| t0 + Duration::from_millis(n);
        assert_eq!(d.consider(lap, t0), None);
        assert_eq!(d.consider(told(114.0, [1.1, 0.0, 0.5]), ms(200)), None);
        assert_eq!(d.consider(lap, ms(400)), None);
        assert_eq!(d.consider(lap, ms(400 + STEADY_MS - 1)), None);
        assert_eq!(d.consider(lap, ms(400 + STEADY_MS)), Some(Orientation::Laptop { lid: 1 }));
        // So does the hinge turning, and the gyroscope not yet still.
        assert_eq!(d.consider(lap, ms(1000)), None);
        assert_eq!(d.consider(told(124.0, [0.93, -0.01, 0.42]), ms(1200)), None);
        assert_eq!(d.consider(lap, ms(1400)), None);
        assert_eq!(d.consider(Told { still: false, ..lap }, ms(1450)), None);
        assert_eq!(d.consider(lap, ms(1500)), None);
        assert_eq!(d.consider(lap, ms(1500 + STEADY_MS - 1)), None);
        assert_eq!(d.consider(lap, ms(1500 + STEADY_MS)), Some(Orientation::Laptop { lid: 1 }));
    }

    #[test]
    fn the_thresholds_hold_what_is_held() {
        let down = Orientation::Book { upside_down: true };
        // Upside down stays to y -0.4, enters at -0.5.
        assert_eq!(decide(told(120.0, [0.0, -0.45, 0.89]), down), Some(down));
        assert_eq!(decide(told(120.0, [0.0, -0.45, 0.89]), BOOK), Some(BOOK));
        // Closed stays to 15°.
        assert_eq!(decide(told(12.0, [0.0, 0.0, 1.0]), Orientation::Closed), Some(Orientation::Closed));
        assert_eq!(decide(told(12.0, [0.0, 0.3, 0.9]), BOOK), Some(BOOK));
    }
}
