//! One connection to each bus, shared.
//!
//! Every `zbus::blocking::Connection` runs an executor thread of its own,
//! and the shell's pieces opened one each - 22 threads on 2026-10-23, some
//! opened and torn down for a single call (the keyring, calls, the setup).
//! A piece that serves an interface or holds a name still builds its own
//! (zbus's Builder: notifications, the network's secret agent, polkit, the
//! posture) - no more: `serve` puts an interface and its names on the
//! shared one. A bus not there at the first ask is asked for again next
//! time, not remembered as missing. The system bus answers in 5 s or not
//! at all (the fingerprint reader, polkit and NetworkManager asked for
//! that; nothing on it takes longer).

use std::sync::{Mutex, OnceLock};

use zbus::blocking::Connection;

static SESSION: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
static SYSTEM: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

fn shared(slot: &OnceLock<Mutex<Option<Connection>>>, open: fn() -> zbus::Result<Connection>) -> zbus::Result<Connection> {
    let mut kept = slot.get_or_init(Default::default).lock().unwrap();
    if let Some(c) = kept.as_ref() {
        return Ok(c.clone());
    }
    let c = open()?;
    *kept = Some(c.clone());
    Ok(c)
}

/// The session bus.
pub fn session() -> zbus::Result<Connection> {
    shared(&SESSION, Connection::session)
}

/// The system bus.
pub fn system() -> zbus::Result<Connection> {
    shared(&SYSTEM, || zbus::blocking::connection::Builder::system()?.method_timeout(std::time::Duration::from_secs(5)).build())
}

/// `iface` served at `path` on `conn`, under `names` (all of them, or the
/// interface comes off again and the error is the name's - as a Builder
/// with the name would have failed to build).
pub fn serve<I: zbus::object_server::Interface>(conn: &Connection, names: &[&str], path: &str, iface: I) -> zbus::Result<()> {
    conn.object_server().at(path, iface)?;
    for name in names {
        // Not queued behind an owner: taken now or not at all (the port's
        // daemon, another prompter or screen saver may hold it).
        let taken = match conn.request_name_with_flags(*name, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
            Ok(zbus::fdo::RequestNameReply::PrimaryOwner) | Ok(zbus::fdo::RequestNameReply::AlreadyOwner) => Ok(()),
            Ok(reply) => Err(zbus::Error::Failure(format!("{name}: {reply:?}, another owner has it"))),
            Err(e) => Err(e),
        };
        if let Err(e) = taken {
            let _ = conn.object_server().remove::<I, _>(path);
            return Err(e);
        }
    }
    Ok(())
}
