//! Delegated TLS: `https://` served by exec'ing the user's own
//! `openssl s_client` (zero cryptography in the binary — charter C2/D14).
//! This module owns the CA-bundle resolution, the fail-closed flag probe,
//! and `OpensslStream`, the child `ByteStream` the HTTP/1.1 framing code
//! (`http1`) drives unchanged.

use std::path::Path;

use super::FetchError;

/// CA bundle search order when `$STELE_CA_FILE` is unset. First existing wins.
pub(crate) const DEFAULT_CA_PATHS: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt", // Debian, Ubuntu, Alpine, Arch
    "/etc/pki/tls/certs/ca-bundle.crt",   // Fedora, RHEL, CentOS
    "/etc/ssl/ca-bundle.pem",             // openSUSE
    "/etc/ssl/cert.pem",                  // Alpine (alt), BSD, macOS
];

/// Resolve the CA bundle path for openssl's `-CAfile`. `$STELE_CA_FILE` wins
/// if set (and must exist); otherwise the first existing default path; else a
/// legible error. No config-file subsystem (the binary has none).
pub(crate) fn resolve_ca_file() -> Result<String, FetchError> {
    if let Ok(p) = std::env::var("STELE_CA_FILE") {
        if !p.is_empty() {
            if Path::new(&p).exists() {
                return Ok(p);
            }
            return Err(FetchError::Tls(format!(
                "STELE_CA_FILE points at {p}, which does not exist. Nothing was fetched."
            )));
        }
    }
    for p in DEFAULT_CA_PATHS {
        if Path::new(p).exists() {
            return Ok((*p).to_string());
        }
    }
    Err(FetchError::Tls(format!(
        "no CA bundle found (tried: {}). Set STELE_CA_FILE to your CA bundle. Nothing was fetched.",
        DEFAULT_CA_PATHS.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serializes every test that mutates process-global env (STELE_CA_FILE),
    // since cargo runs tests in parallel threads. unwrap_or_else tolerates a
    // poisoned lock (a panicking test) so one failure doesn't cascade.
    pub(super) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn stele_ca_file_env_overrides_when_it_exists() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // Point at a file guaranteed to exist: this source file.
        let this = concat!(env!("CARGO_MANIFEST_DIR"), "/src/fetch/https.rs");
        std::env::set_var("STELE_CA_FILE", this);
        let got = resolve_ca_file().expect("existing STELE_CA_FILE resolves");
        std::env::remove_var("STELE_CA_FILE");
        assert_eq!(got, this);
    }

    #[test]
    fn stele_ca_file_missing_path_is_a_legible_error() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("STELE_CA_FILE", "/no/such/ca/bundle.crt");
        let err = resolve_ca_file().expect_err("missing STELE_CA_FILE errors");
        std::env::remove_var("STELE_CA_FILE");
        match err {
            FetchError::Tls(m) => {
                assert!(m.contains("/no/such/ca/bundle.crt"), "message: {m}");
                assert!(m.contains("Nothing was fetched"), "message: {m}");
            }
            other => panic!("expected Tls, got {other:?}"),
        }
    }
}
