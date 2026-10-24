//! The Duo's sensors read by item itself, in Rust, as the port's
//! sfduo-posture (Python) read them: sensorfw's hinge, accelerometer and
//! gyroscope sessions, each a data socket (/run/sensord.sock) that brings
//! frames of readings. From them, as the daemon did: the hinge's angle
//! glided between its whole-degree readings; the posture with hysteresis;
//! Moving; gravity smoothed; the roll about the screen's normal from the
//! gyroscope pulled to gravity's; Still. Told to posture.rs over a
//! channel (no bus in between: the decision ~15 ms sooner, one process
//! fewer), and published on the session bus as org.sfduo.Posture for
//! item/grid and anyone else - unless the daemon still owns the name.
//!
//! While the display is dark the sensors are stopped (main.rs says), and
//! started again as it lights; the first hinge reading then is where the
//! hinge is, not a place to glide to. A sensor sensorfw cannot give yet
//! (still starting, a load past the bus's timeout, a socket gone) is asked
//! for again, 2 s on and doubling to 30 s, on this thread's clock. Lit
//! with sessions open and nothing read for SILENT (the accelerometer
//! streams; sensorfwd hung on 2026-10-23 with its sockets open, its bus
//! dead), the sockets are shut and the sensors asked for again - said in
//! the log, where a frozen posture is otherwise silent.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::{OwnedValue, Value};

use crate::posture::{Said, Told};

const SENSORFW: &str = "com.nokia.SensorService";
const MANAGER: &str = "/SensorManager";
const MANAGER_IFACE: &str = "local.SensorManager";
const SOCKET: &str = "/run/sensord.sock";

const BUS_NAME: &str = "org.sfduo.Posture";
const PATH: &str = "/org/sfduo/Posture";
const IFACE: &str = "org.sfduo.Posture";

/// The hinge: readings every hundred milliseconds in whole degrees; the
/// estimate glides to the last one in ARRIVE readings' time (the daemon's
/// measured choice against a spring and an alpha-beta filter), at a
/// FRAME_MS clock while it moves.
const FRAME_MS: u64 = 16;
const ARRIVE: f64 = 1.5;
const MOVING_DEG_S: f64 = 20.0;
const STILL_AFTER_MS: f64 = 400.0;
const CONFIRM: u32 = 2;
const HYSTERESIS: f64 = 5.0;
const POSTURES: [(&str, f64, f64); 4] = [("closed", -1.0, 10.0), ("laptop", 60.0, 130.0), ("flat", 150.0, 210.0), ("folded", 340.0, 361.0)];

/// The accelerometer: thousandths of g, ten a second.
const ACCEL_INTERVAL_MS: i32 = 100;
const GRAVITY_STEP: f64 = 0.02;
const GRAVITY_SMOOTH: f64 = 0.5;
/// The gyroscope: thousandths of a degree a second, twenty a second
/// whatever is asked (measured 2026-10-22).
const GYRO_INTERVAL_MS: i32 = 20;
const ROLL_STEP: f64 = 0.5;
const ROLL_PULL: f64 = 0.08;
const ROLL_GRAVITY_MAX_Z: f64 = 0.85;
const STILL_DEG_S: f64 = 8.0;
const STILL_AFTER_S: f64 = 0.12;

/// A sensor sensorfw could not give is asked for again after RETRY,
/// doubling with each refusal to RETRY_MAX.
const RETRY: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(30);
/// Lit, sessions open, nothing read for this long: asked for again.
const SILENT: Duration = Duration::from_secs(15);
/// The system bus asked for this many times at start, a second apart.
const BUS_TRIES: u32 = 5;

/// What a sensor's socket brings.
enum Reading {
    Hinge(u32),
    Accel([f64; 3]),
    Gyro([f64; 3]),
}

