//! The cascade (P2, Wave 1): fold the user-agent stylesheet and author sheets
//! onto the DOM to produce one [`ComputedStyle`] per node. The UA sheet is where
//! element semantics live (block vs inline defaults, replaced elements, form
//! controls) — `dom::ast` deliberately stays name-agnostic.

use crate::dom::Dom;
use crate::style::{ComputedStyle, Stylesheet};

/// Compute a style for every node in `dom`, indexed by `NodeId`. Author sheets
/// apply after the UA sheet, in source order (specificity + order resolved by
/// P2). This is contract-testable and gets strict test-first treatment.
pub fn cascade(_dom: &Dom, _author_sheets: &[Stylesheet]) -> Vec<ComputedStyle> {
    todo!("P2: UA sheet + author cascade -> per-node ComputedStyle")
}
