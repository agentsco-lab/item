//! The fingerprint reader on the lock screen, through Droidian's daemon
//! (`org.droidian.fingerprint`, droidian-fpd, on the system bus).
//!
//! It does what the port's sfduo-fingerprint does under phosh, in the
//! compositor, so that the lock screen can say what the reader is doing: the
//! reader is armed (`Identify`) while the phone is locked and lit, armed
//! again 0.4 s after an attempt ends (the daemon gives one up after 30 s,
//! and after a finger it does not know), 1.5 s after it refuses (busy). A known finger
//! unlocks, an unknown one is said on the lock screen; each gets a buzz
//! (feedbackd), as under phosh. The daemon serves one client at a time, so
//! sfduo-fingerprint does not run in the item session.
//!
//! All the bus work is on a thread of its own: the loop tells it whether the
//! reader is wanted, and hears back through a shared list and the ping.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay::reexports::calloop::ping::Ping;

const NAME: &str = "org.droidian.fingerprint";
const PATH: &str = "/org/droidian/fingerprint";
const REARM: Duration = Duration::from_millis(400);
const BACKOFF: Duration = Duration::from_millis(1500);
/// feedbackd's events, as sfduo-fingerprint chose them: 150 ms and 100 ms.
const KNOWN_EVENT: &str = "message-sent-instant";
const UNKNOWN_EVENT: &str = "bell-terminal";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A known finger: unlock.
    Identified,
    /// A finger the reader does not know.
    NotRecognized,
    /// Enrolling: how far (0-100).
    EnrollProgress(i32),
    /// Enrolled: the finger is known now.
    Enrolled,
    /// The enrolling stopped without a finger.
    EnrollFailed,
    /// Enrolling: a touch the reader could not take, and why.
    Poor(Poor),
}

/// Why a touch was not taken (the daemon's AcquisitionInfo, which it sends
/// only when it changes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Poor {
    Partial,
    Insufficient,
    Dirty,
    TooFast,
    TooSlow,
}

enum Msg {
    Want(bool),
    Enroll(String),
    StopEnroll,
    Signal(String, Option<String>, Option<i32>),
}

pub struct Fingerprint {
    tx: Option<Sender<Msg>>,
    events: Arc<Mutex<Vec<Event>>>,
    wanted: bool,
    /// Fingers enrolled when the compositor started.
    pub fingers: usize,
}

