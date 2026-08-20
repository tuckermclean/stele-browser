# Terminus font Plan · Spec: docs/superpowers/specs/2026-08-21-terminus-font-design.md (read it first)

**Goal:** replace the compiled-in `font8x8` bitmap atlas with an embedded, portable Terminus (OFL-1.1) subset
— 191 glyphs (ASCII 0x20-0x7E + Latin-1 0xA0-0xFF) × 5 sizes (12/16/20/24/32) × 2 weights (normal/bold) — as
Stele's sole rendering font, everywhere. Default CSS text size (16px) maps metrics-identically onto the new
font (only glyph *shape* changes there); every other size bucket is a real, owned layout change (spec §4).

**Architecture (one sentence per moving part, see spec for the why):** a new, checked-in extraction tool
(`tools/gen-terminus-glyphs.py`) turns the 10 source BDF files into a committed, generated Rust data module
(`src/text/terminus_glyphs.rs`); a new hand-written `TerminusFont` (`src/text/terminus.rs`) implements the
frozen `Metrics` trait by snapping any requested `size_px` to the nearest of the 5 embedded buckets (spec
§2's tie-break rule) and returning that bucket's real ascent/descent/advance/line-height; `text::text_render_px`
(`src/text/mod.rs`) is repurposed from a flat floor-at-16 into a thin wrapper around the same nearest-size
snap, keeping its name/signature so every existing layout call site needs zero changes; the three production
font-construction call sites (`chrome.rs`, `layout/mod.rs`, `surface/mem.rs`) swap `BitmapFont::vga_8x16()`
for `TerminusFont::new()`; `TextRun` gains a weight field so `font-weight: bold` (cascaded today, rendered
nowhere) finally reaches the rasterizer; `font8x8` (`glyphs.rs`, `bitmap.rs`) is deleted once nothing
production references it; the golden-rebless sweep is its own AGENTS.md-rule-4 integration task, done last.

**Global constraints (every task):** no new crate dependency (pure-Rust generated `const` data + std); report
the `stele-i486` size delta in the PR against the ≈67 KB projection (spec §6); **no local `cargo build`/
`cargo test`** — push and read `m0-acceptance`; every task starts with a failing test (visible red→green in
commit history); pixel-verify (never eyeball) any golden this plan touches, per AGENTS.md rule 4; totality —
no panic on any `char`/`size_px` input, mirroring the existing `BitmapFont`/`glyphs::lookup` contracts this
packet replaces.

**Task ordering / parallelism note:** Tasks 1-2 (glyph data, then metrics) are sequential — Task 2's tests
need Task 1's generated table to exist. Task 3 (rasterization + `TextRun` weight) depends on Task 2's
`TerminusFont` type. Task 4 (call-site swap + `text_render_px`) depends on Task 3. Task 5 (drop font8x8) must
land AFTER Task 4, once nothing production still calls `BitmapFont`/`glyphs`. Task 6 (golden rebless) depends
on ALL of Tasks 1-5 landing first — do not attempt it early. Task 7 (docs/licensing/decisions) can be drafted
in parallel with Tasks 3-5 but its PR-description size-delta line needs Task 6's real CI number, so land it
last. **This is a mostly-sequential packet, unlike acid2-eyes' two independent fixes** — no meaningful
cross-worktree parallelism opportunity; flag this explicitly rather than forcing parallel tracks that don't
exist.

---

### Task 1 — extraction tool + generated glyph tables

**Files:** `tools/gen-terminus-glyphs.py` (new), `src/text/terminus_glyphs.rs` (new, GENERATED — header
comment says so, points back at the tool), `third_party/terminus-font/OFL.TXT` (new, verbatim copy),
`third_party/terminus-font/NOTICE.md` (new, short attribution note).

**Interfaces:** the generated file exposes, per (size, weight) pair, two `const` tables (ASCII block,
Latin-1 block) of packed glyph rows, plus a tiny lookup shim, e.g.:
```rust
pub const TABLES: [[SizeWeightTable; 2]; 5] = [ /* [size][weight] */ ... ];
pub struct SizeWeightTable { pub cell_w: u8, pub cell_h: u8, pub ascii: &'static [GlyphRows], pub latin1: &'static [GlyphRows] }
pub enum GlyphRows { Narrow(&'static [u8]), Wide(&'static [u16]) } // sizes 12/16 vs 20/24/32, spec §2
```
(exact shape is an implementation detail for whoever authors the tool — the constraint is: byte-tight mixed
row width per spec §2/§6, and a lookup path from `(size_bucket, weight, char)` to one glyph's rows.)

**Failing-test-first steps:**
1. Before writing the tool: resolve and record the real upstream source for `terminus-font-4.49.1` (release
   tarball URL + a computed SHA-256) in the tool's own top-of-file doc comment — do not reuse the ephemeral
   scratchpad path this design was grounded against (spec's own Risks section flags this as unresolved).
2. Add a Rust test module (in `terminus_glyphs.rs` itself, or a sibling `#[cfg(test)]` file) that pins down
   a small, HAND-COMPUTED set of expected glyph rows before the generator exists to produce them: bit-reverse
   `ter-u16n.bdf`'s `'A'` bytes (`00 00 3C 42 42 42 42 7E 42 42 42 42 00 00 00 00`, MSB-first per the spec's
   Current State) into the project's LSB-leftmost convention by hand, assert
   `terminus_glyphs::lookup(16.0, FontWeight::Normal, 'A').rows == <hand-computed rows>`. Do the same for
   `ter-u16b.bdf`'s `'A'` (bold, distinct bytes) and `ter-u32n.bdf`'s `'A'` (a wide, `u16`-row bucket, proving
   the mixed-width packing round-trips correctly). **Red** — the module/function doesn't exist yet.
3. Test: a glyph OUTSIDE the 191-glyph subset (e.g. `'日'`) returns the documented fallback box, not a panic
   — total-over-all-`char` contract carried forward from `glyphs::lookup`.
4. Test: every one of the 191×5×2 = 1,910 table entries is present and non-empty (a structural completeness
   check — catches an off-by-one in the extraction range before it ships silently).
5. Author `tools/gen-terminus-glyphs.py`: parse each BDF's `STARTCHAR`/`ENCODING`/`DWIDTH`/`BBX`/`BITMAP`
   blocks, filter to `ENCODING` in `0x20..=0x7E` or `0xA0..=0xFF`, bit-reverse each row's byte(s) (spec's
   MSB→LSB normalization), emit the mixed-width `const` tables into `src/text/terminus_glyphs.rs` with a
   GENERATED-file header (tool name, source tarball URL+hash, regeneration command, explicit note that hand
   edits will be overwritten).
6. Run the tool against the verified BDF source, commit the generated file. Green (CI, not local).
7. Add `third_party/terminus-font/OFL.TXT` (verbatim) and a short `NOTICE.md` (attribution text per spec §1
   — "191-glyph subset... embedded under OFL-1.1", no Reserved-Font-Name misuse).

**Commit:** `feat(text): embed a 191-glyph Terminus (OFL-1.1) subset via a generated glyph table`

---

### Task 2 — `TerminusFont`: nearest-size snap + per-size `Metrics`

**Files:** `src/text/terminus.rs` (new), `src/text/mod.rs` (export the new module alongside `bitmap`).

**Interfaces:** `pub struct TerminusFont;` (stateless — all data lives in the Task 1 `const` tables),
`impl Metrics for TerminusFont` (all 4 required methods + inherited `measure`), plus a `pub(crate) fn
nearest_terminus_size(px: f32) -> f32` — the SINGLE canonical implementation of spec §2's snap rule, reused
by both this module and Task 4's `text_render_px`.

**Failing-test-first steps:**
1. Test `nearest_terminus_size`: each of `[12.0, 16.0, 20.0, 24.0, 32.0]` maps to itself. **Red** (function
   doesn't exist).
2. Test: the four exact midpoints `14.0, 18.0, 22.0, 28.0` each round UP (`14→16`, `18→20`, `22→24`,
   `28→32`).
3. Test: interior non-midpoint values land on the nearer bucket (e.g. `13.0→12.0`, `15.0→16.0`, `19.0→20.0`,
   `30.0→32.0`).
4. Test: below-min and above-max clamp (`0.0→12.0`, `11.9→12.0`, `32.1→32.0`, `1000.0→32.0`).
5. Test: non-finite input (`NaN`, `±INFINITY`) degrades to a defined, finite bucket (mirror
   `text_render_px`'s existing totality test, `mod.rs:109-116` — same discipline, new function).
6. Test: `TerminusFont.ascent/descent/line_height/advance` at each of the 5 buckets match the spec's table
   EXACTLY (`12→10/2/12/6`, `16→12/4/16/8`, `20→16/4/20/10`, `24→19/5/24/12`, `32→26/6/32/16`) — pin the
   numbers, not a formula, since these are real per-size BDF values, not a computed scale.
7. Test: a non-bucket `size_px` (e.g. `18.5`) resolves metrics as if it were the snapped bucket (`20.0`'s
   values), proving the step-function (not continuous-scaling) contract spec §2 documents as a deliberate
   divergence from `BitmapFont`.
8. Test: totality — every method stays finite for `size_px` in `{0.0, -1.0, NaN, INFINITY, NEG_INFINITY,
   f32::MIN, f32::MAX}` (mirror `bitmap.rs`'s own `totality_on_unusual_size_px_no_panic`, same discipline).
9. Implement `TerminusFont` + `nearest_terminus_size` against the Task 1 tables. Green.

**Commit:** `feat(text): TerminusFont — nearest-size Metrics over the embedded Terminus subset`

---

### Task 3 — rasterization + `font-weight: bold` wiring

**Files:** `src/surface/mod.rs` (`TextRun`, `mod.rs:49-55`), `src/surface/mem.rs` (`draw_text`/`draw_glyph`,
`mem.rs:158-287`), `src/backend/raster.rs` (`paint_text`, `raster.rs:453-475`).

**Interfaces:** `TextRun<'a>` gains `pub weight: FontWeight` (reuse `style::computed::FontWeight` — already
`Copy`/`Eq`, no new type needed). `paint_text`'s signature is unchanged (`style: &ComputedStyle` is already a
parameter; it just starts reading `style.font_weight`). `MemSurface::draw_text`/`draw_glyph` are
generalized from the fixed `GLYPH_W`/`GLYPH_H = 8` constants (`mem.rs:198-199`) to per-bucket cell dimensions
read off `TerminusFont`/the glyph it looked up.

**Failing-test-first steps:**
1. Test (`raster.rs`): `paint_text` with a bold-weight `ComputedStyle` builds a `TextRun` whose `weight ==
   FontWeight::Bold`; a normal-weight style builds `weight == FontWeight::Normal`. **Red** — `TextRun` has no
   `weight` field yet, this doesn't compile.
2. Test (`mem.rs`): `draw_text` painting the same char/size at `FontWeight::Normal` vs. `FontWeight::Bold`
   produces DIFFERENT lit-pixel sets (assert at least one pixel differs) — proves bold actually reaches the
   rasterizer, not just the `TextRun` struct.
3. Test (`mem.rs`): a 16px run paints the exact glyph shape from Task 1's hand-verified `'A'` table (replaces
   `mem.rs`'s existing `draw_text_paints_a_native_size_glyph_bottom_aligned_to_the_baseline`-style test,
   which currently pins font8x8's `'A'` bytes — this test's expected bitmap changes to Terminus's; that's
   expected, not a regression, since the glyph SOURCE changed by design).
4. Test (`mem.rs`): a 32px run paints at the WIDE (`u16`-row) bucket correctly — no column truncation/
   misalignment vs. the 16px (`u8`-row) bucket's narrower storage (a real risk given the mixed-width packing,
   spec §2 — this is the concrete regression test for that packing decision).
5. Regression test (`mem.rs`): degenerate/non-finite `size_px`, empty string, zero-alpha color, and
   off-surface placement stay no-ops / don't panic — re-run/adapt `mem.rs`'s existing totality tests
   (`draw_text_degenerate_size_px_is_a_no_op_not_a_panic`, `draw_text_off_surface_bounds_never_panics`,
   `draw_text_huge_size_px_is_bounded_not_a_hang_or_panic`, `draw_text_zero_alpha_color_is_a_no_op`) against
   the new implementation — these contracts must survive the swap unchanged.
6. Implement: add `weight` to `TextRun`; `paint_text` forwards `style.font_weight`; `MemSurface::draw_text`
   calls `TerminusFont::new()` + the snapped size instead of `BitmapFont::vga_8x16()`, looks up
   `(size, weight, ch)` per char, and `draw_glyph` copies/places the looked-up bucket's own cell dimensions
   (no more `GLYPH_W`/`GLYPH_H` constants — the bounds now come from the glyph itself; the off-canvas
   early-out and `MAX_GLYPH_PX` hostile-input cap from `mem.rs:210,249-270` are preserved, just parameterized
   by the actual (bucket) cell size instead of a hardcoded 8×8).
7. Green.

**Commit:** `feat(raster): rasterize Terminus glyphs; wire font-weight: bold into TextRun`

---

### Task 4 — swap production call sites + repurpose `text_render_px`

**Files:** `src/backend/chrome.rs:203,225`, `src/layout/mod.rs:193,207`, `src/text/mod.rs`
(`text_render_px`, `mod.rs:83-89` + its doc comment `mod.rs:46-78`).

**Interfaces:** no new public signatures — `text_render_px(font_size: f32) -> f32` keeps its exact name and
type; its body changes from `font_size.max(16.0)` to `nearest_terminus_size(font_size)` (delegating to Task
2's function, non-finite handling preserved). `chrome.rs`/`layout/mod.rs` swap their `BitmapFont::vga_8x16()`
constructions for `TerminusFont::new()` (or equivalent) — both already consume the value through the generic
`Metrics` trait, so no other code at either call site changes.

**Failing-test-first steps:**
1. Test (`mod.rs`): REPLACE `text_render_px`'s existing three tests (`floors_sub_16px_sizes_to_16`,
   `leaves_16px_and_above_untouched`, `non_finite_and_negative_inputs_stay_total`) — these assert the OLD
   flat-floor contract and will be WRONG (not just red, actively asserting the wrong thing) under the new
   behavior. Write new versions asserting the nearest-of-5 snap instead (reuse Task 2's exact assertions —
   `text_render_px` and `nearest_terminus_size` must now agree on every input, so this is largely the same
   test list ported to the public wrapper). State explicitly in the commit message that this is a deliberate
   contract change, per the function's own long-standing "revisit-trigger" doc comment (`mod.rs:74-78`), not
   an oversight.
2. Test (`chrome.rs`): chrome-bar label metrics at `TEXT_SIZE_PX` (`chrome.rs:203-230`'s constant) resolve
   through `TerminusFont` and produce the expected ascent/line-height for that size's bucket (adapt whatever
   existing chrome text-metrics test covers this, if one exists — grep `chrome.rs`'s test module first).
3. Test (`layout/mod.rs`): the metrics instance `layout::layout` hands to `layout_tree` is a `TerminusFont`,
   not a `BitmapFont` — a compile-level proof via a type-asserting test or a behavioral one (a document with
   an h5/h6 heading now measures at the 12px bucket's advance, not the old floored-to-16 one — ties this
   task directly to the spec §4 reflow prediction).
4. Implement the three call-site swaps + `text_render_px`'s new body/doc comment (update the doc to describe
   the nearest-of-5 snap and point at `TerminusFont`, replacing the now-resolved "revisit-trigger" language).
5. Green.

**Commit:** `feat(text): wire TerminusFont into chrome/layout; text_render_px now snaps to 5 sizes`

---

### Task 5 — drop font8x8; narrow `translit`'s coverage check

**Files:** `src/text/glyphs.rs` (delete), `src/text/bitmap.rs` (delete), `src/text/mod.rs` (remove both
`pub mod`/`pub use` lines), `src/text/translit.rs` (its `has_glyph`-equivalent step 1 check, per that
module's own resolution-order doc comment).

**Interfaces:** `translit::resolve`'s public signature is unchanged; only its internal "does the atlas have a
real glyph" predicate changes source (from `glyphs::has_glyph`, now-deleted, to a direct range check against
the Terminus subset's coverage: `(0x20..=0x7E).contains(&(ch as u32)) || (0xA0..=0xFF).contains(&(ch as
u32))`).

**Failing-test-first steps:**
1. Confirm (grep, not assumed) that nothing OUTSIDE `glyphs.rs`/`bitmap.rs` themselves references
   `BitmapFont`/`glyphs::` in non-test code after Tasks 3-4 land — if anything still does, that's a sequencing
   bug in this plan, stop and fix the ordering before deleting.
2. Test (`translit.rs`): a control char (e.g. `'\u{1}'`) and DEL (`'\u{7F}'`) — previously "atlas has a real
   glyph" under font8x8's full `0x00-0x7F` coverage, now OUTSIDE the narrower Terminus subset — resolve via
   `translit`'s step 2/3 (transliterated if mapped, else dropped-and-counted) instead of rendering a literal
   blank glyph as before. Assert the new, narrower behavior explicitly (this is the coverage-narrowing
   regression the spec's Risks section calls out) — not a silent behavior change nobody wrote down.
3. Test: grep every fixture under `fixtures/` for a literal control character in text content (expected:
   none — HTML whitespace/control normalization strips these upstream of layout) — if this finds one, that
   fixture's golden needs specific attention in Task 6, don't assume it's unaffected.
4. Regression test: the FULL existing ASCII/Latin-1 `translit`/`has_glyph`-adjacent test suite still passes
   with the new coverage predicate (every printable ASCII + Latin-1 char that had a real glyph before still
   does).
5. Implement: delete `glyphs.rs`/`bitmap.rs`, update `text/mod.rs`'s module declarations and its own doc
   comment (`mod.rs:1-17`, which currently narrates font8x8's presence — update it to describe Terminus
   instead), update `translit.rs`'s coverage predicate and its resolution-order doc comment (which currently
   cites `glyphs::has_glyph` by name).
6. Green.

**Commit:** `chore(text): remove font8x8 (superseded by Terminus); narrow translit's coverage check`

