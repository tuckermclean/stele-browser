//! M5 (dialect-completeness) golden: `<noscript>` content renders (Stele has
//! no JavaScript by construction, so `<noscript>` is exactly "what to show
//! when scripting is unavailable" — always, here), run through the real
//! fetch->parse->cascade->box-tree->layout->tty pipeline against
//! `fixtures/noscript.html`, asserted exact against a checked-in golden text
//! dump. PROPOSED (brief §10 blessing discipline) — the implementer
//! regenerates/adds it but never self-blesses; the orchestrator
//! views/blesses.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;

const NOSCRIPT_HTML: &str = include_str!("../fixtures/noscript.html");
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
fn noscript_fixture_tty_dump_matches_golden() {
    let actual = dump(NOSCRIPT_HTML, COLS);
    let golden = include_str!("../goldens/noscript.tty.txt");
    assert_eq!(actual, golden.trim_end_matches('\n'), "tty dump of fixtures/noscript.html changed from the PROPOSED golden");
}

#[test]
fn noscript_content_shows_in_the_render() {
    let actual = dump(NOSCRIPT_HTML, COLS);
    assert!(actual.contains("lives inside a noscript element"), "the noscript's <p> content must render, never be hidden");
    assert!(actual.contains("An ordinary paragraph"), "sanity: the following sibling paragraph also renders");
}
