//! The tour, once, after the first setup: the gestures, one at a time, each
//! going when the user has done it.
//!
//! On the right panel, under the clock: what to do and why, and a faint
//! drop showing the finger's way, again and again. Done - the grid up, the
//! shade out, the ribbon moved, an icon lifted - it goes; the next comes
//! once the user is back on the desktop (the grid down, the shade in, the
//! ribbon still). "Skip" at the foot of the left panel ends it. Ended or
//! done, `~/.config/item/tour-done` keeps it from coming again; TOUR=1
//! shows it all the same, for tests.
//!
//! After it, once: the first time an app is on a panel, at that panel's foot
//! on a dark plate, how to put it away (a swipe up from the bottom edge) and
//! where it is closed (the list in the right shade), with the finger's way -
//! gone when a window is put away or closed (item-tracker #18).
//! `~/.config/item/app-hint-done` keeps it from coming again.

use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

/// The steps: what to do, why, and the finger's way (from, to: shares of
/// the right panel's width and height).
const STEPS: [(&str, &str, (f64, f64), (f64, f64)); 4] = [
    ("Swipe up", "on an empty screen for all your apps.", (0.5, 0.92), (0.5, 0.62)),
    ("Pull down", "from the top for settings and your windows.", (0.5, 0.02), (0.5, 0.34)),
    ("Swipe sideways", "to move between screens, and to apps you left open.", (0.8, 0.6), (0.25, 0.6)),
    ("Hold an icon", "to put it on the dock, move it, or take it off.", (0.78, 0.94), (0.78, 0.94)),
];
/// The hint for apps: its words, the finger's way (shares of the panel), how
/// high its words stand (share of the panel's height).
const APP_WORDS: (&str, &str, &str) = ("Swipe up", "from the bottom edge to put it away;", "close it from the list in the right shade.");
const APP_WAY: ((f64, f64), (f64, f64)) = ((0.5, 0.985), (0.5, 0.80));
const APP_WORDS_Y: f64 = 0.62;
const APP_PAD: f64 = 18.0;

/// The hint for apps: not wanted, waiting for an app, or shown on a panel
/// since a time, with how many windows had gone then.
#[derive(Clone, Copy, PartialEq)]
enum AppHint {
    Off,
    Waiting,
    Showing { since: u64, panel: usize, gone: u64 },
}

/// The finger's way takes this long, then rests, again and again.
const WAY_NS: f64 = 1.1e9;
const REST_NS: f64 = 0.7e9;
const FADE_NS: f64 = 300e6;
const WORDS_Y: f64 = 0.44;
const SKIP_W: f64 = 120.0;
const SKIP_H: f64 = 44.0;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// Showing the step, since when.
    Showing(u64),
    /// Done: waiting for the desktop again.
    Done,
    /// The next one comes from then.
    Next(u64),
}

pub struct Tour {
    step: Option<usize>,
    phase: Phase,
    fonts: Option<(Font, Font)>,
    title: Label,
    line: Label,
    skip: Label,
    skip_bg: MemoryRenderBuffer,
    pressed: bool,
    app: AppHint,
    line2: Label,
    app_bg: Option<MemoryRenderBuffer>,
}

fn marker() -> std::path::PathBuf {
    std::path::Path::new(&std::env::var("HOME").unwrap_or_default()).join(".config/item/tour-done")
}

fn app_marker() -> std::path::PathBuf {
    std::path::Path::new(&std::env::var("HOME").unwrap_or_default()).join(".config/item/app-hint-done")
}

fn mark(path: &std::path::Path) {
    if std::env::var_os("TOUR").is_some() {
        return;
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, "");
}

/// What the tour watches each moment: the grid up, the shade out, the
/// ribbon moving or not at its desks, an icon carried.
pub struct Seen {
    pub grid: bool,
    pub shade: bool,
    pub ribbon_moved: bool,
    pub ribbon_home: bool,
    pub carried: bool,
    /// A panel showing an app, if one does.
    pub app_panel: Option<usize>,
    /// How many windows have been put away or closed so far.
    pub gone: u64,
}

impl Tour {
    pub fn new() -> Tour {
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        let mut skip = Label::new(16.0, [1.0, 1.0, 1.0, 0.8]);
        if let Some(f) = &regular {
            skip.set(f, "Skip");
        }
        Tour {
            step: None,
            phase: Phase::Done,
            fonts: thin.zip(regular),
            title: Label::new(30.0, [1.0, 1.0, 1.0, 0.92]),
            line: Label::new(17.0, [1.0, 1.0, 1.0, 0.7]),
            skip,
            skip_bg: crate::grid::rounded(SKIP_W, SKIP_H, SKIP_H / 2.0, [40, 40, 40, 40]),
            pressed: false,
            app: if std::env::var_os("TOUR").is_some() || !app_marker().exists() { AppHint::Waiting } else { AppHint::Off },
            line2: Label::new(17.0, [1.0, 1.0, 1.0, 0.7]),
            app_bg: None,
        }
    }

