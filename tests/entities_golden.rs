//! M5 (dialect-completeness) golden: HTML entity decoding coverage lock-in.
//! `dom::parser` already decodes HTML 4.01 named + numeric entities (P1);
//! this fixture exercises a representative set through the real
//! fetch->parse->cascade->box-tree->layout->tty pipeline against
//! `fixtures/entities.html`, asserted exact against a checked-in golden text
//! dump. PROPOSED (brief §10 blessing discipline) — the implementer
//! regenerates/adds it but never self-blesses; the orchestrator
//! views/blesses.
//!
//! The font is ASCII-only, so a decoded non-ASCII character (©, —) renders
//! as the "tofu" fallback glyph in a PNG — but `backend::tty`'s `TextGrid`
//! stores real `char`s per cell (`write_marker` writes each source `char`
//! verbatim; no ASCII-only substitution happens at the text-grid level, only
//! in the bitmap-font PNG path), so the TEXT dump carries the actual decoded
//! Unicode character. That's what these assertions check.
//!
//! One entity in the required set — `&nbsp;` — needs a documented carve-out:
//! see `nbsp_decodes_but_layout_s_frozen_whitespace_collapsing_normalizes_it_to_an_ordinary_space`
//! below.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;

const ENTITIES_HTML: &str = include_str!("../fixtures/entities.html");
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
fn entities_fixture_tty_dump_matches_golden() {
    let actual = dump(ENTITIES_HTML, COLS);
    let golden = include_str!("../goldens/entities.tty.txt");
    assert_eq!(actual, golden.trim_end_matches('\n'), "tty dump of fixtures/entities.html changed from the PROPOSED golden");
}

#[test]
fn named_markup_entities_decode_to_their_literal_characters() {
    let actual = dump(ENTITIES_HTML, COLS);
    assert!(actual.contains("a & b"), "&amp; should decode to a literal &");
    assert!(actual.contains("a < b"), "&lt; should decode to a literal <");
    assert!(actual.contains("a > b"), "&gt; should decode to a literal >");
    assert!(actual.contains("\"quoted text\""), "&quot; should decode to a literal \"");
}

#[test]
fn named_symbol_entities_decode_to_their_real_unicode_characters() {
    let actual = dump(ENTITIES_HTML, COLS);
    assert!(actual.contains('\u{00A9}'), "&copy; should decode to U+00A9");
    assert!(actual.contains('\u{00AE}'), "&reg; should decode to U+00AE");
    assert!(actual.contains('\u{2014}'), "&mdash; should decode to U+2014 (also shared by the hex numeric check below)");
}

#[test]
fn numeric_and_hex_numeric_entities_decode_to_the_same_characters_as_their_named_equivalents() {
    let actual = dump(ENTITIES_HTML, COLS);
    assert!(actual.contains("NumCopy: \u{00A9}"), "&#169; (decimal numeric) should decode to U+00A9, same as &copy;");
    assert!(actual.contains("HexDash: a \u{2014} b"), "&#x2014; (hex numeric) should decode to U+2014, same as &mdash;");
}

#[test]
fn unrecognized_entity_passes_through_literally() {
    let actual = dump(ENTITIES_HTML, COLS);
    assert!(
        actual.contains("&notanentity;"),
        "an unrecognized entity name must pass through literally, not be dropped or mis-decoded"
    );
}

#[test]
fn nbsp_decodes_but_layout_s_frozen_whitespace_collapsing_normalizes_it_to_an_ordinary_space() {
    // dom::parser::decode_entities correctly turns &nbsp; into U+00A0 (see
    // dom/parser.rs's own entity tests, e.g. named_and_numeric_entities_in_text
    // and fixture_soup_html_structure, which assert this at the DOM level,
    // pre-layout). `layout::inline`'s tokenizer is FROZEN for this packet
    // (brief: "Do NOT change FROZEN types/signatures ... layout::*") and
    // splits words on `char::is_whitespace()` -- Unicode's White_Space
    // property (perhaps surprisingly) INCLUDES U+00A0, so a decoded NBSP is
    // swept into the same collapsing as an ordinary space and the distinct
    // codepoint never survives into a rendered `Text` fragment's string.
    // This is pre-existing, documented layout behavior (`layout::inline`'s
    // own "v1 always collapses ... a Pre fast-path is a follow-up" module
    // doc), not an entities-decoding bug -- confirmed empirically:
    // `stele --headless --dump-text` on `a&nbsp;b` emits plain 0x20 between
    // "a" and "b", not the 0xC2 0xA0 UTF-8 encoding of U+00A0.
    //
    // So this test asserts what the pipeline ACTUALLY (and correctly, at the
    // decode layer) does: "a" and "b" end up separated by exactly one
    // ordinary space, and the literal entity text never survives.
    let actual = dump(ENTITIES_HTML, COLS);
    assert!(actual.contains("Nbsp: a b"), "decoded nbsp should join \"a\"/\"b\" with a single space, not literal \"&nbsp;\"");
    assert!(!actual.contains("&nbsp;"), "the entity reference itself must not survive undecoded");
}

#[test]
fn nbsp_decodes_to_u00a0_at_the_dom_level_before_layouts_frozen_whitespace_collapsing() {
    // The actual decode-correctness proof for &nbsp; -- at the DOM text
    // level, before layout::inline ever touches it. See the test above for
    // why the tty-rendered text can't carry this same proof.
    let dom_tree = dom::parser::parse(ENTITIES_HTML);
    fn collect_text(dom: &dom::Dom, id: dom::NodeId, out: &mut String) {
        match dom.node(id) {
            dom::Node::Text(t) => out.push_str(t),
            dom::Node::Element(e) => {
                for &c in &e.children {
                    collect_text(dom, c, out);
                }
            }
        }
    }
    let mut text = String::new();
    collect_text(&dom_tree, dom_tree.root(), &mut text);
    assert!(text.contains('\u{00A0}'), "the parsed DOM must carry the real U+00A0 NBSP character before any layout-stage collapsing");
}