enum Event {
    Reading(Reading),
    Dark(bool),
    /// Gravity and the gyroscope wanted (the turn the sensors') or not.
    Streams(bool),
    /// A socket died (the sensor's, that session's): asked for again.
    Lost(&'static str, i32),
}

/// A sensor as sensorfw has it.
struct Sensor {
    name: &'static str,
    iface: &'static str,
    interval_ms: Option<i32>,
    session: Option<i32>,
    /// Its data socket (read on a thread; shut from here when silent).
    sock: Option<Arc<std::os::unix::net::UnixStream>>,
    /// When to ask sensorfw for it again, and how long after that.
    retry_at: Option<Instant>,
    backoff: Duration,
}

/// The values as told last, for the bus's properties.
#[derive(Clone, Default)]
pub struct Values {
    pub angle: f64,
    pub raw: u32,
    pub posture: String,
    pub moving: bool,
    pub velocity: f64,
    pub gravity: [f64; 3],
    pub roll: f64,
    pub still: bool,
}

struct Bus {
    shared: Arc<Mutex<Values>>,
}

#[zbus::interface(name = "org.sfduo.Posture")]
impl Bus {
    #[zbus(property)]
    fn angle(&self) -> f64 {
        self.shared.lock().unwrap().angle
    }
    #[zbus(property)]
    fn raw(&self) -> u32 {
        self.shared.lock().unwrap().raw
    }
    #[zbus(property)]
    fn posture(&self) -> String {
        self.shared.lock().unwrap().posture.clone()
    }
    #[zbus(property)]
    fn moving(&self) -> bool {
        self.shared.lock().unwrap().moving
    }
    #[zbus(property)]
    fn velocity(&self) -> f64 {
        self.shared.lock().unwrap().velocity
    }
    #[zbus(property)]
    fn gravity(&self) -> Vec<f64> {
        self.shared.lock().unwrap().gravity.to_vec()
    }
    #[zbus(property)]
    fn roll(&self) -> f64 {
        self.shared.lock().unwrap().roll
    }
    #[zbus(property)]
    fn still(&self) -> bool {
        self.shared.lock().unwrap().still
    }
}

/// The sensors, running: `told` carries every change to posture.rs;
/// `dark` is for main.rs to say the display went dark or lit.
pub struct Sensors {
    cmds: Sender<Event>,
}

impl Sensors {
    /// Starts the sensors; what they say comes on the receiver returned.
    pub fn start(wake: Ping) -> (Sensors, Receiver<Said>) {
        let (events_tx, events_rx) = channel::<Event>();
        let (told_tx, told_rx) = channel::<Said>();
        let cmds = events_tx.clone();
        let _ = std::thread::Builder::new().name("sensors".into()).spawn(move || run(events_tx, events_rx, told_tx, wake));
        (Sensors { cmds }, told_rx)
    }

    /// The display dark (the sensors stopped) or lit (started, the hinge
    /// read afresh).
    pub fn dark(&self, dark: bool) {
        let _ = self.cmds.send(Event::Dark(dark));
    }

    /// Gravity and the gyroscope read (the turn the sensors') or let go.
    pub fn streams(&self, on: bool) {
        let _ = self.cmds.send(Event::Streams(on));
    }
}

fn posture_for(angle: f64, current: &str) -> &'static str {
    for (name, low, high) in POSTURES {
        let (lo, hi) = if name == current { (low - HYSTERESIS, high + HYSTERESIS) } else { (low, high) };
        if lo <= angle && angle <= hi {
            return name;
        }
    }
    "between"
}