    /// Whether it is to be shown: not done before, or TOUR set.
    pub fn wanted() -> bool {
        std::env::var_os("TOUR").is_some() || !marker().exists()
    }

    /// It starts at `at` (after the setup, as the dock is born).
    pub fn start(&mut self, at: u64) {
        if !Self::wanted() {
            return;
        }
        tracing::info!("tour: starts");
        self.step = Some(0);
        self.phase = Phase::Next(at);
    }

    pub fn active(&self) -> bool {
        self.step.is_some()
    }

    /// Whether it still watches what the user does: the tour, or the hint
    /// for apps waiting or shown.
    pub fn watching(&self) -> bool {
        self.step.is_some() || self.app != AppHint::Off
    }

    fn app_words(&mut self) {
        let Some((thin, regular)) = &self.fonts else { return };
        self.title.set(thin, APP_WORDS.0);
        self.line.set(regular, APP_WORDS.1);
        self.line2.set(regular, APP_WORDS.2);
        let w = self.title.extent.w.max(self.line.extent.w).max(self.line2.extent.w) as f64 + 2.0 * APP_PAD;
        let h = (self.title.extent.h + self.line.extent.h + self.line2.extent.h) as f64 + 16.0 + 2.0 * APP_PAD;
        self.app_bg = Some(crate::grid::rounded(w, h, 22.0, [18, 18, 22, 210]));
    }

    /// The hint for apps, after the tour: shown when an app is on a panel,
    /// gone for good when a window is put away or closed.
    fn watch_app(&mut self, seen: &Seen, now_ns: u64) -> bool {
        match self.app {
            AppHint::Off => false,
            AppHint::Waiting => match seen.app_panel {
                Some(panel) if !seen.shade && !seen.ribbon_moved && !seen.grid => {
                    self.app_words();
                    self.app = AppHint::Showing { since: now_ns, panel, gone: seen.gone };
                    tracing::info!("tour: the hint for apps, on panel {panel}");
                    true
                }
                _ => false,
            },
            AppHint::Showing { since, panel, gone } => {
                if seen.gone > gone {
                    self.app = AppHint::Off;
                    mark(&app_marker());
                    tracing::info!("tour: the hint for apps done");
                    return true;
                }
                match seen.app_panel {
                    Some(p) if p != panel => {
                        self.app = AppHint::Showing { since, panel: p, gone };
                        true
                    }
                    Some(_) if !seen.shade && !seen.ribbon_moved => false,
                    // Away from the app (the desk, the shade): hidden, back
                    // with the next app.
                    _ => {
                        self.app = AppHint::Waiting;
                        true
                    }
                }
            }
        }
    }

    fn words(&mut self, i: usize) {
        let Some((thin, regular)) = &self.fonts else { return };
        self.title.set(thin, STEPS[i].0);
        self.line.set(regular, STEPS[i].1);
    }

    fn end(&mut self) {
        self.step = None;
        mark(&marker());
        tracing::info!("tour: done");
    }

    /// What the user did: the step goes when done, the next comes when
    /// back on the desktop. Returns whether it changed.
    pub fn watch(&mut self, seen: &Seen, now_ns: u64) -> bool {
        let Some(i) = self.step else { return self.watch_app(seen, now_ns) };
        let back = !seen.grid && !seen.shade && seen.ribbon_home && !seen.carried;
        match self.phase {
            Phase::Next(t) if now_ns >= t && back => {
                self.words(i);
                self.phase = Phase::Showing(now_ns);
                true
            }
            Phase::Showing(_) => {
                let done = match i {
                    0 => seen.grid,
                    1 => seen.shade,
                    2 => seen.ribbon_moved,
                    _ => seen.carried,
                };
                if done {
                    tracing::info!("tour: {} done", STEPS[i].0);
                    self.phase = Phase::Done;
                    return true;
                }
                false
            }
            Phase::Done if back => {
                if i + 1 >= STEPS.len() {
                    self.end();
                } else {
                    self.step = Some(i + 1);
                    self.phase = Phase::Next(now_ns + 600_000_000);
                }
                true
            }
            _ => false,
        }
    }

    fn skip_rect() -> Rectangle<f64, Logical> {
        let left = layout::panels()[0];
        Rectangle::new((left.loc.x as f64 + (left.size.w as f64 - SKIP_W) / 2.0, left.size.h as f64 - 120.0).into(), (SKIP_W, SKIP_H).into())
    }

