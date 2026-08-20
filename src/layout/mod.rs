//! Layout: the seam between the styled DOM and positioned fragments.
//!
//! The engine is factored as solvers over a flex substrate (charter): block
//! flow is degenerate column flex, frames are nested viewports, tables are a
//! bespoke min/max-content column solver (P8) feeding fixed bases into flex.
//! Inline layout — text runs, line breaking, 1996 `img align=left` floats — is
//! bespoke and hangs off measure-function leaves (P6). This freeze fixes the
//! input tree and the fragment output; the algorithm is Wave 2.

use crate::style::ComputedStyle;

pub mod block;
pub mod box_tree;
pub mod inline;
pub mod table;
pub mod table_layout;

/// A width/height pair in layout space (CSS pixels, pre-device-scaling).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// A point in layout space (origin top-left).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A positioned rectangle in layout space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

/// The input to layout: a styled box tree. Everything reduces to this.
pub struct LayoutNode {
    pub style: ComputedStyle,
    pub content: BoxContent,
    pub children: Vec<LayoutNode>,
    /// Interactive provenance (P7 interactive-provenance freeze amendment):
    /// `Some` when this box is part of an `<a href>` link or a form control,
    /// `None` for ordinary content. `box_tree` populates this (propagating a
    /// link's `Interactive::Link` onto every descendant box under the `<a>`,
    /// and setting `Interactive::FormControl` on a synthesized control's own
    /// box + its placeholder label); `layout::block::emit` copies it onto
    /// every `Fragment` produced for this node (including every line a
    /// wrapped link's text splits across). This packet only lands and
    /// populates the carrier — no interactive behavior (click/focus/submit)
    /// exists yet, and painters (`backend::tty`/`backend::raster`) ignore it
    /// entirely, so no rendering changes.
    pub interactive: Option<Interactive>,
}

/// Interactive provenance carried from the DOM into the rendered fragment
/// stream (P7 interactive-provenance freeze amendment) — enough for a future
/// interactive shell to highlight a focusable region, follow a link, or
/// submit a form, without re-deriving any of it from the DOM at paint time.
/// Small by design (`Box<str>`, not `String` — these are read-only once
/// built, never mutated in place): a fragment stream can be large, and this
/// rides on every fragment under an interactive element.
#[derive(Debug, Clone)]
pub enum Interactive {
    /// An `<a href>` link. `href` is the raw (unresolved) attribute value —
    /// resolving it against the document's base URL is a future interactive
    /// shell's job, not this carrier's.
    Link { href: Box<str> },
    /// A form control (`<input>`, `<button>`, `<textarea>`, `<select>`).
    /// `kind` is the control's effective type (`"text"`, `"checkbox"`,
    /// `"submit"`, `"select"`, `"button"`, ...); `name` is its `name`
    /// attribute, when present (needed to contribute a `name=value` pair on
    /// submit — see `form::serialize_submit`'s "successful controls"
    /// algorithm); `form_action` is the enclosing `<form>`'s raw `action`
    /// attribute, when the control sits inside one (unresolved — same
    /// resolve-at-submit-time deferral as `Link::href`).
    FormControl { kind: Box<str>, name: Option<Box<str>>, form_action: Option<Box<str>> },
}

