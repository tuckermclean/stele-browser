// SPDX-License-Identifier: GPL-3.0-or-later

//! packet/css-grid golden: the real parse->cascade->box-tree->layout->
//! raster->PNG pipeline (same wiring `stele --headless --dump-png` drives in
//! `main.rs`, minus the fetch hop -- `fixtures/grid.html` is entirely
//! self-contained, `<style>` and all, so `include_str!` matches `tests/
//! css1_float_golden.rs`'s own convention) run against a realistic
//! `repeat(auto-fill, minmax(200px, 1fr))` card grid, asserted exact (by
//! decoded RGBA pixels) against a checked-in golden image.
//!
//! `fixtures/grid.html` is a 6-card grid at an 800px viewport with 20px body
//! padding and a 16px gap; at that width `repeat(auto-fill, minmax(200px,
//! 1fr))` places 3 columns (760px content width; `floor((760 - 200) / (200 +
//! 16)) + 1 = 3` -- see `layout::block::apply_grid`'s doc comment and
//! `tests/layout_block.rs`'s `grid_auto_fill_minmax_computes_the_correct_
//! column_count_at_800px` for the same auto-repeat formula hand-verified at
//! a different width/track-size), so the 6 cards land as two rows of 3, NOT
//! stacked. This is a PROPOSED golden (brief §10 blessing discipline, same
//! as every other golden here): the implementer blesses it only after
//! PIXEL-VERIFYING the multi-column structure programmatically (this
//! project's own "verify goldens with pixel analysis" discipline -- never by
//! eyeballing), never self-trusting it; the orchestrator/reviewer
//! countersigns.

use std::collections::HashMap;

use stele::backend::raster;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::{self, cascade};
use stele::surface::{Color, MemSurface};

const GRID_HTML: &str = include_str!("../fixtures/grid.html");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/grid.png");
const VIEWPORT_WIDTH: u32 = 800;

/// The 6 cards' own `background` colors (`fixtures/grid.html`'s `.c1`..
/// `.c6` rules), in document order.
const CARD_COLORS: [(u8, u8, u8); 6] = [
    (0xff, 0xd0, 0xd0),
    (0xd0, 0xff, 0xd6),
    (0xd0, 0xe0, 0xff),
    (0xff, 0xf2, 0xc0),
    (0xe6, 0xd0, 0xff),
    (0xd0, 0xff, 0xf2),
];

/// Render `html` to PNG bytes via the same pipeline `main.rs`'s `dump_png`
/// drives (fetch hop excluded -- matches `tests/css1_float_golden.rs`'s own
/// `render_png` helper). The fixture's entire layout lives in its inline
/// `<style>` block, so the author sheets MUST be collected.
fn render_png(html: &str) -> Vec<u8> {
    let dom_tree = dom::parser::parse(html);
    let author_sheets =
        style::collect_author_sheets_for_viewport(&dom_tree, VIEWPORT_WIDTH as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return raster::encode_png(&MemSurface::new(1, 1, Color::WHITE));
    };
    let viewport = Size { w: VIEWPORT_WIDTH as f32, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);

    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let (y, h) = (f.rect.origin.y, f.rect.size.h);
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let height = if content_bottom > 0.0 { content_bottom.ceil() as u32 } else { 1 };

    let mut surface = MemSurface::new(VIEWPORT_WIDTH, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &HashMap::new(), Color::WHITE);
    raster::encode_png(&surface)
}

/// Decode PNG bytes to `(width, height, rgba_pixels)`.
fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("valid PNG frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// The first (x, y) pixel in `px` (a `w`x`h` RGBA buffer) within `tolerance`
/// of `target`, scanning row-major from the top-left -- `None` if no such
/// pixel exists.
fn find_pixel(px: &[u8], w: u32, h: u32, target: (u8, u8, u8), tolerance: i32) -> Option<(u32, u32)> {
    let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= tolerance;
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            if idx + 2 >= px.len() {
                continue;
            }
            let (r, g, b) = (px[idx], px[idx + 1], px[idx + 2]);
            if close(r, target.0) && close(g, target.1) && close(b, target.2) {
                return Some((x, y));
            }
        }
    }
    None
}

#[test]
fn grid_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(GRID_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);

    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/grid.png)");
}

#[test]
fn golden_shows_well_formed_nontrivial_pixels() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    assert!(px.chunks(4).any(|p| p != [255, 255, 255, 255]), "golden should not be blank");
}

/// Structural-fidelity check (not a full pixel-diff -- that's `grid_fixture_
/// png_matches_golden_pixels` above): the first three cards (`.c1`/`.c2`/
/// `.c3`) must appear in the SAME row (comparable y) at STRICTLY INCREASING
/// x -- i.e. actually laid out side by side in row 1 of the grid, not
/// stacked one per row (which pre-grid-support `display: grid` handling,
/// silently ignored, would have produced: every card full body-content-width
/// and one per row, no two cards sharing a y at all).
#[test]
fn golden_shows_first_three_cards_side_by_side_in_row_one() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);

    let (x1, y1) = find_pixel(&px, w, h, CARD_COLORS[0], 4).expect("card 1's background color must appear somewhere");
    let (x2, y2) = find_pixel(&px, w, h, CARD_COLORS[1], 4).expect("card 2's background color must appear somewhere");
    let (x3, y3) = find_pixel(&px, w, h, CARD_COLORS[2], 4).expect("card 3's background color must appear somewhere");

    assert!(x1 < x2, "card 1 (x={x1}) must sit left of card 2 (x={x2})");
    assert!(x2 < x3, "card 2 (x={x2}) must sit left of card 3 (x={x3})");

    let row_tolerance = 5u32;
    assert!(
        y1.abs_diff(y2) <= row_tolerance && y2.abs_diff(y3) <= row_tolerance,
        "cards 1/2/3 must share row 1 (comparable y): y1={y1}, y2={y2}, y3={y3} -- \
         a stacked (pre-grid-support) render would put each card at a DIFFERENT, much-further-apart y"
    );
}

/// The 4th card (`.c4`, first card of row 2 -- 3 columns per row) must sit
/// in a LOWER row than the 1st card, proving the grid actually wraps to a
/// second row rather than fitting all 6 cards (or an unbounded number of
/// columns) into one row.
#[test]
fn golden_wraps_to_a_second_row_after_three_columns() {
    let (w, h, px) = decode(GOLDEN_PNG);
    let (_, y1) = find_pixel(&px, w, h, CARD_COLORS[0], 4).expect("card 1's background color must appear somewhere");
    let (_, y4) = find_pixel(&px, w, h, CARD_COLORS[3], 4).expect("card 4's background color must appear somewhere");
    assert!(y4 > y1 + 20, "card 4 (row 2) must sit meaningfully below card 1 (row 1): y1={y1}, y4={y4}");
}
