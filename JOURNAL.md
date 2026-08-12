# JOURNAL

Append-only running log. Newest at the bottom.

## 2026-08-12 — Founding + M0 toolchain

- Repo founded: `tuckermclean/stele-browser` (public). Charter + build brief
  landed via PR #1; base `.gitignore` (tracks `vendor/`, ignores `target/`).
- CI substrate proven: PR #2 boots the monolith-builder image
  (`ghcr.io/tuckermclean/monolith-builder`, pinned by digest
  `sha256:c8978fe3…`, tag `20260811-b25ecd2b24a5`) on a GitHub-hosted runner
  and asserts the toolchain. Findings from the run:
    - rustc 1.99.0-nightly (da80ed070 2026-07-14), toolchain
      `nightly-2026-07-15` active/default; `rust-src` PRESENT.
    - cross `i486-linux-musl-gcc` PRESENT.
    - `qemu-i386` NOT found in image; `cargo-auditable`/`cargo-audit` absent.
- M0 packet (`packet/m0-toolchain`) authored:
    - `targets/i486-monolith-linux-musl.json` derived from the real
      `i586-unknown-linux-musl` spec (dumped via nightly): cpu=i486,
      max-atomic-width=32, relocation static, PIE off, panic=abort,
      hardware x87 float. Validated locally with
      `rustc -Zunstable-options --print cfg` (panic=abort, relocation_model=
      static, target_env=musl, crt-static, x87, atomics ≤ 32 — all as intended).
    - `rust-toolchain.toml` pins `nightly-2026-07-15` + `rust-src`.
    - `.cargo/config.toml` maps the i486 target's linker to
      `i486-linux-musl-gcc` (no global default target — host tests stay native).
    - `Cargo.toml`: `stele` bin; release profile opt-level=z, lto=fat,
      codegen-units=1, panic=abort, strip.
    - `src/main.rs`: M0 hello (no script engine, ever — C3).
    - `accept.sh`: A1 (static i386 ELF) + A4 (qemu-i386 -cpu 486 vs golden)
      live; A2 informational; A3/A5/A6/A7 PENDING against their milestones.
    - `goldens/m0-hello.txt`: the A4 golden.
    - `.github/workflows/m0-acceptance.yml`: build-in-image → accept-on-host.
