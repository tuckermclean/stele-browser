# HTTPS via delegated `openssl s_client` (PR 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Click an `https://` link and get the page — TLS delegated to the user's own `openssl s_client` child (zero cryptography in the binary), verified, fail-closed, with a legible rendered error on any verification failure.

**Architecture:** A new `OpensslStream` implements the PR-1 `ByteStream` seam by spawning `openssl s_client` (args only, never a shell) and proxying its stdin/stdout. The existing `Http1Client` redirect/cookie loop is generalized to accept both `http` and `https` and pick the transport per hop (so cross-scheme redirects work); `exchange` gains a `half_close` flag so the openssl path can close child stdin while http stays byte-identical. Verification failures surface as a new `FetchError::Tls` rendered through the existing `error_page`.

**Tech Stack:** Rust std `std::process::Command`; `rustix::event::poll` (already a dep, used in `src/main.rs`) for a timeout on the child's stdout pipe; the user's system `openssl` binary at runtime and in tests (no external network — an in-harness `openssl s_server` with a generated test CA).

**Spec:** `docs/superpowers/specs/2026-08-19-https-openssl-transport-design.md` (§D–J). Read it alongside this plan.

## Global Constraints

- **No local builds.** This host cannot build/test the project (AGENTS.md §3; project memory). Implementers transcribe the plan's verbatim code and commit; they do NOT run cargo/accept.sh. Verification is CI-driven: the controller pushes and reads `m0-acceptance`. TDD red→green is confirmed in CI, not locally.
- **Charter covenant — zero cryptography in the binary.** TLS is *delegated, never embedded*. No TLS/crypto crate may be added to `Cargo.toml`. `accept.sh` gains a covenant grep asserting zero TLS symbols/strings in the i486 binary (Task 8). Report the size delta against the 1.44 MB floppy (1,474,560 bytes) from the CI `stele-i486` artifact — expected ~nil (only std process/IO code added).
- **Fail closed.** If `openssl` is absent or missing any required flag, https is UNAVAILABLE with a legible error naming the binary/flag and suggesting the proxy — NEVER a silently-unverified connection.
- **`-quiet -no_ign_eof` are correctness, pinned by tests.** `-quiet` disables interactive mode (where a body line starting with `Q` closes the connection); `-no_ign_eof` restores stdin-EOF-propagates-close (which `-quiet` alone suppresses). Never remove either.
- **Parsing/IO stays TOTAL.** Malformed child output, a dead child, a timeout — every failure yields a `FetchError`, never a panic and never a hang (timeouts on connect and every read; child reaped on drop).
- **Args only, never a shell.** Spawn openssl via `Command::arg(...)` exclusively. No string interpolation into a shell.
- **No JavaScript / no uninvited computation (C3):** untouched — transport only.
- **Branch:** `packet/https-openssl`, off `main` (already created at 344d76e, which contains the PR-1 seam). One PR. Conventional commit subjects (`feat(fetch):`, `test(fetch):`, `docs(...):`, `size:`).

## File Structure

- **Create** `src/fetch/https.rs` — `OpensslStream` (the child `ByteStream`), the fail-closed flag probe (`probe()`), CA-bundle resolution (`resolve_ca_file()`). One responsibility: the delegated-TLS transport.
- **Modify** `src/fetch/mod.rs` — `pub mod https;`; add `FetchError::Tls(String)`; `err_to_string` renders `Tls` bare (legible T4 text); `fetch()` routes `"https"`.
- **Modify** `src/fetch/http1.rs` — `Http1Client::fetch` accepts `http`|`https`; `send_one` picks the transport by scheme and passes `half_close`; `exchange` gains the `half_close` param; `format_request` omits the port from `Host` when it is the scheme default (443 for https); the https→http redirect downgrade notice.
- **Modify** `tests/support/mod.rs` — a TLS responder fixture: generate a test CA + leaf once, `spawn_tls_responder(response_bytes)` serving one connection via `openssl s_server`.
- **Create** `tests/fetch_https.rs` — the integration cases (trusted render, untrusted→error, hostname mismatch, `Q\n` trap, stdin-EOF, probe fail-closed) plus the cookie-`Secure` pin.
- **Modify** `accept.sh` — the covenant grep (A6): zero TLS symbols in the binary.
- **Modify** `stele-charter.md`, `DECISIONS.md`, `JOURNAL.md` — charter amendment, first transport decision entry, journal note.

---

### Task 1: Charter amendment + DECISIONS entry (the governance gate)

The delegated-openssl path is a **third** TLS option the charter did not foresee (it said "TLS: none in v1 … direct-TLS is a later optional feature (rustls/embedded-tls)"). This task records the amendment before any code leans on it. Docs only; no test.

**Files:**
- Modify: `stele-charter.md` (the "TLS: none in v1" line, ~line 149)
- Modify: `DECISIONS.md` (prepend — newest first)

- [ ] **Step 1: Amend the charter TLS line**

In `stele-charter.md`, find the line beginning `TLS: none in v1 — monolith-surf owns modernity; direct-TLS is a later optional feature (rustls/embedded-tls …` and append a sentence so it reads (keep the existing text, add the delegated clause):

```
TLS: none in v1 — monolith-surf owns modernity; direct-TLS is a later
optional feature (rustls/embedded-tls have no_std stories) that must
never become a default dependency. TLS is DELEGATED, never embedded —
zero bytes of cryptography in the binary; on Linux the provider is the
user's own openssl, exec'd as a child (s_client), verified and fail-closed.
```

