# S04 — css-values

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, S01, S02 | S05, S06, S07 | L |

**Owns:** `src/style/value.rs`

## Purpose

Imported from the code. `style::value` (crate-private) is the curated property and value layer: it turns one
declaration's tokens (S01) into raw, unresolved fields of a `Declarations` accumulator, which the cascade (S06)
folds and resolves into a `ComputedStyle` (S02). Every function here is total: a value that does not parse
simply does not apply, and the caller counts it as ignored.

- **`Declarations`** holds one `Option` per curated property, still in raw units (`px`, `pt`, `em`, `%`), plus
  the raw `custom` properties (`--name`) and the `deferred` declarations whose value holds a `var()`.
  `overlay` copies every field the other block set over this one, so the later, higher-precedence block wins
  field by field; custom and deferred entries are replaced by name.
- **`apply_property(name, tokens, d) -> bool`** parses one declaration of a lowercase property name and
  returns whether it applied. Most properties read only their first token. The curated set: `color`,
  `background`, `background-color`, `background-image`, `font`, `font-family`, `font-size`, `font-weight`,
  `font-style`, `line-height`, `text-align`, `text-decoration`, `white-space`, `vertical-align`,
  `list-style-type`, `display`, `width`, `height`, `min-`/`max-width`/`height`, `margin` and its four longhands,
  `top`/`right`/`bottom`/`left`, `padding` and its longhands, `border`, `border-top`, `border-width`,
  `border-style`, `border-color`, `float`, `clear`, `position`, `overflow`(`-x`/`-y`), `z-index`, `content`,
  `box-sizing`, `flex-direction`, `flex-wrap`, `justify-content`, `align-items`, `align-self`, `flex-grow`,
  `flex-shrink`, `flex-basis`, `border-spacing`, `border-collapse`, `gap`, `grid-template-columns` and
  `grid-template-rows`. Anything else returns false.
- **Values.** Colours: the 16 HTML names plus `grey`, `orange`, `pink`, `brown`, `cyan`, `magenta` and
  `transparent`, `#rgb`, `#rrggbb`, and `rgb()`/`rgba()` of numbers or percentages, clamped (alpha is
  dropped). Lengths: `px`, `pt`, `em`, `%` and a unitless 0, units in any case; `gap` also reads `rem` as `em`.
  Keywords are matched case-insensitively. A shorthand (`margin`, `padding`, `border-width`) takes 1 to 4
  values in CSS order and is rejected whole when any token is not a value; `border`/`border-top` take width,
  style and colour in any order. Only `solid` is a drawn border style: `none`, `dashed`, `dotted` and the rest
  map to no border. `font-family` maps to one of three faces. The `font` shorthand needs a size and reads
  `[style] [weight] size[/line-height] family`. `background` takes the first colour and the first `url()`
  anywhere in its value; a gradient (`linear-`, `radial-`, `conic-`, with `-webkit-`/`-moz-` prefixes) paints
  as its first colour stop (the box-level rung of the contrast covenant, S03). An unquoted `url()` is rebuilt
  from its tokens, so a `data:` URL with `;` inside survives.
- **`var()`.** `Env` is the inherited custom-property environment; `substitute_vars` replaces every `var(--x)`
  with its value, or its fallback, recursively. An undefined property with no fallback, a cycle, or a nesting
  deeper than 64 makes the whole value invalid (`None`).
- **Presentational hints.** `presentational_hints(tag, attrs)` turns the vintage HTML attributes into a
  `Declarations` block, which the cascade folds between the UA sheet and author CSS: `bgcolor` on any element;
  `color` and `size` on `font` (sizes 1 to 7 are 10, 13, 16, 18, 24, 32 and 48px, `+n`/`-n` steps from 3,
  clamped); `text` on `body`; `align` (left, right, center, justify) on any element but `img`; and on `table`,
  `cellspacing` (both spacing axes) and `border` without `cellspacing` (collapse).

## Tests

The inline module `style::value::tests` in `src/style/value.rs` (119 tests: colours, font families, the
shorthands, every keyword property with a test, borders, spacing and gap, display, backgrounds and URLs, the
font shorthand, `overlay`, presentational hints, `var()` substitution, gradients and grid templates). The
parser tests in `src/style/parser.rs` and the cascade tests in `src/style/cascade.rs` run declarations through
it, and the golden tests (`tests/*_golden.rs`, `./accept.sh --tty-only`) render pages that use it.

