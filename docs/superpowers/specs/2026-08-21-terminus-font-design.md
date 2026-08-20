# Terminus font — design

**Date:** 2026-08-21 · Font-rendering packet (not on the Acid2 track). Replaces the compiled-in `font8x8`
bitmap atlas with an embedded, portable subset of **Terminus** (OFL-1.1, Dimitar Toshkov Zhekov) as Stele's
one and only rendering font, across every backend (tty, fb/raster, chrome, `--dump-png`). **This document is
spec + plan only — no implementation.** The companion plan is
`docs/superpowers/plans/2026-08-21-terminus-font-plan.md`.

## Goal
Swap the glyph source Stele actually paints from `font8x8` (8×8 source, upscaled into a synthetic 8×16
"VGA cell") to real, per-size Terminus bitmaps — five embedded cell sizes × two weights, ASCII + Latin-1
coverage only — with zero external font dependency on any OS (Linux or otherwise), inside the 1.44 MB floppy
budget, and with the layout blast radius (every text-bearing golden re-blessing) owned explicitly rather than
hand-waved.

## Non-negotiables (AGENTS.md, unchanged by this packet)
- **No JavaScript, by construction** (charter C3) — this packet touches only glyph data and rendering; no
  new DOM/CSS execution surface.
- **1.44 MB floppy ceiling** (1,474,560 bytes). Per JOURNAL.md's latest size entry, `stele-i486` is
  **1,287,324 bytes**, i.e. **187,236 bytes (≈182.9 KiB) of headroom** as of that entry — reconfirm this
  number at implementation time (packets may have landed since). The embedded glyph budget below (§6) is
  priced against that headroom, not against the CI A2 gate's more permissive 2,000,000-byte line
  (AGENTS.md rule 2: the floppy is the *real* ceiling).
- **CI-driven build/test.** No local `cargo build`/`cargo test`. Every task pushes and reads `m0-acceptance`;
  goldens are re-blessed from the downloaded CI artifact, never a local render.
- **Goldens are byte-compared; pixel-verify before blessing, never rubber-stamp.** This packet's whole
  premise is a font swap that visibly changes glyph shapes at every text size in the 5-bucket embedded set —
  the golden-rebless sweep (Design §4, Plan Task 6) is scoped as its OWN integration task per AGENTS.md rule
  4, not folded into the code tasks.
- **Test-first.** Every code task below starts with a failing test (glyph-table correctness against known
  BDF bitmaps, nearest-size selection, per-size metrics) before the implementation that turns it green.

## Current state (ground-truthed 2026-08-21)

### The `Metrics` seam (frozen, unchanged by this packet)
`src/text/mod.rs:26-44` — the `Metrics` trait: `ascent(size_px)`, `descent(size_px)`, `line_height(size_px)`,
`advance(ch, size_px)`, and a default `measure(s, size_px)` (sum of per-char advances). All five stay exactly
as they are; this packet adds a *second implementer*, not a trait change. The module doc comment
(`text/mod.rs:1-17`) **already anticipates this packet by name**: *"Shaping-free... v0 is monospace... double-
width scripts and proportional fonts are later refinements"* and the standalone helper below it is even more
explicit.

`src/text/mod.rs:83-89`, `pub fn text_render_px(font_size: f32) -> f32`, today: `font_size.max(16.0)` (a flat
floor at font8x8's one native size, non-finite → `16.0`). Its own doc comment
(`text/mod.rs:46-78`) is the single most load-bearing fact for this packet:

> *"Revisit-trigger: once a second, non-`font8x8` font lands (scalable, or just a different fixed-cell
> atlas), a hardcoded 'never below 16px' floor stops being universally correct — this function (and every
> call site that routes through it) should be reconsidered at that point, not kept as a permanent fixture."*

That trigger has now fired. §2 below turns `text_render_px` from a one-size floor into a five-size *nearest*
snap, **keeping its name and `fn(f32) -> f32` signature** so every existing call site
(`raster.rs:472`; `inline.rs:347,349,372,661,692`) needs zero signature changes — only its body and doc
comment change.

