# S06 — cascade

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, S02, S04, S05 | L05, S07 | L |

**Owns:** `src/style/cascade.rs`, `src/style/ua.rs`

## Purpose

Imported from the code. The cascade folds the user-agent stylesheet, each element's presentational hints,
the author sheets and its inline style onto the DOM (D01), and resolves the result into one `ComputedStyle`
(S02) per node.

- **`cascade(dom, author_sheets) -> Vec<ComputedStyle>`** (`stele::style::cascade::cascade`) returns one style
  per node, indexed by `NodeId`. It walks the tree with an explicit heap stack, never the call stack, so any
  nesting depth is safe and every node still gets its real inherited style. A text node takes its parent's
  style wholesale.
- **Precedence.** For each element every matching rule (S05) is sorted by tier, then specificity, then sheet
  index, then source order, and folded low to high (`Declarations::overlay`, S04). The tiers are the UA sheet
  (0), the element's presentational hints (1, `value::presentational_hints`) and the author sheets (2), so
  specificity is compared across all author sheets together and the sheet index only breaks ties. The inline
  `style="..."` declarations fold last and beat every tier. There is no `!important`.
- **Custom properties.** Each element's environment is its parent's overlaid with its own `--name`
  declarations; the deferred `var()` declarations are substituted against it (S04) and applied after the
  rest, and one that is invalid is dropped.
- **Resolution** (`resolve`): inherited properties (colour, font family, size, weight and style, line height,
  text-align, white-space, list-style-type) fall back to the parent's computed value; the box properties fall
  back to the initial value. `font-size` in `em`/`%` resolves against the parent's size; every other `em`
  against the element's own; `pt` is 96/72 px; `%` lengths stay percentages for layout, and grid-track
  percentages too. `line-height` as a number or percentage multiplies the font size. An inline element that
  is absolutely positioned or fixed becomes a block. The `border` shorthand sets four sides (solid only, a
  solid border without width is 3px), `border-top` then replaces the top side, and the `border-width`/`-style`/
  `-color` longhands override per side.
- **`cascade_pseudo(dom, author_sheets, elem_styles) -> Vec<PseudoStyles>`** resolves `::before`/`::after`
  rules (UA and author tiers) against each element's computed style, with an empty custom-property
  environment, and keeps a pseudo style only when its `content` generates a box. Pseudo-element rules never
  apply to the element itself.
- **The UA sheet** (`ua.rs`, `UA_CSS`) is ordinary CSS parsed once by the same parser: block display for the
  block elements (`noscript` included, since there is no script), `display: none` for `head`, `style`, `title`,
  `script`, `meta`, `link`, `base`, `map` and `area`, the table display values, `li` as `list-item`, form controls
  inline, the classic margins (`body` 8px, `p` 1em, headings, lists with 40px padding, `blockquote`), heading
  sizes 2em to 0.67em in bold, `b`/`strong` bold, `i`/`em` italic, `u` and `a` underlined, `a` blue,
  `tt`/`code`/`pre` monospace, `pre` keeping white space, `center` centred, disc and decimal lists, and `hr` as a
  zero-height box with a 1px solid `#808080` top border.

## Tests

The inline module `style::cascade::tests` in `src/style/cascade.rs` (71 tests: UA defaults, inheritance, units,
precedence across tiers and sheets, inline style, presentational hints, tables, custom properties, depth and
the pseudo-elements). `ua.rs` has no tests of its own; the cascade tests and the golden tests
(`tests/*_golden.rs`, `./accept.sh --tty-only`) render through it.

## Changes

none: imported

## Gate