    /// A touch on Skip: taken (and the tour ended when let go).
    pub fn down(&mut self, pos: Point<f64, Logical>) -> bool {
        self.pressed = self.showing() && Self::skip_rect().contains(pos);
        self.pressed
    }

    pub fn up(&mut self) -> bool {
        if std::mem::take(&mut self.pressed) {
            self.end();
            return true;
        }
        false
    }

    fn showing(&self) -> bool {
        self.step.is_some() && matches!(self.phase, Phase::Showing(_))
    }

    /// Whether it still wants frames.
    pub fn settle(&self) -> bool {
        self.showing() || matches!(self.app, AppHint::Showing { .. })
    }

    /// How shown the step (or the hint for apps) is at `frame_ns`.
    fn shown(&self, frame_ns: u64) -> f64 {
        if self.step.is_none() {
            return match self.app {
                AppHint::Showing { since, .. } => crate::pinpad::ease(frame_ns.saturating_sub(since) as f64 / FADE_NS),
                _ => 0.0,
            };
        }
        match self.phase {
            Phase::Showing(t) => crate::pinpad::ease(frame_ns.saturating_sub(t) as f64 / FADE_NS),
            _ => 0.0,
        }
    }

    /// The finger's drop now: its middle and how strong, if shown.
    pub fn hint(&self, frame_ns: u64) -> Option<((f64, f64), f64, f64)> {
        let (i, t, panel, way) = match (self.step, self.phase, self.app) {
            (Some(i), Phase::Showing(t), _) => (i, t, 1, (STEPS[i].2, STEPS[i].3)),
            (None, _, AppHint::Showing { since, panel, .. }) => (0, since, panel, APP_WAY),
            _ => return None,
        };
        let k = self.shown(frame_ns);
        let p = layout::panels()[panel];
        let at = |(x, y): (f64, f64)| (p.loc.x as f64 + p.size.w as f64 * x, p.size.h as f64 * y);
        let (a, b) = (at(way.0), at(way.1));
        let u = (frame_ns.saturating_sub(t) as f64) % (WAY_NS + REST_NS);
        if i == 3 && self.step.is_some() {
            // Held: the drop swells on the icon and lets go.
            let s = (u / (WAY_NS + REST_NS) * std::f64::consts::TAU).sin().abs();
            return Some((a, 20.0 + 6.0 * s, k));
        }
        let w = (u / WAY_NS).min(1.0);
        let e = crate::pinpad::ease(w);
        let fade = if u > WAY_NS { 1.0 - ((u - WAY_NS) / REST_NS).min(1.0) } else { (w / 0.15).min(1.0) };
        Some(((a.0 + (b.0 - a.0) * e, a.1 + (b.1 - a.1) * e), 22.0, k * fade))
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let mut out = Vec::new();
        let k = self.shown(frame_ns) as f32;
        if k <= 0.0 {
            return out;
        }
        let right = layout::panels()[1];
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, a: f32| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE as f64).round(), (y * SCALE as f64).round()), b, Some(a), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        // The hint for apps: its words on a dark plate at the panel's foot.
        if let (None, AppHint::Showing { panel, .. }, Some(bg)) = (self.step, self.app, &self.app_bg) {
            let p = layout::panels()[panel];
            let cx = |w: i32| p.loc.x as f64 + (p.size.w - w) as f64 / 2.0;
            let y = p.size.h as f64 * APP_WORDS_Y;
            let y2 = y + self.title.extent.h as f64 + 8.0;
            let y3 = y2 + self.line.extent.h as f64 + 8.0;
            put(&mut out, &self.title.buffer, cx(self.title.extent.w), y, k);
            put(&mut out, &self.line.buffer, cx(self.line.extent.w), y2, k);
            put(&mut out, &self.line2.buffer, cx(self.line2.extent.w), y3, k);
            let w = self.title.extent.w.max(self.line.extent.w).max(self.line2.extent.w) + 2 * APP_PAD as i32;
            put(&mut out, bg, cx(w), y - APP_PAD, k);
            return out;
        }
        let cx = |w: i32| right.loc.x as f64 + (right.size.w - w) as f64 / 2.0;
        let y = right.size.h as f64 * WORDS_Y;
        put(&mut out, &self.title.buffer, cx(self.title.extent.w), y, k);
        put(&mut out, &self.line.buffer, cx(self.line.extent.w), y + self.title.extent.h as f64 + 8.0, k);
        let s = Self::skip_rect();
        let a = if self.pressed { k * 0.6 } else { k };
        put(&mut out, &self.skip.buffer, s.loc.x + (SKIP_W - self.skip.extent.w as f64) / 2.0, s.loc.y + (SKIP_H - self.skip.extent.h as f64) / 2.0, a);
        put(&mut out, &self.skip_bg, s.loc.x, s.loc.y, a);
        out
    }
}