### `BitmapFont` (`src/text/bitmap.rs:38-114`) — the current, sole `Metrics` implementer
A `#[derive(...)] struct BitmapFont { cell_width, cell_height, ascent_units, descent_units }` (all `f32`,
design units), built by `with_cell(w, h)` (generic, 3:1 ascent:descent split, floors degenerate geometry to
`1.0`, `bitmap.rs:64-70`) or `vga_8x16()` (`bitmap.rs:74-76`, `= with_cell(8.0, 16.0)`, i.e. 12-unit ascent /
4-unit descent). Every `Metrics` method **scales continuously**: `scale(size_px) = size_px / cell_height`
(`bitmap.rs:82-88`), so `advance`/`ascent`/`descent`/`line_height` are all linear in `size_px` for *any*
positive value — there is no discrete "size bucket" concept today. `BitmapFont::glyph(ch) -> [u8; 8]`
(`bitmap.rs:111-113`) delegates to `super::glyphs::lookup`, independent of `self`'s own cell geometry — the
doc comment there is explicit that the atlas is *always* 8×8 source pixels and only `vga_8x16()` is ever used
to rasterize a real document (`layout::layout` hardcodes it, per `layout/mod.rs:193,207` below).

### `glyphs.rs` (`src/text/glyphs.rs:1-75`+) — the embedded font8x8 atlas being replaced
Two compiled-in tables: `FONT8X8_BASIC` (128 glyphs, `U+0000..=U+007F`, includes blank-glyph control chars)
and `FONT8X8_EXT_LATIN` (96 glyphs, `U+00A0..=U+00FF`), both Public Domain (dhepper/font8x8). **Bit order:
bit 0 (LSB) = leftmost pixel** (`glyphs.rs:42-53`, confirmed empirically against `'A'`'s known silhouette) —
the *opposite* convention from raw BDF (see §1 below; this is a real hazard for the extraction tool).
`lookup(ch)` is total over all of `char`: outside its two ranges it returns `FALLBACK_GLYPH` (a hollow "tofu"
box), but in practice `text::translit::resolve` (see next) intercepts first for anything outside ASCII/Latin-1
and either transliterates or drops-and-counts, so `FALLBACK_GLYPH` is now a lower-level safety net, not the
last word on what paints.

### Every call site that constructs a font today (file:line, exhaustive)
| Site | What it does |
|---|---|
| `src/backend/chrome.rs:203,225` | `BitmapFont::vga_8x16()` — chrome-bar label metrics/rasterization |
| `src/layout/mod.rs:193,207` | `BitmapFont::vga_8x16()` — the metrics instance `layout::layout` hands to `layout_tree` |
| `src/surface/mem.rs:162` | `BitmapFont::vga_8x16()` inside `MemSurface::draw_text` — the actual glyph rasterizer |
| `src/backend/raster.rs:472` | no direct font construction, but calls `text::text_render_px` before building every `TextRun` (`paint_text`, `raster.rs:453-475`) |
| `src/layout/inline.rs:59,345-372,593-692` | `Metrics`-generic (`<M: Metrics>`), calls `text_render_px` at every line-height/advance/space-run site; test-only code additionally hardcodes `BitmapFont::vga_8x16()` at `inline.rs:1331` etc. |
| `src/layout/block.rs:137,363,374,382,...` | `Metrics`-generic (`<M: Metrics>` on `layout_tree`/`measure_node`/table-cache functions); test-only code hardcodes `vga_8x16()` (`block.rs:2560` etc.) |
| `src/backend/tty.rs` | **No font/glyph construction at all** — the tty backend stores raw `char`s in grid cells (`tty.rs:10` doc: "matching `text::BitmapFont::vga_8x16`" is a *metrics-shape* comment, not a rasterization dependency) |

Every *production* (non-test) rasterization/metrics call funnels through exactly three files:
`chrome.rs`, `layout/mod.rs`, `surface/mem.rs`. Everything else (`inline.rs`, `block.rs`) is already
`Metrics`-generic — it takes whichever font the caller hands it and needs **no changes** to accept a new
implementer, confirming the trait boundary is exactly where the module doc said it would be.

### `TextRun` has no weight field — `font-weight: bold` is cascaded but never rendered, today
`src/surface/mod.rs:49-55`:
```rust
pub struct TextRun<'a> {
    pub text: &'a str,
    pub x: i32,
    pub baseline: i32,
    pub size_px: f32,
    pub color: Color,
}
```
No weight/bold field. `ComputedStyle::font_weight` (`style/computed.rs:146-149`, `FontWeight::{Normal,
Bold}`, cascaded correctly — `ua.rs:48-55` sets `b, strong { font-weight: bold }` and all headings bold;
`cascade.rs:1025-1028` tests it) is **read nowhere in `backend/raster.rs` or `backend/tty.rs`** (grep-
confirmed: no `font_weight`/`bold`/`weight` symbol in either file). `paint_text` (`raster.rs:453-475`) builds
its `TextRun` from `style.font_size`/`color` only — `style.font_weight` is computed, cascaded, and then
silently discarded. **This is a real, previously-invisible gap**, not a regression this packet introduces:
`font-weight: bold` has never visually done anything in Stele, because font8x8 has no bold variant to switch
to. Wiring it to Terminus's real bold weight (§3) is *new* rendering behavior, not a font-swap-only change —
call this out explicitly in the PR, since it will visibly change every golden containing `<b>`/`<strong>`/a
heading, independent of the Terminus glyph-shape change itself.

### The default text size, precisely
`style/computed.rs:566`: `ComputedStyle::default().font_size = 16.0` (the CSS UA default). `ua.rs:48-53`
scales headings off it (`h1: 2em` → 32px, ... `h5: 0.83em` ≈ 13.28px, `h6: 0.67em` ≈ 10.72px). Today, *every*
size below 16px is floored UP to 16px by `text_render_px` — h5/h6 currently render at the SAME 16px cell as
body text.

### The BDF sources (already staged, scratchpad; ground-truthed by reading them directly)
`/tmp/.../scratchpad/terminus/terminus-font-4.49.1/ter-u{12,16,20,24,32}{n,b}.bdf` — `CHARS 1356` each,
`SPACING "C"` (fully monospace: every glyph's `DWIDTH` equals the font's cell width, confirmed by scanning
`ter-u16n.bdf` and `ter-u32n.bdf`'s entire ASCII+Latin-1 range — one distinct `DWIDTH` value each). Per-size
geometry (from each file's `FONTBOUNDINGBOX`/`FONT_ASCENT`/`FONT_DESCENT`/`DWIDTH`, normal weight; bold's
`DWIDTH` is identical to normal's at every size, confirmed for size 16):

| CSS size bucket | cell (w×h) | ascent | descent | advance (DWIDTH) | BDF row encoding |
|---|---|---|---|---|---|
| 12 | 6×12 | 10 | 2 | 6 | 1 byte/row (`ceil(6/8)`) |
| 16 | 8×16 | 12 | 4 | 8 | 1 byte/row (`ceil(8/8)`) |
| 20 | 10×20 | 16 | 4 | 10 | 2 bytes/row (`ceil(10/8)`) |
| 24 | 12×24 | 19 | 5 | 12 | 2 bytes/row (`ceil(12/8)`) |
| 32 | 16×32 | 26 | 6 | 16 | 2 bytes/row (`ceil(16/8)`) |

Glyph count per (size, weight): `(0x20..=0x7E)` (95, printable ASCII) + `(0xA0..=0xFF)` (96, Latin-1
supplement) = **191**, grep-confirmed against `ter-u16n.bdf` — exactly the number the task brief's own
"already computed" cost is built on. **Notably excluded from the subset, and thus a real (small) coverage
narrowing vs. today's font8x8**: `0x00-0x1F` (control chars) and `0x7F` (DEL) — font8x8's `FONT8X8_BASIC`
covers the full `0x00-0x7F` and renders those as blank glyphs; Terminus's subset here doesn't embed them at
all. See §3/§Risks for why this is very likely a non-issue in practice, and the regression test that proves
it.

**Bit order — BDF is MSB-first, font8x8 is LSB-first; this MUST be normalized in the generator.** Decoded
`ter-u16n.bdf`'s `'A'` bitmap by hand: row byte `0x42` (`0100 0010`) lights columns 1 and 6 (0-indexed from
the left) when read **bit 7 = leftmost** — the opposite of `glyphs.rs`'s documented `bit 0 = leftmost`
convention. The extraction tool must bit-reverse each row during generation so the new tables share the
*same* leftmost-pixel convention as the (soon-removed) font8x8 tables did — one bit-order mental model
project-wide, not two.

### The OFL-1.1 license text, read directly
`OFL.TXT:1-2`: *"Copyright (C) 2020 Dimitar Toshkov Zhekov, with Reserved Font Name 'Terminus Font'."*
Condition 3 (`OFL.TXT:64-66`): *"No Modified Version of the Font Software may use the Reserved Font Name(s)
unless explicit written permission is granted... This restriction only applies to the primary font name as
presented to the users."* A 191-glyph, 5-size subset is unambiguously a **Modified Version** (OFL's own
definition: *"adding to, deleting, or substituting... any of the components"*). Condition 2 requires the
license text to travel with any bundled/redistributed copy (stand-alone file, human-readable header, or
machine-readable metadata). Condition 1: the font itself may never be sold standalone (not a concern — it's
embedded as an inseparable rendering dependency, not a product).

## Design

### 1. Embedding mechanism
**Recommendation: a checked-in extraction tool, `tools/gen-terminus-glyphs.py`, reads the 10 source BDF files
and emits a single GENERATED Rust source file, `src/text/terminus_glyphs.rs`, committed to the repo.** No
`build.rs`, no BDF file needed at build time — matches AGENTS.md rule 3 (CI-driven, no local/exotic build
step) and keeps the i486 cross-build dependency-free and reproducible from `cargo build` alone.

- **Do NOT commit the source BDFs.** The 10 files used total ≈2.45 MB of third-party text — a real ongoing
  repo-hygiene cost (git history bloat) for zero binary-size benefit (they never ship; only the *generated*
  Rust file does). Instead, `tools/gen-terminus-glyphs.py`'s own doc comment/README pins: the upstream
  release URL, the exact version (`4.49.1`), and a SHA-256 of the tarball to verify against before
  extracting — computed and recorded once, at implementation time, when the tool is actually authored (not
  fabricated here). Anyone regenerating the tables re-downloads, verifies the hash, extracts, and re-runs the
  tool against the extracted directory (`--bdf-dir <path>`).
- **Do commit `OFL.TXT` verbatim** plus a short attribution note, at `third_party/terminus-font/OFL.TXT` +
  `third_party/terminus-font/NOTICE.md` (new directory — the repo has no existing `third_party/`/`vendor/`
  convention; this establishes one, small and self-contained). This satisfies OFL condition 2 at the
  **source/repository level** (every checkout/clone of Stele carries the license alongside the derived
  glyph data) — the compiled `stele-i486` binary itself does **not** embed the license TEXT as bytes (that
  would cost real floppy budget for a string never read at runtime); this is the same posture Stele already
  takes with font8x8's Public-Domain notice (lives in `REPORT.md`/doc comments, not in the binary). Flag this
  judgment call explicitly in the PR description for the operator to confirm, mirroring the acid2-eyes
  packet's practice for judgment calls it couldn't fully close itself.
- **Reserved-name compliance, addressed explicitly:** Stele has no font picker / no `font-family` value ever
  presented to a user as a chosen typeface — Terminus-derived is the *only* rendering path, silently. Per
  OFL condition 3's own text ("applies to the primary font name **as presented to the users**"), there is no
  such presentation surface in this project, so the RFN restriction has nothing to bind to today. Even so, as
  a matter of good practice (and per condition 4, not misusing the author's name to *promote* a modified
  product): the generated file, doc comments, `REPORT.md`, and `LICENSE`-adjacent notes describe this
  explicitly as **"a 191-glyph subset of Terminus Font by Dimitar Toshkov Zhekov, embedded under OFL-1.1"** —
  attribution, not a claim that Stele ships "Terminus" unmodified or that the subset is itself distributed
  under the name "Terminus Font."

### 2. Multi-size font abstraction
**A new type, `TerminusFont` (`src/text/terminus.rs`, new module), implements `Metrics` — `BitmapFont` is
retired (§3), not extended.** `BitmapFont`'s whole model is *continuous* scaling from one design cell
(`scale = size_px / cell_height`, any positive `size_px` gives a linearly-scaled answer) — that model is
simply wrong for Terminus, which ships **discrete, individually-hinted bitmaps** at 5 sizes; synthetically
up/downscaling e.g. the 16px bitmap to approximate 20px would look worse than using the real 20px bitmap that
already exists. So `TerminusFont` does not scale at all — it **snaps**.

**Nearest-size rule** (the single source of truth, exposed as one function reused by both `TerminusFont` and
`text_render_px`, §3): sizes `S = [12.0, 16.0, 20.0, 24.0, 32.0]`.
- `px <= 12.0` → `12.0`; `px >= 32.0` → `32.0`.
- Otherwise, `px` falls between two adjacent buckets `S[i] < px < S[i+1]`: pick the nearer one; on an *exact*
  midpoint (`14`, `18`, `22`, `28`) **round up** to the larger bucket — consistent with the *existing*
  "never shrink below legible" philosophy `text_render_px`'s flat floor already embodied (§ Current state),
  generalized from "always floor to the one size" to "always break a tie toward more legible."

Given a requested `size_px`, `TerminusFont`'s `Metrics` methods snap it to a bucket internally and return
that bucket's **real, hand-verified** ascent/descent/advance/line-height (the table in Current State §BDF
sources) — not a linear scaling of them. `advance`/`ascent`/`descent`/`line_height` are therefore step
functions of `size_px`, not continuous ones; document this divergence from `BitmapFont`'s old contract
directly in `TerminusFont`'s module doc, since it's a real, deliberate behavior change future maintainers
need to understand at a glance.