impl Fingerprint {
    pub fn new(wake: Ping) -> Fingerprint {
        let events = Arc::new(Mutex::new(Vec::new()));
        let none = Fingerprint { tx: None, events: events.clone(), wanted: false, fingers: 0 };
        if std::env::var_os("NO_FINGERPRINT").is_some() {
            return none;
        }
        let Ok(system) = zbus::blocking::Connection::system() else { return none };
        let fingers = system
            .call_method(Some(NAME), PATH, Some(NAME), "GetAll", &())
            .and_then(|r| r.body().deserialize::<Vec<String>>())
            .map(|f| f.len());
        let fingers = match fingers {
            Ok(n) => n,
            Err(e) => {
                tracing::info!("fingerprint: no reader daemon ({e})");
                return none;
            }
        };
        tracing::info!("fingerprint: {fingers} finger(s) enrolled");
        let (tx, rx) = channel::<Msg>();
        // The daemon's signals, into the worker's queue.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(NAME)
            .and_then(|b| b.interface(NAME))
            .map(|b| b.build());
        if let Ok(signals) = rule.and_then(|r| zbus::blocking::MessageIterator::for_match_rule(r, &system, Some(16))) {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for message in signals.flatten() {
                    let member = message.header().member().map(|m| m.to_string()).unwrap_or_default();
                    let arg = message.body().deserialize::<String>().ok();
                    let number = message.body().deserialize::<i32>().ok();
                    if tx.send(Msg::Signal(member, arg, number)).is_err() {
                        break;
                    }
                }
            });
        }
        let worker_events = events.clone();
        std::thread::spawn(move || {
            let call = |method: &str| system.call_method(Some(NAME), PATH, Some(NAME), method, &()).and_then(|r| r.body().deserialize::<i32>());
            let (mut wanted, mut armed, mut enrolling) = (false, false, false);
            let push = |event: Event| {
                worker_events.lock().unwrap().push(event);
                wake.ping();
            };
            let mut next_try = Instant::now();
            loop {
                let wait = if wanted && !armed { next_try.saturating_duration_since(Instant::now()) } else { Duration::from_secs(3600) };
                match rx.recv_timeout(wait) {
                    Ok(Msg::Want(w)) => {
                        wanted = w;
                        if !w && armed {
                            armed = false;
                            let _ = call("Abort");
                            tracing::info!("fingerprint: reader given up");
                        }
                    }
                    Ok(Msg::Enroll(name)) => {
                        if armed {
                            armed = false;
                            let _ = call("Abort");
                        }
                        let reply = system.call_method(Some(NAME), PATH, Some(NAME), "Enroll", &(name.as_str())).and_then(|r| r.body().deserialize::<i32>());
                        match reply {
                            Ok(0) => {
                                enrolling = true;
                                tracing::info!("fingerprint: enrolling");
                            }
                            other => {
                                tracing::info!("fingerprint: Enroll refused ({other:?})");
                                push(Event::EnrollFailed);
                            }
                        }
                    }
                    Ok(Msg::StopEnroll) => {
                        if enrolling {
                            enrolling = false;
                            let _ = call("Abort");
                            tracing::info!("fingerprint: enrolling given up");
                        }
                    }
                    Ok(Msg::Signal(member, _, number)) if enrolling && member == "EnrollProgressChanged" => {
                        tracing::info!("fingerprint: a touch taken, {}%", number.unwrap_or(0));
                        push(Event::EnrollProgress(number.unwrap_or(0)));
                    }
                    Ok(Msg::Signal(member, arg, _)) if enrolling && member == "AcquisitionInfo" => {
                        let poor = match arg.as_deref() {
                            Some("FPACQUIRED_PARTIAL") => Some(Poor::Partial),
                            Some("FPACQUIRED_INSUFFICIENT") => Some(Poor::Insufficient),
                            Some("FPACQUIRED_IMAGER_DIRTY") => Some(Poor::Dirty),
                            Some("FPACQUIRED_TOO_FAST") => Some(Poor::TooFast),
                            Some("FPACQUIRED_TOO_SLOW") => Some(Poor::TooSlow),
                            _ => None,
                        };
                        if let Some(poor) = poor {
                            tracing::info!("fingerprint: a touch not taken ({poor:?})");
                            push(Event::Poor(poor));
                        }
                    }
                    Ok(Msg::Signal(member, _, _)) if enrolling && member == "Added" => {
                        enrolling = false;
                        tracing::info!("fingerprint: enrolled");
                        push(Event::Enrolled);
                        buzz(KNOWN_EVENT);
                    }
                    Ok(Msg::Signal(member, arg, _)) if enrolling && member == "StateChanged" && arg.as_deref() == Some("FPSTATE_IDLE") => {
                        enrolling = false;
                        tracing::info!("fingerprint: enrolling stopped");
                        push(Event::EnrollFailed);
                    }
                    Ok(Msg::Signal(member, arg, _)) => match (member.as_str(), arg.as_deref()) {
                        ("Identified", _) if armed => {
                            armed = false;
                            // Not again before the loop has heard and let the
                            // reader go.
                            next_try = Instant::now() + Duration::from_secs(1);
                            tracing::info!("fingerprint: identified");
                            worker_events.lock().unwrap().push(Event::Identified);
                            wake.ping();
                            buzz(KNOWN_EVENT);
                        }
                        ("ErrorInfo", Some("FINGER_NOT_RECOGNIZED")) if armed => {
                            tracing::info!("fingerprint: not recognized");
                            worker_events.lock().unwrap().push(Event::NotRecognized);
                            wake.ping();
                            buzz(UNKNOWN_EVENT);
                        }
                        // The attempt is over: timed out, failed or done.
                        ("StateChanged", Some("FPSTATE_IDLE")) if armed => {
                            armed = false;
                            next_try = Instant::now() + REARM;
                        }
                        _ => {}
                    },
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if wanted && !armed && !enrolling && Instant::now() >= next_try {
                    match call("Identify") {
                        Ok(0) => {
                            armed = true;
                            tracing::info!("fingerprint: reader armed");
                        }
                        other => {
                            tracing::info!("fingerprint: Identify refused ({other:?}); again in 1.5 s");
                            next_try = Instant::now() + BACKOFF;
                        }
                    }
                }
            }
        });
        Fingerprint { tx: Some(tx), events, wanted: false, fingers }
    }

    /// Whether the reader should listen: the phone locked and lit.
    pub fn want(&mut self, wanted: bool) {
        let wanted = wanted && self.fingers > 0;
        if wanted != self.wanted {
            self.wanted = wanted;
            if let Some(tx) = &self.tx {
                let _ = tx.send(Msg::Want(wanted));
            }
        }
    }

    /// Enroll a finger, named so.
    pub fn enroll(&self, name: &str) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Enroll(name.to_owned()));
        }
    }

    /// Stop enrolling.
    pub fn stop_enroll(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::StopEnroll);
        }
    }

    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut self.events.lock().unwrap())
    }
}

/// A buzz through feedbackd (its event names), on a thread of its own.
pub fn buzz(event: &'static str) {
    std::thread::spawn(move || {
        if let Ok(s) = zbus::blocking::Connection::session() {
            let hints: std::collections::HashMap<&str, zbus::zvariant::Value> = Default::default();
            let _ = s.call_method(Some("org.sigxcpu.Feedback"), "/org/sigxcpu/Feedback", Some("org.sigxcpu.Feedback"), "TriggerFeedback", &("item-compositor", event, hints, -1i32));
        }
    });
}
