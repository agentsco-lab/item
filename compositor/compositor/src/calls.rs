//! Calls over the lock screen, as phosh shows them: gnome-calls has the
//! modem and the call's own window; locked, the compositor shows a call
//! itself, over the lock screen, and answers it through gnome-calls.
//!
//! - gnome-calls (its daemon, started with the session) puts each call on
//!   the session bus under `/org/gnome/Calls` (an ObjectManager; each call
//!   `org.gnome.Calls.Call`: `State`, `Inbound`, `Id` the number,
//!   `DisplayName` the contact). A thread reads them again at every signal
//!   there, and when gnome-calls comes or goes.
//! - A call coming in lights the screen if it is dark and holds it lit while
//!   it rings; locked, the call screen shows: on the left who calls, on the
//!   right Answer and Decline. Answered, the time talked and Hang up. It goes
//!   when the call ends, back to the lock screen.
//! - Unlocked, gnome-calls shows the call itself (its notification and its
//!   window).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::backend::input::TouchSlot;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::reexports::calloop::ping::Ping;
use smithay::utils::{Logical, Physical, Point, Rectangle};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::layout::{self, SCALE};
use crate::shade::ShellElement;
use crate::text::{Font, Label};

const BUS: &str = "org.gnome.Calls";
const ROOT: &str = "/org/gnome/Calls";
const CALL: &str = "org.gnome.Calls.Call";

/// gnome-calls' call states (CuiCallState).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CallState {
    Active,
    Held,
    Dialing,
    Alerting,
    Incoming,
    Waiting,
    Gone,
}

impl CallState {
    fn from(n: u32) -> CallState {
        match n {
            1 => CallState::Active,
            2 => CallState::Held,
            3 => CallState::Dialing,
            4 => CallState::Alerting,
            5 => CallState::Incoming,
            6 => CallState::Waiting,
            _ => CallState::Gone,
        }
    }

    fn ringing(self) -> bool {
        matches!(self, CallState::Incoming | CallState::Waiting)
    }
}

#[derive(Clone, Debug)]
pub struct Call {
    path: OwnedObjectPath,
    pub state: CallState,
    /// The number, and the contact's name if gnome-calls knows it.
    pub number: String,
    pub name: String,
}

/// The calls gnome-calls has now.
fn read(conn: &zbus::blocking::Connection) -> Vec<Call> {
    type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
    let Ok(reply) = conn.call_method(Some(BUS), ROOT, Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &()) else {
        return Vec::new();
    };
    let Ok(objects) = reply.body().deserialize::<Objects>() else {
        return Vec::new();
    };
    let mut calls: Vec<Call> = objects
        .into_iter()
        .filter_map(|(path, ifaces)| {
            let p = ifaces.get(CALL)?;
            let s = |k: &str| p.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
            let state = p.get("State").and_then(|v| u32::try_from(v.try_clone().ok()?).ok()).unwrap_or(0);
            Some(Call { path, state: CallState::from(state), number: s("Id"), name: s("DisplayName") })
        })
        .filter(|c| c.state != CallState::Gone)
        .collect();
    calls.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    calls
}

