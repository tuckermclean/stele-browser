//! packet/presentational-attrs golden: the real fetch(skip)->parse->cascade->
//! box-tree->layout->{tty,raster} pipeline run against
//! `fixtures/presentational.html` (a `<body text>`, a `<center><font
//! size color>` heading, a `bgcolor` paragraph, and an `align="right">`
//! paragraph wrapping a `<font color>` span), asserted against checked-in
//! goldens. This is a PROPOSED golden (brief §10 blessing discipline, same
//! as every other golden in this repo) — the implementer never
//! self-blesses; the orchestrator/reviewer reads `goldens/
//! presentational.tty.txt` and visually inspects `goldens/
//! presentational.png` (a large purple centered heading, a pale-yellow
//! paragraph, and red right-aligned text) and countersigns before either is
//! trusted.
//!
//! Mirrors `tests/hr_rule_golden.rs`'s conventions exactly (`include_str!`/
//! `include_bytes!`, no fetch — this fixture has no external resources).

use std::collections::HashMap;

use stele::backend::{raster, tty};
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const PRESENTATIONAL_HTML: &str = include_str!("../fixtures/presentational.html");
const GOLDEN_TTY: &str = include_str!("../goldens/presentational.tty.txt");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/presentational.png");
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
    raster::paint(&mut surface, &fragments, &HashMap::new());
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
fn presentational_fixture_tty_dump_matches_golden() {
    let actual = dump_tty(PRESENTATIONAL_HTML, COLS);
    assert_eq!(
        actual,
        GOLDEN_TTY.trim_end_matches('\n'),
        "tty dump of fixtures/presentational.html changed from the PROPOSED golden"
    );
}

#[test]
fn presentational_tty_dump_shows_centered_heading_and_right_aligned_paragraph() {
    // Structural guard independent of the exact-match golden above: proves
    // WHY the dump looks the way it does (align="center"/"right" via the
    // new presentational-hint cascade tier), not just that it's
    // byte-identical to a checked-in file.
    let actual = dump_tty(PRESENTATIONAL_HTML, COLS);
    let lines: Vec<&str> = actual.lines().collect();
    let heading = lines.iter().position(|l| l.contains("Big purple centered heading")).expect("heading line present");
    let leading_spaces = lines[heading].chars().take_while(|c| *c == ' ').count();
    assert!(leading_spaces > 10, "the <center>'d heading should be indented well past the left margin, got {leading_spaces} leading spaces");

    let right = lines.iter().position(|l| l.contains("Right-aligned red text.")).expect("right-aligned line present");
    let trailing = lines[right].trim_end();
    assert!(trailing.len() > 60, "align=\"right\" text should be pushed toward the right edge of an 80-col dump");
}

#[test]
fn presentational_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(PRESENTATIONAL_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/presentational.png)");
}

#[test]
fn golden_presentational_png_shows_the_font_color_and_bgcolor_hints() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    let purple = [0x94, 0x00, 0xd3, 255];
    let red = [255, 0, 0, 255];
    let pale_yellow = [0xff, 0xee, 0x88, 255];
    assert!(px.chunks(4).any(|p| p == purple), "golden should show the <font color=\"#9400d3\"> heading");
    assert!(px.chunks(4).any(|p| p == red), "golden should show the <font color=\"red\"> text");
    assert!(px.chunks(4).any(|p| p == pale_yellow), "golden should show the bgcolor=\"#ffee88\" paragraph background");
}
