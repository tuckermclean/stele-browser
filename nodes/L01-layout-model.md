# L01 — layout-model

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | S02, I01, T03 | L02, L03, L04, L05 | S |

**Owns:** `src/layout/mod.rs`, `src/layout/image_map.rs`

## Purpose

Imported from the code. `stele::layout` is the seam between the styled DOM and positioned fragments. Its
module root holds the data model every other layout file and every painter shares, the two entry points, and
the scroll-to lookup. `layout::image_map` holds the client-side image maps (charter K2), because their parsed
areas ride on that model.

The model, in `src/layout/mod.rs`:

- `Size`, `Point` and `Rect`, in CSS pixels before device scaling, with the origin at the top left.
- `LayoutNode`, the input to layout: a `ComputedStyle`, a `BoxContent`, children, an optional `Interactive`
  and an optional `id` (the element's `id` attribute, trimmed but not lowercased).
- `BoxContent`: `Container`; `Text(String)`; `Replaced { intrinsic, image }`, where `image` is an
  `Rc<RgbaImage>` when decoded, else `None` and painted as a placeholder; and `TableCell { colspan, rowspan }`,
  with spans that L05 has already clamped.
- `Interactive`, the provenance carried onto fragments: `Link { href }` (raw, not resolved),
  `FormControl { kind, name, form_action }`, and `ImageMap { areas, natural }`. `areas` is an `Rc<[Area]>` in
  document order. `natural` is the image's decoded pixel size, which the `coords` are written against.
- `Fragment`, the output: a `rect`, a `FragmentKind`, `interactive`, `clip` (the clip rect of the nearest
  `overflow: hidden` ancestors), `id` and `is_fixed`. `FragmentKind` is `Box { style }`,
  `Text { text, baseline, style }` or `Image { image }`.

The entry points:

- `layout(root, viewport)` lays the tree out with `text::TerminusFont` metrics through L04's
  `block::layout_tree`, at the content's height.
- `layout_viewport(root, viewport)` calls `block::layout_tree_viewport` instead, which fixes the root's height
  to `viewport.h`. `--viewport-height` uses it.
- `find_fragment_top(fragments, id)` resolves `--scroll-to <id>`. It takes the first `Box` fragment whose `id`
  equals `id` and returns its padding-top edge: `rect.origin.y` plus that fragment's top border width,
  clamped finite and non-negative through `block::finite_nonneg`. It returns `None` when no fragment carries
  the id.

The image maps, in `src/layout/image_map.rs`:

- `parse_area(shape, coords, href)` builds an `Area { shape, coords, href }` from an `<area>`'s raw
  attributes, per HTML 4.01 §13.6.1. `shape` is trimmed and lowercased: `circle`/`circ` give `Circle`,
  `poly`/`polygon` give `Poly`, `default` gives `Default`, and anything else, missing included, gives `Rect`.
  `coords` is split on commas and each token is trimmed and parsed. An empty token or a token that is not a
  finite number drops the area. `Rect` needs exactly 4 numbers, `Circle` exactly 3 with a radius of at least 0,
  and `Poly` an even count of at least 6; any other count drops the area (`None`), and it is never widened to
  `Default`. `Default` ignores `coords`. `href` is kept as written; without it, it is `None`.
- `hit_test(areas, x, y)` returns the first area, in order, that contains the point. A `Rect` includes its
  edges and accepts its corners in either order, a `Circle` includes its edge, a `Poly` uses the even-odd
  ray-casting test, and `Default` contains every point.
- `hit_test_scaled(areas, natural, rendered, x, y)` takes a point in rendered fragment space. It returns
  `None` when the rendered rect has no width or height, or when the point lies outside it (the left and top
  edges are inside, the right and bottom edges outside). Otherwise it takes the point relative to the rect's
  origin, scales each axis by natural over rendered, and calls `hit_test`.

L05 builds the `ImageMap` carrier. `backend::x11` and the tty shell `browser` hit-test it.

## Tests

The inline module `layout::image_map::tests` in `src/layout/image_map.rs` (25 tests: `parse_area` and the hit
tests) and `layout::find_fragment_top_tests` in `src/layout/mod.rs` (3 tests, run through `layout`). The entry
points are exercised by every layout test, for example `tests/layout_block.rs`, whose
`layout_viewport_*` tests cover `layout_viewport`.

## Changes

none: imported

## Gate

- **L01-AC-1** — `parse_area` matches `shape` case-insensitively: `CIRCLE`, `Poly` and `DEFAULT` give `Circle`, `Poly` and `Default`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::shape_is_case_insensitive` exits 0.
- **L01-AC-2** — A missing or unrecognized `shape` (`hexagon`) parses as `Rect`, not `Default`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::missing_shape_defaults_to_rect layout::image_map::tests::unrecognized_shape_falls_back_to_rect_not_default` exits 0.
- **L01-AC-3** — A `rect` with 3 or 5 coordinates is dropped. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::rect_with_wrong_coord_count_is_dropped_not_widened` exits 0.
- **L01-AC-4** — A `circle` with 2 coordinates, or with a negative radius, is dropped. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::circle_with_wrong_coord_count_is_dropped layout::image_map::tests::circle_with_negative_radius_is_dropped` exits 0.
- **L01-AC-5** — A `poly` with fewer than 6 numbers (fewer than three points) is dropped, and one with 6 parses. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::poly_needs_at_least_three_points_and_even_count` exits 0.
- **L01-AC-6** — A `default` area parses with missing or malformed `coords`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::default_shape_ignores_missing_or_malformed_coords` exits 0.
- **L01-AC-7** — An empty token (`1,,3,4`, `1,2,3,`) or a non-numeric token (`1,two,3,4`) in `coords` drops the area. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::malformed_coords_token_is_dropped` exits 0.
- **L01-AC-8** — `href` is kept as written, and an area without one still parses, with `href` `None`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::rect_shape_parses_with_four_coords layout::image_map::tests::nohref_area_still_parses` exits 0.
- **L01-AC-9** — A `Rect` contains its corners and edges, and coordinates given bottom-right first still describe the same rectangle. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::rect_hit_test_is_boundary_inclusive layout::image_map::tests::rect_hit_test_handles_reversed_coords` exits 0.
- **L01-AC-10** — A `Circle` contains the points on its edge. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::circle_hit_test_edge_is_inclusive` exits 0.
- **L01-AC-11** — A `Poly` contains a point inside the triangle, and not a point to its left, above it or far away. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::polygon_hit_test_inside_and_outside` exits 0.
- **L01-AC-12** — `Default` contains every point. Where two areas contain the point, the first in document order wins. When none contains it, `hit_test` returns `None`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::default_shape_matches_any_point layout::image_map::tests::hit_test_first_match_wins_on_overlap layout::image_map::tests::hit_test_returns_none_when_nothing_matches` exits 0.
- **L01-AC-13** — `hit_test_scaled` takes the point relative to the rendered rect's origin and scales it by natural over rendered size before testing. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::hit_test_scaled_maps_rendered_point_into_natural_space layout::image_map::tests::hit_test_scaled_accounts_for_fragment_origin_offset` exits 0.
- **L01-AC-14** — `hit_test_scaled` returns `None` for a point outside the rendered rect, even over a `Default` area, and for a rendered rect of zero size. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::image_map::tests::hit_test_scaled_out_of_rendered_bounds_returns_none layout::image_map::tests::hit_test_scaled_zero_rendered_size_does_not_panic` exits 0.
- **L01-AC-15** — `find_fragment_top` returns the target box's padding-top edge: 4px below the border-box top for a 4px top border, and the border-box top without a border. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::find_fragment_top_tests::resolves_the_padding_top_edge_past_a_nonzero_border layout::find_fragment_top_tests::zero_border_target_resolves_to_the_border_box_top` exits 0.
- **L01-AC-16** — `find_fragment_top` returns `None` for an id that no fragment carries. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 layout::find_fragment_top_tests::missing_id_returns_none_without_panicking` exits 0.
- **L01-AC-17** — `layout_viewport` lays the tree out at the fixed viewport height: a tall tree's root fragment is 120px high in a 200×120 viewport. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --exact --test-threads=2 layout_viewport_clamps_the_root_fragment_height_to_the_given_viewport_height` exits 0.
- **L01-AC-18** — `find_fragment_top` considers only `Box` fragments, takes the first of several with the same id, and clamps a negative or non-finite top border to 0. A `poly` with an odd count of 7 or more numbers is dropped. `layout` and `layout_viewport` measure text with `TerminusFont`. *Verify:* unpinned.

## Risks

- `layout`, `layout_viewport` and `find_fragment_top` call into `layout::block` (L04), which depends on this
  node for the model. The graph cannot show that edge without a cycle, so a change to L04 can break this
  node's tests.
- The module doc and several field doc comments describe packet history ("this packet only lands the
  carrier", "Tables (P8) are not yet part of this pipeline"), and some of it is stale: tables are laid out
  today (L04).
- `hit_test_scaled` divides by the rendered size only after the zero-size guard. A zero `natural` size maps
  every point to (0, 0) instead of failing.
- Polygon edges are not handled specially: a point exactly on an edge can fall either way.
- `Fragment` carries `ComputedStyle` by value on every box and text run, which costs memory on large pages.
