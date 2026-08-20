//! Block-in-inline regression golden (packet/block-in-inline): a
//! block-level element (`<ol>`/`<li>`) nested inside an inline-display
//! element (`<font>`, CSS initial `display: inline`, see `style::ua`'s UA
//! sheet) must still lay out as its own stacked block box -- not get folded
//! into the inline formatting context and run together on one line.
//!
//! Confirmed real-world breakage: 68k.news (http://68k.news/) wraps every
//! news list in `<font size="4"><ol><li>...`, so every list on that page
//! collapsed to run-on text before this fix.
//!
//! Run through the real fetch->parse->cascade->box-tree->layout->tty
//! pipeline against `fixtures/block-in-inline.html`, same discipline as
//! every other tty golden in this repo (`tests/lists_golden.rs`,
//! `tests/tty_golden.rs`, ...): PROPOSED golden (brief §10 blessing
//! discipline) -- the implementer regenerates/adds it but never
//! self-blesses; the orchestrator views/blesses.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;

const BLOCK_IN_INLINE_HTML: &str = include_str!("../fixtures/block-in-inline.html");
const COLS: usize = 80;

fn dump(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

#[test]
fn block_in_inline_fixture_tty_dump_matches_golden() {
    let actual = dump(BLOCK_IN_INLINE_HTML, COLS);
    let golden = include_str!("../goldens/block-in-inline.txt");
    assert_eq!(
        actual,
        golden.trim_end_matches('\n'),
        "tty dump of fixtures/block-in-inline.html changed from the PROPOSED golden"
    );
}

/// The core regression: a `<font>`-wrapped `<ol>` must produce three
/// SEPARATE lines (one per `<li>`, each carrying its own decimal marker),
/// not one line with every item's marker+text run together
/// ("1. Alpha item2. Beta item3. Gamma item...").
#[test]
fn list_inside_inline_font_wrapper_puts_each_item_on_its_own_line() {
    let actual = dump(BLOCK_IN_INLINE_HTML, COLS);
    let lines: Vec<&str> = actual.lines().collect();

    let alpha_line = lines.iter().position(|l| l.contains("Alpha item"));
    let beta_line = lines.iter().position(|l| l.contains("Beta item"));
    let gamma_line = lines.iter().position(|l| l.contains("Gamma item"));

    assert!(alpha_line.is_some(), "expected an 'Alpha item' line, got: {actual:?}");
    assert!(beta_line.is_some(), "expected a 'Beta item' line, got: {actual:?}");
    assert!(gamma_line.is_some(), "expected a 'Gamma item' line, got: {actual:?}");

    assert_ne!(alpha_line, beta_line, "Alpha and Beta items must not share one line, got: {actual:?}");
    assert_ne!(beta_line, gamma_line, "Beta and Gamma items must not share one line, got: {actual:?}");
    assert_ne!(alpha_line, gamma_line, "Alpha and Gamma items must not share one line, got: {actual:?}");

    // Each item carries its own decimal marker on its own line (list markers,
    // M6 discipline). Spacing-tolerant: Terminus's list-marker advance puts
    // slightly more space after the marker than font8x8 did (a legitimate,
    // intended reflow from the font swap), so assert marker AND text share the
    // line without pinning the exact inter-marker gap.
    assert!(lines[alpha_line.unwrap()].contains("1.") && lines[alpha_line.unwrap()].contains("Alpha item"), "got: {actual:?}");
    assert!(lines[beta_line.unwrap()].contains("2.") && lines[beta_line.unwrap()].contains("Beta item"), "got: {actual:?}");
    assert!(lines[gamma_line.unwrap()].contains("3.") && lines[gamma_line.unwrap()].contains("Gamma item"), "got: {actual:?}");

    // The bug's exact symptom must not appear: markers/items run together.
    assert!(!actual.contains("Alpha item2."), "list items must not run together onto one line, got: {actual:?}");
    assert!(!actual.contains("Beta item3."), "list items must not run together onto one line, got: {actual:?}");
}

/// The trailing `<p>After the list.</p>` (a plain block sibling, unrelated
/// to the `<font>`/`<ol>` nesting) must still render on its own line,
/// unaffected by the fix.
#[test]
fn text_after_the_list_renders_on_its_own_line() {
    let actual = dump(BLOCK_IN_INLINE_HTML, COLS);
    assert!(actual.contains("After the list."), "got: {actual:?}");
    let lines: Vec<&str> = actual.lines().collect();
    let after_line = lines.iter().position(|l| l.contains("After the list."));
    let gamma_line = lines.iter().position(|l| l.contains("Gamma item"));
    assert!(after_line.is_some() && gamma_line.is_some());
    assert_ne!(after_line, gamma_line, "'After the list.' must be on its own line, not glued to the last list item");
}
