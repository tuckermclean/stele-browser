//! Client-side image maps (charter K2): `<img usemap>` + `<map>`/`<area>`.
//!
//! Everything here is pure and unit-tested — no DOM, no layout tree. Two
//! halves:
//!   - [`parse_area`]: turn one `<area>` element's raw `shape`/`coords`/
//!     `href` attribute strings into an [`Area`], per HTML 4.01 §13.6.1.
//!   - [`hit_test`] / [`hit_test_scaled`]: given a point, find the first
//!     (document-order) [`Area`] that contains it.
//!
//! `layout::box_tree` resolves `<img usemap="#name">` against a `<map name>`
//! found anywhere in the document, parses its `<area>` children with
//! [`parse_area`], and carries the result as `layout::Interactive::ImageMap`.
//! `backend::x11::hit_test_pixel` and the tty shell's `browser::enter_command`
//! consume it at click/activation time.

/// An `<area>`'s shape kind (HTML 4.01 §13.6.1, `shape` attribute).
/// Case-insensitive on the wire (`parse_area` lowercases before matching);
/// `Rect` is both the explicit `"rect"`/`"rectangle"` value AND the default
/// when `shape` is absent or unrecognized — the spec's own fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Circle,
    Poly,
    /// The whole image — matches any point, `coords` ignored/absent.
    Default,
}

/// One parsed, already-validated `<area>` region. `coords` is empty for
/// [`Shape::Default`]; for every other shape it has already been checked
/// against that shape's required count (`parse_area` returns `None` instead
/// of constructing an `Area` with a wrong-sized or unparseable `coords`, so
/// every `Area` downstream is safe to index without bounds checks matching
/// its own `shape`). `href` is `None` for a `nohref`/href-less `<area>` — a
/// documented dead zone: `hit_test`/`hit_test_scaled` still return it as the
/// first-match, but a caller that resolves a navigation target sees no href
/// to follow (see `backend::x11::hit_test_pixel`'s and `browser::
/// enter_command`'s own handling).
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub shape: Shape,
    pub coords: Vec<f32>,
    pub href: Option<Box<str>>,
}

/// Parse one `<area shape=".." coords=".." href="..">`'s raw attribute
/// strings into an [`Area`], per HTML 4.01 §13.6.1.
///
/// - `shape` is matched case-insensitively; `"rect"`/`"rectangle"` ->
///   [`Shape::Rect`], `"circle"`/`"circ"` -> [`Shape::Circle`], `"poly"`/
///   `"polygon"` -> [`Shape::Poly`], `"default"` -> [`Shape::Default`]; a
///   missing or unrecognized value falls back to `Rect` (the spec's own
///   default), never to `Default` — an author who mistypes `shape` gets a
///   (likely malformed, then dropped) rect, not an accidental whole-image
///   catch-all.
/// - `coords` is a comma-separated list of numbers; whitespace around each
///   number is trimmed. A double comma, trailing comma, or non-numeric token
///   is a parse failure.
/// - **Malformed `coords` drops the area — it is never widened to
///   [`Shape::Default`].** `Rect` needs exactly 4 numbers, `Circle` exactly 3
///   (with a non-negative radius), `Poly` an even count of at least 6 (>= 3
///   points). Anything else (wrong count, unparseable number, a parse
///   failure in the comma-split) returns `None`: the `<area>` contributes no
///   hit-test region at all, rather than silently becoming a whole-image
///   default that would swallow every click. `Shape::Default` needs no
///   `coords` and is unaffected by anything in the attribute (even a
///   malformed one — it's simply never consulted).
pub fn parse_area(shape: Option<&str>, coords: Option<&str>, href: Option<&str>) -> Option<Area> {
    let shape_kind = match shape.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("circle") | Some("circ") => Shape::Circle,
        Some("poly") | Some("polygon") => Shape::Poly,
        Some("default") => Shape::Default,
        _ => Shape::Rect,
    };

    let coords_vals = match parse_coords(coords) {
        Ok(vals) => vals,
        Err(()) => return None,
    };

    match shape_kind {
        Shape::Rect if coords_vals.len() != 4 => return None,
        Shape::Circle if coords_vals.len() != 3 || coords_vals[2] < 0.0 => return None,
        Shape::Poly if coords_vals.len() < 6 || coords_vals.len() % 2 != 0 => return None,
        _ => {}
    }

    Some(Area {
        shape: shape_kind,
        coords: coords_vals,
        href: href.map(|h| h.into()),
    })
}

