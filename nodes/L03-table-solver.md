# L03 — table-solver

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | L01, S02 | L04 | M |

**Owns:** `src/layout/table.rs`, `src/layout/table_layout.rs`

## Purpose

Imported from the code. The two pure halves of HTML table layout. L04 measures each cell's content, drives
them, and emits the fragments.

`layout::table_layout::place_grid(table)` runs the HTML table auto-placement over a table `LayoutNode`'s
children. The rows are the table's `display: table-row` children, plus the `table-row` children of its
`table-row-group` children (`<thead>`, `<tbody>`, `<tfoot>`), in order. Any other child is skipped. In each
row, only the `BoxContent::TableCell` children are cells. Each cell goes at the first column of its row that
an earlier row's `rowspan` has not taken, and its `colspan` and `rowspan` (0 counts as 1) are clamped to the
grid. The result is a `Grid { columns, rows, cells }`, each `GridCell` with its origin, spans and a reference
to the cell's node. At most `MAX_GRID_DIM` (4,096) rows and columns are placed and at most `MAX_GRID_CELLS`
(262,144) slots are marked.

`layout::table::solve_table(spec)` is the two-pass min/max-content automatic table layout (CSS 2.1 §17.5.2,
HTML 4.01 "auto"). It works on a `TableSpec` (columns, rows, already measured `CellSpec`s with an explicit
grid position, the available width, and the border spacing) and returns a `TableLayout` (column widths, row
heights, and one rect per input cell):

- **Placement.** Columns and rows are clamped to `MAX_GRID_DIM` and the grid area to `MAX_GRID_CELLS`. A cell
  outside the grid, or one that overlaps a cell placed before it, is skipped and gets `Rect::default()`. A
  shared budget caps the slot reads, so placement work stays bounded however many cells the spec lists. A span
  of 0 counts as 1, and a span past the grid's edge is clamped.
- **Columns.** A column's min and max are the largest min and max content of its single-column cells. For
  multi-column cells, narrower spans first, the excess of a cell's min over the sum of its columns' mins goes
  to those columns in proportion to their current max (evenly when those are all 0). Then the max excess is
  spread the same way, and each column's max is raised to at least its min.
- **Width.** When the available width is no more than the sum of the mins plus the gaps, the columns get their
  mins and the table overflows. When it is at least the sum of the maxes, they get their maxes and the rest is
  left unused. In between, each column moves the same fraction of the way from its min to its max.
- **Rows.** A row's height is its tallest single-row cell. A multi-row cell's excess height is split evenly
  over its rows.
- **Rects.** Cells are placed by prefix sums. `border_spacing_x` and `border_spacing_y` are inserted only
  between columns and rows, not at the table's outer edge. A spanning cell's rect covers its columns or rows and
  the gaps between them.
- **Totality.** Every numeric input is clamped (NaN, infinite or negative becomes 0), a cell's max is raised
  to its min, and every output float is clamped again, so nothing non-finite leaks out.

## Tests

The inline module `layout::table::tests` in `src/layout/table.rs` (20 tests), and `tests/table_grid.rs`
(10 tests of `place_grid`). The full table path is in `tests/layout_table.rs` and the `table_*_golden` tests
(L04).

## Changes

none: imported

## Gate

