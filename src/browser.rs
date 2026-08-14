//! The interactive shell (P7, packet `shell-keyboard`, c1): keyboard-driven
//! scroll/focus/follow/submit/history over the frozen `layout::Fragment`
//! stream, built on top of `backend::tty`'s `TextGrid`. MOUSE (c2) is a
//! separate follow-up packet — but [`Page::hit_test`] is factored as a clean
//! `(col, row) -> Option<focusable index>` pure function specifically so c2
//! can reuse it unchanged.
//!
//! ## Pure vs thin (packet brief's own split)
//!
//! Everything in this module is pure and unit-tested: no raw terminal mode,
//! no blocking reads, no `println!`. The one genuinely un-CI-testable part
//! (raw-mode enter/restore, the read/draw loop, `rustix` termios calls) lives
//! in `main.rs`'s new `stele <url>` mode, which calls into this module for
//! every actual decision.
//!
//! ## Focusable-to-DOM mapping (a documented v0 approximation)
//!
//! `layout::Fragment`/`layout::LayoutNode` carry `Interactive` provenance
//! (href / control kind+name+form_action) but NOT the originating DOM
//! `NodeId` — that carrier is frozen (P7 interactive-provenance amendment)
//! and out of this packet's reach. Submitting a form for real needs
//! `form::serialize_submit(dom, form_id, base, activator)`, which DOES need
//! real `NodeId`s (to walk the form's other controls' live attribute
//! values). [`resolve_control_nodes`] recovers them with a SEPARATE,
//! independent walk of the `Dom` ([`collect_dom_interactive`]) that mirrors
//! `layout::box_tree`'s own interactive-tagging rules (display:none subtrees
//! dropped, `<input type=hidden>` produces no box, a `<form>` updates the
//! enclosing-form context for its descendants) closely enough that, for any
//! document without pathological structure (an interactive element nested
//! inside another interactive element), the two lists — fragment-derived
//! focusables and DOM-derived interactive elements — land in the same
//! document order and zip up 1:1. When a document DOES break that
//! assumption, resolution just stops early (documented "best effort, never
//! wrong, never a panic" — see `resolve_control_nodes`'s doc comment): a
//! focusable whose `control_node` field is `None` simply doesn't submit
//! (`Enter` degrades to `Command::None`), it never associates with the WRONG
//! form/control.

use crate::backend::tty::{self, Cell, TextGrid};
use crate::dom::{Dom, Element, Node, NodeId};
use crate::dom_util;
use crate::fetch::{Request, Url};
use crate::form;
use crate::layout::{Fragment, Interactive};
use crate::style::computed::Display;
use crate::style::ComputedStyle;
use crate::surface::Color;

// =========================================================================
// Focusable extraction + hit-testing (pure)
// =========================================================================

/// The DOM identity behind a `FormControl` [`Focusable`], recovered by
/// [`resolve_control_nodes`] — `None` for `Link` focusables (a link's
/// `href`, already carried by `Interactive`, is everything `Enter` needs) and
/// for any `FormControl` the resolver couldn't confidently match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlNode {
    pub control_id: NodeId,
    pub form_id: Option<NodeId>,
}

/// One focusable region of a [`Page`]: a screen-cell rectangle plus the
/// `Interactive` provenance it carries. Contiguous fragments carrying an
/// EQUAL `Interactive` value (same href, or same control kind+name+
/// form_action) are merged into a single `Focusable` — see
/// [`extract_focusables`] — so a link wrapped across several lines, or a
/// form control's synthesized wrapper+label pair, is one Tab stop, not
/// several.
#[derive(Debug, Clone)]
pub struct Focusable {
    /// `(col, row, width, height)` in screen cells — a bounding box over
    /// every merged fragment's own cell rect (see [`extract_focusables`]'s
    /// doc comment for the multi-line-link caveat this implies).
    pub rect_cells: (usize, usize, usize, usize),
    pub interactive: Interactive,
    pub control_node: Option<ControlNode>,
}

/// A fetched + laid-out document, ready for the interactive shell to drive:
/// the full-page character grid (rendered at terminal width, tall enough for
/// ALL content — never just one screen) plus every focusable region in
/// document order.
pub struct Page {
    pub url: Url,
    grid: TextGrid,
    pub focusables: Vec<Focusable>,
    dom: Dom,
}

impl Page {
    /// Build a `Page` from an already fetched+parsed+cascaded+laid-out
    /// document. `dom`/`styles` are needed only to resolve `FormControl`
    /// focusables back to real `NodeId`s (see the module doc comment);
    /// `fragments` is `layout::layout`'s own output, `cols` the terminal
    /// width to render at.
    pub fn build(dom: Dom, styles: &[ComputedStyle], fragments: &[Fragment], cols: usize, url: Url) -> Page {
        let grid = tty::render(fragments, cols);
        let mut focusables = extract_focusables(fragments, cols);
        resolve_control_nodes(&dom, styles, &mut focusables);
        Page { url, grid, focusables, dom }
    }

    /// The full document's rendered height, in cells — bigger than any one
    /// screen for a page taller than the terminal.
    pub fn grid_rows(&self) -> usize {
        self.grid.rows_len()
    }

    /// `(col, row)` -> the index into `focusables` whose rect covers that
    /// cell, or `None` — a clean pure function, factored exactly so the
    /// follow-up mouse packet (c2) can reuse it unchanged.
    pub fn hit_test(&self, col: usize, row: usize) -> Option<usize> {
        self.focusables.iter().position(|f| {
            let (c, r, w, h) = f.rect_cells;
            col >= c && col < c + w && row >= r && row < r + h
        })
    }
}

