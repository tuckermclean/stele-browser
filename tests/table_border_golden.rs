//! packet/table-border golden: the real fetch(skip)->parse->cascade->
//! box-tree->layout->{tty,raster} pipeline run against
//! `fixtures/table-border.html` (`<table border="1">` with a header row and
//! two data rows), asserted against checked-in goldens. This is a PROPOSED
//! golden (brief §10 blessing discipline, same as every other golden in this
//! repo) — the implementer never self-blesses; the orchestrator/reviewer
//! reads `goldens/table-border.txt` and visually inspects
//! `goldens/table-border.png` (a ruled table: a 1px gray frame around the
//! table and a 1px gray rule around every cell) and countersigns before
//! either is trusted.
//!
//! Mirrors `tests/hr_rule_golden.rs`'s own conventions exactly — this
//! fixture has no author CSS, no images, nothing that needs a real `file://`
//! fetch. `<table border="1">` (no `cellspacing`) now resolves to
//! `border-collapse: collapse` (packet/border-collapse), and the tty half
//! shows a real box-drawing grid (`─`/`│`, `┌┐└┘` corners) around the table
//! frame and every cell (packet/border-collapse follow-up,
//! `backend::tty::draw_table_grid_lines`) — a readable ruled grid, not the
//! old "cell text only, no ruled lines" rendering (ordinary non-table
//! 4-side-bordered boxes still draw no tty rule at all, unchanged; see
//! `backend::tty`'s own module docs).

use std::collections::HashMap;

use stele::backend::{raster, tty};
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const TABLE_BORDER_HTML: &str = include_str!("../fixtures/table-border.html");
const GOLDEN_TTY: &str = include_str!("../goldens/table-border.txt");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/table-border.png");
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
fn table_border_fixture_tty_dump_matches_golden() {
    let actual = dump_tty(TABLE_BORDER_HTML, COLS);
    assert_eq!(
        actual,
        GOLDEN_TTY.trim_end_matches('\n'),
        "tty dump of fixtures/table-border.html changed from the PROPOSED golden"
    );
}

#[test]
fn table_border_tty_dump_shows_a_ruled_grid_around_every_cell() {
    // Structural guard independent of the exact-match golden above: packet/
    // border-collapse's tty follow-up draws a real box-drawing grid for a
    // bordered table/cell (`backend::tty::draw_table_grid_lines`), so the
    // dump should show the header + data cells AND rule characters framing
    // them -- the opposite of the old "no ruled lines at all" behavior this
    // test used to assert.
    let actual = dump_tty(TABLE_BORDER_HTML, COLS);
    assert!(actual.contains("Item"));
    assert!(actual.contains("Qty"));
    assert!(actual.contains("Widget"));
    assert!(actual.contains("Gadget"));
    assert!(actual.contains('\u{2500}'), "tty backend should draw '─' rule characters for a bordered/collapsed table");
    // packet/collapse-geometry: a vertical rule shows up either as a plain
    // '│' (a "middle" row with no horizontal edge at that row) OR as one of
    // the corner glyphs ┌┐└┘ (a row where a vertical AND horizontal edge
    // meet -- e.g. every row of THIS fixture's short (26px/~2-tty-row)
    // cells, which never have a "middle" row of their own between their top
    // and bottom edges). Either is real evidence of a drawn vertical rule;
    // requiring the bare '│' specifically was an artifact of the OLD
    // dedup-era row heights, not a load-bearing shape of the grid itself.
    let has_vertical_rule = actual.chars().any(|c| matches!(c, '\u{2502}' | '\u{250C}' | '\u{2510}' | '\u{2514}' | '\u{2518}'));
    assert!(has_vertical_rule, "tty backend should draw a vertical rule (bar or corner) for a bordered/collapsed table");
}

#[test]
fn table_border_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(TABLE_BORDER_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/table-border.png)");
}

#[test]
fn golden_table_border_png_shows_the_gray_ruled_table() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    let gray = [0x80, 0x80, 0x80, 255];
    assert!(px.chunks(4).any(|p| p == gray), "golden should show the table/cell #808080 gray border rule");
}