/// A method of a call's, on a thread: gnome-calls answers when the modem
/// has.
fn act(path: OwnedObjectPath, method: &'static str) {
    std::thread::spawn(move || {
        if let Ok(conn) = zbus::blocking::Connection::session() {
            match conn.call_method(Some(BUS), path.as_str(), Some(CALL), method, &()) {
                Ok(_) => tracing::info!("calls: {method}"),
                Err(e) => tracing::warn!("calls: {method}: {e}"),
            }
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Button {
    Answer,
    Decline,
    HangUp,
}

const BUTTON: f64 = 88.0;
const BUTTONS_FROM_FOOT: f64 = 210.0;
const FADE_NS: f64 = 260e6;

pub struct Calls {
    calls: Arc<Mutex<Vec<Call>>>,
    /// Changed since last asked.
    changed: Arc<std::sync::atomic::AtomicBool>,
    /// A pretended call (CALL_TEST): gnome-calls' are not read.
    pretending: Arc<std::sync::atomic::AtomicBool>,
    /// The calls as last taken, and when the first went active.
    now: Vec<Call>,
    active_since: Option<u64>,
    /// The call screen is up (locked, a call); since when, or since when
    /// gone.
    shown: bool,
    since: u64,
    pressed: Option<(TouchSlot, Button)>,
    fonts: Option<(Font, Font)>,
    who: Label,
    number: Label,
    what: Label,
    words: [Label; 3],
    green: MemoryRenderBuffer,
    red: MemoryRenderBuffer,
    start_icon: Option<MemoryRenderBuffer>,
    stop_icon: Option<MemoryRenderBuffer>,
    dim: Id,
}

fn icon(names: &[&str]) -> Option<MemoryRenderBuffer> {
    names
        .iter()
        .flat_map(|n| ["actions", "status", "apps"].map(|d| format!("/usr/share/icons/Adwaita/symbolic/{d}/{n}.svg")))
        .find(|p| std::path::Path::new(p).exists())
        .and_then(|p| crate::lock::tinted(&p, 36, [255, 255, 255]))
}

impl Calls {
    pub fn new(wake: Ping) -> Calls {
        let calls: Arc<Mutex<Vec<Call>>> = Default::default();
        let changed: Arc<std::sync::atomic::AtomicBool> = Default::default();
        let pretending: Arc<std::sync::atomic::AtomicBool> = Default::default();
        // Read at every signal under /org/gnome/Calls...
        let (c, ch, w, p) = (calls.clone(), changed.clone(), wake.clone(), pretending.clone());
        let reload = Arc::new(move |conn: &zbus::blocking::Connection| {
            if p.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let now = read(conn);
            let mut calls = c.lock().unwrap();
            let differ = calls.len() != now.len() || calls.iter().zip(&now).any(|(a, b)| a.path != b.path || a.state != b.state || a.name != b.name);
            if differ {
                *calls = now;
                ch.store(true, std::sync::atomic::Ordering::Relaxed);
                w.ping();
            }
        });
        let r = reload.clone();
        std::thread::Builder::new()
            .name("calls".into())
            .spawn(move || {
                let Ok(conn) = zbus::blocking::Connection::session() else { return };
                let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).path_namespace(ROOT).map(|b| b.build());
                let Ok(rule) = rule else { return };
                let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(64)) else {
                    tracing::warn!("calls: no watch on gnome-calls");
                    return;
                };
                r(&conn);
                for _ in signals {
                    r(&conn);
                }
            })
            .expect("calls thread");
        // ...and when gnome-calls comes or goes.
        std::thread::Builder::new()
            .name("calls-owner".into())
            .spawn(move || {
                let Ok(conn) = zbus::blocking::Connection::session() else { return };
                let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
                let Ok(changes) = proxy.receive_name_owner_changed_with_args(&[(0, BUS)]) else { return };
                for _ in changes {
                    reload(&conn);
                }
            })
            .expect("calls owner thread");
        let thin = Font::load(&["/usr/share/fonts/truetype/lato/Lato-Light.ttf", "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"]);
        let regular = Font::load(&["/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", "/usr/share/fonts/truetype/lato/Lato-Regular.ttf"]);
        let word = || Label::new(16.0, [1.0, 1.0, 1.0, 0.85]);
        Calls {
            calls,
            changed,
            pretending,
            now: Vec::new(),
            active_since: None,
            shown: false,
            since: 0,
            pressed: None,
            fonts: thin.zip(regular),
            who: Label::new(44.0, [1.0, 1.0, 1.0, 0.97]),
            number: Label::new(20.0, [1.0, 1.0, 1.0, 0.7]),
            what: Label::new(18.0, [1.0, 1.0, 1.0, 0.85]),
            words: [word(), word(), word()],
            green: crate::grid::rounded(BUTTON, BUTTON, BUTTON / 2.0, [0x26, 0xa2, 0x69, 255]),
            red: crate::grid::rounded(BUTTON, BUTTON, BUTTON / 2.0, [0xc0, 0x1c, 0x28, 255]),
            start_icon: icon(&["call-start-symbolic"]),
            stop_icon: icon(&["call-stop-symbolic"]),
            dim: Id::new(),
        }
    }

    /// The call shown: one ringing first, else the first.
    fn call(&self) -> Option<&Call> {
        self.now.iter().find(|c| c.state.ringing()).or(self.now.first())
    }

    /// Any call at all.
    pub fn any(&self) -> bool {
        !self.now.is_empty()
    }

    /// A call is ringing.
    pub fn ringing(&self) -> bool {
        self.now.iter().any(|c| c.state.ringing())
    }

    /// The calls changed: taken; true if anything did. A call that starts
    /// ringing is told by `rang`.
    pub fn take(&mut self, now_ns: u64) -> Option<bool> {
        if !self.changed.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let was_ringing = self.ringing();
        self.now = self.calls.lock().unwrap().clone();
        let active = self.now.iter().any(|c| c.state == CallState::Active);
        match (active, self.active_since) {
            (true, None) => self.active_since = Some(now_ns),
            (false, Some(_)) => self.active_since = None,
            _ => {}
        }
        tracing::info!("calls: {}", self.now.iter().map(|c| format!("{:?}", c.state)).collect::<Vec<_>>().join(", "));
        self.words_for(now_ns);
        Some(!was_ringing && self.ringing())
    }

    /// Whether the call screen is up: locked, with a call.
    pub fn show(&mut self, locked: bool, now_ns: u64) {
        let want = (locked || std::env::var_os("CALL_UNLOCKED").is_some()) && !self.now.is_empty();
        if want != self.shown {
            self.shown = want;
            self.since = now_ns;
            self.pressed = None;
        }
    }

    pub fn holds_screen(&self) -> bool {
        self.shown
    }

    fn k(&self, frame_ns: u64) -> f64 {
        let t = ((frame_ns.saturating_sub(self.since)) as f64 / FADE_NS).min(1.0);
        crate::pinpad::ease(if self.shown { t } else { 1.0 - t })
    }

    /// The words: who, the number, what the call is doing.
    fn words_for(&mut self, now_ns: u64) -> bool {
        let Some((thin, regular)) = &self.fonts else { return false };
        let Some(call) = self.call().cloned() else { return false };
        let who = if call.name.is_empty() { if call.number.is_empty() { "Unknown".to_owned() } else { call.number.clone() } } else { call.name.clone() };
        let number = if call.name.is_empty() { String::new() } else { call.number.clone() };
        let what = match call.state {
            CallState::Incoming | CallState::Waiting => "Incoming call".to_owned(),
            CallState::Dialing | CallState::Alerting => "Calling…".to_owned(),
            CallState::Held => "On hold".to_owned(),
            _ => match self.active_since {
                Some(t) => {
                    let s = now_ns.saturating_sub(t) / 1_000_000_000;
                    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
                }
                None => String::new(),
            },
        };
        let mut changed = self.who.set(thin, &who);
        changed |= self.number.set(regular, &number);
        changed |= self.what.set(regular, &what);
        let ringing = call.state.ringing();
        for (l, w) in self.words.iter_mut().zip(if ringing { ["Decline", "Answer", ""] } else { ["", "", "Hang up"] }) {
            changed |= l.set(regular, w);
        }
        changed
    }

    /// Once a second: the time talked. True if the screen changed.
    pub fn tick(&mut self, now_ns: u64) -> bool {
        self.shown && self.active_since.is_some() && self.words_for(now_ns)
    }

    /// The buttons on the right panel, logical px.
    fn buttons(&self) -> Vec<(Button, Rectangle<f64, Logical>)> {
        let right = layout::panels()[1];
        let y = right.size.h as f64 - BUTTONS_FROM_FOOT;
        let x = |share: f64| right.loc.x as f64 + right.size.w as f64 * share - BUTTON / 2.0;
        let r = |share: f64| Rectangle::new((x(share), y).into(), (BUTTON, BUTTON).into());
        if self.call().is_some_and(|c| c.state.ringing()) {
            vec![(Button::Decline, r(0.28)), (Button::Answer, r(0.72))]
        } else {
            vec![(Button::HangUp, r(0.5))]
        }
    }

    pub fn down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>) {
        // A little more room than drawn, for a thumb.
        let hit = self.buttons().into_iter().find(|(_, r)| {
            let mut r = *r;
            r.loc -= (12.0, 12.0).into();
            r.size += (24.0, 24.0).into();
            r.contains(pos)
        });
        if let Some((b, _)) = hit {
            self.pressed = Some((slot, b));
        }
    }

    pub fn up(&mut self, slot: TouchSlot) {
        let Some((_, b)) = self.pressed.take_if(|(s, _)| *s == slot) else { return };
        let Some(call) = self.call() else { return };
        let path = call.path.clone();
        crate::fingerprint::buzz("button-pressed");
        match b {
            Button::Answer => act(path, "Accept"),
            Button::Decline | Button::HangUp => act(path, "Hangup"),
        }
    }

    pub fn settle(&self, frame_ns: u64) -> bool {
        (frame_ns as f64) < self.since as f64 + FADE_NS + 20e6
    }

    pub fn elements(&self, renderer: &mut GlesRenderer, frame_ns: u64) -> Vec<ShellElement> {
        let k = self.k(frame_ns);
        if k <= 0.0 {
            return Vec::new();
        }
        let alpha = k as f32;
        let s = SCALE as f64;
        let rise = 18.0 * (1.0 - k);
        let mut out = Vec::new();
        let mut put = |out: &mut Vec<ShellElement>, b: &MemoryRenderBuffer, x: f64, y: f64, a: f32| {
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, ((x * s).round(), ((y + rise) * s).round()), b, Some(a), None, None, Kind::Unspecified) {
                out.push(ShellElement::Text(e));
            }
        };
        // The left panel: what, who, the number.
        let left = layout::panels()[0];
        let cx = |w: i32| left.loc.x as f64 + (left.size.w - w) as f64 / 2.0;
        let mut y = 190.0;
        put(&mut out, &self.what.buffer, cx(self.what.extent.w), y, alpha * 0.9);
        y += self.what.extent.h as f64 + 28.0;
        put(&mut out, &self.who.buffer, cx(self.who.extent.w), y, alpha);
        y += self.who.extent.h as f64 + 12.0;
        put(&mut out, &self.number.buffer, cx(self.number.extent.w), y, alpha);
        // The right panel: the buttons, a word under each.
        let pressed = self.pressed.map(|(_, b)| b);
        for (b, r) in self.buttons() {
            let a = alpha * if pressed == Some(b) { 0.65 } else { 1.0 };
            let (bg, ic, word) = match b {
                Button::Answer => (&self.green, &self.start_icon, &self.words[1]),
                Button::Decline => (&self.red, &self.stop_icon, &self.words[0]),
                Button::HangUp => (&self.red, &self.stop_icon, &self.words[2]),
            };
            put(&mut out, &word.buffer, r.loc.x + (BUTTON - word.extent.w as f64) / 2.0, r.loc.y + BUTTON + 14.0, a);
            if let Some(ic) = ic {
                put(&mut out, ic, r.loc.x + (BUTTON - 36.0) / 2.0, r.loc.y + (BUTTON - 36.0) / 2.0, a);
            }
            put(&mut out, bg, r.loc.x, r.loc.y, a);
        }
        // Over the lock screen, which it hides.
        let (w, h) = layout::LAYOUT;
        let rect = Rectangle::<i32, Physical>::from_size((w * SCALE, h * SCALE).into());
        out.push(ShellElement::Solid(SolidColorRenderElement::new(self.dim.clone(), rect, CommitCounter::from((k * 1000.0) as usize), [0.02, 0.02, 0.03, alpha], Kind::Unspecified)));
        out
    }

    /// For CALL_TEST: a call as gnome-calls would show it.
    pub fn pretend(&mut self, state: u32, now_ns: u64) {
        self.pretending.store(true, std::sync::atomic::Ordering::Relaxed);
        let path = OwnedObjectPath::try_from("/org/gnome/Calls/Call/0").unwrap();
        *self.calls.lock().unwrap() = vec![Call { path, state: CallState::from(state), number: "+1 555 0100".into(), name: "Alex Morgan".into() }];
        self.changed.store(true, std::sync::atomic::Ordering::Relaxed);
        self.take(now_ns);
    }
}