/// Split `coords` on commas and parse each trimmed token as `f32`. `None`
/// (attribute absent) or an all-whitespace/empty string is `Ok(vec![])` —
/// legitimate for [`Shape::Default`], which never checks `coords.len()`.
/// Anything with an empty token (double/leading/trailing comma) or a token
/// that doesn't parse as a finite number is `Err(())` — a hard parse failure,
/// which [`parse_area`] turns into "drop the area", never a partial/widened
/// result.
fn parse_coords(coords: Option<&str>) -> Result<Vec<f32>, ()> {
    let Some(s) = coords else { return Ok(Vec::new()) };
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut vals = Vec::new();
    for tok in s.split(',') {
        let t = tok.trim();
        if t.is_empty() {
            return Err(());
        }
        match t.parse::<f32>() {
            Ok(v) if v.is_finite() => vals.push(v),
            _ => return Err(()),
        }
    }
    Ok(vals)
}

/// First-match-wins: the earliest (document/`<area>`-child order) area in
/// `areas` whose region contains `(x, y)`, in whatever coordinate space both
/// `areas` and the point already agree on (natural image-space — see
/// [`hit_test_scaled`] for the rendered-fragment-space wrapper around this).
/// `None` when nothing contains the point, including when `areas` is empty.
pub fn hit_test(areas: &[Area], x: f32, y: f32) -> Option<&Area> {
    areas.iter().find(|a| area_contains(a, x, y))
}

/// [`hit_test`], but the point AND `areas` live in different spaces: `(x,
/// y)` is in RENDERED-fragment space (the same layout-pixel coordinate space
/// as `rendered`, e.g. an `<img>`'s own `Fragment::rect`), while `areas`'
/// `coords` are authored in the image's NATURAL (decoded, pre-scaling) pixel
/// space — an author writes `coords` against the real image dimensions, but
/// CSS/HTML can render that `<img>` at any other size. Scales the point into
/// natural space before delegating to [`hit_test`].
///
/// `None` — never a panic — when `(x, y)` falls outside `rendered`'s bounds,
/// or when `rendered` has zero (or negative, which cannot legitimately occur
/// but is still guarded) width/height: a collapsed rendered box has no
/// meaningful natural-space point to scale to, so this treats every point as
/// a miss rather than dividing by zero.
pub fn hit_test_scaled(areas: &[Area], natural: crate::layout::Size, rendered: crate::layout::Rect, x: f32, y: f32) -> Option<&Area> {
    if rendered.size.w <= 0.0 || rendered.size.h <= 0.0 {
        return None;
    }
    if x < rendered.origin.x || x >= rendered.origin.x + rendered.size.w {
        return None;
    }
    if y < rendered.origin.y || y >= rendered.origin.y + rendered.size.h {
        return None;
    }
    let local_x = x - rendered.origin.x;
    let local_y = y - rendered.origin.y;
    let natural_x = local_x * (natural.w / rendered.size.w);
    let natural_y = local_y * (natural.h / rendered.size.h);
    hit_test(areas, natural_x, natural_y)
}

