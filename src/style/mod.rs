//! Style: parse all CSS syntax, cascade a curated set, hand layout a
//! `ComputedStyle` per node. P2 (Wave 1) owns `parser` + `cascade`; this freeze
//! fixes the `ComputedStyle` shape they must produce.

pub mod author;
pub mod cascade;
pub mod computed;
mod media;
pub mod parser;

mod selector;
mod tokenizer;
mod ua;
mod value;

pub use author::{collect_author_sheets, collect_author_sheets_for_viewport};
pub use computed::ComputedStyle;
pub use parser::Stylesheet;