/// Map one fragment's layout-pixel rect to a screen-cell rect, using the
/// exact same rounding/clamping rules `backend::tty::render`'s own
/// `fill_box` uses (`cell_index`, clamped to `cols`, widened to at least one
/// cell) — so a focusable's highlighted region always lines up with what
/// `tty::render` actually painted there.
fn fragment_cell_rect(f: &Fragment, cols: usize) -> (usize, usize, usize, usize) {
    let row_start = tty::cell_index(f.rect.origin.y, tty::CELL_H);
    let row_end = tty::cell_index(f.rect.origin.y + nonneg(f.rect.size.h), tty::CELL_H).max(row_start + 1);
    let col_start = tty::cell_index(f.rect.origin.x, tty::CELL_W).min(cols);
    let col_end = tty::cell_index(f.rect.origin.x + nonneg(f.rect.size.w), tty::CELL_W).min(cols).max(col_start + 1).min(cols.max(col_start + 1));
    (col_start, row_start, col_end - col_start, row_end - row_start)
}

fn nonneg(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

fn union_rect(a: (usize, usize, usize, usize), b: (usize, usize, usize, usize)) -> (usize, usize, usize, usize) {
    let (ac, ar, aw, ah) = a;
    let (bc, br, bw, bh) = b;
    let left = ac.min(bc);
    let top = ar.min(br);
    let right = (ac + aw).max(bc + bw);
    let bottom = (ar + ah).max(br + bh);
    (left, top, right - left, bottom - top)
}

/// Structural (not `derive`d — `layout::Interactive` has no `PartialEq`,
/// and it's a frozen type this packet can't touch) equality: same variant,
/// same fields. Used only to decide whether two ADJACENT fragments'
/// `Interactive` values are "the same link"/"the same control" for merging.
fn same_interactive(a: &Interactive, b: &Interactive) -> bool {
    match (a, b) {
        (Interactive::Link { href: h1 }, Interactive::Link { href: h2 }) => h1 == h2,
        (
            Interactive::FormControl { kind: k1, name: n1, form_action: fa1 },
            Interactive::FormControl { kind: k2, name: n2, form_action: fa2 },
        ) => k1 == k2 && n1 == n2 && fa1 == fa2,
        _ => false,
    }
}

/// Walk `fragments` in paint (= document) order, collecting one [`Focusable`]
/// per interactive element: fragments carrying an `Interactive` are grouped
/// with their immediately-preceding sibling in the output when (and only
/// when) that sibling carries an EQUAL `Interactive` value — merging a
/// multi-line link's several `Text` fragments, or a form control's
/// wrapper+label pair, into one Tab stop. `control_node` starts `None` on
/// every entry; [`resolve_control_nodes`] fills it in for `FormControl`s.
///
/// Known v0 approximation (brief: "your call"): TWO DISTINCT ADJACENT links
/// sharing the exact same `href` (e.g. two consecutive "next page" links)
/// would merge into a single Focusable spanning the gap between them. Rare
/// in practice, not a correctness hazard (both still navigate to the same
/// place), documented for the DECISIONS ledger rather than solved here.
fn extract_focusables(fragments: &[Fragment], cols: usize) -> Vec<Focusable> {
    let mut out: Vec<Focusable> = Vec::new();
    for f in fragments {
        let Some(interactive) = &f.interactive else { continue };
        let rect = fragment_cell_rect(f, cols);
        if let Some(last) = out.last_mut() {
            if same_interactive(&last.interactive, interactive) {
                last.rect_cells = union_rect(last.rect_cells, rect);
                continue;
            }
        }
        out.push(Focusable { rect_cells: rect, interactive: interactive.clone(), control_node: None });
    }
    out
}

// =========================================================================
// DOM-side interactive walk (recovers real NodeIds for form submission)
// =========================================================================

struct DomInteractive {
    node_id: NodeId,
    form_id: Option<NodeId>,
    interactive: Interactive,
}

/// `layout::box_tree`'s own `is_link`/`is_form`/`is_form_control` predicates
/// are private to that module (not part of any frozen public surface) —
/// small enough (a handful of `match`/attribute-lookup lines each) to
/// duplicate here rather than widen box_tree's own API for one caller.
fn is_link_el(el: &Element) -> bool {
    el.name.as_str() == "a" && el.attrs.get("href").is_some()
}

fn is_form_el(el: &Element) -> bool {
    el.name.as_str() == "form"
}

fn is_form_control_el(el: &Element) -> bool {
    matches!(el.name.as_str(), "input" | "button" | "textarea" | "select")
}

/// Mirrors `box_tree::input_label`'s `"hidden" => return None` branch: a
/// hidden input gets no box at all, so it must never appear as a
/// `DomInteractive` entry either (keeping this walk's output count aligned
/// with the fragment-derived focusable list).
fn is_hidden_input(el: &Element) -> bool {
    el.name.as_str() == "input" && el.attrs.get("type").map(|t| t.eq_ignore_ascii_case("hidden")).unwrap_or(false)
}

/// Mirrors `box_tree::control_kind` exactly (`<input>`'s default `"text"`,
/// `<button>`'s default `"submit"`, any other control tag its own name).
fn control_kind_of(el: &Element) -> String {
    match el.name.as_str() {
        "input" => el.attrs.get("type").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "text".to_string()),
        "button" => el.attrs.get("type").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "submit".to_string()),
        other => other.to_string(),
    }
}

/// Collect every interactive DOM element in document order, `display: none`
/// subtrees dropped entirely (mirrors `box_tree::build_node`'s own early
/// `Display::None` return, which drops the WHOLE subtree, not just that
/// element). `form_action` on the synthesized `Interactive::FormControl` is
/// always `None` here — this walk resolves submission through the real
/// `form_id` `NodeId` (below), never needing the raw action-attribute
/// string `layout::box_tree` threads for a different purpose (painting a
/// tty placeholder), so it's never compared or read; see
/// [`same_interactive`]'s callers — this list's own `Interactive` values are
/// only ever inspected for their VARIANT (Link vs FormControl), never
/// equality-compared against the fragment-derived list.
fn collect_dom_interactive(dom: &Dom, styles: &[ComputedStyle]) -> Vec<DomInteractive> {
    let mut out = Vec::new();
    if !dom.is_empty() {
        walk_dom_interactive(dom, styles, dom.root(), 0, None, &mut out);
    }
    out
}