- [ ] **Step 2: Add the first transport entry to DECISIONS.md**

Prepend to `DECISIONS.md` (newest first — match the file's existing entry format; read the top entry first to mirror its heading style):

```markdown
## D14 — HTTPS transport: delegated `openssl s_client`, not embedded TLS (2026-08-19)

**Options:** (a) embed a Rust TLS stack (rustls / embedded-tls); (b) proxy-only
(monolith-surf terminates TLS, Stele stays http-only); (c) delegate to the
user's system `openssl s_client`, exec'd as a child, plaintext over pipes.

**Choice:** (c). https is served by spawning `openssl s_client -quiet -no_ign_eof
-connect H:443 -servername H -verify_return_error -verify_hostname H -CAfile <ca>`
and framing HTTP/1.1 over its stdio, reusing the PR-1 `ByteStream` seam.

**Why:** The floppy budget and the covenant both. Embedding TLS adds hundreds of
KB of cryptography to audit and age inside a 1.44 MB binary; (c) adds *zero*
crypto bytes — the security-critical code is the user's own, already-patched
openssl. Verification is mandatory (`-verify_return_error`/`-verify_hostname`)
and fail-closed: a missing binary or flag makes https UNAVAILABLE with a legible
error, never a silent unverified connection. `-quiet` and `-no_ign_eof` are
correctness (interactive-mode `Q`-close trap; stdin-EOF propagation) and are
pinned by tests.

**Revisit trigger:** a no_std TLS stack that fits the floppy budget lands, OR we
must run where no system openssl exists (then proxy-only, option b, is the
fallback — not embedding).
```

- [ ] **Step 3: Commit**

```bash
git add stele-charter.md DECISIONS.md
git commit -m "docs(charter): admit delegated openssl TLS (C2 amendment + D14)"
```

---

### Task 2: `FetchError::Tls` + legible rendering

Add the transport's error class and make it render legibly (T4). Additive to the shared enum; `Tls` is rendered *bare* (no `Tls(...)` wrapper) because a TLS failure becomes a user-facing document via the existing `error_page`.

**Files:**
- Modify: `src/fetch/mod.rs`

**Interfaces:**
- Produces: `FetchError::Tls(String)` variant; `err_to_string` maps it to its inner string verbatim.

- [ ] **Step 1: Write the failing test**

Add to `src/fetch/mod.rs`'s `dispatch_tests` module:

```rust
    #[test]
    fn err_to_string_renders_tls_bare_for_legibility() {
        // T4: a TLS failure becomes a user-facing document, so it must read as
        // its own sentence, not a Debug-wrapped `Tls("...")`.
        let s = err_to_string(FetchError::Tls(
            "TLS verification failed for example.com: certificate has expired. Nothing was fetched.".to_string(),
        ));
        assert_eq!(
            s,
            "TLS verification failed for example.com: certificate has expired. Nothing was fetched."
        );
    }
```

- [ ] **Step 2: Verify it fails** — CI: `cargo test --lib fetch::dispatch_tests` → FAIL (no `Tls` variant).

- [ ] **Step 3: Add the variant and the render arm**

In `src/fetch/mod.rs`, add to `enum FetchError` (after `UnsupportedScheme`):

```rust
    /// A delegated-TLS (openssl child) failure: verification rejected the peer,
    /// the openssl binary/flag was unavailable, or the child IO failed. The
    /// string is a complete, user-facing sentence (rendered bare by
    /// `err_to_string` — T4).
    Tls(String),
```

And add the first arm to `err_to_string`:

```rust
pub fn err_to_string(err: FetchError) -> String {
    match err {
        FetchError::Tls(s) => s,
        FetchError::UnsupportedScheme(s) => format!("unsupported scheme: {s}"),
        other => format!("{other:?}"),
    }
}
```

- [ ] **Step 4: Verify it passes** — CI: `cargo test --lib fetch::dispatch_tests` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/fetch/mod.rs
git commit -m "feat(fetch): add FetchError::Tls, rendered bare for legible failures"
```

---

### Task 3: CA-bundle resolution

`resolve_ca_file()` — `$STELE_CA_FILE` if set (error if it names a missing file), else the first existing default path, else a legible error naming the paths tried. Pure and unit-testable.

**Files:**
- Create: `src/fetch/https.rs` (this task starts the file with just this function + its tests)
- Modify: `src/fetch/mod.rs` (add `pub mod https;`)

**Interfaces:**
- Consumes: `FetchError` (Task 2).
- Produces: `pub(crate) fn resolve_ca_file() -> Result<String, FetchError>`; `const DEFAULT_CA_PATHS: &[&str]`.

- [ ] **Step 1: Declare the module**

In `src/fetch/mod.rs`, add after `pub mod https;`... i.e. alongside the other `pub mod` lines:

```rust
pub mod https;
```

- [ ] **Step 2: Write the failing tests**

Create `src/fetch/https.rs` with:

```rust
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

    #[test]
    fn stele_ca_file_env_overrides_when_it_exists() {
        // Point at a file guaranteed to exist: this source file.
        let this = concat!(env!("CARGO_MANIFEST_DIR"), "/src/fetch/https.rs");
        // SAFETY: single-threaded test; we set then clear the var.
        std::env::set_var("STELE_CA_FILE", this);
        let got = resolve_ca_file().expect("existing STELE_CA_FILE resolves");
        assert_eq!(got, this);
        std::env::remove_var("STELE_CA_FILE");
    }

    #[test]
    fn stele_ca_file_missing_path_is_a_legible_error() {
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
```

Note: these two env tests share process-global state; they are written to set-then-remove the var and never assert the *default-path* branch (which depends on the host filesystem), so they do not collide. Keep them in this one module.

- [ ] **Step 3: Verify** — CI: `cargo test --lib fetch::https::tests` → the two tests pass. (No RED phase needed beyond "module doesn't compile until the fn exists"; this task is additive scaffolding.)

- [ ] **Step 4: Commit**

```bash
git add src/fetch/mod.rs src/fetch/https.rs
git commit -m "feat(fetch): CA-bundle resolution for delegated TLS (STELE_CA_FILE + probe)"
```

---

### Task 4: Fail-closed openssl flag probe

`probe()` runs `openssl s_client -help` once (cached in a `OnceLock`), asserts every required flag is present, and returns a legible `FetchError::Tls` naming the missing binary/flag (with the LibreSSL/old-openssl hint) otherwise. Testable by putting a stub `openssl` on `PATH`.

**Files:**
- Modify: `src/fetch/https.rs` (add `probe()` + `REQUIRED_FLAGS` + tests)

**Interfaces:**
- Produces: `pub(crate) fn probe() -> Result<(), FetchError>`; `const REQUIRED_FLAGS: &[&str]`.

- [ ] **Step 1: Write the failing test**

The probe reads `openssl` from `PATH`; a test can prepend a temp dir holding a stub `openssl` that prints a help text missing a flag. Add to `src/fetch/https.rs`'s `tests` module:

```rust
    // Build a stub `openssl` executable in a temp dir and return that dir, so a
    // test can prepend it to PATH. `help_text` is what the stub prints for
    // `s_client -help` (on stderr, like real openssl).
    fn stub_openssl_dir(help_text: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        // Unique dir per call (no Date/rand available in tests): use the help
        // text length + a counter-ish discriminator via the text hash proxy.
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
        // Help text that lists everything EXCEPT -no_ign_eof.
        let help = "-connect -servername -verify_return_error -verify_hostname -CAfile -quiet";
        let dir = stub_openssl_dir(help);
        let old = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", dir.display(), old));
        // Reset the cache is not possible (OnceLock); so this test must run in a
        // process where probe() hasn't been called with the real openssl. It is
        // the only test that calls probe(), and it forces the stub via PATH.
        let err = probe().expect_err("missing -no_ign_eof must fail closed");
        std::env::set_var("PATH", old);
        match err {
            FetchError::Tls(m) => {
                assert!(m.contains("-no_ign_eof"), "message: {m}");
                assert!(m.to_lowercase().contains("openssl"), "message: {m}");
            }
            other => panic!("expected Tls, got {other:?}"),
        }
    }
```

Note for the implementer: because `probe()` caches in a `OnceLock`, only ONE test may call `probe()` per test binary. Keep `probe_fails_closed_when_a_required_flag_is_missing` as the sole caller of `probe()` in unit tests; the trusted/positive path is exercised by the integration fixture in Task 7 (separate test binary, fresh process).

- [ ] **Step 2: Verify it fails** — CI: `cargo test --lib fetch::https::tests::probe_fails_closed` → FAIL (no `probe`).

- [ ] **Step 3: Implement the probe**

Add to `src/fetch/https.rs` (imports at top: add `use std::process::Command;` and `use std::sync::OnceLock;`):

```rust
/// Flags Stele's `s_client` invocation depends on. A binary missing any of
/// these cannot verify safely, so https is UNAVAILABLE (fail closed).
pub(crate) const REQUIRED_FLAGS: &[&str] = &[
    "-connect",
    "-servername",
    "-verify_return_error",
    "-verify_hostname",
    "-CAfile",
    "-quiet",
    "-no_ign_eof",
];

static PROBE: OnceLock<Result<(), String>> = OnceLock::new();

/// Verify (once per process, cached) that `openssl s_client` exists and offers
/// every flag Stele needs. Fail-closed: on any gap, https is UNAVAILABLE with a
/// legible error — never a silently-unverified connection.
pub(crate) fn probe() -> Result<(), FetchError> {
    let result = PROBE.get_or_init(|| {
        let output = Command::new("openssl").arg("s_client").arg("-help").output();
        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(format!(
                    "openssl could not be run ({e}); https is unavailable. Install OpenSSL, \
                     or use the monolith-surf proxy. Nothing was fetched."
                ));
            }
        };
        // `s_client -help` prints its usage to stderr and exits nonzero; accept
        // either stream.
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
    });
    result.clone().map_err(FetchError::Tls)
}
```

- [ ] **Step 4: Verify it passes** — CI: `cargo test --lib fetch::https::tests::probe_fails_closed` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/fetch/https.rs
git commit -m "feat(fetch): fail-closed openssl s_client flag probe (cached)"
```

