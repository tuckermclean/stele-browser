//! M5 golden: the real fetch->parse->cascade(WITH author sheets)->box-tree->
//! layout->tty pipeline run against `fixtures/author-css.html`, asserted
//! exact against a checked-in golden text dump. This is a PROPOSED golden
//! (brief §10 blessing discipline, same as every other golden in this repo)
//! — the implementer regenerates/adds it but never self-blesses; see the M5
//! packet report for the countersign/bless request to the
//! orchestrator/reviewer.
//!
//! Unlike `tests/tty_golden.rs`'s `dump` helper (which still hardcodes
//! `cascade(&dom, &[])`, since none of ITS fixtures carry author CSS), this
//! test's `dump` mirrors `main.rs::dump_text`'s ACTUAL M5 wiring —
//! `style::collect_author_sheets` feeding real sheets into `cascade` — so it
//! is the one golden in this repo that actually exercises author CSS taking
//! effect end to end, not just in a unit test.
//!
//! `fixtures/author-css.html` demonstrates two things the tty dump makes
//! visible as plain text (color isn't tty-visible, so this fixture keys off
//! `display` instead — see its own comments):
//!   1. An author `<style>` block applying at all (`p.hidden { display:
//!      none }` removes the second paragraph from the dump).
//!   2. Inline `style=` outranking that same author rule (the third
//!      paragraph matches `.hidden` too, but its `style="display: block"`
//!      wins, so it's back in the dump) — origin order UA < author < inline,
//!      made visible.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::{self, cascade};

const AUTHOR_CSS_HTML: &str = include_str!("../fixtures/author-css.html");
const COLS: usize = 80;

fn dump(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let author_sheets = style::collect_author_sheets(&dom_tree);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

#[test]
fn author_css_fixture_tty_dump_matches_golden() {
    let actual = dump(AUTHOR_CSS_HTML, COLS);
    let golden = include_str!("../goldens/author-css.tty.txt");
    assert_eq!(
        actual,
        golden.trim_end_matches('\n'),
        "tty dump of fixtures/author-css.html changed from the PROPOSED golden"
    );
}

#[test]
fn author_style_block_display_none_actually_removes_the_paragraph() {
    let actual = dump(AUTHOR_CSS_HTML, COLS);
    assert!(!actual.contains("an author"), "the display:none paragraph should not appear in the dump");
}

#[test]
fn inline_style_overrides_the_matching_author_rule_and_stays_visible() {
    let actual = dump(AUTHOR_CSS_HTML, COLS);
    assert!(
        actual.contains("Visible again"),
        "the paragraph with inline style=\"display: block\" should override the author .hidden rule and stay visible"
    );
}

#[test]
fn without_author_css_wiring_all_three_paragraphs_would_show_ua_only_baseline() {
    // Sanity check on the fixture itself, isolating cascade with NO author
    // sheets (the pre-M5 behavior): every paragraph is visible because
    // `display: none` never reaches the DOM without author CSS wired in.
    // This is the "before" picture M5 fixes -- confirms the golden above
    // really is exercising the new wiring, not something the UA sheet
    // already did on its own.
    let dom_tree = dom::parser::parse(AUTHOR_CSS_HTML);
    let styles = cascade::cascade(&dom_tree, &[]);
    let root = build_box_tree(&dom_tree, &styles, &HashMap::new()).expect("non-empty document");
    let viewport = Size { w: COLS as f32 * 8.0, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    let text = tty::render(&fragments, COLS).to_text();
    assert!(text.contains("an author"), "without author sheets, display:none never applies, so this text is present");
}
