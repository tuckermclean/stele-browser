//! Box-tree builder (P7): turn a parsed + cascaded [`Dom`] into the frozen
//! `layout::LayoutNode` tree the layout engine (P6) consumes. This is the
//! production generalization of the `build_layout_tree` reference helper
//! kept local to `tests/layout_integration.rs` — same `display: none`-drop
//! semantics, plus the replaced-element (`img`) mapping that test helper
//! didn't need.
//!
//! Scope calls (see the P7 report / DECISIONS ledger):
//!   - Only `img` is treated as a replaced element in v0 (per the packet
//!     brief). Its intrinsic size comes from `width`/`height` attributes
//!     when present and parseable as a non-negative finite number; otherwise
//!     it defaults to 0x0 — a documented placeholder, not a guess at a "real"
//!     image size, since no image is decoded on this path (that's P9's fb
//!     backend).
//!   - The DOM walk is capped at [`DEPTH_CAP`] nesting levels, mirroring
//!     `layout::block`'s own (private, unexported) cap of the same value: a
//!     deeply-nested/hostile document (thousands of levels) would otherwise
//!     stack-overflow this recursive walk — a guard-page fault (`SIGABRT`)
//!     that `panic = "abort"` gives no mitigation for, exactly the bug class
//!     P6's `DEPTH_CAP` was introduced to fix (see JOURNAL 2026-08-13 / P6).
//!     Past the cap, a subtree is treated as an empty leaf: a childless
//!     `Container` box (matching `layout::block::translate_any`'s own
//!     over-depth fallback), so pathological nesting degrades gracefully
//!     instead of aborting the process.

use std::collections::HashMap;
use std::rc::Rc;

use crate::dom::{Dom, Element, Node, NodeId};
use crate::dom_util;
use crate::img::RgbaImage;
use crate::layout::inline::LINE_BREAK_SENTINEL;
use crate::layout::{BoxContent, LayoutNode, Size};
use crate::style::computed::{Display, Float};
use crate::style::ComputedStyle;

/// Mirrors `layout::block::DEPTH_CAP` (private to that module). This walk is
/// independent recursion — it never goes through `block`'s taffy translation
/// — so it needs its own bound against the same pathological-nesting attack.
const DEPTH_CAP: usize = 100;

/// The intrinsic size given to an `<img>` with no parseable `width`/`height`
/// attribute. Zero-by-zero rather than a guessed placeholder box: no image is
/// decoded on this path (P9 wires real pixel data + real intrinsic sizing),
/// so any nonzero default would be pure fiction reflected into layout.
const DEFAULT_IMG_INTRINSIC: Size = Size { w: 0.0, h: 0.0 };

/// Build the frozen `LayoutNode` box tree from a parsed + cascaded DOM.
/// `images` is the already fetch+decoded `NodeId -> RgbaImage` map (see
/// `crate::images::collect_images`) — an `<img>` whose `NodeId` has an entry
/// gets that image threaded into its `Replaced` box; one with no entry (not
/// looked up at all — e.g. the `--dump-text` path, which passes an empty map
/// to skip needless fetch/decode work — or looked up but not decoded, e.g. a
/// 404 or malformed image) still gets its `Replaced` box, just with `image:
/// None`, falling back to the intrinsic-size placeholder. Returns `None` if
/// the document is empty or its root is `display: none`.
///
/// Total: never panics on any `dom`/`styles` pairing produced by
/// `dom::parser::parse` + `style::cascade::cascade` (the styles slice is
/// always exactly `dom.len()` long from that pipeline; this function is
/// still defensive against a shorter slice via `styles.get`).
pub fn build_box_tree(
    dom: &Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
) -> Option<LayoutNode> {
    if dom.is_empty() {
        return None;
    }
    build_node(dom, styles, images, dom.root(), 0)
}

fn build_node(
    dom: &Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
    id: NodeId,
    depth: usize,
) -> Option<LayoutNode> {
    let style = styles.get(id)?.clone();
    if style.display == Display::None {
        return None;
    }
    match dom.node(id) {
        Node::Text(text) => Some(LayoutNode {
            style,
            content: BoxContent::Text(text.clone()),
            children: Vec::new(),
        }),
        Node::Element(el) => {
            if is_replaced(el) {
                let mut style = style;
                apply_align_float_hint(el, &mut style);
                return Some(LayoutNode {
                    style,
                    content: BoxContent::Replaced { intrinsic: img_intrinsic(el), image: images.get(&id).cloned() },
                    children: Vec::new(),
                });
            }
            if is_form_control(el) {
                return build_form_control(dom, el, style);
            }
            if is_br(el) {
                // M6 hardening (kitchen-sink coverage found `<br>` was a
                // total no-op — see JOURNAL/DECISIONS): synthesize a leaf
                // carrying `layout::inline::LINE_BREAK_SENTINEL` as its
                // `Text` payload, the same "leaf carrying a stand-in"
                // pattern `build_form_control`/the details marker use above
                // — `BoxContent` is frozen (no dedicated break variant), so
                // the forced-break signal has to ride inside the one payload
                // shape `Text` already offers. `layout::inline::tokenize`
                // recognizes the sentinel and turns it into a forced line
                // break instead of ordinary character data — see that
                // module's doc comments for the full rationale/totality
                // notes. `<br>` is a void element (`dom::parser`'s
                // `VOID_ELEMENTS`), so it never has real children to lose by
                // returning a childless leaf here.
                return Some(LayoutNode {
                    style,
                    content: BoxContent::Text(LINE_BREAK_SENTINEL.to_string()),
                    children: Vec::new(),
                });
            }
            if is_details(el) {
                let node = if depth >= DEPTH_CAP {
                    // Same defensive fallback as the generic branch below:
                    // past the cap, degrade to an empty leaf rather than
                    // recursing into the disclosure logic at all.
                    LayoutNode { style, content: BoxContent::Container, children: Vec::new() }
                } else {
                    build_details_node(dom, styles, images, el, style, depth)
                };
                return Some(node);
            }
            let children = if depth >= DEPTH_CAP {
                Vec::new()
            } else {
                el.children
                    .iter()
                    .filter_map(|&child| build_node(dom, styles, images, child, depth + 1))
                    .collect()
            };
            let content = if style.display == Display::TableCell {
                let (colspan, rowspan) = cell_spans(el);
                BoxContent::TableCell { colspan, rowspan }
            } else {
                BoxContent::Container
            };
            Some(LayoutNode { style, content, children })
        }
    }
}

/// Only `img` is a replaced element in v0 (brief scope: "keep to `img` for
/// now").
fn is_replaced(el: &Element) -> bool {
    el.name.as_str() == "img"
}

/// `<br>` (M6 hardening) — see the `is_br` call site in `build_node` for the
/// forced-line-break synthesis this gates.
fn is_br(el: &Element) -> bool {
    el.name.as_str() == "br"
}