/// The thread: the sessions set up, each socket read on a thread of its
/// own, the readings turned into the posture's values here.
fn run(events_tx: Sender<Event>, events_rx: Receiver<Event>, told: Sender<Said>, wake: Ping) {
    // The system bus, which may not be up as item starts.
    let mut system = crate::bus::system();
    for _ in 1..BUS_TRIES {
        if system.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
        system = crate::bus::system();
    }
    let Ok(system) = system else {
        tracing::warn!("sensors: no system bus after {BUS_TRIES} tries");
        return;
    };
    let shared = Arc::new(Mutex::new(Values { angle: 180.0, posture: "between".into(), gravity: [0.0, 0.0, 1.0], still: true, ..Default::default() }));
    // The bus: ours unless the port's daemon still owns the name.
    let session = crate::bus::session().and_then(|c| crate::bus::serve(&c, &[BUS_NAME], PATH, Bus { shared: shared.clone() }).map(|_| c));
    match &session {
        Ok(_) => tracing::info!("sensors: {BUS_NAME} is item's"),
        Err(e) => tracing::info!("sensors: {BUS_NAME} not taken ({e}): the daemon keeps it"),
    }
    let session = session.ok();
    let publish = |changed: HashMap<&str, Value<'_>>| {
        if let Some(conn) = &session {
            let _ = conn.emit_signal(None::<&str>, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged", &(IFACE, changed, Vec::<String>::new()));
        }
    };

    // The hinge always (the camera's fold, the lock, the dock's pieces);
    // gravity and the gyroscope only when the turn is theirs to decide
    // (prefs.rs, and the dock's button held): by the button, neither
    // streams at all.
    let mut sensors = vec![sensor("hingesensor", "local.HingeSensor", None)];
    if crate::prefs::turn_by_sensors() {
        sensors.extend(streams());
    } else {
        tracing::info!("sensors: the turn is the dock's button's: the hinge alone is read");
    }
    let mut dark = false;
    let now = Instant::now();
    for s in sensors.iter_mut() {
        attempt(&system, s, &events_tx, dark, now);
    }
    // The hinge's value now: the socket brings only changes.
    if let Some(h) = read_hinge(&system) {
        let _ = events_tx.send(Event::Reading(Reading::Hinge(h)));
    }

    // The state, as the daemon kept it.
    let mut v = shared.lock().unwrap().clone();
    let mut raw: Option<u32> = None;
    let mut raw_at = now;
    let mut angle: Option<f64> = None;
    let mut fresh = true;
    let mut gliding = false;
    let mut last_tick = now;
    // When a reading last came while lit (the watchdog's clock).
    let mut heard = now;
    let mut candidate: (&'static str, u32) = ("between", 0);
    let mut gravity_told: Option<[f64; 3]> = None;
    let mut roll: Option<f64> = None;
    let mut roll_told: Option<f64> = None;
    let mut gyro_at: Option<Instant> = None;
    // When the gyroscope last saw motion; None before it has.
    let mut moved_at: Option<Instant> = None;

    let tell = |v: &Values, told: &Sender<Said>, wake: &Ping| {
        let _ = told.send(Said::Told(Told { angle: v.angle, gravity: v.gravity, moving: v.moving, roll: v.roll, still: v.still }));
        wake.ping();
    };

    loop {
        // Woken by the readings, and by the clock only while the hinge
        // glides or a sensor is due to be asked for again: dark and not
        // gliding, the thread sleeps until the display lights.
        let now = Instant::now();
        let mut due: Option<Instant> = (gliding && !dark).then(|| now + Duration::from_millis(FRAME_MS));
        for at in sensors.iter().filter_map(|s| s.retry_at) {
            due = Some(due.map_or(at, |d| d.min(at)));
        }
        if !dark && sensors.iter().any(|s| s.session.is_some()) {
            let at = heard + SILENT;
            due = Some(due.map_or(at, |d| d.min(at)));
        }
        let event = match due {
            Some(at) => match events_rx.recv_timeout(at.saturating_duration_since(now)) {
                Ok(e) => Some(e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match events_rx.recv() {
                Ok(e) => Some(e),
                Err(_) => return,
            },
        };
        let now = Instant::now();
        let mut changed: HashMap<&str, Value<'_>> = HashMap::new();
        if !dark && matches!(event, Some(Event::Reading(_))) {
            heard = now;
        }
        match event {
            Some(Event::Dark(d)) => {
                if d != dark {
                    dark = d;
                    for s in sensors.iter() {
                        call(&system, s, if dark { "stop" } else { "start" });
                    }
                    if dark {
                        // Nothing glides or is decided with the screen off;
                        // gravity and the roll are read afresh as it lights.
                        gliding = false;
                        v.velocity = 0.0;
                        if v.moving {
                            v.moving = false;
                            changed.insert("Moving", false.into());
                            changed.insert("Velocity", 0.0f64.into());
                        }
                        gravity_told = None;
                        roll = None;
                        gyro_at = None;
                        let _ = told.send(Said::Dark(true));
                    } else {
                        fresh = true;
                        heard = now;
                        if let Some(h) = read_hinge(&system) {
                            let _ = events_tx.send(Event::Reading(Reading::Hinge(h)));
                        }
                    }
                    tracing::info!("sensors: display {}", if dark { "off: not reading" } else { "on: reading" });
                }
            }
            Some(Event::Streams(on)) => {
                let have = sensors.len() > 1;
                if on && !have {
                    for mut s in streams() {
                        attempt(&system, &mut s, &events_tx, dark, now);
                        sensors.push(s);
                    }
                    heard = now;
                    tracing::info!("sensors: the turn is the sensors': gravity and the gyroscope read");
                } else if !on && have {
                    for s in sensors.drain(1..) {
                        call(&system, &s, "stop");
                        if let Some(sock) = s.sock {
                            let _ = sock.shutdown(std::net::Shutdown::Both);
                        }
                    }
                    gravity_told = None;
                    roll = None;
                    gyro_at = None;
                    tracing::info!("sensors: the turn is the dock's button's: the hinge alone is read");
                }
            }
            Some(Event::Lost(name, session)) => {
                // Asked for again on this loop's clock, not by sleeping here
                // (three sockets dying together would serialize). A socket
                // the watchdog shut, or of a session since replaced: nothing.
                if let Some(s) = sensors.iter_mut().find(|s| s.name == name && s.session == Some(session)) {
                    s.session = None;
                    s.sock = None;
                    s.retry_at = Some(now + s.backoff);
                }
            }
            Some(Event::Reading(Reading::Hinge(deg))) if !dark => {
                let degrees = deg as f64;
                if fresh {
                    fresh = false;
                    gliding = false;
                    raw = Some(deg);
                    raw_at = now;
                    angle = Some(degrees);
                    v.angle = degrees;
                    v.raw = deg;
                    v.velocity = 0.0;
                    changed.insert("Angle", v.angle.into());
                    changed.insert("Raw", v.raw.into());
                    let name = posture_for(degrees, &v.posture);
                    if name != v.posture {
                        v.posture = name.into();
                        changed.insert("Posture", v.posture.clone().into());
                    }
                    candidate = (name, 0);
                    if v.moving {
                        v.moving = false;
                        changed.insert("Moving", false.into());
                        changed.insert("Velocity", 0.0f64.into());
                    }
                } else {
                    let interval = now.duration_since(raw_at).as_secs_f64().clamp(0.05, 0.3);
                    raw = Some(deg);
                    raw_at = now;
                    changed.insert("Raw", deg.into());
                    v.raw = deg;
                    let a = angle.unwrap_or(degrees);
                    v.velocity = (degrees - a) / (ARRIVE * interval);
                    if !gliding {
                        last_tick = now;
                        gliding = true;
                    }
                    // The posture by the reading, after CONFIRM in a row.
                    let name = posture_for(degrees, &v.posture);
                    if name == v.posture {
                        candidate = (name, 0);
                    } else {
                        let count = if candidate.0 == name { candidate.1 + 1 } else { 1 };
                        candidate = (name, count);
                        if count >= CONFIRM {
                            v.posture = name.into();
                            candidate = (name, 0);
                            changed.insert("Posture", v.posture.clone().into());
                        }
                    }
                    let moving = v.velocity.abs() >= MOVING_DEG_S;
                    if moving != v.moving {
                        v.moving = moving;
                        changed.insert("Moving", moving.into());
                        changed.insert("Velocity", v.velocity.into());
                    }
                }
            }
            Some(Event::Reading(Reading::Accel(xyz))) if !dark => {
                let g = xyz.map(|x| x / 1000.0);
                if gravity_told.is_none() {
                    // Gravity read afresh (at start, the display lit again):
                    // posture.rs decides from the reading told next on.
                    let _ = told.send(Said::Dark(false));
                }
                v.gravity = match gravity_told {
                    None => g,
                    Some(_) => [0, 1, 2].map(|i| v.gravity[i] + (g[i] - v.gravity[i]) * GRAVITY_SMOOTH),
                };
                if gravity_told.is_none_or(|t| (0..3).any(|i| (v.gravity[i] - t[i]).abs() >= GRAVITY_STEP)) {
                    gravity_told = Some(v.gravity);
                    changed.insert("Gravity", v.gravity.to_vec().into());
                }
                let [x, y, z] = v.gravity;
                if z.abs() < ROLL_GRAVITY_MAX_Z && x * x + y * y > 0.1 {
                    let own = x.atan2(y).to_degrees();
                    roll = Some(match roll {
                        None => own,
                        Some(r) => r + ((own - r + 180.0).rem_euclid(360.0) - 180.0) * ROLL_PULL,
                    });
                }
            }
            Some(Event::Reading(Reading::Gyro(xyz))) if !dark => {
                let [wx, wy, wz] = xyz.map(|x| x / 1000.0);
                let moving = wx.abs().max(wy.abs()).max(wz.abs()) >= STILL_DEG_S;
                if moving {
                    moved_at = Some(now);
                }
                let still = !moving && moved_at.is_none_or(|at| now.duration_since(at).as_secs_f64() >= STILL_AFTER_S);
                if still != v.still {
                    v.still = still;
                    changed.insert("Still", still.into());
                }
                if let (Some(at), Some(r)) = (gyro_at, roll) {
                    let dt = now.duration_since(at).as_secs_f64().min(0.2);
                    if moving {
                        roll = Some((r + wz * dt + 180.0).rem_euclid(360.0) - 180.0);
                    }
                }
                gyro_at = Some(now);
            }
            Some(Event::Reading(_)) => {}
            None => {}
        }
        // Lit, sessions open, nothing read for SILENT: the sockets shut
        // (their threads end, Lost brings each sensor back on its clock).
        if !dark && now >= heard + SILENT && sensors.iter().any(|s| s.session.is_some()) {
            tracing::warn!("sensors: nothing read for {} s with the display lit: asked for again", SILENT.as_secs());
            for s in sensors.iter_mut().filter(|s| s.session.is_some()) {
                if let Some(sock) = s.sock.take() {
                    let _ = sock.shutdown(std::net::Shutdown::Both);
                }
                s.session = None;
                s.retry_at = Some(now + s.backoff);
            }
            heard = now;
        }
        // The sensors due to be asked for again.
        for s in sensors.iter_mut() {
            if s.retry_at.is_some_and(|at| at <= now) && attempt(&system, s, &events_tx, dark, now) && s.name == "hingesensor" {
                // Its socket brings only changes: the value now, where the
                // hinge is rather than a place to glide to.
                fresh = true;
                if let Some(h) = read_hinge(&system) {
                    let _ = events_tx.send(Event::Reading(Reading::Hinge(h)));
                }
            }
        }
        // The hinge gliding to its last reading.
        if gliding && !dark {
            let dt = now.duration_since(last_tick).as_secs_f64().min(0.05);
            last_tick = now;
            if let (Some(a), Some(r)) = (angle, raw) {
                let next = a + v.velocity * dt;
                let r = r as f64;
                let arrived = (v.velocity > 0.0 && next >= r) || (v.velocity < 0.0 && next <= r) || v.velocity == 0.0;
                let a = if arrived { r } else { next };
                angle = Some(a);
                v.angle = a;
                changed.insert("Angle", a.into());
                if arrived {
                    gliding = false;
                    v.velocity = 0.0;
                }
                let moving = v.velocity.abs() >= MOVING_DEG_S && now.duration_since(raw_at).as_secs_f64() * 1000.0 < STILL_AFTER_MS;
                if moving != v.moving {
                    v.moving = moving;
                    changed.insert("Moving", moving.into());
                    changed.insert("Velocity", v.velocity.into());
                }
            }
        }
        // The roll told at its step.
        if let Some(r) = roll {
            if roll_told.is_none_or(|t| (r - t).abs() >= ROLL_STEP) {
                roll_told = Some(r);
                v.roll = r;
                changed.insert("Roll", r.into());
            }
        }
        if !changed.is_empty() {
            *shared.lock().unwrap() = v.clone();
            tell(&v, &told, &wake);
            publish(changed);
        }
    }
}

fn sensor(name: &'static str, iface: &'static str, interval_ms: Option<i32>) -> Sensor {
    Sensor { name, iface, interval_ms, session: None, sock: None, retry_at: None, backoff: RETRY }
}

/// Gravity and the gyroscope, as asked for when the turn is theirs.
fn streams() -> [Sensor; 2] {
    [sensor("accelerometersensor", "local.AccelerometerSensor", Some(ACCEL_INTERVAL_MS)), sensor("gyroscopesensor", "local.GyroscopeSensor", Some(GYRO_INTERVAL_MS))]
}

/// `s` asked of sensorfw and started unless dark; refused (sensorfw not up
/// yet, a load past the bus's timeout), asked for again after its backoff.
fn attempt(system: &zbus::blocking::Connection, s: &mut Sensor, events: &Sender<Event>, dark: bool, now: Instant) -> bool {
    if open(system, s, events) {
        s.retry_at = None;
        s.backoff = RETRY;
        // (The watchdog's clock is set by the caller's readings.)
        if !dark {
            call(system, s, "start");
        }
        true
    } else {
        tracing::info!("sensors: {} asked for again in {} s", s.name, s.backoff.as_secs());
        s.retry_at = Some(now + s.backoff);
        s.backoff = (s.backoff * 2).min(RETRY_MAX);
        false
    }
}

/// A session asked of sensorfw for `s`, its socket read on a thread:
/// whether it was given.
fn open(system: &zbus::blocking::Connection, s: &mut Sensor, events: &Sender<Event>) -> bool {
    let r = system
        .call_method(Some(SENSORFW), MANAGER, Some(MANAGER_IFACE), "loadPlugin", &(s.name,))
        .and_then(|_| system.call_method(Some(SENSORFW), MANAGER, Some(MANAGER_IFACE), "requestSensor", &(s.name, 0i64)))
        .and_then(|r| r.body().deserialize::<i32>());
    let session = match r {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("sensors: no {}: {e}", s.name);
            return false;
        }
    };
    s.session = Some(session);
    if let Some(ms) = s.interval_ms {
        let _ = system.call_method(Some(SENSORFW), format!("{MANAGER}/{}", s.name).as_str(), Some(s.iface), "setInterval", &(session, ms));
    }
    let sock = match connect(session) {
        Ok(sock) => Arc::new(sock),
        Err(e) => {
            tracing::warn!("sensors: {}'s socket: {e}", s.name);
            s.session = None;
            return false;
        }
    };
    s.sock = Some(sock.clone());
    tracing::info!("sensors: {} session {session}", s.name);
    let (name, events) = (s.name, events.clone());
    let _ = std::thread::Builder::new().name(format!("sensor {name}")).spawn(move || {
        if let Err(e) = read_socket(&sock, name, &events) {
            tracing::warn!("sensors: {name}'s socket: {e}");
        }
        let _ = events.send(Event::Lost(name, session));
    });
    true
}

fn call(system: &zbus::blocking::Connection, s: &Sensor, method: &str) {
    if let Some(session) = s.session {
        if let Err(e) = system.call_method(Some(SENSORFW), format!("{MANAGER}/{}", s.name).as_str(), Some(s.iface), method, &(session,)) {
            tracing::warn!("sensors: {method} {}: {e}", s.name);
        }
    }
}

/// The hinge's value asked for once (the socket brings only changes).
fn read_hinge(system: &zbus::blocking::Connection) -> Option<u32> {
    let r = system
        .call_method(Some(SENSORFW), "/SensorManager/hingesensor", Some("org.freedesktop.DBus.Properties"), "Get", &("local.HingeSensor", "hinge"))
        .ok()?;
    let v: OwnedValue = r.body().deserialize().ok()?;
    // (timestamp, degrees) in a struct, or a plain number.
    match v.downcast_ref::<zbus::zvariant::Structure>() {
        Ok(s) => s.fields().get(1).and_then(|f| u32::try_from(f.try_clone().ok()?).ok()),
        Err(_) => u32::try_from(v).ok(),
    }
}

/// sensorfw's data socket for a session: a greeting byte, the session's
/// number sent back (2 s for each, or no socket).
fn connect(session: i32) -> std::io::Result<std::os::unix::net::UnixStream> {
    let mut sock = std::os::unix::net::UnixStream::connect(SOCKET)?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))?;
    sock.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut greeting = [0u8; 1];
    sock.read_exact(&mut greeting)?;
    sock.write_all(&session.to_le_bytes())?;
    sock.set_read_timeout(None)?;
    sock.set_write_timeout(None)?;
    Ok(sock)
}

