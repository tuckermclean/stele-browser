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