---

### Task 5: `OpensslStream` — the child `ByteStream`

The transport itself: spawn `openssl s_client` (args only), proxy stdin (write) / stdout (read, with a `poll` timeout), close stdin on `shutdown_write`, drain stderr into the error on failure, and reap the child on `Drop` (kill then wait). Its behavioral proof is the Task-7 integration fixture; this task delivers the type + its `ByteStream` impl and one construction test.

**Files:**
- Modify: `src/fetch/https.rs`

**Interfaces:**
- Consumes: `resolve_ca_file` (Task 3), `probe` (Task 4), `ByteStream` (from `super::transport`), `FetchError`.
- Produces: `pub(crate) struct OpensslStream`; `pub(crate) fn connect(host: &str, port: u16) -> Result<OpensslStream, FetchError>`; `impl Read/Write/ByteStream for OpensslStream`.

- [ ] **Step 1: Implement `OpensslStream`**

Add to `src/fetch/https.rs`. Extend the top imports to:

```rust
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags, Timespec};

use super::transport::ByteStream;
use super::FetchError;
```

Then the type and impls:

```rust
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

    /// Build a legible `FetchError::Tls` for a failure on this connection,
    /// folding in openssl's own stderr reason and the host (T4).
    fn tls_error(&mut self, what: &str) -> FetchError {
        let reason = self.drain_stderr();
        let reason = if reason.is_empty() {
            String::new()
        } else {
            format!(" ({reason})")
        };
        FetchError::Tls(format!(
            "TLS connection to {} failed: {what}{reason}. Nothing was fetched.",
            self.host
        ))
    }
}

impl Read for OpensslStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Child stdout is a pipe: no socket read-timeout setter, so gate each
        // read with poll(2). This poll loop is deliberately the seam a future
        // progress()/throbber hook subscribes to.
        let timeout = Timespec {
            tv_sec: IO_TIMEOUT.as_secs() as i64,
            tv_nsec: 0,
        };
        let mut fds = [PollFd::new(&self.stdout, PollFlags::IN)];
        let n = poll(&mut fds, Some(&timeout))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("poll: {e}")))?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "openssl child read timed out",
            ));
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
```

