//! packet/tty-color golden: the real fetch->parse->cascade(with author
//! CSS)->box-tree->layout->tty pipeline run against `fixtures/tty-color.html`,
//! asserted exact against a checked-in ANSI golden dump. Same discipline as
//! every other golden in this repo (brief §10 blessing discipline) — this is
//! a PROPOSED golden; the implementer generates it but never self-blesses.
//! See the packet report for the countersign/bless request to the
//! orchestrator/reviewer.
//!
//! `dump` mirrors `tests/author_css_golden.rs`'s own `dump` helper (which
//! mirrors `main.rs::dump_text`'s real M5 wiring — `style::
//! collect_author_sheets` feeding real sheets into `cascade`), the exact
//! pipeline needed here since `fixtures/tty-color.html` carries an author
//! `<style>` block. The only difference from that helper is the final
//! step: `to_ansi()` instead of `to_text()`, since this golden exists to pin
//! the B+C contrast-safe color resolution (`TextGrid::to_ansi` /
//! `resolve_cell_colors` in `src/backend/tty.rs`), which `to_text()` is
//! blind to by construction.

use std::collections::HashMap;

use stele::backend::tty;
use stele::dom;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::{self, cascade};

const TTY_COLOR_HTML: &str = include_str!("../fixtures/tty-color.html");
const COLS: usize = 40;

fn dump_ansi(html: &str, cols: usize) -> String {
    let dom_tree = dom::parser::parse(html);
    let author_sheets = style::collect_author_sheets(&dom_tree);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_ansi()
}

#[test]
fn tty_color_fixture_ansi_dump_matches_golden() {
    let actual = dump_ansi(TTY_COLOR_HTML, COLS);
    let golden = include_str!("../goldens/tty-color.ansi");
    assert_eq!(actual, golden.trim_end_matches('\n'), "ANSI dump of fixtures/tty-color.html changed from the PROPOSED golden");
}
