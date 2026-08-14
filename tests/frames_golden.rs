//! `frames` packet golden: the real fetch->parse->frameset-detect->composite
//! pipeline run against `fixtures/frames.html`, asserted exact against a
//! checked-in golden text dump. This is a PROPOSED golden (brief §10
//! blessing discipline) — an implementer never self-blesses; see the
//! packet's report for the countersign/bless request to the
//! orchestrator/reviewer.
//!
//! Unlike `tests/tty_golden.rs`'s fixtures (loaded via `include_str!` to
//! avoid IO, since a single document has no further fetches of its own), a
//! frameset document's `<frame src>` children are each an independent fetch
//! — this test drives the REAL `file://` fetch path (via a `file://` URL
//! built from `CARGO_MANIFEST_DIR`) so `frame_nav.html`/`frame_main.html`
//! actually get resolved and read, exactly as `stele --headless --dump-text
//! fixtures/frames.html` does end to end. `accept.sh`'s A3 check separately
//! drives the real compiled binary the same way.

use std::time::{Duration, Instant};

use stele::fetch::Url;
use stele::{dom, frames};

const COLS: usize = 80;

fn fixture_url(name: &str) -> Url {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name);
    Url::new(format!("file://{}", path.display()))
}

fn frames_fixture_url() -> Url {
    fixture_url("frames.html")
}

#[test]
fn frames_fixture_tty_dump_matches_golden() {
    let url = frames_fixture_url();
    let body = std::fs::read(url.path()).expect("reading fixtures/frames.html");
    let html = String::from_utf8_lossy(&body);
    let dom_tree = dom::parser::parse(&html);
    let frameset_id = frames::find_frameset(&dom_tree).expect("fixtures/frames.html has a <frameset>");
    let actual = frames::render(&url, &dom_tree, frameset_id, COLS).to_text();

    let golden = include_str!("../goldens/frames.tty.txt");
    assert_eq!(actual, golden.trim_end_matches('\n'), "tty dump of fixtures/frames.html changed from the PROPOSED golden");
}

#[test]
fn ordinary_fixture_is_not_routed_through_the_frames_renderer() {
    // basic.html has no <frameset> anywhere: find_frameset must return None
    // so main.rs's routing falls through to the ordinary single-doc
    // pipeline unchanged.
    let html = include_str!("../fixtures/basic.html");
    let dom_tree = dom::parser::parse(html);
    assert!(frames::find_frameset(&dom_tree).is_none());
}

/// MINOR 3 (review): a genuine cross-DOCUMENT A->B->A cycle, not just a
/// same-DOM synthetic one — `fixtures/frame_cycle_a.html`'s only `<frame>`
/// points at `frame_cycle_b.html`, whose only `<frame>` points right back
/// at `frame_cycle_a.html`. Dumping either one drives two REAL `file://`
/// fetches (a->b, b->a) before the cycle check (`FrameCtx::visited`)
/// short-circuits the second reference back to `a` — proving totality
/// end-to-end through the real fetch path, not just via in-memory DOM
/// construction (as `frames::tests::
/// self_referential_frame_src_is_a_bounded_cycle_placeholder` already
/// does). Must return promptly (bounded well under any reasonable test
/// timeout) and must not panic/abort.
#[test]
fn cross_document_frameset_cycle_terminates_promptly_not_a_hang() {
    let url = fixture_url("frame_cycle_a.html");
    let body = std::fs::read(url.path()).expect("reading fixtures/frame_cycle_a.html");
    let html = String::from_utf8_lossy(&body);
    let dom_tree = dom::parser::parse(&html);
    let frameset_id = frames::find_frameset(&dom_tree).expect("fixtures/frame_cycle_a.html has a <frameset>");

    let start = Instant::now();
    let grid = frames::render(&url, &dom_tree, frameset_id, COLS);
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_secs(5), "cyclic frameset dump took too long: {elapsed:?} (possible hang)");
    let text = grid.to_text();
    // A placeholder ("[cycle]"/"[unavailable]"/etc.) is expected somewhere
    // in the bounded output; the real point is that this line was ever
    // reached at all rather than hanging or aborting.
    assert!(text.contains('['), "expected a placeholder marker somewhere in the bounded cyclic dump: {text:?}");
}
