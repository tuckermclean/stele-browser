# L05 — box-tree

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, D03, S02, S06, I01, L01, L02 | none | L |

**Owns:** `src/layout/box_tree.rs`

## Purpose

Imported from the code. `stele::layout::box_tree` turns a parsed, cascaded `Dom` into the `LayoutNode` tree
that layout consumes. `build_box_tree(dom, styles, images)` and `build_box_tree_with_pseudo(dom, styles, images,
pseudo)` take the per-node `ComputedStyle`s and `::before`/`::after` styles (S06) and the decoded images keyed
by `NodeId` (I02). They return `None` for an empty `Dom` or a root with `display: none`. The walk is recursive
and stops at `DEPTH_CAP` (100): a deeper subtree becomes a childless `Container`.

Every node with `display: none` is dropped with its subtree. A text node becomes `BoxContent::Text`, and an
element's `id` attribute, trimmed, goes onto its node. The elements are handled in this order:

- **`img`** becomes `Replaced`, with the decoded image's `Rc` from `images` when there is one. Its intrinsic
  size is the `width`/`height` attributes (non-negative finite numbers). When neither parses, it is the decoded
  image's pixel size, else 0×0. `align=left|right` (any case) becomes `float` when the author's CSS left it at
  `none`. Other `align` values are ignored. `usemap` (with or without `#`, trimmed) is matched exactly,
  case-sensitively, against the `name` of any `<map>` in the document. The map's `<area>` children go through
  L01's `parse_area`, and the image gets `Interactive::ImageMap { areas, natural }`, where `natural` is the
  decoded size, else the intrinsic size. A `usemap` with no matching map leaves a plain image.
- **`object`** with a decoded image becomes `Replaced`, like `img`, and its children (the fallback) are
  dropped. Without one, it is an ordinary container whose children render.
- **Form controls** (`input`, `button`, `textarea`, `select`) become a `Container` holding one synthesized
  `Text` label. Their own DOM children are not walked. Both carry `Interactive::FormControl { kind, name,
  form_action }`, where `form_action` is the enclosing `<form>`'s raw `action`. The labels:
  - `input`: hidden gives no box. Text (the default type) gives `[value]`, or `size` underscores (10 by
    default, at most 100) when there is no value. Password gives one `*` per character. A checkbox is
    `[x]`/`[ ]` and a radio `(*)`/`( )`. Submit/image, reset and button give `[ label ]`, or `[ ]` when the
    label is empty.
  - The label is the first non-empty of the element's own text (for `button`), `value`, `aria-label` and
    `title`. A submit control falls back to `Submit` only inside a form; reset and button never invent one.
  - `textarea` shows its first line, cut to 20 characters and marked `[...]`, and marked `[...]` too when more
    lines follow. `select` shows `[ <text> v]` with the selected option, else the first, else nothing.
- **`form`** is a container whose descendants get its `action`.
- **`a href`** is a container, and it and every descendant get `Interactive::Link { href }` (raw, possibly
  empty), except an image that already has an image map. An `a` without `href` is not a link.
- **`br`** becomes the `Text` `LINE_BREAK_SENTINEL` (L02).
- **`details`** shows its first direct `summary` child, with the marker `v ` when `open` is present (any value)
  and `> ` otherwise. Without one, it shows a synthesized `Details` label. Only when open do its other children
  follow.
- **`ul` and `ol`** number their `li` children whose computed `display` is `list-item`, from `start` (default
  1). Each gets a marker text: `* ` for disc, `o ` for circle, `# ` for square, `N. ` for decimal and lower or
  upper alpha, and none for `list-style-type: none`. A `display: none` child takes no number. A nested list
  restarts at its own start.
- **Every other element** is a `Container` of its children, with the `::before` and `::after` boxes put first
  and last when their `content` is a string. A `display: table-cell` element becomes `TableCell` with `colspan`
  (1 to 1000) and `rowspan` (1 to 65534), where missing, unparseable or 0 counts as 1. A `<table>` then gets its
  presentational attributes, which never descend into a nested table:
  - `border=N` (N > 0) gives the table an N px solid gray (`#808080`) frame and every cell a 1px one, where the
    author's CSS left them unbordered.
  - `cellpadding=N` pads every cell N px on all sides where the author's CSS left it at 0.
  - `border` without `cellpadding` pads the cells 4px. Any `cellpadding`, even 0 or unparseable, suppresses
    that.
  - The table's `border-collapse` is copied onto its cells for the painters.

