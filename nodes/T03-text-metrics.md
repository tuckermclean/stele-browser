# T03 — text-metrics

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | S02, T01, T02 | L01, L02, L04 | S |

**Owns:** `src/text/mod.rs`, `src/text/terminus.rs`

## Purpose

Imported from the code. The seam between the font and the inline layout: the `Metrics` trait, its only
implementer `TerminusFont` over T01's tables, and the size snap every caller applies before measuring or
drawing text. `src/text/mod.rs` is the module root: it declares `terminus`, the crate-private generated
`terminus_glyphs`, the test module `terminus_glyphs_tests` (by `#[path]`, T01) and `translit` (T02), and
re-exports `TerminusFont`.

- `Metrics`: `ascent`, `descent`, `line_height`, `advance(ch)` in pixels at a `size_px`, and `measure(s)`,
  the sum of the advances by default. `layout::inline` breaks lines with it.
- `nearest_terminus_size(px)` (`pub(crate)`) snaps to one of the five buckets 12, 16, 20, 24, 32: at or
  below 12 gives 12, at or above 32 gives 32, between two buckets the nearer one, an exact midpoint (14, 18,
  22, 28) the larger, and a non-finite size 16.
- `text_render_px(font_size)` is the public form of that snap. It is applied only to the size used to
  measure and draw a text run, never to `ComputedStyle::font_size` or the lengths resolved against it.
- `TerminusFont` does not scale: each `Metrics` method returns the snapped bucket's pinned values
  (ascent, descent, line height, advance): 12px 10/2/12/6, 16px 12/4/16/8, 20px 16/4/20/10, 24px
  19/5/24/12, 32px 26/6/32/16. It is monospace: `advance` ignores the character.
- `TerminusFont::glyph(ch, weight, size_px)` returns a `Glyph`: the bucket's cell size, its ascent, and
  the glyph's rows (T01's `lookup`) widened to `u16`, zero past `cell_h` up to 32 rows. A renderer puts
  the glyph's top at `baseline - ascent` (`surface::mem`).

## Tests

The inline modules `text::terminus::tests` in `src/text/terminus.rs` (11 tests: the snap, the metrics
table, the step function, totality, glyph widening) and `text::text_render_px_tests` in `src/text/mod.rs`
(5 tests: the same snap through `text_render_px`). `surface::mem`'s tests place glyphs with the ascent, and
`layout::inline`'s tests and the goldens use the metrics.

## Changes

none: imported

## Gate

- **T03-AC-1** — `nearest_terminus_size` maps each bucket size (12, 16, 20, 24, 32) to itself. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::exact_bucket_values_map_to_themselves -- --test-threads=2` exits 0.
- **T03-AC-2** — An exact midpoint (14, 18, 22, 28) snaps up to the larger bucket. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::exact_midpoints_round_up -- --test-threads=2` exits 0.
- **T03-AC-3** — A size between two buckets snaps to the nearer one (13 to 12, 15 to 16, 19 to 20, 30 to 32). *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::interior_non_midpoint_values_land_on_the_nearer_bucket -- --test-threads=2` exits 0.
- **T03-AC-4** — A size below 12 (0, 11.9) snaps to 12, and above 32 (32.1, 1000) to 32. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::below_min_and_above_max_clamp -- --test-threads=2` exits 0.
- **T03-AC-5** — NaN and ±infinity snap to 16. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::non_finite_input_degrades_to_a_defined_finite_bucket -- --test-threads=2` exits 0.
- **T03-AC-6** — `text_render_px` gives the same buckets: each bucket to itself, midpoints up, interior values to the nearer bucket, below 12 (including negative sizes) to 12, above 32 to 32, non-finite to 16. *Verify:* `cargo test --locked --jobs 2 --lib text::text_render_px_tests:: -- --test-threads=2` exits 0.
- **T03-AC-7** — At each bucket, `TerminusFont`'s ascent, descent, line height and advance are the pinned values (12px 10/2/12/6, 16px 12/4/16/8, 20px 16/4/20/10, 24px 19/5/24/12, 32px 26/6/32/16). *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::metrics_at_each_bucket_match_the_pinned_table_exactly -- --test-threads=2` exits 0.
- **T03-AC-8** — A size between buckets gets exactly the snapped bucket's metrics (18.5 gets the 20px values), not a scaled value. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::non_bucket_size_resolves_metrics_as_the_snapped_bucket -- --test-threads=2` exits 0.
- **T03-AC-9** — For any `size_px` (0, -1, NaN, ±infinity, `f32::MIN`, `f32::MAX`) every metric is finite and `glyph` returns a non-empty cell without a panic. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::totality_on_unusual_size_px_no_panic -- --test-threads=2` exits 0.
- **T03-AC-10** — `glyph` widens a 16px glyph's `u8` rows to `u16` unchanged and leaves the rows past `cell_h` zero. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::glyph_widens_narrow_rows_to_u16_uniformly -- --test-threads=2` exits 0.
- **T03-AC-11** — `glyph` keeps a 32px glyph's full `u16` row values. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::glyph_at_a_wide_bucket_keeps_the_full_u16_row_value -- --test-threads=2` exits 0.
- **T03-AC-12** — `glyph` does not panic for a character outside the subset (CJK, emoji, U+FFFD, U+10FFFF, U+0000), at either weight. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus::tests::glyph_never_panics_for_any_char -- --test-threads=2` exits 0.
- **T03-AC-13** — `glyph` carries the bucket's ascent, so a 16px glyph drawn by the memory surface has its top at `baseline - 12`. *Verify:* `cargo test --locked --jobs 2 --lib surface::mem::tests::draw_text_paints_a_native_size_glyph_bottom_aligned_to_the_baseline -- --test-threads=2` exits 0.
- **T03-AC-14** — `advance` is the same for every character at a size (monospace). *Verify:* unpinned.
- **T03-AC-15** — `measure` is the sum of the characters' advances. *Verify:* unpinned.

## Risks

- The snap is a step function: sizes between buckets render at the nearest bucket, so `h5`/`h6` (about
  13.3 and 10.7px) render at 12px, and anything over 32px at 32px. Box geometry keeps the unsnapped
  `font-size`, so text can be smaller or larger than the box sized for it.
- The pinned metrics in `METRICS` are copied from the BDF sources by hand; a font update (T01) must update
  them too.
- `bucket_of` falls back to the 16px bucket for a size that is not a bucket, and `lookup` (T01) has its
  own nearest match; both are reached only if the snap is bypassed.
- Monospace is assumed by the tty backend's cell grid; a proportional font would change line breaking
  and every golden.