---

### Task 6 — golden re-bless sweep (integration task, its own AGENTS.md rule-4 discipline)

**Files:** every `goldens/*.png` and `goldens/*.txt` containing text (a large majority of the 36 PNG + 23
`.txt` goldens — spec §4), `DECISIONS.md`, `JOURNAL.md`.

**Steps (integration/golden work — the "test" is the CI render + pixel measurement; do NOT fold into Tasks
1-5's commits):**
1. Push the branch with Tasks 1-5 landed; let `m0-acceptance` build and upload the `stele-host`/`renders`
   artifacts.
2. **Before touching any `.txt` golden:** diff every rendered `.txt` output against its current committed
   golden. Per spec §4, these are predicted to be BYTE-IDENTICAL (tty stores raw chars, no glyph
   rasterization dependency). Any difference here is a signal something leaked across the tty/raster seam
   that shouldn't have — stop and root-cause (AGENTS.md rule 5) before proceeding to the PNG goldens at all.
3. **Before touching any `.png` golden:** download the artifact and pixel-measure a representative sample,
   not the full 36:
   - A default-body-text fixture (16px, e.g. `fixtures/basic.html`) — confirm line positions/wrapping are
     UNCHANGED from the current golden (spec §4's "zero metrics change at the default size" claim) and only
     glyph pixel content differs (a connected-component or per-glyph-cell diff, not "looks about the same").
   - A heading-heavy fixture exercising h5/h6 (or any small `font-size`) — confirm the predicted shrink to
     the 12px bucket actually happened, is legible, and isn't garbage/misaligned (spec §4's stated,
     unverified-until-now prediction).
   - A `<b>`/`<strong>`-bearing fixture — confirm bold glyphs are now visibly heavier/distinct from normal
     weight in the SAME fixture (previously identical, per Current State's dead-cascade finding).
   - `fixtures/kitchen-sink.html` (or whichever golden most broadly exercises text) as a general sanity pass.
4. Re-bless the full set only after step 3's sample passes — for the REMAINING goldens (not individually
   sampled), spot-check at minimum that none show blank/missing text regions or an obvious crash-shaped
   artifact (e.g. all-white or all-black render) before blessing in bulk; if anything looks off, pull it out
   and pixel-measure it individually rather than blessing past it.
5. If any render doesn't match prediction (wrong shrink direction, garbage glyphs, misaligned bold), **stop
   and re-diagnose** — do not bless a render that doesn't match the diagnosed change (AGENTS.md rule 4).
6. Bless the full re-bless set; the PR description states exactly what changed (glyph source everywhere;
   h5/h6 and any sub-14px text shrinking; bold now visible; anything above 32px clamping down) and cites this
   task's specific pixel measurements, not "CI is green."
7. Update `DECISIONS.md` (new entry, next free letter — reconfirm against the current file, D65 was latest
   as of the design doc) per spec's Charter/decisions note (5 points: font choice, subset/size cost, snap-
   rule behavior change, font8x8 removal + coverage narrowing, bold wiring). Append `JOURNAL.md` on finishing.
   Flag the "no charter amendment" and "OFL condition 2 reach" judgment calls in the PR description for the
   operator to confirm (spec's own Risks section).

**Commit(s):** `fix(golden): re-bless all text-bearing goldens for the Terminus font swap` (one commit if the
diff is reviewable as a unit; split by fixture group if the reviewer would rather see it in chunks — reviewer
's call, not a hard requirement here).

---

### Task 7 — licensing docs + size-delta report

**Files:** `REPORT.md` (Dependencies & licenses table, `REPORT.md:100-109`; License section,
`REPORT.md:162-174`), `LICENSE`-adjacent notes if any exist beyond `REPORT.md`.

**Steps:**
1. `REPORT.md`'s license table: remove the two `font8x8_basic`/`font8x8_ext_latin` rows (`REPORT.md:108-
   109`), add one row: `**Terminus (191-glyph subset)** (embedded, \`src/text/terminus_glyphs.rs\`) | bitmap
   font | **OFL-1.1** (Dimitar Toshkov Zhekov; see \`third_party/terminus-font/OFL.TXT\`)`.
2. `REPORT.md`'s closing License section (`REPORT.md:162-174`) and its final italicized "Fonts:" line
   (`REPORT.md:172-174`): update from "font8x8_basic (Public Domain)" to the Terminus attribution, and add
   one sentence on OFL-1.1/GPL-3.0-or-later combined-work compatibility (OFL doesn't impose terms
   incompatible with distributing the combined work under GPL-3.0-or-later — common, established practice for
   GPL projects embedding OFL fonts) — a stated judgment, not asserted as a legal conclusion; flagged
   alongside Task 6's other flagged judgment calls.
3. Report the ACTUAL CI-measured `stele-i486` size delta (from Task 6's artifact) against both the ≈67 KB
   projection (spec §6) and the 1,474,560-byte floppy ceiling, in the PR description.

**Commit:** `docs: update REPORT.md licensing for the Terminus font swap`

---

## Verify (whole plan, before opening the PR)
- `cargo test` green in CI (not locally) across all seven tasks' new/updated tests.
- `./accept.sh` green in CI, both host and i486 (`m0-acceptance`).
- Every re-blessed/new golden pixel-measured per AGENTS.md rule 4 — tty `.txt` goldens confirmed byte-
  identical (a should-not-change regression anchor); PNG goldens confirmed correct by the Task 6 sample, not
  bulk-blessed blind.
- `stele-i486` binary size delta reported against the ≈67 KB projection and the 1,474,560-byte floppy
  ceiling — confirm actual headroom remaining, don't just repeat the design's projection.
- `DECISIONS.md` (new entry) + `JOURNAL.md` updated; the "no charter amendment" and "OFL condition 2 reach"
  judgment calls both flagged explicitly in the PR description for the operator.
- Confirm explicitly in the PR description: system-font-path loading (spec §5), italic/oblique wiring, and
  tty SGR bold escapes all remain OUT of scope (future extensions, not this packet) — this packet's bar is
  "the embedded portable baseline renders everywhere, correctly, inside budget," not full Terminus coverage
  or full font-style fidelity.
