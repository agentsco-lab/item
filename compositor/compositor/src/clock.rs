//! The clock on the desktop: with no status bar, the time and the date stand
//! on the free panel - the right one when both are free, none when neither
//! is - where the dock stands. One line in the accent (accent.rs), near the
//! top, set to the panel's right edge: the time bold, the date beside it
//! ("14:51  Fri 2 Oct"), both 36 px,
//! standing still (it used to step aside each minute against burn-in, which
//! looked restless). It takes no touches. Under it, small and grey, the
//! weather now (the system screen's, from met.no): its icon, the
//! temperature, what it is.
//! On a panel it was not on, it fades in (300 ms); carried along the ribbon
//! with its desk, it is not faded.
//!
//! Its look is the owner's (the wallpaper's picker, picker.rs): one of
//! FONTS, a brightness, and where on its panel it stands (its right edge and
//! top, as parts of the panel; by default RIGHT in from the right edge, TOP
//! down) - kept in `~/.config/item/clock`.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;

use std::sync::Mutex;

use smithay::utils::{Logical, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::{local_time, ShellElement};

/// "Fri 2 Oct".
pub fn short_date(tm: &libc::tm) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {}", DAYS[tm.tm_wday as usize % 7], tm.tm_mday, MONTHS[tm.tm_mon as usize % 12])
}
use crate::text::{Font, Label};

const TOP: f64 = 0.08;
/// The line's size, and the room between the time and the date.
const SIZE: f32 = 36.0;
const BETWEEN: i32 = 14;
/// How far in from the panel's right edge the line ends.
const RIGHT: i32 = 40;
const GAP: i32 = 6;
const FADE_NS: u64 = 300_000_000;

/// The clock's fonts: a name, the time's and the date's.
pub const FONTS: [(&str, &str, &str); 5] = [
    ("Lato", "/usr/share/fonts/truetype/lato/Lato-Bold.ttf", "/usr/share/fonts/truetype/lato/Lato-Medium.ttf"),
    ("Light", "/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/lato/Lato-Light.ttf"),
    ("Rounded", "/usr/share/fonts/truetype/quicksand/Quicksand-Bold.ttf", "/usr/share/fonts/truetype/quicksand/Quicksand-Medium.ttf"),
    ("Serif", "/usr/share/fonts/truetype/noto/NotoSerif-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSerif-Regular.ttf"),
    ("Mono", "/usr/share/fonts/truetype/noto/NotoSansMono-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf"),
];

/// The clock's look.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Style {
    /// One of FONTS.
    pub font: usize,
    /// Its brightness: the accent's alpha.
    pub alpha: f32,
    /// Its right edge and top as parts of its panel; None: where it always was.
    pub at: Option<(f64, f64)>,
}

impl Default for Style {
    fn default() -> Style {
        Style { font: 0, alpha: 0.95, at: None }
    }
}

/// The look, and a count of its changes.
static STYLE: Mutex<Option<(Style, u64)>> = Mutex::new(None);

fn style_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/item/clock")
}

/// The look now (read from its file the first time).
pub fn style() -> (Style, u64) {
    let mut s = STYLE.lock().unwrap();
    *s.get_or_insert_with(|| {
        let mut st = Style::default();
        for line in std::fs::read_to_string(style_path()).unwrap_or_default().lines() {
            let w: Vec<&str> = line.split_whitespace().collect();
            match w.as_slice() {
                ["font", f] => st.font = f.parse::<usize>().unwrap_or(0).min(FONTS.len() - 1),
                ["alpha", a] => st.alpha = a.parse::<f32>().unwrap_or(0.95).clamp(0.25, 1.0),
                ["at", x, y] => st.at = x.parse::<f64>().ok().zip(y.parse::<f64>().ok()).map(|(x, y)| (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))),
                _ => {}
            }
        }
        (st, 1)
    })
}

/// A new look, kept.
pub fn set_style(st: Style) {
    let mut s = STYLE.lock().unwrap();
    let version = s.map(|(_, v)| v).unwrap_or(0) + 1;
    *s = Some((st, version));
    let mut text = format!("font {}\nalpha {:.2}\n", st.font, st.alpha);
    if let Some((x, y)) = st.at {
        text.push_str(&format!("at {x:.4} {y:.4}\n"));
    }
    let path = style_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, text);
    tracing::info!("clock: {} at {:.0} %{}", FONTS[st.font].0, st.alpha * 100.0, st.at.map(|(x, y)| format!(", at ({x:.2}, {y:.2})")).unwrap_or_default());
}