/// Parse an `<img>`'s intrinsic size off its `width`/`height` attributes.
/// Only non-negative, finite pixel counts are honored (HTML attribute
/// lengths are unitless integers in v0's dialect — no `%`/`px` suffix
/// handling); anything missing, non-numeric, negative, or non-finite falls
/// back to [`DEFAULT_IMG_INTRINSIC`] component-wise.
fn img_intrinsic(el: &Element) -> Size {
    let w = parse_nonneg(el.attrs.get("width")).unwrap_or(DEFAULT_IMG_INTRINSIC.w);
    let h = parse_nonneg(el.attrs.get("height")).unwrap_or(DEFAULT_IMG_INTRINSIC.h);
    Size { w, h }
}

/// Map the real 1996 `<img align=left|right>` presentational HTML attribute
/// onto the CSS `float` this box's `ComputedStyle` already carries (M4 part
/// 1, closing DECISIONS D14's floats deferral) — `align="left"` ->
/// `Float::Left`, `align="right"` -> `Float::Right`, case-insensitively
/// (`ALIGN="LEFT"` was as common as lowercase in real 1996 markup). Only
/// applied when the cascaded `style.float` is still `Float::None`: author
/// CSS `float` (from a `<style>`/author sheet, already resolved by cascade
/// before this function runs) always wins over the presentational hint,
/// matching the general HTML4 rule that CSS overrides presentational
/// attributes. `align="top"/"middle"/"bottom"` (vertical-align, not float)
/// and any other/missing value are left alone — out of scope per the packet
/// brief, and NOT a fallback to any float side.
fn apply_align_float_hint(el: &Element, style: &mut ComputedStyle) {
    if style.float != Float::None {
        return; // author CSS float wins over the attribute hint.
    }
    let Some(align) = el.attrs.get("align") else { return };
    match align.to_ascii_lowercase().as_str() {
        "left" => style.float = Float::Left,
        "right" => style.float = Float::Right,
        _ => {} // top/middle/bottom/unknown: vertical-align territory, ignored here.
    }
}

// ---------------------------------------------------------------------------
// <details>/<summary> disclosure (M5 dialect-completeness, part 1).
//
// Real browsers give `<details>` interactive click-to-toggle behavior; there
// is no interactive shell in Stele yet (out of scope per the packet brief),
// so this is the STATIC render of whichever state the `open` attribute (a
// plain HTML boolean attribute -- present with any/no value means open,
// absent means closed) already names:
//   - WITHOUT `open`: collapsed. Only the first direct-child `<summary>` is
//     built into the box tree (the clickable label); every other child is
//     dropped entirely -- not laid out, not present in the tree at all,
//     mirroring `display: none`'s own "absent entirely" contract so nothing
//     hidden can leak into a tty dump.
//   - WITH `open` (any/no value): expanded. The summary AND every other
//     child are built normally, in original document order.
//   - No `<summary>` DIRECT child at all: real browsers show a default,
//     localized "Details" label. This dialect has one locale, so the
//     default is always the literal string "Details" (documented choice --
//     the alternative, rendering nothing at all for the label, would make a
//     collapsed no-summary `<details>` silently vanish from the page, which
//     is worse than a slightly-wrong-sounding default).
//   - Multiple `<summary>` children: only the FIRST becomes the disclosure
//     label (matches every real browser). Any later `<summary>` is just
//     ordinary content -- shown (unmarked) when open, dropped when
//     collapsed, exactly like any other non-summary child.
//   - A `<summary>` nested inside some OTHER element (`<details><div>
//     <summary>...` ) is not recognized as the direct-child label at all
//     (matches the HTML5 "first summary element child" rule) -- falls back
//     to the default "Details" label, with the wrapper + buried summary
//     themselves treated as ordinary content.
//
// Disclosure marker (documented convention, since there is no click/icon
// affordance in a text-mode dump to show open/closed otherwise): the
// rendered summary is prefixed with `"> "` when collapsed or `"v "` when
// open -- ASCII-only (matching the bitmap font's own ASCII-only glyph set),
// deterministic, and legible as a crude "twisty" in a tty golden (`v` reads
// as an open-triangle stand-in, `>` as a closed one). The marker is
// synthesized as its own leading `Text` child glued onto the summary box's
// existing children (the same "synthesize a leaf carrying a stand-in"
// pattern `build_form_control` already uses for form placeholders above),
// not spliced into the summary's own text content -- keeps the marker
// independent of whatever markup a `<summary>` happens to contain.
// ---------------------------------------------------------------------------

/// Closed-state marker -- see the module doc section above.
const SUMMARY_CLOSED_MARKER: &str = "> ";
/// Open-state marker -- see the module doc section above.
const SUMMARY_OPEN_MARKER: &str = "v ";
/// The default disclosure label shown when a `<details>` has no direct-child
/// `<summary>` at all -- matches real browsers' own default ("Details" in
/// English; this dialect has one locale).
const DEFAULT_SUMMARY_LABEL: &str = "Details";

fn is_details(el: &Element) -> bool {
    el.name.as_str() == "details"
}

/// The first `<summary>` DIRECT child of a `<details>` element's own
/// `children`, if any -- see the module doc section above for why only a
/// direct child counts.
fn find_first_summary(dom: &Dom, el: &Element) -> Option<NodeId> {
    el.children
        .iter()
        .copied()
        .find(|&c| matches!(dom.node(c), Node::Element(e) if e.name.as_str() == "summary"))
}

/// Build a `<details>` element's box per the disclosure rules documented
/// above. `depth` is guaranteed `< DEPTH_CAP` by the caller (`build_node`
/// handles the past-cap fallback itself, exactly like the generic element
/// branch does), so every recursive `build_node` call here is safe without
/// its own extra depth check.
fn build_details_node(
    dom: &Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
    el: &Element,
    style: ComputedStyle,
    depth: usize,
) -> LayoutNode {
    let is_open = el.attrs.get("open").is_some();
    let marker = if is_open { SUMMARY_OPEN_MARKER } else { SUMMARY_CLOSED_MARKER };
    let summary_id = find_first_summary(dom, el);

    let summary_box = summary_id
        .and_then(|sid| build_node(dom, styles, images, sid, depth + 1))
        .map(|mut node| {
            node.children.insert(0, marker_node(marker, &node.style));
            node
        })
        .unwrap_or_else(|| default_summary_node(marker, &style));

    let mut children = vec![summary_box];
    if is_open {
        for &child in &el.children {
            if Some(child) == summary_id {
                continue; // already placed above, markered.
            }
            if let Some(node) = build_node(dom, styles, images, child, depth + 1) {
                children.push(node);
            }
        }
    }

    LayoutNode { style, content: BoxContent::Container, children }
}

