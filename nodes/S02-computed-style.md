# S02 — computed-style

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | I03, S04, S06, S07, T01, T03 | S |

**Owns:** `src/style/computed.rs`

## Purpose

Imported from the code. `stele::style::computed` defines `ComputedStyle`, the curated set of computed
properties that the cascade (S06) produces, one per DOM node, and that layout and paint consume. It is the
frozen contract between style and layout: every CSS value is parsed upstream, and only these properties
survive the cascade with meaning.

- **The fields.** Text: `color`, `font_family` (`Serif`, `SansSerif`, `Monospace`), `font_size` (px),
  `font_weight` (`Normal`, `Bold`), `font_style` (`Normal`, `Italic`), `line_height` (`Normal` or px),
  `text_align` (left, right, center, justify), `text_decoration` (underline, line-through, overline flags),
  `white_space` (`Normal`, `Pre`), `vertical_align` (baseline, top, middle, bottom), `list_style_type` (disc,
  circle, square, decimal, lower-alpha, upper-alpha, none). Box: `display` (none, inline, block, flex, table,
  table-row, table-cell, table-row-group, list-item, grid), `width`/`height` and their min/max (`Dimension`:
  px, percent or auto), `margin` and `inset` (`Edges` of px, percent or auto), `padding` (`Edges` of px or
  percent), `border` (`Edges<BorderSide>`: width, `BorderStyle` `None`/`Solid`, color), `box_sizing`,
  `float`, `clear`, `position` (static, relative, absolute, fixed), `overflow` (`Visible`, `Hidden`),
  `z_index` (`Auto` or a layer), `content` (normal, none, a string, a URL), `background_color`,
  `background_image` (a URL). Tables: `border_spacing_x`/`_y` (px), `border_collapse`. Flex: direction, wrap,
  `justify_content`, `align_items`, `align_self`, `flex_grow`, `flex_shrink`, `flex_basis`. Grid: `gap`,
  `column_gap` (`None` means `gap` governs both axes), `grid_template_columns`/`_rows` (tracks of px,
  percent or `fr`, `minmax`, and `repeat` with a count, `auto-fill` or `auto-fit`).
- **The initial values** (`ComputedStyle::default()`) are CSS's: inline, black on transparent, serif 16px,
  normal line height, left-aligned, static, visible, content-box, zero margins and padding, no border, no
  image, auto sizes and insets, flex row/no-wrap/flex-start/stretch, grow 0, shrink 1. Two are Stele's own,
  matching the layout constants they replaced: `border_spacing_x` 8px and `border_spacing_y` 0.
- **Helpers.** `ZIndex::layer` gives the paint-order layer (`Auto` is 0). `Content::generates_box` is true
  only for a string (the empty string included) or a URL, so `content: normal|none` makes no
  `::before`/`::after` box. `Edges::all` fills four sides with one value. `BorderSide::default()` is width 0,
  style none, black.

## Tests

The inline module `style::computed::tests` in `src/style/computed.rs` (1 test: the initial values). The value
tests in `src/style/value.rs` cover `Content::generates_box` and `ZIndex::layer`; the cascade tests in
`src/style/cascade.rs` check the defaults a node gets when nothing is declared.

## Changes

none: imported

## Gate

- **S02-AC-1** — The initial values include display inline, color black, a transparent background with no image, flex-grow 0 and flex-shrink 1, a 0px top margin, border spacing 8px by 0, border-collapse separate, no distinct column gap, content normal and overflow visible. *Verify:* `cargo test --locked --jobs 2 --lib style::computed::tests::initial_values_match_css -- --test-threads=2` exits 0.
- **S02-AC-2** — `Content::generates_box` is true for a string (the empty string included) and a URL, false for `normal` and `none`. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::content_generates_box_only_for_str_and_url -- --test-threads=2` exits 0.
- **S02-AC-3** — `ZIndex::layer` is 0 for `Auto` and n for `Layer(n)`, negative layers included. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::z_index_layer_helper -- --test-threads=2` exits 0.
- **S02-AC-4** — The initial box-sizing is content-box. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::box_sizing_defaults_to_content_box_when_never_declared -- --test-threads=2` exits 0.
- **S02-AC-5** — The initial font size is 16px (the UA's `h1 { font-size: 2em }` computes to 32px). *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::ua_heading_font_size_is_relative_em_against_root -- --test-threads=2` exits 0.
- **S02-AC-6** — The initial text-align is left. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::img_align_does_not_set_text_align_stays_left_default -- --test-threads=2` exits 0.
- **S02-AC-7** — The initial border spacing reaches a table that sets none: 8px across, 0 down. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::table_with_no_cellspacing_or_css_keeps_the_default_border_spacing -- --test-threads=2` exits 0.
- **S02-AC-8** — `BorderSide::default()` has width 0: a box that declares no border has no border width. *Verify:* `cargo test --locked --jobs 2 --lib layout::find_fragment_top_tests::zero_border_target_resolves_to_the_border_box_top -- --test-threads=2` exits 0.
- **S02-AC-9** — The initial white-space is normal: text wraps at the line width. *Verify:* `cargo test --locked --jobs 2 --lib layout::inline::tests::multiple_wraps_with_exact_line_count_and_positions -- --test-threads=2` exits 0.
- **S02-AC-10** — The initial line height is normal. *Verify:* `cargo test --locked --jobs 2 --lib layout::inline::tests::single_line_fits -- --test-threads=2` exits 0.
- **S02-AC-11** — The initial align-items is stretch. *Verify:* `cargo test --locked --jobs 2 --test flex_polite_golden -- --test-threads=2` exits 0.
- **S02-AC-12** — The initial font family is serif. *Verify:* unpinned.
- **S02-AC-13** — The initial insets (`top`, `right`, `bottom`, `left`) are auto. *Verify:* unpinned.

## Risks

- `ComputedStyle` is a frozen contract with layout and paint: a new property is a field here, a raw field and
  parser arm in S04, a resolution line in S06 and a consumer in layout, and the dialect it adds is priced in
  the charter's C2 record (`SPEC.md` principle 6).
- Only `solid` borders exist; every other border style computes to `None`, so dashed, dotted and double
  borders are not drawn. `overflow` has no `scroll`/`auto` state: they compute to `Hidden`.
- Every node, text nodes included, carries a full clone of `ComputedStyle` (two `Vec`s and two optional
  strings among about fifty fields), so a large DOM costs that much memory on a 486.
