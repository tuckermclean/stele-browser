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
## 2026-08-12 — Wave 1 · P2 (CSS parser + cascade), subagent-driven

- P2 `style::{tokenizer,selector,value,parser,cascade,ua}`: bespoke CSS tokenizer +
  parser (full syntax, error recovery, ignore-unknown counting per C2), selectors
  (element/.class/#id/descendant/grouping/a:link/:visited + specificity), and the
  cascade (built-in UA sheet + author sheets → per-node `ComputedStyle`, inheritance,
  em/% font-size resolution). std-only, total (200k-iter CSS fuzz, no panics).
- `@media` DEFERRED to M5: parsed but not evaluated (viewport isn't threaded through
  the frozen `parse`/`cascade` signatures). Future orchestrator freeze amendment.
- Loop: implementer (92 green) → reviewer (Spec ❌: 2 Critical + 1 Important, each
  empirically confirmed in a throwaway worktree) → fix round 1 (same agent) → scoped
  re-review (all addressed, no new breakage) → **100 tests green**. Bugs caught
  before main: (a) cross-sheet specificity ignored (later sheet always won);
  (b) `line-height: %` collapsed to 0px; (c) shorthand silently absorbed garbage
  tokens → wrong per-edge values + uncounted.
- Frozen files untouched (`computed.rs`/`ast.rs`/`surface`); covenant clean.

## 2026-08-13 — Wave 1 · P5 (text metrics) + image-dep verification

- Image-decoder crates verified for the i486 floor (PR #9): `gif`/`jpeg-decoder`
  (no rayon)/`png` + `crc32fast`/`miniz_oxide`/`fdeflate`/`weezl` all compile for
  `i486-monolith-linux-musl` under `-Zbuild-std` — the brief's §6 L4 risk retired
  before P4 depends on them. Full `cargo vendor` (C8) deferred to release hardening.
- P5 `text::bitmap`: monospace `BitmapFont` implementing the frozen `Metrics`
  trait (advance/ascent/descent/line-height, all total for any char + positive
  size). std-only. Glyph rasterization / atlas + fontdue-vs-atlas comparison
  DEFERRED to P9 (fb backend) where pixels are consumed; v0 is monospace.
- Loop: implementer (134 green) → reviewer (Spec ✅, Approved; 1 Important) →
  fix round 1 (floor degenerate cell geometry at 1.0 so metrics stay finite) →
  orchestrator-verified → **135 tests green**. Stale `text/mod.rs` module doc
  reconciled by the orchestrator.
- Process note: implementers no longer edit JOURNAL/DECISIONS (orchestrator owns
  them) to avoid cross-packet append conflicts.

## 2026-08-13 — Wave 2 · P6 (layout: block flow + inline engine v1)

- The long pole. `layout::{block,inline}` implements the frozen
  `layout(root, viewport) -> Vec<Fragment>` per charter §158 "solvers over a
  flex substrate": **taffy 0.13** as the box-math primitive (block flow =
  degenerate column flex), with a **bespoke, Metrics-generic inline
  line-breaker** hanging off taffy measure-function leaves — the soul of the
  program, no crate provides it. M2 scope: text runs + line wrapping, **no
  floats** (M4), no tables (P8/M3), no frames (P10).
- **Taffy cleared the i486 floor** (the packet's headline risk): CI `build`
  cross-compiled taffy + the engine for `i486-monolith-linux-musl` under
  `-Zbuild-std`, and `accept` ran it under `qemu-i386 -cpu 486` — green. The
  charter's flex-substrate bet holds on real 486-legal code; no L3 hand-roll
  needed. Brief §6 L3 (hand-roll row/column flex) retired for now.
- Inline engine: whitespace collapsing (CSS normal), soft-wrap at break
  opportunities, half-leading baseline math, cross-run glued-word atomicity
  (`<b>x</b>y` = one unbreakable unit, still two styled runs). Unit-tested
  against a synthetic fixed-metrics impl so every wrap decision / x-position /
  baseline is exactly assertable.
- Loop: implementer (147 lib + 12 layout tests, test-first `68cb9e1` red →
  `4e05b4e` green) → reviewer (Spec ✅; **1 Critical + 2 Important**, the
  Critical empirically reproduced) → fix round 1 (`08bf67f`) → scoped
  re-review (orchestrator: depth cap + feature trim correct, frozen
  `layout/mod.rs` byte-identical) → green.
    - **Critical (caught before main):** the recursive tree walk
      (`translate`/`flatten_inline`/`emit` + taffy's own `compute_layout`)
      had no depth limit — a chain of ~200 nested `Container`s overflowed the
      stack (`SIGABRT`, a guard-page fault `panic=abort` can't catch). ~200
      nested tags is a few hundred bytes of hostile/generated HTML into our
      tag-soup parser. Fix: `DEPTH_CAP = 100`, over-deep subtrees degrade to
      empty boxes; regression test at depth 2000/5000 now returns instead of
      aborting. See DECISIONS D14.
    - **Important:** trimmed taffy to `default-features = false` +
      `["std","taffy_tree","flexbox","block_layout","content_size"]` —
      dropped unused `grid`/`float_layout`/`detailed_layout_info`/`calc` (and
      transitive `smallvec`) for the A2 size budget + vendoring surface.
    - **Important (documented, deferred to M4):** a non-floated inline `<img>`
      between text breaks flow instead of sitting in the line, and a
      `Replaced` grandchild inside a nested inline container is dropped — both
      flagged in code, both land with M4's image + float work (no M2 fixture
      has inline images). See D14.
  Reviewer empirically **disproved** the two subtle bugs most feared:
  border-box-vs-content-box text measurement is correct, and the wrap-width
  comparison is correct — the engine measures inline text against the content
  width, not the border-box.
- NEXT: P7 (tty backend) now unblocks against P6's real fragments — the pair
  that lights up M2's first fixture browsing.

## 2026-08-13 — Wave 2 · P7 (tty render pipeline) — M2 backbone

- The first END-TO-END render: `fetch → parse → cascade → box_tree → layout →
  tty text grid`, exposed as `stele --headless --dump-text <path|url>`. You can
  now browse a fixture as a deterministic text dump. `basic.html` renders to a
  clean heading/paragraph/link layout.
- Closed a real integration gap: nothing in `src/` turned the styled DOM into
  layout's `LayoutNode` tree (it lived only in a test helper). P7 adds
  `layout::box_tree::build_box_tree(dom, &styles) -> Option<LayoutNode>`
  (display:none dropped, text→Text, img→Replaced-with-attr-intrinsic, else
  Container) — depth-capped at 100 like P6, since it's another recursive DOM
  walk that would otherwise re-open the stack-overflow class.
- tty backend (`backend::tty`): maps fragments to an 8×16 char grid
  (col=round(x/8), row=round(y/16) at line-box top), later-paints-over-earlier,
  clips at edges, places by `char` (UTF-8-safe). `to_text` trims trailing
  whitespace. std-only (no dep).
- Loop: implementer (test-first, red/green pairs `689ce68`→`60a4e75`,
  `2bddfb5`→`2fa0471`; +`213add4` accept.sh) → reviewer (Spec ✅; **golden
  COUNTERSIGNED** — every row re-derived from UA sheet + cascade em-resolution
  + font metrics; **1 Critical + 2 Important**) → fix round 1
  (`7b717e2`/`bae6d41`/`02fbb66`/`a661f1e`) → scoped re-review (orchestrator:
  clamp + CI wiring correct, golden byte-identical, frozen intact).
    - **Critical (caught before main):** unbounded `--cols` drove an
      allocation abort via a single CLI flag on ANY document (`vec![vec![' ';
      cols]; rows_needed]` allocates the inner row even when `rows_needed==0`).
      Fix: `MAX_GRID_COLS=2_000` clamp as the first thing in `render`, plus a
      `rows_needed==0` early return — worst-case grid now bounded to 80MB.
      See DECISIONS D17.
    - **Important:** A3 (tty-golden acceptance) called `cargo +nightly build`
      but was wired into the cargo-less `accept` CI job — it would fail or use
      a floating nightly (breaking C9). Fix: A3 runs in the `build` job under
      the pinned toolchain via `accept.sh --tty-only` (no `+nightly`), and
      degrades to PENDING where cargo is absent. A1/A4 stay in `accept`.
    - **Important (documented, deferred):** the tty grid advances one column
      per char regardless of font-size (advance scales with size), so
      mixed-font-size inline runs on one line would misalign — invisible for
      basic.html; flagged in code + D17 for a later fixture.
- The golden-blessing discipline ran as intended: the implementer filed
  `goldens/basic.tty.txt` PROPOSED; the reviewer independently countersigned it
  against the fixture; the orchestrator blesses it here (`accept.sh --bless`
  reproduces it byte-identically).
- Also this session: a **recursion-hardening** packet fixed the twin
  stack-overflow in `style::cascade` (explicit-stack rewrite) that P7's totality
  work surfaced — see the hardening entry above (D15/D16).
- **M2 reached** with P6+P7: real fixtures browse as text, total on hostile
  input end to end. NEXT: P7b interactive tty shell (raw-mode scroll/back; link
  nav pending a small Fragment freeze amendment for href provenance), then
  Wave 2's P8 (table column solver) toward M3.

## 2026-08-13 — Wave 2 · P8 (table column solver) — standalone pure function

- The charter's "genuinely hard part, written in-house": `layout::table`'s
  `solve_table(&TableSpec) -> TableLayout` — the two-pass min/max-content
  automatic table-layout algorithm (CSS 2.1 §17.5.2.2): per-column min/max
  from cells, colspan/rowspan spanning-excess distribution, width resolution
  across the three regimes (≤min / ≥max / proportional slack between), row
  heights, and cell-rect geometry with border-spacing. STANDALONE — cells
  arrive pre-measured and pre-placed (`col`/`row`/`colspan`/`rowspan`);
  integration into `layout()` is deferred to M3 + a freeze amendment (the
  frozen `Display` enum has no Table/Row/Cell variants yet).
- Loop: implementer (test-first `742eb84` red → `61da7d0` green, 15 tests) →
  reviewer (Spec ✅ vs CSS 2.1; **no correctness bugs, no reachable panic** —
  hand-traced the colspan-excess + proportional cases, proved widths sum back
  to `available_width`; found hardening + coverage gaps) → fix round 1
  (`09fce6c`, 20 tests) → scoped re-review (orchestrator: bounds correct,
  frozen `mod.rs` only `+pub mod table;`).
    - **Important (fixed):** the overlap check was O(n²) over an uncapped
      `cells.len()` — a huge hostile `<table>` couldn't panic but could hang
      (DoS). Replaced with an occupied-slot bitset + `MAX_GRID_CELLS=262_144`
      area cap + a shared `placement_budget` capping total slot-reads, so
      placement is O(grid area) regardless of cell count or adversarial spans.
    - **Important (fixed):** the `max_i >= min_i` clamp (the hardest path) had
      zero test coverage — added a dedicated case (the reviewer's numbers).
    - **Minor (fixed):** finite-output scrub (extreme `f32` magnitudes could
      overflow to inf/NaN in output despite input sanitization) + rowspan-
      degenerate and combined colspan+rowspan coverage tests.
  See DECISIONS D18.
- **Wave 2 COMPLETE** (P6 layout · P7 tty pipeline · P8 tables). Toward M3:
  table INTEGRATION (freeze amendment adding `Display::Table/Row/Cell`, box-tree
  wiring feeding `solve_table` output to taffy as fixed bases), frames, forms;
  plus the P7b interactive shell to close M2's "follow links" clause.

## 2026-08-14 — M3 · freeze amendment: CSS table Display values

- The gateway to real tables. Added `Display::{Table, TableRow, TableCell,
  TableRowGroup}` to the frozen enum — the marker the layout engine will key
  off to run `layout::table::solve_table`. This packet ONLY lands the marker +
  wiring and keeps everything green; real table box-tree/taffy integration is
  the next packet.
- Wiring: the CSS value parser maps `table`/`table-row`/`table-cell`/
  `table-row-group` (unknown values still ignored, C2); the UA sheet assigns
  them (`table{display:table}`, `tr`, `td/th`, `thead/tbody/tfoot`). Blast
  radius was tiny — the ONLY exhaustive `match` on `Display` in the codebase is
  `block::map_display`; box_tree/table use `==` only. `map_display` maps all
  four new variants to taffy `Block` for now (tables render as stacked block
  boxes — visually wrong but total + green), explicit arms (no catch-all) so
  the next packet's compiler errors guide the real implementation. See D19.
- Orchestrator reviewed the frozen-type change directly (small, additive,
  cascade test confirms `<table>`→`Display::Table`); 199 lib + 13 bin + 88
  integration tests green, no regressions, no deps, no unsafe.

## 2026-08-14 — M3 · freeze amendment: table cell spans

- Second (and last) table gateway: `box_tree` drops HTML attributes, but the
  column solver needs `<td>`/`<th>` `colspan`/`rowspan`. Added
  `BoxContent::TableCell { colspan: u16, rowspan: u16 }` to the frozen enum —
  a cell is otherwise exactly a `Container` (children in `LayoutNode.children`)
  but carries the two spans. `box_tree` populates it when
  `display == Display::TableCell`, reading + defaulting (missing/unparseable/
  zero → 1) + clamping (colspan ≤ 1000, rowspan ≤ 65534, HTML's own limits),
  mirroring the existing `<img>` intrinsic-attr path.
- Green-keeping: the three exhaustive `BoxContent` matches in `layout::block`
  (`translate_any`/`is_inline_ish`/`flatten_inline`) share the `Container` arm
  via `Container | TableCell { .. }`, each tagged
  `TODO(table-layout packet)` — so a cell still renders as a stacked block
  until the real layout lands, and the next packet gets compiler-guided sites.
- Orchestrator reviewed directly (only `BoxContent` changed; box_tree/block
  only). 201 lib tests + all integration suites green, no deps, no unsafe.
  See D20. NEXT: the table-layout feature — a `Display::Table` measure-leaf in
  the engine that grids the cells (via the spans), measures min/max content,
  runs `solve_table`, and paints the solved rects; + `tables.html` + golden.

## 2026-08-14 — M3 · table layout — 1996 tables render

- `layout::table_layout` + `layout::block` wiring: a `Display::Table` container
  becomes a bespoke measure-leaf (exactly parallel to the inline engine). Grid
  auto-placement (advance past prior colspans, reserve rowspan slots) →
  per-cell min/max-content + height measurement via taffy intrinsic sizing →
  `solve_table` (P8) → cells painted at solved rects, cell content laid out
  within each cell's box so columns align. `<table>` now renders as a real
  grid in the tty dump — rowspan/colspan and column alignment all correct.
- Loop: implementer (test-first `c7ca987` red → `c910476` green) → reviewer
  (Spec ✅; **golden COUNTERSIGNED** — every column re-derived char-for-char;
  **regression audit of block.rs +475/−60 came back clean** — no non-table
  path changed; **1 Critical**) → fix round 1 (`af95a6b`) → scoped re-review
  (orchestrator: cap + cache correct, golden byte-identical, frozen clean).
    - **Critical (caught before main):** per-cell measurement was unbounded —
      ~7 taffy sub-layouts per cell (measure re-run in emit), capped only by
      the 262K *placement* budget, so a wide hostile `<table>` (tens of
      thousands of `<td>`s, <1MB HTML) hung for minutes — the same DoS class
      P8 fixed for placement, resurfacing on the expensive taffy axis. Fix:
      `MAX_TABLE_MEASURED_CELLS=2_000` (over-cap tables degrade to block, cheap
      + total) + a `RefCell` cache keyed on `avail_w` so emit reuses the solve
      (~7×→~3×) + wide-table stress tests. See D21.
    - Two real bugs fixed during green: tables need taffy `item_is_table` to
      shrink-wrap (else they stretch full width); the table's measured size
      must include border-spacing gaps.
- Two-pass phasing: measure widths → solve columns → lay out each cell at its
  solved width → solve again for row heights + rects. Border-spacing hardcoded
  (8px h / 0px v — no `ComputedStyle` field). M3 simplifications (deferred):
  `table-layout:fixed`, `<caption>/<colgroup>/<col>`, `vertical-align`
  (cells top-align), author `<style>` sheets (not yet wired into cascade).
  See D21. **M3 tables DONE.** NEXT: frames, forms.

## 2026-08-14 — M3 · forms (submit serializer + control rendering)

- Two deliverables. (1) `form::serialize_submit(dom, form_id, base, activator)
  -> fetch::Request` — HTML 4.01 §17.13.2 form submission as a PURE function:
  successful-controls gating (named + enabled; checkbox/radio only if checked;
  activating submit only), `application/x-www-form-urlencoded` with self-rolled
  percent-encoding (space→`+`, unreserved kept, else `%XX` over UTF-8 bytes),
  GET → replace the action's query, POST → urlencoded body + Content-Type. (2)
  Form controls rendered as synthesized TEXT placeholders in `box_tree` (empty
  `Box` fragments are invisible in tty, D17): `[Ada]`/`[______]` inputs,
  `[x]`/`[ ]` checkbox, `(*)`/`( )` radio, `[ Send ]` buttons, `[ Green v]`
  select — so `<form>` renders legibly. Real widgets are the fb backend (M4).
- Loop: implementer (test-first, 2 red→green pairs + fixture/golden) →
  reviewer (Spec ✅; **golden COUNTERSIGNED** control-by-control; **no-JS
  covenant confirmed** — serializer is a pure fn, wired to no event path; no
  reachable panic; **2 Important + minors**) → fix round 1 (`7eb622c`) →
  scoped re-review (orchestrator: fixes correct, frozen empty-diff, golden
  byte-identical).
    - **Important (fixed):** `<select multiple>` dropped all but one selected
      option (data loss) — now emits one pair per selected option.
    - **Important (fixed):** `type=image` treated as plain submit — documented
      as a v0 simplification (no click coordinates in a no-mouse/no-JS browser)
      + test pinning it.
    - **Minors (fixed):** depth-cap drift + duplication resolved by extracting
      shared DOM-walk helpers into a crate-private `dom_util` (one `DEPTH_CAP`,
      used by both `form` and `box_tree`); added GET-existing-query test.
- Totality: `dom_util::DEPTH_CAP` bounds the control walk; all DOM access
  OOB-guarded. Frozen surface untouched (new `form`/`dom_util` modules only);
  no deps; no `unsafe`. v0 simplifications (D22): `type=file` → filename only
  (no multipart), checkbox/radio absent value → `on`, placeholder glyph
  conventions. See D22. **M3 forms DONE.** NEXT: frames (last M3 feature).

## 2026-08-14 — M4 · fbdev backend — M4 COMPLETE

- The real Linux framebuffer output path (`backend::fb`), safe + std-only —
  NO `rustix`/ioctl/mmap (those need `unsafe`), NO new deps, NO `unsafe`.
  Geometry from **sysfs** (`/sys/class/graphics/fb0/{virtual_size,bits_per_pixel,
  stride}`, plain text reads); a pure, unit-tested **pixel converter**
  (mem-Surface RGBA8 → 32bpp BGRX8888 / 16bpp RGB565, other bpp → error); and a
  total device-write path (open `/dev/fb0`, write `height*stride` bytes, clip a
  larger surface, `u64` overflow guard on the geometry). `--render-fb <src>`
  CLI mode renders a page straight to the console framebuffer.
- CI can't test the device (no `/dev/fb0` on the runner) — by design the
  device path is TOTAL (absent/garbage device/sysfs → clean `Err`, never a
  panic), and that error path genuinely runs green in CI (the device really is
  absent). The pixel converter + geometry parsing (the testable core) have full
  hand-computed RED→GREEN tests; the *render* itself is already validated by the
  mem-Surface PNG goldens — this packet only reformats those bytes for hardware.
- Orchestrator-reviewed directly (contained, no deps/unsafe/frozen change):
  converter correctness (BGRX byte order, RGB565 packing) + provable in-bounds
  writes (columns clipped by `stride/bpp`) + overflow guard verified by reading.
  346 lib tests green. See D27.
- Human test path (for REPORT.md): on a Linux VT with an active fbcon, or under
  `qemu-system` with `-vga std`, `stele --headless --render-fb fixtures/basic.html`
  paints the page to the screen.
- **M4 COMPLETE** (pixel foundation · images/THE SCREENSHOT · floats+inline ·
  fbdev). Milestones M0–M4 done. NEXT: **M5 — dialect completeness** (the CSS
  slice): `flex-polite.html` pixel-green (flexbox already lays out via taffy;
  now verify it to pixels), `@media` evaluation (needs a viewport freeze
  amendment threading the viewport into cascade), `details`/`summary`,
  `noscript`, full entity coverage, cookie jar surfaced, and the `--stats`
  ignored-declaration counter (C2 "count what we refuse", made visible).

## 2026-08-14 — M4 · floats + inline images — closes D14

- The two image-flow features deferred from M2 (D14), bespoke in the inline
  engine (taffy has no floats): (1) a non-floated `<img>` is now an INLINE ATOM
  sitting in the line between text (was block-level; the D14 "grandchild
  Replaced dropped" gap closed too); (2) `img align=left`/`right` FLOATS — the
  image is pulled out of line flow, placed at the block's left/right edge, and
  `inline::layout_runs` shortens/offsets every line that vertically overlaps
  the float so text WRAPS around it and returns to full width below.
- `box_tree` maps `<img align>` → `ComputedStyle.float` (presentational hint,
  author CSS wins). No freeze needed — `float`/`clear` were already in
  `ComputedStyle`.
- **Golden `goldens/images.png` regenerated (800×495), orchestrator-VIEWED and
  blessed:** 4 labeled images now inline with their labels, a left-floated 48px
  square with text wrapping ~3 lines then returning to full width below it, and
  a gif inline between "before"/"after". Real CSS float layout, rendered.
- Loop: implementer (test-first `6ef82cb` red → `dc89310` green) → reviewer
  (Spec ✅ Approved; no Critical, **no reachable panic or infinite loop** — the
  line-breaker is a single bounded `for` pass, no `while`/retry construct; all
  adversarial float shapes tested incl. 600 floated imgs through the full
  pipeline; **1 inert Minor**) → fix round (`52a396b`) → orchestrator re-verify.
    - **Minor (fixed, defense-in-depth):** the non-floated atom's paint height
      escaped the `MAX_DIM` clamp its width got (a hostile `<img height=1e12>`
      → 1e12-tall rect, inert only because frozen `blit` clips) — now clamped
      at the source via `inline::clamp_dim`.
- Totality bounds: `MAX_FLOATS=256`, `MAX_DIM=1e6`, per-line exclusion O(floats);
  a zero-width float can't spin (first item on a fresh line always places →
  guaranteed progress). Frozen paths empty-diff; no deps; no unsafe/JS. 327 lib
  tests green. See D26.
- **M4 nearly done:** foundation ✅ · images ✅ (screenshot) · floats+inline ✅.
  NEXT: the fbdev (rustix) device backend — real Linux framebuffer output (not
  CI-testable; the mem-Surface PNG goldens are the verification) — then M4
  closes and M5 (dialect completeness) begins.

## 2026-08-14 — M4 · images — THE SCREENSHOT

- `<img>` renders real pixels. Pipeline: `images::collect_images` (new
  driver-level pre-pass — walk `<img src>`, resolve against the doc's FINAL
  URL, fetch, decode via P4 `img::decode_bytes`, frame 0) → `HashMap<NodeId,
  Rc<RgbaImage>>` → `box_tree` threads it into `BoxContent::Replaced { intrinsic,
  image }` (freeze amendment, commit 1, `Rc` so `LayoutNode` clones don't copy
  buffers) → `block::emit` emits `FragmentKind::Image` → `raster::paint` →
  `MemSurface::blit` (frozen stub now real: nearest-neighbor scale, alpha
  blend, fully clipped). Decode gated to the `--dump-png` path (tty passes an
  empty map).
- **THE SCREENSHOT** (`goldens/images.png`, 800×347, orchestrator-VIEWED and
  blessed, saved to REPORT.md): `fixtures/images.html` renders an "Images"
  heading + four labeled swatches — red PNG, a photographic JPEG patch, blue
  GIF, and the animated GIF's yellow frame-0 (frame 1 is green — frame-0 rule
  confirmed). All four decoders working end to end.
- Loop: implementer (test-first, 11 commits) → reviewer (Spec ✅; blit + pre-pass
  total per-image; **1 Critical + 1 Important + minors**) → fix round 1
  (`14b5702`) → orchestrator re-verify (frozen clean, both goldens byte-identical).
    - **Critical (fixed):** unbounded aggregate image memory — `MAX_IMAGES=256`
      capped count and `MAX_DECODE_PIXELS` capped each (~244MiB), but no dedup/
      budget → 256×244MiB ≈ 61GiB (or same-src ×256) → OOM abort. Fix: dedup
      decodes by resolved URL (shared `Rc`) + `MAX_TOTAL_IMAGE_BYTES=256MiB`
      aggregate budget (halt decoding past it → placeholders). The 4th axis
      this aggregate-bomb pattern has surfaced on (tables/tty/raster/images).
    - **Important (fixed):** image base was the pre-redirect URL — relative
      `<img>` 404'd after a redirect. Now uses `Response::final_url`.
    - Minors: blit truncated-buffer test; repeated-src dedup test.
- Freeze surface untouched except the one `Replaced.image` field; no deps; no
  unsafe/async/TLS/JS (blocking std fetch only). 305 lib tests green. See D25.
- **M4 progress:** foundation ✅ · images ✅ (THE SCREENSHOT). NEXT: `img
  align=left` floats (inline engine), then the fbdev (rustix) device backend.

## 2026-08-14 — M4 · pixel foundation — STELE DRAWS

- The renderer produces PIXELS. Embedded a public-domain 8×8 bitmap font
  (`font8x8`, `src/text/glyphs.rs`, license in REPORT.md); implemented the
  frozen `MemSurface::draw_text` stub (glyphs baseline-aligned, nearest-neighbor
  scaled per `size_px`, clipped); added a `backend::raster` painter (Fragments →
  Surface: Box bg+borders, Text, Image skipped for the next packet) + PNG
  encode; and a `--dump-png <src> <out>` CLI (800px wide, content-driven height).
- **First pixel golden `goldens/basic.png` (800×247)** — the orchestrator
  VIEWED it and countersigned: `basic.html` renders faithfully (scaled
  "Welcome"/"Section One" headings, blue "link", paragraph flow). Test decodes
  both PNGs and compares RGBA arrays; `accept.sh` A3e added.
- Loop: implementer (test-first, 8 commits) → reviewer (Spec ✅; no reachable
  panic — every glyph/rect/PNG path clamped; **1 Important + minors**) → fix
  round 1 (`6f114c0` red → `3ba1f0e` green) → scoped re-review (frozen clean,
  golden byte-identical).
    - **Important (fixed):** off-screen glyphs paid full `O(w·h)` raster cost
      (fragments exist beyond the 20k-px surface cap), so a long doc at a large
      `font-size` was `O(chars·1024²)` — a bounded-but-minutes CPU hang
      (measured 17.1s → ~0ms after the fix). Fix: O(1) screen-bbox intersection
      early-return in `draw_glyph`. `MAX_GLYPH_PX=1024` bounds one glyph;
      `MAX_PNG_HEIGHT=20_000` caps the surface alloc at 64MB (tty
      `MAX_GRID_ROWS` analog).
    - Minors: transparent-border short-circuit; a `debug_assert` pinning
      `MemSurface`'s `w*h*4` buffer invariant.
- Frozen surface/Metrics/layout types untouched (only `text/mod.rs` adds
  `pub(crate) mod glyphs;`); no new deps (`png` already present); no unsafe.
  283 lib tests green. See D24.
- **M4 in progress.** NEXT: images — `MemSurface::blit` + wire the P4 decoders
  (`<img src>`→fetch→decode→`RgbaImage`→blit) + animated-GIF frame-0 +
  `images.html` → THE SCREENSHOT. Then `img align=left` floats + the fbdev
  (rustix) device backend.

## 2026-08-14 — M3 · frames (frameset viewports) — M3 COMPLETE

- `frames` module: a driver-level feature ABOVE the single-document pipeline
  (no frozen-type change). Detect a `<frameset>` document, partition the
  viewport per `rows`/`cols` (fixed px → percent-of-remainder → star split),
  and recursively render each `<frame src>` as an INDEPENDENT sub-document
  (its own fetch→parse→cascade→layout→`tty::render`), compositing the
  sub-grids into the viewport via new additive `TextGrid::blank`/`blit`. main.rs
  routes frameset docs here; normal docs unchanged. `<noframes>` never rendered
  (we render real frames).
- Loop: implementer (test-first `9174993` red → `107f2d1` green) → reviewer
  (Spec ✅; **golden COUNTERSIGNED** — geometry re-derived programmatically;
  totality bounds all traced & confirmed; **1 blocking + 1 important + minors**)
  → fix round 1 (`68b4f29`) → scoped re-review (orchestrator: void fix + golden
  byte-identical + frozen clean).
    - **Blocking (fixed):** `<frame>` wasn't in the parser's `VOID_ELEMENTS`,
      so real framesets (`<frame src=x>` with NO close tag) collapsed to one
      cell — the feature was hollow on authentic markup despite passing its own
      golden (the fixture had been bent to non-real `</frame>` syntax). Fix:
      added `"frame"` to `VOID_ELEMENTS` (parser impl, not frozen `ast.rs`) +
      test; fixture rewritten to real void syntax; golden regenerated
      byte-identical (parsing changed, geometry didn't).
    - **Important (fixed):** all N frame sub-grids were held before
      compositing (content-driven height → each up to `MAX_GRID_ROWS` tall) →
      wide-bomb memory blowup. Fix: incremental compositing (render→blit→drop
      per cell); canvas sized from track math; peak memory = canvas + one
      in-flight child. Content taller than its track now clips at the frame
      boundary (faithful to frame viewport/scroll semantics).
    - **Minor (fixed):** added real cross-document A→B→A cycle fixtures + test
      (proves totality end-to-end via the file:// fetch path).
- Frame-bomb totality (all verified): `MAX_FRAME_DEPTH=6`, `MAX_TOTAL_FRAMES=128`
  (global budget threaded by &mut, not per-level), `MAX_TRACKS_PER_DIMENSION=32`,
  same-URL-on-path cycle check with `MAX_FRAME_DEPTH` as an unconditional
  backstop. 248 lib tests green; frozen types empty-diff; no deps; no unsafe.
  See D23.
- **M3 COMPLETE** (tables · forms · frames). Milestones M0–M3 done. NEXT: **M4**
  — fb backend (fbdev via rustix) + mem-Surface pixel goldens + wire the image
  decoders + animated GIF + `img align=left` float layout → THE SCREENSHOT.

## 2026-08-13 — Hardening · recursion totality (cascade + parser)

- Surfaced while building P7: the P6 unbounded-recursion crash class had a
  twin in already-merged code. `style::cascade`'s internal `visit` walked the
  DOM with plain Rust-call recursion, no depth bound — a DOM nested ~3000
  deep `SIGABRT`s (guard-page fault, uncatchable under `panic=abort`) during
  **cascade**, i.e. before layout's `DEPTH_CAP` ever runs. Charter's "the rock
  does not unwind" made this a real must-fix, not an M6 nicety: nesting depth
  is entirely page-controlled (quote threads, WYSIWYG exports, nested-table
  markup).
- Fix (test-first, `df04f0b` red → `eedb274` green): rewrote `visit` as an
  **explicit-stack** (`Vec<Frame>` with `Enter`/`Exit` frames) iterative walk
  — no call-stack recursion, so no depth can overflow it. Chosen over a
  depth-cap-and-degrade because it keeps cascade **fully correct at any
  depth**: every node, however deep, still gets its real resolved+inherited
  style. Semantics are byte-identical to the old recursion (same `ancestors`
  chain visible at match time via the `Exit` frame, same parent-style
  propagation, same "text takes parent's style wholesale", same source order
  via reverse-push). Frozen `cascade`/`ComputedStyle` signatures unchanged.
- The `dom::parser` was investigated and found **already total** at depth (it
  drives an explicit `Vec`-backed open-element stack, not per-nesting
  recursion) — left unchanged, with guard tests added (5000 nested `<div>`s
  asserting the tree is that deep, not silently truncated; 5000 unclosed-tag
  soup). Regression tests: cascade total + correct at depth 3000/5000
  (inheritance verified to reach the innermost node, past any naive cap).
  See DECISIONS D15. Orchestrator reviewed the transform directly (mechanical,
  semantics-preserving) before merge; CI gates the i486 build.

## 2026-08-13 — Wave 1 · P4 (image decoders: PNG/JPEG/GIF + dispatcher)

- P4 `img::{png,jpeg,gif}` behind the frozen `Decode` trait, all output
  normalized to straight-alpha RGBA8: PNG (all color types + bit depths →
  RGBA8 via EXPAND|STRIP_16|ALPHA, palette+tRNS), JPEG baseline **and**
  progressive (jpeg-decoder, rayon off), GIF incl. **animated** — full
  logical-screen compositing with disposal methods (Keep/Background→
  transparent/Previous), sub-rect offsets clamped to canvas, per-frame
  delay (centiseconds ×10 → ms). Dispatcher: `sniff_format` (magic bytes) +
  `decode_bytes` (Content-Type hint, falling back to sniff when the hint is
  absent or wrong — servers lie). Deps unchanged (gif/jpeg-decoder/png
  already landed in PR #9); `Cargo.toml`/`Cargo.lock` diff empty.
- Totality on hostile bytes is the whole game (attacker-controlled input,
  `panic=abort` target): every decoder + the dispatcher return
  `DecodeError`, never panic — no `unwrap`/`expect`/OOB in decoder code
  (the one `expect` is on a compile-time constant). A shared `check_pixel_cap`
  (64M px, `MAX_DECODE_PIXELS`) rejects decompression-bomb dimensions before
  any large allocation.
- Loop: implementer (207 green, test-first `0b77705` red → `7ea05ec` green) →
  reviewer (Spec ✅; 1 **Critical** + 2 Important, the Critical empirically
  proven) → fix round 1 (`d6344ea`) → scoped re-review (orchestrator: fixes
  correct, frozen `img/mod.rs` 1–49 byte-identical, no new deps) →
  **212 tests green** (5 new regression tests).
    - **Critical (caught before main):** JPEG enforced the pixel cap *after*
      `decoder.decode()`, so a tiny crafted **progressive** JPEG with a huge
      SOF (e.g. 65535×65535) drove a multi-GB coefficient-buffer allocation —
      an uncatchable OOM abort on the 486 — before the cap ran. Fix: parse
      the frame header only (`read_info` → `info`), `check_pixel_cap` against
      the declared dims, *then* `decode()`. PNG/GIF already ordered correctly.
    - **Important:** the `gif` crate's default 50MB/frame memory limit
      (~12.5M px) is stricter than our advertised 64M-px cap and would
      silently reject legitimate images — now `set_memory_limit` is pinned to
      `MAX_DECODE_PIXELS*4` so our `check_pixel_cap` is the one gate.
    - **Important:** added oversized-dimension `Unsupported` regression tests
      for all three decoders + a real CMYK JPEG fixture exercising the
      `CMYK32 → Unsupported` arm.
- Deferred (ledger): APNG not decoded (only GIF animation is required); JPEG
  16-bit `L16` `Unsupported` arm left untested (no practical encoder for that
  lossless-only depth) — noted, not a flaky fixture. See DECISIONS D13.
- **Wave 1 COMPLETE** with P4: parser (P1) · CSS (P2) · fetch (P3) · images
  (P4) · text metrics (P5) all merged/green. Next: Wave 2 — the layout engine
  (block flow + inline) and the tty backend toward M2.
