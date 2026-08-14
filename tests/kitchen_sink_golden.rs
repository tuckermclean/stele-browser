//! M6 golden (acceptance A5): `fixtures/kitchen-sink.html`, "the everything
//! page" — a single document exercising the WHOLE curated dialect at once
//! (headings, inline markup, lists, blockquote, pre, hr, br, a table with
//! colspan/rowspan, a form, plain + floated images, author-CSS flexbox,
//! details/summary open+closed, noscript, entities, and a small `@media`
//! rule) as a coverage benchmark + regression net — NOT an adversarial
//! torture test (`fixtures/soup.html`/`tests/fuzz_totality.rs` are that).
//!
//! Two goldens, both PROPOSED (brief §10 blessing discipline, same as every
//! other golden in this repo — the implementer never self-blesses; the
//! orchestrator/reviewer views `goldens/kitchen-sink.png` and reads
//! `goldens/kitchen-sink.tty.txt` to countersign):
//!   - `goldens/kitchen-sink.tty.txt`: the real fetch(skip)->parse->cascade
//!     (WITH author sheets, `@media` flattened against the tty viewport)->
//!     box-tree->layout->tty pipeline, matching `tests/tty_golden.rs`'s own
//!     convention (`include_str!`, no image fetch — `--dump-text` never
//!     decodes images either, see `main.rs::dump_text`'s doc comment).
//!   - `goldens/kitchen-sink.png`: the real fetch(`file://`)->parse->cascade
//!     ->images pre-pass->box-tree->layout->raster->PNG pipeline, matching
//!     `tests/images_golden.rs`'s own convention (a REAL `file://` fetch,
//!     since the fixture's `<img src>`s are genuine relative siblings on
//!     disk that need decoding for the screenshot to be real pixels, not
//!     placeholders) at the same 800px default width `main.rs::dump_png`
//!     uses.
//!
//! `accept.sh`'s A5 block drives the compiled binary over this same fixture
//! end to end (real process, real fetch layer) as an independent check.

use std::collections::HashMap;

use stele::backend::{raster, tty};
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::{Fetch, Request, Url};
use stele::images;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::{self, cascade};
use stele::surface::{Color, MemSurface};

const KITCHEN_SINK_HTML: &str = include_str!("../fixtures/kitchen-sink.html");
const GOLDEN_TTY: &str = include_str!("../goldens/kitchen-sink.tty.txt");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/kitchen-sink.png");
const COLS: usize = 80;
const PNG_VIEWPORT_WIDTH: u32 = 800;

fn fixture_url() -> Url {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/kitchen-sink.html");
    Url::new(format!("file://{}", path.display()))
}

fn fetch_fixture_html(url: &Url) -> String {
    let body = FileFetcher::new().fetch(&Request::get(url.clone())).expect("fixture should fetch").body;
    String::from_utf8_lossy(&body).into_owned()
}

/// tty dump via the real author-CSS pipeline (no image fetch — `--dump-text`
/// never decodes images either; see module docs).
fn dump_tty(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let viewport_w = cols as f32 * 8.0;
    let author_sheets = style::collect_author_sheets_for_viewport(&dom_tree, viewport_w);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: viewport_w, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

/// PNG render via the real fetch->parse->cascade->images->box-tree->layout->
/// raster->PNG pipeline (see module docs for why this needs a real
/// `file://` fetch, unlike the tty half above).
fn render_png() -> Vec<u8> {
    let url = fixture_url();
    let html = fetch_fixture_html(&url);
    let dom_tree = dom::parser::parse(&html);
    let author_sheets = style::collect_author_sheets_for_viewport(&dom_tree, PNG_VIEWPORT_WIDTH as f32);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let decoded = images::collect_images(&dom_tree, &url);
    let Some(root) = build_box_tree(&dom_tree, &styles, &decoded) else {
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
    raster::paint(&mut surface, &fragments);
    raster::encode_png(&surface)
}

fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("valid PNG frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn kitchen_sink_tty_dump_matches_golden() {
    let actual = dump_tty(KITCHEN_SINK_HTML, COLS);
    assert_eq!(
        actual,
        GOLDEN_TTY.trim_end_matches('\n'),
        "tty dump of fixtures/kitchen-sink.html changed from the PROPOSED golden"
    );
}

#[test]
fn kitchen_sink_png_matches_golden_pixels() {
    let actual_bytes = render_png();
    let (aw, ah, apx) = decode_png(&actual_bytes);
    let (gw, gh, gpx) = decode_png(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/kitchen-sink.png)");
}

#[test]
fn golden_kitchen_sink_png_is_well_formed_and_nontrivial() {
    let (w, h, px) = decode_png(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    assert!(px.chunks(4).any(|p| p != [255, 255, 255, 255]), "golden should not be blank");
}

/// Every real `<img>` in the fixture should actually decode (a plain inline
/// GIF and a floated PNG) — a silent regression to all-placeholder
/// rendering would still produce SOME pixels and could slip past the
/// pixel-exact check alone (mirrors `tests/images_golden.rs`'s own such
/// check).
#[test]
fn all_kitchen_sink_images_actually_decode() {
    let url = fixture_url();
    let html = fetch_fixture_html(&url);
    let dom_tree = dom::parser::parse(&html);
    let decoded = images::collect_images(&dom_tree, &url);
    assert_eq!(decoded.len(), 2, "both <img>s in fixtures/kitchen-sink.html should decode");
}

/// `<br>` (M6 hardening fix — see JOURNAL/DECISIONS): the fixture's
/// "Line one of a paragraph.<br>Line two, after an explicit break." must
/// actually render as two separate lines, not one run-together line —
/// proves the forced-break fix end to end through the real pipeline this
/// golden itself exercises, not just `layout::inline`'s own unit tests.
#[test]
fn br_forces_a_real_line_break_in_the_kitchen_sink_render() {
    let actual = dump_tty(KITCHEN_SINK_HTML, COLS);
    let line_one_idx = actual.lines().position(|l| l.contains("Line one of a paragraph.")).expect("line one present");
    let line_two_idx = actual.lines().position(|l| l.contains("Line two, after an explicit break.")).expect("line two present");
    assert_ne!(line_one_idx, line_two_idx, "<br> should split these onto separate lines");
    // And, just as importantly, they must not have been silently
    // concatenated onto ONE line with no split at all.
    assert!(
        !actual.lines().any(|l| l.contains("paragraph.Line two")),
        "the <br> must not be a no-op that glues the two sentences together"
    );
}

/// Disclosure state, in the real combined document: the collapsed
/// `<details>`'s body must not leak, and the expanded one's body must show —
/// same contract `fixtures/details.html`'s own golden proves in isolation,
/// re-checked here in combination with every other feature on the page.
#[test]
fn details_disclosure_state_is_correct_in_combination() {
    let actual = dump_tty(KITCHEN_SINK_HTML, COLS);
    assert!(!actual.contains("Body text that should not appear while collapsed."));
    assert!(actual.contains("Body text that SHOULD appear while expanded."));
}
