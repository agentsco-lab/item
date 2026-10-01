//! Pages over the app that opens them (item's org.sfduo.Dock.Follow and
//! org.sfduo.Phoc.Present, which the port's Settings calls).
//!
//! The port's Settings opens GNOME Settings' and Mobile Settings' pages,
//! each a window of another program. Before it does, it says
//! `Follow(app_id, leader)` on the session bus: that program's windows go
//! where the leader's is, not on the other panel. Here a follower's window
//! takes its leader's page in the ribbon, over it, the other panel as it
//! was; closed or put away, the leader's page is there again (ribbon.rs's
//! cover). `Present(app_id)` brings a window of it put away back first, so
//! the page it is asked for comes up in it. Who follows whom is kept in the
//! runtime directory, as the dock kept it (`Follows`, for the shell).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;

#[derive(Default)]
struct Shared {
    follows: HashMap<String, String>,
    /// App ids asked to be presented, not yet done.
    present: Vec<String>,
}

struct Dock {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

fn kept() -> std::path::PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    std::path::Path::new(&dir).join("item-follows")
}

#[zbus::interface(name = "org.sfduo.Dock")]
impl Dock {
    fn follow(&self, app_id: String, leader: String) {
        let mut s = self.shared.lock().unwrap();
        if s.follows.get(&app_id) != Some(&leader) {
            tracing::info!("follow: {app_id} goes where {leader} is");
            s.follows.insert(app_id, leader);
            let text: String = s.follows.iter().map(|(a, l)| format!("{a} {l}\n")).collect();
            let _ = std::fs::write(kept(), text);
        }
        drop(s);
        // A window of it open already goes over its leader now.
        self.wake.ping();
    }

    #[zbus(property)]
    fn follows(&self) -> HashMap<String, String> {
        self.shared.lock().unwrap().follows.clone()
    }

    /// The old dock's: which half each app was put on. Nothing here: the
    /// ribbon places windows.
    #[zbus(property)]
    fn placed(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    #[zbus(property)]
    fn busy(&self) -> String {
        String::new()
    }
}

struct Phoc {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

#[zbus::interface(name = "org.sfduo.Phoc")]
impl Phoc {
    fn present(&self, app_id: String) {
        self.shared.lock().unwrap().present.push(app_id);
        self.wake.ping();
    }
}

pub struct Follow {
    shared: Arc<Mutex<Shared>>,
    _conn: Option<zbus::blocking::Connection>,
}

impl Follow {
    pub fn new(wake: Ping) -> Follow {
        let follows: HashMap<String, String> = std::fs::read_to_string(kept())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(' ').map(|(a, b)| (a.to_owned(), b.to_owned())))
            .collect();
        let shared = Arc::new(Mutex::new(Shared { follows, present: Vec::new() }));
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name("org.sfduo.Dock"))
            .and_then(|b| b.name("org.sfduo.Phoc"))
            .and_then(|b| b.serve_at("/org/sfduo/Dock", Dock { shared: shared.clone(), wake: wake.clone() }))
            .and_then(|b| b.serve_at("/org/sfduo/Phoc", Phoc { shared: shared.clone(), wake }))
            .and_then(|b| b.build());
        match &conn {
            Ok(_) => tracing::info!("follow: org.sfduo.Dock and org.sfduo.Phoc"),
            Err(e) => tracing::warn!("follow: not on the bus: {e}"),
        }
        Follow { shared, _conn: conn.ok() }
    }

    /// The app whose windows `app_id`'s go over, if any.
    pub fn leader(&self, app_id: &str) -> Option<String> {
        self.shared.lock().unwrap().follows.get(app_id).cloned()
    }

    /// The app ids asked to be presented since last asked.
    pub fn take_present(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().unwrap().present)
    }
}
