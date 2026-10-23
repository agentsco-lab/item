//! The keyboard of item's own: the tile under a window, as tall as its
//! rows (the windows on its panel are made shorter for it: state.rs's
//! `follow_keyboard`), drawn by the compositor in its own glass and typing
//! straight into the window above it (typing.rs, state.rs's `send_keys`:
//! key presses from the compositor's keyboard, Cyrillic through the
//! keymap's second group). The power key shows and hides it (input.rs);
//! the Agent entry brings it up under its terminal (its vibe layer, the
//! here). stevia, the port's keyboard, stays installed as the way back:
//! `item-switch keyboard stevia` (`choice`).
//!
//! Along its top a field: the words the microphone heard (voice.rs) wait
//! here, Enter sends them; later the suggestions. Under it the keys in
//! layers - letters (English, Russian without ъ and ё: ten or eleven to a
//! row), signs on two pages, and vibe: the agent's keys, Esc, Tab and
//! Shift+Tab, the arrows, Home and End, the pages, Ctrl and Alt that stick
//! for the next key, Ctrl+C, y, n and the slash. A layer is changed by its
//! key; the language by the globe; the microphone is held to talk.
//!
//! Editing by the finger: a held backspace repeats, after a while by the
//! word (Ctrl+W in a terminal, Ctrl+Backspace elsewhere); a swipe along the
//! space bar walks the cursor; a letter held opens a row of what else it
//! can be (the digit on the top row, ё and ъ in Russian), chosen by
//! sliding along the row. The keyboard types by key presses alone, so it
//! offers only what typing.rs can send as one: no accents, no dashes or
//! quotes of the printers (`every_key_and_alternative_has_a_key`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::{Logical, Physical, Point, Rectangle};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};
use crate::typing;

/// The field along its top.
const FIELD_H: f64 = 50.0;
/// The keys: the tile's side padding, the gap between keys and rows, a
/// key's height, where the rows begin, the room under the last.
const SIDE: f64 = 12.0;
/// The frame turned (layout.rs): the keys split in two with this gap in the
/// middle, each half under its thumb; a flex key (the space) bridges it.
const SPLIT: f64 = 140.0;
const GAP: f64 = 6.0;
const KEY_H: f64 = 52.0;
const ROW_GAP: f64 = 8.0;
const ROWS_Y: f64 = FIELD_H + 12.0;
const ROWS: f64 = 4.0;
const BOTTOM: f64 = 14.0;
/// The tile's height: the field and the four rows every layer has.
pub const TILE_H: i32 = (ROWS_Y + ROWS * KEY_H + (ROWS - 1.0) * ROW_GAP + BOTTOM) as i32;
/// A touch that moves this far presses nothing.
const MOVE: f64 = 14.0;
/// A microphone let go sooner than this said nothing.
const TOO_SHORT_S: f64 = 0.4;
/// The glass: the tile's dark over the wallpaper.
const GLASS: [f32; 4] = [0.07, 0.08, 0.11, 0.92];
/// The tile's top corners' radius.
pub const ROUND: f32 = 18.0;
/// The tile's dark tint, its share over the blurred screen (tile.frag).
pub const TINT: f32 = 0.62;
/// The rise: a spring, a little over the top and back (omega rad/s, the
/// damping ratio), done in about a quarter second.
const RISE_OMEGA: f64 = 24.0;
const RISE_ZETA: f64 = 0.78;
/// The key under the finger shown big above it: the popup's height and
/// how much wider than the key it is.
const POP_H: f64 = 62.0;
const POP_WIDER: f64 = 24.0;
const POP_ABOVE: f64 = 8.0;
/// A held backspace: the first repeat after this, then one every
/// REPEAT_NS; after HOLD_WORD_NS whole words, one every WORD_NS.
const REPEAT_AFTER_NS: u64 = 400_000_000;
const REPEAT_NS: u64 = 60_000_000;
const HOLD_WORD_NS: u64 = 1_500_000_000;
const WORD_NS: u64 = 180_000_000;
/// A letter held this long opens its row of alternatives.
const HOLD_ALTS_NS: u64 = 350_000_000;
/// The least width of a cell in the row of alternatives.
const ALT_W: f64 = 46.0;
/// A swipe along the space bar: one cursor step per this many px.
const CURSOR_STEP: f64 = 18.0;

/// Which keyboard the phone uses: item's own, or the port's stevia
/// (`~/.config/item/keyboard`: "item" or "stevia"; item-switch writes it).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Choice {
    Item,
    Stevia,
}

pub fn choice() -> Choice {
    let path = format!("{}/.config/item/keyboard", std::env::var("HOME").unwrap_or_default());
    match std::fs::read_to_string(path).unwrap_or_default().trim() {
        "stevia" => Choice::Stevia,
        _ => Choice::Item,
    }
}

/// Whether an app is a terminal: the keyboard comes up with the agent's
/// keys for one.
pub fn is_terminal(app_id: &str) -> bool {
    matches!(app_id, "org.gnome.Console" | "foot" | "Alacritty" | "kitty" | "org.gnome.Terminal" | "item-agent")
}

