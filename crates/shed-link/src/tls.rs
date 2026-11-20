//! TLS for talking to a nexus: the usual web roots, plus a CA file of
//! one's own for a nexus whose certificate one made.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

pub(crate) fn client_config(ca_file: Option<&Path>) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(file) = ca_file {
        let file = shed_save::expand_home(&file.to_string_lossy());
        let mut added = 0;
        for cert in CertificateDer::pem_file_iter(&file).with_context(|| format!("reading {}", file.display()))? {
            let cert = cert.with_context(|| format!("reading {}", file.display()))?;
            roots.add(cert).with_context(|| format!("a certificate in {}", file.display()))?;
            added += 1;
        }
        anyhow::ensure!(added > 0, "no certificates in {}", file.display());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS setup")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Whether an address's host (`host[:port]/…`) is this machine: plain
/// `http`/`ws` is only for a nexus here (tests, a nexus on the same host),
/// since the workspace's token would travel in the clear.
fn loopback(rest: &str) -> bool {
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn plain_refused(scheme: &str) -> anyhow::Error {
    anyhow::anyhow!("{scheme}:// would send the workspace's token unencrypted; use https:// (plain {scheme}:// only to this machine)")
}

/// The link's WebSocket address for a nexus's address: `https://n.example`
/// → `wss://n.example/link` (`http` → `ws`, to this machine only); a
/// `ws(s)://` address is taken as it is, `/link` added if missing.
pub(crate) fn link_url(base: &str) -> anyhow::Result<String> {
    let base = base.trim().trim_end_matches('/');
    let (scheme, rest) = base.split_once("://").with_context(|| format!("{base:?} is not an address (https://…)"))?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "wss",
        "http" | "ws" if loopback(rest) => "ws",
        plain @ ("http" | "ws") => return Err(plain_refused(plain)),
        other => anyhow::bail!("{other}:// is not an address the link can use (https://…)"),
    };
    let path = if rest.ends_with("/link") { String::new() } else { "/link".into() };
    Ok(format!("{scheme}://{rest}{path}"))
}

/// An HTTP API address under a nexus's address: `https://n.example` and
/// `api/v1/pair/start` → `https://n.example/api/v1/pair/start`.
pub(crate) fn api_url(base: &str, route: &str) -> anyhow::Result<String> {
    let base = base.trim().trim_end_matches('/');
    let (scheme, rest) = base.split_once("://").with_context(|| format!("{base:?} is not an address (https://…)"))?;
    let base = match scheme.to_ascii_lowercase().as_str() {
        "https" => base.to_string(),
        "wss" => base.replacen(scheme, "https", 1),
        "http" if loopback(rest) => base.to_string(),
        "ws" if loopback(rest) => base.replacen(scheme, "http", 1),
        plain @ ("http" | "ws") => return Err(plain_refused(plain)),
        other => anyhow::bail!("{other}:// is not an address the link can use (https://…)"),
    };
    let base = base.strip_suffix("/link").unwrap_or(&base);
    Ok(format!("{base}/{}", route.trim_start_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(link_url("https://n.example.com/").unwrap(), "wss://n.example.com/link");
        assert_eq!(link_url("http://localhost:8080/shed").unwrap(), "ws://localhost:8080/shed/link");
        assert_eq!(link_url("ws://[::1]:9").unwrap(), "ws://[::1]:9/link");
        // Plain addresses elsewhere would send the token in the clear.
        for plain in ["http://n.example.com", "ws://n.example.com/link", "http://10.0.0.5:7870", "http://127.0.0.1.evil.example"] {
            assert!(link_url(plain).is_err(), "{plain}");
            assert!(api_url(plain, "api/v1/pair/start").is_err(), "{plain}");
        }
        assert_eq!(link_url("ws://127.0.0.1:9/link").unwrap(), "ws://127.0.0.1:9/link");
        assert!(link_url("n.example.com").is_err());
        assert!(link_url("ftp://n").is_err());
        assert_eq!(api_url("https://n.example.com", "api/v1/pair/start").unwrap(), "https://n.example.com/api/v1/pair/start");
        assert_eq!(api_url("ws://127.0.0.1:9/link", "/api/v1/pair/poll").unwrap(), "http://127.0.0.1:9/api/v1/pair/poll");
    }

    #[test]
    fn the_usual_roots_load() {
        assert!(client_config(None).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("none.pem");
        std::fs::write(&empty, "").unwrap();
        assert!(client_config(Some(&empty)).is_err());
    }
}
