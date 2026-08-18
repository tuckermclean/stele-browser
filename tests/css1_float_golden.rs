// SPDX-License-Identifier: GPL-3.0-or-later

//! packet/block-floats golden: the real parse->cascade->box-tree->layout->
//! raster->PNG pipeline (same wiring `stele --headless --dump-png` drives
//! in `main.rs`, minus the fetch hop -- `fixtures/css1-float-5526c.html` is
//! entirely self-contained, `<style>` and all, so `include_str!` matches
//! `tests/png_golden.rs`'s own convention) run against the W3C CSS1
//! §5.5.26 "display/box/float/clear test" fixture, asserted exact (by
//! decoded RGBA pixels) against a checked-in golden image.
//!
//! `fixtures/css1-float-5526c.html`'s reference rendering
//! (`fixtures/evidence/css1-float-5526c.reference.gif`) is a Mondrian grid:
//! a narrow red `<dt>` column on the left spanning most of the content
//! height, a white `<dd>` column on the right containing a row of yellow
//! `<li>` cards side by side (NOT stacked) plus a floated `<blockquote>`/
//! `<h1>`. Before packet/block-floats, `ComputedStyle.float`/`.clear` were
//! complete no-ops for block-level boxes (see
//! `fixtures/evidence/css1-float-5526c.diagnosis.md`), so Stele rendered
//! this fixture as one tall vertical stack, in document order, full width
//! -- nothing like the reference. This is a PROPOSED golden (brief §10
//! blessing discipline, same as every other golden here): the implementer
//! blesses it only after PIXEL-VERIFYING structural fidelity against the
//! reference GIF programmatically (this project's own "verify goldens with
//! pixel analysis" discipline -- never by eyeballing), never self-trusting
//! it; the orchestrator/reviewer countersigns. Full pixel-identity to the
//! W3C reference is NOT the claim (font rasterization, form-widget chrome,
//! and Stele's 800px viewport vs. the reference's 531px all differ) --
//! structural/positional fidelity (the red column, the white column, the
//! side-by-side card row) is.

use std::collections::HashMap;

use stele::backend::raster;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const CSS1_FLOAT_HTML: &str = include_str!("../fixtures/css1-float-5526c.html");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/css1-float-5526c.png");
const VIEWPORT_WIDTH: u32 = 800;

/// Render `html` to PNG bytes via the same pipeline `main.rs`'s `dump_png`
/// drives (fetch hop excluded -- see module docs; matches `tests/png_golden.
/// rs`'s own `render_png` helper).
fn render_png(html: &str) -> Vec<u8> {
    let dom_tree = dom::parser::parse(html);
    let styles = cascade::cascade(&dom_tree, &[]);
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

#[test]
fn css1_float_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(CSS1_FLOAT_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);

    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/css1-float-5526c.png)");
}

#[test]
fn golden_shows_the_dt_column_as_a_narrow_red_band_on_the_left() {
    // Structural-fidelity smoke test (not a full pixel-diff against the W3C
    // reference GIF -- that's the implementer's own one-time programmatic
    // verification before blessing, documented in the PR): the leftmost
    // column of the golden's content area should be dominated by the `dt`
    // rule's `background-color: rgb(204,0,0)` red, not the page's own white
    // background -- i.e. the red `dt` float actually rendered as a
    // left-hand column, not as a full-width stacked block (which would put
    // white/other colors at the very left edge below its own short height).
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);

    let is_reddish = |r: u8, g: u8, b: u8| r > 150 && g < 80 && b < 80;
    // Sample a vertical strip well below the top chrome (body border/
    // margin) so we're inside the dt/dd row, not the page's own top edge.
    let y_start = h / 8;
    let y_end = (h * 7) / 8;

    // Scan every column in the left quarter of the page (the dt column sits
    // inset from the very left edge by body/dl padding+border, not flush
    // against x=0) and take the BEST vertical red-fraction among them --
    // this is robust to exactly which x the dt column's padding/border
    // lands on, while still requiring a genuinely tall, narrow, near-left
    // red band to exist (not just a handful of scattered red pixels).
    let scan_x_end = (w / 4).max(1);
    let mut best_fraction = 0.0f32;
    let mut best_x = 0u32;
    for x in 0..scan_x_end {
        let mut red_rows = 0u32;
        let mut sampled_rows = 0u32;
        for y in y_start..y_end {
            let idx = ((y * w + x) * 4) as usize;
            if idx + 2 < px.len() {
                let (r, g, b) = (px[idx], px[idx + 1], px[idx + 2]);
                sampled_rows += 1;
                if is_reddish(r, g, b) {
                    red_rows += 1;
                }
            }
        }
        if sampled_rows > 0 {
            let fraction = red_rows as f32 / sampled_rows as f32;
            if fraction > best_fraction {
                best_fraction = fraction;
                best_x = x;
            }
        }
    }

    assert!(
        best_fraction > 0.5,
        "expected SOME column in the left quarter of the page to be dominated by the dt column's red \
         (rgb(204,0,0)) across most of the content height -- best column was x={best_x} at {best_fraction:.2} \
         red fraction. A stacked-block (pre-packet) render would NOT show a tall, narrow red column here."
    );
}