- **S06-AC-1** — By the UA sheet a `div` is a block and a `span` inline. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::div_is_block_span_is_inline_by_ua_defaults -- --test-threads=2` exits 0.
- **S06-AC-2** — `head` and `style` are `display: none`. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::head_and_style_are_display_none -- --test-threads=2` exits 0.
- **S06-AC-3** — `table`, `thead`, `tbody`, `tr`, `th` and `td` get the table display values. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::table_elements_get_table_display_values_from_ua_sheet -- --test-threads=2` exits 0.
- **S06-AC-4** — `li` is `list-item` by default, and inline style can make it a block or inline. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::li_gets_list_item_display_from_ua_sheet_by_default style::cascade::tests::author_css_can_override_li_display_away_from_list_item` exits 0.
- **S06-AC-5** — An absolutely positioned inline element becomes a block; a relatively positioned or static one stays inline. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::absolutely_positioned_inline_is_blockified -- --test-threads=2` exits 0.
- **S06-AC-6** — `display: grid` and a `grid-template-columns` reach the computed style, and the template does not inherit. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::author_display_grid_and_grid_template_cascade_onto_computed_style -- --test-threads=2` exits 0.
- **S06-AC-7** — A child inherits its parent's colour and font family. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::child_inherits_color_and_font_family_from_parent -- --test-threads=2` exits 0.
- **S06-AC-8** — `background-image` does not inherit. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::background_image_does_not_inherit -- --test-threads=2` exits 0.
- **S06-AC-9** — Box properties (margin) do not inherit. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::box_properties_do_not_inherit -- --test-threads=2` exits 0.
- **S06-AC-10** — The background colour (from `bgcolor` too) does not inherit. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::bgcolor_variants_agree_and_do_not_inherit -- --test-threads=2` exits 0.
- **S06-AC-11** — `border-top` sets only the top side; the other sides stay at the default. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::border_top_declaration_sets_only_the_top_side style::cascade::tests::hr_ua_rule_renders_as_a_zero_height_box_with_a_solid_top_border` exits 0.
- **S06-AC-12** — `border-width` (in `em`), `border-style` and `border-color` resolve per side without a `border` shorthand. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::border_width_style_color_longhands_resolve_per_side_with_no_border_shorthand -- --test-threads=2` exits 0.
- **S06-AC-13** — A solid `border` shorthand gives its width, style and colour to the sides. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::border_shorthand_alone_is_unaffected_by_the_new_longhand_fields style::parser::tests::border_shorthand_solid_only` exits 0.
- **S06-AC-14** — An `hr` is a block of height 0 with a 1px solid `#808080` top border and no other border. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::hr_ua_rule_renders_as_a_zero_height_box_with_a_solid_top_border -- --test-threads=2` exits 0.
- **S06-AC-15** — `font-size` in `em` resolves against the parent's computed size. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::em_font_size_resolves_against_parent_computed_size -- --test-threads=2` exits 0.
- **S06-AC-16** — `font-size` in `%` resolves against the parent's computed size. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::percent_font_size_resolves_against_parent_computed_size -- --test-threads=2` exits 0.
- **S06-AC-17** — Width, height, padding, margin and border width in `em` resolve against the element's own font size. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::width_height_padding_em_resolve_against_the_raw_unfloored_font_size -- --test-threads=2` exits 0.
- **S06-AC-18** — An element with no `font-size` inherits its parent's. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::font_size_inherits_when_not_set -- --test-threads=2` exits 0.
- **S06-AC-19** — A `line-height` percentage resolves against the element's font size. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::line_height_percent_resolves_against_font_size -- --test-threads=2` exits 0.
- **S06-AC-20** — The UA `h1` is 2em, 32px at the root. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::ua_heading_font_size_is_relative_em_against_root -- --test-threads=2` exits 0.
- **S06-AC-21** — `b` is bold and `i` italic by default. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::bold_and_italic_ua_defaults -- --test-threads=2` exits 0.
- **S06-AC-22** — An `a` is underlined by default. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::anchor_has_underline_by_default -- --test-threads=2` exits 0.
- **S06-AC-23** — `pre` keeps white space and is monospace by default. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::pre_is_white_space_pre_and_monospace_by_default -- --test-threads=2` exits 0.
- **S06-AC-24** — A `ul` lists with discs and an `ol` with decimals by default. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::list_style_type_defaults_disc_and_decimal -- --test-threads=2` exits 0.
- **S06-AC-25** — Of two author sheets with equal rules, the later wins. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::multiple_author_sheets_apply_in_order -- --test-threads=2` exits 0.
- **S06-AC-26** — Specificity is compared across author sheets: `#foo` beats `p` whichever sheet comes first. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::cross_sheet_specificity_wins_regardless_of_sheet_order style::cascade::tests::cross_sheet_specificity_also_holds_in_reverse_sheet_order style::parser::tests::specificity_id_beats_class` exits 0.
- **S06-AC-27** — On equal specificity in one sheet, the later rule wins. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::later_source_order_wins_specificity_ties -- --test-threads=2` exits 0.
- **S06-AC-28** — Inline style beats the author sheets, the UA sheet and presentational hints. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::inline_style_beats_author_sheet style::cascade::tests::inline_style_beats_ua_author_and_everything_else_end_to_end style::cascade::tests::inline_style_beats_presentational_hint_too` exits 0.
- **S06-AC-29** — Inline style applies with no author sheet, and a malformed declaration in it is skipped while the others apply. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::inline_style_applies_with_no_author_sheet style::cascade::tests::malformed_inline_style_is_ignored_other_properties_still_apply_no_panic` exits 0.
- **S06-AC-30** — Author CSS beats a presentational hint (colour, border spacing, border collapse). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::author_css_beats_presentational_hint style::cascade::tests::css_border_spacing_beats_cellspacing_attribute style::cascade::tests::author_css_separate_overrides_the_table_border_presentational_hint` exits 0.
- **S06-AC-31** — Presentational hints reach the computed style: `<font color>` beats the inherited colour while a plain sibling still inherits, `<font size>`, `align="center"` on `p`/`div`/`td`, `<body text>` inherited by descendants, `cellspacing`, and `<table border>` collapsing. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::font_color_attribute_sets_computed_color style::cascade::tests::presentational_hint_overrides_inherited_color_but_plain_sibling_still_inherits style::cascade::tests::font_size_attribute_scale_absolute_and_relative style::cascade::tests::align_center_sets_text_align_on_block_elements style::cascade::tests::body_text_attribute_sets_color_and_inherits_to_descendants style::cascade::tests::table_cellspacing_attribute_resolves_onto_the_table_style style::cascade::tests::table_border_attribute_alone_resolves_to_collapse` exits 0.
- **S06-AC-32** — A table with no `cellspacing` or CSS keeps the default border spacing, 8px by 0. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::table_with_no_cellspacing_or_css_keeps_the_default_border_spacing -- --test-threads=2` exits 0.
- **S06-AC-33** — CSS `border-spacing` and `cellspacing` resolve onto the table, both axes. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::css_border_spacing_property_resolves_onto_the_table style::cascade::tests::table_cellspacing_attribute_resolves_onto_the_table_style` exits 0.
- **S06-AC-34** — `border-collapse` from `<table border>` or from CSS reaches the table's computed style. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::table_border_attribute_alone_resolves_to_collapse style::cascade::tests::author_css_border_collapse_property_resolves_onto_the_table` exits 0.
- **S06-AC-35** — The output has one style per DOM node, at 3,000 and 5,000 levels of nesting too. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::cascade_output_len_matches_dom_len style::cascade::tests::cascade_is_total_on_dom_nested_3000_deep style::cascade::tests::cascade_is_total_on_dom_nested_5000_deep` exits 0.
- **S06-AC-36** — The deepest element of a very deep tree still gets its real style: no depth cap degrades it to the defaults. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::cascade_inherits_correctly_past_any_naive_depth_cap -- --test-threads=2` exits 0.
- **S06-AC-37** — A custom property declared on `:root`, or on an ancestor, reaches a descendant's `var()` through elements that declare none. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::root_custom_property_resolves_via_var_on_a_descendant style::cascade::tests::custom_property_inherits_through_intermediate_elements_with_no_custom_properties_of_their_own` exits 0.
- **S06-AC-38** — A `var()` chain resolves against the element's own custom properties, and an attribute-scoped override (`html[data-theme="dark"] { --bg: ... }`) applies only with the attribute. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::var_chain_resolves_through_another_custom_property style::cascade::tests::attribute_scoped_custom_property_override_gates_on_the_attribute` exits 0.
- **S06-AC-39** — A `var()` with a fallback and no declared property takes the fallback. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::var_fallback_is_used_when_the_custom_property_is_never_declared_anywhere -- --test-threads=2` exits 0.
- **S06-AC-40** — A `var()` that cannot resolve (undefined with no fallback, or a cycle) drops the declaration: the colour stays black, and the cascade does not hang. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::cascade::tests::var_with_no_fallback_and_undefined_custom_property_falls_back_to_css_initial style::cascade::tests::var_cycles_are_invalid_and_the_cascade_never_hangs` exits 0.
- **S06-AC-41** — `::before` with `content: "B"` gets a pseudo style with its own declarations, `::after` with `content: none` gets none, and an element with no pseudo rules gets neither. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::cascade_pseudo_before_after_generate_only_with_content -- --test-threads=2` exits 0.
- **S06-AC-42** — `content: ""` still generates a pseudo box. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::empty_string_content_still_generates_pseudo_box -- --test-threads=2` exits 0.
- **S06-AC-43** — A `::before` rule does not style the element itself. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::unsupported_selector_kinds_parse_without_choking_and_do_not_match -- --test-threads=2` exits 0.
- **S06-AC-44** — Author CSS beats the UA sheet (`.g { display: grid }` over the UA's `div { display: block }`). *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::author_display_grid_and_grid_template_cascade_onto_computed_style -- --test-threads=2` exits 0.
- **S06-AC-45** — The UA gives `body` an 8px margin and `p` a 1em vertical margin. *Verify:* `cargo test --locked --jobs 2 --bin stele tests::dump_text_over_file_fetch_matches_the_tty_golden -- --test-threads=2` exits 0.
- **S06-AC-46** — The UA makes `h2` 1.5em and `h1` bold. *Verify:* `cargo test --locked --jobs 2 --bin stele tests::dump_png_color_scheme_default_matches_the_pre_t1d_render -- --test-threads=2` exits 0.
- **S06-AC-47** — The UA centres a `center` element's content. *Verify:* `cargo test --locked --jobs 2 --test presentational_golden -- --test-threads=2` exits 0.
- **S06-AC-48** — The UA gives `ul` and `ol` a 40px left padding. *Verify:* `cargo test --locked --jobs 2 --test block_in_inline_golden -- --test-threads=2` exits 0.
- **S06-AC-49** — The UA keeps form controls inline. *Verify:* `cargo test --locked --jobs 2 --test css1_float_golden -- --test-threads=2` exits 0.
- **S06-AC-50** — The UA makes `noscript` a block. *Verify:* `cargo test --locked --jobs 2 --lib layout::box_tree::tests::noscript_is_block_level_per_the_ua_sheet -- --test-threads=2` exits 0.
- **S06-AC-51** — The UA hides `map` and `area`. *Verify:* `cargo test --locked --jobs 2 --lib layout::box_tree::tests::map_and_area_elements_produce_no_boxes_of_their_own -- --test-threads=2` exits 0.
- **S06-AC-52** — A solid border with no width is 3px. *Verify:* unpinned.
- **S06-AC-53** — A `pt` length is 96/72 px. *Verify:* unpinned.
- **S06-AC-54** — The UA makes `h6` 0.67em, underlines `u`, sets `code` in monospace and gives `blockquote` 1em 40px margins. *Verify:* unpinned.

## Risks

- A deferred `var()` declaration is applied after every plain one, whatever its precedence: a lower-precedence
  `color: var(--a)` beats a higher-precedence `color: red` on the same element.
- Pseudo-element styles resolve with an empty custom-property environment, so a `var()` inside a
  `::before`/`::after` rule takes its fallback or is dropped.
- A `var()` that cannot resolve drops the declaration rather than making the property `unset`; for a box
  property the result is the same, for an inherited one it is the parent's value.
- Rule matching is linear in rules × elements and clones each element's attributes into its `ElementInfo`;
  every node, text included, holds a full `ComputedStyle` clone.
- The UA sheet is the dialect's element semantics: a new element default here changes rendering for every
  page, and goldens with it.
