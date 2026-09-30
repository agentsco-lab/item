//! Notifications: the compositor is the session's notification server
//! (org.freedesktop.Notifications on the session bus - phosh's job, and
//! phosh is stopped while we run). A notification goes to the right shade,
//! which keeps what is going on, and shows as a banner at the top of the
//! right panel for 4 s. A tap on it invokes its default action; its close
//! button, or the banner's timeout, is the user's or its own dismissal.
//!
//! The bus runs on zbus's own threads; the list is shared, and the loop is
//! woken with a ping when it changes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::ping::Ping;
use zbus::zvariant::OwnedValue;

const PATH: &str = "/org/freedesktop/Notifications";
const NAME: &str = "org.freedesktop.Notifications";
/// How long a banner stays.
pub const BANNER_NS: u64 = 4_000_000_000;

#[derive(Clone)]
pub struct Note {
    pub id: u32,
    pub app: String,
    /// An icon name or path, from the notification or its app.
    pub icon: String,
    pub summary: String,
    pub body: String,
    pub default_action: bool,
    pub at_ns: u64,
}

#[derive(Default)]
struct Shared {
    notes: Vec<Note>,
    next: u32,
}

struct Server {
    shared: Arc<Mutex<Shared>>,
    wake: Ping,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".into(), "actions".into()]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let mut shared = self.shared.lock().unwrap();
        let id = if replaces_id != 0 && shared.notes.iter().any(|n| n.id == replaces_id) {
            replaces_id
        } else {
            shared.next += 1;
            shared.next
        };
        // The icon: the notification's image-path hint (where notify-send
        // puts -i), its app_icon, else its desktop entry's app.
        let hint = |k: &str| hints.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).filter(|s| !s.is_empty());
        let icon = hint("image-path")
            .or_else(|| hint("image_path"))
            .or_else(|| (!app_icon.is_empty()).then_some(app_icon))
            .map(|i| i.strip_prefix("file://").map(str::to_owned).unwrap_or(i))
            .or_else(|| hint("desktop-entry").and_then(|d| crate::apps::entry(&format!("{d}.desktop"))).and_then(|e| e.icon))
            .unwrap_or_default();
        let note = Note {
            id,
            app: app_name,
            icon,
            summary,
            // One line of the body, without markup.
            body: strip_markup(&body).lines().next().unwrap_or("").to_owned(),
            default_action: actions.chunks(2).any(|a| a[0] == "default"),
            at_ns: hybris_hwc::now_ns(),
        };
        tracing::info!("notification {id} from {} (icon {:?}): {}", note.app, note.icon, note.summary);
        shared.notes.retain(|n| n.id != id);
        shared.notes.insert(0, note);
        drop(shared);
        self.wake.ping();
        id
    }

    fn close_notification(&self, id: u32) {
        self.shared.lock().unwrap().notes.retain(|n| n.id != id);
        self.wake.ping();
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("item-compositor".into(), "agentsco-lab".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}

fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
}

pub struct Notes {
    shared: Arc<Mutex<Shared>>,
    conn: Option<zbus::blocking::Connection>,
}

impl Notes {
    pub fn new(wake: Ping) -> Notes {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let server = Server { shared: shared.clone(), wake };
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(NAME))
            .and_then(|b| b.serve_at(PATH, server))
            .and_then(|b| b.build());
        let conn = match conn {
            Ok(c) => {
                tracing::info!("notifications: serving {NAME}");
                Some(c)
            }
            Err(e) => {
                tracing::warn!("notifications: {NAME} not served: {e}");
                None
            }
        };
        Notes { shared, conn }
    }

    /// The notifications, newest first.
    pub fn list(&self) -> Vec<Note> {
        self.shared.lock().unwrap().notes.clone()
    }

    /// The newest one, if it came within the banner's time.
    pub fn banner(&self, now_ns: u64) -> Option<Note> {
        self.shared.lock().unwrap().notes.first().filter(|n| now_ns < n.at_ns + BANNER_NS).cloned()
    }

    /// Dismissed by the user (reason 2).
    pub fn dismiss(&self, id: u32) {
        self.shared.lock().unwrap().notes.retain(|n| n.id != id);
        if let Some(c) = &self.conn {
            if let Err(e) = c.emit_signal(None::<()>, PATH, NAME, "NotificationClosed", &(id, 2u32)) {
                tracing::warn!("notifications: NotificationClosed: {e}");
            }
        }
    }

    /// Tapped: its default action, and gone.
    pub fn invoke(&self, id: u32) {
        let note = self.shared.lock().unwrap().notes.iter().find(|n| n.id == id).cloned();
        if let Some(n) = note {
            if let (true, Some(c)) = (n.default_action, &self.conn) {
                if let Err(e) = c.emit_signal(None::<()>, PATH, NAME, "ActionInvoked", &(id, "default")) {
                    tracing::warn!("notifications: ActionInvoked: {e}");
                }
            }
            self.dismiss(id);
        }
    }
}
