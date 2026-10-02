//! The clock on the desktop: with no status bar, the time and the date stand
//! on the free panel - the right one when both are free, none when neither
//! is - where the dock stands. One line in the accent (accent.rs), near the
//! top: the time bold, the date beside it ("14:51  Fri 2 Oct"), both 30 px;
//! each minute it steps up to 12 px from its place, so no pixel of it is lit
//! all day on the OLED. It takes no touches. Under it, small and grey, the
//! weather now (the system screen's, from met.no): its icon, the
//! temperature, what it is.
//! On a panel it was not on, it fades in (300 ms); carried along the ribbon
//! with its desk, it is not faded.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;

use crate::layout::{self, SCALE};
use crate::shade::{local_time, ShellElement};

/// "Fri 2 Oct".
fn short_date(tm: &libc::tm) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {}", DAYS[tm.tm_wday as usize % 7], tm.tm_mday, MONTHS[tm.tm_mon as usize % 12])
}
use crate::text::{Font, Label};

const TOP: f64 = 0.08;
/// The line's size, and the room between the time and the date.
const SIZE: f32 = 30.0;
const BETWEEN: i32 = 14;
const SHIFT: i32 = 12;
const GAP: i32 = 6;
const FADE_NS: u64 = 300_000_000;

pub struct Clock {
    fonts: Option<(Font, Font)>,
    time: Label,
    date: Label,
    /// The weather line, its icon, and what they were made from.
    weather: Label,
    weather_icon: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
    weather_was: Option<(i64, String)>,
    /// The step aside, logical px, and the minute it was taken for.
    step: (i32, i32),
    minute: i32,
    /// The panel it was last drawn on, and since when.
    on: std::cell::Cell<(Option<usize>, u64)>,
    /// The accent it is drawn in (accent.rs's version).
    accent: u64,
}

impl Clock {
    pub fn new() -> Clock {
        let bold = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let medium = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Medium.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let mut clock = Clock {
            fonts: bold.zip(medium),
            time: Label::new(SIZE, [1.0; 4]),
            date: Label::new(SIZE, [1.0; 4]),
            weather: Label::new(16.0, [1.0, 1.0, 1.0, 0.45]),
            weather_icon: None,
            weather_was: None,
            step: (0, 0),
            minute: -1,
            on: std::cell::Cell::new((None, 0)),
            accent: 0,
        };
        clock.refresh();
        clock
    }

