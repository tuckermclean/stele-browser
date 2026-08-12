//! Layout: the seam between the styled DOM and positioned fragments.
//!
//! The engine is factored as solvers over a flex substrate (charter): block
//! flow is degenerate column flex, frames are nested viewports, tables are a
//! bespoke min/max-content column solver (P8) feeding fixed bases into flex.
//! Inline layout — text runs, line breaking, 1996 `img align=left` floats — is
//! bespoke and hangs off measure-function leaves (P6). This freeze fixes the
//! input tree and the fragment output; the algorithm is Wave 2.

use crate::style::ComputedStyle;

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
pub fn layout(_root: &LayoutNode, _viewport: Size) -> Vec<Fragment> {
    todo!("P6/P8: block flow + inline engine + table column solver")
}