/// The synthesized marker box glued in front of a real `<summary>`'s own
/// (recursively built) children -- see the module doc section above.
fn marker_node(marker: &str, style: &ComputedStyle) -> LayoutNode {
    LayoutNode { style: style.clone(), content: BoxContent::Text(marker.to_string()), children: Vec::new() }
}

/// The synthesized `"> Details"`/`"v Details"` label shown in place of a
/// missing `<summary>` -- see the module doc section above.
fn default_summary_node(marker: &str, style: &ComputedStyle) -> LayoutNode {
    LayoutNode {
        style: style.clone(),
        content: BoxContent::Container,
        children: vec![LayoutNode {
            style: style.clone(),
            content: BoxContent::Text(format!("{marker}{DEFAULT_SUMMARY_LABEL}")),
            children: Vec::new(),
        }],
    }
}

fn parse_nonneg(raw: Option<&str>) -> Option<f32> {
    let v: f32 = raw?.trim().parse().ok()?;
    if v.is_finite() && v >= 0.0 {
        Some(v)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Form-control rendering (P-forms, part 2).
//
// Real form widgets (a text field you can type into, a real dropdown) are
// the fb backend's job (M4) -- there is no pixel surface here, only a box
// tree that eventually becomes tty text. `backend::tty` paints NOTHING for
// a plain `Box` fragment (DECISIONS D17), so a control left as an empty
// generated box would be invisible in the golden. Instead, each control
// synthesizes a small `Container` holding exactly one synthesized
// `BoxContent::Text` placeholder label (mirroring the `img` -> `Replaced`
// mapping above: a DOM element becomes a leaf-ish box carrying a stand-in,
// not its literal DOM children) -- the label flows through the ordinary
// inline/text pipeline and shows up as real characters in the tty dump.
//
// Placeholder convention (documented for the DECISIONS ledger -- see the
// packet report):
//   - text-like `<input>` (`text`/`password`/`search`/`email`/`url`/`tel`/
//     `number`, or no `type` at all): `[<value>]`, tight brackets; with no
//     `value`, `[<underscores sized to the `size` attr, default 10>]` --
//     UNDERSCORES, not literal spaces: `layout::inline`'s bespoke
//     whitespace-collapsing is unconditional in v1 ("v1 always collapses; a
//     Pre fast-path is a follow-up", per that module's own docs) -- it
//     collapses ANY run of whitespace to a single space regardless of a
//     node's `white-space` style, so a run of literal spaces here would
//     silently collapse down to one space by the time it reaches the tty
//     grid, defeating the whole point of a size-sized placeholder. `_` is
//     never whitespace, so it survives that collapsing untouched, and reads
//     just as clearly as a blank field (`[____________]`) in a text-mode
//     dump -- arguably more so, since bare spaces between `[`/`]` are
//     visually indistinguishable from the surrounding background anyway.
//     `password` masks its value with one `*` per character instead of the
//     literal text (no `value` still falls back to the same underscore
//     blank -- nothing to mask).
//   - `<input type=checkbox>`: `[x]` checked, `[ ]` unchecked.
//   - `<input type=radio>`: `(*)` checked, `( )` unchecked.
//   - `<input type=submit|image>`, `<input type=reset>`, `<input
//     type=button>`, and `<button>` (any/no `type`): `[ <label> ]` --
//     spaced brackets, distinct from the tight text-field brackets so a
//     document with both a text field and a submit button doesn't read as
//     visually identical. Label priority: `value` attribute, else (for
//     `<button>` only, which can hold markup) its child text, else a
//     default ("Submit" for submit/image/`<button>`, "Reset" for reset,
//     "Button" for the `button` input type).
//   - `<input type=hidden>`: no box, no text at all -- hidden really means
//     invisible, even in a text-mode dump.
//   - `<textarea>`: its own text content verbatim if short and single-line;
//     otherwise the first line, hard-truncated to
//     [`MAX_TEXTAREA_CHARS`] characters, with a trailing `[...]` marker
//     (also appended, untruncated, when the first line is short but more
//     lines follow) -- "first line + [...] if long", per the packet brief.
//   - `<select>`: `[ <selected option's text> v]` -- the `v` is a crude
//     ASCII stand-in for a dropdown affordance. The selected option is the
//     first descendant `<option selected>`; with none marked, the first
//     `<option>` at all (matching every real browser's fallback, not
//     spec-mandated but universal practice); with no options whatsoever,
//     the text is simply empty (`[  v]`) -- total, not a special case.
//
// `display` for all of these (UUA sheet, `style/ua.rs`): `inline`. This is
// CSS's own initial value already (`ComputedStyle::default().display ==
// Display::Inline`), so the explicit UA rule is redundant with that
// default -- it's added anyway, spelled out, so the choice is a documented
// decision rather than an accident of what the initial value happens to
// be. Inline lets a control flow naturally after its `<label>` text on the
// same line (`Name: [__________]`), which is legible in a text-mode dump;
// `block` would force every control onto its own line, wasting vertical
// space a 25x80 terminal doesn't have to spare. Real widget geometry
// (fixed pixel width matching `size`, a real dropdown affordance) is the
// fb backend's job (M4) -- this is a text placeholder, not a form widget.
// ---------------------------------------------------------------------------

/// Default `<input>` "visible width" (the `size` attribute) when absent --
/// matches the packet brief's "default ~10".
const DEFAULT_CONTROL_SIZE: usize = 10;
/// Upper bound on a (possibly hostile) `size` attribute, so a document
/// author (or attacker) can't make this synthesize an arbitrarily long
/// placeholder string. Far past any real form field.
const MAX_CONTROL_SIZE: usize = 100;
/// How many characters of a `<textarea>`'s first line are shown before
/// truncating with `[...]` -- generous enough to be legible, short enough
/// to keep a form fixture's golden readable in an 80-column dump.
const MAX_TEXTAREA_CHARS: usize = 20;

fn is_form_control(el: &Element) -> bool {
    matches!(el.name.as_str(), "input" | "button" | "textarea" | "select")
}

/// Build a form control's box: a childless-in-DOM `Container` holding
/// exactly one synthesized `Text` child (the placeholder label). Returns
/// `None` for `<input type=hidden>` (and, defensively, any future control
/// kind added to [`is_form_control`] without a matching arm below) -- no
/// box at all, matching `display: none`'s own "absent entirely" contract
/// so a hidden field can never leak its value into the tty dump.
fn build_form_control(dom: &Dom, el: &Element, style: ComputedStyle) -> Option<LayoutNode> {
    let label = control_label(dom, el)?;
    Some(LayoutNode {
        content: BoxContent::Container,
        children: vec![LayoutNode { style: style.clone(), content: BoxContent::Text(label), children: Vec::new() }],
        style,
    })
}

fn control_label(dom: &Dom, el: &Element) -> Option<String> {
    match el.name.as_str() {
        "input" => input_label(el),
        "button" => Some(bracket_spaced(&button_label(dom, el))),
        "textarea" => Some(textarea_label(dom, el)),
        "select" => Some(select_label(dom, el)),
        _ => None,
    }
}

fn input_label(el: &Element) -> Option<String> {
    let ty = el.attrs.get("type").unwrap_or("text").to_ascii_lowercase();
    Some(match ty.as_str() {
        "hidden" => return None,
        "checkbox" => {
            if dom_util::is_checked(el) {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        }
        "radio" => {
            if dom_util::is_checked(el) {
                "(*)".to_string()
            } else {
                "( )".to_string()
            }
        }
        "submit" | "image" => bracket_spaced(&attr_or(el, "value", "Submit")),
        "reset" => bracket_spaced(&attr_or(el, "value", "Reset")),
        "button" => bracket_spaced(&attr_or(el, "value", "Button")),
        "password" => bracket_tight(&password_mask(el)),
        // text/search/email/url/tel/number, and any unrecognized/future
        // type, all render as a plain text field.
        _ => bracket_tight(&text_field_value(el)),
    })
}

fn bracket_tight(s: &str) -> String {
    format!("[{s}]")
}

fn bracket_spaced(s: &str) -> String {
    format!("[ {s} ]")
}

fn attr_or(el: &Element, name: &str, default: &str) -> String {
    match el.attrs.get(name) {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => default.to_string(),
    }
}

/// `size` attribute, defaulted and clamped -- see [`DEFAULT_CONTROL_SIZE`]/
/// [`MAX_CONTROL_SIZE`]. `0` (technically meaningless as a field width) and
/// anything unparseable also fall back to the default rather than
/// synthesizing an empty/zero-width placeholder.
fn control_size(el: &Element) -> usize {
    el.attrs
        .get("size")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .map(|n| n.min(MAX_CONTROL_SIZE))
        .unwrap_or(DEFAULT_CONTROL_SIZE)
}

/// Blank-field filler character -- see the module doc comment's "Placeholder
/// convention" section for why this is `_` rather than a literal space.
const BLANK_FILL: &str = "_";

fn text_field_value(el: &Element) -> String {
    match el.attrs.get("value") {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => BLANK_FILL.repeat(control_size(el)),
    }
}

fn password_mask(el: &Element) -> String {
    match el.attrs.get("value") {
        Some(v) if !v.is_empty() => "*".repeat(v.chars().count()),
        _ => BLANK_FILL.repeat(control_size(el)),
    }
}

/// `<button>`'s label: an explicit `value` attribute wins if present
/// (mirroring `<input type=submit>`'s own value-attribute-first rule so the
/// two stay consistent), else the button's own child text (it's the one
/// form control that can hold markup/text content), else "Submit" -- HTML's
/// own default `type` for a `<button>` with no `type` attribute at all.
fn button_label(dom: &Dom, el: &Element) -> String {
    if let Some(v) = el.attrs.get("value") {
        if !v.is_empty() {
            return v.to_string();
        }
    }
    let text = dom_util::collect_text(dom, el);
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        trimmed.to_string()
    } else {
        "Submit".to_string()
    }
}

/// See the module-level "Placeholder convention" doc comment above for the
/// exact truncation rule.
fn textarea_label(dom: &Dom, el: &Element) -> String {
    let text = dom_util::collect_text(dom, el);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let has_more_lines = lines.next().is_some();
    let char_count = first.chars().count();
    if char_count > MAX_TEXTAREA_CHARS {
        let truncated: String = first.chars().take(MAX_TEXTAREA_CHARS).collect();
        format!("{truncated}[...]")
    } else if has_more_lines {
        format!("{first}[...]")
    } else {
        first.to_string()
    }
}

/// Rendering convention for a `<select>`, single- or multi-valued alike:
/// always show just the FIRST selected `<option>`'s text (or the first
/// option at all, with none marked `selected`) — this is a text-mode
/// placeholder, not a real scrollable widget (that's the fb backend's job,
/// M4), so there's no legible way to show "3 of 7 options selected" in one
/// line without real widget geometry. `dom_util::collect_options` (shared
/// with `form.rs`, which DOES need every selected option for real multi-
/// select submission) is reused here for just its first-match lookup.
fn select_label(dom: &Dom, el: &Element) -> String {
    let options = dom_util::collect_options(dom, el, 0);
    let chosen = options.iter().find(|o| o.selected).or_else(|| options.first());
    let text = chosen.map(|o| o.text.as_str()).unwrap_or("");
    format!("[ {text} v]")
}

/// Max `colspan`/`rowspan` a table cell is allowed to carry, per the HTML
/// spec's own limits on these attributes. Clamping here — rather than
/// trusting the wire — keeps the eventual column solver (P8) from being
/// handed an attacker-controlled grid width/height to iterate over.
const MAX_COLSPAN: u16 = 1000;
const MAX_ROWSPAN: u16 = 65534;

/// Parse a `<td>`/`<th>`'s `colspan`/`rowspan` attributes. Missing,
/// unparseable, or zero values default to `1` (HTML's own default and floor
/// for both attributes — a span of 0 has no visual meaning); out-of-range
/// values clamp to [`MAX_COLSPAN`]/[`MAX_ROWSPAN`] rather than being rejected
/// outright, so a hostile document degrades to a large-but-bounded cell
/// instead of losing the cell's content entirely.
fn cell_spans(el: &Element) -> (u16, u16) {
    (parse_span(el.attrs.get("colspan"), MAX_COLSPAN), parse_span(el.attrs.get("rowspan"), MAX_ROWSPAN))
}

fn parse_span(raw: Option<&str>, max: u16) -> u16 {
    // Parse as u32 first so an absurdly large literal (more digits than a
    // u16 holds) parses successfully and then clamps, rather than failing
    // to parse and silently falling back to the same default (1) a
    // deliberately malformed value would — clamping and defaulting are
    // different outcomes worth keeping distinct even though both are safe.
    let v: u32 = match raw.and_then(|s| s.trim().parse().ok()) {
        Some(v) => v,
        None => return 1,
    };
    if v == 0 {
        1
    } else {
        v.min(max as u32) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::style::cascade;
    use crate::style::parser;

    fn find(d: &dom::Dom, tag: &str) -> Option<dom::NodeId> {
        find_all(d, tag).into_iter().next()
    }

    fn find_all(d: &dom::Dom, tag: &str) -> Vec<dom::NodeId> {
        let mut out = Vec::new();
        fn walk(d: &dom::Dom, id: dom::NodeId, tag: &str, out: &mut Vec<dom::NodeId>) {
            if let Some(el) = d.node(id).element() {
                if el.name.as_str() == tag {
                    out.push(id);
                }
                for &c in &el.children {
                    walk(d, c, tag, out);
                }
            }
        }
        walk(d, d.root(), tag, &mut out);
        out
    }

    fn count_nodes(node: &LayoutNode) -> usize {
        1 + node.children.iter().map(count_nodes).sum::<usize>()
    }

    /// Concatenate every `Text` box's content across a `LayoutNode` subtree,
    /// in tree order, with NO separator inserted between sibling text nodes
    /// -- unlike `find_text` (which only ever looks inside one node's own
    /// text at a time), this lets a test assert on text that spans multiple
    /// adjacent boxes (e.g. a details-disclosure marker box glued in front
    /// of a `<summary>`'s own text box) the same way it will actually read
    /// once laid out inline.
    fn collect_all_text(node: &LayoutNode, out: &mut String) {
        if let BoxContent::Text(t) = &node.content {
            out.push_str(t);
        }
        for c in &node.children {
            collect_all_text(c, out);
        }
    }

    fn find_text<'a>(node: &'a LayoutNode, needle: &str) -> Option<&'a LayoutNode> {
        if let BoxContent::Text(t) = &node.content {
            if t.contains(needle) {
                return Some(node);
            }
        }
        for c in &node.children {
            if let Some(found) = find_text(c, needle) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn display_none_element_and_its_subtree_are_dropped() {
        let d = dom::parser::parse("<div>keep</div><div id=\"gone\">drop <b>me</b></div>");
        let sheet = parser::parse("#gone { display: none; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root not display:none");
        assert!(find_text(&root, "keep").is_some());
        assert!(find_text(&root, "drop").is_none());
        assert!(find_text(&root, "me").is_none());
    }

    #[test]
    fn text_node_maps_to_box_content_text() {
        let d = dom::parser::parse("<p>hello</p>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let text_node = find_text(&root, "hello").expect("text fragment present");
        assert!(matches!(&text_node.content, BoxContent::Text(t) if t == "hello"));
    }

    #[test]
    fn plain_element_maps_to_container_with_recursive_children() {
        let d = dom::parser::parse("<div><span>a</span><span>b</span></div>");
        let styles = cascade::cascade(&d, &[]);
        assert!(find(&d, "div").is_some());
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        // Walk down to the div's box by structural shape: it has exactly two
        // Container children, each containing one Text("a"/"b").
        let div_box = {
            fn find_div(node: &LayoutNode) -> Option<&LayoutNode> {
                if matches!(node.content, BoxContent::Container)
                    && node.children.len() == 2
                    && node.children.iter().all(|c| matches!(c.content, BoxContent::Container))
                {
                    return Some(node);
                }
                for c in &node.children {
                    if let Some(found) = find_div(c) {
                        return Some(found);
                    }
                }
                None
            }
            find_div(&root).expect("div-shaped container present")
        };
        assert!(find_text(&div_box.children[0], "a").is_some());
        assert!(find_text(&div_box.children[1], "b").is_some());
    }

    #[test]
    fn img_element_maps_to_replaced_with_attribute_intrinsic_size() {
        let d = dom::parser::parse(r#"<img src="x.png" width="120" height="80">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match img.content {
            BoxContent::Replaced { intrinsic, .. } => {
                assert_eq!(intrinsic.w, 120.0);
                assert_eq!(intrinsic.h, 80.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_with_a_decoded_entry_in_the_images_map_carries_that_image() {
        let d = dom::parser::parse(r#"<img src="x.png" width="2" height="2">"#);
        let styles = cascade::cascade(&d, &[]);
        let img_id = find(&d, "img").expect("img node present");

        let decoded = Rc::new(RgbaImage { width: 2, height: 2, pixels: vec![9, 9, 9, 255].repeat(4) });
        let mut images = HashMap::new();
        images.insert(img_id, decoded.clone());

        let root = build_box_tree(&d, &styles, &images).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match &img.content {
            BoxContent::Replaced { image, .. } => {
                let got = image.as_ref().expect("decoded image should be threaded through");
                assert!(Rc::ptr_eq(got, &decoded), "should carry the exact Rc from the images map, not a copy");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_with_no_images_map_entry_has_no_image() {
        // No entry for this <img>'s NodeId (e.g. the --dump-text path, which
        // always passes an empty map) -> `image` stays `None`, falling back
        // to the intrinsic-size placeholder, exactly like before this
        // packet's threading landed.
        let d = dom::parser::parse(r#"<img src="x.png" width="2" height="2">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match &img.content {
            BoxContent::Replaced { image, .. } => assert!(image.is_none()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_without_dimensions_defaults_to_zero_intrinsic() {
        let d = dom::parser::parse(r#"<img src="x.png">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match img.content {
            BoxContent::Replaced { intrinsic, .. } => {
                assert_eq!(intrinsic.w, 0.0);
                assert_eq!(intrinsic.h, 0.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_align_left_maps_to_float_left_presentational_hint() {
        let d = dom::parser::parse(r#"<img src="x.png" align="left" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Left);
    }

    #[test]
    fn img_align_right_maps_to_float_right_presentational_hint() {
        let d = dom::parser::parse(r#"<img src="x.png" align="right" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Right);
    }

    #[test]
    fn img_align_is_case_insensitive() {
        let d = dom::parser::parse(r#"<img src="x.png" align="LEFT" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Left);
    }

    #[test]
    fn img_align_top_middle_bottom_are_ignored_not_mapped_to_float() {
        // Out of scope (vertical-align, not float) per the packet brief.
        for v in ["top", "middle", "bottom", "nonsense"] {
            let html = format!(r#"<img src="x.png" align="{v}" width="10" height="10">"#);
            let d = dom::parser::parse(&html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
            fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
                if matches!(node.content, BoxContent::Replaced { .. }) {
                    return Some(node);
                }
                node.children.iter().find_map(find_img)
            }
            let img = find_img(&root).expect("img box present");
            assert_eq!(img.style.float, crate::style::computed::Float::None, "align={v} must not set float");
        }
    }

    #[test]
    fn author_css_float_wins_over_align_attribute_hint() {
        // Author CSS `float: right` must win over the `align="left"`
        // presentational hint (the hint only applies when the cascaded
        // `style.float` is still `Float::None`).
        let d = dom::parser::parse(r#"<img src="x.png" align="left" width="10" height="10">"#);
        let sheet = crate::style::parser::parse("img { float: right; }");
        let styles = cascade::cascade(&d, &[sheet]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Right, "author CSS float must win over align hint");
    }

    #[test]
    fn img_with_no_align_attribute_keeps_float_none() {
        let d = dom::parser::parse(r#"<img src="x.png" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::None);
    }

    #[test]
    fn nested_structure_and_order_are_preserved() {
        let d = dom::parser::parse("<ul><li>one</li><li>two</li><li>three</li></ul>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_ul(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Container) && node.children.len() == 3 {
                return Some(node);
            }
            node.children.iter().find_map(find_ul)
        }
        let ul = find_ul(&root).expect("ul-shaped container present");
        assert!(find_text(&ul.children[0], "one").is_some());
        assert!(find_text(&ul.children[1], "two").is_some());
        assert!(find_text(&ul.children[2], "three").is_some());
    }

    #[test]
    fn empty_document_yields_a_root_with_no_children() {
        let d = dom::Dom::new(); // seeded with a bare <html> root, no children
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("bare <html> root is not display:none");
        assert!(root.children.is_empty());
        assert!(matches!(root.content, BoxContent::Container));
    }

    #[test]
    fn table_cell_maps_to_box_content_table_cell_with_spans() {
        let d = dom::parser::parse(
            r#"<table><tr><td colspan="2" rowspan="3">x</td><td>y</td></tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_cells<'a>(node: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
            if matches!(node.content, BoxContent::TableCell { .. }) {
                out.push(node);
            }
            for c in &node.children {
                find_cells(c, out);
            }
        }
        let mut cells = Vec::new();
        find_cells(&root, &mut cells);
        assert_eq!(cells.len(), 2, "expected two table cells");

        match cells[0].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 2);
                assert_eq!(rowspan, 3);
            }
            _ => unreachable!(),
        }
        // The cell's children (its text content) are still built underneath
        // it, exactly as a Container's would be.
        assert!(find_text(cells[0], "x").is_some());

        match cells[1].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1, "missing colspan defaults to 1");
                assert_eq!(rowspan, 1, "missing rowspan defaults to 1");
            }
            _ => unreachable!(),
        }
        assert!(find_text(cells[1], "y").is_some());
    }

    #[test]
    fn table_cell_span_parsing_defaults_and_clamps() {
        let d = dom::parser::parse(
            r#"<table><tr>
                <td colspan="0" rowspan="0">a</td>
                <td colspan="abc" rowspan="xyz">b</td>
                <td colspan="99999" rowspan="99999">c</td>
            </tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_cells<'a>(node: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
            if matches!(node.content, BoxContent::TableCell { .. }) {
                out.push(node);
            }
            for c in &node.children {
                find_cells(c, out);
            }
        }
        let mut cells = Vec::new();
        find_cells(&root, &mut cells);
        assert_eq!(cells.len(), 3);

        // colspan="0"/rowspan="0" -> min 1, never 0.
        match cells[0].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1);
                assert_eq!(rowspan, 1);
            }
            _ => unreachable!(),
        }
        // Unparseable -> default 1.
        match cells[1].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1);
                assert_eq!(rowspan, 1);
            }
            _ => unreachable!(),
        }
        // Absurdly large -> clamped (colspan <= 1000, rowspan <= 65534).
        match cells[2].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1000);
                assert_eq!(rowspan, 65534);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn display_none_root_yields_none() {
        let d = dom::parser::parse("<html><body>x</body></html>");
        let sheet = parser::parse("html { display: none; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        assert!(build_box_tree(&d, &styles, &HashMap::new()).is_none());
    }

    // ------------------------------------------------------------------
    // Form-control rendering (P-forms, part 2): each control synthesizes a
    // placeholder `Text` label instead of laying out its DOM children (which
    // for `<input>` don't exist at all -- it's a void element -- and for
    // `<button>`/`<textarea>`/`<select>` are submission-only content, not
    // meant to be walked as ordinary boxes). See `build_form_control`'s doc
    // comment for the exact bracket convention asserted below.
    // ------------------------------------------------------------------

    #[test]
    fn text_input_renders_bracketed_value() {
        let d = dom::parser::parse(r#"<input type="text" name="a" value="hi">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[hi]").is_some());
    }

    #[test]
    fn text_input_without_type_defaults_to_text_behavior() {
        let d = dom::parser::parse(r#"<input name="a" value="hi">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[hi]").is_some());
    }

    #[test]
    fn text_input_without_value_renders_underscores_sized_to_size_attr() {
        // Underscores, not literal spaces: `layout::inline` unconditionally
        // collapses whitespace runs in v1 (see that module's own docs), so
        // a run of plain spaces here would collapse down to one space by
        // the time it reaches the tty grid -- `_` is never whitespace, so
        // it survives untouched. See `build_form_control`'s module doc
        // comment ("Placeholder convention") for the full rationale.
        let d = dom::parser::parse(r#"<input type="text" name="a" size="4">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[____]").is_some(), "expected 4 underscores inside brackets");
    }

    #[test]
    fn text_input_without_value_or_size_defaults_to_ten_underscores() {
        let d = dom::parser::parse(r#"<input type="text" name="a">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let expected = format!("[{}]", "_".repeat(10));
        assert!(find_text(&root, &expected).is_some());
    }

    #[test]
    fn password_input_masks_value_with_asterisks() {
        let d = dom::parser::parse(r#"<input type="password" name="p" value="secret">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[******]").is_some());
    }

    #[test]
    fn checkbox_shows_x_when_checked_and_blank_when_not() {
        let d = dom::parser::parse(r#"<input type="checkbox" name="c" checked>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[x]").is_some());

        let d2 = dom::parser::parse(r#"<input type="checkbox" name="c">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ ]").is_some());
    }

    #[test]
    fn radio_shows_star_when_checked_and_blank_when_not() {
        let d = dom::parser::parse(r#"<input type="radio" name="r" checked>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "(*)").is_some());

        let d2 = dom::parser::parse(r#"<input type="radio" name="r">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "( )").is_some());
    }

    #[test]
    fn submit_input_shows_value_or_default_submit_label() {
        let d = dom::parser::parse(r#"<input type="submit" value="Go">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Go ]").is_some());

        let d2 = dom::parser::parse(r#"<input type="submit">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ Submit ]").is_some());
    }

    #[test]
    fn reset_and_button_type_inputs_show_bracketed_labels() {
        let d = dom::parser::parse(r#"<input type="reset">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Reset ]").is_some());

        let d2 = dom::parser::parse(r#"<input type="button" value="Click">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ Click ]").is_some());
    }

    #[test]
    fn hidden_input_renders_nothing() {
        let d = dom::parser::parse(r#"<div>before<input type="hidden" name="x" value="topsecret123"><span>after</span></div>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "before").is_some());
        assert!(find_text(&root, "after").is_some());
        assert!(find_text(&root, "topsecret123").is_none(), "hidden input's value must never appear");
    }

    #[test]
    fn button_element_shows_value_then_child_text_then_default() {
        let d = dom::parser::parse(r#"<button>Send</button>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Send ]").is_some());

        let d2 = dom::parser::parse(r#"<button value="X">Ignored</button>"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ X ]").is_some(), "value attr takes priority over child text");

        let d3 = dom::parser::parse(r#"<button></button>"#);
        let styles3 = cascade::cascade(&d3, &[]);
        let root3 = build_box_tree(&d3, &styles3, &HashMap::new()).expect("root present");
        assert!(find_text(&root3, "[ Submit ]").is_some(), "default when no value/child text");
    }

    #[test]
    fn textarea_shows_short_text_verbatim() {
        let d = dom::parser::parse(r#"<textarea name="n">hello</textarea>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "hello").is_some());
    }

    #[test]
    fn textarea_truncates_long_first_line() {
        let d = dom::parser::parse(r#"<textarea name="n">this line is definitely longer than twenty chars</textarea>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[...]").is_some(), "long content should be truncated with an ellipsis marker");
    }

    #[test]
    fn textarea_marks_multiline_content_even_if_first_line_is_short() {
        let d = dom::parser::parse("<textarea name=\"n\">line one\nline two</textarea>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "line one[...]").is_some());
    }

    #[test]
    fn select_shows_selected_option_text() {
        let d = dom::parser::parse(
            r#"<select name="color"><option value="r">Red</option><option value="g" selected>Green</option></select>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Green v]").is_some());
    }

    #[test]
    fn select_with_no_selected_option_defaults_to_first() {
        let d = dom::parser::parse(
            r#"<select name="color"><option value="r">Red</option><option value="g">Green</option></select>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Red v]").is_some());
    }

    #[test]
    fn select_with_no_options_renders_without_panicking() {
        let d = dom::parser::parse(r#"<select name="color"></select>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[  v]").is_some());
    }

    #[test]
    fn form_controls_never_recurse_into_their_own_dom_children_as_generic_boxes() {
        // A <select>'s <option>s must not show up as their own independent
        // Container/Text boxes distinct from the synthesized label -- the
        // whole control is exactly one Container + one Text child.
        let d = dom::parser::parse(r#"<select name="c"><option>Only</option></select>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_select_box(node: &LayoutNode) -> Option<&LayoutNode> {
            let is_select_shaped = node.children.len() == 1
                && matches!(&node.children[0].content, BoxContent::Text(t) if t.contains('[') && t.contains('v'));
            if is_select_shaped {
                return Some(node);
            }
            for c in &node.children {
                if let Some(found) = find_select_box(c) {
                    return Some(found);
                }
            }
            None
        }
        let select_box = find_select_box(&root).expect("select-shaped container present");
        assert_eq!(select_box.children.len(), 1, "select must synthesize exactly one label child");
    }


    #[test]
    fn deeply_nested_dom_does_not_abort_and_returns() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str("leaf");
        for _ in 0..depth {
            html.push_str("</div>");
        }
        // `dom::parser::parse` is iterative (a `Vec`-backed stack, not
        // program-stack recursion) so it handles this depth fine — verified
        // separately. `style::cascade::cascade`'s `visit`, however, IS
        // recursive with no depth cap of its own (a pre-existing gap this
        // packet does not own/fix — flagged to the orchestrator; see the P7
        // report) and reliably stack-overflows (SIGABRT) on a DOM this deep,
        // independent of anything `build_box_tree` does. To isolate exactly
        // what THIS function is responsible for, synthesize a same-length,
        // all-default styles vector here instead of calling cascade.
        let d = dom::parser::parse(&html);
        let styles = vec![ComputedStyle::default(); d.len()];

        // Must return (not abort/hang) even though the DOM nests far past
        // DEPTH_CAP.
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
        let root = root.unwrap();

        // M1 (reviewer follow-up): don't just assert "it returned" — that
        // alone wouldn't catch a regression that silently dropped the depth
        // cap but happened not to crash at this particular depth/stack size.
        // Positively confirm the cap actually fired: the "leaf" text sits
        // 3000 levels deep, far past DEPTH_CAP, so it must be ABSENT from
        // the built tree (the over-deep subtree was truncated to an empty
        // container before ever reaching it) ...
        assert!(find_text(&root, "leaf").is_none(), "content past DEPTH_CAP should have been dropped, not built");
        // ... and the total node count must stay bounded near DEPTH_CAP, not
        // anywhere close to the full 3000-deep chain.
        let total = count_nodes(&root);
        assert!(total > 0);
        assert!(
            total <= DEPTH_CAP + 5,
            "expected the tree to be truncated near DEPTH_CAP ({DEPTH_CAP}), got {total} nodes — the depth cap may not be firing"
        );
    }

    // ------------------------------------------------------------------
    // <details>/<summary> disclosure (M5 part 1): a <details> WITHOUT an
    // `open` attribute is collapsed -- only its first <summary> child (the
    // clickable label) is built into the box tree; every other child is
    // dropped. `<details open>` (any/no value) is expanded -- the summary
    // AND every other child are built normally. See `build_details_node`'s
    // doc comment for the full disclosure-marker convention asserted below.
    // ------------------------------------------------------------------

    #[test]
    fn collapsed_details_without_open_shows_only_the_summary() {
        let d = dom::parser::parse("<details><summary>Label</summary><p>Hidden content</p></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "Label").is_some());
        assert!(find_text(&root, "Hidden content").is_none(), "non-summary children must be dropped when collapsed");
    }

    #[test]
    fn open_details_shows_the_summary_and_every_other_child() {
        let d = dom::parser::parse(r#"<details open><summary>Label</summary><p>Shown content</p></details>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "Label").is_some());
        assert!(find_text(&root, "Shown content").is_some(), "non-summary children must be kept when open");
    }

    #[test]
    fn details_open_attribute_with_any_value_still_counts_as_open() {
        // HTML boolean attribute semantics: `open`, `open=""`, `open="open"`
        // are all equally "present" -- only its ABSENCE means collapsed.
        for markup in ["<details open>", "<details open=\"\">", "<details open=\"open\">"] {
            let html = format!("{markup}<summary>Label</summary><p>Shown</p></details>");
            let d = dom::parser::parse(&html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
            assert!(find_text(&root, "Shown").is_some(), "for {markup}");
        }
    }

    #[test]
    fn details_disclosure_marker_reflects_open_vs_closed_state() {
        // Closed: `> ` prefix. Open: `v ` prefix (ASCII, deterministic --
        // see the module doc section on the disclosure-marker convention).
        let closed = dom::parser::parse("<details><summary>Label</summary></details>");
        let styles = cascade::cascade(&closed, &[]);
        let root = build_box_tree(&closed, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> Label"), "closed details should show a '> ' marker, got: {text:?}");

        let open = dom::parser::parse("<details open><summary>Label</summary></details>");
        let styles2 = cascade::cascade(&open, &[]);
        let root2 = build_box_tree(&open, &styles2, &HashMap::new()).expect("root present");
        let mut text2 = String::new();
        collect_all_text(&root2, &mut text2);
        assert!(text2.contains("v Label"), "open details should show a 'v ' marker, got: {text2:?}");
    }

    #[test]
    fn details_with_no_summary_uses_the_default_label_when_collapsed() {
        let d = dom::parser::parse("<details><p>Only content</p></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> Details"), "no <summary> should fall back to the default label, got: {text:?}");
        assert!(!text.contains("Only content"), "still collapsed: non-summary content must not appear");
    }

    #[test]
    fn details_with_no_summary_uses_the_default_label_when_open() {
        let d = dom::parser::parse("<details open><p>Only content</p></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v Details"), "no <summary> should fall back to the default label, got: {text:?}");
        assert!(text.contains("Only content"), "open: the real content should still show alongside the default label");
    }

    #[test]
    fn multiple_summaries_only_the_first_becomes_the_disclosure_label_when_open() {
        let d = dom::parser::parse("<details open><summary>First</summary><summary>Second</summary></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v First"), "the FIRST summary carries the marker, got: {text:?}");
        assert!(text.contains("Second"), "a second <summary> still renders as ordinary content when open");
        assert!(!text.contains("v Second"), "only the first summary gets the disclosure marker");
    }

    #[test]
    fn multiple_summaries_collapsed_shows_only_the_first_summary() {
        let d = dom::parser::parse("<details><summary>First</summary><summary>Second</summary></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> First"));
        assert!(!text.contains("Second"), "everything past the first summary must be dropped when collapsed");
    }

    #[test]
    fn summary_not_a_direct_child_is_not_recognized_as_the_disclosure_label() {
        // Only a DIRECT-child <summary> is the disclosure label (matches the
        // HTML5 "first summary element child" rule) -- one buried inside a
        // wrapper element falls back to the default label, and (since it's
        // not treated as the summary) is itself just ordinary content shown
        // or dropped along with everything else per the open/closed state.
        let d = dom::parser::parse("<details open><div><summary>Buried</summary></div></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v Details"), "no direct-child summary -> default label, got: {text:?}");
        assert!(text.contains("Buried"), "the buried summary still renders as ordinary content when open");
    }

    #[test]
    fn details_with_no_children_at_all_does_not_panic() {
        let d = dom::parser::parse("<details></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> Details"));
    }

    #[test]
    fn nested_details_do_not_panic() {
        let d = dom::parser::parse(
            "<details open><summary>Outer</summary><details><summary>Inner</summary><p>Inner content</p></details></details>",
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
        let root = root.unwrap();
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v Outer"));
        assert!(text.contains("> Inner"), "the nested (collapsed) details keeps its own marker/summary");
        assert!(!text.contains("Inner content"), "the nested details is itself collapsed (no open attr)");
    }

    // ------------------------------------------------------------------
    // <noscript> (M5 part 2): Stele has no JavaScript by construction, so
    // <noscript> content is exactly "what to show when scripting is
    // unavailable" -- always, here. It must render like any other block
    // container, never like <script>/<style>/<head> (display: none).
    // ------------------------------------------------------------------

    #[test]
    fn noscript_content_is_visible_not_hidden() {
        let d = dom::parser::parse("<noscript><p>fallback content</p></noscript>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "fallback content").is_some());
    }

    #[test]
    fn noscript_is_block_level_per_the_ua_sheet() {
        let d = dom::parser::parse("<noscript>x</noscript>");
        let styles = cascade::cascade(&d, &[]);
        let noscript_id = find(&d, "noscript").expect("noscript present");
        assert_eq!(styles[noscript_id].display, Display::Block);
    }

    // ------------------------------------------------------------------
    // List markers (M6): `<ul>/<ol>/<li>` render with no bullets/numbers at
    // all today -- kitchen-sink coverage flagged this gap. Each `<li>`
    // built as a direct child of a `<ul>`/`<ol>` gets a synthesized leading
    // `Text` marker glued onto its children (same "synthesize a leaf
    // carrying a stand-in" convention `build_details_node`'s disclosure
    // marker and `build_form_control`'s placeholder labels already use),
    // chosen from that `<li>`'s own (possibly author-overridden, inherited)
    // `ComputedStyle::list_style_type` and an ordinal counted per-list, in
    // document order, over `<li>` DIRECT children only. See
    // `build_list_container_node`'s doc comment for the full convention.
    // ------------------------------------------------------------------

    #[test]
    fn ul_items_each_get_a_leading_bullet_marker() {
        let d = dom::parser::parse("<ul><li>a</li><li>b</li></ul>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("* a"), "expected a bullet marker before item a, got: {text:?}");
        assert!(text.contains("* b"), "expected a bullet marker before item b, got: {text:?}");
    }

    #[test]
    fn ol_items_get_sequential_ordinal_markers() {
        let d = dom::parser::parse("<ol><li>a</li><li>b</li><li>c</li></ol>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. a"), "got: {text:?}");
        assert!(text.contains("2. b"), "got: {text:?}");
        assert!(text.contains("3. c"), "got: {text:?}");
    }

    #[test]
    fn list_style_type_none_suppresses_the_marker() {
        let d = dom::parser::parse(r#"<ul style="list-style-type: none;"><li>a</li></ul>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert_eq!(text.trim(), "a", "list-style-type: none must suppress the marker entirely, got: {text:?}");
    }

    #[test]
    fn nested_list_ordinals_restart_at_one_per_list() {
        let d = dom::parser::parse(
            "<ol><li>outer-1<ol><li>inner-1</li><li>inner-2</li></ol></li><li>outer-2</li></ol>",
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. outer-1"), "got: {text:?}");
        assert!(text.contains("1. inner-1"), "nested list must restart its own ordinal at 1, got: {text:?}");
        assert!(text.contains("2. inner-2"), "got: {text:?}");
        assert!(text.contains("2. outer-2"), "outer list's own counter must not be perturbed by the nested list, got: {text:?}");
    }

    #[test]
    fn li_with_no_list_parent_gets_no_marker_and_does_not_panic() {
        let d = dom::parser::parse("<div><li>stray</li></div>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert_eq!(text.trim(), "stray", "an <li> outside any list must render with no synthesized marker");
    }

    #[test]
    fn empty_list_does_not_panic() {
        let d = dom::parser::parse("<ul></ul><ol></ol>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
    }

    #[test]
    fn huge_ordered_list_does_not_panic_and_numbers_the_last_item() {
        let mut html = String::from("<ol>");
        for i in 0..10_000 {
            html.push_str(&format!("<li>item{i}</li>"));
        }
        html.push_str("</ol>");
        let d = dom::parser::parse(&html);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
        let root = root.unwrap();
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. item0"), "got prefix: {:?}", &text[..text.len().min(40)]);
        assert!(text.contains("10000. item9999"), "expected the last item's ordinal to reach 10000");
    }

    #[test]
    fn deeply_nested_lists_do_not_abort() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<ul><li>");
        }
        html.push_str("leaf");
        for _ in 0..depth {
            html.push_str("</li></ul>");
        }
        let d = dom::parser::parse(&html);
        let styles = vec![ComputedStyle::default(); d.len()];
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
    }

    #[test]
    fn ordered_list_honors_the_start_attribute() {
        let d = dom::parser::parse(r#"<ol start="5"><li>a</li><li>b</li></ol>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("5. a"), "got: {text:?}");
        assert!(text.contains("6. b"), "got: {text:?}");
    }
}