## Tests

The inline module `layout::box_tree::tests` in `src/layout/box_tree.rs` (117 tests, most of them parse HTML,
cascade it and build the tree). Through the pipeline: `tests/layout_floats.rs`, `tests/lists_golden.rs`,
`tests/details_golden.rs`, `tests/table_border_golden.rs`, `tests/form_submit.rs`,
`tests/kitchen_sink_golden.rs` and `accept.sh`'s object and image goldens.

## Changes

none: imported

## Gate

- **L05-AC-1** — An element with `display: none` is dropped with its subtree, and a root with `display: none` builds no tree. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::display_none_element_and_its_subtree_are_dropped layout::box_tree::tests::display_none_root_yields_none` exits 0.
- **L05-AC-2** — An element's `id` attribute goes onto its node, and a text node has none. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::element_id_attribute_survives_onto_layout_node` exits 0.
- **L05-AC-3** — An element becomes a container whose children follow the document order. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::plain_element_maps_to_container_with_recursive_children` exits 0.
- **L05-AC-4** — `::before` and `::after` with string content become a container box first and last among the element's children. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::before_after_generate_prepended_and_appended_boxes` exits 0.
- **L05-AC-5** — An `img` is `Replaced` at its `width`/`height` attributes, which win over a decoded image's size. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::img_element_maps_to_replaced_with_attribute_intrinsic_size layout::box_tree::tests::img_element_with_explicit_dimensions_ignores_the_decoded_images_real_size` exits 0.
- **L05-AC-6** — An `img` without size attributes takes the decoded image's pixel size. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::img_element_without_dimensions_falls_back_to_the_decoded_images_real_intrinsic_size` exits 0.
- **L05-AC-7** — An `img` carries the very `Rc` the images map holds for it. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::img_element_with_a_decoded_entry_in_the_images_map_carries_that_image` exits 0.
- **L05-AC-8** — An `object` with a decoded image is `Replaced` with no children, and of two nested objects the inner one with the image is. An `object` without one renders its fallback children. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::object_element_with_a_decoded_entry_in_the_images_map_carries_that_image_and_suppresses_fallback layout::box_tree::tests::nested_object_outer_no_image_inner_has_image_resolves_to_inner_replaced layout::box_tree::tests::object_element_with_no_images_map_entry_renders_its_fallback_children` exits 0.
- **L05-AC-9** — `align=left` and `align=right` (any case) float an image left and right. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::img_align_left_maps_to_float_left_presentational_hint layout::box_tree::tests::img_align_is_case_insensitive` exits 0.
- **L05-AC-10** — The author's CSS `float` wins over `align`, and `align=top|middle|bottom` sets no float. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::author_css_float_wins_over_align_attribute_hint layout::box_tree::tests::img_align_top_middle_bottom_are_ignored_not_mapped_to_float` exits 0.
- **L05-AC-11** — A table cell becomes `TableCell` with its spans. Missing, 0 or unparseable spans count as 1, and `colspan` is clamped to 1000 and `rowspan` to 65534. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::table_cell_maps_to_box_content_table_cell_with_spans layout::box_tree::tests::table_cell_span_parsing_defaults_and_clamps` exits 0.
- **L05-AC-12** — `border=N` gives the table an N px solid `#808080` frame and every cell a 1px one. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::table_border_attribute_stamps_table_and_1px_cell_borders layout::box_tree::tests::table_border_n_controls_table_frame_width_cells_stay_1px` exits 0.
- **L05-AC-13** — `border=0` gives no borders. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::table_border_zero_or_absent_yields_no_borders` exits 0.
- **L05-AC-14** — A cell border the author's CSS set is kept, and a nested `border=0` table's cells get no border from the outer table. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::table_border_attribute_never_overrides_author_css_border layout::box_tree::tests::table_border_nested_table_border_zero_does_not_inherit_outer_cells` exits 0.
- **L05-AC-15** — `cellpadding=N` pads every cell N px on all sides, but not the table itself. Cell padding the author's CSS set is kept, and a nested table gets its own. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::table_cellpadding_attribute_stamps_every_cell_all_sides layout::box_tree::tests::table_cellpadding_attribute_never_overrides_author_css_padding layout::box_tree::tests::table_cellpadding_nested_table_padding_is_independent` exits 0.
- **L05-AC-16** — `border` without `cellpadding` pads the cells by `DEFAULT_TABLE_BORDER_CELL_PADDING`, and an unparseable `cellpadding` still suppresses that. A table without `border` gets no default padding. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::table_border_with_no_cellpadding_gets_the_default_padding layout::box_tree::tests::table_border_with_unparseable_cellpadding_still_suppresses_the_default layout::box_tree::tests::table_without_a_border_attribute_gets_no_default_padding` exits 0.
- **L05-AC-17** — A text input shows `[value]`, also without a `type`, and a password input one `*` per character. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::text_input_renders_bracketed_value layout::box_tree::tests::text_input_without_type_defaults_to_text_behavior layout::box_tree::tests::password_input_masks_value_with_asterisks` exits 0.
- **L05-AC-18** — A text input without a value shows `size` underscores, and 10 without `size`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::text_input_without_value_renders_underscores_sized_to_size_attr layout::box_tree::tests::text_input_without_value_or_size_defaults_to_ten_underscores` exits 0.
- **L05-AC-19** — A checkbox shows `[x]` checked and `[ ]` not, and a radio `(*)` and `( )`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::checkbox_shows_x_when_checked_and_blank_when_not layout::box_tree::tests::radio_shows_star_when_checked_and_blank_when_not` exits 0.
- **L05-AC-20** — A submit input shows `[ value ]`, or `[ Submit ]` inside a form without a value, and `[ ]` outside a form. Reset and button inputs never invent a label. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::submit_input_shows_value_or_default_submit_label layout::box_tree::tests::submit_input_outside_a_form_never_invents_submit layout::box_tree::tests::reset_and_button_type_inputs_never_invent_a_default_label` exits 0.
- **L05-AC-21** — A control's label falls back from `value` to `aria-label` to `title`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::input_label_honors_aria_label_and_title_rungs layout::box_tree::tests::icon_button_uses_aria_label_not_submit layout::box_tree::tests::button_label_honors_title_rung` exits 0.
- **L05-AC-22** — A hidden input renders nothing. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::hidden_input_renders_nothing` exits 0.
- **L05-AC-23** — A `button`'s own text beats its `value`, which beats the in-form default, and `type=button` with nothing shows `[ ]`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::button_element_text_beats_value_beats_default layout::box_tree::tests::button_type_button_with_nothing_renders_empty_not_submit` exits 0.
- **L05-AC-24** — A `textarea` cuts a long first line and marks it `[...]`, and marks `[...]` after a short first line when more lines follow. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::textarea_truncates_long_first_line layout::box_tree::tests::textarea_marks_multiline_content_even_if_first_line_is_short` exits 0.
- **L05-AC-25** — A `select` shows `[ <text> v]` with the selected option, else the first. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::select_shows_selected_option_text layout::box_tree::tests::select_with_no_selected_option_defaults_to_first` exits 0.
- **L05-AC-26** — A form control does not walk its own DOM children: a `select` box has exactly one label child. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::form_controls_never_recurse_into_their_own_dom_children_as_generic_boxes` exits 0.
- **L05-AC-27** — A closed `details` shows only its first summary. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::collapsed_details_without_open_shows_only_the_summary layout::box_tree::tests::multiple_summaries_collapsed_shows_only_the_first_summary` exits 0.
- **L05-AC-28** — An open `details`, whatever the `open` attribute's value, shows the summary and every other child, and a second `summary` renders as plain content. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::open_details_shows_the_summary_and_every_other_child layout::box_tree::tests::details_open_attribute_with_any_value_still_counts_as_open layout::box_tree::tests::multiple_summaries_only_the_first_becomes_the_disclosure_label_when_open` exits 0.
- **L05-AC-29** — The summary's marker is `> ` closed and `v ` open. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::details_disclosure_marker_reflects_open_vs_closed_state` exits 0.
- **L05-AC-30** — Without a direct `summary` child, the label is `Details`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::details_with_no_summary_uses_the_default_label_when_collapsed layout::box_tree::tests::details_with_no_summary_uses_the_default_label_when_open layout::box_tree::tests::summary_not_a_direct_child_is_not_recognized_as_the_disclosure_label` exits 0.
- **L05-AC-31** — A `ul` item gets `* `. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::ul_items_each_get_a_leading_bullet_marker` exits 0.
- **L05-AC-32** — An `ol` numbers its items 1, 2, 3, up to 10,000 in a long list. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::ol_items_get_sequential_ordinal_markers layout::box_tree::tests::huge_ordered_list_does_not_panic_and_numbers_the_last_item` exits 0.
- **L05-AC-33** — `list-style-type: none` gives no marker. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::list_style_type_none_suppresses_the_marker` exits 0.
- **L05-AC-34** — `start` sets the first number. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::ordered_list_honors_the_start_attribute` exits 0.
- **L05-AC-35** — An `li` whose `display` is not `list-item` gets no marker and takes no number. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::li_with_non_block_display_gets_no_marker layout::box_tree::tests::ol_item_with_non_block_display_does_not_consume_an_ordinal layout::box_tree::tests::li_with_block_display_gets_no_marker layout::box_tree::tests::ol_item_with_block_display_does_not_consume_an_ordinal` exits 0.
- **L05-AC-36** — Every descendant of an `a href`, its wrapped text and an image inside it included, carries `Interactive::Link` with the raw href, an empty one too, and the text next to the link does not. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::link_text_fragments_carry_the_interactive_link_marker layout::box_tree::tests::wrapped_link_text_spanning_multiple_lines_carries_the_same_href_on_every_line layout::box_tree::tests::link_nested_in_paragraph_in_list_item_is_tagged_sibling_text_is_not layout::box_tree::tests::link_wrapping_an_image_tags_the_image_too layout::box_tree::tests::empty_href_is_still_a_link_and_does_not_panic` exits 0.
- **L05-AC-37** — An `a` without `href` is not a link. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::anchor_without_href_attribute_is_not_tagged_as_a_link` exits 0.
- **L05-AC-38** — A form control carries its kind, name and the enclosing form's action, and none outside a form. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::form_controls_carry_the_interactive_form_control_marker_with_kind_name_and_action layout::box_tree::tests::form_control_outside_any_form_has_no_form_action` exits 0.
- **L05-AC-39** — `usemap` finds a `<map>` anywhere in the document. Inside an `a href` the image keeps its image map, not the link. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::img_usemap_resolves_map_anywhere_in_the_document_not_just_an_ancestor layout::box_tree::tests::img_usemap_inside_a_href_keeps_image_map_not_the_anchor_link` exits 0.
- **L05-AC-40** — Map names match case-sensitively, and a `usemap` with no matching map leaves a plain image. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::map_name_matching_is_case_sensitive layout::box_tree::tests::dangling_usemap_with_no_matching_map_is_a_plain_image` exits 0.
- **L05-AC-41** — The image map's natural size is the decoded image's size, not the attributes, and the intrinsic size when nothing was decoded. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::image_map_natural_size_prefers_the_decoded_image_over_attrs layout::box_tree::tests::image_map_natural_size_falls_back_to_intrinsic_without_a_decode` exits 0.
- **L05-AC-42** — A DOM deeper than `DEPTH_CAP` returns a tree without the content past the cap. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::box_tree::tests::deeply_nested_dom_does_not_abort_and_returns` exits 0.
- **L05-AC-43** — `<br>` becomes a forced break: the kitchen-sink fixture's sentences around it render on separate lines. *Verify:* `cargo test --locked --jobs 2 --test kitchen_sink_golden -- --exact --test-threads=2 br_forces_a_real_line_break_in_the_kitchen_sink_render` exits 0.
- **L05-AC-44** — The table's `border-collapse` is copied onto its cells: the kitchen-sink tty dump matches its golden. *Verify:* `cargo test --locked --jobs 2 --test kitchen_sink_golden -- --exact --test-threads=2 kitchen_sink_tty_dump_matches_golden` exits 0.
- **L05-AC-45** — A bordered table without `cellpadding` pads its cells 4px (the test compares with the constant). A short single-line `textarea` is shown without a marker (the test looks for a substring). The circle, square and alpha markers are `o `, `# ` and letters, and `size` is capped at 100 characters. `::before`/`::after` with non-string content generate nothing. The order of a list's items is not checked separately from L05-AC-3. *Verify:* unpinned.

## Risks

- A `form` without `action` passes no action to its controls, so a submit control inside it shows `[ ]`
  instead of `[ Submit ]` and carries no `form_action`.
- A link tags every descendant, form controls inside it included, and a link's or a form's own element gets
  no `::before`/`::after` boxes (only the generic element path adds them).
- `DEPTH_CAP` repeats L04's and I02's caps by hand, and the walk is recursive.
- `usemap` scans the whole DOM once per image, which is quadratic on a page with many mapped images.
- Form-control labels are ASCII art in the dialect's text. Changing one changes the tty and PNG goldens.
