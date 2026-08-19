# ByteStream Seam + Centralized Fetch Dispatch (PR 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a `ByteStream` transport seam under the HTTP/1.1 client and collapse six copy-pasted scheme-dispatch sites into one `fetch::fetch`, with **zero behavior change**, so that PR 2 can add `https` as a single new match arm and a single new `ByteStream` impl.

**Architecture:** Define a `ByteStream: Read + Write` trait (with `shutdown_write`) that `TcpStream` implements; make `http1`'s read/exchange helpers generic over it so a different transport can slot in later. Separately, add `fetch::fetch(&Request)` (the scheme→fetcher table) and `fetch::err_to_string` (preserves each call site's exact error text), and rewire the six wrapper functions to delegate to them.

**Tech Stack:** Rust (std only), `std::net::TcpStream`, the in-repo `tests/support` TCP fixture server. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-19-https-openssl-transport-design.md` (§ "Current state", § "PR 1"). Read it alongside this plan.

## Global Constraints

- **No local builds.** This host is resource-limited; do **not** run `cargo build`/`cargo test`/`accept.sh` locally (AGENTS.md §3; project memory "No local builds — CI does it"). Verification is CI-driven: push the branch, read the `m0-acceptance` workflow (`cargo test` + `accept.sh`). The `cargo` commands shown in verify steps are the **logical assertion** the CI job runs — they name what must pass, not a command to run on this machine.
- **Zero behavior change (this whole PR).** All PNG/tty goldens must stay **byte-identical**; `tests/fetch_http1.rs` framing tests must stay green **untouched**. If any golden byte-differs, the refactor is not inert — root-cause it; **do not re-bless** (AGENTS.md §4).
- **Parsing stays TOTAL.** Malformed wire input yields a `FetchError`, never a panic. Preserve every existing timeout, `MAX_RESPONSE_BYTES` cap, and the CRLF-injection rejection verbatim.
- **No new wire behavior in PR 1.** The `ByteStream::shutdown_write` method is defined and implemented for `TcpStream` but is **not called** on the HTTP path in this PR (calling it would add a FIN-after-request — a real on-the-wire change). It exists as the seam PR 2 consumes.
- **Branch:** `packet/bytestream-seam`, off `main`. Conventional commit subjects (`refactor(fetch):`, `test(fetch):`, `docs(fetch):`). One PR (already open as draft #78).
- **No JavaScript / no uninvited computation (C3):** untouched — this is transport plumbing only.

## File Structure

- **Create** `src/fetch/transport.rs` — the `ByteStream` trait and its `TcpStream` impl. New home so PR 2's `OpensslStream` has a natural place to live; keeps `mod.rs` from growing.
- **Modify** `src/fetch/mod.rs` — declare `pub mod transport;`; add `pub fn fetch(&Request) -> Result<Response, FetchError>` (the scheme table) and `pub fn err_to_string(FetchError) -> String` (exact-text error mapping); add `#[cfg(test)]` unit tests for both.
- **Modify** `src/fetch/http1.rs` — make the read helpers generic over `R: Read`; factor the write+read core into `exchange<S: ByteStream>`; `send_one` connects a `TcpStream` and calls `exchange`. No logic changes inside the helpers.
- **Modify** the six wrapper functions to delegate to `fetch::fetch` + `err_to_string`, and update their doc-comments:
  - `src/main.rs` — `fetch_response` (~line 273), `fetch_request` (~line 1523).
  - `src/images.rs` — `fetch_response` (~line 236).
  - `src/stylesheets.rs` — `fetch_response` (~line 208).
  - `src/bg_images.rs` — `fetch_response` (~line 99).
  - `src/frames.rs` — `fetch_body` (~line 447).

---

### Task 1: The `ByteStream` seam + generic `http1` helpers

Pure refactor of `http1.rs`. Deliverable: `http1` reads/exchanges over a generic transport; `tests/fetch_http1.rs` stays green **unchanged** (that green is the inertness proof — there is no new behavioral test here because nothing behaves differently).

**Files:**
- Create: `src/fetch/transport.rs`
- Modify: `src/fetch/mod.rs` (add `pub mod transport;` only)
- Modify: `src/fetch/http1.rs` (generic helpers + `exchange`)
- Test (unchanged, must stay green): `tests/fetch_http1.rs`

