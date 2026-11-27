# L04 — block-layout

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | S02, I01, T03, L01, L02, L03 | none | L |

**Owns:** `src/layout/block.rs`

## Purpose

Imported from the code. `stele::layout::block` is the layout engine. It translates a `LayoutNode` tree onto the
vendored `taffy` flex/grid/block substrate (charter: block flow is degenerate column flex), runs taffy with
L02's inline engine and L03's table solver hanging off measure functions, and walks the result back into
paint-ordered `Fragment`s.

- **Entry points.** `layout_tree(root, viewport, metrics)` gives the root the viewport's width (border-box)
  and lets its height follow the content. `layout_tree_viewport` also fixes the root's height to
  `viewport.h`. A non-positive or non-finite viewport width lays out at max-content.
- **Translation.** `Text` becomes an inline leaf. `Replaced` becomes a leaf of its intrinsic size (clamped by
  `finite_nonneg`). A `display: table` container becomes a table leaf, when the table budget (`TABLE_DEPTH_CAP`,
  2 nested levels) allows and it has at most `MAX_TABLE_MEASURED_CELLS` (2,000) cells. Any other `Container`
  or `TableCell` becomes a taffy node of its `display`: `block`, `inline`, the table displays and `list-item`
  all map to block, `flex` to flex, `grid` to grid and `none` to none. Recursion stops at `DEPTH_CAP` (100).
- **Inline runs.** Outside flex and grid containers, each maximal run of inline-ish children is flattened into
  one inline leaf for L02, with the container's `text-align`. Inline-ish means text, images, and
  `display: inline` containers with no block-level descendant. An inline container that holds a block (a list
  inside `<font>`, say) is laid out as a block instead. In a flex or grid container every child is its own
  item, and whitespace-only text is dropped.
- **Style mapping.** `base_style` maps size, min and max size, margin, padding, border widths, `float`,
  `clear`, `box-sizing` and `position` with insets. `box-sizing` is content-box unless declared, and always
  border-box for the table displays. `static` and `relative` map to taffy's relative, and `absolute` and
  `fixed` to absolute. A `static` box ignores its insets. Non-finite lengths become 0 or `auto`. Flex
  direction, wrap, justify, align, grow, shrink, basis and gaps are mapped, with `column-gap` falling back to
  `gap`. Grid templates (`repeat`, `auto-fill`, `auto-fit`, `minmax`, `fr`, px, %) are mapped too.
- **Margin collapsing.** taffy collapses every pair of adjacent block siblings. A pre-pass rewrites the
  margins so that two in-flow block siblings collapse to the larger margin only when neither touching edge has
  a visible border or padding and neither is floated, inline-ish or a table display. Whitespace-only text
  between them does not count. Otherwise the margins add up. Percentage margins are left to taffy. Flex and
  grid items are not touched.
- **Tables.** A table leaf places its grid with L03's `place_grid` and measures each cell's min and max
  content width. It solves the columns, lays each cell out at its width to get its height, and solves again.
  The result is cached per available width. `border-spacing` applies between cells in the separated model.
  Under `border-collapse: collapse` the spacing is 0, adjacent cells share one grid line of the widest cell
  border, and the table's own frame overlaps the edge cells. A cell's first fragment is stretched to its row
  height.
- **Emission.** A container pushes a `Box` fragment and then its children in paint order: negative `z-index`
  positioned children, static children, `z-index: 0` positioned ones, then positive ones, each group in
  `z-index` and then source order. Its `id` goes on its own `Box` fragment only. `overflow: hidden` clips the
  descendants (intersected with outer clips) but not the box itself. A `position: fixed` child is laid out
  against the viewport origin, flagged `is_fixed` down its subtree, and clipped by the root's viewport clip.
  An image with pixels becomes an `Image` fragment, and one without a `Box` placeholder. Inline leaves become
  `Text` fragments per positioned run, inline images sit with their bottom on the baseline, and floats sit at
  their placed rects. Every fragment carries its source's `Interactive`.

## Tests

