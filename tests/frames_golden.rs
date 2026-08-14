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

use stele::fetch::Url;
use stele::{dom, frames};

const COLS: usize = 80;

fn frames_fixture_url() -> Url {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/frames.html");
    Url::new(format!("file://{}", path.display()))
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