/// Does `area`'s region contain `(x, y)`? `Rect`/`Circle` boundaries are
/// explicitly inclusive (a point exactly on the edge counts as inside — the
/// natural reading of "the rectangle from (x1,y1) to (x2,y2)" and "points
/// within radius r"). `Poly` uses the standard even-odd ray-casting test,
/// whose boundary behavior is not specially handled (a point exactly on a
/// polygon edge may go either way depending on which edge) — acceptable
/// here since HTML `coords` are integer-ish pixel hints, not a precision
/// contract. `Default` always matches (the whole image).
fn area_contains(area: &Area, x: f32, y: f32) -> bool {
    match area.shape {
        Shape::Rect => {
            let [x1, y1, x2, y2] = [area.coords[0], area.coords[1], area.coords[2], area.coords[3]];
            let (min_x, max_x) = (x1.min(x2), x1.max(x2));
            let (min_y, max_y) = (y1.min(y2), y1.max(y2));
            x >= min_x && x <= max_x && y >= min_y && y <= max_y
        }
        Shape::Circle => {
            let (cx, cy, r) = (area.coords[0], area.coords[1], area.coords[2]);
            let dx = x - cx;
            let dy = y - cy;
            dx * dx + dy * dy <= r * r
        }
        Shape::Poly => point_in_polygon(&area.coords, x, y),
        Shape::Default => true,
    }
}