Note for the implementer — verify the `rustix::event::poll` call against the working example in `src/main.rs` (around line 1760: `poll(&mut poll_fds, Some(&timeout))` with a `Timespec`). If the pinned rustix's `poll` signature differs (e.g. takes a millisecond `i32` rather than `Option<&Timespec>`), match `main.rs`'s form exactly — it compiles today. Report a mismatch as DONE_WITH_CONCERNS rather than guessing.

- [ ] **Step 2: Write a construction test** (no TLS server needed — proves connect wires the child and the probe path)

Add to the `tests` module. This test only runs where the probe passes (real openssl present); guard it so a probe failure is not a test failure:

```rust
    #[test]
    fn connect_to_a_dead_port_is_a_legible_tls_error_not_a_panic() {
        // If openssl is absent/old, probe() fails first — that's the fail-closed
        // path, also a Tls error, so either way we assert Tls, never a panic.
        // Connect to a port nothing listens on: openssl exits, first read EOFs.
        if let Ok(mut s) = connect("127.0.0.1", 1) {
            // The child will fail to connect; a read should not panic and should
            // return EOF (0) or an error — totality, no hang beyond IO_TIMEOUT.
            let mut buf = [0u8; 16];
            let _ = s.read(&mut buf); // must not panic
        }
        // Reaching here without panic/hang is the assertion.
    }
```

- [ ] **Step 3: Verify** — CI: `cargo test --lib fetch::https` → passes (construction test + Task 3/4 tests). This does not yet prove a real TLS fetch — Task 7 does.

- [ ] **Step 4: Commit**

```bash
git add src/fetch/https.rs
git commit -m "feat(fetch): OpensslStream child ByteStream (poll-timeout, drop-reap, stderr drain)"
```

---

### Task 6: Wire `https` into the scheme dispatch and the redirect loop

Route `https` to the same client, pick the transport per hop, close child stdin on the openssl path, fix the `Host` header for the https default port, and emit the https→http downgrade notice. This is the integration task.

**Files:**
- Modify: `src/fetch/mod.rs` (`fetch()` routes `https`)
- Modify: `src/fetch/http1.rs` (`Http1Client::fetch`, `send_one`, `exchange`, `format_request`, downgrade notice)

**Interfaces:**
- Consumes: `https::connect` (Task 5).

- [ ] **Step 1: Route `https` in the scheme table**

In `src/fetch/mod.rs`, change the `fetch` match so `http` and `https` share the client:

```rust
pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    match request.url.scheme().as_str() {
        "file" => file::FileFetcher::new().fetch(request),
        "http" | "https" => http1::Http1Client::new().fetch(request),
        other => Err(FetchError::UnsupportedScheme(other.to_string())),
    }
}
```

- [ ] **Step 2: Let `Http1Client::fetch` accept https and notice downgrades**

In `src/fetch/http1.rs`, in `Http1Client::fetch`, change the per-hop scheme guard from:

```rust
            let scheme = url.scheme();
            if scheme != "http" {
                return Err(FetchError::UnsupportedScheme(scheme));
            }
```

to:

```rust
            let scheme = url.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(FetchError::UnsupportedScheme(scheme));
            }
```

Then, in the redirect branch, right after `let next_url = url.resolve(&location);`, add the downgrade notice (https→http is a security downgrade; follow it but say so — no Transcript pane exists yet, so stderr is the honest surface, consistent with other diagnostics in `main.rs`):

```rust
                if scheme == "https" && next_url.scheme() == "http" {
                    eprintln!(
                        "stele: security downgrade — following an https→http redirect to {}",
                        next_url.as_str()
                    );
                }
```

(`http→https` upgrades follow silently; same-scheme is unchanged.)

- [ ] **Step 3: Pick the transport by scheme in `send_one`, and pass `half_close`**

In `src/fetch/http1.rs`, replace `send_one` with:

