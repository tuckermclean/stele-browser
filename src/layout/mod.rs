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
}

/// What a box holds.
pub enum BoxContent {
    /// A generated box that lays its children out per `style.display`.
    Container,
    /// Character data for the inline engine to break into lines.
    Text(String),
    /// A replaced element with an intrinsic pixel size (img, form control).
    Replaced { intrinsic: Size },
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