**Weight selection:** `ComputedStyle::font_weight` is exactly binary (`FontWeight::{Normal, Bold}`,
`computed.rs:146-149`) — a 1:1 match to Terminus's `n`/`b` BDF pairs, no interpolation needed.
`TerminusFont::glyph(ch: char, weight: FontWeight, size_px: f32) -> Glyph` picks the (bucket, weight) table
and looks up `ch`; total over all of `char` (a char outside the 191-glyph subset returns a small fallback
box at the bucket's own cell size — same "never vanish, never panic" contract `glyphs::lookup` already
guaranteed, carried forward rather than dropped).

**Storage vs. lookup-API split** (keeps both the byte budget and the code simple): sizes 12/16 (cell width
≤ 8) store one `u8` per bitmap row; sizes 20/24/32 (cell width 9-16) store one `u16` per row — this mixed
width is exactly what the §6 budget below is priced on. `TerminusFont::glyph`'s *public* return type widens
`u8` rows to `u16` at lookup time (a free `as u16` cast) so callers see one uniform row type regardless of
which bucket they hit — storage stays byte-tight, the API stays simple.

### 3. Integration / default
Terminus becomes the **only** render font — swap all three production call sites (§ Current state's table)
from `BitmapFont::vga_8x16()` to `TerminusFont::new()` (or equivalent constructor): `chrome.rs:203,225`,
`layout/mod.rs:193,207`, and `surface/mem.rs`'s `draw_text`/`draw_glyph` (which additionally needs
generalizing from font8x8's fixed `GLYPH_W = GLYPH_H = 8` constants, `mem.rs:198-199`, to the snapped
bucket's own cell dimensions — `draw_glyph`'s nearest-neighbor scale loop, `mem.rs:249-287`, becomes a
*copy* loop instead once glyphs are already stored at their real target size, since there's no upscale/
downscale left to do within a bucket). `layout/inline.rs`/`layout/block.rs` need **no changes** — they're
already `Metrics`-generic.