- Decisions recorded: D1–D7 (see DECISIONS.md).
- **M0 ACCEPTANCE GREEN** (PR #3, run 31625819900): build-in-image → accept-on-host.
    - A1 PASS: `ELF 32-bit LSB executable, Intel 80386, statically linked, stripped`.
    - A4 PASS: `qemu-i386 -cpu 486` output matches golden — the target binary
      executes 486-legal code.
    - A2: 301,372 bytes (0.29 MB) vs 2.0 MB budget — huge headroom.
  Four real toolchain fixes were needed to get the i486 build to link, each
  caught by CI and fixed in turn:
    1. this nightly gates `.json` targets behind `-Zjson-target-spec`.
    2. self-contained musl crt/libc doesn't exist for a custom build-std target
       → drop `crt-objects-fallback`, let the cross gcc supply crt (D1-adjacent).
    3. rustc requires the self-contained object lists empty when self-contained
       is off → remove `pre/post-link-objects-fallback`.
    4. GCC musl toolchain ships `libgcc_eh.a`, not LLVM `libunwind` → alias it
       as `libunwind.a`; panic=abort never unwinds (D7).
- **INTERFACE FREEZE authored** (packet/interface-freeze): the small, typed,
  compiling core Wave 1 builds against. lib+bin split; crate-level modules:
    - `dom::ast` — closed `Node` sum type (Element | Text), arena DOM, AttrMap,
      ElementName. Covenant clean (the A6 grep finds no forbidden substring;
      element semantics live in the UA sheet, not the AST). `dom::parser` stub.
    - `style::ComputedStyle` — the full curated §4 property set with CSS initial
      defaults; `style::{parser,cascade}` stubs (Stylesheet shape frozen).
    - `surface::Surface` trait + `Color` primitive + `MemSurface` (real pixel
      ops for goldens; blit/text stubbed for P9/P5).
    - `fetch::{Request,Response,Url,Method,Fetch}` + `cookies::CookieJar` skeleton.
    - `img::{Decode,RgbaImage,Frame}`; `text::Metrics` trait.
    - `layout::{LayoutNode,BoxContent,Fragment,FragmentKind}` + geometry.
  Host `cargo check --all-targets` clean; 5 unit tests pass (arena, attrs,
  initial style values, mem-surface fill/bounds).
- PROCESS CORRECTION (operator note): Wave 1 (and future implementation) is to
  be executed by IMPLEMENTER + REVIEWER subagents on a cheaper model, with the
  orchestrator (Opus) planning/dispatching/merging only — matching brief §10's
  topology and keeping cost sane. Subagents default to Sonnet.
- NEXT (after this + #3 merge): Wave 1 fans out to implementer subagents —
  P1 parser · P2 CSS · P3 fetch · P4 image decoders · P5 text/metrics — each
  test-first, reviewer-checked, one packet/PR.

## 2026-08-12 — Wave 1 · P1 (dom parser), subagent-driven

- Executed via subagents (Sonnet), orchestrator (Opus) planning/dispatch/merge only.
- P1 `dom::parser`: bespoke tag-soup parser → arena `Dom`, full HTML 4.01 named
  entity table + numeric, void/raw-text handling (`<script>` discarded — no node;
  `<style>/<textarea>/<title>` kept as raw text; `<noscript>` first-class),
  implied-close (p/li/td/th/tr/dd/dt/option), b/i mis-nesting tolerance,
  unclosed-at-EOF, total (never panics). Fixtures: basic.html, soup.html.
- Loop: implementer (26 green) → reviewer (Spec ✅, no Critical; 1 Important +
  2 Minor) → fix round 1 (same agent) → scoped re-review (all addressed, no new
  breakage) → **29 tests green**. Important bug caught before main: self-closing
  `/>` on non-void elements built a wrong tree.
- Deferred Minor (ledger): `<script/>` (self-closed) skips the raw-text slurp, so
  trailing text after a self-closed script is not discarded — rare malformed
  case; note for a later soup-fixture / hardening pass.
- `ast.rs` frozen/untouched; covenant grep clean; std-only; no unsafe.

## 2026-08-12 — Wave 1 · P3 (fetch: HTTP/1.1 + file:// + cookies + fixture server)

- Orchestrator call for this packet: bespoke std-only HTTP/1.1 over
  `std::net::TcpStream` instead of `httparse` (the crate-vendoring apparatus
  isn't wired up yet; unblocking Wave 1 beat waiting) — DECISIONS D8. gzip
  deferred to a later packet — DECISIONS D11; `Accept-Encoding: identity`
  only for now, fixture server answers identity.
- Test-first per brief §10: one red commit (typed `todo!()` skeletons for
  `url.rs`/`http1.rs`/`file.rs` + the full contract/integration suite +
  the in-process fixture server), confirmed failing (`cargo +nightly test`:
  12 cookie-jar panics), then four green implementation commits
  (url → cookies → file → http1).
- `src/fetch/url.rs`: bespoke minimal `Url` parsing (scheme/host/port/path/
  query) + RFC 3986 §5 relative-reference resolution (verified against the
  RFC's own §5.4 worked examples), total — malformed input degrades to an
  opaque path rather than panicking.
- `src/fetch/cookies.rs`: `CookieJar` contract suite — Set-Cookie parsing
  (Domain/Path/Secure; Expires/Max-Age/HttpOnly/SameSite parsed-and-ignored,
  DECISIONS D9), RFC 6265 domain-match (positive/negative, incl. subdomain
  vs host-only) and path-match (segment-boundary, not string-prefix),
  Secure-only-on-https, `header_for` serialization, Netscape jar
  `to_netscape`/`from_netscape` round trip. Leading-dot domain convention
  doubles as the Netscape flag column with no extra struct field
  (DECISIONS D10).
- `src/fetch/file.rs`: `file://` → `Response` (200, extension-based
  content-type); missing file / non-file scheme → `FetchError`, never a
  panic.
- `src/fetch/http1.rs`: `Http1Client` (owns the `CookieJar`, since the
  frozen `Fetch::fetch` signature has no side channel for one). Request
  formatting (Host/User-Agent/Accept/Accept-Encoding/Connection, POST
  Content-Type default + Content-Length). Response parsing is TOTAL:
  status line, case-insensitive/folded headers, Content-Length body,
  chunked body (trailers consumed), read-until-EOF fallback — every
  malformed-input path (garbage bytes, truncated headers, oversized
  Content-Length, bad chunk size) is a `FetchError`, never a panic; a
  64 MiB response cap bounds memory. Redirect loop follows 301/302/303/
  307/308 up to 5 hops (`TooManyRedirects` on a 6th), applies the usual
  method-preservation rules, resolves `Location` via `Url::resolve`, and
  runs the cookie jar (send + ingest) at every hop, not just the final one.
  `https://` (or any non-http scheme) is unconditionally
  `FetchError::UnsupportedScheme` — no TLS, ever.
- `tests/support/mod.rs`: in-process fixture HTTP server (bind
  `127.0.0.1:0`, background thread, one thread per connection) serving
  `fixtures/*`, chunked/content-length/redirect-chain/echo-method/
  POST-echo/Set-Cookie routes, plus a raw-bytes one-shot listener for
  malformed-response tests. Tests never touch the external network.
- `git diff --stat` on `src/fetch/mod.rs` vs the freeze commit shows only
  three added `pub mod` lines (file/http1/url) — no frozen type or trait
  signature changed. No new deps: `Cargo.toml`/`Cargo.lock` unchanged.
- **75 tests green** (`cargo +nightly test`: 41 lib incl. 12 cookie
  contract tests + 4 file + 17 http1 + 13 url), 0 failed.