/// The socket read: frames of a count and as many readings - 16 bytes for
/// the hinge (u64 time, u32 value, 4 pad), 24 for x, y, z (u64, three f32,
/// 4 pad). The last reading of a frame is the one told. Ends with the
/// socket (closed by sensorfwd, or shut from the loop when silent).
fn read_socket(mut sock: &std::os::unix::net::UnixStream, name: &'static str, events: &Sender<Event>) -> std::io::Result<()> {
    let size = if name == "hingesensor" { 16 } else { 24 };
    let mut pending: Vec<u8> = Vec::with_capacity(4096);
    let mut buf = [0u8; 8192];
    loop {
        let n = sock.read(&mut buf)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed"));
        }
        pending.extend_from_slice(&buf[..n]);
        let mut last: Option<Reading> = None;
        while pending.len() >= 4 {
            let count = u32::from_le_bytes([pending[0], pending[1], pending[2], pending[3]]) as usize;
            if count > 1000 {
                pending.clear();
                break;
            }
            let end = 4 + size * count;
            if pending.len() < end {
                break;
            }
            if count > 0 {
                let r = &pending[end - size..end];
                last = Some(if name == "hingesensor" {
                    Reading::Hinge(u32::from_le_bytes([r[8], r[9], r[10], r[11]]))
                } else {
                    let f = |o: usize| f32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]) as f64;
                    let xyz = [f(8), f(12), f(16)];
                    if name == "gyroscopesensor" { Reading::Gyro(xyz) } else { Reading::Accel(xyz) }
                });
            }
            pending.drain(..end);
        }
        if let Some(r) = last {
            if events.send(Event::Reading(r)).is_err() {
                return Ok(());
            }
        }
    }
}