**`font-weight: bold` gets wired for the first time** (§ Current state — it does nothing today):
`TextRun` (`surface/mod.rs:49-55`) gains a `pub weight: FontWeight` field (or a plain `bold: bool` — either
is fine; `FontWeight` avoids a second boolean encoding of the same cascade value). `paint_text`
(`raster.rs:453-475`) reads `style.font_weight` (already in scope — `style: &ComputedStyle` is already a
parameter) and forwards it into the `TextRun` it builds; `MemSurface::draw_text` passes it through to
`TerminusFont::glyph`. This is genuinely new, testable behavior — call it out as such in the PR, not as an
incidental side effect of the font swap.

**Default size mapping — the load-bearing fact for the blast-radius argument (§4):** CSS default
`font-size: 16px` (`computed.rs:566`) snaps to the **16px bucket** — `ter-u16{n,b}`, cell **8×16**, ascent
**12**, descent **4**, advance **8**. Compare to today's `BitmapFont::vga_8x16()` at 16px: cell 8×16, ascent
12 (`12 * scale(16)=1.0`), descent 4, advance 8 (`bitmap.rs:220-223`, its own pinned test). **These are
numerically identical.** At the engine's default text size, swapping fonts changes *only which pixels paint
inside each 8×16 cell* (real Terminus glyph shapes vs. font8x8's upscaled-from-8×8 shapes) — box geometry,
line-breaking, and line height at the default size are unchanged.

**Fixed-width fallback:** `FontStyle::Italic` (also cascaded, `ua.rs`/`computed.rs`) stays unwired — Terminus
ships an oblique ('v') variant for only sizes 14 and 16 (neither/only-one in the embedded 5-size set,
confirmed by directory listing), so there's no clean full-coverage italic to embed without synthesizing a
slant, which this packet does not do. Flagged as an explicit non-goal below, not silently dropped.

### 4. Metrics/layout blast radius — owned explicitly
Per-bucket comparison, today's font8x8-via-`vga_8x16` (scaled) vs. native Terminus (all five embedded
buckets):