fn walk_dom_interactive(dom: &Dom, styles: &[ComputedStyle], id: NodeId, depth: usize, form_id: Option<NodeId>, out: &mut Vec<DomInteractive>) {
    if depth >= dom_util::DEPTH_CAP {
        return;
    }
    let Some(style) = styles.get(id) else { return };
    if style.display == Display::None {
        return;
    }
    let Node::Element(el) = dom.node(id) else { return };

    if is_form_control_el(el) {
        if is_hidden_input(el) {
            return;
        }
        let interactive = Interactive::FormControl {
            kind: control_kind_of(el).into_boxed_str(),
            name: el.attrs.get("name").map(|s| s.to_string().into_boxed_str()),
            form_action: None,
        };
        out.push(DomInteractive { node_id: id, form_id, interactive });
        return;
    }
    if is_form_el(el) {
        for &child in &el.children {
            walk_dom_interactive(dom, styles, child, depth + 1, Some(id), out);
        }
        return;
    }
    if is_link_el(el) {
        let href = el.attrs.get("href").unwrap_or("").to_string();
        out.push(DomInteractive { node_id: id, form_id, interactive: Interactive::Link { href: href.into_boxed_str() } });
        // A link's own descendants (e.g. `<a href><img></a>`) never produce
        // a SEPARATE entry: `layout::box_tree::tag_interactive` tags every
        // descendant fragment with this SAME `Interactive::Link`, so they
        // already merge into this one focusable (see `extract_focusables`).
        return;
    }
    for &child in &el.children {
        walk_dom_interactive(dom, styles, child, depth + 1, form_id, out);
    }
}

/// Zip `focusables` (fragment/document order) against a fresh DOM walk
/// (also document order) position-by-position, filling in `control_node`
/// for every `FormControl` entry. Stops at the first position where the two
/// lists' variants (Link vs FormControl) disagree — a best-effort resolver,
/// never a source of wrong associations: past that point every remaining
/// `FormControl` simply keeps `control_node: None` (Enter degrades to a
/// no-op on it, per the module's totality contract), rather than risk
/// wiring a submit button to the wrong `<form>`. See the module doc comment
/// for when the two lists can legitimately diverge.
fn resolve_control_nodes(dom: &Dom, styles: &[ComputedStyle], focusables: &mut [Focusable]) {
    let dom_list = collect_dom_interactive(dom, styles);
    let mut fi = 0;
    let mut di = 0;
    while fi < focusables.len() && di < dom_list.len() {
        let same_kind = matches!(
            (&focusables[fi].interactive, &dom_list[di].interactive),
            (Interactive::Link { .. }, Interactive::Link { .. }) | (Interactive::FormControl { .. }, Interactive::FormControl { .. })
        );
        if !same_kind {
            break;
        }
        if let Interactive::FormControl { .. } = &focusables[fi].interactive {
            focusables[fi].control_node = Some(ControlNode { control_id: dom_list[di].node_id, form_id: dom_list[di].form_id });
        }
        fi += 1;
        di += 1;
    }
}

// =========================================================================
// ViewState + actions (pure state transitions)
// =========================================================================

/// The interactive shell's mutable view over a [`Page`]: scroll position,
/// which focusable (if any) is focused, and the terminal geometry it was
/// last computed for. `Copy` — small, and every action below returns a new
/// value rather than mutating in place, keeping `apply_key` a pure function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewState {
    pub scroll_row: usize,
    pub focus: Option<usize>,
    /// Terminal width in cells (matches the `cols` `Page` was rendered at).
    pub cols: usize,
    /// CONTENT rows visible at once — terminal height MINUS the one status
    /// line `render_frame` always reserves at the bottom (see that fn).
    pub rows: usize,
}

impl ViewState {
    /// The load-time view: `rows` clamped to at least 1 (a degenerate
    /// 0-row terminal must never divide-by-zero/underflow anything below),
    /// scrolled to the top, focus on the first focusable that's already
    /// visible in that initial viewport — or, if none is (e.g. the page
    /// opens with a tall banner before its first link), the very first
    /// focusable in the document (brief: "focus = the first focusable
    /// that's visible (or the first overall)"). A page with no focusables
    /// at all starts with `focus: None`.
    pub fn initial(page: &Page, cols: usize, rows: usize) -> ViewState {
        let rows = rows.max(1);
        let focus = if page.focusables.is_empty() {
            None
        } else {
            Some(page.focusables.iter().position(|f| f.rect_cells.1 < rows).unwrap_or(0))
        };
        ViewState { scroll_row: 0, focus, cols, rows }
    }

    fn max_scroll(&self, page: &Page) -> usize {
        page.grid_rows().saturating_sub(self.rows)
    }
}

/// Bytes -> [`Key`], pure. Never produced for a byte sequence that doesn't
/// (yet, or ever) resolve to a recognized key — see [`KeyParser`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Tab,
    ShiftTab,
    Enter,
    Backspace,
    F5,
    /// `0x03` — delivered as a literal byte only because the shell's raw
    /// mode deliberately clears `ISIG` too (see `main.rs`'s raw-mode setup
    /// doc comment); otherwise the tty driver would consume it as `SIGINT`
    /// before it ever reached `read`.
    CtrlC,
    /// Any other printable ASCII byte (`0x20..=0x7e`) — `apply_key` only
    /// gives `'q'`/`'r'` any meaning today; everything else is a no-op, kept
    /// as `Char` rather than dropped so a later editing packet has
    /// something to build on.
    Char(char),
}

