//! CV ID on the lock screen (item-tracker #109; an experiment, CVID=1),
//! through item-face (crates/item-face, org.sfduo.Face on the system bus):
//! it holds the camera, the models and the faces kept, and says only
//! whether a face was known. Here: the camera held open while the lock
//! screen is lit and a face is kept (quick to start; let go shut or dark), a look asked for as the lid opens or the screen lights
//! locked, the doors opened on Identified - as for a known finger.
//!
//! Enrolling: in the first setup, unseen, the face of the one typing the PIN
//! - the owner - is taken, and kept once the PIN is accepted: item-face
//! checks the PIN itself (Keep), so no app can put its face in the owner's
//! place.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a face is kept (item-face's Enrolled, then its Kept and our
/// Remove): the tile's state, and whether a look is worth asking for.
static ENROLLED: AtomicBool = AtomicBool::new(false);

pub fn enrolled() -> bool {
    ENROLLED.load(Ordering::Relaxed)
}
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use smithay::reexports::calloop::ping::Ping;

const NAME: &str = "org.sfduo.Face";
const PATH: &str = "/org/sfduo/Face";
/// No further folded than this (degrees, 180 flat): past it the camera
/// turns away from the one holding the Duo.
pub const FOLD_MAX: f64 = 250.0;

enum Ask {
    Open,
    Close,
    Identify,
    Enrol,
    Cancel,
    Keep(String),
    Remove,
}

#[derive(Default)]
struct Heard {
    matched: AtomicBool,
    taken: AtomicBool,
}

pub struct Face {
    asks: Option<Sender<Ask>>,
    heard: Arc<Heard>,
    held: bool,
    looking: bool,
}

impl Face {
    pub fn new(wake: Ping) -> Face {
        let heard: Arc<Heard> = Default::default();
        if std::env::var_os("CVID").is_none() {
            return Face { asks: None, heard, held: false, looking: false };
        }
        let (tx, rx) = channel::<Ask>();
        let _ = std::thread::Builder::new().name("face".into()).spawn(move || {
            let Ok(bus) = zbus::blocking::Connection::system() else {
                tracing::warn!("cvid: no system bus");
                return;
            };
            match bus.call_method(Some(NAME), PATH, Some(NAME), "Enrolled", &()).and_then(|r| r.body().deserialize::<u32>()) {
                Ok(n) => {
                    tracing::info!("cvid: {n} face frame(s) kept");
                    ENROLLED.store(n > 0, Ordering::Relaxed);
                }
                Err(e) => tracing::warn!("cvid: Enrolled: {e}"),
            }
            for ask in rx {
                let (method, pin) = match &ask {
                    Ask::Open => ("Open", None),
                    Ask::Close => ("Close", None),
                    Ask::Identify => ("Identify", None),
                    Ask::Enrol => ("Enrol", None),
                    Ask::Cancel => ("Cancel", None),
                    Ask::Remove => ("Remove", None),
                    Ask::Keep(pin) => ("Keep", Some(pin.clone())),
                };
                let reply = match pin {
                    Some(mut pin) => {
                        let r = bus.call_method(Some(NAME), PATH, Some(NAME), method, &(pin.as_str(),)).and_then(|r| r.body().deserialize::<bool>());
                        // SAFETY: zeroing the bytes keeps the String valid UTF-8.
                        unsafe { std::ptr::write_bytes(pin.as_mut_ptr(), 0, pin.len()) };
                        r.map(|kept| tracing::info!("cvid: the face {}", if kept { "kept" } else { "not kept: the PIN refused" }))
                    }
                    None => bus.call_method(Some(NAME), PATH, Some(NAME), method, &()).map(|_| ()),
                };
                if let Err(e) = reply {
                    tracing::warn!("cvid: {method}: {e}");
                }
            }
        });
        // Its answers, for this user.
        let (h, uid) = (heard.clone(), unsafe { libc::getuid() });
        let _ = std::thread::Builder::new().name("face-heard".into()).spawn(move || {
            let Ok(bus) = zbus::blocking::Connection::system() else { return };
            let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(NAME).and_then(|b| b.path(PATH)).map(|b| b.build());
            let Ok(rule) = rule else { return };
            let Ok(signals) = zbus::blocking::MessageIterator::for_match_rule(rule, &bus, Some(16)) else { return };
            for msg in signals.flatten() {
                let hdr = msg.header();
                let Some(member) = hdr.member() else { continue };
                let body = msg.body();
                match member.as_str() {
                    "Identified" => {
                        if let Ok((who, alike)) = body.deserialize::<(u32, f64)>() {
                            if who == uid {
                                tracing::info!("cvid: a known face (alike {alike:.3})");
                                h.matched.store(true, Ordering::Relaxed);
                                wake.ping();
                            }
                        }
                    }
                    "NotIdentified" => {
                        if body.deserialize::<(u32,)>().is_ok_and(|(who,)| who == uid) {
                            tracing::info!("cvid: no known face");
                        }
                    }
                    "Kept" => {
                        if body.deserialize::<(u32, u32)>().is_ok_and(|(who, _)| who == uid) {
                            ENROLLED.store(true, Ordering::Relaxed);
                            wake.ping();
                        }
                    }
                    "Taken" => {
                        if body.deserialize::<(u32, u32)>().is_ok_and(|(who, _)| who == uid) {
                            tracing::info!("cvid: the face taken");
                            h.taken.store(true, Ordering::Relaxed);
                            wake.ping();
                        }
                    }
                    _ => {}
                }
            }
        });
        tracing::info!("cvid: through {NAME}");
        Face { asks: Some(tx), heard, held: false, looking: false }
    }

    fn ask(&self, a: Ask) {
        if let Some(tx) = &self.asks {
            let _ = tx.send(a);
        }
    }

    /// The camera held open (while locked), or let go.
    pub fn hold(&mut self, on: bool) {
        if self.asks.is_some() && on != self.held {
            self.held = on;
            self.ask(if on { Ask::Open } else { Ask::Close });
        }
    }

    /// A look at once (the lid opening, before the panels light).
    pub fn look_now(&mut self) {
        if self.asks.is_some() && !self.looking {
            self.looking = true;
            self.ask(Ask::Identify);
        }
    }

    /// Each turn of the loop: a look wanted (the lock screen lit and a face
    /// would open it), or a face to enrol (the setup's PIN being typed).
    pub fn want(&mut self, on: bool, enrol: bool) {
        if self.asks.is_none() || on == self.looking {
            return;
        }
        self.looking = on;
        self.ask(if !on { Ask::Cancel } else if enrol { Ask::Enrol } else { Ask::Identify });
    }

    pub fn on(&self) -> bool {
        self.asks.is_some()
    }

    /// Face Unlock off: the face kept removed.
    pub fn remove(&self) {
        ENROLLED.store(false, Ordering::Relaxed);
        self.ask(Ask::Remove);
    }

    /// The PIN accepted: the face taken as it was typed is kept.
    pub fn keep(&self, pin: String) {
        self.ask(Ask::Keep(pin));
    }

    /// Whether a known face was seen since last asked.
    pub fn take_matched(&self) -> bool {
        self.heard.matched.swap(false, Ordering::Relaxed)
    }

    /// Whether the face to enrol was all taken since last asked.
    pub fn take_taken(&self) -> bool {
        self.heard.taken.swap(false, Ordering::Relaxed)
    }
}
