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

// =========================================================================
// Mouse input (pure): xterm SGR wire parsing lives here, alongside the gpm
// byte layer further down — see that section's own doc comment for why gpm
// is split the same "pure (de)serialize, unit-tested" vs "thin socket
// connect/read loop, main.rs, manual" way this whole module already splits
// keyboard input.
// =========================================================================

/// Which physical button a press/release [`MouseEvent`] names — xterm SGR's
/// `Cb & 0b11` and gpm's `GPM_B_LEFT`/`GPM_B_MIDDLE`/`GPM_B_RIGHT` bits both
/// collapse onto this one small enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// What happened, decoded from either the xterm SGR wire format
/// ([`parse_sgr_mouse`]) or a gpm `Gpm_Event` record ([`gpm_event_to_mouse`]).
/// `Move` covers both xterm's motion bit and gpm's `GPM_MOVE`/`GPM_DRAG` —
/// kept as a variant (rather than dropped at the parse boundary) so a future
/// packet has something to build on; [`apply_mouse`] treats it as a no-op
/// today (this packet's brief only asks for click-to-follow + wheel-to-scroll).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press(MouseButton),
    Release(MouseButton),
    WheelUp,
    WheelDown,
    Move,
}

/// A decoded mouse event in VIEWPORT cell coordinates (0-based: `col`/`row`
/// `0` is the top-left cell of whatever's currently on screen — the SAME
/// coordinate space `render_frame`'s window uses, NOT the full page's cell
/// grid). [`apply_mouse`] is the one place that translates `row` into a page
/// row, via `view.scroll_row`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub col: usize,
    pub row: usize,
    pub kind: MouseKind,
}

/// Everything [`KeyParser::feed`] can produce: a keyboard [`Key`] (c1,
/// unchanged) or a mouse [`MouseEvent`] (c2, parsed from an xterm SGR
/// sequence — see [`parse_escape`]'s `b'<'` arm). One parser, two event
/// kinds — the packet brief's own "extend it to also emit mouse events"
/// framing, rather than a sibling parser duplicating all the buffering/
/// partial-sequence logic `KeyParser` already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key(Key),
    Mouse(MouseEvent),
}

enum ParseOutcome {
    Empty,
    Incomplete,
    /// Discard `usize` bytes from the front of the buffer (an unrecognized
    /// or invalid prefix) and keep parsing whatever's left in the SAME
    /// `feed` call — see [`KeyParser::feed`].
    Invalid(usize),
    Complete(InputEvent, usize),
}

/// A small buffering byte-stream -> [`InputEvent`] parser, so a keypress or
/// mouse sequence whose bytes arrive split across two `read()`s (entirely
/// plausible over a slow pty/serial link — an i486-era concern, not a
/// theoretical one) still resolves correctly instead of misparsing the tail
/// as something else. Pure: `feed` takes bytes, returns events, no I/O.
#[derive(Debug, Default)]
pub struct KeyParser {
    buf: Vec<u8>,
}

impl KeyParser {
    pub fn new() -> Self {
        KeyParser { buf: Vec::new() }
    }

    /// Feed newly-read bytes in; returns every [`InputEvent`] that became
    /// complete as a result (zero, one, or several — a fast paste/repeat or
    /// a drag can deliver many events in one `read()`). Any trailing partial
    /// sequence stays buffered for the next call. Total: the internal loop
    /// always consumes at least one byte per iteration on anything but
    /// `Empty`/`Incomplete` (which both `break`), so it can never spin
    /// forever on adversarial input.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputEvent> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        loop {
            match try_parse_one(&self.buf) {
                ParseOutcome::Empty | ParseOutcome::Incomplete => break,
                ParseOutcome::Invalid(n) => {
                    let n = n.min(self.buf.len());
                    self.buf.drain(0..n);
                }
                ParseOutcome::Complete(event, n) => {
                    events.push(event);
                    let n = n.min(self.buf.len());
                    self.buf.drain(0..n);
                }
            }
        }
        events
    }
}

fn try_parse_one(buf: &[u8]) -> ParseOutcome {
    let Some(&b0) = buf.first() else { return ParseOutcome::Empty };
    match b0 {
        0x1b => parse_escape(buf),
        0x09 => ParseOutcome::Complete(InputEvent::Key(Key::Tab), 1),
        0x0d | 0x0a => ParseOutcome::Complete(InputEvent::Key(Key::Enter), 1),
        0x7f | 0x08 => ParseOutcome::Complete(InputEvent::Key(Key::Backspace), 1),
        0x03 => ParseOutcome::Complete(InputEvent::Key(Key::CtrlC), 1),
        0x20..=0x7e => ParseOutcome::Complete(InputEvent::Key(Key::Char(b0 as char)), 1),
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
        b'A' => ParseOutcome::Complete(InputEvent::Key(Key::Up), 3),
        b'B' => ParseOutcome::Complete(InputEvent::Key(Key::Down), 3),
        b'C' => ParseOutcome::Complete(InputEvent::Key(Key::Right), 3),
        b'D' => ParseOutcome::Complete(InputEvent::Key(Key::Left), 3),
        b'Z' => ParseOutcome::Complete(InputEvent::Key(Key::ShiftTab), 3),
        b'5' => parse_tilde_seq(buf, Key::PageUp),
        b'6' => parse_tilde_seq(buf, Key::PageDown),
        b'1' => parse_f5_seq(buf),
        b'<' => parse_sgr_mouse(buf),
        _ => ParseOutcome::Invalid(2), // drop "\e[", reparse buf[2..] fresh
    }
}

/// `"\e[5~"` (PageUp) / `"\e[6~"` (PageDown).
fn parse_tilde_seq(buf: &[u8], key: Key) -> ParseOutcome {
    if buf.len() < 4 {
        return ParseOutcome::Incomplete;
    }
    if buf[3] == b'~' {
        ParseOutcome::Complete(InputEvent::Key(key), 4)
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
        ParseOutcome::Complete(InputEvent::Key(Key::F5), 5)
    } else {
        ParseOutcome::Invalid(2)
    }
}

