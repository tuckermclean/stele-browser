//! packet/table-spacing golden: the real fetch(skip)->parse->cascade->
//! box-tree->layout->{tty,raster} pipeline run against
//! `fixtures/table-spacing.html` (`<table border="1" cellpadding="6"
//! cellspacing="4">` with a header row and two data rows), asserted against
//! checked-in goldens. This is a PROPOSED golden (brief §10 blessing
//! discipline, same as every other golden in this repo) — the implementer
//! never self-blesses; the orchestrator/reviewer reads
//! `goldens/table-spacing.txt` and visually inspects
//! `goldens/table-spacing.png` (a ruled table whose cell text sits clearly
//! INSIDE its border, not touching it, with visible gaps between cells) and
//! countersigns before either is trusted.
//!
//! Mirrors `tests/table_border_golden.rs`'s own conventions exactly.

use std::collections::HashMap;

use stele::backend::{raster, tty};
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const TABLE_SPACING_HTML: &str = include_str!("../fixtures/table-spacing.html");
const GOLDEN_TTY: &str = include_str!("../goldens/table-spacing.txt");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/table-spacing.png");
const COLS: usize = 80;
const PNG_VIEWPORT_WIDTH: u32 = 800;

fn dump_tty(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

fn render_png(html: &str) -> Vec<u8> {
    let dom_tree = dom::parser::parse(html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return raster::encode_png(&MemSurface::new(1, 1, Color::WHITE));
    };
    let viewport = Size { w: PNG_VIEWPORT_WIDTH as f32, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);

    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let (y, h) = (f.rect.origin.y, f.rect.size.h);
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let height = if content_bottom > 0.0 { content_bottom.ceil() as u32 } else { 1 };

    let mut surface = MemSurface::new(PNG_VIEWPORT_WIDTH, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &HashMap::new(), Color::WHITE);
    raster::encode_png(&surface)
}

fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("valid PNG frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn table_spacing_fixture_tty_dump_matches_golden() {
    let actual = dump_tty(TABLE_SPACING_HTML, COLS);
    assert_eq!(
        actual,
        GOLDEN_TTY.trim_end_matches('\n'),
        "tty dump of fixtures/table-spacing.html changed from the PROPOSED golden"
    );
}

#[test]
fn table_spacing_tty_dump_shows_every_cell_text() {
    let actual = dump_tty(TABLE_SPACING_HTML, COLS);
    assert!(actual.contains("Item"));
    assert!(actual.contains("Qty"));
    assert!(actual.contains("Widget"));
    assert!(actual.contains("Gadget"));
}

#[test]
fn table_spacing_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(TABLE_SPACING_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/table-spacing.png)");
}

#[test]
fn golden_table_spacing_png_shows_the_gray_ruled_table() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    let gray = [0x80, 0x80, 0x80, 255];
    assert!(px.chunks(4).any(|p| p == gray), "golden should show the table/cell #808080 gray border rule");
}