enum ParseOutcome {
    Empty,
    Incomplete,
    /// Discard `usize` bytes from the front of the buffer (an unrecognized
    /// or invalid prefix) and keep parsing whatever's left in the SAME
    /// `feed` call — see [`KeyParser::feed`].
    Invalid(usize),
    Complete(Key, usize),
}

/// A small buffering byte-stream -> [`Key`] parser, so a keypress whose
/// escape sequence arrives split across two `read()`s (entirely plausible
/// over a slow pty/serial link — an i486-era concern, not a theoretical
/// one) still resolves correctly instead of misparsing the tail as
/// something else. Pure: `feed` takes bytes, returns keys, no I/O.
#[derive(Debug, Default)]
pub struct KeyParser {
    buf: Vec<u8>,
}

impl KeyParser {
    pub fn new() -> Self {
        KeyParser { buf: Vec::new() }
    }

    /// Feed newly-read bytes in; returns every [`Key`] that became complete
    /// as a result (zero, one, or several — a fast paste/repeat can deliver
    /// many keys in one `read()`). Any trailing partial escape sequence
    /// stays buffered for the next call. Total: the internal loop always
    /// consumes at least one byte per iteration on anything but
    /// `Empty`/`Incomplete` (which both `break`), so it can never spin
    /// forever on adversarial input.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.buf.extend_from_slice(bytes);
        let mut keys = Vec::new();
        loop {
            match try_parse_one(&self.buf) {
                ParseOutcome::Empty | ParseOutcome::Incomplete => break,
                ParseOutcome::Invalid(n) => {
                    let n = n.min(self.buf.len());
                    self.buf.drain(0..n);
                }
                ParseOutcome::Complete(key, n) => {
                    keys.push(key);
                    let n = n.min(self.buf.len());
                    self.buf.drain(0..n);
                }
            }
        }
        keys
    }
}

fn try_parse_one(buf: &[u8]) -> ParseOutcome {
    let Some(&b0) = buf.first() else { return ParseOutcome::Empty };
    match b0 {
        0x1b => parse_escape(buf),
        0x09 => ParseOutcome::Complete(Key::Tab, 1),
        0x0d | 0x0a => ParseOutcome::Complete(Key::Enter, 1),
        0x7f | 0x08 => ParseOutcome::Complete(Key::Backspace, 1),
        0x03 => ParseOutcome::Complete(Key::CtrlC, 1),
        0x20..=0x7e => ParseOutcome::Complete(Key::Char(b0 as char), 1),
        _ => ParseOutcome::Invalid(1),
    }
}

fn parse_escape(buf: &[u8]) -> ParseOutcome {
    if buf.len() < 2 {
        return ParseOutcome::Incomplete; // a lone ESC: wait for more (see KeyParser's docs)
    }
    if buf[1] != b'[' {
        return ParseOutcome::Invalid(1); // drop just the ESC; reparse the rest fresh
    }
    if buf.len() < 3 {
        return ParseOutcome::Incomplete;
    }
    match buf[2] {
        b'A' => ParseOutcome::Complete(Key::Up, 3),
        b'B' => ParseOutcome::Complete(Key::Down, 3),
        b'C' => ParseOutcome::Complete(Key::Right, 3),
        b'D' => ParseOutcome::Complete(Key::Left, 3),
        b'Z' => ParseOutcome::Complete(Key::ShiftTab, 3),
        b'5' => parse_tilde_seq(buf, Key::PageUp),
        b'6' => parse_tilde_seq(buf, Key::PageDown),
        b'1' => parse_f5_seq(buf),
        _ => ParseOutcome::Invalid(2), // drop "\e[", reparse buf[2..] fresh
    }
}

/// `"\e[5~"` (PageUp) / `"\e[6~"` (PageDown).
fn parse_tilde_seq(buf: &[u8], key: Key) -> ParseOutcome {
    if buf.len() < 4 {
        return ParseOutcome::Incomplete;
    }
    if buf[3] == b'~' {
        ParseOutcome::Complete(key, 4)
    } else {
        ParseOutcome::Invalid(2)
    }
}

/// `"\e[15~"` (F5).
fn parse_f5_seq(buf: &[u8]) -> ParseOutcome {
    if buf.len() < 4 {
        return ParseOutcome::Incomplete;
    }
    if buf[3] != b'5' {
        return ParseOutcome::Invalid(2);
    }
    if buf.len() < 5 {
        return ParseOutcome::Incomplete;
    }
    if buf[4] == b'~' {
        ParseOutcome::Complete(Key::F5, 5)
    } else {
        ParseOutcome::Invalid(2)
    }
}

/// The outcome of an action: either nothing (a pure scroll/focus change,
/// already folded into the returned `ViewState`) or a request for the thin
/// I/O loop to go fetch something / exit.
#[derive(Debug, Clone)]
pub enum Command {
    None,
    Navigate(Url),
    Submit(Request),
    Back,
    Reload,
    Quit,
}

/// The one pure state-transition function the whole shell runs on: given a
/// key and the current view over `page`, return the next `ViewState` and
/// any `Command` the thin I/O loop should carry out. Total over any
/// `ViewState`/`Page` (including an empty page with zero focusables, or a
/// `view.focus` index stale for `page` — defensive `.get()`, never a panic).
pub fn apply_key(key: Key, view: ViewState, page: &Page) -> (ViewState, Command) {
    match key {
        Key::Up => (scroll_by(view, page, -1), Command::None),
        Key::Down => (scroll_by(view, page, 1), Command::None),
        Key::PageUp => (scroll_by(view, page, -(view.rows as i64)), Command::None),
        Key::PageDown => (scroll_by(view, page, view.rows as i64), Command::None),
        Key::Tab => (move_focus(view, page, 1), Command::None),
        Key::ShiftTab => (move_focus(view, page, -1), Command::None),
        Key::Enter => (view, enter_command(view, page)),
        Key::Backspace => (view, Command::Back),
        Key::F5 => (view, Command::Reload),
        Key::CtrlC => (view, Command::Quit),
        Key::Char('q') => (view, Command::Quit),
        Key::Char('r') => (view, Command::Reload),
        Key::Char(_) | Key::Left | Key::Right => (view, Command::None),
    }
}