/// Bound on how many bytes of a `<params>` body a SGR mouse sequence can
/// grow to before giving up and declaring it garbage rather than waiting
/// forever for a terminator that will never come — a hostile/binary stream
/// on stdin must never make `KeyParser::feed` buffer unboundedly. Real SGR
/// sequences are a handful of bytes (`"<0;999;999M"` is 12 including the
/// `<`); 64 is generous headroom.
const MAX_SGR_MOUSE_LEN: usize = 64;

/// `"\e[<Cb;Cx;CyM"` (button press/drag/wheel) or `"\e[<Cb;Cx;Cym"`
/// (release) — xterm's SGR extended mouse-reporting format, enabled by
/// writing `\e[?1000h\e[?1006h` at shell start (see `main.rs`). `buf[0..3]`
/// is already known to be `ESC [ <` (this fn is only reached from
/// `parse_escape`'s `b'<'` arm). Scans for the terminating `M`/`m` byte;
/// any OTHER byte in between that isn't a digit or `;` is garbage — dropped
/// (consuming through the offending byte, so the same byte is never
/// reparsed and can't cause an infinite loop), never a panic.
fn parse_sgr_mouse(buf: &[u8]) -> ParseOutcome {
    let mut i = 3;
    while i < buf.len() {
        match buf[i] {
            b'M' | b'm' => break,
            b'0'..=b'9' | b';' => i += 1,
            _ => return ParseOutcome::Invalid(i + 1),
        }
    }
    if i >= buf.len() {
        if i - 3 > MAX_SGR_MOUSE_LEN {
            return ParseOutcome::Invalid(i); // never terminated -- give up on what's buffered so far
        }
        return ParseOutcome::Incomplete;
    }
    let is_release = buf[i] == b'm';
    let consumed = i + 1;
    let body = &buf[3..i];
    match decode_sgr_body(body, is_release) {
        Some(event) => ParseOutcome::Complete(InputEvent::Mouse(event), consumed),
        None => ParseOutcome::Invalid(consumed),
    }
}

/// Parse the `Cb;Cx;Cy` body (ASCII, already isolated from its `ESC [ <`
/// prefix and `M`/`m` terminator by [`parse_sgr_mouse`]) into a
/// [`MouseEvent`]. `None` for anything malformed: non-UTF8 body, wrong
/// field count, a non-numeric field, or `Cx`/`Cy` of `0` (xterm's columns/
/// rows are 1-based; `0` never occurs on the wire and isn't representable
/// as a 0-based `usize` coordinate after subtracting 1).
fn decode_sgr_body(body: &[u8], is_release: bool) -> Option<MouseEvent> {
    let s = std::str::from_utf8(body).ok()?;
    let mut parts = s.split(';');
    let cb: u32 = parts.next()?.parse().ok()?;
    let cx: usize = parts.next()?.parse().ok()?;
    let cy: usize = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // an extra field -- not a well-formed SGR triple
    }
    if cx == 0 || cy == 0 {
        return None;
    }
    let kind = decode_sgr_button(cb, is_release)?;
    Some(MouseEvent { col: cx - 1, row: cy - 1, kind })
}

/// Decode `Cb` (the xterm SGR button/modifier byte) per the packet brief:
/// `Cb & 64` set -> a wheel notch (`Cb & 1` picks up/down); `Cb & 32` set ->
/// motion (a drag if a button is also held, a bare move otherwise — this
/// build doesn't distinguish the two, see `MouseKind::Move`'s doc comment);
/// otherwise `Cb & 0b11` names the button, and `is_release` (from the `M`
/// vs `m` terminator) picks `Press`/`Release`. `Cb & 0b11 == 3` (the legacy
/// X10 "no button" sentinel SGR mode has no need for, since SGR always uses
/// an explicit `m` suffix for releases) is treated as `Move` too, rather
/// than fabricating a button that was never actually pressed.
fn decode_sgr_button(cb: u32, is_release: bool) -> Option<MouseKind> {
    if cb & 64 != 0 {
        return Some(if cb & 1 != 0 { MouseKind::WheelDown } else { MouseKind::WheelUp });
    }
    if cb & 32 != 0 {
        return Some(MouseKind::Move);
    }
    let button = match cb & 0b11 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        _ => return Some(MouseKind::Move),
    };
    Some(if is_release { MouseKind::Release(button) } else { MouseKind::Press(button) })
}

// =========================================================================
// gpm client protocol (Linux-console mouse daemon) — pure byte
// (de)serialization only, unit-tested below. The live `/dev/gpmctl`
// `UnixStream` connect/write/read loop is thin, manual glue that lives in
// `main.rs` (no gpm daemon in CI, same "NOT end-to-end CI-testable" split
// this whole module already applies to raw-mode termios).
// =========================================================================

/// `Gpm_Connect`'s exact wire layout (`gpm.h`; i486 target: little-endian,
/// natural C alignment, `short`=2B `int`=4B) — four `u16`s then two `i32`s,
/// NO padding (the `i32`s already start 4-byte aligned, at offset 8). Total
/// 16 bytes, written once immediately after connecting to `/dev/gpmctl` —
/// see `main.rs`'s connect call for the live write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpmConnect {
    pub event_mask: u16,
    pub default_mask: u16,
    pub min_mod: u16,
    pub max_mod: u16,
    pub pid: i32,
    pub vc: i32,
}

pub const GPM_CONNECT_SIZE: usize = 16;

/// `0xFFFF` — "receive every event type" (`eventMask`); see `main.rs`'s
/// connect call, which uses this for `event_mask` and `0` for the other
/// three `u16` fields (`defaultMask`/`minMod`/`maxMod` — accept any
/// modifier state, pass nothing through to a default handler).
pub const GPM_EVENT_MASK_ALL: u16 = 0xFFFF;

