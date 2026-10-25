//! item-mirror-relay: item's mirror (mirror.rs, the user's socket in the
//! runtime directory) carried to standard output - for item/grid, which
//! reads it over ssh (the phone has no socat or nc; a few lines of Python
//! did this until 2026-10-26).

use std::io::{Read, Write};

fn main() {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let path = format!("{dir}/item-mirror.sock");
    let mut s = match std::os::unix::net::UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("item-mirror-relay: {path}: {e}");
            std::process::exit(1);
        }
    };
    let mut out = std::io::stdout().lock();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
        }
    }
}