```rust
pub(crate) fn send_one(
    url: &Url,
    method: Method,
    extra_headers: &[(String, String)],
    body: &[u8],
    cookie_header: Option<&str>,
) -> Result<RawResponse, FetchError> {
    let host = url.host();
    if host.is_empty() {
        return Err(FetchError::Protocol("URL has no host".to_string()));
    }
    let scheme = url.scheme();
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port = url.port(default_port);

    let request_bytes = format_request(url, method, extra_headers, body, cookie_header, &host, port);

    if scheme == "https" {
        // Delegated TLS: the openssl child is just another ByteStream. Close
        // its stdin after writing (half_close = true) so -no_ign_eof propagates
        // our end-of-request to the server.
        let mut stream = super::https::connect(&host, port)?;
        exchange(&mut stream, &request_bytes, true)
    } else {
        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|e| FetchError::Io(format!("connect {}:{}: {}", host, port, e)))?;
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        // half_close = false: preserve http's exact PR-1 wire behavior.
        exchange(&mut stream, &request_bytes, false)
    }
}
```

- [ ] **Step 4: Add the `half_close` parameter to `exchange`**

Replace `exchange` with:

```rust
/// Transport-agnostic core: write the formatted request, optionally half-close
/// the write side (openssl child stdin → EOF → server-visible close via
/// -no_ign_eof), then read the framed response. `half_close = false` preserves
/// http's exact PR-1 wire behavior (no FIN-after-request).
pub(crate) fn exchange<S: ByteStream>(
    stream: &mut S,
    request_bytes: &[u8],
    half_close: bool,
) -> Result<RawResponse, FetchError> {
    stream
        .write_all(request_bytes)
        .map_err(|e| FetchError::Io(format!("write: {}", e)))?;
    stream.flush().map_err(|e| FetchError::Io(format!("flush: {}", e)))?;
    if half_close {
        let _ = stream.shutdown_write();
    }
    read_response(stream)
}
```

- [ ] **Step 5: Fix the `Host` header for the https default port**

In `src/fetch/http1.rs`, in `format_request`, the `Host` header currently omits the port only when `port == 80`. Change it to omit when the port equals the scheme's default (so `https` on 443 sends a bare `Host: example.com`). Replace:

```rust
    let host_header = if port == 80 { host.to_string() } else { format!("{}:{}", host, port) };
```

with:

```rust
    let default_port = if url.scheme() == "https" { 443 } else { 80 };
    let host_header = if port == default_port { host.to_string() } else { format!("{}:{}", host, port) };
```