impl GpmConnect {
    /// Serialize to the exact 16-byte little-endian record gpm expects on
    /// the wire — see [`GpmConnect`]'s own doc comment for the byte layout.
    pub fn to_bytes(&self) -> [u8; GPM_CONNECT_SIZE] {
        let mut out = [0u8; GPM_CONNECT_SIZE];
        out[0..2].copy_from_slice(&self.event_mask.to_le_bytes());
        out[2..4].copy_from_slice(&self.default_mask.to_le_bytes());
        out[4..6].copy_from_slice(&self.min_mod.to_le_bytes());
        out[6..8].copy_from_slice(&self.max_mod.to_le_bytes());
        out[8..12].copy_from_slice(&self.pid.to_le_bytes());
        out[12..16].copy_from_slice(&self.vc.to_le_bytes());
        out
    }
}

/// `Gpm_Event`'s exact wire layout (`gpm.h`). Byte offsets (verified against
/// the natural-alignment rules the packet brief names — `int`=4B `short`=2B,
/// no `#pragma pack`):
///
/// | field       | offset | size | type |
/// |-------------|--------|------|------|
/// | buttons     | 0      | 1    | u8   |
/// | modifiers   | 1      | 1    | u8   |
/// | vc          | 2      | 2    | u16  |
/// | dx          | 4      | 2    | i16  |
/// | dy          | 6      | 2    | i16  |
/// | x           | 8      | 2    | i16  |
/// | y           | 10     | 2    | i16  |
/// | event_type  | 12     | 4    | i32  |
/// | clicks      | 16     | 4    | i32  |
/// | margin      | 20     | 4    | i32  |
/// | wdx         | 24     | 2    | i16  |
/// | wdy         | 26     | 2    | i16  |
///
/// Total 28 bytes. No padding anywhere: the four `i16`s at offsets 4-11 land
/// the following `i32` at offset 12, already 4-byte aligned; `wdx`/`wdy`
/// tack directly onto the end after the third `i32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpmEvent {
    pub buttons: u8,
    pub modifiers: u8,
    pub vc: u16,
    pub dx: i16,
    pub dy: i16,
    pub x: i16,
    pub y: i16,
    pub event_type: i32,
    pub clicks: i32,
    pub margin: i32,
    pub wdx: i16,
    pub wdy: i16,
}

pub const GPM_EVENT_SIZE: usize = 28;

pub const GPM_B_LEFT: u8 = 4;
pub const GPM_B_MIDDLE: u8 = 2;
pub const GPM_B_RIGHT: u8 = 1;

pub const GPM_MOVE: i32 = 1;
pub const GPM_DRAG: i32 = 2;
pub const GPM_DOWN: i32 = 4;
pub const GPM_UP: i32 = 8;

