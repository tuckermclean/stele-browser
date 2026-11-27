# S03 — contrast-repair

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | S07 | S |

**Owns:** `src/style/contrast.rs`

## Purpose

Imported from the code. `stele::style::contrast` is the run-level rung of the contrast covenant: pure colour
arithmetic, total over any two 8-bit colours, with no layout knowledge. The pixel backend
(`backend::raster`) calls `repair_fg` on every text run against the run's effective background, and
`main.rs`'s `--audit-contrast` uses `contrast_ratio` and `CONTRAST_MIN` to check that every repaired run
reads. The box-level rung (a gradient painted as its first colour stop) is in S04.

- `relative_luminance(color)` is WCAG 2.x relative luminance: each sRGB channel linearised
  (`c / 12.92` up to 0.03928, else `((c + 0.055) / 1.055)^2.4`), weighted 0.2126 R + 0.7152 G + 0.0722 B. Alpha
  is ignored.
- `contrast_ratio(a, b)` is `(L_lighter + 0.05) / (L_darker + 0.05)`, the same whichever argument is
  lighter; black on white is 21.
- `CONTRAST_MIN` is 3.0, a deliberately loose "readable at all" floor (WCAG AA for text is 4.5), so that only
  text that is close to invisible is recoloured.
- `repair_fg(fg, effective_bg)` returns `fg` unchanged when it clears `CONTRAST_MIN` against the background,
  and otherwise whichever of black or white has the higher ratio (black on a tie), never a third colour. One
  of the two always clears the floor: the worst background still gives about 4.6.

## Tests

The inline module `style::contrast::tests` in `src/style/contrast.rs` (10 tests: repairs, unchanged colours,
the 21:1 ratio, order independence, a colour against itself, the luminance of black and white, and a sweep of
gray backgrounds). `src/main.rs`'s `--audit-contrast` tests and the backend's tests exercise it through
rendering.

## Changes

none: imported

## Gate

- **S03-AC-1** — The relative luminance of black is 0 and of white is 1. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::relative_luminance_of_black_is_zero_and_white_is_one -- --test-threads=2` exits 0.
- **S03-AC-2** — The contrast ratio of black and white is 21. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::black_white_contrast_ratio_is_the_canonical_21_to_1 -- --test-threads=2` exits 0.
- **S03-AC-3** — The contrast ratio does not depend on the order of its arguments. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::contrast_ratio_is_order_independent -- --test-threads=2` exits 0.
- **S03-AC-4** — The contrast ratio of a colour with itself is 1. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::contrast_ratio_of_a_color_with_itself_is_1 -- --test-threads=2` exits 0.
- **S03-AC-5** — A foreground that already clears the floor is returned unchanged, even when it is neither black nor white. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::a_compliant_foreground_is_returned_unchanged_even_if_not_black_or_white -- --test-threads=2` exits 0.
- **S03-AC-6** — White text on white is repaired to black, and black on black to white. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::contrast::tests::white_on_white_repairs_to_black style::contrast::tests::black_on_black_repairs_to_white` exits 0.
- **S03-AC-7** — On a mid-gray background an illegible foreground is repaired to black, the better of the two. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::mid_gray_background_picks_black_over_white -- --test-threads=2` exits 0.
- **S03-AC-8** — For every gray background, the repaired foreground clears `CONTRAST_MIN`. *Verify:* `cargo test --locked --jobs 2 --lib style::contrast::tests::repair_always_clears_the_floor_across_a_spread_of_gray_backgrounds -- --test-threads=2` exits 0.
- **S03-AC-9** — A channel value up to 0.03928 linearises as `c / 12.92`, above it by the 2.4 power curve. *Verify:* unpinned.

## Risks

- The floor is 3:1, looser than WCAG AA's 4.5:1 for text, so low-contrast but visible text is left as the
  page wrote it. Raising it would recolour more author-intended text.
- A repair is always black or white, so a coloured link or heading that fails the floor loses its hue.
- Alpha is ignored: a translucent foreground is judged as if opaque. The raster backend passes only opaque
  effective backgrounds.
- `backend::tty` has its own `contrast_ratio` and floor (`src/backend/tty.rs`); the two can drift.