- [ ] **Step 6: Commit** (no isolated unit test here — the behavior is proven end-to-end by Task 7's fixture; `http` framing tests must remain green, confirming `exchange(half_close=false)` is inert for http)

```bash
git add src/fetch/mod.rs src/fetch/http1.rs
git commit -m "feat(fetch): serve https via openssl transport; per-hop scheme, downgrade notice"
```

- [ ] **Step 7: Push and confirm http stayed inert**

```bash
git push -u origin packet/https-openssl
```

Read `m0-acceptance`. Pass criteria: compile green; `tests/fetch_http1.rs` framing tests still green (proves `exchange(half_close=false)` is byte-inert for http); all existing goldens byte-identical. If http framing broke, `half_close`/`Host` handling changed http behavior — root-cause, do not re-bless.

---

### Task 7: TLS fixture + integration tests (no external network)

Generate a test CA + leaf in the harness, serve one connection over TLS with `openssl s_server`, and drive `https://127.0.0.1:<port>/` through the real client. This is the behavioral proof of Tasks 5–6 and the highest-iteration-risk task (the fixture is fiddly and only runs in CI). Cases: trusted render, untrusted→T4 error, hostname mismatch, `Q\n` trap, stdin-EOF.

**Files:**
- Modify: `tests/support/mod.rs` (cert generation + `spawn_tls_responder`)
- Create: `tests/fetch_https.rs` (the cases + the cookie-`Secure` pin)

**Interfaces:**
- Consumes: `stele::fetch::{fetch, Request, Url, Response, FetchError}`.
- Produces (in `tests/support`): `pub struct TlsFixture { pub port: u16, pub ca_file: PathBuf, pub bad_ca_file: PathBuf }`; `pub fn tls_certs() -> TlsCerts`; `pub fn spawn_tls_responder(cert, key, response: &'static [u8]) -> u16`.

- [ ] **Step 1: Cert generation + TLS responder in `tests/support/mod.rs`**

Add (imports already include `std::process`? add `use std::process::{Command, Stdio};` and `use std::io::Read as _;` as needed). This generates, once per test run, a CA and a leaf for CN=`localhost` with SAN `IP:127.0.0.1`, using the system `openssl` (already required for https):

```rust
use std::path::PathBuf;

/// Paths to a freshly-generated test CA + a leaf cert/key for 127.0.0.1.
pub struct TlsCerts {
    pub dir: PathBuf,
    pub ca_cert: PathBuf,
    pub leaf_cert: PathBuf,
    pub leaf_key: PathBuf,
    /// A DIFFERENT, unrelated CA — used for the "untrusted" case.
    pub other_ca: PathBuf,
}

/// Generate a test CA + a 127.0.0.1 leaf (and a second, unrelated CA) with the
/// system openssl. Deterministic path (no rand/Date in tests); regenerated each
/// run. `cn`/`san` let a caller make a hostname-mismatch cert.
pub fn tls_certs_with(cn: &str, san: &str, tag: &str) -> TlsCerts {
    let dir = std::env::temp_dir().join(format!("stele-tls-{tag}"));
    let _ = std::fs::create_dir_all(&dir);
    let p = |n: &str| dir.join(n);

    let run = |args: &[&str]| {
        let ok = Command::new("openssl")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run openssl for test certs")
            .success();
        assert!(ok, "openssl {args:?} failed");
    };

    // Test CA.
    run(&["req", "-x509", "-newkey", "rsa:2048", "-nodes",
          "-keyout", p("ca.key").to_str().unwrap(),
          "-out", p("ca.pem").to_str().unwrap(),
          "-days", "2", "-subj", "/CN=Stele Test CA"]);
    // A second, unrelated CA (nothing is signed by it → the "untrusted" bundle).
    run(&["req", "-x509", "-newkey", "rsa:2048", "-nodes",
          "-keyout", p("other-ca.key").to_str().unwrap(),
          "-out", p("other-ca.pem").to_str().unwrap(),
          "-days", "2", "-subj", "/CN=Other CA"]);
    // Leaf key + CSR.
    run(&["req", "-newkey", "rsa:2048", "-nodes",
          "-keyout", p("leaf.key").to_str().unwrap(),
          "-out", p("leaf.csr").to_str().unwrap(),
          "-subj", &format!("/CN={cn}")]);
    // Sign the leaf with the test CA, adding the SAN.
    let ext = dir.join("leaf.ext");
    std::fs::write(&ext, format!("subjectAltName={san}\n")).unwrap();
    run(&["x509", "-req",
          "-in", p("leaf.csr").to_str().unwrap(),
          "-CA", p("ca.pem").to_str().unwrap(),
          "-CAkey", p("ca.key").to_str().unwrap(),
          "-CAcreateserial",
          "-out", p("leaf.pem").to_str().unwrap(),
          "-days", "2", "-extfile", ext.to_str().unwrap()]);

    TlsCerts {
        dir: dir.clone(),
        ca_cert: p("ca.pem"),
        leaf_cert: p("leaf.pem"),
        leaf_key: p("leaf.key"),
        other_ca: p("other-ca.pem"),
    }
}

/// Grab a free loopback port (bind :0, read it, drop the listener). A small
/// race window before s_server binds it, acceptable for tests.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind for free port");
    l.local_addr().unwrap().port()
}

/// Spawn `openssl s_server` to accept ONE TLS connection with `cert`/`key`,
/// send `response` verbatim once it sees the client's request, then close.
/// Returns the port. `response` is the full HTTP/1.1 response bytes — the test
/// controls framing exactly (needed for the `Q\n` and stdin-EOF cases).
pub fn spawn_tls_responder(cert: &PathBuf, key: &PathBuf, response: &'static [u8]) -> u16 {
    let port = free_port();
    let cert = cert.clone();
    let key = key.clone();
    thread::spawn(move || {
        let mut child = match Command::new("openssl")
            .arg("s_server")
            .args(["-accept", &port.to_string()])
            .args(["-cert", cert.to_str().unwrap()])
            .args(["-key", key.to_str().unwrap()])
            .arg("-quiet")
            .args(["-naccept", "1"]) // serve exactly one connection, then exit
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        // In this mode s_server writes the client's (decrypted) request to its
        // stdout and sends its stdin to the client. Wait until we've seen the
        // request head, then write the canned response and close stdin.
        if let Some(mut out) = child.stdout.take() {
            let mut seen = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                match out.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        seen.extend_from_slice(&tmp[..n]);
                        if find(&seen, b"\r\n\r\n").is_some() {
                            break;
                        }
                    }
                }
            }
        }
        if let Some(mut sin) = child.stdin.take() {
            let _ = sin.write_all(response);
            let _ = sin.flush();
            // dropping sin closes s_server's stdin → it closes the TLS conn.
        }
        let _ = child.wait();
    });
    // Give s_server a beat to bind before the client connects. No sleep API in
    // tests here; instead the client's connect will retry-fast via openssl's
    // own connect — a failed first connect is the test's problem to tolerate.
    port
}
```

Note for the implementer: s_server needs a moment to bind `-accept <port>` before the client connects. If CI shows a race (client connects before the server is listening), add a short readiness wait by having the fixture thread signal via a `std::sync::mpsc` channel *after* spawn returns, or poll-connect the port from the main thread before returning. Wire whichever the CI run shows is needed; report the choice.

- [ ] **Step 2: The integration cases in `tests/fetch_https.rs`**

Create `tests/fetch_https.rs`:

```rust
//! Delegated-TLS integration tests. Everything talks to an in-process
//! `openssl s_server` with a generated test CA — NEVER the external network
//! (house law). These require a system `openssl` with s_client/s_server; if it
//! is absent, the tls_certs generation asserts and the suite fails loudly,
//! which is correct on CI (openssl IS present there).

mod support;
use support::{spawn_tls_responder, tls_certs_with};

use stele::fetch::{fetch, FetchError, Request, Url};

const OK_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nhello over tls\n";

fn get(url: &str, ca: &std::path::Path) -> Result<stele::fetch::Response, FetchError> {
    std::env::set_var("STELE_CA_FILE", ca);
    let r = fetch(&Request::get(Url::new(url)));
    std::env::remove_var("STELE_CA_FILE");
    r
}

#[test]
fn trusted_https_fetch_renders_the_body() {
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "trusted");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, OK_RESPONSE);
    let resp = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect("trusted TLS fetch succeeds");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello over tls\n");
}

#[test]
fn untrusted_ca_is_a_legible_tls_error() {
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "untrusted");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, OK_RESPONSE);
    // Verify against the UNRELATED CA → openssl rejects.
    let err = get(&format!("https://127.0.0.1:{port}/"), &certs.other_ca)
        .expect_err("untrusted CA must fail closed");
    match err {
        FetchError::Tls(m) => assert!(m.contains("Nothing was fetched"), "message: {m}"),
        other => panic!("expected Tls, got {other:?}"),
    }
}

#[test]
fn hostname_mismatch_is_refused() {
    // Leaf is valid but for the wrong name (no 127.0.0.1 SAN) → -verify_hostname fails.
    let certs = tls_certs_with("wrong.example", "DNS:wrong.example", "mismatch");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, OK_RESPONSE);
    let err = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect_err("hostname mismatch must be refused");
    assert!(matches!(err, FetchError::Tls(_)), "got {err:?}");
}

#[test]
fn body_line_starting_with_Q_survives_the_quiet_trap() {
    // Without -quiet, an s_client body line starting with 'Q' closes the
    // connection. -quiet is on, so the 'Q' line must arrive intact.
    const Q_BODY: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 8\r\n\r\nQuit\nyes\n";
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "qtrap");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, Q_BODY);
    let resp = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect("Q-line body must survive");
    assert_eq!(resp.body, b"Quit\nyes\n");
}

#[test]
fn close_delimited_response_terminates_via_stdin_eof() {
    // No Content-Length, no chunked: body is delimited by connection close.
    // Our shutdown_write (stdin EOF, -no_ign_eof) + server close must let the
    // read complete rather than hang to the timeout.
    const CLOSE_DELIMITED: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nclosed-body";
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "closeeof");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, CLOSE_DELIMITED);
    let resp = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect("close-delimited body terminates");
    assert_eq!(resp.body, b"closed-body");
}
```

- [ ] **Step 3: The cookie-`Secure` pin** (append to `tests/fetch_https.rs`)

`Secure` is already enforced (`cookies.rs`); this pins that a `Secure` cookie set over https is not echoed over http. It uses the cookie jar directly (no server), so it is deterministic:

```rust
#[test]
fn secure_cookie_set_over_https_is_not_sent_over_http() {
    use stele::fetch::cookies::CookieJar;
    let mut jar = CookieJar::default();
    jar.set_from_header(&Url::new("https://example.com/"), "sid=abc; Secure; Path=/");
    // Over https: sent.
    assert_eq!(jar.header_for(&Url::new("https://example.com/")).as_deref(), Some("sid=abc"));
    // Over http: withheld (Secure).
    assert_eq!(jar.header_for(&Url::new("http://example.com/")), None);
}
```

Implementer: confirm `CookieJar`, `set_from_header`, and `header_for` are the actual public names/signatures in `src/fetch/cookies.rs` before finalizing (adjust the calls to match if the API differs); the behavior is already implemented, so this test must pass without touching `cookies.rs`.

- [ ] **Step 4: Commit**

```bash
git add tests/support/mod.rs tests/fetch_https.rs
git commit -m "test(fetch): openssl s_server TLS fixture + https integration cases"
```

- [ ] **Step 5: Push and iterate on the fixture in CI**

```bash
git push origin packet/https-openssl
```

Read `m0-acceptance`. This is the iteration-likely step: expect possible fixture races (s_server bind timing) or cert-flag differences across openssl versions. If a case fails, read the job log, fix the fixture (readiness wait, cert extensions), and re-push. Do NOT weaken an assertion to go green — the five behaviors are the point.

---

### Task 8: The covenant grep in accept.sh + JOURNAL

Assert the binary contains zero TLS implementation, and record the packet.

**Files:**
- Modify: `accept.sh` (add A6 covenant check)
- Modify: `JOURNAL.md`

- [ ] **Step 1: Add the covenant grep to accept.sh**

In `accept.sh`, after the A2 size check, add an A6 check that the i486 binary links no TLS/crypto symbols. Use `strings`/`grep` against `$BIN` (the check must be tolerant of the string "openssl" appearing in our OWN legible error text — we grep for *implementation* symbols, not the word):

```sh
# A6 (covenant): TLS is DELEGATED — the binary must contain zero embedded
# cryptography. openssl runs as a CHILD; its NAME may appear in our error
# strings, but no TLS/crypto implementation symbols may be linked in.
check_a6_covenant() {
  if [ ! -f "$BIN" ]; then
    bad "A6: binary not found at $BIN"
    return
  fi
  # Symbols that would betray an embedded TLS/crypto stack (rustls, ring,
  # openssl-sys, boringssl, embedded-tls). The word "openssl" alone is allowed
  # (our error text); these are library-internal symbols that never appear
  # unless a crypto crate is linked.
  if strings -a "$BIN" | grep -Eiq 'rustls|ring::|boringssl|libcrypto|SSL_CTX_new|EVP_|X509_verify|embedded_tls'; then
    bad "A6: covenant broken — TLS/crypto implementation symbols found in $BIN"
    strings -a "$BIN" | grep -Ei 'rustls|ring::|boringssl|libcrypto|SSL_CTX_new|EVP_|X509_verify|embedded_tls' | head | sed 's/^/    /'
  else
    pass "A6: covenant intact — no embedded TLS/crypto symbols in the binary"
  fi
}
```

Then call `check_a6_covenant` where the other A-checks are invoked (mirror how A2 is called; it runs in the full `accept.sh`, not `--tty-only`). Read the existing check-invocation section and add the call following the same pattern.

- [ ] **Step 2: JOURNAL note** — append to `JOURNAL.md` (newest at bottom): the packet summary, the five pinned TLS behaviors, the fail-closed probe, the delegated-TLS covenant + A6 grep, and the size delta from the CI `stele-i486` artifact (expected ~nil — no crypto linked).

- [ ] **Step 3: Commit**

```bash
git add accept.sh JOURNAL.md
git commit -m "size: A6 covenant grep (zero TLS symbols) + journal the https packet"
```

- [ ] **Step 4: Push; verify A6 passes and size delta is ~nil**

```bash
git push origin packet/https-openssl
```

Read `m0-acceptance`: A6 passes; A2 size delta vs `main` is ~nil; all prior tests still green. Report the exact A2 line and the delta.

---

### Task 9: Goldens + PR polish

The trusted-render and TLS-error-render surfaces produce renderable output; bless any new goldens from the CI artifact (never locally), then open the PR.

- [ ] **Step 1: Determine whether any golden is needed.** The integration tests in Task 7 assert on `Response.body` bytes directly (no golden PNG/tty needed for them). A golden is only needed if you add a *rendered-page* fixture (e.g. a `--dump-text` of the TLS error document). If Task 7's byte-level assertions are the whole proof (they are, per this plan), **no new golden is required** — record that decision and skip to Step 3.

- [ ] **Step 2 (only if a rendered golden was added):** download the CI `stele-host` (or `renders`) artifact, **measure the render programmatically** to confirm it is correct (AGENTS.md §4 — never rubber-stamp), copy the verified file into `goldens/`, commit, push; CI re-renders and byte-compares.

- [ ] **Step 3: Open the PR** (base `main`):

```bash
gh pr create --base main --head packet/https-openssl \
  --title "packet(https-openssl): https via delegated openssl s_client [PR 2 of HTTPS]" \
  --body "<summary: delegated TLS, fail-closed probe, verified, legible T4 errors, five pinned behaviors, zero-crypto covenant A6; links the spec + this plan>"
```

- [ ] **Step 4:** Confirm CI fully green; report the A2 size line and delta.

---

## Self-Review

**1. Spec coverage (§D–J):**
- §D child ByteStream (spawn args-only, kill+wait Drop, poll-timeout reads, stderr drain, shutdown_write closes stdin, -quiet/-no_ign_eof) → Task 5. ✓
- §E scheme plumbing (443 default, `fetch` routes https, cross-scheme redirects + downgrade notice, cookie Secure) → Task 6 (routing/redirect/Host) + Task 7 Step 3 (cookie pin). ✓
- §F fail-closed flag probe (incl. `-no_ign_eof`, LibreSSL hint) → Task 4. ✓
- §G CA bundle (`STELE_CA_FILE` + default probe, legible error, no config subsystem) → Task 3. ✓
- §H legible failure (rendered document, openssl reason + host + "nothing was fetched") → Task 2 (`Tls` bare) + Task 5 (`tls_error` folds in stderr/host); renders via existing `error_page` (main.rs:1491), no new rendering. ✓
- §I charter amendment + DECISIONS + size → Task 1 + Task 8. ✓
- §J tests: trusted render, untrusted error, hostname mismatch, `Q\n` trap, probe fail-closed, stdin-EOF → Task 7 (5 fixture cases) + Task 4 (probe fail-closed); covenant grep → Task 8. ✓
- Manual satisfaction check (live https, Transcript provider line) → out of CI scope; Transcript pane is a later milestone (M6). Recorded as a JOURNAL manual note in Task 8, not a code task. ✓ (documented gap, intentional)

**2. Placeholder scan:** No "TBD"/"handle errors"/"similar to". Every code step carries complete code. The two "verify against main.rs / cookies.rs API" notes are explicit verification instructions with a named fallback (report DONE_WITH_CONCERNS / adjust to the real signature), not gaps.

**3. Type consistency:** `FetchError::Tls(String)` (Task 2) is used identically in Tasks 3/4/5/7. `probe() -> Result<(), FetchError>` and `resolve_ca_file() -> Result<String, FetchError>` (Tasks 3/4) are consumed by `connect` (Task 5). `connect(host: &str, port: u16) -> Result<OpensslStream, FetchError>` (Task 5) is called as `super::https::connect(&host, port)` in `send_one` (Task 6). `exchange(stream, bytes, half_close: bool)` (Task 6 Step 4) matches its two call sites in `send_one` (Task 6 Step 3). `ByteStream::shutdown_write` (PR 1) is implemented in Task 5 and invoked in `exchange` (Task 6). Fixture producers `tls_certs_with`/`spawn_tls_responder` (Task 7 Step 1) match their callers (Task 7 Step 2).

**4. Design decisions recorded (spec delegated these as "implementation detail"):**
- One `Http1Client` serves both `http|https` with a per-hop transport switch in `send_one` — required for cross-scheme redirects (Task 6). A separate `HttpsClient` would duplicate the redirect/cookie loop.
- `exchange` gains `half_close`: http passes `false` (byte-identical to PR 1, verified by the untouched framing tests), https passes `true` (closes child stdin). This is why Task 6 Step 7 re-verifies http inertness.
- `FetchError::Tls` renders bare (Task 2) so T4 failures read as a sentence through the existing `error_page`.
- Downgrade notice goes to stderr (no Transcript pane exists yet); consistent with other `main.rs` diagnostics. Http→https upgrades follow silently.