| bucket | advance (old→new) | line_height (old→new) | ascent (old→new) | descent (old→new) |
|---|---|---|---|---|
| 12 | 6 → 6 | 12 → 12 | 9 → **10** | 3 → **2** |
| 16 (default) | 8 → 8 | 16 → 16 | 12 → 12 | 4 → 4 |
| 20 | 10 → 10 | 20 → 20 | 15 → **16** | 5 → **4** |
| 24 | 12 → 12 | 24 → 24 | 18 → **19** | 6 → **5** |
| 32 | 16 → 16 | 32 → 32 | 24 → **26** | 8 → **6** |

**Horizontal layout (advance) and line-height are identical at every bucket boundary** — Terminus's own cell
proportions (width = height/2 at every size) happen to match the ratio `BitmapFont::vga_8x16`'s scaling
already assumed. Only the ascent:descent **baseline split** shifts by 1-2px at four of the five buckets (the
default 16px bucket is exact-zero change). This is a real but *small* vertical nudge, not a width/wrapping
change, at the five bucket values themselves.

**The actual reflow driver is the *nearest-size snap itself*, not the per-bucket metrics.** Today,
*every* `font_size < 16` floors UP to 16 (flat floor). Under the new nearest-of-5 rule, any element whose
computed `font-size` is **below the 12/16 midpoint (14px)** now snaps DOWN to the 12px bucket instead of UP
to 16px — a genuinely smaller render (advance 6 vs. 8, line-height 12 vs. 16) for anything in that range.
Concretely, in the shipped UA stylesheet: `h6 { font-size: 0.67em }` (≈10.72px off a 16px parent) and `h5`
(≈13.28px) both currently render at the 16px floor and will now render at the **12px bucket** — visibly
smaller headings, a real, intended-but-must-be-verified layout change. Any *author* stylesheet with a small
`font-size` (e.g. `font-size: 10px` captions) is affected the same way. Symmetrically, any author `font-size`
**above 32px** (today scaled up arbitrarily by `BitmapFont`, e.g. `font-size: 48px` → advance 24, ascent 36)
now **clamps down to the 32px bucket** (advance 16, ascent 26) instead of continuing to scale — a real
shrink for oversized headings, a Terminus-embedding tradeoff (no glyph source exists above 32px in the
subset) that must be stated plainly, not glossed over.

