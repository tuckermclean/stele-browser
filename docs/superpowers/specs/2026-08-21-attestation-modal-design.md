# Attestation page ("attestation modal") — design

**Date:** 2026-08-21 · Dogfooding packet, thematic centerpiece. Adds `about:attestations`: a page Stele
renders **through its own engine** listing every third-party attribution + license bundled into the binary —
Stele's own GPL-3.0-or-later notice, the Cargo dependency roster, and Terminus's OFL-1.1 text (closing the
user-facing-attribution judgment call the Terminus packet's design flagged as unresolved, D66). "Stele" = a
stone monument covered in inscriptions; this page is "the most thoroughly attested object on the disc."
**This document is spec + plan only — no implementation.** The companion plan is
`docs/superpowers/plans/2026-08-21-attestation-modal-plan.md`.

## Naming note (read this before "modal" confuses anyone)
There is no floating dialog, overlay, or JS-style modal anywhere in this design — Stele has no such UI
primitive and charter C3 forbids the execution surface a real modal would imply. "Attestation modal" is the
brief's working title for the *feature*; what actually ships is an ordinary **navigated page**, reached via a
URL scheme (`about:attestations`) and a chrome affordance, laid out and painted by the exact same block/inline
pipeline as any other document. This document says "attestation page" throughout and treats "modal" as a
codename, not a literal design constraint.

## Goal
1. A new `about:` URL scheme, handled entirely in-process (no network, no filesystem) — `about:attestations`
   returns an embedded HTML document; any other `about:<x>` (including bare `about:`) degrades to a small,
   honest "unknown about: page" instead of a fetch error (totality, AGENTS.md rule 5's discipline).
2. The embedded HTML aggregates: Stele's own license (GPL-3.0-or-later), the Cargo dependency roster with
   real SPDX license identifiers, and Terminus's OFL-1.1 attribution + full license text — legible at the
   engine's default rendering, inside a size budget priced against the **real, current** floppy headroom.
3. A small chrome affordance in the `--x11` interactive shell that navigates to `about:attestations`.

## Non-negotiables (AGENTS.md, unchanged by this packet)
- **No JavaScript, by construction** (charter C3) — the attestation page is static markup through the
  existing DOM/CSS/layout pipeline; no new executable surface, no new DOM node kind.
- **1.44 MB floppy ceiling** (1,474,560 bytes). Per DECISIONS D66 (Terminus font packet, the most recent
  size-relevant entry), `stele-i486` is **1,377,436 bytes** as of that packet landing — **97,124 bytes
  (≈94.85 KiB) of headroom**, not the ~97 KB figure's imprecise rounding. Reconfirm at implementation time
  (packets may have landed in the interim) before pricing this packet's actual embedded bytes against it.
- **CI-driven build/test.** No local `cargo build`/`cargo test`. Every task pushes and reads `m0-acceptance`;
  goldens are re-blessed from the downloaded CI artifact, never a local render.
- **Goldens are byte-compared; pixel/text-verify before blessing, never rubber-stamp.** New goldens this
  packet adds (a `--dump-text`/`--dump-png` render of `about:attestations`) get the same discipline as any
  other — read the actual text/pixels, don't bless blind.
- **Test-first.** Every code task starts with a failing test before the implementation that turns it green.

## Current state (ground-truthed 2026-08-21, this session)

### The scheme dispatch table (frozen shape, one new arm)
`src/fetch/mod.rs:108-114`:
```rust
pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    match request.url.scheme().as_str() {
        "data" => data::fetch(request),
        "file" => file::FileFetcher::new().fetch(request),
        "http" | "https" => http1::Http1Client::new().fetch(request),
        other => Err(FetchError::UnsupportedScheme(other.to_string())),
    }
}
```
`Request`/`Response` (`src/fetch/mod.rs:33-70`): `Response { status: u16, final_url: Url, headers: Vec<(String,
String)>, body: Vec<u8> }`. `src/fetch/data.rs` is the closest sibling to what `about:` needs: a free `pub fn
fetch(request: &Request) -> Result<Response, FetchError>` (not the `Fetch` trait — that's for stateful
fetchers like `FileFetcher`/`Http1Client`; `data`/`about` are pure functions, no state, no I/O), building a
`Response` by hand with `status: 200`. `about::fetch` is the same shape.

### `Url::scheme()`/`Url::path()` already handle `about:attestations` correctly
`src/fetch/url.rs:70-88`'s `split_scheme` requires the scheme to start with a letter and contain only
letters/digits/`+`/`-`/`.` — `"about"` qualifies. Since `about:attestations` has no `//` authority, `parse()`
(`url.rs:90-135`) skips authority parsing entirely and the whole remainder becomes `path` with **no leading
slash**: `Url::new("about:attestations").path() == "attestations"` (not `"/attestations"`). The `about::fetch`
handler should match on `url.path()` directly (or equivalently strip the `about:` prefix from `url.as_str()`,
mirroring `data::fetch`'s own `strip_prefix` style) — no url-parsing gap to close here, confirmed by tracing
`parse()` by hand against this exact input.

### `resolve_url` — a real gap that MUST be closed for this packet to be testable via the CLI
`src/main.rs:380-392`:
```rust
fn resolve_url(raw: &str) -> Url {
    let scheme = Url::new(raw).scheme();
    if scheme == "http" || scheme == "file" {
        return Url::new(raw);
    }
    // ...falls through to treating `raw` as a filesystem path...
}
```
Every `--dump-text`/`--dump-png`/`--render-fb`/`--x11` entry point resolves its `<src>` argument through this
one function (9 call sites, grep-confirmed: `main.rs:508,538,646,808,980,1095,1171,1804,2408`). Today, passing
`about:attestations` on the CLI does **not** reach `fetch::fetch` at all — `resolve_url` falls through to its
filesystem-path branch, treats the literal string `about:attestations` as a relative path, and produces a
bogus `file://<cwd>/about:attestations` URL. **This packet must add `scheme == "about"` to `resolve_url`'s
passthrough set** — a one-line, load-bearing fix, without which `--dump-text about:attestations` and the CI
goldens this design relies on cannot exist. (Note, out of scope: `resolve_url` also doesn't pass through
`"https"` — a pre-existing gap unrelated to this packet, not touched here.)

### The chrome click-dispatch pattern (`--x11`, `src/backend/chrome.rs` + `run_x11` in `main.rs`)
`chrome::layout(win_w, win_h) -> ChromeLayout` (`chrome.rs:56-90`) computes pixel `Rect`s for `top`, `back`,
`address`, `throbber`, `viewport`, `status` — pure geometry, no I/O. `chrome::draw` paints those rects into a
`Surface` from a `ChromeState` snapshot. `run_x11` (`main.rs:1793`+) owns the interaction: on `ButtonPress`
button 1 it computes `XIntent::Click { x, y }` (`main.rs:2153`), then two independent hit-tests already
coexist at the call site (`main.rs:1948-1993`):
1. `x11_point_in_rect(lay.back, x, y) && history.can_go_back()` → `history.back()` (chrome-bar hit, `main.rs:1949`).
2. `xproto::hit_test_pixel(&state.fragments, doc_x, doc_y)` → resolves a **document** `<a href>` at the click
   point, `history.current().resolve(&href)`, then `history.navigate(new_url)` (`main.rs:1991-1993`) — this is
   how ordinary in-page links already navigate.

The chrome affordance for this packet is architecturally identical to (1), not (2): a **new named rect** in
`ChromeLayout` (alongside `back`/`throbber`), drawn by `chrome::draw`, hit-tested in `run_x11` the same way
`lay.back` is, calling `history.navigate(Url::new("about:attestations"))` directly (no document `href` to
resolve — the target is a fixed, well-known URL, not user content).

### `white-space: pre` is cascaded but **not enforced by layout** — a real, previously undocumented gap
`src/style/computed.rs:174,411` defines and cascades a `WhiteSpace` enum (`Normal`/`Pre`); `ua.rs:60` sets
`pre { white-space: pre; }`; `cascade.rs:1040-1044` even pins a test that `<pre>` cascades `WhiteSpace::Pre`.
But grepping the *consumers* of `white_space` outside `cascade.rs`/`value.rs` turns up **nothing** — and
`src/layout/box_tree.rs:1182`'s own doc comment says it outright: *"`layout::inline`'s bespoke
whitespace-collapsing is unconditional in v1 ('v1 always collapses; a Pre fast-path is a follow-up')... it
collapses ANY run of whitespace to a single space regardless of a node's white-space style."* This is the same
category of finding as the Terminus design's `font-weight: bold` discovery (cascaded, never consumed) —
**do not build this page's license-text rendering around `<pre>`** expecting preserved line breaks; it will
silently collapse every embedded license text into one long run of prose. §Design 4 below designs around this
directly (paragraph-per-blank-line `<p>` splitting, done ahead of time by the generator/embedder, not relying
on runtime whitespace preservation).

### `<ul>`/`<li>`/headings already render correctly (no gap)
`ua.rs:62-77`: `ul{list-style-type:disc}`, `ol{list-style-type:decimal}`, `li{display:list-item}` (real marker
synthesis, packet/display-list-item). `fixtures/kitchen-sink.html:32-39` exercises this exact combination and
has a passing golden. Headings (`h1`-`h6`) are UA-styled and goldened throughout the existing fixture set. The
dependency roster (§Design 3) can safely use a `<ul>` of `<li>` rows; no new layout capability needed anywhere
in this page.

### The real, i486-target-filtered Cargo dependency graph (read directly, not guessed)
`Cargo.toml` declares 5 direct runtime deps: `gif = "0.13"`, `jpeg-decoder = "0.3"` (no rayon), `png = "0.17"`,
`rustix = "1.1.4"` (features `termios, std, event`), `taffy = "0.13"` (default-features off; `std, taffy_tree,
flexbox, block_layout, float_layout, content_size, grid`). `Cargo.lock`'s full resolved graph (30 packages)
was read directly and its dependency EDGES traced (not assumed):
```
stele -> gif, jpeg-decoder, png, rustix, taffy
taffy -> arrayvec, serde, slotmap
gif -> color_quant, weezl
png -> bitflags(1.3.2), crc32fast, fdeflate, flate2, miniz_oxide
flate2 -> crc32fast, miniz_oxide
fdeflate -> simd-adler32
miniz_oxide -> adler2, simd-adler32
rustix -> bitflags(2.13.1), errno, libc, linux-raw-sys, windows-sys
errno -> libc, windows-sys
windows-sys -> windows-link
serde -> serde_core, serde_derive
serde_derive -> proc-macro2, quote, syn
syn/proc-macro2 -> unicode-ident
slotmap -> version_check
```
Two things this trace surfaces that a naive "list everything in Cargo.lock" approach would get wrong:
1. **`windows-sys`/`windows-link`** appear only via `rustix`'s and `errno`'s Windows-only (`cfg(windows)`)
   dependency edges — Cargo.lock is not target-filtered by default, so it lists them even though they never
   compile for `i486-unknown-linux-musl`. **Do not credit them** — the generator (§Design 3) must resolve the
   graph with `cargo metadata --filter-platform <the i486 target triple>` (read the exact triple from
   `targets/` or `rust-toolchain.toml`/`accept.sh` at implementation time) to get the *真* per-target set, not
   the raw lockfile union.
2. **`serde_derive`, `syn`, `quote`, `proc-macro2`, `unicode-ident`, `version_check`** are proc-macro /
   build-script-only dependencies — they run on the **build host** at compile time and contribute **zero
   bytes** to the shipped `stele-i486` binary. `serde` itself (the runtime crate `taffy` actually links
   against, providing `Serialize`/`Deserialize` on its types) is a real runtime dependency and should be
   credited; its build-time derive machinery should not. This is a real, principled scoping call for
   "bundled into Stele" (§Design 3's roster only lists what's actually linked into the artifact) — flagged
   explicitly, not silently narrowed.

**Real per-package licenses, read directly from each crate's own `Cargo.toml` `license` field in the local
`~/.cargo/registry` source cache** (ground truth, not guessed — this environment already has these crates
fetched):

| Crate | SPDX (from its own `Cargo.toml`) | Bundled `LICENSE*` file(s) in the published crate? |
|---|---|---|
| `taffy` 0.13.0 | `MIT` | **No** — the published crate tarball has no `LICENSE*` file at all (confirmed: `ls` on the registry cache). Text must come from the pinned upstream repo (`github.com/DioxusLabs/taffy`), fetched + hashed like the Terminus BDFs were. |
| `gif`, `jpeg-decoder`, `png` | `MIT OR Apache-2.0` | Yes, both `LICENSE-MIT`/`LICENSE-APACHE` |
| `rustix` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | Yes, all three |
| `arrayvec`, `serde`, `bitflags` (both 1.3.2 and 2.13.1), `errno`, `libc`, `linux-raw-sys`, `weezl`, `crc32fast`, `cfg-if`, `fdeflate`, `flate2` | `MIT OR Apache-2.0` | Yes, `LICENSE-MIT`/`LICENSE-APACHE` |
| `adler2` | `0BSD OR MIT OR Apache-2.0` | Yes |
| `miniz_oxide` | `MIT OR Zlib OR Apache-2.0` | Yes |
| `color_quant`, `simd-adler32` | `MIT` (sole) | Yes, one `LICENSE`/`LICENSE.md` |
| `slotmap` | `Zlib` (**sole** — no MIT/Apache option) | Yes, one `LICENSE` (884 bytes) |

Every dual/triple-licensed crate here offers `MIT` as one option — picking MIT uniformly for those lets one
license FAMILY (MIT) cover nearly the entire roster, with `slotmap`'s Zlib as the one genuine outlier. Actual
`LICENSE-MIT` file byte sizes, read directly: mostly **1,020–1,100 bytes** each (`arrayvec` 1071, `serde`
1023, `png` 1048, `rustix` 1023, `libc` 1066, `crc32fast` 1097, …). **Content-hash dedup is real, not
theoretical**: `rustix`, `linux-raw-sys`, and `serde`'s `LICENSE-MIT` files are **byte-identical** (same md5,
confirmed) — the generator naturally collapses these into one shared text block referenced three times, no
manual bookkeeping needed.

### Existing license artifacts to reuse, not re-author
- `LICENSE` (repo root): full **GPL-3.0-or-later** text, **35,149 bytes** — matches `Cargo.toml`'s own
  `license = "GPL-3.0-or-later"` and `REPORT.md`'s License section. Far too large to embed in full (see
  §Design 2's size reasoning) — this page embeds Stele's *own* short GPL notice, not the full text.
- `third_party/terminus-font/OFL.TXT` (verbatim upstream OFL-1.1, **4,480 bytes**) and
  `third_party/terminus-font/NOTICE.md` (**1,791 bytes**, prose attribution) — both already exist from the
  Terminus packet (D66) and are exactly what OFL condition 2 requires travel with the font. `NOTICE.md`
  explicitly says today: *"the compiled `stele-i486` binary does **not** additionally embed the license text
  as bytes... user-facing attribution lands via the forthcoming attestation modal"* (D66's own decision entry,
  `DECISIONS.md` line ~18-19) — **this packet is that forthcoming page**, and embedding `OFL.TXT` verbatim via
  `include_str!` is the concrete action that resolves D66's own flagged judgment call, not a new one.

### `REPORT.md` is stale (found, not this packet's fault, worth fixing here since it's adjacent)
`REPORT.md:108-109` still lists `font8x8_basic`/`font8x8_ext_latin` as bundled fonts and `REPORT.md:172-174`'s
closing italic line still says *"Fonts: font8x8_basic (Public Domain)"* — both **wrong** today; `font8x8` was
deleted and Terminus shipped (D66, confirmed: `src/text/glyphs.rs`/`bitmap.rs` no longer exist, `ls
src/text/` shows only `terminus.rs`/`terminus_glyphs.rs`/`translit.rs`/`mod.rs`). This is a pre-existing doc
drift this packet did not cause, but since the attestation page and `REPORT.md`'s license table describe
overlapping ground truth, the plan folds a `REPORT.md` refresh into its docs task (§Plan) rather than leaving
two now-easily-cross-checked sources disagreeing.

### `accept.sh`'s golden letter-suffix scheme is exhausted for tty goldens
Every single-letter suffix `A3`–`A3z` is **already in use** (grep-confirmed: `A3`, `A3b`...`A3z`, all 26
present). The PNG series (`A5`–`A5w`) still has `x`/`y`/`z` free. A new **tty** golden for
`about:attestations` cannot take a free single-letter `A3<x>` slot — none exists. Flagged as a concrete
naming decision the plan must make explicitly (§Design/§Risks), not discovered mid-implementation.

## Design

### 1. Scheme + handler
**`about:` is a new arm in `fetch::fetch`'s match (`src/fetch/mod.rs:108-114`), implemented in a new sibling
module `src/fetch/about.rs`** (same shape as `data.rs`: a free `pub fn fetch(request: &Request) -> Result<Response,
FetchError>`, no `Fetch` trait, no I/O):
```rust
"about" => about::fetch(request),
```
- **`about:attestations`** (matched on `url.path() == "attestations"`, confirmed correct parsing above) →
  `Ok(Response { status: 200, final_url: request.url.clone(), headers: vec![("content-type".into(),
  "text/html".into())], body: <the assembled page, §Design 2/4> })`.
- **Any other `about:<x>`, including bare `about:` (empty path)** → also `Ok(Response { status: 200, ... })`,
  body = a tiny (~200-400 byte) static HTML fragment: a one-line "unknown `about:` page" message plus a link
  to `about:attestations` (the one page that DOES exist). **`about::fetch` is total and infallible over every
  possible `about:` URL** — it never returns `Err`, mirroring the "never vanish" contract `glyphs::lookup`/
  `translit::resolve` already guarantee elsewhere in the codebase, and giving a strictly better degradation
  than propagating a `FetchError` (which would make `dump_text` return an empty string / `dump_png` return a
  blank 1×1 PNG — see `main.rs`'s own `blank_png` doc comment for that existing, worse, fallback shape).
- **`resolve_url` (`main.rs:380-392`) gains `|| scheme == "about"`** in its passthrough condition — without
  this one-line fix, no CLI entry point (`--dump-text`, `--dump-png`, `--x11`) can ever reach this handler
  (§Current state).

### 2. Content + size
**Recommendation: embed real per-dependency license text (not a single generic "MIT" summary), because the
size cost of doing so honestly is small relative to the ~95 KiB headroom** — this is not a case where
summarizing buys meaningful floppy budget back, so there is no good reason to under-attribute.

| Piece | Source | Size |
|---|---|---|
| Stele's own license notice | Short, hand-authored GPL-3.0-or-later notice (copyright line + "this program is free software... no warranty... see LICENSE / gnu.org/licenses" — the standard short-form notice GPL projects use in-program, NOT the full 35 KB text) | ~0.6–0.8 KB |
| Terminus OFL-1.1, full text | `include_str!("../../third_party/terminus-font/OFL.TXT")` (already committed, verbatim, reused as-is) | 4,480 B (exact, measured) |
| Cargo dependency roster (table: name, version, SPDX) | Generated (§Design 3) | ~20 rows × ~90 B ≈ 1.8 KB |
| Deduped MIT license texts (one block per DISTINCT text, content-hash deduped) | Generated, sourced from each crate's own vendored `LICENSE-MIT` (or the pinned upstream repo for `taffy`, which ships none) | ~13–16 distinct texts × ~1.05 KB ≈ **14–17 KB** |
| `slotmap`'s Zlib text | Vendored `LICENSE` (884 B, exact) | 0.9 KB |
| HTML structure (headings, intro/etymology paragraph, section wrappers) | Hand-authored static shell, `assets/attestations.html` | ~1.5–2.5 KB |
| **Total, estimated** | | **≈24–28 KB** |

Against **97,124 B (≈94.85 KiB)** headroom (D66's own latest number, reconfirm at implementation time): this
consumes roughly **25–30%** of current headroom, leaving **≈68–73 KB** for whatever lands after — a real but
modest price, explicitly reported (not guessed) once Task 2's generator runs for real in CI. **This estimate
is deliberately not fabricated to more precision than the inputs support** — the exact MIT-text dedup count
depends on which crates happen to share byte-identical files (three confirmed already: `rustix`/
`linux-raw-sys`/`serde`), which the plan's generator computes exactly, not this document.

**What's deliberately NOT embedded:** Stele's own full GPL-3.0-or-later text (35 KB — the repo-level `LICENSE`
file already satisfies GPL distribution requirements for anyone who clones/downloads the source; the compiled
binary shipping a short notice + a pointer is the same posture already established for Terminus's OFL
condition 2 at the repo level, D66) and Apache-2.0 full text for any dual-licensed dep (all of them also
offer MIT, which this page picks uniformly — Apache's own text is ~11 KB per instance and would roughly
double the roster's cost for zero additional attribution completeness, since choosing MIT is a fully valid,
complete discharge of a `MIT OR Apache-2.0` license grant).

### 3. How the dependency list is produced
**Recommendation: a checked-in generator, `tools/gen-attestations.py` (mirrors `tools/gen-terminus-glyphs.py`'s
precedent exactly), emitting a committed, GENERATED Rust module** — not a hand-maintained `const` table.
Rationale: the Terminus precedent already established this pattern for exactly this reason (reproducibility,
no staleness when `Cargo.lock` changes, CI-driven regeneration rather than a human remembering to update a
table by hand) — a hand-maintained dependency roster would silently rot the first time any dep bumps a
version or a new one is added, which is a *worse* failure mode for a page whose entire purpose is accuracy.

- **Inputs:** `cargo metadata --format-version=1 --filter-platform <i486 target triple>` (resolves the REAL,
  target-filtered dependency graph — closing the Windows-only-crate trap found in §Current state) run once by
  whoever regenerates (a human or CI job with network access to crates.io — this is a build-TIME generation
  step, same posture as `gen-terminus-glyphs.py`'s BDF download, not a Stele runtime dependency); each
  package's own `license` field (already present in `cargo metadata`'s JSON, no separate license-detection
  library needed) plus its on-disk `LICENSE*` file(s) under `~/.cargo/registry/src/.../<pkg>-<version>/`
  (present after any ordinary `cargo fetch`).
- **Exclusions, explicit:** proc-macro/build-only crates (`serde_derive`, `syn`, `quote`, `proc-macro2`,
  `unicode-ident`, `version_check` — confirmed build-time-only in §Current state) are excluded from the
  roster — they're tooling, not bundled artifact content. This exclusion is a stated judgment call, flagged
  in the PR for the operator to confirm (mirrors how the Terminus design flagged its own judgment calls).
- **The `taffy` gap:** `taffy` 0.13.0 ships no `LICENSE*` file in its published crate (confirmed above) — the
  generator needs a pinned fallback source (the exact upstream tag/commit of `github.com/DioxusLabs/taffy`
  matching `0.13.0`, downloaded + SHA-256-verified once, same discipline `gen-terminus-glyphs.py` already
  applies to the Terminus BDF tarball). Resolving the exact pin is a Plan Task 1 action item, not fabricated
  here (mirrors the Terminus design's own "risk, not resolved in this document" posture for its BDF source).
- **Output shape:** a generated `src/fetch/attestations_data.rs` (GENERATED-file header, points back at the
  tool, like `terminus_glyphs.rs`'s own header) exposing a `const` roster (`name`, `version`, `spdx`,
  `license_block_id`) plus content-hash-deduped `const` license-text blocks, e.g.:
  ```rust
  pub struct DepEntry { pub name: &'static str, pub version: &'static str, pub spdx: &'static str, pub license_block: usize }
  pub const DEPS: &[DepEntry] = &[ /* ... */ ];
  pub const LICENSE_BLOCKS: &[&str] = &[ /* deduped MIT/Zlib/etc. texts */ ];
  ```
  (exact shape is an implementation detail for whoever authors the tool, mirroring the Terminus plan's own
  "constraint stated, shape not over-specified here" posture.)

### 4. Rendering — a plain document, not a literal modal
The page is assembled at `about::fetch` call time (or as a `once`-computed `static` — cheap either way; this
page is fetched rarely, not perf-sensitive) by concatenating:
1. A hand-authored **static shell**, `assets/attestations.html` — `<h1>Attestations</h1>`, a short paragraph
   on the "Stele = inscribed stone monument" naming rationale, an `<h2>` for Stele's own license, an `<h2>`
   for the dependency roster, an `<h2>` for Terminus.
2. Stele's own short GPL notice — **as one or more `<p>` elements, not `<pre>`** (§Current state's
   `white-space: pre` finding — the collapsing-whitespace default is fine for ordinary prose paragraphs, which
   is exactly what a short notice already reads as).
3. The dependency roster as a `<ul>`, one `<li>` per dep: `"<name> <version> — <SPDX>"` (real, working
   markup — `fixtures/kitchen-sink.html`'s own `<ul><li>` already goldens this exact combination, §Current
   state).
4. The OFL-1.1 full text (`include_str!`'d verbatim) and any embedded MIT/Zlib texts, **pre-split into `<p>`
   elements at each source blank line** by the generator (§Design 3) — NOT wrapped in a single `<pre>` block.
   This is the direct, load-bearing consequence of the `white-space: pre` gap: splitting on blank lines ahead
   of time (a trivial string operation the generator already performs once, at build time) preserves the
   license texts' real paragraph/condition structure through the engine's real (collapsing) whitespace model,
   without depending on a layout feature that doesn't exist yet. Each condition/paragraph of OFL/MIT/Zilch
   text becomes its own `<p>`, which the block layout already renders as a visually separated block — legible
   structure, zero new engine capability required.

No `<pre>`, no table (a `<ul>` is simpler and already fully supported; `<table>` also works per other
fixtures but buys nothing extra here), no CSS this page needs beyond what `ua.rs` already provides for plain
`h1`/`h2`/`p`/`ul`/`li` — the page deliberately uses **zero author-supplied CSS**, keeping its embedded-byte
cost to markup + text only (§Design 2's table has no CSS line item because there is none).

### 5. Chrome affordance
**A new small rect in `ChromeLayout`** (`src/backend/chrome.rs:33-46`), sized/positioned similarly to
`throbber` (a small square, inset from an edge) — e.g. an "ⓘ" or "©" glyph button in the top bar, to the left
of the throbber or in the status bar's right edge (exact placement is an implementation-time layout call, not
fixed here; the constraint is: doesn't overlap `back`/`address`/`throbber`, degrades to zero-size under the
same `saturating_sub` clamping discipline every other `ChromeLayout` field already uses for tiny windows).
`chrome::draw` paints it (same pattern as `draw_back_button`/`draw_throbber`) — no new drawing primitive
needed, `draw_centered_glyph`/`fill_rect` already exist and are reused. **Wiring in `run_x11`:** the
`ButtonPress` handler (`main.rs:1948-1993`) gains a third hit-test alongside `lay.back`'s, in the same `if`
chain, calling `history.navigate(Url::new("about:attestations"))` directly (`chrome::ChromeState`/
`x11_chrome_state` need no new fields — `history.current()` already reflects the new URL on the next redraw,
exactly like any other `history.navigate` call site, e.g. `main.rs:1993`, `main.rs:2560`).

**Manual-verify, explicitly:** per AGENTS.md's own established split (this repo's `browser-chrome` design
already draws this line, `docs/superpowers/specs/2026-08-20-browser-chrome-design.md` — pure `layout`/`draw`
are unit- and golden-tested; `run_x11`'s event loop is not, and never has been, in this codebase), the new
rect's geometry (`ChromeLayout`) and paint (`chrome::draw`) ARE unit- and `--dump-png --chrome`-golden-testable
today; the actual click → navigate wiring inside `run_x11` is manual-verify only, same as the back button's
own click handling already is. **What CI actually gates:** (a) the `about:` fetch handler returns the right
body for `about:attestations` and degrades correctly for unknown `about:` URLs (pure unit tests, `fetch::about`);
(b) a `--dump-text`/`--dump-png` golden of `about:attestations` itself, proving the embedded HTML renders
legibly through the real pipeline; (c) the NEW chrome rect's geometry/paint, via the existing
`chrome::layout`/`chrome::draw` unit tests and (optionally) a `--dump-png --chrome` golden showing the new
button rendered in an otherwise-ordinary chrome frame. The click itself is not automatable without a real X11
harness, same as every other `run_x11` interaction in this codebase.

## Testing / fixtures (see plan for full task-by-task breakdown)
- **`fetch::about` (Rust, TDD):** `about:attestations` returns `status: 200`, `content-type: text/html`, and a
  body containing specific, grep-able substrings (the GPL notice's key phrase, at least one dependency name +
  version, the OFL license's own "Reserved Font Name" phrase) — asserting real content landed, not just "some
  bytes came back." Any other `about:<x>` (including `about:blank`, `about:`, `about:xyz`) also returns
  `status: 200` with the small unknown-page body — proving totality, never an `Err`/panic over arbitrary
  `about:` input (a fuzz-style test over a handful of hostile strings: empty, very long, non-ASCII).
- **`resolve_url` (Rust, TDD):** `resolve_url("about:attestations").as_str() == "about:attestations"` (passes
  through, doesn't get mangled into a bogus `file://` path) — a new case alongside the existing
  `resolve_url_passes_through_http_and_file_schemes` test (`main.rs:3075`).
- **Golden (`--dump-text about:attestations`, new `goldens/attestations.txt`):** confirms the tty render shows
  the roster + Terminus attribution as legible text, no missing-glyph tofu, no crash.
- **Golden (`--dump-png about:attestations out.png`, new `goldens/attestations.png`):** confirms the page
  lays out and paints without overflow/garbage at the default 800px viewport width; pixel-spot-check (not
  eyeball) that text fills a reasonable content height and doesn't blank-render.
- **`chrome::layout`/`chrome::draw` (Rust, TDD):** the new rect's geometry doesn't overlap existing rects at a
  normal window size; degenerate/tiny windows still don't panic (mirrors every existing `ChromeLayout` field's
  own totality tests, `chrome.rs`'s test module).
- **Size budget:** the actual `stele-i486` size delta, measured from the CI artifact once Tasks land, reported
  against this document's ≈24–28 KB estimate and the 97,124 B headroom.

## Risks / unknowns not fully grounded here
- **The exact MIT-text dedup count** (§Design 2's "13–16 distinct texts") is a projection from partial
  content-hashing (one confirmed 3-way dedup group) — the plan's generator computes the real number; this
  document's byte estimate has a stated ±3 KB-ish range rather than false precision.
- **`taffy`'s missing `LICENSE*` file** needs a pinned upstream source (repo tag/commit + SHA-256) resolved at
  implementation time, not fabricated here — same posture as the Terminus design's own unresolved BDF source
  citation.
- **`accept.sh`'s exhausted `A3` letter suffix** (§Current state) needs an explicit naming decision before the
  golden task lands — e.g. a two-character extension (`A3aa`) or folding this into the still-open `A5x`/`A5y`
  slot for the PNG golden while inventing a fresh single-letter (or numbered) block for the tty golden. Not
  resolved here; the plan's golden task must pick one and say why.
- **Exact chrome-button placement** (§Design 5) is left as an implementation-time call — this document
  states the constraint (no overlap, same clamping discipline) but not exact pixel geometry, deliberately
  mirroring how the browser-chrome design itself left some layout constants to the implementing task.
- **Whether Stele's own short GPL notice text is legally sufficient** (vs. requiring more of the full text)
  is a judgment call, not a legal ruling — flagged for the operator to confirm in the PR, exactly like the
  Terminus design flagged its own OFL condition 2 reach as a judgment call (D66) that THIS packet is meant to
  resolve for Terminus specifically, while opening the analogous question for Stele's own GPL notice.

## Charter / decisions note
This is a **new scheme handler** (`about:`) — the same category of change `data:` (Acid2 Packet 4) and
`https` were: a new way to REACH content, not a new thing the DOM/CSS dialect can express. Per the pattern
those two packets set (`stele-charter.md`'s "What Stele Speaks" governs document *vocabulary*, not transport/
scheme surface), **no charter amendment is expected** for the scheme itself — flag this judgment in the PR
per AGENTS.md rule 6, mirroring how `data:`/`https` were characterized. The embedded page's HTML itself uses
zero new elements/properties (headings, `<p>`, `<ul>`/`<li>` — all already in the adopted dialect). Record a
new `DECISIONS.md` entry (next free letter after D66, **D67** as of this writing — reconfirm at implementation
time) covering: (1) `about:` as a new, fetch-dispatched, purely-in-process scheme, infallible over all inputs;
(2) the embedded-vs-summarized license-text call (embed real per-dep texts, MIT preferred over Apache-2.0
where dual-licensed, full OFL-1.1, short GPL notice not full text) and its measured byte cost; (3) the
generated-roster-over-hand-maintained-table choice, mirroring the Terminus precedent; (4) the chrome
affordance's manual-verify scope, mirroring the browser-chrome design's own layout/draw-vs-interaction split;
(5) the `resolve_url` fix as a small, necessary, in-scope bugfix this packet owns.
