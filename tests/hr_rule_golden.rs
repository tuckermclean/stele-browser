//! packet/hr-rule golden: the real fetch(skip)->parse->cascade->box-tree->
//! layout->{tty,raster} pipeline run against `fixtures/hr-rule.html`
//! (`<p>Above the rule.</p><hr><p>Below the rule.</p>`), asserted against
//! checked-in goldens. This is a PROPOSED golden (brief §10 blessing
//! discipline, same as every other golden in this repo) — the implementer
//! never self-blesses; the orchestrator/reviewer reads `goldens/hr-rule.txt`
//! and visually inspects `goldens/hr-rule.png` (a gray horizontal line
//! between the two paragraphs) and countersigns before either is trusted.
//!
//! Mirrors `tests/tty_golden.rs`'s (`include_str!`, no fetch) and
//! `tests/png_golden.rs`'s (same, no external resources needed) own
//! conventions exactly — this fixture has no author CSS, no images, nothing
//! that needs a real `file://` fetch.

use std::collections::HashMap;

use stele::backend::{raster, tty};
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const HR_RULE_HTML: &str = include_str!("../fixtures/hr-rule.html");
const GOLDEN_TTY: &str = include_str!("../goldens/hr-rule.txt");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/hr-rule.png");
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
fn hr_rule_fixture_tty_dump_matches_golden() {
    let actual = dump_tty(HR_RULE_HTML, COLS);
    assert_eq!(actual, GOLDEN_TTY.trim_end_matches('\n'), "tty dump of fixtures/hr-rule.html changed from the PROPOSED golden");
}

#[test]
fn hr_rule_tty_dump_shows_a_full_rule_line_between_the_two_paragraphs() {
    // Structural guard independent of the exact-match golden above: proves
    // WHY the dump looks the way it does, not just that it's byte-identical
    // to a checked-in file.
    let actual = dump_tty(HR_RULE_HTML, COLS);
    let lines: Vec<&str> = actual.lines().collect();
    let above = lines.iter().position(|l| l.contains("Above the rule.")).expect("should render the first paragraph");
    let below = lines.iter().position(|l| l.contains("Below the rule.")).expect("should render the second paragraph");
    assert!(below > above, "'Below the rule.' should render after 'Above the rule.'");
    let rule_line = lines[above + 1..below].iter().find(|l| l.contains('\u{2500}'));
    let rule_line = rule_line.expect("a '─' rule line should appear between the two paragraphs");
    assert!(rule_line.chars().filter(|&c| c == '\u{2500}').count() > 10, "the rule should span a real run of columns, not a stray char");
}

#[test]
fn hr_rule_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_png(HR_RULE_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/hr-rule.png)");
}

#[test]
fn golden_hr_rule_png_shows_a_gray_horizontal_line() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    let gray = [0x80, 0x80, 0x80, 255];
    assert!(px.chunks(4).any(|p| p == gray), "golden should show the hr's gray (#808080) rule line");
}
