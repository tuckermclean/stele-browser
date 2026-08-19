# HTTPS via delegated `openssl s_client` — design

**Date:** 2026-08-19
**Status:** approved design, pre-implementation
**Scope:** two packets — (1) `packet/bytestream-seam`, (2) `packet/https-openssl`

## Goal

Click an `https://` link and get the page. Nothing else — no core split, no async
runtime, no new scheme surface beyond `https`. TLS is **delegated, never embedded**: zero
bytes of cryptography in the binary; the Linux provider is the user's own `openssl`,
exec'd as a child process.

This is a charter amendment. `stele-charter.md` currently says *"TLS: none in v1 —
monolith-surf owns modernity; direct-TLS is a later optional feature (rustls/embedded-tls
…)"*. The delegated-`openssl`-child is a **third** path the charter did not foresee, and
strictly cheaper on the floppy budget than embedded crypto. It is admitted here by C2's
amendment process and recorded in `DECISIONS.md` (first transport entry).

## Non-negotiables this design serves

- **No JavaScript / no uninvited computation (C3).** Untouched — this is transport only.
- **1.44 MB floppy = 1,474,560 bytes.** Delegated TLS links **zero** crypto; expected
  size delta ~nil. Measured against the CI `stele-i486` artifact / the A2 line, not
  guessed.
- **CI compiles; we usually don't.** Push branches; `m0-acceptance` builds host + i486,
  runs `cargo test` and `accept.sh`. Goldens blessed from the CI `stele-host` artifact
  after programmatic pixel-verification — never rubber-stamped, never i486-built locally.
- **Test-first, root-cause-first.** Every behavioral change lands with a failing test
  first.
- **Parsing is TOTAL.** Malformed wire input yields a `FetchError`, never a panic — this
  invariant already holds in `http1` and must survive the seam.

## Current state (what the code looks like today)

- `src/fetch/mod.rs` — the `Fetch` trait, `Request`/`Response`/`FetchError`, and the
  frozen `Url` newtype.
- `src/fetch/http1.rs` — `Http1Client: Fetch` over a concrete `std::net::TcpStream`.
  `send_one` connects/writes/reads; the read helpers (`read_response`, `read_fixed_body`,
  `read_until_eof_body`, `read_chunked_body`, `read_line_end`) take `&mut TcpStream` and
  **only read**.
- `src/fetch/url.rs` — bespoke URL parsing; `port(default)` already supports arbitrary
  defaults, so 443 for `https` is a one-liner.
- `src/fetch/cookies.rs` — the cookie jar. **`Secure` is already implemented**:
  `cookies.rs:58` computes `is_secure_origin = scheme == "https"` and gates `Secure`
  cookies on it. This design only pins that behavior with a test.
- **Six copy-pasted scheme-dispatch sites**, each `match url.scheme() { "file" => …,
  "http" => …, other => Err(unsupported) }`: `src/main.rs` (×3, incl. the form-POST path
  that dispatches a full `&Request`), `src/images.rs`, `src/stylesheets.rs`,
  `src/bg_images.rs`, `src/frames.rs`. Cookie jars are **not** persisted across these
  calls today (each constructs `Http1Client::new()` fresh); this design preserves that.
- `tests/support/mod.rs` — an in-process `TcpListener` fixture server (brief §7).
  `tests/fetch_http1.rs` pins framing: chunked decode, Content-Length, garbage
  tolerance. These are the tests PR 1 must keep byte-green, and the module PR 2 extends
  with an `openssl s_server` fixture.

## PR 1 — `packet/bytestream-seam`

**A pure refactor. Zero behavior change. All PNG/tty goldens byte-identical.** The proof
that it is inert is that `tests/fetch_http1.rs` stays green untouched.

### A. The ByteStream seam

```rust
// src/fetch/mod.rs (or a new src/fetch/transport.rs)
pub trait ByteStream: Read + Write {
    /// Close the write half so the peer sees EOF while we still read the
    /// response. TcpStream => shutdown(Shutdown::Write); the openssl child
    /// (PR 2) => close child stdin.
    fn shutdown_write(&mut self) -> std::io::Result<()>;
}

impl ByteStream for std::net::TcpStream {
    fn shutdown_write(&mut self) -> std::io::Result<()> {
        self.shutdown(std::net::Shutdown::Write)
    }
}
```

- `send_one` opens/writes/reads over a `ByteStream` rather than a concrete `TcpStream`.
  `Http1Client` still connects with `TcpStream::connect` exactly as today — the only
  change is the static type it hands downstream.
- The read helpers become generic over `R: Read` (they never write), the smallest change
  that lets a different `ByteStream` slot in for PR 2. No logic edits inside them.
- Totality, timeouts, `MAX_RESPONSE_BYTES`, CRLF-injection rejection: all preserved
  verbatim.

### B. Centralized dispatch

```rust
// src/fetch/mod.rs
pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    match request.url.scheme().as_str() {
        "file" => FileFetcher::new().fetch(request),
        "http" => Http1Client::new().fetch(request),
        other  => Err(FetchError::UnsupportedScheme(other.to_string())),
    }
}
```

The six inline `match url.scheme()` blocks each collapse to a call to `fetch::fetch`,
keeping their own local `.map(|r| r.body)` / `.map_err(|e| format!(...))`. Behavior
identical (same schemes, same errors). After this, adding `https` is **one arm in one
place**.

### C. Tests / proof

- `tests/fetch_http1.rs` framing tests: unchanged, still green (the inertness proof).
- New small dispatch unit test: `fetch::fetch` routes `file://` correctly and returns
  `UnsupportedScheme` for an unknown scheme. (`http` is already covered via the fixture
  server.)
