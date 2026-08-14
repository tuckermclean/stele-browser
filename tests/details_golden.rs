//! M5 (dialect-completeness) golden: `<details>`/`<summary>` disclosure
//! collapse, run through the real fetch->parse->cascade->box-tree->layout->
//! tty pipeline against `fixtures/details.html`, asserted exact against a
//! checked-in golden text dump. PROPOSED (brief §10 blessing discipline,
//! same as every other golden in this repo) — the implementer regenerates/
//! adds it but never self-blesses; the orchestrator views/blesses.
//!
//! `fixtures/details.html` has one collapsed `<details>` (no `open`
//! attribute — its body must NOT appear) and one `<details open>` (its body
//! MUST appear), each with its own `<summary>` label carrying the
//! `>`/`v` disclosure marker documented in `layout::box_tree`'s
//! "<details>/<summary> disclosure" doc section.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;

const DETAILS_HTML: &str = include_str!("../fixtures/details.html");
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
fn details_fixture_tty_dump_matches_golden() {
    let actual = dump(DETAILS_HTML, COLS);
    let golden = include_str!("../goldens/details.tty.txt");
    assert_eq!(actual, golden.trim_end_matches('\n'), "tty dump of fixtures/details.html changed from the PROPOSED golden");
}

#[test]
fn collapsed_details_body_is_absent_from_the_render() {
    let actual = dump(DETAILS_HTML, COLS);
    assert!(actual.contains("> Collapsed section"), "the collapsed summary's marker + label should show");
    assert!(!actual.contains("should not see this collapsed"), "the collapsed body must not appear");
}

#[test]
fn expanded_details_body_is_present_in_the_render() {
    let actual = dump(DETAILS_HTML, COLS);
    assert!(actual.contains("v Expanded section"), "the expanded summary's marker + label should show");
    assert!(actual.contains("SHOULD see this expanded"), "the expanded body must appear");
}
