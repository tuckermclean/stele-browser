//! The bespoke frozen-dialect HTML parser with 1996-grade tag-soup recovery.
//!
//! P1 (Wave 1) owns this file. Its contract: consume arbitrary HTML text and
//! produce a [`Dom`]. Full syntax is parsed; a curated semantic set (brief §4)
//! is kept; the remainder is skipped per the standards' forward-compat rules.
//!
//! Two consumed-at-parse elements deserve note here, since this is where the
//! covenant is actually applied (the AST cannot express what this file refuses
//! to build): `<style>` contents are handed to the CSS layer, and executable
//! wire elements are discarded outright — no node is ever constructed for them,
//! which is exactly why `dom::ast` has no variant to hold one. `<noscript>`
//! content, by contrast, is rendered first-class (charter C3, the JS treaty).

use crate::dom::Dom;

/// Parse a document. Recovery rules (implied close for `p`/`li`/`td`/`tr`,
/// b/i mis-nesting tolerance, unclosed-everything at EOF) are P1's remit.
pub fn parse(_input: &str) -> Dom {
    todo!("P1: bespoke tag-soup parser")
}
