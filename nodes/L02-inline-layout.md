# L02 — inline-layout

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | L01, S02, I01, T03 | L04, L05 | M |

**Owns:** `src/layout/inline.rs`

## Purpose

Imported from the code. `stele::layout::inline` is the bespoke inline engine. `layout_runs(runs,
available_width, text_align, metrics)` breaks a flat list of `InlineRun`s into line boxes and places the
floated images among them. An `InlineRun` is one span of content with one `ComputedStyle` and an optional
`Interactive`. Its content is either `Text` or `Replaced { intrinsic, image }`. L04 flattens each run of
inline-level boxes into such a list and calls `layout_runs` from its measure function and when it emits
fragments.

The result is an `InlineLayout` with `size`, `lines` and `floats`. Each `LineBox` has a `rect`
relative to the inline container's content box, a `baseline` below the line's top, and `PositionedRun`s. A
`PositionedRun` has the index of its source run, its text, and its `x` and `width` relative to the line box.

- **Whitespace.** Under `white-space: normal`, any run of whitespace, across run boundaries too, collapses
  into one break opportunity, and a line keeps one space before a word that had one. Under `pre`, a newline is
  a hard break, interior spaces are kept, `\r` is dropped and a tab becomes one space.
- **Forced breaks.** `LINE_BREAK_SENTINEL` (U+E000), which L05 emits for `<br>`, ends the line whatever room is
  left. A bare break still gets one line of height, from its own run's style.
- **Breaking.** Words are measured with `metrics` at `text_render_px(font_size)`. Pieces with no whitespace
  between them, across runs too, form one cluster that never splits. A cluster wraps to a new line when it
  does not fit after the content already on the line. A cluster wider than the line overflows on a line of
  its own and is never split. A non-positive or non-finite available width counts as 0, so each cluster gets
  its own line.
- **Line metrics.** The line height is the larger of the tallest item's line height and the largest ascent
  plus the largest descent. An explicit `line-height` in px is used as authored, and `normal` uses the font's
  line height. The baseline sits at the ascent plus half the leftover leading.
- **Inline images.** A non-floated `Replaced` run is an atom: an unbreakable word of its intrinsic width, with
  no text, its ascent its height and its descent 0, so it sits on the baseline. The leading space is not part
  of its box. It glues to text that touches it with no whitespace.
- **Floats.** `float: left|right` `Replaced` runs leave the flow and are placed first, from the top of the
  inline formatting context at y = 0, whatever their source position. Same-side floats stack sideways and drop
  to a new row below the tallest float of the row when the next one does not fit. A float wider than the
  container is cut to its width. At most `MAX_FLOATS` (256) are placed. Each line tests the floats at its own
  top y: a left float moves the line's start right and shortens it, a right float only shortens it. Lines below
  the floats are full width. With floats and no other content there are no lines, and `size` covers the
  floats. `clear` has no effect here.
- **Alignment.** `text-align: center` shifts each line's runs by half the slack, and `right` by all of it,
  within that line's float-reduced width. `left` and `justify` do not shift. A shift is never negative.
- **Totality.** Every intrinsic size and measurement passes `clamp_dim` (non-finite or negative becomes 0, and
  the cap is 1,000,000 px). Empty or whitespace-only input gives no lines and a zero size.

## Tests

The inline module `layout::inline::tests` in `src/layout/inline.rs` (41 tests, most with a fixed 10px-advance
metric, the font-size ones with `TerminusFont`). Through the pipeline: `tests/layout_floats.rs` (`img
align=left` text wrap, inline images, many floats), `tests/kitchen_sink_golden.rs` (`<br>`), and the tty and
PNG goldens.

## Changes

none: imported

## Gate