**This changes every text-bearing golden.** `goldens/` currently holds 36 PNG goldens + 23 tty `.txt`
goldens (`ls goldens/*.png goldens/*.txt` — grep-confirmed count as of this design). Nearly all touch text.
The plan's golden-rebless task (Plan Task 6) is its own AGENTS.md-rule-4 integration task: render everything
via CI, pixel-spot-check a representative sample (default-body-text fixture at 16px to confirm the "zero
metrics change, only glyph shape" claim above; a heading-heavy fixture to confirm h5/h6 visibly shrink as
predicted, not garbage; a `<b>`/`<strong>` fixture to confirm bold now visibly differs from normal weight,
where it previously didn't) before blessing anything — not a rubber stamp.

**tty `.txt` goldens should NOT change at all** — the tty backend stores raw `char`s in grid cells with no
glyph rasterization dependency (§ Current state), so this font swap is a strong, cheap regression anchor: if
any `.txt` golden differs after this packet, that is itself a signal something leaked across the seam that
shouldn't have, and needs root-causing before blessing anything (AGENTS.md rule 5).

### 5. Portability + optional system-font enhancement (future, not this packet)
The embedded 191-glyph × 5-size × 2-weight subset is the **portable baseline** — zero external font-file
dependency, works identically on the bare i486 floppy target, any other Linux, and (since it's pure Rust
`const` data with no OS font-loading API involved) any non-Linux host this codebase is ever built for.
**Explicitly out of scope, flagged as a clean future extension point:** loading fuller Terminus coverage
(the full 1356-glyph BDF range, or sizes this subset excludes like 14/18/22/28) from a system font path when
present — e.g. the note that "the Monolith now ships Terminus" is exactly the kind of environment where a
FUTURE packet could detect an on-disk Terminus BDF/PSF and prefer it over the embedded subset for extended
coverage, falling back to the embedded baseline when absent. The extension point is `TerminusFont`'s own
glyph-lookup boundary (`TerminusFont::glyph`) — a future implementation could consult an optional
system-loaded table before falling back to the embedded `const` tables, with no change to `Metrics` or any
call site. Not designed further here — sizing that spike is a separate packet's job.

### 6. Size budget
Byte cost per glyph-row, mixed encoding (§2): sizes 12/16 → 1 byte/row; sizes 20/24/32 → 2 bytes/row.
`Σ(rows × bytes/row)` across the 5 buckets = `12×1 + 16×1 + 20×2 + 24×2 + 32×2 = 180` bytes per glyph, summed
once per (size, weight) combination. Total: `191 glyphs × 2 weights × 180 bytes = 68,760 bytes ≈ 67.1 KiB` —
matches the task brief's "already computed" ~67 KB almost exactly (this derivation is reproducible: it's the
literal byte count of the `const` row data, before any Rust codegen/alignment overhead, which should be
negligible for flat byte-array data). Against the **187,236-byte** headroom cited in Current State: **≈118.5
KiB (≈121,476 bytes) of floppy headroom remains** after this packet, assuming no other packet has landed in
the interim and consumed part of it — the plan's final task re-measures this for real against the CI
artifact rather than trusting the projection.

