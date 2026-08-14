//! M5 flex-polite golden: the real fetch(skip)->parse->cascade(WITH author
//! sheets)->box-tree->layout->raster->PNG pipeline run against
//! `fixtures/flex-polite.html`, asserted exact (decoded RGBA pixels) against
//! a checked-in golden image. PROPOSED golden (brief §10 blessing
//! discipline, same as every other golden in this repo) -- the implementer
//! never self-blesses; the orchestrator/reviewer visually inspects
//! `goldens/flex-polite.png` and countersigns before it's trusted.
//!
//! ## How this exercises the REAL author-CSS pipeline
//!
//! `flex-polite.html`'s entire layout is driven by CSS in an inline
//! `<style>` block (`display: flex`, `justify-content`, `align-items`,
//! `flex-grow`, `gap`, `width`, ...). A golden test that hardcodes
//! `cascade(&dom, &[])` (the pattern `tests/png_golden.rs`/`tty_golden.rs`
//! use for their author-CSS-free fixtures) would cascade the UA sheet ONLY:
//! every element styled `display: flex` by the `<style>` block would fall
//! back to the UA sheet's plain `display: block`, and this fixture would
//! render as a boring vertical stack -- no flex would ever run. So, exactly
//! like `tests/author_css_golden.rs` (the M5 packet's own precedent for this
//! problem) and `main.rs`'s real `dump_png`, `render_png` below calls
//! [`stele::style::collect_author_sheets`] on the parsed DOM and feeds the
//! result into `cascade::cascade` as `author_sheets` -- the exact same two
//! calls `main.rs::dump_png` makes (module docs there literally say "M5:
//! feed cascade real author sheets"). Only the fetch hop is skipped (the
//! fixture is read via `include_str!`, matching every other *_golden.rs
//! convention in this repo, `author_css_golden.rs` included) -- there is no
//! separate/duplicated cascade path here that could silently drift from
//! what `--dump-png` actually does.

use std::collections::HashMap;

use stele::backend::raster;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Fragment, FragmentKind, Rect, Size};
use stele::style::computed::{Dimension, Display, JustifyContent};
use stele::style::{self, cascade};
use stele::surface::{Color, MemSurface};

const FLEX_POLITE_HTML: &str = include_str!("../fixtures/flex-polite.html");
const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/flex-polite.png");
const VIEWPORT_WIDTH: u32 = 800;

/// Render `html` to PNG bytes via the real author-CSS pipeline -- see module
/// docs for exactly how/why this matches `main.rs`'s `dump_png` wiring.
fn render_png(html: &str) -> Vec<u8> {
    let dom_tree = dom::parser::parse(html);
    let author_sheets = style::collect_author_sheets(&dom_tree);
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
    raster::paint(&mut surface, &fragments);
    raster::encode_png(&surface)
}

/// Same real author-CSS pipeline as [`render_png`], but stopping at
/// fragments (no raster/PNG step) -- for the geometry assertions below,
/// which check RECTS, not pixels.
fn render_fragments(html: &str) -> Vec<Fragment> {
    let dom_tree = dom::parser::parse(html);
    let author_sheets = style::collect_author_sheets(&dom_tree);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let root = build_box_tree(&dom_tree, &styles, &HashMap::new()).expect("flex-polite fixture is non-empty");
    let viewport = Size { w: VIEWPORT_WIDTH as f32, h: 100_000.0 };
    layout::layout(&root, viewport)
}

fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("valid PNG frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn box_rects(fragments: &[Fragment]) -> Vec<(Rect, stele::style::ComputedStyle)> {
    fragments
        .iter()
        .filter_map(|f| match &f.kind {
            FragmentKind::Box { style } => Some((f.rect, style.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn flex_polite_png_matches_golden_pixels() {
    let actual_bytes = render_png(FLEX_POLITE_HTML);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);

    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/flex-polite.png)");
}

#[test]
fn golden_png_is_well_formed_and_shows_the_distinct_flex_regions() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);

    // The header/footer's dark navy background (#24344d) and the aside's
    // pale blue background (#e4ecf7) must both actually be painted --
    // confirms this isn't an all-white/blank canvas standing in for a
    // pipeline that silently rendered nothing.
    let navy = [0x24, 0x34, 0x4d, 255];
    let pale_blue = [0xe4, 0xec, 0xf7, 255];
    assert!(px.chunks(4).any(|p| p == navy), "golden should contain the header/footer navy background");
    assert!(px.chunks(4).any(|p| p == pale_blue), "golden should contain the aside's pale-blue background");
}

/// Without author CSS wired in (the pre-M5 `cascade(&dom, &[])` baseline),
/// every `display: flex` declaration in the fixture's `<style>` block never
/// reaches any element -- the UA sheet's plain `display: block` is all that
/// applies, so `header` and `.layout`'s children stack vertically instead of
/// laying out as flex rows. This is the "before" picture the CRITICAL
/// pipeline note calls out: it proves the golden above is really exercising
/// author-CSS-driven flex, not something that would render identically
/// either way.
#[test]
fn without_author_css_the_header_children_would_stack_instead_of_flex() {
    let dom_tree = dom::parser::parse(FLEX_POLITE_HTML);
    let styles = cascade::cascade(&dom_tree, &[]); // no author sheets: pre-M5 baseline
    let root = build_box_tree(&dom_tree, &styles, &HashMap::new()).expect("non-empty document");
    let viewport = Size { w: VIEWPORT_WIDTH as f32, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    let boxes = box_rects(&fragments);
    // No box anywhere is `display: flex` -- the whole document degrades to
    // plain block stacking without the author `<style>` block applying.
    assert!(
        !boxes.iter().any(|(_, style)| style.display == Display::Flex),
        "without author sheets, no element should compute display:flex"
    );
}

/// Geometry proof (not just "it rendered a PNG"): the real pipeline's
/// fragments show the header's title to the LEFT of its nav links
/// (justify-content: space-between actually pushed them to opposite
/// edges), and the article/aside sit side by side with the article WIDER
/// than the fixed-width aside (flex-grow: 1 actually grew it) -- shapes a
/// block-only layout could not produce.
#[test]
fn header_title_sits_left_of_nav_via_justify_content_space_between() {
    let fragments = render_fragments(FLEX_POLITE_HTML);
    let boxes = box_rects(&fragments);

    // The header box itself: display:flex + justify-content:space-between
    // is unique to `header` in this fixture (nav/`.layout` are flex too, but
    // neither sets justify-content, so both stay at the FlexStart default).
    let (header_rect, _) = boxes
        .iter()
        .find(|(_, s)| s.display == Display::Flex && s.justify_content == JustifyContent::SpaceBetween)
        .expect("header box (display:flex; justify-content:space-between) not found");

    // nav: display:flex with gap:20px is unique across the whole fixture
    // (`.layout` is the only other flex container with a nonzero gap, at
    // 24px).
    let (nav_rect, _) = boxes
        .iter()
        .find(|(_, s)| s.display == Display::Flex && (s.gap - 20.0).abs() < 0.01)
        .expect("nav box (display:flex; gap:20px) not found");

    // The title (`h1`): the only OTHER box fully inside the header's rect
    // whose x sits to the left of nav's own box -- nav's own children (the
    // three links) all start at-or-after nav's own x, so this can only be
    // the h1 box.
    let title_rect = boxes
        .iter()
        .map(|(r, _)| r)
        .filter(|r| {
            r.origin.x > header_rect.origin.x
                && r.origin.x < nav_rect.origin.x
                && r.origin.y >= header_rect.origin.y
                && r.origin.y < header_rect.origin.y + header_rect.size.h
        })
        .min_by(|a, b| a.origin.x.partial_cmp(&b.origin.x).unwrap())
        .expect("title (h1) box not found to the left of nav inside the header");

    assert!(
        title_rect.origin.x < nav_rect.origin.x,
        "the header's nav ({:?}) must sit to the RIGHT of the title ({:?})",
        nav_rect,
        title_rect
    );
    // Vertically aligned on the same row (align-items: center keeps their
    // vertical centers close even though the two boxes differ in height).
    let title_center_y = title_rect.origin.y + title_rect.size.h / 2.0;
    let nav_center_y = nav_rect.origin.y + nav_rect.size.h / 2.0;
    assert!(
        (title_center_y - nav_center_y).abs() < 1.0,
        "align-items: center should vertically center the title and nav on the same row"
    );
}

#[test]
fn article_and_aside_sit_side_by_side_with_article_wider_via_flex_grow() {
    let fragments = render_fragments(FLEX_POLITE_HTML);
    let boxes = box_rects(&fragments);

    // aside: the only box with the fixed 220px width the fixture's `<style>`
    // gives it.
    let (aside_rect, _) =
        boxes.iter().find(|(_, s)| s.width == Dimension::Px(220.0)).expect("aside box (width:220px) not found");

    // main: the only OTHER flex-item box on the same row as aside (i.e. the
    // `.layout` flex container's other child) -- identified as the box
    // immediately to the left of aside sharing (approximately) its y origin
    // and taller-or-equal height (both are stretched to the same row height
    // by the default align-items: stretch).
    let (main_rect, _) = boxes
        .iter()
        .find(|(r, _)| {
            (r.origin.y - aside_rect.origin.y).abs() < 1.0 && r.origin.x < aside_rect.origin.x && r.size.w > 220.0
        })
        .expect("main box (flex-grow:1, wider than the 220px aside) not found");

    // Side by side: same-ish y, different x.
    assert!((main_rect.origin.y - aside_rect.origin.y).abs() < 1.0, "main and aside should start on the same row");
    assert!(main_rect.origin.x < aside_rect.origin.x, "main must sit to the LEFT of aside");
    // flex-grow: 1 actually grew main past the fixed-width aside.
    assert!(
        main_rect.size.w > aside_rect.size.w,
        "main (flex-grow:1, {}px) should be wider than aside (fixed, {}px)",
        main_rect.size.w,
        aside_rect.size.w
    );
}
