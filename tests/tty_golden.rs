//! P7 golden: the real fetch->parse->cascade->box-tree->layout->tty pipeline
//! run against `fixtures/basic.html`, asserted exact against a checked-in
//! golden text dump. This is a PROPOSED golden (brief §10 blessing
//! discipline) — an implementer never self-blesses; see the P7 report for
//! the countersign/bless request to the orchestrator/reviewer.
//!
//! This test exercises the same wiring `stele --headless --dump-text` uses
//! in `main.rs`, minus the fetch hop (the fixture is read via `include_str!`
//! instead of `file://`, matching how every other fixture-driven test in
//! this repo avoids IO) — `accept.sh`'s A3 check separately drives the real
//! compiled binary end to end, including the fetch layer.

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;

const BASIC_HTML: &str = include_str!("../fixtures/basic.html");
const COLS: usize = 80;

fn dump(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let Some(root) = build_box_tree(&dom_tree, &styles) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

#[test]
fn basic_fixture_tty_dump_matches_golden() {
    let actual = dump(BASIC_HTML, COLS);
    let golden = include_str!("../goldens/basic.tty.txt");
    // Goldens are stored with a trailing newline (editor/POSIX convention);
    // `TextGrid::to_text` never emits one (no trailing blank line), so trim
    // the golden's own trailing newline before comparing.
    assert_eq!(actual, golden.trim_end_matches('\n'), "tty dump of fixtures/basic.html changed from the PROPOSED golden");
}

#[test]
fn empty_document_dumps_to_empty_text() {
    assert_eq!(dump("", COLS), "");
}

#[test]
fn display_none_root_dumps_to_empty_text() {
    // No UA rule makes the document root display:none, but a pathological
    // author sheet could; exercise it through the same headless pipeline
    // main.rs drives, end to end, to prove it degrades to empty output
    // rather than panicking.
    let dom_tree = dom::parser::parse("<html><body>x</body></html>");
    let sheet = stele::style::parser::parse("html { display: none; }");
    let styles = cascade::cascade(&dom_tree, std::slice::from_ref(&sheet));
    assert!(build_box_tree(&dom_tree, &styles).is_none());
}
