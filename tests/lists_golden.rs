//! M6 (list markers) golden: `<ul>/<ol>/<li>` marker synthesis, run through
//! the real fetch->parse->cascade->box-tree->layout->tty pipeline against
//! `fixtures/lists.html`, asserted exact against a checked-in golden text
//! dump. PROPOSED (brief §10 blessing discipline, same as every other
//! golden in this repo) — the implementer regenerates/adds it but never
//! self-blesses; the orchestrator views/blesses.
//!
//! `fixtures/lists.html` exercises: a plain `<ul>` (disc marker), a plain
//! `<ol>` (decimal marker), a nested `<ol>` inside an outer `<ol>`'s item
//! (per-list ordinal counting -- the nested list must restart at 1 without
//! perturbing the outer list's own count), `list-style-type: none` (no
//! marker at all), and `<ol start="5">` (seeds the ordinal). See
//! `layout::box_tree`'s "List markers (M6)" doc section for the full
//! marker-glyph/format convention this locks in.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;

const LISTS_HTML: &str = include_str!("../fixtures/lists.html");
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
fn lists_fixture_tty_dump_matches_golden() {
    let actual = dump(LISTS_HTML, COLS);
    let golden = include_str!("../goldens/lists.tty.txt");
    assert_eq!(actual, golden.trim_end_matches('\n'), "tty dump of fixtures/lists.html changed from the PROPOSED golden");
}

#[test]
fn unordered_and_ordered_markers_render() {
    let actual = dump(LISTS_HTML, COLS);
    assert!(actual.contains("* First bullet item"), "got: {actual:?}");
    assert!(actual.contains("* Second bullet item"), "got: {actual:?}");
    assert!(actual.contains("1. First ordinal item"), "got: {actual:?}");
    assert!(actual.contains("2. Second ordinal item"), "got: {actual:?}");
    assert!(actual.contains("3. Third ordinal item"), "got: {actual:?}");
}

#[test]
fn nested_ordered_list_restarts_its_own_ordinal() {
    let actual = dump(LISTS_HTML, COLS);
    assert!(actual.contains("1. Outer item one"), "got: {actual:?}");
    assert!(actual.contains("2. Outer item two"), "got: {actual:?}");
    assert!(actual.contains("1. Nested item one"), "nested list must restart at 1, got: {actual:?}");
    assert!(actual.contains("2. Nested item two"), "got: {actual:?}");
    assert!(actual.contains("3. Outer item three"), "outer count must not skip past the nested list, got: {actual:?}");
}

#[test]
fn list_style_type_none_suppresses_markers_in_the_fixture() {
    let actual = dump(LISTS_HTML, COLS);
    assert!(actual.contains("No marker on this item"), "got: {actual:?}");
    assert!(!actual.contains("* No marker on this item"), "list-style-type: none must suppress the marker");
    assert!(!actual.contains("* Nor this one"), "list-style-type: none must suppress the marker");
}

#[test]
fn ordered_list_start_attribute_is_honored() {
    let actual = dump(LISTS_HTML, COLS);
    assert!(actual.contains("5. Fifth item"), "got: {actual:?}");
    assert!(actual.contains("6. Sixth item"), "got: {actual:?}");
}