The inline module `layout::block::tests` in `src/layout/block.rs` (22 tests: style mapping, inline-ish
classification, clipping, ids, fixed positioning). Through `layout::layout`: `tests/layout_block.rs` (box
model, margin collapsing, flex, grid, viewport, z-order), `tests/layout_block_floats.rs` (block-level floats),
`tests/layout_floats.rs`, `tests/layout_table.rs`, `tests/layout_integration.rs`, and the PNG and tty goldens
(`grid_golden`, `flex_polite_golden`, `css1_float_golden`, `table_*_golden`, `accept.sh`'s `A5c` to `A5p`).

## Changes

none: imported

## Gate

- **L04-AC-1** — Margins, borders and padding nest: an inner box sits at the outer padding plus its own margin. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 nested_margin_padding_border_produce_expected_rects asymmetric_margins_are_honored` exits 0.
- **L04-AC-2** — A block with `width: auto` fills the viewport's width. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 block_width_auto_fills_container` exits 0.
- **L04-AC-3** — Adjacent block siblings' margins collapse to the larger one (or the shared value when equal), across whitespace-only text too. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 adjacent_block_siblings_collapse_to_the_larger_margin adjacent_block_siblings_with_equal_margins_collapse_to_that_margin whitespace_only_text_between_block_siblings_does_not_break_collapsing` exits 0.
- **L04-AC-4** — A visible border or padding on the touching edges stops the collapse. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 a_border_between_block_siblings_prevents_collapse padding_between_block_siblings_prevents_collapse` exits 0.
- **L04-AC-5** — Floated siblings, table rows and flex column items do not collapse their margins. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 floated_sibling_margins_do_not_collapse table_row_sibling_margins_do_not_collapse flex_column_item_margins_do_not_collapse` exits 0.
- **L04-AC-6** — A replaced box takes its intrinsic size in the flow, and one with pixels emits an `Image` fragment at its rect, not a placeholder `Box`. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 replaced_node_occupies_intrinsic_size_in_flow replaced_node_with_an_image_emits_an_image_fragment_at_its_rect_not_a_placeholder_box` exits 0.
- **L04-AC-7** — An extremely deep tree lays out without aborting the process (`DEPTH_CAP`). *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 extremely_deep_nesting_does_not_abort_the_process` exits 0.
- **L04-AC-8** — `layout_tree_viewport` fixes the root's height to the viewport's, so an `overflow: hidden` root clips a taller child. `layout_tree` grows to the content. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 layout_viewport_clamps_the_root_fragment_height_to_the_given_viewport_height layout_viewport_with_overflow_hidden_root_clips_a_taller_child layout_content_height_on_the_same_tall_tree_still_grows_to_content` exits 0.
- **L04-AC-9** — `overflow: hidden` clips a box's descendants to its border box and leaves the box itself unclipped. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::overflow_hidden_clips_descendants_not_the_container_itself` exits 0.
- **L04-AC-10** — A node's `id` is on exactly one fragment, its own box. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::emit_stamps_id_onto_the_owning_box_fragment_only` exits 0.
- **L04-AC-11** — A `position: fixed` box is flagged `is_fixed`, its static sibling is not, and it is placed against the viewport, not its parent, at any nesting depth. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::fixed_positioned_box_fragment_is_flagged_is_fixed layout::block::tests::position_fixed_anchors_to_the_viewport_not_the_dom_parent layout::block::tests::position_fixed_anchoring_is_unaffected_by_ancestor_nesting_depth` exits 0.
- **L04-AC-12** — A `position: fixed` box inside an `overflow: hidden` root gets the root's viewport clip. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::position_fixed_inside_an_overflow_hidden_root_gets_the_viewport_clip` exits 0.
- **L04-AC-13** — Positioned children paint after their static siblings, and among them a negative `z-index` paints first, then `z-index: auto`, then a positive one. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 positioned_child_paints_after_static_sibling_regardless_of_source_order positioned_children_paint_in_z_index_then_source_order` exits 0.
- **L04-AC-14** — An inline container that holds a block descendant, through a chain of inline wrappers too, is not inline-ish, so a list inside an inline wrapper is not folded into one inline leaf. One that holds only text or an image stays inline-ish. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::is_inline_ish_false_for_inline_container_with_a_block_descendant layout::block::tests::contains_block_descendant_true_through_a_chain_of_inline_wrappers layout::block::tests::contains_block_descendant_false_for_inline_wrapping_a_replaced_atom layout::block::tests::is_inline_ish_true_for_inline_container_wrapping_a_replaced_atom` and `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 block_level_list_inside_an_inline_wrapper_is_not_folded_into_one_inline_leaf` exit 0.
- **L04-AC-15** — `display: list-item` lays out exactly like `block`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::list_item_display_lays_out_identically_to_block` exits 0.
- **L04-AC-16** — An ordinary box is content-box sized and an explicit `box-sizing: border-box` is honoured. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::base_style_defaults_to_content_box_for_an_ordinary_element layout::block::tests::base_style_honors_an_explicit_border_box_declaration layout::block::tests::map_box_sizing_is_a_straight_rename` and `cargo test --locked --jobs 2 --test layout_table -- --exact --test-threads=2 plain_block_box_with_width_padding_border_uses_content_box_sizing` exit 0.
- **L04-AC-17** — A table cell with width, padding and border keeps border-box sizing. *Verify:* `cargo test --locked --jobs 2 --test layout_table -- --exact --test-threads=2 table_cell_with_width_padding_border_keeps_border_box_sizing` exits 0.
- **L04-AC-18** — `min-width` and the other min and max sizes map to taffy's min and max size, `fixed` and `absolute` map to taffy's absolute, and a `static` box ignores its insets. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::block::tests::base_style_maps_min_max_width_height_to_taffys_native_min_max_size layout::block::tests::base_style_maps_position_and_inset_to_taffy layout::block::tests::static_box_ignores_inset_offsets` exits 0.
- **L04-AC-19** — A flex row lays its items out left to right, spaced by `column-gap` rather than `row-gap`, and `flex-grow` takes the remaining width. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 flex_row_lays_items_out_left_to_right_with_gap flex_row_uses_column_gap_not_row_gap_for_horizontal_item_spacing flex_grow_item_takes_remaining_width_beside_a_fixed_width_sibling` exits 0.
- **L04-AC-20** — `justify-content: space-between` pushes the items to opposite edges, and whitespace-only text between flex items takes no gap. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 justify_content_space_between_pushes_items_to_opposite_edges whitespace_only_text_between_flex_items_does_not_consume_gap_space` exits 0.
- **L04-AC-21** — A grid of three `1fr` columns places items side by side with its gap, `repeat(3, 1fr)` equals three bare tracks, and `repeat(auto-fill, minmax(...))` gives the right column count at 800px. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 grid_three_equal_fr_columns_places_items_side_by_side grid_gap_produces_expected_inter_item_spacing grid_repeat_3_1fr_is_equivalent_to_three_bare_1fr_tracks grid_auto_fill_minmax_computes_the_correct_column_count_at_800px` exits 0.
- **L04-AC-22** — Two `float: left` blocks sit side by side, and a float sits beside a normal-flow sibling, not above it. *Verify:* `cargo test --locked --jobs 2 --test layout_block_floats -- --exact --test-threads=2 two_float_left_siblings_sit_side_by_side_not_stacked float_left_and_normal_flow_sibling_coexist_not_stacked_below` exits 0.
- **L04-AC-23** — A `float: right` block sits at its containing block's right edge, and `clear: both` goes below both floats. *Verify:* `cargo test --locked --jobs 2 --test layout_block_floats -- --exact --test-threads=2 float_right_block_sits_at_containing_blocks_right_edge clear_both_is_pushed_below_both_floats_bottom_edges` exits 0.
- **L04-AC-24** — A table's columns line up across rows, its `border-spacing` sets the gap between columns (8px by default), and a row-spanning cell's box is stretched over its rows. *Verify:* `cargo test --locked --jobs 2 --test layout_table -- --exact --test-threads=2 plain_2x2_table_columns_align_across_rows table_border_spacing_style_controls_the_gap_between_columns table_with_default_style_still_uses_8px_border_spacing separate_table_with_same_spacing_style_still_shows_the_gap rowspan_cell_box_spans_summed_row_height` exits 0.
- **L04-AC-25** — Under `border-collapse: collapse`, single-span cells share one grid line, a spanning cell covers the full grid width, and a table's own frame coincides with its edge cells. *Verify:* `cargo test --locked --jobs 2 --test layout_table -- --exact --test-threads=2 collapsed_single_span_cells_share_one_grid_line_no_table_frame collapsed_colspan_cell_spans_the_full_grid_width collapsed_table_with_own_frame_border_coincides_with_edge_cells` exits 0.
- **L04-AC-26** — Cell padding offsets the cell's content from the cell box and grows its min and max width. *Verify:* `cargo test --locked --jobs 2 --test layout_table -- --exact --test-threads=2 cell_padding_offsets_content_from_the_cell_box_origin cell_padding_grows_min_max_width_and_intrinsic_height` exits 0.
- **L04-AC-27** — Deeply nested tables lay out in bounded time, because only `TABLE_DEPTH_CAP` (2) levels are measured as tables. *Verify:* `cargo test --locked --jobs 2 --test layout_table -- --exact --test-threads=2 very_deeply_nested_tables_do_not_abort deeply_nested_tables_do_not_abort` exits 0.
- **L04-AC-28** — A table past `MAX_TABLE_MEASURED_CELLS` (2,000 cells) lays out as plain blocks. Several positioned children with the same `z-index` keep their source order. Under `border-collapse: collapse` the `border-spacing` is ignored. Max-content measurement uses a 10,000,000px width. Percentage margins are left to taffy's own collapsing, and a parent's margin collapses with its child's only as taffy does it. Inline elements paint no box of their own. A floated image's fragment sits at the float's rect. *Verify:* unpinned (`wide_table_past_measurement_cap_degrades_promptly` also passes, in about a second, without the cap; the z-index test has one child per layer; the collapsed-spacing test passes with the spacing kept).

## Risks

- Parent and child margin collapsing is taffy's own behaviour and is not controlled here: a box with no
  padding or border can merge its margin with its first child's.
- Inline elements (`<a>`, `<em>`, `<span>`) paint no background or border, only their text's colour and font.
- Tables nest at most `TABLE_DEPTH_CAP` (2) levels as real tables; deeper tables, and tables past 2,000 cells,
  lay out as plain stacked blocks.
- Each table is measured several times (min, max, then laid out at the solved widths), which costs time on
  the 486 for large tables.
- `FragmentKind::Image` (L01) holds an `RgbaImage` by value, so `push_replaced_fragment` copies an image's
  pixels out of its `Rc` into every fragment on every layout. Animated playback (issue #119) would pay that
  copy per frame unless the fragment shares the `Rc`.
- `DEPTH_CAP` (100) is repeated by hand in L05, I02 and `dom_util`.
- The module doc's opening describes `Replaced` as a placeholder with "no pixel data", which is stale: images
  are painted today.
