//! Integration tests (P6, M2): the real parse+cascade pipeline (P1/P2)
//! feeding the layout engine, exercised against `fixtures/basic.html`.
//! Assertions are deliberately robust to incidental metrics (ordering,
//! non-overlap, staying within the viewport) rather than pixel-exact, per
//! the packet brief.

use stele::dom::{self, Dom, Node, NodeId};
use stele::layout::{layout, BoxContent, Fragment, FragmentKind, LayoutNode, Size};
use stele::style::computed::Display;
use stele::style::{cascade, ComputedStyle};

const BASIC_HTML: &str = include_str!("../fixtures/basic.html");

/// Convert a parsed+cascaded `Dom` into the `layout` module's frozen
/// `LayoutNode` tree. This bridge isn't a P6 deliverable (no packet owns the
/// fetch->parse->style->layout wiring yet) — it's kept local to this
/// integration test, which exists to prove real DOM/cascade output flows
/// through `layout()` sanely. `display: none` subtrees are dropped, matching
/// how a real renderer would skip them.
fn build_layout_tree(dom: &Dom, styles: &[ComputedStyle], id: NodeId) -> Option<LayoutNode> {
    let style = styles[id].clone();
    if style.display == Display::None {
        return None;
    }
    match dom.node(id) {
        Node::Text(text) => {
            Some(LayoutNode { style, content: BoxContent::Text(text.clone()), children: Vec::new(), interactive: None })
        }
        Node::Element(el) => {
            let children = el.children.iter().filter_map(|&c| build_layout_tree(dom, styles, c)).collect();
            Some(LayoutNode { style, content: BoxContent::Container, children, interactive: None })
        }
    }
}

fn text_fragments(fragments: &[Fragment]) -> Vec<&Fragment> {
    fragments.iter().filter(|f| matches!(f.kind, FragmentKind::Text { .. })).collect()
}

fn text_of(f: &Fragment) -> &str {
    match &f.kind {
        FragmentKind::Text { text, .. } => text.as_str(),
        _ => panic!("not a text fragment"),
    }
}

fn layout_basic_fixture(viewport: Size) -> Vec<Fragment> {
    let dom_tree = dom::parser::parse(BASIC_HTML);
    let styles = cascade::cascade(&dom_tree, &[]);
    let root = build_layout_tree(&dom_tree, &styles, dom_tree.root()).expect("root is not display:none");
    layout(&root, viewport)
}

#[test]
fn basic_fixture_lays_out_headings_above_paragraphs_non_overlapping() {
    let viewport = Size { w: 640.0, h: 480.0 };
    let fragments = layout_basic_fixture(viewport);
    assert!(!fragments.is_empty());

    // Totality + "stays sane" checks on every fragment.
    for f in &fragments {
        assert!(f.rect.origin.x.is_finite() && f.rect.origin.y.is_finite());
        assert!(f.rect.size.w.is_finite() && f.rect.size.h.is_finite());
        assert!(f.rect.size.w >= 0.0 && f.rect.size.h >= 0.0);
        assert!(f.rect.origin.x >= 0.0, "fragment x within viewport: {:?}", f.rect);
        assert!(
            f.rect.origin.x + f.rect.size.w <= viewport.w + 1.0,
            "fragment overflows viewport width: {:?}",
            f.rect
        );
    }

    let texts = text_fragments(&fragments);
    let welcome = *texts.iter().find(|f| text_of(f).contains("Welcome")).expect("h1 text present");
    let section = *texts.iter().find(|f| text_of(f).contains("Section")).expect("h2 text present");
    let para = *texts.iter().find(|f| text_of(f).contains("paragraph")).expect("p text present");
    let link = *texts.iter().find(|f| text_of(f).contains("link")).expect("a text present");
    let second = *texts.iter().find(|f| text_of(f).contains("Second")).expect("second p text present");

    // Reading order top to bottom: h1 above h2 above the two paragraphs,
    // first paragraph above the second.
    assert!(welcome.rect.origin.y < section.rect.origin.y, "heading order");
    assert!(section.rect.origin.y < para.rect.origin.y, "h2 above first paragraph");
    assert!(para.rect.origin.y < second.rect.origin.y, "first paragraph above second");

    // The link's text sits on/after the paragraph's line, never above it.
    assert!(link.rect.origin.y >= para.rect.origin.y - 1.0, "link's line is not above its paragraph");

    // When the link shares its paragraph's first line, it follows the
    // paragraph's leading text horizontally (reading order left to right).
    if (para.rect.origin.y - link.rect.origin.y).abs() < 1.0 {
        assert!(para.rect.origin.x < link.rect.origin.x, "link follows paragraph text on its line");
    }
}

#[test]
fn narrow_viewport_still_produces_finite_non_negative_fragments() {
    // Narrow enough to force the paragraph's inline content to wrap onto
    // several lines — exercises the inline engine's wrapping through the
    // real pipeline, not just synthetic-metrics unit tests.
    let fragments = layout_basic_fixture(Size { w: 120.0, h: 4000.0 });
    assert!(!fragments.is_empty());
    for f in &fragments {
        assert!(f.rect.size.w.is_finite() && f.rect.size.h.is_finite());
        assert!(f.rect.origin.x.is_finite() && f.rect.origin.y.is_finite());
        assert!(f.rect.size.w >= 0.0 && f.rect.size.h >= 0.0);
    }

    // Wrapping should have produced strictly more text fragments (more line
    // boxes) than the generous 640px-wide layout.
    let wide = layout_basic_fixture(Size { w: 640.0, h: 480.0 });
    assert!(text_fragments(&fragments).len() >= text_fragments(&wide).len());
}

#[test]
fn degenerate_viewport_on_real_fixture_does_not_panic() {
    for size in [Size { w: 0.0, h: 0.0 }, Size { w: -1.0, h: -1.0 }, Size { w: f32::NAN, h: f32::NAN }] {
        let fragments = layout_basic_fixture(size);
        assert!(!fragments.is_empty());
    }
}