- **L02-AC-1** — No runs give no lines and a zero size, and a whitespace-only run (spaces, tab, newline) gives no lines. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::empty_runs_produce_zero_lines layout::inline::tests::whitespace_only_run_produces_zero_lines` exits 0.
- **L02-AC-2** — Under `white-space: normal` a source newline collapses (`a    b\nc` is one line). Under `pre` the newline breaks the line and interior spaces are kept (`a    b`, then `c`). *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::white_space_pre_breaks_on_newline_and_preserves_interior_spaces` exits 0.
- **L02-AC-3** — Text that fits is one line with one positioned run at x 0. The baseline is at the ascent, the line is the font's line height high, and `size` is the text's width by one line. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::single_line_fits` exits 0.
- **L02-AC-4** — Text wraps at the word that does not fit, with lines stacked by line height. Content that exactly fills the width stays on the line (`aa bb` in 50px). *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::wraps_at_the_right_word layout::inline::tests::multiple_wraps_with_exact_line_count_and_positions` exits 0.
- **L02-AC-5** — A word wider than the line is never split: alone it is one overflowing line, and among other words it gets a line of its own. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::unbreakable_overflow_does_not_panic layout::inline::tests::overlong_word_among_others_gets_its_own_line` exits 0.
- **L02-AC-6** — Each source run's text is its own positioned run at its own x. Runs that touch with no whitespace (`bold` + `text`) stay together on one line even when they overflow. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::per_run_x_positions_across_multiple_source_runs layout::inline::tests::glued_cross_run_word_stays_on_one_line` exits 0.
- **L02-AC-7** — A negative, NaN, negative-infinite or zero available width counts as 0: each word gets its own line. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::negative_or_nan_available_width_does_not_panic` exits 0.
- **L02-AC-8** — An explicit px `line-height` sets the line height as authored and centres the ink in it (40px gives a baseline of 23 with an ascent of 8 and a descent of 2). A line is never shorter than its glyphs' ascent plus descent. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::explicit_line_height_overrides_metrics layout::inline::tests::explicit_css_line_height_generous_enough_to_dominate_stays_exactly_as_authored layout::inline::tests::explicit_css_line_height_tighter_than_the_snapped_ink_still_grows_to_fit_it` exits 0.
- **L02-AC-9** — A non-floated image sits on the line between words as a run with no text, its own intrinsic width, and x after the leading space. Text touching it with no whitespace stays on its line. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::non_floated_replaced_atom_sits_inline_between_words layout::inline::tests::glued_word_next_to_atom_with_no_whitespace_stays_together` exits 0.
- **L02-AC-10** — An inline image's bottom sits on the baseline: a 50px image on a text line gives a baseline of 50 and a line 52px high. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::tall_inline_atom_grows_the_line_box` exits 0.
- **L02-AC-11** — An image wider than the line overflows on its own line, and one that does not fit after text wraps to a new line at x 0. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::atom_wider_than_available_width_overflows_its_own_line_not_split layout::inline::tests::atom_wraps_to_its_own_line_when_it_does_not_fit_after_text` exits 0.
- **L02-AC-12** — NaN, infinite or negative intrinsic sizes of inline and floated images give finite sizes. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::atom_with_nonfinite_or_negative_intrinsic_does_not_panic layout::inline::tests::float_with_nonfinite_or_zero_intrinsic_does_not_panic_or_hang` exits 0.
- **L02-AC-13** — A left float sits at (0, 0). Lines beside it start at its right edge and are shortened, and the first line below it starts at x 0. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::left_float_offsets_and_shortens_overlapping_lines` exits 0.
- **L02-AC-14** — A right float sits at the container's right edge, and the lines beside it are shortened but still start at x 0. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::right_float_only_shortens_available_width_not_the_start_x` exits 0.
- **L02-AC-15** — A float with no other content is still placed, with no lines, and `size` is the float's footprint. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::floats_with_no_following_text_are_still_placed_and_sized` exits 0.
- **L02-AC-16** — Same-side floats stack sideways and wrap to a new row below the tallest float of the first row. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::multiple_same_side_floats_stack_then_wrap_to_a_new_row` exits 0.
- **L02-AC-17** — A float wider than the container is cut to the container's width. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::float_wider_than_container_clamps_to_full_width` exits 0.
- **L02-AC-18** — Of 2,000 floats, at most `MAX_FLOATS` are placed. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::many_floats_are_bounded_and_do_not_hang` exits 0.
- **L02-AC-19** — `center` shifts a line by half its slack and `right` by all of it, moving every run on the line by the same amount and each wrapped line by its own slack. `left` does not shift. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::text_align_center_offsets_the_line_by_half_the_slack layout::inline::tests::text_align_right_offsets_the_line_by_the_full_slack layout::inline::tests::text_align_left_is_unchanged_offset_zero layout::inline::tests::text_align_center_multi_run_shifts_every_fragment_on_the_line layout::inline::tests::text_align_center_each_wrapped_line_centers_independently` exits 0.
- **L02-AC-20** — The alignment shift is never negative on an overflowing line, and it is measured within the width a float leaves. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::inline::tests::text_align_offset_never_goes_negative_when_line_overflows_avail layout::inline::tests::text_align_center_measures_within_the_float_reduced_available_width` exits 0.
- **L02-AC-21** — `LINE_BREAK_SENTINEL` forces a line break: the kitchen-sink fixture's two sentences around its `<br>` render on separate lines. *Verify:* `cargo test --locked --jobs 2 --test kitchen_sink_golden -- --exact --test-threads=2 br_forces_a_real_line_break_in_the_kitchen_sink_render` exits 0.
- **L02-AC-22** — Text is measured at `text_render_px(font_size)`. A bare `<br>` line takes its height from its own run's style. Under `pre`, `\r` is dropped and a tab is one space. `justify` does not shift. `clear` has no effect. Floats start at y = 0 whatever their source position, and a left and a right float are not checked against each other. *Verify:* unpinned (`TerminusFont` snaps the size again itself, so the font-size tests pass without this call).

## Risks

- The module doc says `white-space: pre` "is not yet honored", but `tokenize` honours it; the doc is stale.
- Floats always start at the top of their inline formatting context. A mid-paragraph `<img align=left>`
  floats above the text that precedes it, and a float never continues into the next block.
- A line's float exclusion is tested only at the line's top y, so a line taller than the gap below a float can
  overlap it.
- Left and right floats are placed independently and can overlap in a narrow container.
- `LINE_BREAK_SENTINEL` is a private-use character, so a document whose text contains U+E000 gets a forced
  break there.
- The size snap is applied twice, here through `text_render_px` and again inside `TerminusFont`, so the two can
  drift apart without a test noticing.
