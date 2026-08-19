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

## 2026-08-14 — M5 · external `<link>` stylesheets

- External CSS applies: `<link rel="stylesheet" href>` is fetched, parsed, and
  cascaded (previously ignored — only inline `<style>`/`style=` worked). New
  `stylesheets::collect_all_author_sheets(dom, base, viewport)` walks the DOM
  ONCE, treating `<link>` and `<style>` as one document-ordered author-sheet
  sequence (so a later `<style>` correctly overrides an earlier `<link>`).
  Fetches hrefs (file://+http, driver-level like `images::collect_images`)
  against the doc's FINAL url; `rel` matched case-insensitively among
  space-separated tokens; a `<link media>` gates the whole sheet against the
  viewport; `MAX_LINKS=32`; fetch/parse failures skip that sheet (no panic);
  `@import` inside a fetched sheet stays ignored. Wired into
  dump_text/dump_png/render_fb + frames (each frame resolves `<link>` against
  its OWN url — `render_single_document` gained a `base_url`).
- Golden `goldens/link-css.tty.txt` + `fixtures/link-css.{html,css}` (blessed):
  external sheet applies + document-order precedence + a non-stylesheet
  `rel=icon` link causing no failure. All existing goldens byte-identical.
- Frozen sigs (`cascade`/`parse`/`ComputedStyle`/`layout`) untouched; no deps;
  no unsafe. Rebased onto the interactive-provenance amendment (both had
  touched `frames.rs`); 699 tests green. Documented gap: `--stats` still
  counts only `<style>` (undercounts `<link>`-sourced ignored decls). See D35.

## 2026-08-18 — X11 responsiveness + SetInputFocus fix

- **SetInputFocus BadMatch fixed:** the pre-loop `set_input_focus` (right after
  MapWindow, before the window was viewable) drew `BadMatch(8)` on real servers.
  Moved it to the first `Expose` (window guaranteed viewable) — clears the error
  and actually grabs keyboard focus on WM-less servers (Xfbdev/TinyX).
- **CopyArea scrolling:** scroll used to re-crop, re-convert (~786K px RGBA→bpp),
  and re-`PutImage` the whole ~3MB window every notch. Now `scroll_blit` plans a
  server-side `CopyArea` (opcode 62) to shift the retained rows within the
  window + a `PutImage` of ONLY the newly-exposed strip (`put_image_at` with a
  `dst_y_base`). A 60px line-scroll drops from ~3.15MB to a 28-byte CopyArea +
  ~246KB strip (~12-13x less wire + convert work). Full redraw still used for
  Expose/resize/reload/nav. Unit tests: `encode_copy_area`, `put_image_requests`
  dst_y_base, `scroll_blit` (down/up/jump/no-op). See D52.
- RAM (full-page surface) left as-is — shrinking it trades RAM for
  re-render-on-scroll CPU, the wrong trade on a 486; flagged for a design call.

## 2026-08-15 — X11 backend (spike) · stele --x11 <url> — a real window

- Hand-rolled minimal X11 client (`src/backend/x11.rs`, self-contained, pure Rust
  over `std::os::unix::net::UnixStream` — NO libX11, NO x11rb, NO new deps, NO
  unsafe). Implements only the core-protocol slice Stele needs against a
  bitmap-only kdrive/Xfbdev server: `$DISPLAY` parse, `.Xauthority`
  MIT-MAGIC-COOKIE-1 extraction, connection setup encode + reply parse (id
  base/mask, root/visual/depth, pixmap formats, max-request-length, keycodes),
  id allocator, CreateWindow/MapWindow/CreateGC/PutImage/GetKeyboardMapping,
  PutImage request-banding under max-request-length, keysym→Key table, core
  event parse (KeyPress/ButtonPress/Expose/ConfigureNotify), `hit_test_pixel`
  over `layout::Fragment`.
- Reuses the existing render (`raster`→`Surface`) + `fb.rs` RGBA→screen-format
  conversion + the `browser.rs` model. `main.rs::run_x11` is the manual-only
  interactive loop (mirrors `run_browser`'s thin-I/O split): Expose repaint,
  arrows/PageUp/PageDown/wheel scroll (crop + re-PutImage, no re-layout),
  left-click hit-test → navigate, F5 reload, ConfigureNotify reflow, q/Esc quit.
- 45 new unit tests cover every pure encoder/parser/hit-test (synthetic buffers);
  the socket/loop is manual (can't open a window in CI). 718 tests green. See D51.
- **Run:** inside `startx` (Xfbdev), `stele --x11 <url>` → a scrollable, clickable
  window rendering the page. First graphical interactive backend.

## 2026-08-15 — CORRECT-6 · border-collapse: collapse (shared-grid-line geometry)

- **border-collapse (freeze amendment):** `ComputedStyle.border_collapse:
  BorderCollapse{Separate,Collapse}` (default Separate). CSS `border-collapse`
  parses; a bare `<table border>` (no cellspacing) → collapse hint (author
  `separate` wins). `<table border cellspacing=N>` stays separate.
- **Correct collapse geometry (the second, working design — the first was
  wrong):** the initial attempt deduped each cell to top+left borders and leaned
  on the table frame for right/bottom. That was architecturally broken —
  bordered tables got 2px top/left (frame + cell border stacked), and
  CSS-celled tables with NO table frame lost their right/bottom outer edges
  entirely (open grid). It was DELETED. The working design keeps every cell's
  full 4 borders and lays the table out so adjacent cells OVERLAP by exactly one
  border-width, so shared edges land on the same pixels and render as a single
  1px line; the first/last cells' outer borders form a complete frame (when the
  table also has a frame border, the grid bases at the table's border-box origin
  so the frame overlaps the edge cells too). `border-spacing` forced to 0 in
  collapse. Colspan/rowspan cells span the right number of grid lines and
  correctly suppress interior lines within their own span. Implemented in
  `block.rs` (`collapse_grid_lines`/`collapse_cell_extent`/`collapse_adjust_cell_rects`);
  no painter change. Uniform border width only (differing-width CSS conflict
  resolution deferred).
- **Pixel-verified (independently, by the orchestrator):** `table-border.png` —
  lines at x{8,65,98}/y{8,33,58,83}, ALL 1px, no doubling, closed frame.
  `kitchen-sink.png` — x{0,65,106,195}/y{611,636,661,686,711}, ALL 1px, complete
  4-sided frame despite no table-level border, no column gaps, rowspan interior
  line correctly clipped. Separate-mode goldens byte-identical.
- **tty:** box-drawing table grids (`─`/`│`) for `Display::Table`/`TableCell`
  bordered boxes; `<hr>`/non-table unchanged; grid-line rule lands at the far
  grid-line index so collapsed cells' lines coincide. Default 4px cell padding
  for bare `<table border>` keeps the tty separator readable. See D50.

## 2026-08-15 — CORRECT-5 · <table cellpadding/cellspacing> (border-spacing freeze amendment)

- **Gap:** `cellpadding` and `cellspacing` did nothing.
- **cellspacing → border-spacing (freeze amendment):** `ComputedStyle` gains
  `border_spacing_x/y: f32` (defaults 8.0/0.0 — EXACTLY the old
  `block::BORDER_SPACING_X/Y` constants, so no table golden shifts). CSS
  `border-spacing: <len> <len>?` parses; `cellspacing="N"` → both axes. The
  solver already honored `border_spacing_x/y`; `block.rs` now feeds them from
  the table's own style instead of the constants.
- **cellpadding → padding:** `apply_table_cellpadding_attribute`/`stamp_cell_padding`
  in box_tree mirror the `<table border>` stamp (DEPTH_CAP-bounded, stops at
  nested tables, author padding wins). NO measure/emit change was needed — taffy
  border-box sizing + the universal `base_style` padding mapping already thread a
  cell's padding through `cell_min_max_width`/`cell_content_layout`/`emit` (the
  subagent verified this empirically). Non-inherited border-spacing (only a
  table's own value is ever read). No unsafe. 646 tests green; no pre-existing
  golden changed. Golden `table-spacing.png` (orchestrator-viewed): text inset by
  6px padding, 4px gaps between cells. See D49.
- **Note (→ next packet):** the visible cell gaps + doubled borders are the CSS
  `separate` model (correct here — explicit cellspacing=4). `border-collapse:
  collapse` (merged single grid lines) is the next packet; the default 8px
  border-spacing is also oversized (tty constant leaking into pixels) and will be
  addressed there.

## 2026-08-15 — CORRECT-4 · <table border=N> draws ruled tables

- **Gap:** `<table border="1">` (vintage ruled tables) did nothing.
- **Fix (box_tree, post-cascade — `border` isn't inherited, so correct like the
  `float` hint):** `apply_table_border_attribute` on each built `<table>`: parse
  `border` as a non-neg int (absent/`0`/garbage → no-op); stamp the table box
  with an `Npx` solid `#808080` border and every descendant `TableCell` with a
  `1px` solid `#808080` border via a DEPTH_CAP-bounded `stamp_cell_borders` walk
  that STOPS at nested `Display::Table` boxes (an inner `border=0` table governs
  its own cells). Per-box gated on `border_is_cascade_default` so author CSS on
  the table or any cell wins. Renders in the pixel/fb backend (existing border
  painter); tty draws no 4-side borders by design (unchanged). Frozen types
  untouched; no unsafe. 630 lib tests green; no pre-existing golden changed.
  Golden `table-border.png` (orchestrator-viewed) shows a gray ruled table.
  See D48.

## 2026-08-15 — CORRECT-3 · presentational attributes via a cascade tier

- **Gap:** `<font color>`, `<font size>`, `bgcolor`, block-element `align=`, and
  `<body text>` did NOTHING — vintage sites (68k.news' purple `<font>` headline,
  bgcolor tables, `align=center` cells) rendered unstyled.
- **Fix — a real cascade tier, not post-cascade mutation.** `resolve_declarations`'
  sort key widened from `is_author: bool` to `tier: u8` (UA=0, presentational=1,
  author=2; `0<2` preserves the old `false<true` UA-vs-author order bit-for-bit,
  inline `style=""` still overlaid on top). New `value::presentational_hints(tag,
  attrs) -> Declarations` folds in as the tier-1 candidate:
  - `bgcolor` (any element) → background-color (`#rrggbb`/`#rgb`/bare-hex/named).
  - `<font color>` → color; `<font size>` → font-size (HTML4 scale 1→10px…7→48px,
    `+N`/`-N` relative to base 3, clamped, garbage ignored, never panics).
  - `align=left|center|right|justify` on any element EXCEPT `<img>` → text-align
    (`<img align>` stays float via the untouched box_tree hint).
  - `<body text>` → color (inherits document-wide).
  Because hints live IN the cascade, they correctly override an INHERITED
  ancestor color (the case post-cascade mutation got wrong) yet still lose to
  author CSS and inline style. Frozen `ComputedStyle` untouched; no unsafe; no
  deps.
- Goldens (orchestrator-viewed): `presentational.tty.txt` (centering/right-align)
  + `presentational.png` (purple sized heading, pale-yellow bg, red right text).
  No pre-existing golden changed. 907 tests green. See D47.
- **Deferred follow-ups:** `<body link/vlink/alink>`, `<table border>`,
  `cellpadding`/`cellspacing`, `<td width/valign/nowrap>`, `<font face>`.

## 2026-08-15 — CORRECT-2 · <hr> renders as a real horizontal rule (tty + pixel)

- **Gap:** `<hr>` was a `display:block` void element with only a margin — an empty
  box that rendered as blank space, no visible rule.
- **Fix:** UA `hr { height:0; border-top:1px solid #808080; margin:0.5em 0; }`
  (new `border-top` longhand in the CSS parser + a top-only override in
  `cascade::resolve_border`). Pixel/fb: the existing border painter already
  draws the top edge → a gray line, no backend change. TTY: new
  `draw_top_border_rule` — the tty's first border rendering — draws `'─'`
  (U+2500) across a box's top row in the border color, but ONLY when the top is
  the box's SOLE solid border (so `<hr>` and intentional separator divs render a
  rule; full 4-side-bordered tables/flex boxes draw nothing in tty, unchanged
  from v0). Bounded by the clipped column span; no unsafe; frozen
  `ComputedStyle`/`Edges<BorderSide>` byte-identical (only internal
  `value::Declarations` gained a field).
- **Rebless (orchestrator-viewed):** `kitchen-sink.tty.txt` + `kitchen-sink.png`
  — that fixture already had an `<hr>`, which now shows its rule and gains real
  1px geometry (content below shifts down); table/flex borders stay full in
  pixel and absent in tty (confirmed no stray top-lines). New goldens
  `hr-rule.txt` / `hr-rule.png`. 869 tests green. See D46.

## 2026-08-15 — CORRECT-1 · text-align: center/right now honored (+ <center>)

- **Gap:** `text-align` was parsed + inherited in `ComputedStyle` but the inline
  engine ignored it (module doc: "not attempted here") — every line rendered
  left-aligned. And the UA sheet gave `<center>` only `display:block`, no
  centering. So vintage centered headers/nav (68k.news' title + section bar)
  rendered flush-left.
- **Fix:** UA `center { text-align: center; }` (inherits to descendants).
  `inline.rs` gains `align_offset`/`apply_line_align`: `text_align` is threaded
  from the containing block through `layout_runs`, and at each line-close
  (`<br>`, wrap, final flush) the line's runs shift right by
  `Center => (avail-width)/2`, `Right => avail-width`, clamped ≥ 0, STACKED on
  top of any float-exclusion offset. `max_width`/content-width reporting
  untouched → shrink-to-fit + flex sizing unaffected; left-aligned output is
  byte-identical. Justify treated as Left (v0). No frozen type; no unsafe.
- **Rebless:** `goldens/flex-polite.png` — that fixture's own
  `footer { text-align:center }` was previously silently ignored; the footer now
  centers correctly (verified footer-only: 798/457600 px in an 8-row band, all
  other content pixel-identical; orchestrator viewed old vs new). Golden
  `text-align.txt` added. 598 tests green. See D45.

## 2026-08-14 — UI-9 · fix: block content inside an inline wrapper collapsed to one line (68k.news)

- **Bug (user-reported, http://68k.news/):** every news list is
  `<font size="4"><ol><li>...`, and Stele rendered all items run-on on one line
  (`1. A2. B3. C`) — the whole site was unusable. Isolated to a 2-line repro:
  `<ol><li>` in `<body>` breaks correctly; the same list inside `<font>` did not.
- **Root cause:** `block.rs::is_inline_ish` judged a `display:inline` container
  (`<font>`) inline-ish regardless of contents, so `translate_container_children`
  folded it — and its block `<ol>`/`<li>` descendants — into ONE inline
  formatting-context leaf via `flatten_inline` (which recursed unconditionally),
  erasing the block boundaries (breaks, list-item layout).
- **Fix (CSS block-in-inline):** new depth-capped `contains_block_descendant`;
  `is_inline_ish` now requires `display==Inline && !contains_block_descendant`.
  An inline box holding a block descendant is no longer folded — it's translated
  as its own node and the existing block/inline partitioning (which already
  handles mixed children via anonymous-block splitting) lays the list out
  correctly. `Text`/`Replaced` never count as block, so D14's `<em><img></em>`
  stays inline. No frozen type; no unsafe; depth-capped (total). 854 tests green,
  new accept.sh check A3n. Golden `block-in-inline.txt` blessed. See D44.
- **68k.news now renders as a real news site**: headlines, then each source's
  articles as a proper numbered list, one per line — with the per-link
  highlighting from UI-8 (D43).

## 2026-08-14 — UI-8 · fix: wrapped link highlighted its whole bounding box

- **Bug (user-reported):** focusing a link whose text wraps across >1 terminal
  row highlighted a solid rectangle of non-link cells — "selecting a link
  selects the whole paragraph." Worst on link-dense pages where long links wrap
  many lines.
- **Root cause:** `extract_focusables` merged a link's per-line `Fragment`s with
  `union_rect` into a single bounding-box `rect_cells`, and `render_frame`
  filled that box solid. A link on rows 20 (cols 5..40) + 21 (cols 0..15) got
  the union rows 20..21 × cols 0..40 highlighted.
- **Fix:** `Focusable` gains `cell_spans: Vec<(c,r,w,h)>` — one rect per
  contributing fragment (per line). `render_frame` highlights each span, not the
  union. `rect_cells` stays as the bounding box for hit-testing + initial-focus
  ordering (fine as a box). Single-line links: one span == old rect, no change.
  browser.rs-only; no frozen type; no unsafe. 574 tests green. See D43.

## 2026-08-14 — UI-7 · editable forms + submit, responsive resize — the browser you can actually use

- **Editable text inputs.** Focusing a plain `<input>` puts the shell in an
  implicit edit mode (`editing_focus` → `apply_key_editing`): printable keys
  insert at a cursor, Backspace deletes in-field (NOT Back), Left/Right move the
  cursor, Enter submits the owning form. While a text field is focused `q`/`r`
  TYPE (they no longer Quit/Reload); when focus is a link/button/checkbox every
  key behaves as before (no regression). Buffers live in `ViewState.fields`
  (BTreeMap focusable-idx → String, cursor is a char offset with UTF-8-correct
  byte mapping), seeded lazily from the DOM default `value`, persisted across
  Tab between fields, capped at `MAX_FIELD_LEN=4096` against unbounded growth.
- **Submit.** Enter builds the request through the unchanged
  `Command::Submit(Request)` path: `form::serialize_submit_with_overrides`
  reuses the existing successful-controls walk + `encode_www_form` percent
  encoder, but any edited `<input>` (matched by `NodeId` via `control_node`)
  contributes the TYPED buffer instead of its DOM default. A multi-field form
  submits everything typed, from whichever field pressed Enter. GET wired;
  POST/`select`/`textarea` and Enter-activating a named submit button are
  documented v0 follow-ups. **http://google.com is now usable: focus box, type,
  Enter → `/search?q=...`.**
- **Responsive terminal resize.** The raw-mode `poll` grew a 250ms timeout
  (`POLL_TIMEOUT`, rustix `Timespec`); the loop re-queries `tcgetwinsize` at the
  top each tick and rebuilds the page + `clamp_scroll` only on an actual size
  change — so a resize reflows within ~250ms with NO keypress (measured 202ms),
  no `SIGWINCH` handler, no `unsafe`. A `dirty` flag gates the clear+redraw so
  idle timeout ticks neither flicker nor busy-spin (verified 0 bytes / 0 CPU
  ticks over 2s idle). See D41 (forms), D42 (resize).
- 831 workspace tests green; `accept.sh --tty-only` unaffected (headless path
  untouched); the interactive loop is the manually-pty-verified thin half.

## 2026-08-14 — UI-6 · tty readability (contrast guarantee) — the black-on-black fix

- **Bug:** `TextGrid::to_ansi` emitted the literal CSS foreground (default
  `Color::BLACK`) with the terminal-default background (SGR 49) whenever a cell
  had no author background. On a dark terminal that is BLACK-ON-BLACK —
  unstyled pages, error pages, and pages that set only a text color (e.g.
  httpforever) were unreadable. Goldens never caught it because they only ever
  asserted `to_text()` (chars), never the emitted color.
- **Fix (B+C):** new pure `resolve_cell_colors(fg, bg) -> (Option<Color>,
  Option<Color>)` — `None` means "emit the terminal's own default" (39/49).
  - **B — terminal-native canvas:** no author background → defer to the
    terminal theme. Unset/near-black/near-white foreground → `39` (terminal fg,
    guaranteed visible); chromatic mid-tones (link blue) pass through so
    meaningful color survives on both light and dark terminals.
  - **C — readability guarantee:** author DID set a background → we control the
    cell, so force a legible foreground: keep the author fg only if WCAG
    contrast ≥ 4.5:1, else snap to black/white by background luminance. Kills
    hostile dark-on-dark and light-on-light.
  `to_ansi` now routes every cell through it and run-length-collapses on the
  *resolved* pair; `to_text()` byte-identical (all prior tty goldens intact);
  no unsafe; no deps.
- **Golden with COLOR at last:** `goldens/tty-color.ansi` (blessed) asserts the
  emitted SGR — plain→`39;49`, hostile `#222/#111`→white-on-#111, card
  `#333/#eee`→black-on-#eee, link `#3366cc`→blue-on-default. Contrast can't
  regress silently again. See D40.
- **Known tradeoff (D40):** luminance is the non-gamma form specified in the
  packet; it errs toward over-forcing contrast (safe: never yields an illegible
  pair, only turns some author grays to pure black/white). Gamma-corrected
  luminance for author-gray fidelity is a follow-up.

## 2026-08-14 — UI-5 · background-image (+ --no-bg-images)

- CSS `background-image: url(...)` (and the `background` shorthand's url) now
  renders in the pixel backends: the image is fetched, decoded (P4), and TILED
  (repeat, top-left) behind a box's content. Freeze amendment: one field,
  `ComputedStyle.background_image: Option<Box<str>>` (raw url, not inherited).
  Decoded pixels reach the painter via a side-map (`bg_images::collect_bg_images
  -> HashMap<url, Rc<RgbaImage>>`, driver-level like `images::collect_images`),
  so no fragment/LayoutNode amendment — `raster::paint` gains a `bg_images`
  param. Bounded: dedup by resolved url, `MAX_BG_IMAGES=32` + the shared
  aggregate-byte budget; decode failures fall back to background-color; tiling
  is a bespoke `put_pixel` loop intersected with the surface bounds (a hostile
  huge box costs ≤ one iteration per on-surface pixel).
- **`--no-bg-images`** kill switch (default OFF = images ON): skips the pre-pass
  entirely → boxes show only their background-color. Tested to yield a distinct
  image-free render (hostile pages' image backgrounds nuked with one flag).
  Pixel-only (tty shows background-color via ANSI, not images — documented).
- Golden `goldens/bg-image.png` (800×195, orchestrator-VIEWED + blessed): a
  red-bordered tile repeated across a box with white text layered on top —
  tiling + text-over-bg both correct. Existing goldens byte-identical; frozen =
  the one `ComputedStyle` field; no deps; no unsafe. 551 lib tests green.
  See D39.
- **The requested round is COMPLETE**: GPL-3.0 license, `<link>` CSS,
  backgrounds (color in tty, color+image in pixels), and an interactive shell
  driven by KEYBOARD + MOUSE (gpm console + xterm) on a 486.

## 2026-08-14 — UI-4 · mouse (gpm + xterm) — point and click

- Mouse is first-class, on a bare Linux VT via **gpm** AND in terminal
  emulators via xterm SGR (auto-detected: `/dev/gpmctl` if present, else
  `\e[?1000h\e[?1006h`; never both). Click a link → follow; wheel → scroll
  (3 lines/notch). Reuses the shell's pure `hit_test` + `enter_command` (a
  click activates exactly like Enter).
- **Bespoke gpm** — no libgpm, no FFI, no `unsafe`: a `std` `UnixStream` to
  `/dev/gpmctl`, hand-serialized `Gpm_Connect` (16B) / parsed `Gpm_Event` (28B,
  exact LE offsets, size-checked → `None` on a short record). VC derived from
  `/proc/self/fd/0` (`/dev/ttyN`→N, else 0 fallback). rustix gained the
  `"event"` feature for `poll(2)` (watch stdin + the gpm fd) — cross-compiled +
  ran on i486 (CI green); no new crates.
- Pure + unit-tested (14 new): SGR parse, gpm (de)serialize, `apply_mouse`
  (viewport→page via scroll offset → hit_test → Command). Thin/manual: the
  socket connect + poll loop (no gpm/mouse-tty in CI — verify on a VT with gpm,
  or an xterm/tmux/ssh; mouse mode is disabled again on quit). Frozen types
  untouched; 521 lib tests green. See D38.
- **Stele is a browser you drive with keyboard AND mouse** — on a 486 console.
  NEXT: (e) background-image + `--no-bg-images` (the last of the requested
  round).

## 2026-08-14 — UI-3 · interactive shell (keyboard) — STELE IS A BROWSER

- `stele <url>` (no `--headless`) now launches a live terminal browser you
  DRIVE. `src/browser.rs` (new lib module, 27 pure unit tests) owns the whole
  model: `Focusable` extraction from the `interactive`-tagged fragments (+ a
  pure `hit_test(col,row)` for the mouse packet to reuse), `ViewState`
  (scroll/focus/size), a buffering `KeyParser` (partial escapes across reads,
  total on hostile bytes), `apply_key` (one pure state transition), `History`
  back-stack, `render_frame` (viewport window + yellow focus-highlight + status
  line via `to_ansi`), and DOM-side form resolution for submit.
- Keys: ↑↓/PgUp/PgDn SCROLL; Tab/Shift-Tab cycle focus (wrap + auto-scroll to
  reveal); Enter FOLLOWS a focused link (`url.resolve(href)`) or SUBMITS a
  focused submit button (reusing `form::serialize_submit`, default values);
  Backspace = back; F5 or `r` = reload; `q`/Ctrl-C = quit. Non-submit control
  editing (typing/toggling) is the c2/later scope.
- **New dep `rustix`** (charter-blessed; `default-features=false`, features
  `["termios","std"]`) for raw-mode termios + terminal-size — its linux_raw
  backend **cross-compiled clean for i486 and ran under qemu -cpu 486** (CI
  green), same posture as taffy. No `unsafe` in our code (rustix safe API). The
  thin I/O loop (`main::run_browser`: raw enter/restore, blocking read, draw)
  is the only un-CI-tested part (no terminal in CI) — the implementer verified
  it end-to-end over a real pty (Tab-highlight, follow, back, submit, restore).
- Frozen types untouched; 761 tests green. Raw mode also clears ISIG (Ctrl-C →
  byte 0x03) but keeps OPOST (no `\r\n` staircase); normal quit restores the
  terminal (a genuine mid-loop panic wouldn't under panic=abort, but the loop
  is total). See D37. NEXT: (c2) MOUSE — gpm `/dev/gpmctl` socket + xterm SGR,
  reusing `hit_test`. Then (e) background-image.

## 2026-08-14 — UI-2 · colored tty render (fg/bg + ANSI)

- The terminal gets COLOR. `TextGrid`'s cell is now `Cell { ch, fg, bg }` (was
  bare `char`); `Box` fragments finally PAINT their `background_color` into the
  cells they cover (D17's "nothing in tty" retired for backgrounds), text takes
  its `fg` from `style.color` and keeps the box's `bg`. New `to_ansi()` emits
  24-bit SGR (`\e[38;2;r;g;b;48;2;r;g;bm`), run-length-optimized (one escape
  per color change), reset per line — the render the interactive shell draws.
- `to_text()` UNCHANGED (reads only `Cell.ch`) → every existing tty golden
  byte-identical (guard test `to_text_is_blind_to_cell_color`); frames still
  composite (`blit` copies colored cells, `to_text` strips). Verified real
  flex-polite → `to_ansi`: navy `#24344d` header with white text, cream `main`,
  pale-blue `aside` — the PNG's colors, now in the terminal.
- Also parsed the **`background` shorthand** (`background: navy`, `background:
  red url(x) no-repeat` → color; url()-contents skipped so an image filename
  can't be misread as a color; `none`/no-color → unapplied + counted, C2).
- Frozen types untouched (`Fragment`/`ComputedStyle`/`surface::Color`); no
  deps; no unsafe. 467 lib + goldens green. See D36. NEXT: (c) the interactive
  shell draws this via `to_ansi`.

## 2026-08-14 — UI-1 · interactive-provenance freeze amendment

- The hook the interactive shell needs: `layout::Interactive { Link { href } |
  FormControl { kind, name, form_action } }`, and an `interactive:
  Option<Interactive>` field on the frozen `Fragment` AND `LayoutNode`.
  `box_tree` tags an `<a href>`'s whole built subtree with `Link` (so wrapped
  link text all points at one href) and a synthesized form control with
  `FormControl`; `block::emit` copies it onto EVERY fragment (each wrapped
  line, replaced atoms, floats, boxes). Raw `href` (unresolved) — the shell
  resolves at follow time.
- ZERO behavior change: painters ignore the field, so every tty + png golden
  is BYTE-IDENTICAL (verified). Only `layout::mod` frozen types changed; no
  other frozen type; no deps; no unsafe. 421 lib tests green.
- Process note: this ran concurrently with the `<link>` CSS packet in the
  SHARED working tree — a race (the other packet saw this one's uncommitted
  layout WIP and had to isolate into a worktree). Both recovered cleanly, but
  the lesson stands: **parallel packets must use worktree isolation.** See D34.
- NEXT (UI): (b) colored tty render (fg/bg cells + ANSI + `background`
  shorthand) → (c) the interactive shell (rustix raw mode, arrow-scroll,
  Tab-focus, follow-links via this `href`, Backspace-back, F5/r-refresh, +
  gpm/xterm MOUSE as first-class).

## 2026-08-14 — M6 · list-item markers

- Closes the one real dialect gap the kitchen-sink exposed: `<ul>/<ol>/<li>`
  rendered with NO bullets/numbers (markers were never implemented). `box_tree`
  now prepends a synthesized marker `Text` to each `<li>` (same stand-in
  mechanism as details/form-control markers), keyed on the nearest list parent.
- Support: `disc`/`circle`/`square` → ASCII bullets `* `/`o `/`# ` (ASCII, not
  Unicode `•`, since the bitmap font is ASCII-only — keeps BOTH the PNG and tty
  outputs meaningful); `decimal` → `N. `; `lower/upper-alpha` → bijective
  base-26; `none` → no marker. `lower/upper-roman` absent from the frozen
  `ListStyleType` enum (nothing to add). `<ol start="N">` honored; nested lists
  restart ordinals at 1 (per-list counter); `display:none` items don't consume
  an ordinal (no gaps). Total on li-outside-list, empty/huge/deep lists.
- Goldens: `fixtures/lists.html` + `goldens/lists.tty.txt` (bullets/decimals/
  nested-restart/none/start=5, all blessed). `goldens/kitchen-sink.png` +
  `.tty.txt` REGENERATED (lists now show `* ` and `1./2.`) — orchestrator
  re-VIEWED + re-blessed; every other golden confirmed byte-identical.
- Frozen paths empty-diff; no deps; no unsafe. 410 lib + list/golden tests
  green; accept.sh A3m. See D33.
- **M6 remaining is release ceremony, not features:** A7 attestation (`cargo
  vendor` + `cargo-auditable`/`audit` — needs the audit tooling added to the
  monolith-builder image, so flagged for the operator), the A5 instruction-speed
  budget, and REPORT.md finalized. The browser itself is feature-complete for
  v0.1.

## 2026-08-14 — M6 · hardening core (kitchen-sink + fuzz)

- Release hardening, part 1. `fixtures/kitchen-sink.html` — the everything-page
  (headings/inline markup/links/lists/blockquote/pre/br/table w. rowspan+colspan/
  form/inline+floated images/flexbox/details/noscript/entities, styled via
  `<style>`+inline). `goldens/kitchen-sink.png` (800×1504) + `.tty.txt`
  (orchestrator-VIEWED + blessed): every feature renders correctly IN
  COMBINATION — float wraps and returns, flex grows beside a fixed box, table
  borders + spans, details open/closed, noscript shown. The A5 coverage proof.
- **Hand-rolled mutation FUZZ** (`tests/fuzz_totality.rs`, ~4400 deterministic
  iterations across 4 categories — HTML byte-mutation, random blobs, random
  CSS, image-decode — seeded xorshift64*, ~0.6s): drives the WHOLE pipeline
  (parse→collect_author_sheets→flatten_media→cascade→box_tree→layout→tty/raster)
  asserting no panic/abort. `panic=abort` makes any find a hard, visible
  failure. Found exactly ONE bug; none other in 4400 iters — strong totality
  signal for the whole engine.
    - **Bug found + fixed:** `<br>` was a total NO-OP (void element that
      contributed nothing to layout — `a<br>b` ran together on one line). Fix:
      `box_tree` emits a `LINE_BREAK_SENTINEL` (U+E000, a PUA char) inside the
      frozen `BoxContent::Text`, which `inline` recognizes as a forced break —
      no freeze amendment (documented tradeoff: literal U+E000 in real content
      would misrender as a break, cosmetic-only, never a panic; a proper
      `BoxContent::LineBreak` amendment is the clean path if wanted later).
- A2 (size ≤2MB) flipped from informational to a HARD accept gate — comfortably
  met (i486 binary ~542KB). Frozen types zero-diff (only `box_tree`/`inline`
  impl changed); no deps; no unsafe. 653 tests green. See D32.
- Gaps surfaced (pre-existing, documented, NOT regressions): list markers
  (`<ul>/<ol>/<li>` render without bullets/numbers — never implemented) and
  `<pre>` whitespace-collapse. NEXT M6: list markers (quick follow-up), then the
  A7 attestation ceremony (`cargo vendor` + `cargo-auditable`/`audit`), the A5
  instruction-speed budget, and REPORT.md finalized.

## 2026-08-14 — M5 · dialect completeness (details/noscript/entities/--stats)

- Finishes the curated dialect's remaining items (all in `box_tree`/`ua`/`main`
  impl — frozen types zero-diff):
  - **`<details>/<summary>`**: honors the `open` attribute — collapsed (no
    `open`) renders ONLY the first `<summary>` (others dropped); `<details
    open>` renders summary + body. Default `"Details"` label when no summary;
    ASCII disclosure markers `> ` (closed) / `v ` (open). Golden shows a
    collapsed section (body absent) beside an open one (body shown).
  - **`<noscript>`**: added to the UA `display:block` group — its content is
    SHOWN (Stele runs no script by construction, so `<noscript>` is "what to
    render when scripting is off" = always). Golden shows the fallback `<p>`.
  - **entities**: `fixtures/entities.html` verifies HTML 4.01 named + decimal +
    hex numeric decoding — `&copy;`→©, `&reg;`→®, `&mdash;`→—, `&#169;`→©,
    `&#x2014;`→—, unknown `&notanentity;` stays literal. All correct, NO P1 bug.
    Documented finding (frozen-layout, not fixed): `&nbsp;` decodes to U+00A0
    but `inline`'s tokenizer collapses it (Rust `char::is_whitespace` includes
    NBSP) → renders as a plain space; flagged for a later inline-engine tweak.
  - **`--stats`**: C2 "count what we refuse" — prints to STDERR
    `N ignored declaration(s), N ignored at-rule(s), N media block(s)`
    aggregated across author sheets; stdout/goldens unaffected (verified via a
    real subprocess spawn).
- Orchestrator-reviewed directly + 3 goldens countersigned (details open/closed,
  noscript-visible, entities decoded). Frozen paths empty-diff; no deps; no
  unsafe. 401 lib + bin/golden tests green; accept.sh A3j/A3k/A3l. See D31.
- **M5 essentially COMPLETE**: author CSS · flexbox pixel-green · @media ·
  details/summary · noscript · entities · --stats. Remaining minor item: wire
  the cookie jar (C6) to a plain-file for cross-invocation persistence
  (HTTP-only; fixtures are file://) — small, can fold into M6. NEXT: **M6** —
  hardening (parser fuzz, size/strip pass, A1–A7 acceptance all green, REPORT.md
  finalized) — the release gate.

## 2026-08-14 — M5 · @media — responsive CSS responds

- `@media` queries now WORK. They were parsed only to be COUNTED and discarded
  (`media_at_rules`); this packet parses + STORES the query + its rules and
  evaluates them against the render viewport in a PRE-PASS — so `cascade`'s
  frozen signature never needs a viewport (its diff vs main is zero).
- `style::media` (new): `MediaQuery` parser + `matches(viewport_width)` (types
  `all`/`screen`, `(min/max/width: px)`, `and`, `,`=OR; `print`/unknown/`not`/
  malformed fail closed) + `flatten_media(sheet, viewport) -> Stylesheet`.
  Parser stores `Stylesheet.media_rules`; `parse_rule` refactored to share a
  single global `order` counter across top-level and `@media`-body rules, so a
  matching `@media` block folds in at exactly its source position (a later
  `@media` still beats an earlier equal-specificity top-level rule) with no
  positional fixups. Pipeline: `collect_author_sheets_for_viewport` →
  `flatten_media` → `cascade`. Viewport width per mode: dump_text `cols*8`,
  dump_png 800, fb device width, frames per-region width.
- Goldens (blessed) — `fixtures/media-query.html` at two widths prove both
  branches: WIDE (640px) → `(max-width:500px)` doesn't fire, sidebar visible;
  NARROW (320px, `--cols 40`) → fires, sidebar `display:none` and a narrow
  notice appears. Tested through the REAL viewport-aware pipeline (+ a baseline
  test proving the flatten is load-bearing).
- `min/max-height` out of scope (every mode lays out at content-driven
  effectively-unbounded height — no meaningful height viewport). Orchestrator-
  reviewed directly + goldens countersigned; frozen zero-diff; no deps/unsafe.
  388 lib tests green; accept.sh A3h/A3i. See D30.
- **M5 progressing:** author CSS ✅ · flexbox ✅ · @media ✅. NEXT:
  `details`/`summary`, `noscript`, full entity coverage, `--stats` counter.

## 2026-08-14 — M5 · flex-polite pixel-green — MODERN FLEX ON A 486

- The charter's headline promise, delivered and VISIBLE: a modern no-JS blog
  layout in real CSS flexbox, styled via an author `<style>` block, rendered to
  pixels. `fixtures/flex-polite.html` → `goldens/flex-polite.png` (800×572,
  orchestrator-VIEWED and blessed): a `justify-content: space-between` header
  (title left / nav right, `align-items: center` across font sizes), a
  two-column body (`flex-grow: 1` article beside a fixed 220px `aside`), a
  full-width footer — backgrounds/borders/padding all from author CSS.
- **Genuine flex bug found + fixed** (first real flex render surfaced it, as
  hoped): `block::translate_container_children`'s flex branch turned EVERY child
  into a flex item — including whitespace-only `Text` nodes from source
  newlines/indentation — so `<nav>\n <a>..</a>\n <a>..</a>\n</nav>` grew a
  phantom zero-width flex item that doubled the `gap`. Per CSS Flexbox §4 a
  child text node of only collapsible whitespace generates NO anonymous flex
  item; fixed via `is_whitespace_only_text` skip. RED test measured 82px vs
  66px expected gap; GREEN after. All other flex mechanics (row, flex-grow vs
  fixed sibling, space-between, gap, align-items) were correct on first render.
- The golden test renders through the REAL author-CSS pipeline
  (`collect_author_sheets` + `cascade(dom, &sheets)`), with a load-bearing test
  proving it (`without_author_css_the_header_children_would_stack_instead_of_flex`).
- Orchestrator-reviewed directly + visually blessed; frozen paths empty-diff;
  no deps; no unsafe. 363 lib + flex geometry/golden tests green. accept.sh A3g.
  See D29.
- **M5 progressing:** author CSS ✅ · flex-polite ✅. NEXT: `@media` evaluation
  (needs a small viewport freeze amendment threading the viewport into cascade),
  then `details`/`summary`, `noscript`, full entities, `--stats` counter.

## 2026-08-14 — M5 · author CSS wired — pages get their own styles

- **The CSS engine is now actually used.** Discovery: every `cascade()` call in
  the render pipeline passed `&[]` for author sheets, and inline `style=` was
  ignored — so P2's whole CSS cascade (selectors/specificity/origins) had been
  built and unit-tested but NEVER fed a real page's styles. Every fixture had
  rendered UA-only. This packet wires author CSS end to end.
- `style::collect_author_sheets(dom)` (new `style::author`, explicit-stack
  walk) extracts every `<style>` block's raw text → `parser::parse` → author
  sheets in document order. `cascade`'s `visit` now folds each element's inline
  `style="..."` (via `parser::parse_inline`) LAST, so inline is the
  highest-precedence origin (beats author + UA per property), signature
  unchanged (read off the `Element` in hand). Wired into all `--dump-text`/
  `--dump-png`/`--render-fb`/frames cascade sites.
- Golden `goldens/author-css.tty.txt` (PROPOSED → orchestrator-blessed): an
  author rule `p.hidden{display:none}` hides one paragraph; a third paragraph
  with the same class but `style="display:block"` stays visible — origin order
  UA < author < inline made concrete. All existing goldens confirmed
  byte-identical (no prior golden fixture used author CSS).
- Orchestrator-reviewed directly (contained cascade change + new module):
  correct fold, depth-safe extraction, frozen unchanged; 363 lib tests green
  incl. pathological (5000-rule `<style>`, 3000-deep DOM, garbage inline) — no
  panic. No deps, no unsafe. `<link rel=stylesheet>` external CSS deferred
  (needs a fetch pre-pass like images). See D28.
- **M5 underway.** NEXT: `flex-polite.html` pixel-green (flexbox already lays
  out via taffy; now it can be styled + rendered + viewed), then `@media`
  evaluation, `details`/`summary`, `noscript`, `--stats`.

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

## 2026-08-19 — HTTPS PR 1: ByteStream seam + centralized fetch dispatch (packet/bytestream-seam, #78)

- **First of two packets** delivering `https://` via a delegated `openssl s_client`
  child (TLS delegated, never embedded — zero crypto bytes in the binary). Design:
  `docs/superpowers/specs/2026-08-19-https-openssl-transport-design.md`; plan:
  `docs/superpowers/plans/2026-08-19-bytestream-seam.md`.
- **This packet is a PURE REFACTOR — zero behavior change:**
  - New `src/fetch/transport.rs`: `trait ByteStream: Read + Write { shutdown_write }`
    with the only PR-1 impl, `TcpStream`. The seam PR 2's openssl child slots into.
    `shutdown_write` is defined but deliberately **not called** on the HTTP path here
    (calling it would add a FIN-after-request — a wire change).
  - `http1`'s five read helpers made generic over `R: Read`; write+read core factored
    into `exchange<S: ByteStream>`; `send_one` still `TcpStream::connect`s and calls it.
    Helper bodies left byte-identical (totality, timeouts, `MAX_RESPONSE_BYTES`,
    CRLF-injection rejection all preserved verbatim).
  - `fetch::fetch(&Request)` — the single scheme→fetcher table — plus
    `fetch::err_to_string`, which exists so the six rewired call sites keep byte-identical
    error text. The six copy-pasted `match url.scheme()` blocks (`main.rs` ×2,
    `images.rs`, `stylesheets.rs`, `bg_images.rs`, `frames.rs`) now delegate; adding
    `https` in PR 2 is one arm in one place.
- **Proof of inertness:** `tests/fetch_http1.rs` framing tests (chunked / Content-Length /
  garbage) unchanged & green; all PNG/tty goldens **byte-identical** (accept.sh A5);
  4 new `fetch` dispatch unit tests. CI `m0-acceptance` green on the final push.
- **Size (bonus win):** i486 binary **1,213,524 B**, **−4,096 B vs the branch point**
  (deduplicating six inlined dispatch/format copies into one shrank it) — 261,036 B under
  the 1.44 MB floppy ceiling (A2: 1,213,524 ≤ 2,000,000).
- Executed subagent-driven (3 impl tasks, per-task review + Opus whole-branch review, all
  clean). Deferred cosmetic-only doc nits: `transport.rs` module doc and four module `//!`
  headers still carry pre-refactor phrasing — non-blocking, for a later touch.
- **Next: PR 2 (`packet/https-openssl`)** — `OpensslStream` child `ByteStream`, `https`
  scheme + 443, fail-closed flag probe, `STELE_CA_FILE` + default-path CA probe, legible
  TLS-failure error document, charter C2 amendment + first `DECISIONS.md` transport entry,
  and the `openssl s_server` fixture suite + covenant grep (zero TLS symbols in the binary).