fn scroll_by(view: ViewState, page: &Page, delta: i64) -> ViewState {
    let max = view.max_scroll(page) as i64;
    let next = (view.scroll_row as i64 + delta).clamp(0, max.max(0));
    ViewState { scroll_row: next as usize, ..view }
}

/// Tab (`dir: 1`) / Shift-Tab (`dir: -1`): move `focus` to the next/previous
/// focusable, WRAPPING (documented choice — brief left this "optional,
/// document"), then scroll to reveal it (see [`reveal`]). A page with no
/// focusables is a total no-op.
fn move_focus(view: ViewState, page: &Page, dir: i64) -> ViewState {
    if page.focusables.is_empty() {
        return view;
    }
    let n = page.focusables.len() as i64;
    let next_idx = match view.focus {
        None => if dir > 0 { 0 } else { n - 1 },
        Some(f) => (f as i64 + dir).rem_euclid(n),
    } as usize;
    let mut next = ViewState { focus: Some(next_idx), ..view };
    reveal(&mut next, page, next_idx);
    next
}

/// Adjust `view.scroll_row` (in place) so focusable `idx`'s rect is fully
/// within the viewport: scroll UP if its top is above the current window,
/// DOWN if its bottom is below it. A focusable taller than the whole
/// viewport (pathological, but not impossible for a hostile document)
/// aligns to its top rather than oscillating.
fn reveal(view: &mut ViewState, page: &Page, idx: usize) {
    let Some(f) = page.focusables.get(idx) else { return };
    let (_, row, _, h) = f.rect_cells;
    let bottom = row + h.saturating_sub(1);
    if row < view.scroll_row {
        view.scroll_row = row;
    } else if bottom >= view.scroll_row + view.rows {
        view.scroll_row = bottom + 1 - view.rows;
    }
    view.scroll_row = view.scroll_row.min(view.max_scroll(page));
}

/// `submit`/`image` are the only control kinds that trigger a submission
/// when activated (mirrors `form::successful_input`'s own `"submit" |
/// "image"` activator arm — a `<button>`'s default `control_kind` is
/// already `"submit"`, so this one check covers `<button>` too).
fn is_submit_kind(kind: &str) -> bool {
    matches!(kind, "submit" | "image")
}

fn enter_command(view: ViewState, page: &Page) -> Command {
    let Some(idx) = view.focus else { return Command::None };
    let Some(f) = page.focusables.get(idx) else { return Command::None };
    match &f.interactive {
        Interactive::Link { href } => Command::Navigate(page.url.resolve(href)),
        Interactive::FormControl { kind, .. } => {
            if !is_submit_kind(kind) {
                // Editing (text/checkbox/radio/select/...) is a later
                // packet — see the module doc comment's own scope note.
                return Command::None;
            }
            match f.control_node {
                Some(ControlNode { control_id, form_id: Some(form_id) }) => {
                    Command::Submit(form::serialize_submit(&page.dom, form_id, &page.url, Some(control_id)))
                }
                // No enclosing form (invalid markup) or resolution failed
                // (see `resolve_control_nodes`'s doc comment): nothing
                // sensible to submit -- degrade to a no-op, never a panic.
                _ => Command::None,
            }
        }
    }
}

// =========================================================================
// History
// =========================================================================

/// A simple back-stack: `Navigate` pushes and becomes current, `Back` pops
/// to the previous entry. No forward stack (brief: "optional — document") —
/// v0 scope call, same posture as every other "documented, not solved here"
/// choice in this packet.
#[derive(Debug, Clone)]
pub struct History {
    stack: Vec<Url>,
}

impl History {
    pub fn new(initial: Url) -> Self {
        History { stack: vec![initial] }
    }

    /// The current URL. `stack` is never empty by construction (`new`
    /// seeds it, `back` refuses to pop the last entry) — see `back`'s own
    /// doc comment.
    pub fn current(&self) -> &Url {
        self.stack.last().expect("History::stack is never empty")
    }

    pub fn navigate(&mut self, url: Url) {
        self.stack.push(url);
    }

    /// Pop back to the previous URL; a no-op (returns `false`) at the very
    /// first entry — there's nothing further back to go, and popping the
    /// last element would violate the "never empty" invariant `current`
    /// relies on.
    pub fn back(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            true
        } else {
            false
        }
    }
}

// =========================================================================
// Frame rendering (pure)
// =========================================================================

/// Focus-highlight colors — reverse-video-ish but a FIXED pair (not a swap
/// of whatever the content's own fg/bg happened to be), so the highlight is
/// always visibly distinct regardless of the underlying document's colors.
/// "Your call, visible" per the packet brief; DECISIONS-worthy.
const HIGHLIGHT_FG: Color = Color::BLACK;
const HIGHLIGHT_BG: Color = Color::rgb(255, 255, 0);

/// Render one drawable frame: the `view.rows`-tall viewport window starting
/// at `view.scroll_row`, the focused focusable's cells highlighted, plus a
/// bottom status line (current URL + a short key hint) — exactly what the
/// thin I/O loop writes to the terminal each iteration (after a cursor-home
/// escape it prepends itself; that's the one bit of genuinely
/// terminal-specific glue left to `main.rs`). Deterministic: same
/// `page`/`view` always produces the same string.
pub fn render_frame(page: &Page, view: &ViewState) -> String {
    let mut window = page.grid.window(view.scroll_row, view.rows);
    if let Some(idx) = view.focus {
        if let Some(f) = page.focusables.get(idx) {
            highlight(&mut window, f.rect_cells, view.scroll_row, view.rows);
        }
    }
    format!("{}\n{}", window.to_ansi(), status_line(page, view.cols))
}