## Changes

none: imported

## Gate

- **S04-AC-1** — `red`, `#f00`, `#ff0000`, `rgb(1, 2, 3)` and `transparent` parse to their colours, and an unknown name parses to none. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::parses_named_hex_and_rgb_colors -- --test-threads=2` exits 0.
- **S04-AC-2** — A `color` value that does not parse is not applied. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::unparseable_color_is_not_applied -- --test-threads=2` exits 0.
- **S04-AC-3** — `font-family` maps `monospace` and `Courier New, monospace` to monospace, `Arial, sans-serif` to sans-serif and `"Times New Roman", serif` to serif; an empty list maps to nothing. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::classifies_generic_and_named_font_families -- --test-threads=2` exits 0.
- **S04-AC-4** — `margin` with one value sets all four sides, with two sets top/bottom then right/left, and with four sets top, right, bottom, left. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::margin_shorthand_expands_1_2_3_4_values -- --test-threads=2` exits 0.
- **S04-AC-5** — `margin: auto` sets the sides to auto. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::margin_auto_is_recognized -- --test-threads=2` exits 0.
- **S04-AC-6** — A `margin` or `padding` shorthand with any token that is not a length is rejected whole: no side is set. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::margin_shorthand_rejects_the_whole_declaration_if_any_token_is_unrecognized style::value::tests::padding_shorthand_rejects_the_whole_declaration_if_any_token_is_unrecognized` exits 0.
- **S04-AC-7** — `position` takes `static`, `relative`, `absolute` and `fixed`. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::apply_property_parses_position_keyword -- --test-threads=2` exits 0.
- **S04-AC-8** — `overflow` (and `overflow-x`/`-y`) maps `visible` to visible and `hidden`, `scroll` and `auto` to hidden; an unknown keyword is not applied. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::apply_property_parses_overflow_keyword -- --test-threads=2` exits 0.
- **S04-AC-9** — `z-index` takes `auto` and signed integers; `1.5` and other words are not applied. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::parses_z_index_keyword_and_integers style::value::tests::z_index_is_total_on_garbage` exits 0.
- **S04-AC-10** — `content` takes `normal`, `none`, a string (the empty string included) and `url(...)`; anything else is not applied. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::parses_content_property -- --test-threads=2` exits 0.
- **S04-AC-11** — `top`, `right`, `bottom` and `left` each set their own inset side, from a length (negative included), a percentage or `auto`. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::apply_property_parses_inset_offsets_like_margin -- --test-threads=2` exits 0.
- **S04-AC-12** — `min-width`/`min-height` take lengths, percentages and `auto`; `max-width`/`max-height` also take `none`, as no limit. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::min_max_width_height_parse_like_width_height_plus_none -- --test-threads=2` exits 0.
- **S04-AC-13** — Units match in any case: `10PX`, `1.5Em`, `2REM` in `gap` (read as `em`) and `1FR`. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::dimension_units_resolve_case_insensitively -- --test-threads=2` exits 0.
- **S04-AC-14** — `border` takes its width, style and colour in any order (`solid red 2px`). *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_shorthand_parses_width_style_color_in_any_order -- --test-threads=2` exits 0.
- **S04-AC-15** — A `border` with a non-solid style (`2px dashed red`) draws no border: its style is none. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::border_shorthand_non_solid_style_renders_as_none -- --test-threads=2` exits 0.
- **S04-AC-16** — A `border` with a trailing unknown token, or with a percentage width, is rejected whole. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::border_shorthand_rejects_the_whole_declaration_on_a_trailing_unrecognized_token style::value::tests::border_shorthand_rejects_a_percentage_width` exits 0.
- **S04-AC-17** — `border-top` sets its own field and leaves the all-sides `border` unset. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_top_longhand_parses_into_its_own_field_leaving_border_unset -- --test-threads=2` exits 0.
- **S04-AC-18** — A `border-top` with an unknown token is rejected whole. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_top_garbage_token_invalidates_the_whole_declaration -- --test-threads=2` exits 0.
- **S04-AC-19** — `border-width` with four values sets top, right, bottom, left. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_width_four_values_expand_top_right_bottom_left -- --test-threads=2` exits 0.
- **S04-AC-20** — `border-width` rejects a percentage. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_width_rejects_percentage_like_the_border_shorthand_does -- --test-threads=2` exits 0.
- **S04-AC-21** — `border-style: solid` is solid, and `border-style: dashed` applies as none rather than being rejected. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::border_style_solid_is_recognized style::value::tests::border_style_non_solid_keywords_map_to_none_not_rejected` exits 0.
- **S04-AC-22** — `border-color` takes a colour. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_color_parses_a_named_or_hex_color -- --test-threads=2` exits 0.
- **S04-AC-23** — `border-width`, `border-style` and `border-color` set neither `border` nor `border-top`. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_width_style_color_longhands_do_not_touch_the_border_shorthand_field -- --test-threads=2` exits 0.
- **S04-AC-24** — `box-sizing` takes `content-box` and `border-box`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::box_sizing_border_box_is_recognized style::value::tests::box_sizing_content_box_is_recognized` exits 0.
- **S04-AC-25** — `box-sizing: padding-box` is not applied. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::box_sizing_rejects_an_unknown_keyword -- --test-threads=2` exits 0.
- **S04-AC-26** — `border-spacing` with one value sets both axes, and with two sets x then y. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::border_spacing_one_value_sets_both_axes style::value::tests::border_spacing_two_values_set_x_then_y` exits 0.
- **S04-AC-27** — `border-spacing` rejects a percentage, a word, three values and an empty value. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::border_spacing_rejects_percent_and_garbage_and_too_many_values -- --test-threads=2` exits 0.
- **S04-AC-28** — `gap` with one value sets the row gap and no distinct column gap; with two it sets the row gap then the column gap. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::gap_one_value_sets_row_gap_only_leaving_column_gap_unset style::value::tests::gap_two_values_set_row_then_column_distinctly` exits 0.
- **S04-AC-29** — `gap` rejects a percentage in either position, a word, three values and an empty value. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::gap_rejects_percent_and_garbage_and_too_many_values -- --test-threads=2` exits 0.
- **S04-AC-30** — `border-collapse` takes `collapse` and `separate`; another word is not applied. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::border_collapse_property_parses_collapse_and_separate style::value::tests::border_collapse_unknown_value_is_not_applied` exits 0.
- **S04-AC-31** — A property outside the curated set is not applied. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::unknown_property_is_not_applied -- --test-threads=2` exits 0.
- **S04-AC-32** — `display` takes `table`, `table-row`, `table-cell`, `table-row-group` and `list-item` in any case; `table-column` is not applied. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::parses_table_display_values style::value::tests::parses_list_item_display_value style::value::tests::unknown_display_value_is_still_ignored` exits 0.
- **S04-AC-33** — `display: grid` is grid. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::display_grid_parses -- --test-threads=2` exits 0.
- **S04-AC-34** — `background` takes its colour from a bare name, a hex colour or `rgb()`, anywhere among `url()` and repeat keywords. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::background_shorthand_extracts_a_bare_color style::value::tests::background_shorthand_extracts_hex_color style::value::tests::background_shorthand_extracts_color_from_amid_image_and_repeat style::value::tests::background_shorthand_extracts_color_regardless_of_token_order style::value::tests::background_shorthand_rgb_function_color` exits 0.
- **S04-AC-35** — A colour name inside a `url()` is never read as the background colour (`url(red.png)` sets only the image). *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::background_shorthand_url_inside_never_misidentified_as_color -- --test-threads=2` exits 0.
- **S04-AC-36** — `background` takes its image from a `url()` anywhere in the value, with or without a colour. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::background_shorthand_extracts_both_color_and_image style::value::tests::background_shorthand_extracts_image_regardless_of_token_order style::value::tests::background_shorthand_with_a_url_but_no_color_sets_image_only` exits 0.
- **S04-AC-37** — A `background` with neither a colour nor a `url()` (`none`, a word) is not applied. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::background_shorthand_with_no_color_or_url_is_not_applied style::value::tests::background_shorthand_garbage_is_not_applied` exits 0.
- **S04-AC-38** — `font` reads a leading style and weight, the size, an optional `/line-height` (a number or a percentage) and the family (`italic bold 12px sans-serif`, `10px/1 Verdana, sans-serif`, `12px/150% serif`, `1em`). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::font_shorthand_extracts_size_and_family style::value::tests::font_shorthand_size_without_line_height_or_family style::value::tests::font_shorthand_extracts_leading_style_and_weight style::value::tests::font_shorthand_percent_line_height` exits 0.
- **S04-AC-39** — A `font` without a size is not applied. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::font_shorthand_without_a_size_is_not_applied -- --test-threads=2` exits 0.
- **S04-AC-40** — `background-image` takes `url(x.png)`, a double- or single-quoted URL, and an unquoted path. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::background_image_parses_a_bare_url style::value::tests::background_image_parses_a_quoted_url style::value::tests::background_image_parses_an_unquoted_url_with_a_path` exits 0.
- **S04-AC-41** — An unquoted `data:` URL keeps its `;` and its case: the Acid2 forehead image round-trips byte for byte, through `background-image` and `background` alike. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::background_image_parses_an_unquoted_data_url_with_an_internal_semicolon style::value::tests::background_image_round_trips_the_real_acid2_forehead_data_url_byte_for_byte` exits 0.
- **S04-AC-42** — `background-image: none`, a word, and an unterminated `url(` are not applied. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::background_image_none_clears_and_is_not_applied style::value::tests::background_image_garbage_is_not_applied style::value::tests::background_image_unterminated_url_does_not_panic_and_is_not_applied` exits 0.
- **S04-AC-43** — `overlay` lets the later block's values win field by field and keeps the fields it does not set. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::overlay_lets_later_values_win_per_field -- --test-threads=2` exits 0.
- **S04-AC-44** — `<font color>` sets the colour; a `color` attribute on another element does not. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::font_color_attribute_maps_to_color_declaration style::value::tests::color_attribute_on_a_non_font_element_is_ignored` exits 0.
- **S04-AC-45** — `bgcolor` sets the background colour on any element, written `#ffcc00`, `ffcc00`, in either case, or as a name; an invalid value is ignored. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::bgcolor_accepts_hash_bare_hex_and_named_colors_identically style::value::tests::bgcolor_applies_on_any_element_not_just_body style::value::tests::invalid_bgcolor_is_ignored` exits 0.
- **S04-AC-46** — `<font size>` 1 to 7 is 10, 13, 16, 18, 24, 32 and 48px. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::font_size_absolute_scale_1_through_7 -- --test-threads=2` exits 0.
- **S04-AC-47** — `<font size="+1">` is 18px and `-1` is 13px (a step from 3), and `+10`/`-10` clamp to 48px and 10px. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::font_size_relative_plus_and_minus_one_step_from_default style::value::tests::font_size_relative_clamps_at_scale_bounds` exits 0.
- **S04-AC-48** — `<font size>` of `abc`, `0`, `8`, an empty value, `3.5` or `++1` is ignored. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::font_size_garbage_or_absolute_out_of_range_is_ignored -- --test-threads=2` exits 0.
- **S04-AC-49** — `align` of `left`, `right`, `center` or `justify` sets text-align on `p`, `div` and `td`, but not on `img`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::align_center_left_right_justify_map_to_text_align_on_non_img_elements style::value::tests::align_is_ignored_on_img_elements` exits 0.
- **S04-AC-50** — `align` of `top`, `middle`, `bottom` or an unknown word sets no text-align. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::align_top_middle_bottom_are_ignored_everywhere -- --test-threads=2` exits 0.
- **S04-AC-51** — `<body text>` sets the colour; `text` on another element does not. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::body_text_attribute_maps_to_color style::value::tests::text_attribute_on_a_non_body_element_is_ignored` exits 0.
- **S04-AC-52** — `<table cellspacing>` sets both spacing axes in px, 0 included; on another element, or as a word, a negative, a fraction or an overflowing number, it is ignored. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::table_cellspacing_sets_both_border_spacing_axes style::value::tests::table_cellspacing_zero_is_a_valid_explicit_value style::value::tests::cellspacing_on_a_non_table_element_is_ignored style::value::tests::cellspacing_garbage_or_negative_is_ignored_never_panics` exits 0.
- **S04-AC-53** — `<table border>` without `cellspacing` hints border-collapse: collapse; with `cellspacing`, without `border`, or on another element there is no hint. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::table_border_with_no_cellspacing_sets_collapse_hint style::value::tests::table_border_with_cellspacing_does_not_set_collapse_hint style::value::tests::table_with_no_border_attribute_does_not_set_collapse_hint style::value::tests::border_collapse_hint_only_applies_to_table_elements` exits 0.
- **S04-AC-54** — `var(--x)` is replaced by the value of `--x`, through a chain of custom properties. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::substitute_vars_resolves_a_defined_custom_property style::value::tests::substitute_vars_resolves_a_chain_through_another_custom_property` exits 0.
- **S04-AC-55** — An undefined `var()` takes its fallback. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::substitute_vars_uses_fallback_when_undefined -- --test-threads=2` exits 0.
- **S04-AC-56** — An undefined `var()` with no fallback makes the value invalid. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::substitute_vars_undefined_with_no_fallback_is_invalid -- --test-threads=2` exits 0.
- **S04-AC-57** — A direct or two-step `var()` cycle makes the value invalid. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::substitute_vars_direct_self_cycle_is_invalid style::value::tests::substitute_vars_two_step_cycle_is_invalid` exits 0.
- **S04-AC-58** — A pathologically deep nest of `var()` fallbacks is cut off by the depth cap: the value is invalid, with no panic and no hang. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::substitute_vars_pathologically_deep_nested_fallback_does_not_panic_or_hang -- --test-threads=2` exits 0.
- **S04-AC-59** — In `background`, a `radial-gradient` or `linear-gradient` sets the background colour to its first colour stop, hex or named. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::radial_gradient_background_shorthand_extracts_first_color_stop style::value::tests::linear_gradient_named_color_stops_picks_the_first` exits 0.
- **S04-AC-60** — In `background-image`, `-webkit-` and `-moz-` prefixed gradients set the background colour to their first stop. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::webkit_prefixed_gradient_in_background_image_is_recognized style::value::tests::moz_prefixed_gradient_in_background_image_is_recognized` exits 0.
- **S04-AC-61** — `background-color` itself falls back to a gradient's first stop. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::background_color_property_itself_falls_back_to_a_gradients_first_stop -- --test-threads=2` exits 0.
- **S04-AC-62** — An ordinary `background-color` value is parsed as a colour. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::ordinary_background_color_values_are_unaffected_by_the_gradient_fallback -- --test-threads=2` exits 0.
- **S04-AC-63** — A gradient with no colour stop that parses sets no background colour. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::gradient_with_no_parseable_color_stops_leaves_background_unset -- --test-threads=2` exits 0.
- **S04-AC-64** — `grid-template-columns` takes a list of `fr` tracks, and px and `fr` tracks mixed. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::grid_template_columns_parses_a_bare_track_list style::value::tests::grid_template_columns_parses_mixed_length_and_fr_tracks` exits 0.
- **S04-AC-65** — `grid-template-columns` takes `repeat()` with a count, `auto-fill` or `auto-fit`, of plain or `minmax()` tracks, after a leading track too. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::grid_template_columns_parses_repeat_with_a_plain_count style::value::tests::grid_template_columns_parses_repeat_auto_fill_with_minmax style::value::tests::grid_template_columns_parses_repeat_auto_fit style::value::tests::grid_template_columns_mixes_a_leading_bare_track_with_a_repeat` exits 0.
- **S04-AC-66** — `grid-template-rows` sets the rows and leaves the columns unset. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::grid_template_rows_parses_independently_of_columns -- --test-threads=2` exits 0.
- **S04-AC-67** — A grid template with a component that does not parse (`1fr auto`, a `minmax` without its comma) is rejected whole, and so is an empty one. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::value::tests::grid_template_columns_rejects_an_unparseable_component_entirely style::value::tests::grid_template_columns_rejects_malformed_minmax style::value::tests::grid_template_columns_empty_value_does_not_apply` exits 0.
- **S04-AC-68** — A negative `fr` track is rejected. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::negative_fr_is_rejected -- --test-threads=2` exits 0.
- **S04-AC-69** — `margin` with three values sets top, then right and left, then bottom. *Verify:* unpinned.
- **S04-AC-70** — `font-weight: bold` is bold and `font-style: italic` is italic. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::bold_and_italic_ua_defaults -- --test-threads=2` exits 0.
- **S04-AC-71** — A numeric `font-weight` of 600 or more is bold, below it normal. *Verify:* unpinned.
- **S04-AC-72** — `text-decoration: underline` sets the underline. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::anchor_has_underline_by_default -- --test-threads=2` exits 0.
- **S04-AC-73** — `text-decoration` also takes `line-through` and `overline`. *Verify:* unpinned.
- **S04-AC-74** — `white-space: pre` is pre. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::pre_is_white_space_pre_and_monospace_by_default -- --test-threads=2` exits 0.
- **S04-AC-75** — `line-height` takes a bare number; `float: left` and `clear: both` apply. *Verify:* `cargo test --locked --jobs 2 --test css1_float_golden -- --test-threads=2` exits 0.
- **S04-AC-76** — `justify-content: space-between`, `align-items: center` and `text-align: center` apply. *Verify:* `cargo test --locked --jobs 2 --test flex_polite_golden -- --test-threads=2` exits 0.
- **S04-AC-77** — `flex-grow` takes a number. *Verify:* `cargo test --locked --jobs 2 --test kitchen_sink_golden -- --test-threads=2` exits 0.
- **S04-AC-78** — The `padding-left` longhand sets the left padding. *Verify:* `cargo test --locked --jobs 2 --test block_in_inline_golden -- --test-threads=2` exits 0.
- **S04-AC-79** — The `margin-left` longhand sets the left margin. *Verify:* unpinned.
- **S04-AC-80** — `vertical-align: middle` is middle. *Verify:* unpinned.
- **S04-AC-81** — `list-style-type: square` is square. *Verify:* unpinned.
- **S04-AC-82** — `flex-direction: column`, `flex-wrap: wrap`, `align-self: center` and `flex-basis: auto` apply. *Verify:* unpinned.
- **S04-AC-83** — `rgb()` takes percentages, scaled to 0 to 255. *Verify:* unpinned.
- **S04-AC-84** — `orange` parses to (255, 165, 0). *Verify:* unpinned.
- **S04-AC-85** — A generic family anywhere in a `font-family` list wins over the first name's hints, and without one the first name's hints (`courier`, `mono`, `arial`, `verdana`, ...) pick monospace or sans-serif. *Verify:* unpinned.
- **S04-AC-86** — `pt` lengths are kept as points for the cascade to convert. *Verify:* unpinned.

## Risks

- Most properties read only their first token, so trailing junk is silently accepted (`color: red blue` is
  red), and so is `!important`: it is never parsed, so it neither raises precedence nor rejects the
  declaration, except in the multi-token shorthands (`margin`, `padding`, `border`, `border-width`,
  `border-spacing`, `gap`, grid templates), where the `!` rejects the whole declaration.
- `rgba()` drops its alpha, so a translucent colour paints opaque; `hsl()`, the CSS3 extended colour names
  and `#rgba`/`#rrggbbaa` are not parsed.
- Only `solid` borders are drawn; the other styles parse to no border. `overflow: scroll|auto` clips like
  `hidden`.
- A gradient paints as one solid colour, its first stop: a page whose text relies on a dark end of the
  gradient can be repaired to another colour by S03.
- A single-value `gap` clears the column gap only within its own block: `overlay` copies a field only when
  it is set, so a lower-precedence two-value `gap` keeps its column gap under a later one-value `gap`.
- `font-family` keeps no name, only a face: it decides by a generic keyword anywhere in the list, else by
  hints in the first name (`courier`, `mono`, `arial`, `verdana`, ...), else serif.
- What the dialect speaks is fixed by the charter's C2 record: a new property, value or keyword here is a
  dialect change whose amendment and size price land in the spec PR that plans the node (`SPEC.md`
  principle 6).