## Testing / fixtures (see plan for full task-by-task breakdown)
- **Glyph-table correctness (Rust, TDD):** pin specific, hand-verified glyph bytes (e.g. `'A'` at 16px
  normal/bold, `'A'` at 32px) as `assert_eq!` against manually bit-reversed expected values computed
  directly from the BDF source in Current State — written FIRST (red, since `terminus_glyphs.rs` doesn't
  exist), made green by running the generator and committing its output.
- **Nearest-size selection (Rust, TDD):** every bucket value maps to itself; every midpoint (14/18/22/28)
  rounds up; below-12 and above-32 clamp; a handful of interior non-boundary values land on the expected
  bucket.
- **Per-size metrics (Rust, TDD):** `TerminusFont::{ascent,descent,line_height,advance}` at each of the 5
  buckets match the table in Current State exactly (not a scaled approximation).
- **Weight selection + `TextRun` wiring:** bold vs. normal produce visibly different glyph bitmaps for the
  same char/size; `paint_text` forwards `style.font_weight` correctly (unit test on the `TextRun` it builds,
  not just an eyeballed PNG).
- **Coverage-narrowing regression (translit):** a control char / DEL reaching `translit::resolve` degrades
  the same way any other genuinely unrepresentable char does today (transliterated if mapped, else dropped
  and counted) — not a panic, not silently different from the documented resolution order. Grep every
  fixture first to confirm none currently feeds a literal control char through a text run (expected: none —
  HTML whitespace/control handling strips these long before layout), and assert it as a stated, checked fact
  rather than an assumption.
