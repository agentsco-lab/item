//! HTTPS from the compositor itself, in place of wget and curl: the voice
//! (OpenRouter), the system screen's weather (met.no) and the port's latest
//! release (GitHub), Unsplash's photos. One agent for the process: rustls,
//! the certificates checked against the phone's own trust store (the
//! platform verifier, the ca-certificates wget and curl used), https only -
//! a redirect to http refused too.
//!
//! A key goes in a request's header and nowhere else: no configuration file
//! on the disk, no command line in /proc, no child process at all. Nothing
//! here logs a header or a body.

use std::sync::OnceLock;
use std::time::Duration;

/// item's name to the services it asks.
const USER_AGENT: &str = concat!("item-shell/", env!("CARGO_PKG_VERSION"), " github.com/agentsco-lab/item");
/// The most an answer may be (a photo at the Duo's size is a few MB).
const MAX_BODY: u64 = 32 << 20;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        ureq::Agent::config_builder()
            .https_only(true)
            .user_agent(USER_AGENT)
            .tls_config(tls)
            // An error's status read here, its body too (the voice's
            // service says why in it).
            .http_status_as_error(false)
            .build()
            .into()
    })
}

/// An answer: its status and body.
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn check(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("{url}: https only"))
    }
}

fn read(mut r: ureq::http::Response<ureq::Body>) -> Result<Answer, String> {
    let status = r.status().as_u16();
    let body = r.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|e| e.to_string())?;
    Ok(Answer { status, body })
}

/// A GET, with these headers, given `timeout` in all.
pub fn get(url: &str, headers: &[(&str, &str)], timeout: Duration) -> Result<Answer, String> {
    check(url)?;
    let mut req = agent().get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let r = req.config().timeout_global(Some(timeout)).build().call().map_err(|e| e.to_string())?;
    read(r)
}

/// A POST of JSON, with these headers, given `timeout` in all.
pub fn post_json(url: &str, headers: &[(&str, &str)], body: &str, timeout: Duration) -> Result<Answer, String> {
    check(url)?;
    let mut req = agent().post(url).header("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let r = req.config().timeout_global(Some(timeout)).build().send(body).map_err(|e| e.to_string())?;
    read(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the phone (the trust store is its): `item-compositor-test net --ignored`.
    #[test]
    #[ignore]
    fn reaches_the_services() {
        let t = Duration::from_secs(20);
        let met = get("https://api.met.no/weatherapi/locationforecast/2.0/compact?lat=50.45&lon=30.52", &[("Accept", "application/json")], t).unwrap();
        assert!(met.ok(), "met.no {}", met.status);
        assert!(String::from_utf8_lossy(&met.body).contains("timeseries"));
        let gh = get("https://api.github.com/repos/agentsco-lab/surfaceduo-droidian/releases/latest", &[("Accept", "application/vnd.github+json")], t).unwrap();
        assert!(gh.ok(), "github {}", gh.status);
        // Unsplash's API without a key: refused, but over TLS, with a status.
        let un = get("https://api.unsplash.com/photos/random", &[], t).unwrap();
        assert_eq!(un.status, 401);
        // OpenRouter without a key: a status too, not a TLS error.
        let or = post_json("https://openrouter.ai/api/v1/audio/transcriptions", &[], "{}", t).unwrap();
        assert!(!or.ok());
        assert!(get("http://api.met.no/", &[], t).is_err());
    }
}
