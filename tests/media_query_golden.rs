//! M5 media golden: the real fetch(skip)->parse->cascade(WITH author sheets,
//! `@media` FLATTENED against a real viewport width)->box-tree->layout->tty
//! pipeline run against `fixtures/media-query.html`, at TWO widths, each
//! asserted exact against its own checked-in golden text dump. PROPOSED
//! goldens (brief §10 blessing discipline, same as every other golden in
//! this repo) — the implementer regenerates/adds them but never
//! self-blesses; see the M5 packet report for the countersign/bless request
//! to the orchestrator/reviewer.
//!
//! `fixtures/media-query.html`'s `<style>` block:
//!   .sidebar { display: block; }
//!   .note { display: none; }
//!   @media (max-width: 500px) {
//!     .sidebar { display: none; }
//!     .note { display: block; }
//!   }
//! The `@media` block comes AFTER the baseline rules in source order, so on
//! a viewport that matches it (narrow), its declarations win same-
//! specificity cascade ties against the earlier baseline ones -- exactly as
//! if it had been written inline at that point in the source
//! (`style::media::flatten_media`'s whole design). This makes the toggle
//! visible in a plain tty dump (no color needed, `display: none` actually
//! removes text):
//!   - WIDE (80 cols = 640px, `(max-width: 500px)` does NOT match): the
//!     sidebar text is present, the narrow-notice text is ABSENT.
//!   - NARROW (`--cols 40` = 320px, DOES match): the sidebar text is
//!     ABSENT, the narrow-notice text is present.
//! The "Main article text..." paragraph has no `@media`-controlled
//! `display` at all, so it is present at BOTH widths -- proof the pipeline
//! renders the rest of the document normally, not just toggling everything.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::{self, cascade};

const MEDIA_QUERY_HTML: &str = include_str!("../fixtures/media-query.html");
const CELL_W: f32 = 8.0;

/// Real pipeline, mirroring `main.rs::dump_text`'s ACTUAL M5-media wiring:
/// `style::collect_author_sheets_for_viewport` (not the plain, non-viewport
/// `collect_author_sheets`) so `@media` is flattened against `cols * 8px`
/// before `cascade` ever runs -- same viewport-width derivation `dump_text`
/// itself uses.
fn dump(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let viewport_width = cols as f32 * CELL_W;
    let author_sheets = style::collect_author_sheets_for_viewport(&dom_tree, viewport_width);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: viewport_width, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

#[test]
fn wide_viewport_tty_dump_matches_golden() {
    let actual = dump(MEDIA_QUERY_HTML, 80);
    let golden = include_str!("../goldens/media-query-wide.tty.txt");
    assert_eq!(
        actual,
        golden.trim_end_matches('\n'),
        "tty dump of fixtures/media-query.html at 80 cols (640px) changed from the PROPOSED golden"
    );
}

#[test]
fn narrow_viewport_tty_dump_matches_golden() {
    let actual = dump(MEDIA_QUERY_HTML, 40);
    let golden = include_str!("../goldens/media-query-narrow.tty.txt");
    assert_eq!(
        actual,
        golden.trim_end_matches('\n'),
        "tty dump of fixtures/media-query.html at 40 cols (320px) changed from the PROPOSED golden"
    );
}

#[test]
fn wide_viewport_shows_sidebar_and_hides_narrow_notice() {
    let actual = dump(MEDIA_QUERY_HTML, 80);
    assert!(actual.contains("SIDEBAR VISIBLE"), "sidebar should be visible at 640px (query does not match)");
    assert!(!actual.contains("NARROW WIDTH NOTICE"), "narrow-notice should be absent at 640px");
}

#[test]
fn narrow_viewport_hides_sidebar_and_shows_narrow_notice() {
    let actual = dump(MEDIA_QUERY_HTML, 40);
    assert!(!actual.contains("SIDEBAR VISIBLE"), "sidebar should be hidden at 320px (query matches)");
    assert!(actual.contains("NARROW WIDTH NOTICE"), "narrow-notice should be visible at 320px");
}

#[test]
fn main_content_is_present_at_both_widths() {
    for cols in [80, 40] {
        let actual = dump(MEDIA_QUERY_HTML, cols);
        assert!(actual.contains("Main article text"), "at {cols} cols");
    }
}

/// Sanity check on the fixture itself: WITHOUT viewport-aware flattening
/// (the pre-M5 baseline -- plain `collect_author_sheets`, `@media` always
/// inert), the sidebar is visible and the narrow-notice is absent
/// REGARDLESS of cols. This is the "before" picture M5 fixes -- confirms
/// the goldens above really are exercising the new @media wiring, not
/// something the baseline rules already did on their own.
#[test]
fn without_media_flattening_the_narrow_notice_never_appears_even_at_narrow_cols() {
    let dom_tree = dom::parser::parse(MEDIA_QUERY_HTML);
    let author_sheets = style::collect_author_sheets(&dom_tree); // pre-M5 baseline: no viewport
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let root = build_box_tree(&dom_tree, &styles, &HashMap::new()).expect("non-empty document");
    let viewport = Size { w: 40.0 * CELL_W, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    let text = tty::render(&fragments, 40).to_text();
    assert!(text.contains("SIDEBAR VISIBLE"), "without @media flattening, the baseline rule always wins");
    assert!(!text.contains("NARROW WIDTH NOTICE"), "without @media flattening, @media never applies");
}