/// Parse one fixed-size `Gpm_Event` record off `buf` (`main.rs`'s live
/// socket read loop hands in exactly `GPM_EVENT_SIZE` bytes at a time —
/// see its own doc comment). `None` for anything shorter than
/// `GPM_EVENT_SIZE` — a short/garbled read (the totality contract the
/// packet brief calls for: "a short/garbled record from the socket ->
/// skip, no panic") rather than an out-of-bounds index panic.
pub fn parse_gpm_event(buf: &[u8]) -> Option<GpmEvent> {
    if buf.len() < GPM_EVENT_SIZE {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
    let i16_at = |i: usize| i16::from_le_bytes([buf[i], buf[i + 1]]);
    let i32_at = |i: usize| i32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    Some(GpmEvent {
        buttons: buf[0],
        modifiers: buf[1],
        vc: u16_at(2),
        dx: i16_at(4),
        dy: i16_at(6),
        x: i16_at(8),
        y: i16_at(10),
        event_type: i32_at(12),
        clicks: i32_at(16),
        margin: i32_at(20),
        wdx: i16_at(24),
        wdy: i16_at(26),
    })
}

/// Map a decoded `Gpm_Event` to a [`MouseEvent`] in the SAME 0-based
/// viewport-cell coordinate space [`parse_sgr_mouse`] produces (`x`/`y` are
/// gpm's 1-based absolute cell position — see [`GpmEvent`]'s field table).
/// A wheel record (`wdy != 0` — only present on gpm builds that report it,
/// per the packet brief) takes priority over the button fields, mirroring a
/// real mouse: a wheel notch and a button press never arrive as the same
/// physical event. `x`/`y` of `0` or negative (shouldn't happen on a real
/// gpm stream, but nothing stops a garbled record from claiming it) ->
/// `None`, never an underflow panic on the `- 1`.
pub fn gpm_event_to_mouse(ev: &GpmEvent) -> Option<MouseEvent> {
    if ev.x < 1 || ev.y < 1 {
        return None;
    }
    let col = (ev.x - 1) as usize;
    let row = (ev.y - 1) as usize;
    if ev.wdy != 0 {
        let kind = if ev.wdy > 0 { MouseKind::WheelUp } else { MouseKind::WheelDown };
        return Some(MouseEvent { col, row, kind });
    }
    let button = if ev.buttons & GPM_B_LEFT != 0 {
        Some(MouseButton::Left)
    } else if ev.buttons & GPM_B_RIGHT != 0 {
        Some(MouseButton::Right)
    } else if ev.buttons & GPM_B_MIDDLE != 0 {
        Some(MouseButton::Middle)
    } else {
        None
    };
    if ev.event_type & GPM_DOWN != 0 {
        return button.map(|b| MouseEvent { col, row, kind: MouseKind::Press(b) });
    }
    if ev.event_type & GPM_UP != 0 {
        return button.map(|b| MouseEvent { col, row, kind: MouseKind::Release(b) });
    }
    if ev.event_type & (GPM_MOVE | GPM_DRAG) != 0 {
        return Some(MouseEvent { col, row, kind: MouseKind::Move });
    }
    None
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

/// Scroll amount (in cell rows) for one wheel notch — "a few lines per
/// notch" (packet brief), matching common terminal-emulator/browser feel
/// without needing per-notch acceleration. Documented choice, DECISIONS-
/// worthy (not derived from anything else in this codebase).
const WHEEL_SCROLL_LINES: i64 = 3;

/// The pure mouse counterpart to [`apply_key`]: given a [`MouseEvent`] in
/// VIEWPORT cell coordinates and the current view over `page`, return the
/// next `ViewState` and any `Command` the thin I/O loop should carry out.
/// Total over any `ViewState`/`Page`/`MouseEvent` — an empty page, an
/// off-grid `row`/`col`, a click on the status line — see each branch/
/// helper's own comment.
pub fn apply_mouse(ev: MouseEvent, view: ViewState, page: &Page) -> (ViewState, Command) {
    match ev.kind {
        MouseKind::WheelUp => (scroll_by(view, page, -WHEEL_SCROLL_LINES), Command::None),
        MouseKind::WheelDown => (scroll_by(view, page, WHEEL_SCROLL_LINES), Command::None),
        MouseKind::Press(MouseButton::Left) => mouse_click(ev.col, ev.row, view, page),
        // Right/middle press, any release, and Move/Drag: no behavior is
        // defined by this packet's brief (click-to-follow + wheel-to-scroll
        // only) -- a documented no-op, not a silently-dropped bug.
        MouseKind::Press(_) | MouseKind::Release(_) | MouseKind::Move => (view, Command::None),
    }
}

/// A left click/press at viewport `(col, row)`: `row >= view.rows` is the
/// status line `render_frame` always draws below the content window (or
/// further off-grid still) — ignored (documented: "clicks on the status
/// line row are ignored"). Otherwise translate to a PAGE row via
/// `view.scroll_row` (col unchanged, `saturating_add` so an adversarial/
/// huge `row` can't overflow) and `page.hit_test` it: a hit sets
/// `view.focus` and activates it exactly like `Enter` (`enter_command`, the
/// SAME follow/submit path `apply_key(Key::Enter, ...)` uses); a miss
/// (empty space) leaves `view` untouched entirely — no focus change, no
/// command (documented choice, brief: "your call" — a stray click on empty
/// space never loses whatever was already focused, e.g. mid-form).
fn mouse_click(col: usize, row: usize, view: ViewState, page: &Page) -> (ViewState, Command) {
    if row >= view.rows {
        return (view, Command::None);
    }
    let page_row = view.scroll_row.saturating_add(row);
    match page.hit_test(col, page_row) {
        Some(idx) => {
            let next = ViewState { focus: Some(idx), ..view };
            let cmd = enter_command(next, page);
            (next, cmd)
        }
        None => (view, Command::None),
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

    fn keys(events: Vec<InputEvent>) -> Vec<Key> {
        events
            .into_iter()
            .map(|e| match e {
                InputEvent::Key(k) => k,
                InputEvent::Mouse(m) => panic!("expected only Key events, got a Mouse event: {m:?}"),
            })
            .collect()
    }

    #[test]
    fn parses_arrow_keys() {
        let mut p = KeyParser::new();
        assert_eq!(keys(p.feed(b"\x1b[A")), vec![Key::Up]);
        assert_eq!(keys(p.feed(b"\x1b[B")), vec![Key::Down]);
        assert_eq!(keys(p.feed(b"\x1b[C")), vec![Key::Right]);
        assert_eq!(keys(p.feed(b"\x1b[D")), vec![Key::Left]);
    }

    #[test]
    fn parses_page_up_down_and_shift_tab_and_f5() {
        let mut p = KeyParser::new();
        assert_eq!(keys(p.feed(b"\x1b[5~")), vec![Key::PageUp]);
        assert_eq!(keys(p.feed(b"\x1b[6~")), vec![Key::PageDown]);
        assert_eq!(keys(p.feed(b"\x1b[Z")), vec![Key::ShiftTab]);
        assert_eq!(keys(p.feed(b"\x1b[15~")), vec![Key::F5]);
    }

    #[test]
    fn parses_tab_enter_backspace_and_plain_chars() {
        let mut p = KeyParser::new();
        assert_eq!(keys(p.feed(b"\t")), vec![Key::Tab]);
        assert_eq!(keys(p.feed(b"\r")), vec![Key::Enter]);
        assert_eq!(keys(p.feed(b"\n")), vec![Key::Enter]);
        assert_eq!(keys(p.feed(&[0x7f])), vec![Key::Backspace]);
        assert_eq!(keys(p.feed(&[0x08])), vec![Key::Backspace]);
        assert_eq!(keys(p.feed(b"q")), vec![Key::Char('q')]);
        assert_eq!(keys(p.feed(b"r")), vec![Key::Char('r')]);
        assert_eq!(keys(p.feed(&[0x03])), vec![Key::CtrlC]);
    }

    #[test]
    fn a_partial_escape_sequence_split_across_two_feeds_still_resolves() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b"), Vec::<InputEvent>::new(), "buffered, waiting for more");
        assert_eq!(keys(p.feed(b"[A")), vec![Key::Up]);
    }

    #[test]
    fn a_partial_tilde_sequence_split_byte_by_byte_still_resolves() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b"), Vec::<InputEvent>::new());
        assert_eq!(p.feed(b"["), Vec::<InputEvent>::new());
        assert_eq!(p.feed(b"1"), Vec::<InputEvent>::new());
        assert_eq!(p.feed(b"5"), Vec::<InputEvent>::new());
        assert_eq!(keys(p.feed(b"~")), vec![Key::F5]);
    }

    #[test]
    fn a_lone_escape_with_nothing_following_stays_buffered_not_a_panic() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b"), Vec::<InputEvent>::new());
        // Nothing more ever arrives -- must not panic on repeated feeds of
        // more of the same, or an empty feed.
        assert_eq!(p.feed(b""), Vec::<InputEvent>::new());
    }

    #[test]
    fn garbage_bytes_never_panic_and_do_not_falsely_report_special_keys() {
        let mut p = KeyParser::new();
        // A long run of high-bit-set / control bytes that never form any
        // recognized sequence -- must terminate (no infinite loop) and
        // never panic.
        let garbage: Vec<u8> = (0u8..=255).collect();
        let events = p.feed(&garbage);
        for e in events {
            if let InputEvent::Key(k) = e {
                assert!(!matches!(k, Key::Up | Key::Down | Key::Left | Key::Right | Key::PageUp | Key::PageDown | Key::F5 | Key::ShiftTab));
            }
        }
    }

    #[test]
    fn unknown_csi_sequence_falls_back_to_the_trailing_byte_as_plain_char() {
        let mut p = KeyParser::new();
        // "\e[Q" isn't a recognized sequence; the ESC+'[' prefix is
        // discarded and 'Q' is reparsed as a plain character, matching real
        // terminal input parsers (never silently drops real keystrokes).
        assert_eq!(keys(p.feed(b"\x1b[Q")), vec![Key::Char('Q')]);
    }

    // ------------------------------------------------------- mouse parsing

    #[test]
    fn parses_sgr_mouse_left_press() {
        let mut p = KeyParser::new();
        let events = p.feed(b"\x1b[<0;10;5M");
        assert_eq!(events, vec![InputEvent::Mouse(MouseEvent { col: 9, row: 4, kind: MouseKind::Press(MouseButton::Left) })]);
    }

    #[test]
    fn parses_sgr_mouse_wheel_up() {
        let mut p = KeyParser::new();
        let events = p.feed(b"\x1b[<64;3;2M");
        assert_eq!(events, vec![InputEvent::Mouse(MouseEvent { col: 2, row: 1, kind: MouseKind::WheelUp })]);
    }

    #[test]
    fn parses_sgr_mouse_wheel_down() {
        let mut p = KeyParser::new();
        let events = p.feed(b"\x1b[<65;3;2M");
        assert_eq!(events, vec![InputEvent::Mouse(MouseEvent { col: 2, row: 1, kind: MouseKind::WheelDown })]);
    }

    #[test]
    fn parses_sgr_mouse_right_press() {
        let mut p = KeyParser::new();
        let events = p.feed(b"\x1b[<2;1;1M");
        assert_eq!(events, vec![InputEvent::Mouse(MouseEvent { col: 0, row: 0, kind: MouseKind::Press(MouseButton::Right) })]);
    }

    #[test]
    fn parses_sgr_mouse_release() {
        let mut p = KeyParser::new();
        let events = p.feed(b"\x1b[<0;10;5m");
        assert_eq!(events, vec![InputEvent::Mouse(MouseEvent { col: 9, row: 4, kind: MouseKind::Release(MouseButton::Left) })]);
    }

    #[test]
    fn malformed_sgr_mouse_sequence_is_dropped_not_panicking() {
        let mut p = KeyParser::new();
        let events = p.feed(b"\x1b[<abc");
        assert!(events.iter().all(|e| !matches!(e, InputEvent::Mouse(_))), "malformed SGR sequence must never produce a mouse event: {events:?}");
    }

    #[test]
    fn a_partial_sgr_mouse_sequence_split_across_two_feeds_still_resolves() {
        let mut p = KeyParser::new();
        assert_eq!(p.feed(b"\x1b[<0;10;"), Vec::<InputEvent>::new(), "buffered, waiting for the rest");
        let events = p.feed(b"5M");
        assert_eq!(events, vec![InputEvent::Mouse(MouseEvent { col: 9, row: 4, kind: MouseKind::Press(MouseButton::Left) })]);
    }

    #[test]
    fn an_unterminated_sgr_mouse_sequence_past_the_length_cap_is_dropped_not_panicking() {
        let mut p = KeyParser::new();
        let mut junk = b"\x1b[<".to_vec();
        junk.extend(std::iter::repeat(b'9').take(200)); // digits forever, no M/m terminator
        let events = p.feed(&junk);
        assert!(events.iter().all(|e| !matches!(e, InputEvent::Mouse(_))));
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

    // ------------------------------------------------------------- gpm wire

    #[test]
    fn gpm_connect_serializes_to_the_exact_16_byte_record() {
        let c = GpmConnect { event_mask: GPM_EVENT_MASK_ALL, default_mask: 0, min_mod: 0, max_mod: 0, pid: 1234, vc: 1 };
        let bytes = c.to_bytes();
        let mut expected = Vec::new();
        expected.extend_from_slice(&GPM_EVENT_MASK_ALL.to_le_bytes());
        expected.extend_from_slice(&0u16.to_le_bytes());
        expected.extend_from_slice(&0u16.to_le_bytes());
        expected.extend_from_slice(&0u16.to_le_bytes());
        expected.extend_from_slice(&1234i32.to_le_bytes());
        expected.extend_from_slice(&1i32.to_le_bytes());
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[..], &expected[..]);
    }

    /// Hand-build a `Gpm_Event` byte record field-by-field at the exact
    /// offsets [`GpmEvent`]'s doc comment documents, rather than going
    /// through `GpmEvent`/a serializer (there is no client-side serializer —
    /// only gpmd ever writes these) -- this is what actually pins the wire
    /// layout down.
    fn build_gpm_event_bytes(buttons: u8, event_type: i32, x: i16, y: i16, wdy: i16) -> [u8; GPM_EVENT_SIZE] {
        let mut buf = [0u8; GPM_EVENT_SIZE];
        buf[0] = buttons;
        buf[1] = 0; // modifiers
        buf[2..4].copy_from_slice(&1u16.to_le_bytes()); // vc
        buf[4..6].copy_from_slice(&0i16.to_le_bytes()); // dx
        buf[6..8].copy_from_slice(&0i16.to_le_bytes()); // dy
        buf[8..10].copy_from_slice(&x.to_le_bytes());
        buf[10..12].copy_from_slice(&y.to_le_bytes());
        buf[12..16].copy_from_slice(&event_type.to_le_bytes());
        buf[16..20].copy_from_slice(&1i32.to_le_bytes()); // clicks
        buf[20..24].copy_from_slice(&0i32.to_le_bytes()); // margin
        buf[24..26].copy_from_slice(&0i16.to_le_bytes()); // wdx
        buf[26..28].copy_from_slice(&wdy.to_le_bytes());
        buf
    }

    #[test]
    fn gpm_event_parses_a_hand_built_left_down_record() {
        let buf = build_gpm_event_bytes(GPM_B_LEFT, GPM_DOWN, 10, 5, 0);
        let ev = parse_gpm_event(&buf).expect("a well-formed 28-byte record parses");
        assert_eq!(ev.buttons, GPM_B_LEFT);
        assert_eq!(ev.x, 10);
        assert_eq!(ev.y, 5);
        assert_eq!(ev.event_type, GPM_DOWN);
        assert_eq!(ev.wdy, 0);

        let mouse = gpm_event_to_mouse(&ev).expect("a left-down record maps to a MouseEvent");
        assert_eq!(mouse, MouseEvent { col: 9, row: 4, kind: MouseKind::Press(MouseButton::Left) });
    }

    #[test]
    fn gpm_event_wheel_record_maps_to_wheel_up_and_down() {
        let up = build_gpm_event_bytes(0, GPM_MOVE, 3, 3, 1);
        let ev_up = parse_gpm_event(&up).unwrap();
        assert_eq!(gpm_event_to_mouse(&ev_up).unwrap().kind, MouseKind::WheelUp);

        let down = build_gpm_event_bytes(0, GPM_MOVE, 3, 3, -1);
        let ev_down = parse_gpm_event(&down).unwrap();
        assert_eq!(gpm_event_to_mouse(&ev_down).unwrap().kind, MouseKind::WheelDown);
    }

    #[test]
    fn gpm_event_too_short_record_is_skipped_not_panicking() {
        let buf = [0u8; 10];
        assert_eq!(parse_gpm_event(&buf), None);
        let almost = [0u8; GPM_EVENT_SIZE - 1];
        assert_eq!(parse_gpm_event(&almost), None);
    }

    #[test]
    fn gpm_event_with_no_button_and_no_wheel_maps_to_no_mouse_event_on_a_down_type() {
        // A DOWN-typed record but `buttons == 0` (nothing pressed, hostile/
        // garbled) has no meaningful button to report -- must not
        // fabricate one.
        let buf = build_gpm_event_bytes(0, GPM_DOWN, 1, 1, 0);
        let ev = parse_gpm_event(&buf).unwrap();
        assert_eq!(gpm_event_to_mouse(&ev), None);
    }

    #[test]
    fn gpm_event_zero_or_negative_coordinates_never_underflow_panic() {
        let buf = build_gpm_event_bytes(GPM_B_LEFT, GPM_DOWN, 0, 0, 0);
        let ev = parse_gpm_event(&buf).unwrap();
        assert_eq!(gpm_event_to_mouse(&ev), None);
    }

    // ------------------------------------------------------------ apply_mouse

    #[test]
    fn apply_mouse_wheel_scrolls_a_few_lines_and_clamps() {
        let html: String = (0..100).map(|i| format!("<p>line {i}</p>")).collect();
        let page = build_page(&html, 40, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 40, rows: 5 };
        let (next, cmd) = apply_mouse(MouseEvent { col: 0, row: 0, kind: MouseKind::WheelDown }, view, &page);
        assert!(matches!(cmd, Command::None));
        assert_eq!(next.scroll_row, WHEEL_SCROLL_LINES as usize);

        let (back, _) = apply_mouse(MouseEvent { col: 0, row: 0, kind: MouseKind::WheelUp }, next, &page);
        assert_eq!(back.scroll_row, 0);
        let (still_zero, _) = apply_mouse(MouseEvent { col: 0, row: 0, kind: MouseKind::WheelUp }, back, &page);
        assert_eq!(still_zero.scroll_row, 0, "must clamp at zero, not underflow");

        let max = page.grid_rows().saturating_sub(5);
        let at_bottom = ViewState { scroll_row: max, focus: None, cols: 40, rows: 5 };
        let (after, _) = apply_mouse(MouseEvent { col: 0, row: 0, kind: MouseKind::WheelDown }, at_bottom, &page);
        assert_eq!(after.scroll_row, max, "must clamp at max, not overshoot");
    }

    #[test]
    fn apply_mouse_click_on_a_link_focuses_and_navigates() {
        let html = r#"<a href="/next">Next</a>"#;
        let page = build_page(html, 80, "http://example.com/dir/page.html");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24 };
        let (c, r, _, _) = page.focusables[0].rect_cells;
        let ev = MouseEvent { col: c, row: r, kind: MouseKind::Press(MouseButton::Left) };
        let (next, cmd) = apply_mouse(ev, view, &page);
        assert_eq!(next.focus, Some(0));
        match cmd {
            Command::Navigate(url) => assert_eq!(url.as_str(), "http://example.com/next"),
            other => panic!("expected Navigate, got {other:?}"),
        }
    }

    #[test]
    fn apply_mouse_click_on_a_submit_control_builds_the_request_accounting_for_scroll_offset() {
        let html: String = (0..50).map(|i| format!("<p>line {i}</p>")).collect::<String>()
            + r#"<form action="/search" method="get"><input type="text" name="q"><input type="submit" name="go" value="Go"></form>"#;
        let page = build_page(&html, 80, "http://example.com/");
        let submit_idx = page.focusables.len() - 1;
        let (c, r, _, _) = page.focusables[submit_idx].rect_cells;
        // Scroll so the control is a few rows down in the viewport (not at
        // viewport row 0) -- this is the whole point of the test: the click
        // coordinate is in VIEWPORT space and must be translated through
        // `scroll_row` to the right PAGE row before hit-testing.
        let scroll_row = r.saturating_sub(2);
        let view = ViewState { scroll_row, focus: None, cols: 80, rows: 24 };
        let viewport_row = r - scroll_row;
        let ev = MouseEvent { col: c, row: viewport_row, kind: MouseKind::Press(MouseButton::Left) };
        let (next, cmd) = apply_mouse(ev, view, &page);
        assert_eq!(next.focus, Some(submit_idx));
        match cmd {
            Command::Submit(req) => assert_eq!(req.url.as_str(), "http://example.com/search?q=&go=Go"),
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn apply_mouse_click_on_empty_space_is_a_no_op() {
        let html = r#"<a href="/x">link</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: Some(0), cols: 80, rows: 24 };
        let ev = MouseEvent { col: 70, row: 20, kind: MouseKind::Press(MouseButton::Left) };
        let (next, cmd) = apply_mouse(ev, view, &page);
        assert_eq!(next, view, "no focus/scroll change on a miss");
        assert!(matches!(cmd, Command::None));
    }

    #[test]
    fn apply_mouse_click_on_the_status_line_row_is_ignored() {
        let html = r#"<a href="/x">link</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 5 };
        // `view.rows` is the content-row count; row == view.rows is the
        // status line render_frame draws just below it.
        let ev = MouseEvent { col: 0, row: 5, kind: MouseKind::Press(MouseButton::Left) };
        let (next, cmd) = apply_mouse(ev, view, &page);
        assert_eq!(next, view);
        assert!(matches!(cmd, Command::None));
    }

    #[test]
    fn apply_mouse_off_grid_coordinates_never_panic() {
        let page = build_page(r#"<a href="/x">l</a>"#, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24 };
        let ev = MouseEvent { col: usize::MAX, row: usize::MAX, kind: MouseKind::Press(MouseButton::Left) };
        let (_, cmd) = apply_mouse(ev, view, &page);
        assert!(matches!(cmd, Command::None));

        // A huge column but an in-viewport row: hit_test must still just
        // report "no match" rather than panic on the bounds comparison.
        let ev2 = MouseEvent { col: usize::MAX, row: 0, kind: MouseKind::Press(MouseButton::Left) };
        let (_, cmd2) = apply_mouse(ev2, view, &page);
        assert!(matches!(cmd2, Command::None));
    }

    #[test]
    fn apply_mouse_right_and_middle_press_and_release_and_move_are_no_ops() {
        let html = r#"<a href="/x">link</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24 };
        let (c, r, _, _) = page.focusables[0].rect_cells;
        for kind in [
            MouseKind::Press(MouseButton::Right),
            MouseKind::Press(MouseButton::Middle),
            MouseKind::Release(MouseButton::Left),
            MouseKind::Move,
        ] {
            let ev = MouseEvent { col: c, row: r, kind };
            let (next, cmd) = apply_mouse(ev, view.clone(), &page);
            assert_eq!(next, view, "{kind:?} must not change view");
            assert!(matches!(cmd, Command::None), "{kind:?} must be a no-op command");
        }
    }

    // ------------------------------------------------- editable text inputs
    // packet/shell-forms (P7 c3): text `<input>`s become typeable, and
    // Enter/a form's submit control build a real `Command::Submit` out of
    // whatever the user actually typed -- see the module's ViewState doc
    // comment ("fields"/"cursor") and `apply_key`'s edit-mode dispatch.

    #[test]
    fn is_text_input_kind_truth_table() {
        for kind in ["text", "search", "email", "url", "tel", "password", "number", "", "made-up-future-type"] {
            assert!(is_text_input_kind(kind), "{kind:?} should be text-like");
        }
        for kind in ["submit", "button", "reset", "checkbox", "radio", "hidden", "image", "file"] {
            assert!(!is_text_input_kind(kind), "{kind:?} should NOT be text-like");
        }
    }

    #[test]
    fn typing_into_a_focused_text_input_appends_to_its_buffer_and_advances_the_cursor() {
        let html = r#"<form action="/search"><input type="text" name="q"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (view, _) = apply_key(Key::Tab, view, &page); // focus the input
        assert_eq!(view.focus, Some(0));
        let (view, _) = apply_key(Key::Char('c'), view, &page);
        let (view, _) = apply_key(Key::Char('a'), view, &page);
        let (view, cmd) = apply_key(Key::Char('t'), view, &page);
        assert!(matches!(cmd, Command::None));
        assert_eq!(view.fields.get(&0).map(String::as_str), Some("cat"));
        assert_eq!(view.cursor, 3);
    }

    #[test]
    fn backspace_in_edit_mode_deletes_from_the_buffer_and_is_not_command_back() {
        let html = r#"<form action="/search"><input type="text" name="q"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (view, _) = apply_key(Key::Tab, view, &page);
        let (view, _) = apply_key(Key::Char('h'), view, &page);
        let (view, _) = apply_key(Key::Char('i'), view, &page);
        let (view, cmd) = apply_key(Key::Backspace, view, &page);
        assert!(matches!(cmd, Command::None), "backspace while editing must not be Command::Back");
        assert_eq!(view.fields.get(&0).map(String::as_str), Some("h"));
        assert_eq!(view.cursor, 1);
    }

    #[test]
    fn q_and_r_type_into_a_focused_text_input_instead_of_quitting_or_reloading() {
        let html = r#"<form action="/search"><input type="text" name="q"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (view, _) = apply_key(Key::Tab, view, &page);
        let (view, cmd_q) = apply_key(Key::Char('q'), view, &page);
        assert!(matches!(cmd_q, Command::None), "q must not quit while editing");
        let (view, cmd_r) = apply_key(Key::Char('r'), view, &page);
        assert!(matches!(cmd_r, Command::None), "r must not reload while editing");
        assert_eq!(view.fields.get(&0).map(String::as_str), Some("qr"));
    }

    #[test]
    fn q_r_and_backspace_on_a_focused_link_still_quit_reload_and_go_back() {
        let html = r#"<a href="/x">link</a>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: Some(0), cols: 80, rows: 24, ..Default::default() };
        let (_, cmd_q) = apply_key(Key::Char('q'), view.clone(), &page);
        assert!(matches!(cmd_q, Command::Quit));
        let (_, cmd_r) = apply_key(Key::Char('r'), view.clone(), &page);
        assert!(matches!(cmd_r, Command::Reload));
        let (_, cmd_bs) = apply_key(Key::Backspace, view, &page);
        assert!(matches!(cmd_bs, Command::Back));
    }

    #[test]
    fn left_and_right_move_the_cursor_and_clamp_at_the_buffer_bounds() {
        let html = r#"<form action="/search"><input type="text" name="q"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (view, _) = apply_key(Key::Tab, view, &page);
        let (view, _) = apply_key(Key::Char('a'), view, &page);
        let (view, _) = apply_key(Key::Char('b'), view, &page);
        assert_eq!(view.cursor, 2);
        let (view, _) = apply_key(Key::Left, view, &page);
        assert_eq!(view.cursor, 1);
        let (view, _) = apply_key(Key::Left, view, &page);
        assert_eq!(view.cursor, 0);
        let (view, _) = apply_key(Key::Left, view, &page); // already at 0 -- must clamp, not underflow
        assert_eq!(view.cursor, 0);
        let (view, _) = apply_key(Key::Right, view, &page);
        let (view, _) = apply_key(Key::Right, view, &page);
        assert_eq!(view.cursor, 2);
        let (view, _) = apply_key(Key::Right, view, &page); // already at len -- must clamp
        assert_eq!(view.cursor, 2);
    }

    #[test]
    fn enter_in_a_filled_text_field_submits_the_owning_form_with_the_typed_value() {
        let html = r#"<form action="/search"><input name="q"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (mut view, _) = apply_key(Key::Tab, view, &page);
        for c in "hello world".chars() {
            let (next, _) = apply_key(Key::Char(c), view, &page);
            view = next;
        }
        let (_, cmd) = apply_key(Key::Enter, view, &page);
        match cmd {
            Command::Submit(req) => {
                assert!(
                    req.url.as_str() == "http://example.com/search?q=hello+world" || req.url.as_str() == "http://example.com/search?q=hello%20world",
                    "{}",
                    req.url.as_str()
                );
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn multi_field_form_persists_buffers_across_tab_and_submits_both() {
        let html = r#"<form action="/go"><input name="a"><input name="b"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        assert_eq!(page.focusables.len(), 2);
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (view, _) = apply_key(Key::Tab, view, &page); // focus field a
        let (mut view, _) = apply_key(Key::Char('1'), view, &page);
        view = apply_key(Key::Tab, view, &page).0; // focus field b, buffer for a must persist
        assert_eq!(view.fields.get(&0).map(String::as_str), Some("1"), "field a's buffer must survive the Tab");
        let (view, _) = apply_key(Key::Char('2'), view, &page);
        let (_, cmd) = apply_key(Key::Enter, view, &page);
        match cmd {
            Command::Submit(req) => assert_eq!(req.url.as_str(), "http://example.com/go?a=1&b=2"),
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_a_focused_text_input_now_submits_its_form_instead_of_a_no_op() {
        // packet/shell-forms behavior change (spec-mandated, not a
        // regression): before this packet, Enter on a focused text input
        // was a documented no-op ("editing is a later packet"). This
        // packet's whole point is teaching the shell to edit + submit text
        // fields, so Enter on one now behaves like pressing the form's own
        // submit button -- see `apply_key_editing`'s Enter arm.
        let html = r#"<form action="/search" method="get">
            <input type="text" name="q">
            <input type="submit" name="go" value="Go">
        </form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: Some(0), cols: 80, rows: 24, ..Default::default() };
        let (_, cmd) = apply_key(Key::Enter, view, &page);
        match cmd {
            // No submit button was "clicked" (Enter fired from inside the
            // text field itself, not the button) -- see `submit_owning_form`
            // -- so `go` does NOT appear; only `q`'s own (empty, untyped)
            // value does.
            Command::Submit(req) => assert_eq!(req.url.as_str(), "http://example.com/search?q="),
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn clicking_a_text_input_focuses_it_without_submitting() {
        let html = r#"<form action="/search"><input type="text" name="q"><input type="submit" value="Go"></form>"#;
        let page = build_page(html, 80, "http://example.com/");
        let view = ViewState { scroll_row: 0, focus: None, cols: 80, rows: 24, ..Default::default() };
        let (c, r, _, _) = page.focusables[0].rect_cells;
        let ev = MouseEvent { col: c, row: r, kind: MouseKind::Press(MouseButton::Left) };
        let (next, cmd) = apply_mouse(ev, view, &page);
        assert_eq!(next.focus, Some(0));
        assert!(matches!(cmd, Command::None), "clicking into a text field must not submit");
    }

    // --------------------------------------------------------- clamp_scroll

    #[test]
    fn clamp_scroll_pulls_an_out_of_range_offset_back_to_the_new_max() {
        let html: String = (0..100).map(|i| format!("<p>line {i}</p>")).collect();
        let page = build_page(&html, 40, "http://example.com/");
        let max = page.grid_rows().saturating_sub(5);
        let view = ViewState { scroll_row: max + 500, focus: None, cols: 40, rows: 5, ..Default::default() };
        let clamped = clamp_scroll(view, &page);
        assert_eq!(clamped.scroll_row, max);
    }

    #[test]
    fn clamp_scroll_leaves_an_in_range_offset_unchanged() {
        let html: String = (0..100).map(|i| format!("<p>line {i}</p>")).collect();
        let page = build_page(&html, 40, "http://example.com/");
        let view = ViewState { scroll_row: 3, focus: None, cols: 40, rows: 5, ..Default::default() };
        let clamped = clamp_scroll(view.clone(), &page);
        assert_eq!(clamped, view);
    }
}