/// Standard even-odd ray-casting point-in-polygon test. `coords` is
/// `[x0, y0, x1, y1, ..., xn-1, yn-1]` — already validated by `parse_area`
/// to have an even length >= 6 (>= 3 points) for any `Area` this is called
/// on, so the indexing below never goes out of bounds.
fn point_in_polygon(coords: &[f32], x: f32, y: f32) -> bool {
    let n = coords.len() / 2;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (coords[2 * i], coords[2 * i + 1]);
        let (xj, yj) = (coords[2 * j], coords[2 * j + 1]);
        if (yi > y) != (yj > y) {
            let x_intersect = (xj - xi) * (y - yi) / (yj - yi) + xi;
            if x < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Point, Rect, Size};

    // -----------------------------------------------------------------
    // parse_area
    // -----------------------------------------------------------------

    #[test]
    fn rect_shape_parses_with_four_coords() {
        let a = parse_area(Some("rect"), Some("10,10,50,50"), Some("/x")).unwrap();
        assert_eq!(a.shape, Shape::Rect);
        assert_eq!(a.coords, vec![10.0, 10.0, 50.0, 50.0]);
        assert_eq!(a.href.as_deref(), Some("/x"));
    }

    #[test]
    fn shape_is_case_insensitive() {
        assert_eq!(parse_area(Some("CIRCLE"), Some("5,5,5"), None).unwrap().shape, Shape::Circle);
        assert_eq!(parse_area(Some("Poly"), Some("0,0,10,0,5,10"), None).unwrap().shape, Shape::Poly);
        assert_eq!(parse_area(Some("DEFAULT"), None, None).unwrap().shape, Shape::Default);
    }

    #[test]
    fn missing_shape_defaults_to_rect() {
        let a = parse_area(None, Some("1,2,3,4"), None).unwrap();
        assert_eq!(a.shape, Shape::Rect);
    }

    #[test]
    fn unrecognized_shape_falls_back_to_rect_not_default() {
        let a = parse_area(Some("hexagon"), Some("1,2,3,4"), None).unwrap();
        assert_eq!(a.shape, Shape::Rect);
    }

    #[test]
    fn rect_with_wrong_coord_count_is_dropped_not_widened() {
        assert!(parse_area(Some("rect"), Some("1,2,3"), Some("/x")).is_none());
        assert!(parse_area(Some("rect"), Some("1,2,3,4,5"), Some("/x")).is_none());
    }

    #[test]
    fn circle_with_wrong_coord_count_is_dropped() {
        assert!(parse_area(Some("circle"), Some("1,2"), Some("/x")).is_none());
    }

    #[test]
    fn circle_with_negative_radius_is_dropped() {
        assert!(parse_area(Some("circle"), Some("5,5,-1"), Some("/x")).is_none());
    }

    #[test]
    fn poly_needs_at_least_three_points_and_even_count() {
        assert!(parse_area(Some("poly"), Some("0,0,10,0"), Some("/x")).is_none()); // only 2 points
        assert!(parse_area(Some("poly"), Some("0,0,10,0,5"), Some("/x")).is_none()); // odd count
        assert!(parse_area(Some("poly"), Some("0,0,10,0,5,10"), Some("/x")).is_some());
    }

    #[test]
    fn default_shape_ignores_missing_or_malformed_coords() {
        assert!(parse_area(Some("default"), None, Some("/x")).is_some());
        assert!(parse_area(Some("default"), Some("not,numbers"), Some("/x")).is_some());
    }

    #[test]
    fn malformed_coords_token_is_dropped() {
        assert!(parse_area(Some("rect"), Some("1,,3,4"), Some("/x")).is_none());
        assert!(parse_area(Some("rect"), Some("1,two,3,4"), Some("/x")).is_none());
        assert!(parse_area(Some("rect"), Some("1,2,3,"), Some("/x")).is_none());
    }

    #[test]
    fn nohref_area_still_parses() {
        let a = parse_area(Some("rect"), Some("0,0,10,10"), None).unwrap();
        assert_eq!(a.href, None);
    }

    #[test]
    fn empty_coords_string_is_missing_for_rect() {
        assert!(parse_area(Some("rect"), Some(""), Some("/x")).is_none());
    }

    // -----------------------------------------------------------------
    // hit_test: rect
    // -----------------------------------------------------------------

    fn area(shape: Shape, coords: &[f32], href: &str) -> Area {
        Area { shape, coords: coords.to_vec(), href: Some(href.into()) }
    }

    #[test]
    fn rect_hit_test_is_boundary_inclusive() {
        let a = area(Shape::Rect, &[10.0, 10.0, 20.0, 20.0], "/r");
        assert!(area_contains(&a, 10.0, 10.0)); // top-left corner
        assert!(area_contains(&a, 20.0, 20.0)); // bottom-right corner
        assert!(area_contains(&a, 15.0, 10.0)); // top edge
        assert!(!area_contains(&a, 9.9, 15.0));
        assert!(!area_contains(&a, 20.1, 15.0));
    }

    #[test]
    fn rect_hit_test_handles_reversed_coords() {
        // Authors sometimes write x2,y2 < x1,y1 — still a valid rectangle.
        let a = area(Shape::Rect, &[20.0, 20.0, 10.0, 10.0], "/r");
        assert!(area_contains(&a, 15.0, 15.0));
    }

    // -----------------------------------------------------------------
    // hit_test: circle
    // -----------------------------------------------------------------

    #[test]
    fn circle_hit_test_edge_is_inclusive() {
        let a = area(Shape::Circle, &[0.0, 0.0, 10.0], "/c");
        assert!(area_contains(&a, 10.0, 0.0)); // exactly on the edge
        assert!(area_contains(&a, 0.0, 0.0)); // center
        assert!(!area_contains(&a, 10.1, 0.0));
    }

    // -----------------------------------------------------------------
    // hit_test: polygon
    // -----------------------------------------------------------------

    #[test]
    fn polygon_hit_test_inside_and_outside() {
        // A triangle (0,0)-(10,0)-(5,10).
        let a = area(Shape::Poly, &[0.0, 0.0, 10.0, 0.0, 5.0, 10.0], "/p");
        assert!(area_contains(&a, 5.0, 1.0)); // clearly inside
        assert!(!area_contains(&a, 5.0, -1.0)); // above the triangle
        assert!(!area_contains(&a, -1.0, 1.0)); // left of the triangle
        assert!(!area_contains(&a, 20.0, 20.0)); // far outside
    }

    // -----------------------------------------------------------------
    // hit_test: default + overlap precedence
    // -----------------------------------------------------------------

    #[test]
    fn default_shape_matches_any_point() {
        let a = area(Shape::Default, &[], "/d");
        assert!(area_contains(&a, 0.0, 0.0));
        assert!(area_contains(&a, -1000.0, 999.0));
    }

    #[test]
    fn hit_test_first_match_wins_on_overlap() {
        let areas = vec![
            area(Shape::Rect, &[0.0, 0.0, 100.0, 100.0], "/first"),
            area(Shape::Rect, &[10.0, 10.0, 50.0, 50.0], "/second"),
        ];
        // Both areas contain (20, 20) — the FIRST in document order wins,
        // even though the second is geometrically "smaller"/"on top".
        let hit = hit_test(&areas, 20.0, 20.0).unwrap();
        assert_eq!(hit.href.as_deref(), Some("/first"));
    }

    #[test]
    fn hit_test_returns_none_when_nothing_matches() {
        let areas = vec![area(Shape::Rect, &[0.0, 0.0, 10.0, 10.0], "/r")];
        assert!(hit_test(&areas, 50.0, 50.0).is_none());
    }

    #[test]
    fn hit_test_on_empty_areas_returns_none() {
        assert!(hit_test(&[], 5.0, 5.0).is_none());
    }

    // -----------------------------------------------------------------
    // hit_test_scaled: natural vs rendered space
    // -----------------------------------------------------------------

    #[test]
    fn hit_test_scaled_maps_rendered_point_into_natural_space() {
        // A 16x16 natural image rendered at 48x16 (3x horizontal scale only).
        let areas = vec![area(Shape::Rect, &[0.0, 0.0, 8.0, 16.0], "/left")];
        let natural = Size { w: 16.0, h: 16.0 };
        let rendered = Rect { origin: Point { x: 100.0, y: 200.0 }, size: Size { w: 48.0, h: 16.0 } };
        // Rendered-space x=100+20=120 -> local 20 -> natural 20/48*16 ≈ 6.67, inside [0,8].
        assert!(hit_test_scaled(&areas, natural, rendered, 120.0, 208.0).is_some());
        // Rendered-space x=100+40=140 -> local 40 -> natural 40/48*16 ≈ 13.3, outside [0,8].
        assert!(hit_test_scaled(&areas, natural, rendered, 140.0, 208.0).is_none());
    }

    #[test]
    fn hit_test_scaled_accounts_for_fragment_origin_offset() {
        let areas = vec![area(Shape::Rect, &[0.0, 0.0, 10.0, 10.0], "/a")];
        let natural = Size { w: 10.0, h: 10.0 };
        let rendered = Rect { origin: Point { x: 50.0, y: 50.0 }, size: Size { w: 10.0, h: 10.0 } };
        assert!(hit_test_scaled(&areas, natural, rendered, 55.0, 55.0).is_some());
        // Same natural-space point, but without the origin offset applied —
        // a point outside the rendered box entirely.
        assert!(hit_test_scaled(&areas, natural, rendered, 5.0, 5.0).is_none());
    }

    #[test]
    fn hit_test_scaled_out_of_rendered_bounds_returns_none() {
        let areas = vec![area(Shape::Default, &[], "/d")];
        let natural = Size { w: 10.0, h: 10.0 };
        let rendered = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: 10.0, h: 10.0 } };
        assert!(hit_test_scaled(&areas, natural, rendered, 10.1, 5.0).is_none());
        assert!(hit_test_scaled(&areas, natural, rendered, -0.1, 5.0).is_none());
    }

    #[test]
    fn hit_test_scaled_zero_rendered_size_does_not_panic() {
        let areas = vec![area(Shape::Default, &[], "/d")];
        let natural = Size { w: 10.0, h: 10.0 };
        let rendered = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: 0.0, h: 0.0 } };
        assert!(hit_test_scaled(&areas, natural, rendered, 0.0, 0.0).is_none());
    }

    #[test]
    fn hit_test_scaled_zero_natural_size_does_not_panic() {
        // A decoded-size-unavailable fallback (natural == 0x0): scaling
        // collapses every point to natural-space (0,0), never divides by
        // zero (the guard is on `rendered`'s size, not `natural`'s).
        let areas = vec![area(Shape::Default, &[], "/d")];
        let natural = Size { w: 0.0, h: 0.0 };
        let rendered = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: 20.0, h: 20.0 } };
        assert!(hit_test_scaled(&areas, natural, rendered, 10.0, 10.0).is_some());
    }
}