**Interfaces:**
- Produces:
  - `pub trait ByteStream: std::io::Read + std::io::Write { fn shutdown_write(&mut self) -> std::io::Result<()>; }` in `crate::fetch::transport`.
  - `impl ByteStream for std::net::TcpStream`.
  - `pub(crate) fn exchange<S: ByteStream>(stream: &mut S, request_bytes: &[u8]) -> Result<RawResponse, FetchError>` in `http1`.
  - Read helpers re-typed generic over `R: std::io::Read` (same names: `read_response`, `read_fixed_body`, `read_until_eof_body`, `read_chunked_body`, `read_line_end`).

- [ ] **Step 1: Create the transport module**

Create `src/fetch/transport.rs`:

```rust
//! The transport seam under `http1`. `Http1Client` reads/writes over a
//! `ByteStream` rather than a concrete socket, so a different provider can
//! slot in without touching the HTTP/1.1 framing code. PR 1 ships the seam
//! and its only impl (`TcpStream`); PR 2 adds an `openssl s_client` child
//! that implements the same trait (delegated TLS — see
//! docs/superpowers/specs/2026-08-19-https-openssl-transport-design.md).

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};

/// A bidirectional byte transport for one HTTP exchange. `Read` is the
/// response side, `Write` is the request side.
pub trait ByteStream: Read + Write {
    /// Close the write half so the peer sees EOF on our request side while we
    /// keep reading the response. `TcpStream` => `shutdown(Shutdown::Write)`;
    /// PR 2's openssl child => close child stdin. NOTE: not called on the
    /// HTTP path in PR 1 (that would add a FIN-after-request — a wire change);
    /// it is the seam PR 2's transport relies on.
    fn shutdown_write(&mut self) -> io::Result<()>;
}

impl ByteStream for TcpStream {
    fn shutdown_write(&mut self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}
```

- [ ] **Step 2: Declare the module**

In `src/fetch/mod.rs`, add alongside the other `pub mod` lines (after `pub mod http1;`):

```rust
pub mod transport;
```

- [ ] **Step 3: Make the read helpers generic over `R: Read`**

In `src/fetch/http1.rs`, change the signatures below from `&mut TcpStream` to `&mut R` with an `R: Read` bound. **Change only the signatures — every line of the bodies stays exactly as-is.**

```rust
pub(crate) fn read_response<R: Read>(stream: &mut R) -> Result<RawResponse, FetchError> { /* unchanged body */ }

fn read_fixed_body<R: Read>(stream: &mut R, buf: &mut Vec<u8>, start: usize, len: usize) -> Result<Vec<u8>, FetchError> { /* unchanged */ }

fn read_until_eof_body<R: Read>(stream: &mut R, buf: &mut Vec<u8>, start: usize) -> Result<Vec<u8>, FetchError> { /* unchanged */ }

fn read_chunked_body<R: Read>(stream: &mut R, buf: &mut Vec<u8>, start: usize) -> Result<Vec<u8>, FetchError> { /* unchanged */ }

fn read_line_end<R: Read>(stream: &mut R, buf: &mut Vec<u8>, tmp: &mut [u8; 4096], pos: usize) -> Result<usize, FetchError> { /* unchanged */ }
```

The `use std::net::TcpStream;` import stays (still used by `send_one`). Add `use super::transport::ByteStream;` to the imports.

- [ ] **Step 4: Factor the write+read core into `exchange`, and simplify `send_one`**

In `src/fetch/http1.rs`, replace the tail of `send_one` (the write/flush/read part) with a call to a new generic `exchange`. `send_one` keeps connecting the `TcpStream` and setting timeouts exactly as today:

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
    let port = url.port(80);

    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|e| FetchError::Io(format!("connect {}:{}: {}", host, port, e)))?;
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let request_bytes = format_request(url, method, extra_headers, body, cookie_header, &host, port);
    exchange(&mut stream, &request_bytes)
}