/// What a box holds.
pub enum BoxContent {
    /// A generated box that lays its children out per `style.display`.
    Container,
    /// Character data for the inline engine to break into lines.
    Text(String),
    /// A replaced element with an intrinsic pixel size (img, form control).
    /// `image` is the decoded pixel data to paint, when known — `None` until
    /// the M4 images packet's fetch+decode pre-pass resolves it (or when
    /// decoding failed/was skipped), in which case the box still lays out at
    /// its `intrinsic` size but paints as a placeholder. `Rc` (not an owned
    /// `RgbaImage`) so cloning a `LayoutNode` during layout translation never
    /// copies the underlying pixel buffer — single-threaded throughout, so
    /// `Rc` (not `Arc`) is the right shared-ownership primitive here.
    Replaced { intrinsic: Size, image: Option<std::rc::Rc<crate::img::RgbaImage>> },
    /// A table cell (`<td>`/`<th>`, `display: table-cell`). Otherwise just
    /// like `Container` — its children live in `LayoutNode.children` exactly
    /// as a `Container`'s do — but it additionally carries the `colspan`/
    /// `rowspan` HTML attributes the table column solver (P8) needs to build
    /// the grid. `box_tree` has already parsed, defaulted (missing/
    /// unparseable/zero → 1), and clamped these (`colspan` <= 1000, `rowspan`
    /// <= 65534, per the HTML spec's own limits) — this carrier holds
    /// already-sane values, no further validation needed downstream.
    ///
    /// This packet only lands the carrier: until the table-layout packet
    /// wires `solve_table`, a `TableCell` still translates through
    /// `layout::block` exactly like a `Container` (a plain stacked block) —
    /// see `layout::block::translate_any`.
    TableCell { colspan: u16, rowspan: u16 },
}

/// The output of layout: paint-ordered, positioned fragments the `Surface`
/// draws. (Carrying `ComputedStyle` by value keeps the seam simple; P6 may
/// swap to a handle if profiling on the 486 asks for it.)
pub struct Fragment {
    pub rect: Rect,
    pub kind: FragmentKind,
    /// See [`LayoutNode::interactive`]'s doc comment — copied verbatim from
    /// the source `LayoutNode` by `layout::block::emit`. `None` for ordinary
    /// content; painters ignore it (no rendering change from this field's
    /// presence).
    pub interactive: Option<Interactive>,
    /// The nearest ancestor `overflow:hidden` container's border box, in the
    /// same layout-`Rect` coordinate space as `rect`, intersected through
    /// every such ancestor (Acid2 Packet 5, Task 2) — `None` means
    /// unclipped. Stamped by `layout::block::emit`'s threaded `clip`
    /// parameter; painted by `backend::raster::paint_at` via
    /// `Surface::set_clip`. A box is never clipped by its OWN `overflow:
    /// hidden` (only its descendants are), so this is the clip IN FORCE at
    /// the point `emit` pushes the fragment, not one derived from the
    /// fragment's own style.
    pub clip: Option<Rect>,
}

pub enum FragmentKind {
    /// A box's background + borders.
    Box { style: ComputedStyle },
    /// A run of text sitting on `baseline` (relative to `rect.origin.y`).
    Text {
        text: String,
        baseline: f32,
        style: ComputedStyle,
    },
    /// A decoded image to blit into `rect`.
    Image { image: crate::img::RgbaImage },
}

/// Lay `root` out into a `viewport` and return paint-ordered fragments.
///
/// M2 (P6): block flow (taffy's degenerate-column-flex substrate) + the
/// bespoke inline engine (`inline` module) for text wrapping. Tables (P8)
/// are not yet part of this pipeline — a `table`/`tr`/`td` tree lays out as
/// plain blocks until the column solver lands. Text metrics are the one
/// `Metrics` impl P5 shipped (`text::BitmapFont`, monospace) — font-family
/// selection is a later refinement once a proportional font exists.
pub fn layout(root: &LayoutNode, viewport: Size) -> Vec<Fragment> {
    let font = crate::text::BitmapFont::vga_8x16();
    block::layout_tree(root, viewport, &font)
}

/// [`layout`]'s opt-in fixed-viewport sibling (packet/fixed-viewport, design
/// doc `docs/superpowers/specs/2026-08-20-fixed-viewport-design.md`): lays
/// `root` out at a FIXED `viewport.h`, not just the content-driven height
/// `layout` always uses — so `html { overflow: hidden }` (P5's clip
/// machinery) clips the document's descendants into a `viewport.w` x
/// `viewport.h` window instead of the default content-height sprawl. `layout`
/// itself is completely untouched by this addition (same `block::layout_tree`
/// call, same behavior, every existing golden byte-identical) — this calls
/// the new `block::layout_tree_viewport` instead.
pub fn layout_viewport(root: &LayoutNode, viewport: Size) -> Vec<Fragment> {
    let font = crate::text::BitmapFont::vga_8x16();
    block::layout_tree_viewport(root, viewport, &font)
}