    /// Brings it up to the minute; returns whether it changed.
    pub fn refresh(&mut self) -> bool {
        let Some((bold, medium)) = &self.fonts else { return false };
        let now = local_time();
        let minute = now.tm_hour * 60 + now.tm_min;
        // The accent changed: drawn again in it.
        let accent = crate::accent::version();
        if minute == self.minute && accent == self.accent {
            return false;
        }
        if accent != self.accent {
            self.accent = accent;
            let mut c = crate::accent::get_f();
            c[3] = 0.95;
            self.time.set_color(c);
            self.date.set_color(c);
        }
        let stepped = minute != self.minute;
        self.minute = minute;
        self.time.set(bold, &format!("{}:{:02}", now.tm_hour, now.tm_min));
        self.date.set(medium, &short_date(&now));
        if !stepped {
            return true;
        }
        // A step aside each minute: a small generator on the minute.
        let mut x = (minute as u32).wrapping_mul(2654435761) ^ 0x9e37_79b9;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x % (2 * SHIFT as u32 + 1)) as i32 - SHIFT
        };
        self.step = (next(), next());
        true
    }

    /// The weather now; returns whether the line changed.
    pub fn set_weather(&mut self, now: Option<(f64, String, String)>) -> bool {
        let Some((_, regular)) = &self.fonts else { return false };
        let key = now.as_ref().map(|(t, word, _)| (t.round() as i64, word.clone()));
        if key == self.weather_was {
            return false;
        }
        self.weather_was = key;
        match now {
            Some((t, word, icon)) => {
                self.weather.set(regular, &format!("{}°  {word}", t.round() as i64));
                self.weather_icon = crate::lock::tinted(&format!("/usr/share/icons/Adwaita/symbolic/status/{icon}-symbolic.svg"), 20, [255, 255, 255]);
            }
            None => {
                self.weather.set(regular, "");
                self.weather_icon = None;
            }
        }
        true
    }

    pub fn warm_up(&self, renderer: &mut GlesRenderer) -> usize {
        [&self.time, &self.date]
            .iter()
            .filter(|l| MemoryRenderBufferRenderElement::from_buffer(renderer, (0.0, 0.0), &l.buffer, None, None, None, Kind::Unspecified).is_ok())
            .count()
    }

    /// Whether it is still fading in at `frame_ns`.
    pub fn fading(&self, frame_ns: u64) -> bool {
        let (on, since) = self.on.get();
        on.is_some() && frame_ns < since + FADE_NS
    }

    /// What it draws on `panel`, if any, at `frame_ns`; `carried` along the
    /// ribbon, its panel is where it came from and it is not faded.
    /// `middle`: where its middle is, logical x, when it goes along with the
    /// dock from one panel to the other (not faded then); else the panel's.
    pub fn elements(&self, renderer: &mut GlesRenderer, panel: Option<usize>, frame_ns: u64, carried: bool, middle: Option<f64>) -> Vec<ShellElement> {
        // Going along: already on the panel it goes to, so it is not faded
        // in there when it arrives.
        if middle.is_some() {
            self.on.set((panel, 0));
        }
        let carried = carried || middle.is_some();
        let (was, since) = self.on.get();
        if !carried && panel != was {
            self.on.set((panel, if was.is_none() && since == 0 { 0 } else { frame_ns }));
        }
        let Some(panel) = panel else { return Vec::new() };
        let since = self.on.get().1;
        let k = if carried || since == 0 { 1.0 } else { (frame_ns.saturating_sub(since) as f32 / FADE_NS as f32).clamp(0.0, 1.0) };
        let alpha = k * k * (3.0 - 2.0 * k);
        let rect = layout::panels()[panel];
        let top = (rect.size.h as f64 * TOP) as i32 + self.step.1;
        let mut out = Vec::new();
        let mut y = top;
        let mid = middle.map(|m| m.round() as i32).unwrap_or(rect.loc.x + rect.size.w / 2);
        // One line: the time, then the date, the two centred together.
        let line_w = self.time.extent.w + BETWEEN + self.date.extent.w;
        let mut x = mid - line_w / 2 + self.step.0;
        for label in [&self.time, &self.date] {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                ((x * SCALE) as f64, (y * SCALE) as f64),
                &label.buffer,
                Some(alpha),
                None,
                None,
                Kind::Unspecified,
            ) {
                out.push(ShellElement::Text(e));
            }
            x += label.extent.w + BETWEEN;
        }
        y += self.time.extent.h.max(self.date.extent.h) + GAP;
        // The weather, its icon before it, a little below the date.
        if self.weather.extent.w > 0 {
            let icon_w = if self.weather_icon.is_some() { 26 } else { 0 };
            let x = mid - (self.weather.extent.w + icon_w) / 2 + self.step.0;
            let y = y + 4;
            let put = |out: &mut Vec<ShellElement>, renderer: &mut GlesRenderer, b: &smithay::backend::renderer::element::memory::MemoryRenderBuffer, x: i32, y: i32, a: f32| {
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * SCALE) as f64, (y * SCALE) as f64), b, Some(a), None, None, Kind::Unspecified) {
                    out.push(ShellElement::Text(e));
                }
            };
            if let Some(icon) = &self.weather_icon {
                put(&mut out, renderer, icon, x, y + (self.weather.extent.h - 20) / 2, alpha * 0.5);
            }
            put(&mut out, renderer, &self.weather.buffer, x + icon_w, y, alpha);
        }
        out
    }
}
