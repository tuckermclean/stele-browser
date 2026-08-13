//! Block flow + the taffy translation (P6, M2): map a [`LayoutNode`] tree
//! onto taffy (charter §158's flex substrate), run layout with the bespoke
//! inline engine hanging off measure-function leaves, and walk the result
//! back into paint-ordered [`Fragment`]s.
//!
//! RED skeleton: body pending the green commit.

use crate::layout::{Fragment, LayoutNode, Size};
use crate::text::Metrics;

/// Lay `root` out into `viewport` using `metrics` for text, and return the
/// paint-ordered fragment vector. See module docs; body pending (RED
/// skeleton).
pub fn layout_tree<M: Metrics>(_root: &LayoutNode, _viewport: Size, _metrics: &M) -> Vec<Fragment> {
    todo!("P6: taffy translation + block flow + fragment emission (M2)")
}