/// Whether an app is a browser: addresses and mail are typed there, so the
/// letters' bottom row gets @ and the signs row .com (the whole window,
/// whatever the field; the field's own kind is not told to the compositor).
pub fn is_browser(app_id: &str) -> bool {
    matches!(app_id, "org.gnome.Epiphany" | "firefox" | "org.mozilla.firefox" | "chromium" | "chromium-browser" | "org.chromium.Chromium")
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Layer {
    Letters,
    /// Its page: digits and the common signs, or the rest.
    Signs(u8),
    Vibe,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    En,
    Ru,
}

/// A key: what it does.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Key {
    /// Typed as it is (shifted: its upper case).
    Char(char),
    /// Typed as it is, a word (.com).
    Text(&'static str),
    /// A key press, with its own modifiers.
    Code(typing::Key),
    Shift,
    Delete,
    Space,
    Enter,
    Layer(Layer),
    Lang,
    Mic,
    /// Stick for the next key.
    Ctrl,
    Alt,
}

/// A key in a row: what it is, its name, its width in units of a plain
/// key (0: the space, taking what is left).
struct Slot {
    key: Key,
    label: String,
    w: f64,
}

fn slot(key: Key, label: &str, w: f64) -> Slot {
    Slot { key, label: label.to_owned(), w }
}

fn chars(row: &str) -> Vec<Slot> {
    row.chars().map(|c| slot(Key::Char(c), &c.to_string(), 1.0)).collect()
}

const EN: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const RU: [&str; 3] = ["йцукенгшщзх", "фывапролджэ", "ячсмитьбю"];
const SIGNS_0: [&str; 3] = ["1234567890", "-/:;()$&@\"", ".,?!'`"];
/// The second page is short of its row: the currency signs and the middle
/// dot have no key on the US layout (typing.rs), so they are not offered.
const SIGNS_1: [&str; 3] = ["[]{}#%^*+=", "_\\|~<>", ".,?!'`"];

/// What the keyboard asks the state to do.
pub enum Ask {
    /// Words typed into the window above it.
    Text(String),
    /// A key pressed on it.
    Key(typing::Key),
}

struct Touch {
    slot: TouchSlot,
    /// The key under it (an index into `placed`, checked at each use: the
    /// rows may be laid again under it), or the field's clear.
    on: Option<usize>,
    /// The recording is this finger's: its lift sends it to be heard.
    mic: bool,
    start: Point<f64, Logical>,
    moved: bool,
    /// When it landed, and when a held backspace fires next.
    at: u64,
    next: u64,
    /// A held backspace fired: the lift does nothing more.
    fired: bool,
    /// Along the space bar: the cursor steps sent (right positive).
    cursor: Option<i32>,
    /// The row of what else the held letter can be, and the one under the
    /// finger.
    alts: Option<(Vec<char>, usize)>,
}

/// What else a letter can be, held: the top row's digit; the Russian
/// letters left off the rows; the rest of the punctuation. Only what
/// typing::key_of can send as a key press is here (the accents, the dashes
/// and the printers' quotes are not: the compositor has no way to type
/// them). Looked up by the lower case; the shift makes them upper case on
/// the way out.
fn alts_of(c: char, lang: Lang) -> Vec<char> {
    let c = c.to_lowercase().next().unwrap_or(c);
    let s = match (lang, c) {
        (Lang::En, 'q') => "1",
        (Lang::En, 'w') => "2",
        (Lang::En, 'e') => "3",
        (Lang::En, 'r') => "4",
        (Lang::En, 't') => "5",
        (Lang::En, 'y') => "6",
        (Lang::En, 'u') => "7",
        (Lang::En, 'i') => "8",
        (Lang::En, 'o') => "9",
        (Lang::En, 'p') => "0",
        (Lang::Ru, 'й') => "1",
        (Lang::Ru, 'ц') => "2",
        (Lang::Ru, 'у') => "3",
        (Lang::Ru, 'к') => "4",
        (Lang::Ru, 'е') => "5ё",
        (Lang::Ru, 'н') => "6",
        (Lang::Ru, 'г') => "7",
        (Lang::Ru, 'ш') => "8",
        (Lang::Ru, 'щ') => "9",
        (Lang::Ru, 'з') => "0",
        (Lang::Ru, 'ь') => "ъ",
        (_, '.') => "!?",
        (_, ',') => ";:",
        (_, '\'') => "\"",
        _ => "",
    };
    s.chars().collect()
}

const CLEAR: usize = usize::MAX;

/// A key as placed on the tile: its rectangle (the tile's coordinates),
/// what it is, its name, whether it stands out (Enter, a stuck modifier,
/// the shift on).
struct Placed {
    rect: Rectangle<f64, Logical>,
    key: Key,
    label: String,
    lit: bool,
}

pub struct Keyboard {
    /// The panel it is on, or coming up or going down on.
    panel: Option<usize>,
    /// Wanted up (the target of the rise).
    up: bool,
    /// How far up it is: 0 below the screen's bottom, 1 in place; a spring
    /// (settle), a little over 1 on the way up.
    rise: f64,
    rise_v: f64,
    rise_at: u64,
    /// The window typed into is a browser: @ on the bottom row, .com on the signs;
    /// a terminal: a word is deleted with Ctrl+W.
    web: bool,
    terminal: bool,
    /// What the finger asked for between lifts: cursor steps, repeats
    /// (`asks`).
    queue: Vec<Ask>,
    pub layer: Layer,
    lang: Lang,
    /// The languages the language key goes through (prefs.rs).
    langs: Vec<Lang>,
    shift: bool,
    ctrl: bool,
    alt: bool,
    /// The words waiting in the field.
    field: String,
    status: String,
    status_at: u64,
    /// The fingers down on it, one a slot: the split deck keyboard is for
    /// two thumbs, the second down before the first is up.
    touches: Vec<Touch>,
    placed: Vec<Placed>,
    recording: Option<crate::voice::Recording>,
    results: Receiver<Result<String, String>>,
    done: Sender<Result<String, String>>,
    wake: Ping,
    font: Option<Font>,
    /// Slabs by their width (px), the plain and the pressed; the accent's
    /// for Enter, by the accent's version.
    slabs: RefCell<HashMap<i32, (MemoryRenderBuffer, MemoryRenderBuffer)>>,
    accent: RefCell<(u64, HashMap<i32, MemoryRenderBuffer>)>,
    /// The popups' slabs by width, in the accent; the chosen cell's.
    pops: RefCell<(u64, HashMap<i32, MemoryRenderBuffer>)>,
    picks: RefCell<HashMap<i32, MemoryRenderBuffer>>,
    labels: RefCell<HashMap<(String, u8), Label>>,
    field_bg: MemoryRenderBuffer,
    id: Id,
}

impl Keyboard {
    pub fn new(wake: Ping) -> Keyboard {
        let (done, results) = channel();
        let font = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let mut k = Keyboard {
            panel: None,
            up: false,
            rise: 0.0,
            rise_v: 0.0,
            rise_at: 0,
            web: false,
            terminal: false,
            queue: Vec::new(),
            layer: Layer::Letters,
            lang: Lang::En,
            langs: vec![Lang::En, Lang::Ru],
            shift: false,
            ctrl: false,
            alt: false,
            field: String::new(),
            status: String::new(),
            status_at: 0,
            touches: Vec::new(),
            placed: Vec::new(),
            recording: None,
            results,
            done,
            wake,
            font,
            slabs: RefCell::new(HashMap::new()),
            accent: RefCell::new((0, HashMap::new())),
            pops: RefCell::new((0, HashMap::new())),
            picks: RefCell::new(HashMap::new()),
            labels: RefCell::new(HashMap::new()),
            field_bg: crate::grid::rounded(layout::panels()[0].size.w as f64 - 2.0 * SIDE, FIELD_H - 12.0, 12.0, [255, 255, 255, 18]),
            id: Id::new(),
        };
        k.lay();
        k
    }

    // ---- the rows -------------------------------------------------------------

    /// The language key, with more than one language to go through.
    fn lang_slot(&self) -> Option<Slot> {
        (self.langs.len() > 1).then(|| slot(Key::Lang, if self.lang == Lang::En { "en" } else { "ру" }, 1.0))
    }

    /// The languages from the preferences (prefs.rs): the one typed in
    /// stays if it is among them, else the first.
    pub fn set_langs(&mut self, langs: Vec<Lang>) {
        if langs == self.langs || langs.is_empty() {
            return;
        }
        self.langs = langs;
        if !self.langs.contains(&self.lang) {
            self.lang = self.langs[0];
        }
        self.lay();
    }

    fn rows(&self) -> Vec<Vec<Slot>> {
        // Shifted, a letter's key stays its lower-case self (the
        // alternatives are looked up by it, `up` makes the upper case, and
        // the keys stay the same under a thumb when the other toggles the
        // shift) and only its name is upper.
        let keyed = |row: &str| -> Vec<Slot> {
            row.chars().map(|c| slot(Key::Char(c), &if self.shift { c.to_uppercase().collect::<String>() } else { c.to_string() }, 1.0)).collect()
        };
        let letters = |rows: &[&str; 3], ends: f64| -> Vec<Vec<Slot>> {
            let mut r: Vec<Vec<Slot>> = rows.iter().map(|row| keyed(row)).collect();
            r[2].insert(0, slot(Key::Shift, "shift", ends));
            r[2].push(slot(Key::Delete, "delete", ends));
            r
        };
        let bottom = |first: Slot, second: Slot, dot: char| -> Vec<Slot> {
            let mut row = vec![first, second];
            row.extend(self.lang_slot());
            row.extend([slot(Key::Space, "", 0.0), slot(Key::Char(dot), &dot.to_string(), 1.0), slot(Key::Mic, "mic", 1.0), slot(Key::Enter, "enter", 1.6)]);
            row
        };
        let mut rows = match self.layer {
            Layer::Letters => match self.lang {
                Lang::En => letters(&EN, 1.5),
                Lang::Ru => letters(&RU, 1.0),
            },
            Layer::Signs(page) => {
                let src = if page == 0 { &SIGNS_0 } else { &SIGNS_1 };
                let mut r: Vec<Vec<Slot>> = src.iter().map(|row| chars(row)).collect();
                r[2].insert(0, slot(Key::Layer(Layer::Signs(1 - page)), if page == 0 { "#+=" } else { "123" }, 1.5));
                r[2].push(slot(Key::Delete, "delete", 1.5));
                r
            }
            Layer::Vibe => {
                use typing as t;
                vec![
                    vec![slot(Key::Code(t::ESC_KEY), "esc", 2.0), slot(Key::Code(t::TAB_KEY), "tab", 2.0), slot(Key::Code(t::SHIFT_TAB), "shift tab", 2.0), slot(Key::Ctrl, "ctrl", 2.0), slot(Key::Alt, "alt", 2.0)],
                    vec![slot(Key::Code(t::LEFT_KEY), "left", 2.0), slot(Key::Code(t::UP_KEY), "up", 2.0), slot(Key::Code(t::RIGHT_KEY), "right", 2.0), slot(Key::Code(t::PAGE_UP_KEY), "page up", 2.0), slot(Key::Code(t::CTRL_C), "ctrl c", 2.0)],
                    vec![slot(Key::Code(t::HOME_KEY), "home", 2.0), slot(Key::Code(t::DOWN_KEY), "down", 2.0), slot(Key::Code(t::END_KEY), "end", 2.0), slot(Key::Code(t::PAGE_DOWN_KEY), "page down", 2.0), slot(Key::Delete, "delete", 2.0)],
                ]
            }
        };
        rows.push(match self.layer {
            Layer::Letters if self.web => vec![
                slot(Key::Layer(Layer::Signs(0)), "123", 1.2),
                self.lang_slot().unwrap_or_else(|| slot(Key::Char('-'), "-", 1.0)),
                slot(Key::Char('@'), "@", 1.0),
                slot(Key::Space, "", 0.0),
                slot(Key::Layer(Layer::Vibe), "vibe", 1.3),
                slot(Key::Char('.'), ".", 1.0),
                slot(Key::Mic, "mic", 1.0),
                slot(Key::Enter, "enter", 1.5),
            ],
            Layer::Letters => bottom(slot(Key::Layer(Layer::Signs(0)), "123", 1.3), slot(Key::Layer(Layer::Vibe), "vibe", 1.3), '.'),
            // Addresses: ".com" lives on the signs layer (the owner's word,
            // 2026-10-23: the arrows beat it on the letters row).
            Layer::Signs(_) if self.web => {
                let mut row = bottom(slot(Key::Layer(Layer::Letters), "abc", 1.3), slot(Key::Layer(Layer::Vibe), "vibe", 1.3), ',');
                let after_space = row.iter().position(|s| s.key == Key::Space).map_or(row.len(), |i| i + 1);
                row.insert(after_space, slot(Key::Text(".com"), ".com", 1.3));
                row
            }
            Layer::Signs(_) => bottom(slot(Key::Layer(Layer::Letters), "abc", 1.3), slot(Key::Layer(Layer::Vibe), "vibe", 1.3), ','),
            Layer::Vibe => vec![
                slot(Key::Layer(Layer::Letters), "abc", 1.3),
                slot(Key::Layer(Layer::Signs(0)), "123", 1.3),
                slot(Key::Char('y'), "y", 1.0),
                slot(Key::Char('n'), "n", 1.0),
                slot(Key::Char('/'), "/", 1.0),
                slot(Key::Space, "", 0.0),
                slot(Key::Mic, "mic", 1.0),
                slot(Key::Enter, "enter", 1.6),
            ],
        });
        rows
    }

    /// The keys placed on the tile for the layer, language and shift of
    /// now.
    fn lay(&mut self) {
        let split = if layout::turned() { SPLIT } else { 0.0 };
        let w = layout::panels()[0].size.w as f64 - split;
        let rows = self.rows();
        // A plain key's width: the fullest row of plain keys sets it (ten
        // in English, eleven in Russian).
        let n = rows.iter().map(|r| r.iter().map(|s| s.w).sum::<f64>()).fold(0.0, f64::max).max(10.0);
        let unit = (w - 2.0 * SIDE - (n - 1.0) * GAP) / n;
        let mut placed = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let y = ROWS_Y + i as f64 * (KEY_H + ROW_GAP);
            let fixed: f64 = row.iter().map(|s| s.w * unit).sum::<f64>() + (row.len() as f64 - 1.0) * GAP;
            let flex = row.iter().filter(|s| s.w == 0.0).count();
            let (mut x, rest) = if flex > 0 { (SIDE, (w - 2.0 * SIDE - fixed) / flex as f64) } else { ((w - fixed) / 2.0, 0.0) };
            // Split: the row's second half over by the gap; a flex key last
            // in the first half spans it.
            let half = (row.len() + 1) / 2;
            for (j, s) in row.iter().enumerate() {
                let kw = if s.w == 0.0 { rest } else { s.w * unit };
                let (kx, kw_drawn) = (if j >= half { x + split } else { x }, if s.w == 0.0 && j + 1 == half { kw + split } else { kw });
                let lit = match s.key {
                    Key::Enter => true,
                    Key::Shift => self.shift,
                    Key::Ctrl => self.ctrl,
                    Key::Alt => self.alt,
                    _ => false,
                };
                placed.push(Placed { rect: Rectangle::new((kx, y).into(), (kw_drawn, KEY_H).into()), key: s.key, label: s.label.clone(), lit });
                x += kw + GAP;
            }
        }
        // A finger down stays on its key where the key is the same in the
        // new rows (the shift toggled under the other thumb, the frame
        // turned), and is let go where the rows changed under it (the
        // layer, the language, the languages): its index means nothing in
        // the new rows. Let go of the microphone, the recording is dropped.
        let same = |t: &Touch| t.on.is_none_or(|i| i == CLEAR || (i < placed.len() && self.placed.get(i).map(|p| p.key) == Some(placed[i].key)));
        let mut mic_gone = false;
        self.touches.retain(|t| {
            let keep = same(t);
            mic_gone |= !keep && t.mic;
            keep
        });
        if mic_gone {
            if let Some(r) = self.recording.take() {
                let _ = r.stop();
            }
        }
        self.placed = placed;
    }

    // ---- shown or not ---------------------------------------------------------

    /// On a panel, in a layer; or away. From away it rises (settle);
    /// going away it drops, and is on its panel until it is down. From
    /// one panel to the other it just moves.
    pub fn show(&mut self, panel: Option<usize>, layer: Option<Layer>) {
        // The frame turned (layout.rs): the lower panel is the deck, the
        // keyboard's whichever window it types into.
        let panel = panel.map(|p| if layout::turned() { 1 } else { p });
        if panel.is_some() != self.up || panel.is_some_and(|p| self.panel != Some(p)) {
            tracing::info!("keyboard: {}", match panel { Some(0) => "on the left panel", Some(_) => "on the right panel", None => "away" });
        }
        // The layer asked for is taken as the keyboard comes up; up already,
        // it stays as it was left - one keyboard across the windows (the
        // owner's word, 2026-10-22), not one that flips with each app.
        if let Some(l) = layer.filter(|_| !self.up) {
            self.layer = l;
            self.shift = false;
            self.lay();
        }
        match panel {
            Some(p) => {
                self.panel = Some(p);
                if !self.up {
                    self.up = true;
                    self.rise_at = hybris_hwc::now_ns();
                    self.lay();
                    self.wake.ping();
                }
            }
            None => {
                self.touches.clear();
                if let Some(r) = self.recording.take() {
                    let _ = r.stop();
                }
                // The modifiers stuck for the next key are not kept for
                // the next time it is up.
                if self.ctrl || self.alt {
                    self.ctrl = false;
                    self.alt = false;
                    self.lay();
                }
                if self.up {
                    self.up = false;
                    self.rise_at = hybris_hwc::now_ns();
                    self.wake.ping();
                }
            }
        }
    }

    /// What the window typed into is (state.rs tells it): a browser's
    /// bottom row changes for addresses; a terminal deletes words its way.
    pub fn set_window(&mut self, web: bool, terminal: bool) {
        self.terminal = terminal;
        if web != self.web {
            self.web = web;
            // Up, the keys stay as they are (one keyboard across the windows);
            // the browser's row comes with the next rise.
            if !self.up {
                self.lay();
            }
        }
    }

    /// Wanted up: there, or on its way.
    pub fn out(&self) -> bool {
        self.up
    }

    /// The panel it is seen on: its own while it rises, stands or drops.
    pub fn panel(&self) -> Option<usize> {
        self.panel
    }

    /// Whether it stands in place (the rise done).
    pub fn landed(&self) -> bool {
        self.up && self.rise_v == 0.0 && self.rise == 1.0
    }

    /// The height the windows on its panel give up for it: from the moment
    /// it starts up (state.rs's follow_keyboard; the window changes once,
    /// at the tap, and the tile rises over the strip it left - the glass
    /// under the tile is then the wallpaper's, still), none from the
    /// moment it starts down.
    pub fn cut(&self) -> Option<(usize, i32)> {
        if layout::turned() {
            return None;
        }
        self.panel.filter(|_| self.up).map(|p| (p, TILE_H))
    }

    /// How much of the deck the tile takes while up, turned or not: the
    /// flow's room stops above it (flow.rs). `cut` is for windows, and
    /// turned, windows give nothing up.
    pub fn deck_cut(&self) -> Option<i32> {
        self.up.then_some(TILE_H)
    }

    /// The keys laid again for the frame's width (the turn, layout.rs).
    pub fn relay(&mut self) {
        // The field's slab is the panel's width.
        self.field_bg = crate::grid::rounded(layout::panels()[0].size.w as f64 - 2.0 * SIDE, FIELD_H - 12.0, 12.0, [255, 255, 255, 18]);
        self.lay();
        if layout::turned() && self.up {
            self.panel = Some(1);
        }
    }

    /// How far below its place it is drawn now (logical px).
    fn drop_now(&self) -> f64 {
        (1.0 - self.rise) * TILE_H as f64
    }

    /// The tile's rectangle on screen as drawn now, with its left edge at
    /// `x` when the ribbon carries it (output.rs's glass under it).
    pub fn tile_rect(&self, x: Option<f64>) -> Option<Rectangle<i32, Logical>> {
        let panel = self.panel?;
        let r = Self::rect(panel);
        let x = x.unwrap_or(r.loc.x);
        Some(Rectangle::new((x.round() as i32, (r.loc.y + self.drop_now()).round() as i32).into(), (r.size.w.round() as i32, r.size.h.round() as i32).into()))
    }

    /// The tile's rectangle on the screen.
    fn rect(panel: usize) -> Rectangle<f64, Logical> {
        let r = layout::panels()[panel];
        Rectangle::new((r.loc.x as f64, (r.loc.y + r.size.h - TILE_H) as f64).into(), (r.size.w as f64, TILE_H as f64).into())
    }

    fn field_rect() -> Rectangle<f64, Logical> {
        Rectangle::new((SIDE, 6.0).into(), (layout::panels()[0].size.w as f64 - 2.0 * SIDE, FIELD_H - 12.0).into())
    }

    fn say(&mut self, text: &str) {
        self.status = text.to_owned();
        self.status_at = hybris_hwc::now_ns();
    }

    // ---- touch ------------------------------------------------------------------

    /// A finger down while the keyboard is on a panel: on the tile it is the
    /// keyboard's. False: not the keyboard's.
    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, _time_us: u64) -> bool {
        let Some(panel) = self.panel.filter(|_| self.up && self.rise > 0.9) else { return false };
        let rect = Self::rect(panel);
        if !rect.contains(pos) {
            return false;
        }
        // The slot down already (its lift lost): the new finger takes it.
        if let Some(old) = self.touches.iter().position(|t| t.slot == slot) {
            if self.touches.remove(old).mic {
                if let Some(r) = self.recording.take() {
                    let _ = r.stop();
                }
            }
        }
        let local = pos - rect.loc;
        let on = self.placed.iter().position(|p| p.rect.contains(local)).or_else(|| {
            let f = Self::field_rect();
            (!self.field.is_empty() && f.contains(local) && local.x > f.loc.x + f.size.w - 80.0).then_some(CLEAR)
        });
        let key = on.and_then(|i| self.placed.get(i)).map(|p| p.key);
        let mut mic = false;
        if key == Some(Key::Mic) && self.recording.is_none() {
            match crate::voice::record(&crate::voice::config()) {
                Ok(r) => {
                    self.recording = Some(r);
                    mic = true;
                    self.say("listening");
                }
                Err(e) => self.say(&e),
            }
        }
        if let Some(key) = key {
            crate::haptic::tick(match key {
                Key::Enter | Key::Layer(_) | Key::Lang | Key::Shift | Key::Ctrl | Key::Alt => crate::haptic::Tick::Firm,
                _ => crate::haptic::Tick::Light,
            });
        }
        let now = hybris_hwc::now_ns();
        self.touches.push(Touch { slot, on, mic, start: pos, moved: false, at: now, next: now + REPEAT_AFTER_NS, fired: false, cursor: None, alts: None });
        true
    }

    /// What the finger asked for since the last call, without a lift: the
    /// cursor's steps, a held backspace's repeats.
    pub fn asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.queue)
    }

    /// The fingers still down, time passing: a held backspace repeats, a
    /// held letter opens its row (`asks` carries the keys out), each for
    /// the finger that holds it.
    pub fn hold(&mut self, now: u64) {
        let terminal = self.terminal;
        let lang = self.lang;
        for t in self.touches.iter_mut() {
            let Some(i) = t.on.filter(|&i| i != CLEAR && !t.moved) else { continue };
            let Some(key) = self.placed.get(i).map(|p| p.key) else { continue };
            match key {
                Key::Delete => {
                    while now >= t.next {
                        t.fired = true;
                        if now.saturating_sub(t.at) >= HOLD_WORD_NS {
                            self.queue.push(Ask::Key(if terminal { typing::CTRL_W } else { typing::CTRL_BACKSPACE }));
                            crate::haptic::tick(crate::haptic::Tick::Light);
                            t.next += WORD_NS;
                        } else {
                            self.queue.push(Ask::Key(typing::BACKSPACE_KEY));
                            t.next += REPEAT_NS;
                        }
                    }
                }
                Key::Char(c) if t.alts.is_none() && now.saturating_sub(t.at) >= HOLD_ALTS_NS => {
                    let alts = alts_of(c, lang);
                    if !alts.is_empty() {
                        t.alts = Some((alts, 0));
                        crate::haptic::tick(crate::haptic::Tick::Firm);
                    }
                }
                _ => {}
            }
        }
    }

    /// The row of alternatives over the key `p`: its left edge and its
    /// cells' width (the tile's coordinates).
    fn alts_row(p: &Placed, n: usize) -> (f64, f64) {
        let cw = p.rect.size.w.max(ALT_W);
        let w = cw * n as f64;
        let x = (p.rect.loc.x + (p.rect.size.w - cw) / 2.0).clamp(SIDE / 2.0, (Self::rect(0).size.w - w - SIDE / 2.0).max(SIDE / 2.0));
        (x, cw)
    }

    pub fn holds(&self, slot: TouchSlot) -> bool {
        self.touches.iter().any(|t| t.slot == slot)
    }

    pub fn motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, _time_us: u64) {
        let Some(t) = self.touches.iter_mut().find(|t| t.slot == slot) else { return };
        let key = t.on.and_then(|i| self.placed.get(i)).map(|p| p.key);
        if key == Some(Key::Mic) {
            return;
        }
        let (dx, dy) = (pos.x - t.start.x, pos.y - t.start.y);
        // The row of alternatives open: the finger slides along it.
        if let (Some(p), Some((alts, picked))) = (t.on.and_then(|i| self.placed.get(i)), t.alts.as_mut()) {
            let (x, cw) = Self::alts_row(p, alts.len());
            let local_x = pos.x - self.panel.map(|p| Self::rect(p).loc.x).unwrap_or(0.0);
            let now = (((local_x - x) / cw).floor().max(0.0) as usize).min(alts.len() - 1);
            if now != *picked {
                *picked = now;
                crate::haptic::tick(crate::haptic::Tick::Light);
            }
            return;
        }
        // Along the space bar: the cursor walks.
        if key == Some(Key::Space) && (t.cursor.is_some() || dx.abs() > MOVE) {
            let steps = (dx / CURSOR_STEP).trunc() as i32;
            let sent = t.cursor.get_or_insert(0);
            while *sent < steps {
                self.queue.push(Ask::Key(typing::RIGHT_KEY));
                crate::haptic::tick(crate::haptic::Tick::Light);
                *sent += 1;
            }
            while *sent > steps {
                self.queue.push(Ask::Key(typing::LEFT_KEY));
                crate::haptic::tick(crate::haptic::Tick::Light);
                *sent -= 1;
            }
            return;
        }
        if dx.abs() > MOVE || dy.abs() > MOVE {
            t.moved = true;
            t.on = None;
        }
    }

    /// Let go: the key under the finger does its thing.
    pub fn up(&mut self, slot: TouchSlot) -> Option<Ask> {
        let at = self.touches.iter().position(|t| t.slot == slot)?;
        let t = self.touches.remove(at);
        let i = t.on?;
        if i == CLEAR {
            self.field.clear();
            return None;
        }
        // The rows laid again under the finger (lay): nothing.
        let key = self.placed.get(i)?.key;
        if key == Key::Mic {
            // Another finger's recording is not this one's to send.
            if !t.mic {
                return None;
            }
            let r = self.recording.take()?;
            let secs = r.seconds();
            let pcm = r.stop();
            if secs < TOO_SHORT_S || pcm.len() < 2000 {
                self.say("hold it while you speak");
                return None;
            }
            self.say("listening\u{2026}");
            crate::voice::transcribe(pcm, crate::voice::config(), self.done.clone(), self.wake.clone());
            return None;
        }
        if t.moved || t.fired || t.cursor.is_some() {
            return None;
        }
        // The one chosen from the row of alternatives.
        if let Some((alts, picked)) = t.alts {
            let c = alts[picked];
            let c = if self.shift { c.to_uppercase().next().unwrap_or(c) } else { c };
            self.shift = false;
            self.ctrl = false;
            self.alt = false;
            self.lay();
            return Some(Ask::Text(c.to_string()));
        }
        // The modifiers stuck for this key.
        let (ctrl, alt) = (self.ctrl, self.alt);
        let with = |mut k: typing::Key| {
            k.ctrl |= ctrl;
            k.alt |= alt;
            k
        };
        let ask = match key {
            Key::Char(c) => {
                let c = if self.shift { c.to_uppercase().next().unwrap_or(c) } else { c };
                if ctrl || alt {
                    typing::key_of(c).map(|k| Ask::Key(with(k)))
                } else {
                    Some(Ask::Text(c.to_string()))
                }
            }
            Key::Text(t) => Some(Ask::Text(t.to_owned())),
            Key::Code(k) => Some(Ask::Key(with(k))),
            Key::Delete => Some(Ask::Key(with(typing::BACKSPACE_KEY))),
            Key::Space => Some(Ask::Key(with(typing::SPACE_KEY))),
            Key::Enter => {
                if self.field.is_empty() {
                    Some(Ask::Key(with(typing::ENTER_KEY)))
                } else {
                    let mut text = std::mem::take(&mut self.field);
                    text.push('\n');
                    Some(Ask::Text(text))
                }
            }
            Key::Shift => {
                self.shift = !self.shift;
                None
            }
            Key::Layer(l) => {
                self.layer = l;
                self.shift = false;
                None
            }
            Key::Lang => {
                let i = self.langs.iter().position(|l| *l == self.lang).unwrap_or(0);
                self.lang = self.langs[(i + 1) % self.langs.len()];
                self.shift = false;
                None
            }
            Key::Ctrl => {
                self.ctrl = !self.ctrl;
                None
            }
            Key::Alt => {
                self.alt = !self.alt;
                None
            }
            Key::Mic => None,
        };
        // A character typed takes the shift off, and the stuck modifiers
        // once used.
        if matches!(key, Key::Char(_) | Key::Text(_) | Key::Code(_) | Key::Delete | Key::Space | Key::Enter) {
            if self.shift && matches!(key, Key::Char(_)) {
                self.shift = false;
            }
            self.ctrl = false;
            self.alt = false;
        }
        self.lay();
        ask
    }

    /// The touches taken away (the turn's synthetic cancel, libinput's):
    /// they do nothing - no key, no shift or layer or language toggled,
    /// the field kept; a recording under way is dropped, not sent to be
    /// heard.
    pub fn cancel(&mut self) {
        self.touches.clear();
        if let Some(r) = self.recording.take() {
            let _ = r.stop();
            self.status.clear();
        }
    }

    /// The words that came back from the microphone: into the field.
    pub fn poll(&mut self) {
        match self.results.try_recv() {
            Ok(Ok(text)) if text.is_empty() => self.say("nothing heard"),
            Ok(Ok(text)) => {
                if !self.field.is_empty() {
                    self.field.push(' ');
                }
                self.field.push_str(&text);
                self.status.clear();
            }
            Ok(Err(e)) => self.say(&e),
            Err(_) => {}
        }
    }

    /// After a frame: whether it still moves - the rise or the drop, a
    /// spring; listening, the mic pulses.
    pub fn settle(&mut self, frame_ns: u64) -> bool {
        let target = if self.up { 1.0 } else { 0.0 };
        let still = self.rise == target && self.rise_v == 0.0;
        if !still {
            let dt = (frame_ns.saturating_sub(self.rise_at) as f64 / 1e9).clamp(0.0, 0.05);
            self.rise_at = frame_ns;
            let a = -RISE_OMEGA * RISE_OMEGA * (self.rise - target) - 2.0 * RISE_ZETA * RISE_OMEGA * self.rise_v;
            self.rise_v += a * dt;
            self.rise += self.rise_v * dt;
            if (self.rise - target).abs() < 0.003 && self.rise_v.abs() < 0.08 {
                self.rise = target;
                self.rise_v = 0.0;
                if !self.up {
                    self.panel = None;
                }
                tracing::debug!("keyboard: {}", if self.up { "stands" } else { "down" });
            } else if !self.up && self.rise < 0.0 {
                self.rise = 0.0;
                self.rise_v = 0.0;
                self.panel = None;
            }
        }
        !still || self.recording.is_some() || self.touches.iter().any(|t| t.on.is_some())
    }

    // ---- drawing ----------------------------------------------------------------

    fn slab(&self, w: f64, pressed: bool) -> MemoryRenderBuffer {
        let px = w.round() as i32;
        let mut s = self.slabs.borrow_mut();
        let (plain, lit) = s.entry(px).or_insert_with(|| (crate::grid::rounded(px as f64, KEY_H, 12.0, [255, 255, 255, 30]), crate::grid::rounded(px as f64, KEY_H, 12.0, [255, 255, 255, 110])));
        if pressed { lit.clone() } else { plain.clone() }
    }

    fn accent_slab(&self, w: f64) -> MemoryRenderBuffer {
        let px = w.round() as i32;
        let mut a = self.accent.borrow_mut();
        if a.0 != crate::accent::version() {
            *a = (crate::accent::version(), HashMap::new());
        }
        a.1.entry(px).or_insert_with(|| crate::grid::rounded(px as f64, KEY_H, 12.0, crate::accent::get())).clone()
    }

    fn pop_slab(&self, w: f64) -> MemoryRenderBuffer {
        let px = w.round() as i32;
        let mut a = self.pops.borrow_mut();
        if a.0 != crate::accent::version() {
            *a = (crate::accent::version(), HashMap::new());
        }
        a.1.entry(px).or_insert_with(|| crate::grid::rounded(px as f64, POP_H, 14.0, crate::accent::get())).clone()
    }

    /// The cell under the finger in the row of alternatives: lighter.
    fn pick_slab(&self, cw: f64) -> MemoryRenderBuffer {
        let px = cw.round() as i32;
        self.picks.borrow_mut().entry(px).or_insert_with(|| crate::grid::rounded(cw - 6.0, POP_H - 8.0, 10.0, [255, 255, 255, 90])).clone()
    }

    fn label(&self, text: &str, size: u8) -> Option<Label> {
        let font = self.font.as_ref()?;
        let mut l = self.labels.borrow_mut();
        Some(
            l.entry((text.to_owned(), size))
                .or_insert_with(|| {
                    let mut label = Label::new(size as f32, [1.0, 1.0, 1.0, 1.0]);
                    label.set(font, text);
                    label
                })
                .clone(),
        )
    }

    /// The tile's keys and field, topmost first, with its left edge at `x`
    /// when the ribbon carries it along with its window's page (output.rs);
    /// with `backdrop`, the plain dark tile under them - without, the
    /// glass is output.rs's (tile_rect).
    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64, x: Option<f64>, backdrop: bool) -> Vec<ShellElement> {
        let Some(panel) = self.panel else { return Vec::new() };
        self.drawn_at(renderer, frame_ns, x.unwrap_or(Self::rect(panel).loc.x), backdrop)
    }

    fn drawn_at(&self, renderer: &mut GlesRenderer, frame_ns: u64, x: f64, backdrop: bool) -> Vec<ShellElement> {
        // Its own panel's rectangle: turned (layout.rs) the panels stand
        // one above the other, and the keyboard is on the lower.
        let own = Self::rect(self.panel.unwrap_or(0));
        let rect = Rectangle::new((x, own.loc.y + self.drop_now()).into(), own.size);
        let origin = rect.loc;
        let s = SCALE as f64;
        let mut out = Vec::new();
        let put = |out: &mut Vec<ShellElement>, renderer: &mut GlesRenderer, b: &MemoryRenderBuffer, x: f64, y: f64, alpha: f32| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, (((origin.x + x) * s).round(), ((origin.y + y) * s).round()), b, Some(alpha), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        // The first element is the topmost: the key under each finger big
        // above it, the field's words, each key's name over its slab (lit
        // for Enter and what is on or stuck; brighter under a finger),
        // the glass last.
        for t in &self.touches {
            let Some(p) = t.on.and_then(|i| self.placed.get(i)) else { continue };
            if let Some((alts, picked)) = t.alts.as_ref() {
                let (x, cw) = Self::alts_row(p, alts.len());
                let w = cw * alts.len() as f64;
                let py = p.rect.loc.y - POP_ABOVE - POP_H;
                for (j, c) in alts.iter().enumerate() {
                    let c = if self.shift { c.to_uppercase().next().unwrap_or(*c) } else { *c };
                    if let Some(l) = self.label(&c.to_string(), 28) {
                        put(&mut out, renderer, &l.buffer, x + cw * j as f64 + (cw - l.extent.w as f64) / 2.0, py + (POP_H - l.extent.h as f64) / 2.0, 1.0);
                    }
                }
                put(&mut out, renderer, &self.pick_slab(cw), x + cw * *picked as f64 + 3.0, py + 4.0, 1.0);
                put(&mut out, renderer, &self.pop_slab(w), x, py, 1.0);
            } else if matches!(p.key, Key::Char(_) | Key::Text(_)) {
                let w = p.rect.size.w + POP_WIDER;
                let px = (p.rect.loc.x - POP_WIDER / 2.0).clamp(SIDE / 2.0, rect.size.w - w - SIDE / 2.0);
                let py = p.rect.loc.y - POP_ABOVE - POP_H;
                let label = if let Key::Char(c) = p.key { if self.shift { c.to_uppercase().collect() } else { c.to_string() } } else { p.label.clone() };
                if let Some(l) = self.label(&label, 32) {
                    put(&mut out, renderer, &l.buffer, px + (w - l.extent.w as f64) / 2.0, py + (POP_H - l.extent.h as f64) / 2.0, 1.0);
                }
                put(&mut out, renderer, &self.pop_slab(w), px, py, 1.0);
            }
        }
        let f = Self::field_rect();
        let field_text = if !self.field.is_empty() {
            self.field.clone()
        } else if !self.status.is_empty() && frame_ns.saturating_sub(self.status_at) < 10_000_000_000 {
            self.status.clone()
        } else if self.recording.is_some() {
            "listening".to_owned()
        } else {
            String::new()
        };
        if !field_text.is_empty() {
            if let Some(l) = self.label(&field_text, 20) {
                put(&mut out, renderer, &l.buffer, f.loc.x + 16.0, f.loc.y + (f.size.h - l.extent.h as f64) / 2.0, if self.field.is_empty() { 0.6 } else { 1.0 });
            }
            if !self.field.is_empty() {
                if let Some(l) = self.label("clear", 16) {
                    put(&mut out, renderer, &l.buffer, f.loc.x + f.size.w - l.extent.w as f64 - 16.0, f.loc.y + (f.size.h - l.extent.h as f64) / 2.0, 0.7);
                }
            }
        }
        put(&mut out, renderer, &self.field_bg, f.loc.x, f.loc.y, 1.0);
        for (i, p) in self.placed.iter().enumerate() {
            let pressed = self.touches.iter().any(|t| t.on == Some(i));
            let size = if p.label.chars().count() == 1 { 24 } else { 16 };
            if let Some(l) = self.label(&p.label, size) {
                put(&mut out, renderer, &l.buffer, p.rect.loc.x + (p.rect.size.w - l.extent.w as f64) / 2.0, p.rect.loc.y + (p.rect.size.h - l.extent.h as f64) / 2.0, 1.0);
            }
            let slab = if p.key == Key::Enter || (p.lit && !pressed) { self.accent_slab(p.rect.size.w) } else { self.slab(p.rect.size.w, pressed) };
            let alpha = if p.key == Key::Mic && self.recording.is_some() { 0.5 + 0.5 * ((frame_ns as f64 / 1e9 * 2.4).sin() * 0.5 + 0.5) as f32 } else { 1.0 };
            put(&mut out, renderer, &slab, p.rect.loc.x, p.rect.loc.y, alpha);
        }
        if backdrop {
            let phys = Rectangle::<i32, Physical>::new(origin.to_physical(s).to_i32_round(), rect.size.to_physical(s).to_i32_round());
            out.push(ShellElement::Solid(SolidColorRenderElement::new(self.id.clone(), phys, CommitCounter::default(), GLASS, Kind::Unspecified)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the rows and the rows of alternatives offer, in
    /// both languages and in the upper case the shift makes, is one
    /// typing.rs can send as a key press: the compositor types by key
    /// presses alone, and a key without one would tick and type nothing.
    #[test]
    fn every_key_and_alternative_has_a_key() {
        let typeable = |c: char| typing::key_of(c).is_some() && c.to_uppercase().all(|u| typing::key_of(u).is_some());
        let rows = [(Lang::En, &EN), (Lang::Ru, &RU), (Lang::En, &SIGNS_0), (Lang::En, &SIGNS_1), (Lang::Ru, &SIGNS_0), (Lang::Ru, &SIGNS_1)];
        for (lang, rows) in rows {
            for c in rows.iter().flat_map(|r| r.chars()) {
                assert!(typeable(c), "{c:?} on the rows has no key");
                for a in alts_of(c, lang) {
                    assert!(typeable(a), "{a:?}, an alternative of {c:?}, has no key");
                }
            }
        }
        // The bottom rows' own characters (rows()).
        for c in ".,@-yn/".chars() {
            assert!(typeable(c), "{c:?} on the bottom row has no key");
            for a in alts_of(c, Lang::En).into_iter().chain(alts_of(c, Lang::Ru)) {
                assert!(typeable(a), "{a:?}, an alternative of {c:?}, has no key");
            }
        }
    }

    #[test]
    fn alternatives_are_found_by_the_lower_case() {
        assert_eq!(alts_of('E', Lang::En), alts_of('e', Lang::En));
        assert_eq!(alts_of('Е', Lang::Ru), vec!['5', 'ё']);
        assert!(alts_of('e', Lang::En).contains(&'3'));
    }
}
