//! The keyring's secrets for the shell: the Secret Service on the session
//! bus (gnome-keyring's login keyring on the port, open with the PIN). A key
//! for a service the shell speaks to is read when it is needed and kept in
//! no file: put in with `item-secret set NAME` (the value on its standard
//! input) from a terminal, or over ssh from item/grid; the compositor reads
//! it with `get`. Both go through a plain session: the bus is the user's
//! own.

use std::collections::HashMap;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const SERVICE: &str = "org.freedesktop.secrets";
const PATH: &str = "/org/freedesktop/secrets";
const IFACE: &str = "org.freedesktop.Secret.Service";
const ITEM: &str = "org.freedesktop.Secret.Item";
#[allow(dead_code)] // item-secret's (bin/item-secret.rs)
const COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
const SCHEMA: &str = "lab.agentsco.item.Secret";

/// A secret as the service hands it: the session, its parameters, the
/// value, its content type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn err(e: zbus::Error) -> String {
    format!("keyring: {e}")
}

fn service(conn: &Connection) -> Result<Proxy<'static>, String> {
    Proxy::new(conn, SERVICE, PATH, IFACE).map_err(err)
}

fn open_session(conn: &Connection) -> Result<OwnedObjectPath, String> {
    let (_, session): (OwnedValue, OwnedObjectPath) = service(conn)?.call("OpenSession", &("plain", Value::from(""))).map_err(err)?;
    Ok(session)
}

fn attributes(name: &str) -> HashMap<&str, &str> {
    HashMap::from([("xdg:schema", SCHEMA), ("item", name)])
}

/// The secret kept under `name`, if there is one (the keyring unlocked for
/// it if it is locked and no prompt is needed).
pub fn get(name: &str) -> Result<Option<String>, String> {
    let conn = crate::bus::session().map_err(err)?;
    let svc = service(&conn)?;
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc.call("SearchItems", &(attributes(name),)).map_err(err)?;
    let mut items = unlocked;
    if items.is_empty() && !locked.is_empty() {
        let (now_unlocked, _prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = svc.call("Unlock", &(locked,)).map_err(err)?;
        items = now_unlocked;
    }
    let Some(item) = items.first() else { return Ok(None) };
    let session = open_session(&conn)?;
    let secret: Secret = Proxy::new(&conn, SERVICE, item.clone(), ITEM).map_err(err)?.call("GetSecret", &(session,)).map_err(err)?;
    Ok(Some(String::from_utf8_lossy(&secret.2).trim().to_owned()))
}

/// The names of the secrets kept here (item's own schema), sorted.
#[allow(dead_code)] // item-secret's (bin/item-secret.rs)
pub fn list() -> Result<Vec<String>, String> {
    let conn = crate::bus::session().map_err(err)?;
    let svc = service(&conn)?;
    let attrs: HashMap<&str, &str> = HashMap::from([("xdg:schema", SCHEMA)]);
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc.call("SearchItems", &(attrs,)).map_err(err)?;
    let mut names = Vec::new();
    for item in unlocked.iter().chain(locked.iter()) {
        let p = Proxy::new(&conn, SERVICE, item.clone(), ITEM).map_err(err)?;
        let a: HashMap<String, String> = p.get_property("Attributes").map_err(err)?;
        if let Some(n) = a.get("item") {
            names.push(n.clone());
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// The secret under `name` forgotten; whether there was one.
#[allow(dead_code)] // item-secret's (bin/item-secret.rs)
pub fn forget(name: &str) -> Result<bool, String> {
    let conn = crate::bus::session().map_err(err)?;
    let svc = service(&conn)?;
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc.call("SearchItems", &(attributes(name),)).map_err(err)?;
    let mut any = false;
    for item in unlocked.iter().chain(locked.iter()) {
        let p = Proxy::new(&conn, SERVICE, item.clone(), ITEM).map_err(err)?;
        let _prompt: OwnedObjectPath = p.call("Delete", &()).map_err(err)?;
        any = true;
    }
    Ok(any)
}

/// `value` kept under `name` in the default keyring, in place of what was
/// there. The keyring must be unlocked (the phone's PIN opens it).
#[allow(dead_code)] // item-secret's (bin/item-secret.rs)
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let conn = crate::bus::session().map_err(err)?;
    let collection = ObjectPath::try_from(COLLECTION).map_err(|e| e.to_string())?;
    let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service(&conn)?.call("Unlock", &(vec![collection.clone()],)).map_err(err)?;
    if prompt.as_str() != "/" {
        return Err("keyring: the login keyring is locked - unlock the phone first".into());
    }
    let session = open_session(&conn)?;
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Item.Label", Value::from(format!("item: {name}")));
    props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attributes(name)));
    let secret = (session, Vec::<u8>::new(), value.as_bytes().to_vec(), "text/plain");
    let (_item, prompt): (OwnedObjectPath, OwnedObjectPath) = Proxy::new(&conn, SERVICE, collection, "org.freedesktop.Secret.Collection").map_err(err)?.call("CreateItem", &(props, secret, true)).map_err(err)?;
    if prompt.as_str() != "/" {
        return Err("keyring: a prompt is needed to keep it".into());
    }
    Ok(())
}