fn highlight(window: &mut TextGrid, rect: (usize, usize, usize, usize), scroll_row: usize, rows: usize) {
    let (col, row, w, h) = rect;
    let cols = window.cols();
    for r in row..row + h {
        if r < scroll_row || r >= scroll_row + rows {
            continue;
        }
        let wr = r - scroll_row;
        for c in col..(col + w).min(cols) {
            let mut cell: Cell = window.get(wr, c);
            cell.fg = HIGHLIGHT_FG;
            cell.bg = HIGHLIGHT_BG;
            window.set(wr, c, cell);
        }
    }
}

const KEY_HINT: &str = "\u{2191}\u{2193} scroll  Tab links  \u{23ce} follow  \u{232b} back  F5 reload  q quit";

/// The bottom status line: current URL + [`KEY_HINT`], padded/truncated
/// (by `char`, matching `tty`'s own per-char cell model) to exactly `cols`
/// wide so it always occupies one full, stable terminal row.
fn status_line(page: &Page, cols: usize) -> String {
    let raw = format!("{}  {}", page.url.as_str(), KEY_HINT);
    let mut chars: Vec<char> = raw.chars().collect();
    if chars.len() > cols {
        chars.truncate(cols);
    } else {
        chars.resize(cols, ' ');
    }
    chars.into_iter().collect()
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom as dom_mod;
    use crate::fetch::Method;
    use crate::layout::box_tree::build_box_tree;
    use crate::layout::{self, Size};
    use crate::style::cascade;

    /// The full real pipeline (parse -> cascade -> box-tree -> layout),
    /// mirroring `main.rs`'s own `dump_text` -- but keeping `dom`/`styles`/
    /// `fragments` alive so `Page::build` can use all three, unlike
    /// `dump_text`, which only needs the final rendered string.
    fn build_page(html: &str, cols: usize, url: &str) -> Page {
        let dom = dom_mod::parser::parse(html);
        let styles = cascade::cascade(&dom, &[]);
        let root = build_box_tree(&dom, &styles, &Default::default()).expect("non-empty fixture");
        let viewport = Size { w: cols as f32 * 8.0, h: 100_000.0 };
        let fragments = layout::layout(&root, viewport);
        // `Page::build` needs `dom`/`styles` again for NodeId resolution,
        // so re-derive them (cheap, deterministic, real fixtures only) --
        // `fragments` borrows nothing from the first `dom`/`styles` pair
        // (Fragment owns its own `Interactive` data), so this is safe.
        let dom2 = dom_mod::parser::parse(html);
        let styles2 = cascade::cascade(&dom2, &[]);
        Page::build(dom2, &styles2, &fragments, cols, Url::new(url.to_string()))
    }

    // ------------------------------------------------------- focusables

    #[test]
    fn extracts_two_links_and_a_form_control_in_document_order() {
        let html = r#"<a href="/one">One</a><p>text</p><a href="/two">Two</a>
            <form action="/go" method="get"><input type="text" name="q"><input type="submit" value="Go"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        assert_eq!(page.focusables.len(), 4, "2 links + 2 controls: {:#?}", page.focusables.iter().map(|f| &f.interactive).collect::<Vec<_>>());
        match &page.focusables[0].interactive {
            Interactive::Link { href } => assert_eq!(&**href, "/one"),
            other => panic!("expected Link, got {other:?}"),
        }
        match &page.focusables[1].interactive {
            Interactive::Link { href } => assert_eq!(&**href, "/two"),
            other => panic!("expected Link, got {other:?}"),
        }
        match &page.focusables[2].interactive {
            Interactive::FormControl { kind, name, .. } => {
                assert_eq!(&**kind, "text");
                assert_eq!(name.as_deref(), Some("q"));
            }
            other => panic!("expected FormControl, got {other:?}"),
        }
        match &page.focusables[3].interactive {
            Interactive::FormControl { kind, .. } => assert_eq!(&**kind, "submit"),
            other => panic!("expected FormControl, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_finds_the_right_focusable_and_none_outside_any() {
        let html = r#"<a href="/one">One</a><br><a href="/two">Two</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        assert_eq!(page.focusables.len(), 2);
        let (c0, r0, _, _) = page.focusables[0].rect_cells;
        let (c1, r1, _, _) = page.focusables[1].rect_cells;
        assert_eq!(page.hit_test(c0, r0), Some(0));
        assert_eq!(page.hit_test(c1, r1), Some(1));
        assert_eq!(page.hit_test(70, 70), None);
    }

    // ------------------------------------------------------------- scroll

    #[test]
    fn down_and_page_down_clamp_at_max_scroll() {
        let html: String = (0..100).map(|i| format!("<p>line {i}</p>")).collect();
        let page = build_page(&html, 40, "http://example.com/");
        let max = page.grid_rows().saturating_sub(5);
        let mut view = ViewState { scroll_row: 0, focus: None, cols: 40, rows: 5 };
        for _ in 0..1000 {
            let (next, _) = apply_key(Key::Down, view, &page);
            view = next;
        }
        assert_eq!(view.scroll_row, max);
        let (after_pgdn, _) = apply_key(Key::PageDown, view, &page);
        assert_eq!(after_pgdn.scroll_row, max, "already at max, PageDown must not overshoot");
    }

    #[test]
    fn up_and_page_up_clamp_at_zero() {
        let html = "<p>hi</p>";
        let page = build_page(html, 40, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 40, rows: 5 };
        let (after_up, _) = apply_key(Key::Up, view, &page);
        assert_eq!(after_up.scroll_row, 0);
        let (after_pgup, _) = apply_key(Key::PageUp, view, &page);
        assert_eq!(after_pgup.scroll_row, 0);
    }

    // -------------------------------------------------------------- focus

    #[test]
    fn tab_cycles_focusables_in_order_and_wraps() {
        let html = r#"<a href="/a">A</a><a href="/b">B</a><a href="/c">C</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24 };
        let (v1, _) = apply_key(Key::Tab, view, &page);
        assert_eq!(v1.focus, Some(0));
        let (v2, _) = apply_key(Key::Tab, v1, &page);
        assert_eq!(v2.focus, Some(1));
        let (v3, _) = apply_key(Key::Tab, v2, &page);
        assert_eq!(v3.focus, Some(2));
        let (v4, _) = apply_key(Key::Tab, v3, &page); // wraps
        assert_eq!(v4.focus, Some(0));
    }

    #[test]
    fn shift_tab_reverses_and_wraps_from_none_to_the_last() {
        let html = r#"<a href="/a">A</a><a href="/b">B</a><a href="/c">C</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24 };
        let (v1, _) = apply_key(Key::ShiftTab, view, &page);
        assert_eq!(v1.focus, Some(2));
        let (v2, _) = apply_key(Key::ShiftTab, v1, &page);
        assert_eq!(v2.focus, Some(1));
    }

    #[test]
    fn tab_onto_an_offscreen_focusable_scrolls_to_reveal_it() {
        let html: String = (0..200).map(|i| format!("<p>line {i}</p>")).collect::<String>() + r#"<a href="/x">link</a>"#;
        let page = build_page(&html, 40, "http://example.com/");
        let mut view = ViewState { scroll_row: 0, focus: None, cols: 40, rows: 5 };
        let (next, _) = apply_key(Key::Tab, view, &page);
        view = next;
        let idx = view.focus.expect("a focusable exists");
        let (_, row, _, h) = page.focusables[idx].rect_cells;
        assert!(row >= view.scroll_row, "focused element's top must be within the viewport");
        assert!(row + h - 1 < view.scroll_row + view.rows, "focused element's bottom must be within the viewport");
    }

    // -------------------------------------------------------------- enter

    #[test]
    fn enter_on_a_link_navigates_to_the_resolved_href() {
        let html = r#"<a href="/next">Next</a>"#;
        let page = build_page(html, 80, "http://example.com/dir/page.html");
        let view = ViewState { scroll_row: 0, focus: Some(0), cols: 80, rows: 24 };
        let (_, cmd) = apply_key(Key::Enter, view, &page);
        match cmd {
            Command::Navigate(url) => assert_eq!(url.as_str(), "http://example.com/next"),
            other => panic!("expected Navigate, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_a_submit_control_builds_the_serialized_request() {
        let html = r#"<form action="/search" method="get">
            <input type="text" name="q">
            <input type="submit" name="go" value="Go">
        </form>"#;
        let page = build_page(html, 80, "http://example.com/");
        assert_eq!(page.focusables.len(), 2);
        let view = ViewState { scroll_row: 0, focus: Some(1), cols: 80, rows: 24 };
        let (_, cmd) = apply_key(Key::Enter, view, &page);
        match cmd {
            Command::Submit(req) => {
                assert_eq!(req.method, Method::Get);
                // q's own (empty) value + only the ACTIVATED submit's own
                // name=value -- matches form::serialize_submit's own
                // "successful controls" contract.
                assert_eq!(req.url.as_str(), "http://example.com/search?q=&go=Go");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_a_plain_text_input_is_a_no_op() {
        let html = r#"<form action="/search" method="get">
            <input type="text" name="q">
            <input type="submit" name="go" value="Go">
        </form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: Some(0), cols: 80, rows: 24 };
        let (next, cmd) = apply_key(Key::Enter, view, &page);
        assert!(matches!(cmd, Command::None));
        assert_eq!(next, view);
    }

    // ------------------------------------------------------------- history

    #[test]
    fn navigate_then_back_returns_the_prior_url() {
        let mut h = History::new(Url::new("http://example.com/a".to_string()));
        h.navigate(Url::new("http://example.com/b".to_string()));
        assert_eq!(h.current().as_str(), "http://example.com/b");
        assert!(h.back());
        assert_eq!(h.current().as_str(), "http://example.com/a");
    }

    #[test]
    fn back_at_the_first_entry_is_a_no_op() {
        let mut h = History::new(Url::new("http://example.com/a".to_string()));
        assert!(!h.back());
        assert_eq!(h.current().as_str(), "http://example.com/a");
    }

    #[test]
    fn reload_keeps_the_current_url() {
        // "Reload" has no dedicated History method: the shell just re-fetches
        // `history.current()` -- this test pins that `current()` is stable
        // across calls with no intervening navigate/back.
        let h = History::new(Url::new("http://example.com/a".to_string()));
        assert_eq!(h.current().as_str(), h.current().as_str());
    }

    // --------------------------------------------------------- key parsing

    #[test]
    fn parses_arrow_keys() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b[A"), vec![Key::Up]);
        assert_eq!(p.feed(b"\x1b[B"), vec![Key::Down]);
        assert_eq!(p.feed(b"\x1b[C"), vec![Key::Right]);
        assert_eq!(p.feed(b"\x1b[D"), vec![Key::Left]);
    }

    #[test]
    fn parses_page_up_down_and_shift_tab_and_f5() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b[5~"), vec![Key::PageUp]);
        assert_eq!(p.feed(b"\x1b[6~"), vec![Key::PageDown]);
        assert_eq!(p.feed(b"\x1b[Z"), vec![Key::ShiftTab]);
        assert_eq!(p.feed(b"\x1b[15~"), vec![Key::F5]);
    }

    #[test]
    fn parses_tab_enter_backspace_and_plain_chars() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\t"), vec![Key::Tab]);
        assert_eq!(p.feed(b"\r"), vec![Key::Enter]);
        assert_eq!(p.feed(b"\n"), vec![Key::Enter]);
        assert_eq!(p.feed(&[0x7f]), vec![Key::Backspace]);
        assert_eq!(p.feed(&[0x08]), vec![Key::Backspace]);
        assert_eq!(p.feed(b"q"), vec![Key::Char('q')]);
        assert_eq!(p.feed(b"r"), vec![Key::Char('r')]);
        assert_eq!(p.feed(&[0x03]), vec![Key::CtrlC]);
    }

    #[test]
    fn a_partial_escape_sequence_split_across_two_feeds_still_resolves() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b"), Vec::<Key>::new(), "buffered, waiting for more");
        assert_eq!(p.feed(b"[A"), vec![Key::Up]);
    }

    #[test]
    fn a_partial_tilde_sequence_split_byte_by_byte_still_resolves() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b"), Vec::<Key>::new());
        assert_eq!(p.feed(b"["), Vec::<Key>::new());
        assert_eq!(p.feed(b"1"), Vec::<Key>::new());
        assert_eq!(p.feed(b"5"), Vec::<Key>::new());
        assert_eq!(p.feed(b"~"), vec![Key::F5]);
    }

    #[test]
    fn a_lone_escape_with_nothing_following_stays_buffered_not_a_panic() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b"), Vec::<Key>::new());
        // Nothing more ever arrives -- must not panic on repeated feeds of
        // more of the same, or an empty feed.
        assert_eq!(p.feed(b""), Vec::<Key>::new());
    }

    #[test]
    fn garbage_bytes_never_panic_and_do_not_falsely_report_special_keys() {
        let mut p = KeyParser::new();
        // A long run of high-bit-set / control bytes that never form any
        // recognized sequence -- must terminate (no infinite loop) and
        // never panic.
        let garbage: Vec<u8> = (0u8..=255).collect();
        let keys = p.feed(&garbage);
        for k in keys {
            assert!(!matches!(k, Key::Up | Key::Down | Key::Left | Key::Right | Key::PageUp | Key::PageDown | Key::F5 | Key::ShiftTab));
        }
    }

    #[test]
    fn unknown_csi_sequence_falls_back_to_the_trailing_byte_as_plain_char() {
        let mut p = KeyParser::new();
        // "\e[Q" isn't a recognized sequence; the ESC+'[' prefix is
        // discarded and 'Q' is reparsed as a plain character, matching real
        // terminal input parsers (never silently drops real keystrokes).
        assert_eq!(p.feed(b"\x1b[Q"), vec![Key::Char('Q')]);
    }

    // --------------------------------------------------------------- frame

    #[test]
    fn frame_render_windows_the_viewport_at_the_scroll_offset() {
        let html: String = (0..10).map(|i| format!("<p>line{i}</p>")).collect();
        let page = build_page(&html, 20, "http://example.com/");
        let view = ViewState { scroll_row: 2, focus: None, cols: 20, rows: 3 };
        let frame = render_frame(&page, &view);
        // 3 content lines + 1 status line = 4 lines.
        assert_eq!(frame.lines().count(), 4);
    }

    #[test]
    fn frame_render_highlights_the_focused_cells_differently_from_unfocused() {
        let html = r#"<a href="/x">link</a>"#;
        let page = build_page(html, 20, "http://example.com/");
        let unfocused = ViewState { scroll_row: 0, focus: None, cols: 20, rows: 2 };
        let focused = ViewState { scroll_row: 0, focus: Some(0), cols: 20, rows: 2 };
        let frame_unfocused = render_frame(&page, &unfocused);
        let frame_focused = render_frame(&page, &focused);
        assert_ne!(frame_unfocused, frame_focused);
        let highlight_escape = format!("48;2;{};{};{}", HIGHLIGHT_BG.r, HIGHLIGHT_BG.g, HIGHLIGHT_BG.b);
        assert!(frame_focused.contains(&highlight_escape), "focused frame should carry the highlight bg escape: {frame_focused:?}");
        assert!(!frame_unfocused.contains(&highlight_escape), "unfocused frame should not");
    }

    #[test]
    fn frame_render_status_line_carries_the_url() {
        let page = build_page("<p>hi</p>", 40, "http://example.com/page.html");
        let view = ViewState { scroll_row: 0, focus: None, cols: 40, rows: 3 };
        let frame = render_frame(&page, &view);
        let status = frame.lines().last().unwrap();
        assert!(status.contains("http://example.com/page.html"), "{status:?}");
    }

    // ----------------------------------------------------------- totality

    #[test]
    fn empty_page_tab_and_enter_are_no_ops_scroll_stays_bounded() {
        let page = build_page("", 40, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 40, rows: 5 };
        let (v1, c1) = apply_key(Key::Tab, view, &page);
        assert_eq!(v1.focus, None);
        assert!(matches!(c1, Command::None));
        let (v2, c2) = apply_key(Key::Enter, view, &page);
        assert_eq!(v2, view);
        assert!(matches!(c2, Command::None));
        let (v3, _) = apply_key(Key::Down, view, &page);
        assert_eq!(v3.scroll_row, 0);
    }

    #[test]
    fn a_huge_synthetic_page_does_not_panic_on_extraction_or_scroll() {
        let html: String = (0..3000).map(|i| format!(r#"<a href="/{i}">l{i}</a>"#)).collect();
        let page = build_page(&html, 80, "http://example.com/");
        assert!(page.focusables.len() > 1000);
        let mut view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24 };
        for _ in 0..50 {
            let (next, _) = apply_key(Key::Tab, view, &page);
            view = next;
            let (next, _) = apply_key(Key::PageDown, view, &page);
            view = next;
        }
    }

    #[test]
    fn stale_focus_index_past_the_end_of_focusables_does_not_panic() {
        let html = r#"<a href="/a">A</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: Some(999), cols: 80, rows: 24 };
        let (_, cmd) = apply_key(Key::Enter, view, &page);
        assert!(matches!(cmd, Command::None));
        let _ = render_frame(&page, &view); // must not panic either
    }
}
