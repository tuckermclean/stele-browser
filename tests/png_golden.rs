//! M4 golden: the real fetch->parse->cascade->box-tree->layout->raster->PNG
//! pipeline run against `fixtures/basic.html`, asserted exact (by decoded
//! RGBA pixels, not raw PNG bytes — robust to encoder metadata differences)
//! against a checked-in golden image. This is a PROPOSED golden (brief §10
//! blessing discipline, same as `tests/tty_golden.rs`'s own `basic.tty.txt`)
//! — an implementer never self-blesses; the orchestrator/reviewer visually
//! inspects `goldens/basic.png` and countersigns before it's trusted.
//!
//! This test exercises the same wiring `stele --headless --dump-png` uses
//! in `main.rs`, minus the fetch hop (the fixture is read via `include_str!`
//! instead of `file://`, matching `tests/tty_golden.rs`'s own convention) —
//! and the same default 800px viewport width `main.rs`'s `--dump-png` uses.

use stele::backend::raster;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const BASIC_HTML: &str = include_str!("../fixtures/basic.html");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/basic.png");
const VIEWPORT_WIDTH: u32 = 800;

/// Render `html` to PNG bytes via the same pipeline `main.rs`'s `dump_png`
/// drives (fetch hop excluded — see module docs).
fn render_png(html: &str) -> Vec<u8> {
    let dom_tree = dom::parser::parse(html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let Some(root) = build_box_tree(&dom_tree, &styles) else {
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
    raster::paint(&mut surface, &fragments);
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
fn basic_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(BASIC_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);

    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/basic.png)");
}

#[test]
fn golden_png_is_well_formed_and_nontrivial() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    // Sanity: not an all-white blank canvas -- some ink was actually painted.
    assert!(px.chunks(4).any(|p| p != [255, 255, 255, 255]), "golden should not be blank");
}

#[test]
fn empty_document_renders_to_a_blank_all_white_canvas() {
    // Matches main.rs's real `--dump-png` behavior: an empty (but
    // successfully parsed) document still builds a root box with no
    // fragments, so the canvas comes back at the full viewport width with a
    // content-driven (here: minimum, `1px`) height -- all-white, but not
    // the 1x1 `blank_png()` fallback (that's reserved for a build_box_tree
    // `None`, e.g. a `display: none` root, which "" is not).
    let bytes = render_png("");
    let (w, h, px) = decode(&bytes);
    assert_eq!((w, h), (VIEWPORT_WIDTH, 1));
    assert!(px.chunks(4).all(|p| p == [255, 255, 255, 255]));
}