- **L03-AC-1** — In a 2×2 table with room to spare, each column is as wide as its widest cell's max content, and the cells tile the columns and rows. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::simple_2x2_fixed_widths_tile_correctly layout::table::tests::over_constrained_columns_get_max_no_fill` exits 0.
- **L03-AC-2** — When the available width is below the sum of the columns' min widths, the columns get their mins. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::under_constrained_columns_get_min_and_overflow` exits 0.
- **L03-AC-3** — When the available width exceeds the sum of the max widths, the columns get their maxes and the leftover width is not handed out. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::over_constrained_columns_get_max_no_fill` exits 0.
- **L03-AC-4** — Between the two sums, each column moves the same fraction of the way from its min to its max. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::in_between_distributes_slack_proportionally` exits 0.
- **L03-AC-5** — A spanning cell's excess min width goes to its columns in proportion to their max widths, and so does its excess max width. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::colspan_min_excess_distributes_proportionally_to_max layout::table::tests::colspan_max_excess_distributes_proportionally_to_max` exits 0.
- **L03-AC-6** — When a spanning cell's min excess pushes a column's min above its max, the max is raised to the min. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::colspan_min_excess_clamps_max_when_it_would_undercut_min` exits 0.
- **L03-AC-7** — A row-spanning cell's excess height is split evenly over its rows, and its rect covers them. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::rowspan_distributes_excess_into_row_heights` exits 0.
- **L03-AC-8** — Border spacing goes only between columns and rows, and the first cell sits at (0, 0). *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::border_spacing_offsets_rects` exits 0.
- **L03-AC-9** — A cell outside the grid gets a zero rect, and a table with no columns has no column widths and zero rects. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::out_of_range_col_row_never_panics layout::table::tests::zero_columns_never_panics` exits 0.
- **L03-AC-10** — A `colspan` or `rowspan` of 0 counts as 1, and one past the grid's edge is clamped to it. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::degenerate_colspans_clamp_not_panic layout::table::tests::degenerate_rowspans_clamp_not_panic` exits 0.
- **L03-AC-11** — Of two overlapping cells, the earlier one is placed and the later one gets a zero rect. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::overlapping_cells_skip_the_later_one` exits 0.
- **L03-AC-12** — Negative and NaN inputs are clamped, and inputs near `f32::MAX` still give finite, non-negative outputs. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::nan_and_negative_inputs_never_panic_and_stay_finite layout::table::tests::extreme_magnitudes_scrub_to_finite_output` exits 0.
- **L03-AC-13** — A cell that spans columns and rows covers the summed tracks: in a 3×3 grid of 10px tracks with no spacing, a 2×2 cell is 20×20. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::table::tests::combined_colspan_and_rowspan_tiles_correctly` exits 0.
- **L03-AC-14** — `place_grid` places a 2×2 table's cells at their columns and rows, and a `colspan` cell takes several columns, moving the next cell along. *Verify:* `cargo test --locked --jobs 2 --test table_grid -- --exact --test-threads=2 plain_2x2_places_cells_at_expected_positions colspan_cell_occupies_multiple_columns` exits 0.
- **L03-AC-15** — The rows of `thead` and `tbody` groups flatten into one grid. *Verify:* `cargo test --locked --jobs 2 --test table_grid -- --exact --test-threads=2 header_and_body_row_groups_flatten_into_one_grid` exits 0.
- **L03-AC-16** — A `rowspan` cell reserves its column in the next rows, so the next row's first cell moves past it. *Verify:* `cargo test --locked --jobs 2 --test table_grid -- --exact --test-threads=2 rowspan_cell_reserves_column_in_next_row` exits 0.
- **L03-AC-17** — A row child that is not a table cell is skipped. *Verify:* `cargo test --locked --jobs 2 --test table_grid -- --exact --test-threads=2 stray_non_cell_children_in_a_row_are_skipped` exits 0.
- **L03-AC-18** — `place_grid` on a table of 20,000 rows places at most `MAX_GRID_DIM` (4,096) of them. *Verify:* `cargo test --locked --jobs 2 --test table_grid -- --exact --test-threads=2 many_rows_completes_promptly_and_stays_bounded` exits 0.
- **L03-AC-19** — A row is as tall as its tallest single-row cell. `place_grid` clamps a span to `MAX_GRID_DIM` from the cell's origin. The grid area cap of 262,144 slots and the placement budget hold in both functions, and a budget running out leaves the remaining cells unplaced. `solve_table` handles spanning cells in ascending span order, and clamps each output column width and row height again. *Verify:* unpinned (the 2×2 test's cells grow in order; `huge_spans_clamp_and_never_panic` only bounds the grid below 100,000; the extreme-magnitude test overflows only in a cell rect).

## Risks

- `table.rs`'s module doc still says the solver "is not wired into `layout()` yet" and that its only caller is
  its own tests. That is stale: L04 drives it for every `display: table` box.
- Spare width is never handed out and border spacing is not added at the outer edge. Both are documented
  simplifications of CSS's separated-borders model, so a table is narrower than a real UA draws it.
- The two caps are written out twice, once in each file, and can drift apart.
- A row's excess height is split evenly, not by content, so a tall `rowspan` cell can make short rows taller
  than a real UA would.
