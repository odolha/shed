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

/// The link's WebSocket address for a nexus's address: `https://n.example`
/// → `wss://n.example/link` (`http` → `ws`); a `ws(s)://` address is taken
/// as it is, `/link` added if missing.
pub(crate) fn link_url(base: &str) -> anyhow::Result<String> {
    let base = base.trim().trim_end_matches('/');
    let (scheme, rest) = base.split_once("://").with_context(|| format!("{base:?} is not an address (https://…)"))?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        other => anyhow::bail!("{other}:// is not an address the link can use (https://…)"),
    };
    let path = if rest.ends_with("/link") { String::new() } else { "/link".into() };
    Ok(format!("{scheme}://{rest}{path}"))
}

/// An HTTP API address under a nexus's address: `https://n.example` and
/// `api/v1/pair/start` → `https://n.example/api/v1/pair/start`.
pub(crate) fn api_url(base: &str, route: &str) -> anyhow::Result<String> {
    let base = base.trim().trim_end_matches('/');
    let (scheme, _) = base.split_once("://").with_context(|| format!("{base:?} is not an address (https://…)"))?;
    let base = match scheme.to_ascii_lowercase().as_str() {
        "https" | "http" => base.to_string(),
        "wss" => base.replacen(scheme, "https", 1),
        "ws" => base.replacen(scheme, "http", 1),
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