- **Golden re-bless (its own integration task, Plan Task 6):** CI render sweep, pixel-spot-check a
  representative sample per §4 above, tty `.txt` goldens treated as a should-not-change regression anchor,
  then bless.

## Risks / unknowns not fully grounded here
- **Exact upstream download URL + SHA-256 for `terminus-font-4.49.1`** is not pinned in this document — the
  BDFs used for all measurements above are already staged in the session scratchpad, not fetched fresh by
  this design. Plan Task 1 must resolve and record a verifiable source (release tarball URL + hash) before
  the extraction tool is authored, not fabricate one.
- **Current `stele-i486` size** (1,287,324 bytes) is sourced from JOURNAL.md's latest entry, not a fresh CI
  measurement taken for this design — likely still accurate (no other packet in the branch history since
  looks size-relevant) but should be reconfirmed, not assumed, before this packet reports its own delta.
- **`h5`/`h6` visibly shrinking** (§4) is a predicted, derivable consequence of the nearest-size rule, not
  something rendered and eyeballed as part of this design — the plan's golden-rebless task must confirm it
  lands as predicted (smaller, still legible, not garbage) rather than assume the derivation is sufficient.
- **OFL condition 2's exact reach** (whether license text must additionally travel with the compiled
  `stele-i486` binary itself, vs. the source repository being sufficient) is a judgment call, not a legal
  ruling — flagged for the operator to confirm in the PR, consistent with how this repo has handled similar
  judgment calls before (e.g. the acid2-eyes packet's charter-amendment flag).

## Charter / decisions note
This is a **rendering-fidelity change to an already-adopted CSS property** (`font-weight`'s `bold` keyword
already parses/cascades per C2's existing dialect record — this packet makes it finally *paint* something
different, it does not add a new keyword, property, element, or URI scheme) plus an internal asset swap (the
glyph source Stele rasterizes from). **No `stele-charter.md` "What Stele Speaks" amendment is expected** —
flag this judgment in the PR description for the operator to confirm, per AGENTS.md rule 6's own guidance
not to assume silently. Record a new `DECISIONS.md` entry (next free letter after the current latest, D65 as
of this writing — reconfirm at implementation time) covering: (1) Terminus chosen over other embeddable
bitmap fonts and why (OFL-1.1, multi-size, real bold weight, vs. font8x8's single 8×8 source with no bold);
(2) the 191-glyph / 5-size / 2-weight subset boundary and its ≈67 KB cost; (3) the nearest-size snap
replacing the flat 16px floor, and the resulting `h5`/`h6`/oversized-heading behavior change; (4) font8x8's
removal (`glyphs.rs`, `bitmap.rs`) and the resulting Latin-1-only, no-control-char coverage narrowing; (5)
`font-weight: bold` wired to real rendering for the first time.
