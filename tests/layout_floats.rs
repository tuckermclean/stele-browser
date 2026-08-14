//! Integration tests (M4 floats + inline images packet): the real
//! parse->cascade->box_tree->layout pipeline exercised against small
//! hand-written HTML snippets, proving parts 1-3 (align->float hint,
//! non-floated inline replaced atoms, float exclusion) work together end to
//! end — not just at the `inline::layout_runs`/`box_tree::build_box_tree`
//! unit level (see `src/layout/inline.rs` and `src/layout/box_tree.rs`'s own
//! `#[cfg(test)]` modules for the narrower unit coverage).

use std::collections::HashMap;
use std::rc::Rc;

use stele::dom::Node;
use stele::img::RgbaImage;
use stele::layout::box_tree::build_box_tree;
use stele::layout::{layout, Fragment, FragmentKind, Size};
use stele::style::cascade;

/// Render `html` through the real parse->cascade->box_tree->layout
/// pipeline, stubbing a trivially-decoded 1x1 `RgbaImage` for every `<img>`
/// element present so `FragmentKind::Image` (not the M2-era placeholder
/// `Box`) is what comes out for every `<img>` — the fixture images
/// themselves aren't the point of these tests, only the box geometry is.
fn render(html: &str, viewport_w: f32) -> Vec<Fragment> {
    let dom = stele::dom::parser::parse(html);
    let styles = cascade::cascade(&dom, &[]);

    let mut images: HashMap<stele::dom::NodeId, Rc<RgbaImage>> = HashMap::new();
    let stub = Rc::new(RgbaImage { width: 1, height: 1, pixels: vec![255, 0, 0, 255] });
    for id in 0..dom.len() {
        if let Node::Element(el) = dom.node(id) {
            if el.name.as_str() == "img" {
                images.insert(id, stub.clone());
            }
        }
    }

    let root = build_box_tree(&dom, &styles, &images).expect("root present");
    layout(&root, Size { w: viewport_w, h: 100_000.0 })
}

fn text_fragments(fragments: &[Fragment]) -> Vec<&Fragment> {
    fragments.iter().filter(|f| matches!(f.kind, FragmentKind::Box { .. } | FragmentKind::Image { .. })).collect()
}

fn image_or_placeholder_rects(fragments: &[Fragment]) -> Vec<stele::layout::Rect> {
    fragments
        .iter()
        .filter_map(|f| match f.kind {
            FragmentKind::Image { .. } => Some(f.rect),
            _ => None,
        })
        .collect()
}