/// Transport-agnostic core: write the already-formatted request bytes, then
/// read the framed response. Generic over `ByteStream` so PR 2's openssl
/// child reuses it unchanged. Does NOT call `shutdown_write` (see the trait
/// doc): PR 1 preserves the exact wire behavior `send_one` had before.
pub(crate) fn exchange<S: ByteStream>(
    stream: &mut S,
    request_bytes: &[u8],
) -> Result<RawResponse, FetchError> {
    stream
        .write_all(request_bytes)
        .map_err(|e| FetchError::Io(format!("write: {}", e)))?;
    stream.flush().map_err(|e| FetchError::Io(format!("flush: {}", e)))?;
    read_response(stream)
}
```

- [ ] **Step 5: Commit**

```bash
git add src/fetch/transport.rs src/fetch/mod.rs src/fetch/http1.rs
git commit -m "refactor(fetch): extract ByteStream seam under http1 (no behavior change)"
```

- [ ] **Step 6: Push and verify inertness via CI**

```bash
git push -u origin packet/bytestream-seam
```

Then read the `m0-acceptance` run. Pass criteria (the logical assertion; CI runs it, not this host):
- `cargo test --test fetch_http1` — **all green, unchanged** (framing: chunked, Content-Length, garbage tolerance).
- `cargo test` — compiles and passes; no signature broke a caller.
- `accept.sh` — all PNG/tty goldens **byte-identical**.

If any golden differs: the seam is not inert. Root-cause (likely an accidental body edit in Step 3/4). Do **not** re-bless.

---

### Task 2: Centralized dispatch — `fetch::fetch` + `err_to_string`

Add the single scheme table and the exact-text error mapper, with unit tests. Deliverable: the two functions exist and route/format correctly. Nothing calls them yet (Task 3 rewires callers), so this task is independently reviewable and cannot change rendered output.

**Files:**
- Modify: `src/fetch/mod.rs` (add the two `pub fn`s + `#[cfg(test)]` tests)

**Interfaces:**
- Consumes: `Request`, `Response`, `FetchError`, `Fetch` (all already in `mod.rs`); `file::FileFetcher`, `http1::Http1Client`.
- Produces:
  - `pub fn fetch(request: &Request) -> Result<Response, FetchError>`
  - `pub fn err_to_string(err: FetchError) -> String`

- [ ] **Step 1: Write the failing tests**

Add to `src/fetch/mod.rs` (bottom of file):

```rust
#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn fetch_routes_file_scheme_to_the_file_fetcher() {
        // A file:// URL to a path that does not exist must reach FileFetcher
        // (=> Io error), proving dispatch — NOT UnsupportedScheme.
        let err = fetch(&Request::get(Url::new("file:///stele/does/not/exist")))
            .expect_err("nonexistent file must error");
        assert!(
            matches!(err, FetchError::Io(_)),
            "expected Io (dispatched to FileFetcher), got {err:?}"
        );
    }

    #[test]
    fn fetch_rejects_unknown_scheme() {
        let err = fetch(&Request::get(Url::new("gopher://example.com/")))
            .expect_err("unknown scheme must error");
        assert!(matches!(err, FetchError::UnsupportedScheme(ref s) if s == "gopher"), "got {err:?}");
    }

    #[test]
    fn err_to_string_preserves_unsupported_scheme_text() {
        // Must match the exact string the six call sites produced before
        // centralization: "unsupported scheme: <scheme>".
        let s = err_to_string(FetchError::UnsupportedScheme("gopher".to_string()));
        assert_eq!(s, "unsupported scheme: gopher");
    }

    #[test]
    fn err_to_string_debug_formats_other_errors() {
        let s = err_to_string(FetchError::Protocol("boom".to_string()));
        assert_eq!(s, "Protocol(\"boom\")");
    }
}
```

- [ ] **Step 2: Verify the tests fail (compile error: `fetch`/`err_to_string` undefined)**

Logical assertion (CI): `cargo test --lib fetch::dispatch_tests` → FAIL, `cannot find function fetch` / `err_to_string`.

- [ ] **Step 3: Implement the two functions**

Add to `src/fetch/mod.rs` (after the `Fetch` trait definition), and import the fetchers at the top of the file (`use file::FileFetcher; use http1::Http1Client;` — or reference by path as below):

