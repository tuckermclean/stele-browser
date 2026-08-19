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

/// Flags Stele's `s_client` invocation depends on. A binary missing any of
/// these cannot verify safely, so https is UNAVAILABLE (fail closed).
pub(crate) const REQUIRED_FLAGS: &[&str] = &[
    "-connect", "-servername", "-verify_return_error",
    "-verify_hostname", "-CAfile", "-quiet", "-no_ign_eof",
];

/// The actual probe work (no cache) — spawns `openssl s_client -help` and
/// checks every required flag. Returned as `Result<(), String>` so tests can
/// call it directly and deterministically (no `OnceLock` interference).
fn probe_uncached() -> Result<(), String> {
    let output = std::process::Command::new("openssl").arg("s_client").arg("-help").output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return Err(format!(
                "openssl could not be run ({e}); https is unavailable. Install OpenSSL, \
                 or use the monolith-surf proxy. Nothing was fetched."
            ));
        }
    };
    // `s_client -help` prints usage to stderr and exits nonzero; accept either.
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for flag in REQUIRED_FLAGS {
        if !help.contains(flag) {
            return Err(format!(
                "this openssl's s_client is missing {flag} (it may be LibreSSL or too old); \
                 https is unavailable. Install OpenSSL, or use the monolith-surf proxy. \
                 Nothing was fetched."
            ));
        }
    }
    Ok(())
}

static PROBE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// Verify (once per process, cached) that openssl s_client is usable. Fail
/// closed: any gap is a legible `FetchError::Tls`, never an unverified fetch.
pub(crate) fn probe() -> Result<(), FetchError> {
    let result = PROBE.get_or_init(probe_uncached);
    result.clone().map_err(FetchError::Tls)
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

    // Build a stub `openssl` in a temp dir; return the dir to prepend to PATH.
    fn stub_openssl_dir(help_text: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("stele-stub-openssl-{}", help_text.len()));
        let _ = std::fs::create_dir_all(&dir);
        let script = format!("#!/bin/sh\ncat >&2 <<'EOF'\n{help_text}\nEOF\nexit 1\n");
        let bin = dir.join("openssl");
        let mut f = std::fs::File::create(&bin).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).unwrap();
        dir
    }

    #[test]
    fn probe_fails_closed_when_a_required_flag_is_missing() {
        // Mutates PATH → serialize with the CA tests via ENV_LOCK. Calls
        // probe_uncached (NOT the cached probe()) so it never pollutes the
        // OnceLock that connect() uses.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let help = "-connect -servername -verify_return_error -verify_hostname -CAfile -quiet"; // no -no_ign_eof
        let dir = stub_openssl_dir(help);
        let old = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", dir.display(), old));
        let result = probe_uncached();
        std::env::set_var("PATH", old);
        let msg = result.expect_err("missing -no_ign_eof must fail closed");
        assert!(msg.contains("-no_ign_eof"), "message: {msg}");
        assert!(msg.to_lowercase().contains("openssl"), "message: {msg}");
    }
}