fn texts(fragments: &[Fragment]) -> Vec<&str> {
    fragments
        .iter()
        .filter_map(|f| match &f.kind {
            FragmentKind::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Part 1 + 3 end to end: `<img align=left>` becomes a `float: left` box
/// (part 1), pulled out of the paragraph's line flow and placed at the
/// content box's left edge (part 3), with text wrapping around it — the
/// first line's text must start to the RIGHT of the image, not overlap it,
/// and not stack below it as its own block (the pre-M4 behavior).
#[test]
fn img_align_left_floats_and_text_wraps_around_it() {
    let html = r#"<p><img src="x.png" align="left" width="40" height="40">some wrapping text here</p>"#;
    let fragments = render(html, 200.0);

    let images = image_or_placeholder_rects(&fragments);
    assert_eq!(images.len(), 1, "exactly one placeholder/image box for the floated img");
    let img_rect = images[0];
    assert_eq!(img_rect.origin.x, 0.0, "left float sits at the content box's left edge");
    assert_eq!(img_rect.size, Size { w: 40.0, h: 40.0 });

    let ts = texts(&fragments);
    assert!(!ts.is_empty(), "paragraph text must still render");

    // No text fragment whose vertical span overlaps the float's should start
    // to the LEFT of the float's right edge (i.e. text wraps around, not
    // through, the image).
    for f in &fragments {
        if let FragmentKind::Text { .. } = f.kind {
            let overlaps_float_band = f.rect.origin.y < img_rect.origin.y + img_rect.size.h
                && f.rect.origin.y + f.rect.size.h > img_rect.origin.y;
            if overlaps_float_band {
                assert!(
                    f.rect.origin.x >= img_rect.origin.x + img_rect.size.w,
                    "text at y={} (x={}) overlaps the float's column",
                    f.rect.origin.y,
                    f.rect.origin.x
                );
            }
        }
    }
}

/// Part 2 end to end: a non-floated inline `<img>` between two words sits on
/// the SAME line as the surrounding text (same `y`), rather than breaking
/// flow into its own block box below/above the text (the pre-M4/D14 gap).
#[test]
fn non_floated_inline_img_sits_on_the_same_line_as_surrounding_text() {
    let html = r#"<p>before <img src="x.png" width="10" height="10"> after</p>"#;
    let fragments = render(html, 800.0);

    let before = fragments
        .iter()
        .find(|f| matches!(&f.kind, FragmentKind::Text { text, .. } if text.trim() == "before"))
        .expect("'before' text fragment present");
    let after = fragments
        .iter()
        .find(|f| matches!(&f.kind, FragmentKind::Text { text, .. } if text.contains("after")))
        .expect("'after' text fragment present");
    let images = image_or_placeholder_rects(&fragments);
    assert_eq!(images.len(), 1);
    let img = images[0];

    // Same line: all three share (approximately) the same line-box y origin,
    // and the atom's vertical span overlaps the text's (rather than sitting
    // entirely above/below it in its own stacked block).
    assert_eq!(before.rect.origin.y, after.rect.origin.y, "inline img must not push 'after' to a new line");
    let overlaps = img.origin.y < before.rect.origin.y + before.rect.size.h && img.origin.y + img.size.h > before.rect.origin.y;
    assert!(overlaps, "the atom must be vertically within the same line box as the surrounding text");

    // Left-to-right order preserved: before.x < img.x < after.x.
    assert!(before.rect.origin.x < img.origin.x, "'before' must precede the image");
    assert!(img.origin.x < after.rect.origin.x, "the image must precede 'after'");
}

/// Totality smoke test over the REAL pipeline (not just `inline::
/// layout_runs`'s own unit tests): a page with many floated images must
/// render promptly without panicking or hanging.
#[test]
fn many_floated_images_in_one_paragraph_do_not_hang_or_panic() {
    let mut html = String::from("<p>");
    for _ in 0..600 {
        html.push_str(r#"<img src="x.png" align="left" width="5" height="5">"#);
    }
    html.push_str("word ".repeat(50).as_str());
    html.push_str("</p>");

    let fragments = render(&html, 300.0);
    assert!(!fragments.is_empty());
    for f in &fragments {
        assert!(f.rect.size.w.is_finite() && f.rect.size.h.is_finite());
        assert!(f.rect.origin.x.is_finite() && f.rect.origin.y.is_finite());
    }
}

/// `clear` with no float in scope must not panic (totality requirement) —
/// documented as a no-op in this M4 scope (see `inline::place_floats`'s doc
/// comment: floats never escape their own IFC here, so there's nothing for
/// `clear` to meaningfully act on yet).
#[test]
fn clear_with_no_float_present_does_not_panic() {
    let html = r#"<p style="clear: left;">no floats here, just clear</p>"#;
    let fragments = render(html, 200.0);
    assert!(!fragments.is_empty());
}

/// A float wider than its containing block clamps to the full content width
/// (totality requirement) rather than overflowing it or hanging layout.
#[test]
fn float_wider_than_container_clamps_and_does_not_panic() {
    let html = r#"<p><img src="x.png" align="left" width="9999" height="20">text after a huge float</p>"#;
    let fragments = render(html, 150.0);
    let images = image_or_placeholder_rects(&fragments);
    assert_eq!(images.len(), 1);
    assert!(images[0].size.w <= 150.0, "float width must clamp to the containing block's content width");
    assert!(!texts(&fragments).is_empty(), "text must still render below/after the oversized float");
}

/// Silence an "unused function" warning for `text_fragments` while keeping
/// it available for future assertions in this file (mirrors the pattern in
/// other packet test files that keep small shared helpers around).
#[test]
fn helper_smoke_test_render_produces_at_least_one_box_or_image() {
    let fragments = render("<p><img src=\"x.png\" width=\"4\" height=\"4\">hi</p>", 100.0);
    assert!(!text_fragments(&fragments).is_empty());
}