```rust
/// The single scheme -> fetcher table. Every driver-level wrapper delegates
/// here, so a new scheme (PR 2 adds `https`) is one arm in ONE place instead
/// of six. Cookie jars are intentionally per-call (fresh `Http1Client`), which
/// matches the pre-centralization behavior — no jar was shared across these
/// call sites before.
pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    match request.url.scheme().as_str() {
        "file" => file::FileFetcher::new().fetch(request),
        "http" => http1::Http1Client::new().fetch(request),
        other => Err(FetchError::UnsupportedScheme(other.to_string())),
    }
}

/// Render a `FetchError` as the driver-level `String` the call sites use.
/// Preserves the EXACT text each site produced before centralization: an
/// unsupported scheme reads "unsupported scheme: <scheme>"; everything else
/// is the `Debug` form the sites already showed via `format!("{e:?}")`.
pub fn err_to_string(err: FetchError) -> String {
    match err {
        FetchError::UnsupportedScheme(s) => format!("unsupported scheme: {s}"),
        other => format!("{other:?}"),
    }
}
```

- [ ] **Step 4: Verify the tests pass**

Logical assertion (CI): `cargo test --lib fetch::dispatch_tests` → PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/fetch/mod.rs
git commit -m "test(fetch): add fetch dispatch + err_to_string with unit tests"
```

(Test and implementation in one commit is acceptable here since both are new, additive, and caller-less; the red→green is visible in this task's steps and re-runnable in CI. If you prefer strict red→green history, split Steps 1 and 3 into two commits.)

---

### Task 3: Rewire the six call sites to delegate

Replace each inline `match url.scheme()` with a call to `fetch::fetch` + `err_to_string`, and update the doc-comments so they stay true (the scheme table now lives in one place; the thin per-module wrapper remains because each returns a different shape and some live in the lib vs. bin crate). Deliverable: identical behavior; all goldens byte-identical; framing tests green.

**Files:**
- Modify: `src/main.rs`, `src/images.rs`, `src/stylesheets.rs`, `src/bg_images.rs`, `src/frames.rs`
- Proof (unchanged, must stay green): `tests/fetch_http1.rs`, all `*_golden.rs`

**Interfaces:**
- Consumes: `crate::fetch::fetch`, `crate::fetch::err_to_string` (Task 2). In the bin crate (`main.rs`) these are `stele::fetch::fetch` / `stele::fetch::err_to_string`.

- [ ] **Step 1: Rewire `src/main.rs::fetch_response`**

Replace the `match` body (keep the fn signature and its doc-comment's first paragraph; update the last sentence that says https "is a clean `Err`" — that stays true, just note the table moved):

```rust
fn fetch_response(url: &Url) -> Result<Response, String> {
    stele::fetch::fetch(&Request::get(url.clone())).map_err(stele::fetch::err_to_string)
}
```

Update the doc-comment sentence to: *"Scheme dispatch lives in `fetch::fetch` (one table); every other scheme — including `https`, which this build does not yet serve — comes back a clean `Err`, never a panic."*

- [ ] **Step 2: Rewire `src/main.rs::fetch_request`**

```rust
fn fetch_request(req: &Request) -> Result<Response, String> {
    stele::fetch::fetch(req).map_err(stele::fetch::err_to_string)
}
```

Update its doc-comment to point at `fetch::fetch` as the shared table rather than describing an inline dispatch.

- [ ] **Step 3: Rewire `src/images.rs::fetch_response`**

```rust
fn fetch_response(url: &Url) -> Result<Response, String> {
    crate::fetch::fetch(&Request::get(url.clone())).map_err(crate::fetch::err_to_string)
}
```

Update the doc-comment: it currently argues for *not* sharing a driver module across call sites. Revise to: *"The thin per-module wrapper stays (this one returns the full `Response` for its `Content-Type` decode hint), but the scheme table itself is now shared in `fetch::fetch`, so a new scheme lands once."*

- [ ] **Step 4: Rewire `src/stylesheets.rs::fetch_response`**

```rust
fn fetch_response(url: &Url) -> Result<Response, String> {
    crate::fetch::fetch(&Request::get(url.clone())).map_err(crate::fetch::err_to_string)
}
```

Update the doc-comment the same way (thin wrapper kept; scheme table centralized).

- [ ] **Step 5: Rewire `src/bg_images.rs::fetch_response`**

```rust
fn fetch_response(url: &Url) -> Result<Response, String> {
    crate::fetch::fetch(&Request::get(url.clone())).map_err(crate::fetch::err_to_string)
}
```

Update the doc-comment the same way.

- [ ] **Step 6: Rewire `src/frames.rs::fetch_body`**

Note this one maps to the body and keeps that shape:

```rust
fn fetch_body(url: &Url) -> Result<Vec<u8>, String> {
    crate::fetch::fetch(&Request::get(url.clone()))
        .map(|r| r.body)
        .map_err(crate::fetch::err_to_string)
}
```

Update the doc-comment the same way (thin wrapper kept, returns body; scheme table centralized).

- [ ] **Step 7: Check for now-unused imports**

Each rewired module may no longer use `FileFetcher` / `Http1Client` / `Fetch` directly. Remove any import those modules no longer reference (e.g. `use crate::fetch::http1::Http1Client;`, `use crate::fetch::file::FileFetcher;`). Leave imports still used by other code in the module. (CI will flag `unused_imports` as a warning; on this project treat it as must-fix.)

- [ ] **Step 8: Commit**

```bash
git add src/main.rs src/images.rs src/stylesheets.rs src/bg_images.rs src/frames.rs
git commit -m "refactor(fetch): route all six call sites through fetch::fetch (no behavior change)"
```

- [ ] **Step 9: Push and verify inertness via CI**

```bash
git push origin packet/bytestream-seam
```

Read `m0-acceptance`. Pass criteria:
- `cargo test` — green (framing + all golden tests + new dispatch tests).
- `accept.sh` — every PNG/tty golden **byte-identical** to `main`.
- No `unused_imports` / `dead_code` warnings from the rewire.

If a golden byte-differs: not inert — root-cause; do **not** re-bless.

---

### Task 4: PR polish — flip out of draft

Deliverable: PR #78 ready for review with a green CI and an accurate description.

- [ ] **Step 1: Confirm CI is fully green** on the latest push (build host + i486, `cargo test`, `accept.sh` all pass; renders gallery shows no golden diffs).

- [ ] **Step 2: Update the PR checklist** in #78 (tick the implemented items) and confirm the body still matches what landed.

- [ ] **Step 3: Mark ready for review**

```bash
gh pr ready 78
```

- [ ] **Step 4: Append a JOURNAL.md note** (newest at bottom) summarizing: seam extracted, six sites centralized, zero golden change, size delta ~nil (name the A2 line from CI). Commit and push.

---

## Self-Review

**1. Spec coverage (PR 1 scope only):**
- §A ByteStream seam → Task 1 (trait + `TcpStream` impl + generic helpers + `exchange`). ✓
- §B Centralized dispatch → Task 2 (`fetch::fetch`) + Task 3 (six sites rewired). ✓
- §C Tests/proof → framing tests stay green (Tasks 1 & 3); new dispatch unit tests (Task 2); goldens byte-identical (Tasks 1, 3, 4). ✓
- Cookie `Secure`, https child, charter amendment, `openssl s_server` fixtures → **PR 2**, not this plan. Correctly out of scope.

**2. Placeholder scan:** No "TBD"/"handle errors"/"similar to". The `/* unchanged body */` markers in Task 1 Step 3 are deliberate signed instructions ("change only the signature; bodies stay verbatim") against the existing, already-written functions in `http1.rs`, not gaps to fill.

**3. Type consistency:** `fetch(&Request) -> Result<Response, FetchError>` and `err_to_string(FetchError) -> String` are named identically in Task 2's interface block, its implementation, and all six Task 3 call sites. `exchange<S: ByteStream>` / `read_response<R: Read>` names match between Task 1's interface block and steps. `ByteStream::shutdown_write` signature matches between `transport.rs` and the trait doc.

**4. Behavior-preservation checks baked in:** `err_to_string` exists specifically so the unsupported-scheme string is unchanged (Task 2 pins it); `exchange` deliberately omits `shutdown_write` so the wire bytes are unchanged (Global Constraints + Task 1 trait doc); "byte-identical goldens" is the pass gate on every push step.