- `accept.sh`: no golden changes expected; if any golden byte-differs, the refactor is
  not inert and that is a bug to root-cause, not to bless.

## PR 2 — `packet/https-openssl`

### D. The child ByteStream

`OpensslStream` implements `ByteStream`:

- Spawn with `std::process::Command` **args only, never a shell**:
  ```
  openssl s_client -quiet -connect HOST:443 -servername HOST \
    -verify_return_error -verify_hostname HOST [-CAfile <ca>]
  ```
- `stdin` = plaintext request (write half); `stdout` = response (read half); `stderr`
  captured for error reporting. `kill_on_drop(true)`; `wait()` to reap. One child per
  connection.
- `shutdown_write` closes child stdin (drops the `ChildStdin`). `Read`/`Write` proxy to
  the child's stdout/stdin. Connect/read timeouts mirror `http1`'s `IO_TIMEOUT`.
- Always send `Connection: close`; child stdout EOF = connection close. The existing
  framing/read code drives it **unchanged** because it is just another `ByteStream`.
- `-quiet` is **correctness, not cosmetics**: without it, `s_client` runs interactive
  mode where a response body line beginning with `Q` closes the connection. Never remove;
  pinned by a test (§J).

### E. Scheme plumbing

- `url.rs`: `https` scheme resolves to default port 443 (`port(443)` at the call site;
  `url.rs` already supports an arbitrary default — no parser change beyond wiring).
- `fetch::fetch` gains `"https" => HttpsClient::new().fetch(request)` (or `Http1Client`
  parameterized by a connector — implementation detail; the read/parse path is shared).
- Redirects may cross schemes: `http → https` follows silently (the common upgrade);
  `https → http` emits a **legible downgrade notice** in the status/Transcript line, then
  follows (documented choice).
- Cookie `Secure`: already enforced (`cookies.rs:58`); add a test pinning that a `Secure`
  cookie set over https is **not** sent back over http.

### F. Fail-closed flag probe

On first https use (result cached for the process):

- Parse `openssl s_client -help`; if the binary is absent or any required flag
  (`-quiet`, `-connect`, `-servername`, `-verify_return_error`, `-verify_hostname`,
  `-CAfile`) is missing, https is **UNAVAILABLE** with a legible error naming the
  binary/flag and suggesting the proxy.
- **Never** fall through to a silently-unverified connection. Fail closed.

### G. CA bundle

- Precedence: `$STELE_CA_FILE` if set → else probe the usual paths, first hit wins:
  `/etc/ssl/certs/ca-certificates.crt`, `/etc/pki/tls/certs/ca-bundle.crt`, and the
  other common locations. None found → legible error (name the paths tried).
- **No config-file subsystem** — the binary has none today and this design adds none.
- (Monolith-side follow-up, other repo: ship `ca-certificates`. Out of scope here.)

### H. Legible failure (T4 — K4 extended to transport)

A TLS verify failure produces a **rendered error document**, never a blank screen,
carrying: openssl's stderr reason (expired / hostname mismatch / untrusted CA), the host,
and the sentence "nothing was fetched." Refusal is a document.

### I. Charter + decisions + size

- Amend `stele-charter.md` (the "TLS: none in v1" line, currently line 149) to admit
  delegated TLS: *"TLS is delegated, never embedded — zero bytes of cryptography in the
  binary; the Linux provider is the user's own openssl, exec'd."*
- `DECISIONS.md`: first transport entry — options (embedded rustls / proxy-only /
  exec'd openssl), the choice, the why (floppy budget + covenant: no crypto to audit or
  age), and the revisit-trigger (a no_std TLS story that fits the budget, or dropping the
  openssl dependency assumption).
- Report the binary **size delta against the floppy** from the CI `stele-i486` artifact.
  Expected ~nil (no crypto linked). If non-trivial, root-cause before merge.

### J. Tests (CI, no external network — house law)

Extend `tests/support` with an `openssl s_server` fixture: a generated **test CA + leaf
cert** created in the harness. Cases:

1. **Trusted path** (`-CAfile` = test CA): page renders → golden.
2. **Untrusted** (CA not trusted): T4 error document → golden.
3. **Hostname mismatch** cert: refusal, reason surfaced.
4. **`-quiet` trap**: a body containing `Q\n` at a line start survives intact (pinned
   forever).
5. **Probe fail-closed**: a stub `openssl` on `PATH` missing a required flag → legible
   UNAVAILABLE error.

`accept.sh` additions:

- The **covenant grep**: a symbol/strings audit asserting **zero TLS implementation** in
  the binary (no TLS/crypto symbols linked).
- The five tests above wired into the suite.

**Manual satisfaction check, journaled:** click a live https link on a polite site;
screenshot the page and the Transcript line showing the provider
(`tls: openssl x.y.z, delegated`). (Manual — outside CI, recorded in `JOURNAL.md`.)

## Order & working mode

1. PR 1 (`packet/bytestream-seam`) → review → CI-green → merge.
2. PR 2 (`packet/https-openssl`) off updated `main` → review → CI-green + verified
   goldens + size delta → merge.

Implementation is orchestrated via subagents in isolated git worktrees; the orchestrator
**verifies** (pixel-checks goldens, reads the actual diffs, confirms the size delta) and
never accepts "CI is green" as proof the work is *right*. The shared seam (PR 1) is
pre-assigned to a single packet so parallel branches cannot collide on it.

## Explicitly out of scope (YAGNI)

- Persistent cross-fetch cookie jar (current behavior is fresh-per-fetch; preserved).
- Any config-file subsystem.
- Embedded/no_std TLS.
- Non-Linux TLS providers.
- Connection reuse / keep-alive (one child per connection; `Connection: close`).
- HTTP/2, ALPN negotiation beyond whatever `s_client` defaults to.