pub struct Clock {
    fonts: Option<(Font, Font)>,
    time: Label,
    date: Label,
    /// The weather line, its icon, and what they were made from.
    weather: Label,
    weather_icon: Option<smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
    weather_was: Option<(i64, String)>,
    /// The minute it shows.
    minute: i32,
    /// The panel it was last drawn on, and since when.
    on: std::cell::Cell<(Option<usize>, u64)>,
    /// The accent it is drawn in (accent.rs's version).
    accent: u64,
    /// Carried along the ribbon at the last frame.
    was_carried: std::cell::Cell<bool>,
    /// FONTS loaded: the time's and the date's.
    all_fonts: Vec<Option<(Font, Font)>>,
    /// The look it is drawn in (`style`'s count).
    styled: u64,
    /// Where it was last drawn, logical px: what a finger takes hold of.
    pub last_rect: std::cell::Cell<Option<Rectangle<f64, Logical>>>,
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
            minute: -1,
            on: std::cell::Cell::new((None, 0)),
            accent: 0,
            was_carried: Default::default(),
            all_fonts: FONTS.iter().map(|(_, t, d)| Font::load(&[t]).zip(Font::load(&[d]))).collect(),
            styled: 0,
            last_rect: Default::default(),
        };
        clock.refresh();
        clock
    }

    /// Brings it up to the minute; returns whether it changed.
    pub fn refresh(&mut self) -> bool {
        let (st, styled) = style();
        let Some((bold, medium)) = self.all_fonts.get(st.font).and_then(|f| f.as_ref()).or(self.fonts.as_ref()) else { return false };
        let now = local_time();
        let minute = now.tm_hour * 60 + now.tm_min;
        // The accent or the look changed: drawn again in it.
        let accent = crate::accent::version();
        if minute == self.minute && accent == self.accent && styled == self.styled {
            return false;
        }
        if accent != self.accent || styled != self.styled {
            self.accent = accent;
            self.styled = styled;
            let mut c = crate::accent::get_f();
            c[3] = st.alpha;
            // A new font with the same colour: set_color clears the text, so
            // the lines are made again.
            self.time.set_color([c[0], c[1], c[2], c[3] - 0.001]);
            self.date.set_color([c[0], c[1], c[2], c[3] - 0.001]);
            self.time.set_color(c);
            self.date.set_color(c);
        }
        self.minute = minute;
        self.time.set(bold, &format!("{}:{:02}", now.tm_hour, now.tm_min));
        self.date.set(medium, &short_date(&now));
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
    /// `moved`: the picker's finger moving it (logical px); `alpha`: its
    /// brightness as the picker's slider has it.
    #[allow(clippy::too_many_arguments)]
    pub fn elements(&self, renderer: &mut GlesRenderer, panel: Option<usize>, frame_ns: u64, carried: bool, middle: Option<f64>, moved: (f64, f64), alpha_now: Option<f32>) -> Vec<ShellElement> {
        // Going along (with the dock, or carried with its desk by the
        // ribbon): on the panel it ends on as it arrives, not faded in there.
        if middle.is_some() {
            self.on.set((panel, 0));
        }
        if carried {
            self.was_carried.set(true);
        } else if self.was_carried.replace(false) {
            self.on.set((panel, 0));
        }
        let carried = carried || middle.is_some();
        let (was, since) = self.on.get();
        if !carried && panel != was {
            self.on.set((panel, if was.is_none() && since == 0 { 0 } else { frame_ns }));
        }
        let Some(panel) = panel else {
            self.last_rect.set(None);
            return Vec::new();
        };
        let since = self.on.get().1;
        let k = if carried || since == 0 { 1.0 } else { (frame_ns.saturating_sub(since) as f32 / FADE_NS as f32).clamp(0.0, 1.0) };
        let alpha = k * k * (3.0 - 2.0 * k);
        // The slider's brightness against the one it was drawn in.
        let alpha = alpha * alpha_now.map(|a| a / style().0.alpha.max(0.01)).unwrap_or(1.0);
        let rect = layout::panels()[panel];
        let mid = middle.map(|m| m.round() as i32).unwrap_or(rect.loc.x + rect.size.w / 2);
        let (fx, fy) = style().0.at.unwrap_or((1.0 - RIGHT as f64 / rect.size.w as f64, TOP));
        let top = (rect.size.h as f64 * fy + moved.1).round() as i32;
        let mut out = Vec::new();
        let mut y = top;
        // Its right edge where its look puts it (RIGHT in from the panel's).
        let end = mid - rect.size.w / 2 + (rect.size.w as f64 * fx + moved.0).round() as i32;
        // One line: the time, then the date, the two centred together.
        let line_w = self.time.extent.w + BETWEEN + self.date.extent.w;
        let mut x = end - line_w;
        let line_h = self.time.extent.h.max(self.date.extent.h) + GAP + if self.weather.extent.w > 0 { self.weather.extent.h + 4 } else { 0 };
        self.last_rect.set(Some(Rectangle::new(((end - line_w) as f64, top as f64).into(), (line_w as f64, line_h as f64).into())));
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
            let x = end - (self.weather.extent.w + icon_w);
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
