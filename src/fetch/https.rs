//! Delegated TLS: `https://` served by exec'ing the user's own
//! `openssl s_client` (zero cryptography in the binary — charter C2/D14).
//! This module owns the CA-bundle resolution, the fail-closed flag probe,
//! and `OpensslStream`, the child `ByteStream` the HTTP/1.1 framing code
//! (`http1`) drives unchanged.

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags, Timespec};

use super::transport::ByteStream;
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

/// Per-read (and per-connect) timeout on the child's stdout pipe. Mirrors
/// `http1`'s `IO_TIMEOUT`; a stalled TLS peer must not hang the browser.
const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Cap on stderr we read back for an error message — bounded so a chatty child
/// can't run us out of memory, and drained BEFORE `wait()` to avoid the
/// full-pipe deadlock.
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// A single TLS connection, delegated to an `openssl s_client` child. `Write`
/// is the plaintext request side (child stdin → TLS → server); `Read` is the
/// response side (server → TLS → child stdout). One child per connection.
pub(crate) struct OpensslStream {
    child: Child,
    /// `Option` so `shutdown_write` can drop (close) it while we keep reading.
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
    stderr: Option<ChildStderr>,
    host: String,
}

/// Connect to `host:port` over TLS via `openssl s_client`, verifying against
/// the resolved CA bundle. Fail-closed: probe first; any spawn/verify problem
/// is a legible `FetchError::Tls`.
pub(crate) fn connect(host: &str, port: u16) -> Result<OpensslStream, FetchError> {
    probe()?; // fail closed before we ever open a socket
    let ca = resolve_ca_file()?;

    let child = Command::new("openssl")
        .arg("s_client")
        .arg("-quiet") // no interactive mode (the `Q`-at-line-start trap)
        .arg("-no_ign_eof") // let our stdin EOF propagate a close (-quiet implies -ign_eof)
        .args(["-connect", &format!("{host}:{port}")])
        .args(["-servername", host])
        .arg("-verify_return_error") // a verify failure aborts, non-zero exit
        .args(["-verify_hostname", host])
        .args(["-CAfile", &ca])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            FetchError::Tls(format!(
                "could not start openssl ({e}); https is unavailable. Nothing was fetched."
            ))
        })?;

    let mut child = child;
    let stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FetchError::Tls("openssl child produced no stdout".to_string()))?;
    let stderr = child.stderr.take();

    Ok(OpensslStream {
        child,
        stdin,
        stdout,
        stderr,
        host: host.to_string(),
    })
}

impl OpensslStream {
    /// Drain up to `MAX_STDERR_BYTES` of the child's stderr (openssl's verify
    /// reason) — MUST be called before `wait()` on the error path to avoid a
    /// full-pipe deadlock.
    fn drain_stderr(&mut self) -> String {
        let mut out = Vec::new();
        if let Some(mut err) = self.stderr.take() {
            let mut buf = [0u8; 4096];
            while out.len() < MAX_STDERR_BYTES {
                match err.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => out.extend_from_slice(&buf[..n.min(MAX_STDERR_BYTES - out.len())]),
                }
            }
        }
        String::from_utf8_lossy(&out).trim().to_string()
    }

    /// On an exchange failure over this connection, decide whether it was a
    /// TLS-layer failure (verify/handshake/connect) and, if so, return a legible
    /// `FetchError::Tls` folding in openssl's reason + the host; otherwise return
    /// the framing `fallback` unchanged. Called by `http1::send_one` (T4).
    pub(crate) fn tls_error_or(&mut self, fallback: FetchError) -> FetchError {
        // If our own read timed out, the child may still be running — don't
        // block on wait(); return the timeout and let Drop kill it.
        if let FetchError::Io(ref m) = fallback {
            if m.contains("timed out") {
                return fallback;
            }
        }
        // The child closed stdout (it exited/failed) — safe to reap and read its
        // stderr. A non-zero exit means the failure was at the TLS layer even if
        // -quiet trimmed the stderr reason.
        let status_failed = match self.child.wait() {
            Ok(s) => !s.success(),
            Err(_) => true,
        };
        let reason = self.drain_stderr();
        if status_failed || !reason.is_empty() {
            let detail = if reason.is_empty() {
                "verification or handshake failed".to_string()
            } else {
                reason
            };
            FetchError::Tls(format!(
                "TLS connection to {} failed ({detail}). Nothing was fetched.",
                self.host
            ))
        } else {
            fallback
        }
    }
}

impl Read for OpensslStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Child stdout is a pipe: no socket read-timeout setter, so gate each
        // read with poll(2). Deliberately the seam a future progress()/throbber
        // hook subscribes to.
        let timeout = Timespec { tv_sec: IO_TIMEOUT.as_secs() as i64, tv_nsec: 0 };
        let mut fds = [PollFd::new(&self.stdout, PollFlags::IN)];
        let n = poll(&mut fds, Some(&timeout))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("poll: {e}")))?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "openssl child read timed out"));
        }
        self.stdout.read(buf)
    }
}

impl Write for OpensslStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.stdin.as_mut() {
            Some(s) => s.write(buf),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "openssl child stdin already closed",
            )),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.stdin.as_mut() {
            Some(s) => s.flush(),
            None => Ok(()),
        }
    }
}

impl ByteStream for OpensslStream {
    fn shutdown_write(&mut self) -> io::Result<()> {
        // Dropping ChildStdin closes it → EOF on the child's stdin → (with
        // -no_ign_eof) openssl closes its write side to the server.
        self.stdin = None;
        Ok(())
    }
}

impl Drop for OpensslStream {
    fn drop(&mut self) {
        // No `kill_on_drop` in std (that is tokio): kill THEN wait. The wait is
        // load-bearing — kill without reap leaks a zombie per connection.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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

    #[test]
    fn connect_to_a_dead_port_is_a_legible_tls_error_not_a_panic() {
        // Spawns openssl / reads env → serialize with the other env tests.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // If openssl is absent/old, probe() fails first — also a Tls error, so
        // either way no panic/hang. Port 1: nothing listens; openssl exits.
        if let Ok(mut s) = connect("127.0.0.1", 1) {
            let mut buf = [0u8; 16];
            let _ = s.read(&mut buf); // must not panic
        }
        // Reaching here without panic/hang is the assertion.
    }
}
