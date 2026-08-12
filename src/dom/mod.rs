//! The document object model: a closed, arena-allocated tree (`ast`) plus the
//! bespoke frozen-dialect parser that fills it (`parser`, P1).

pub mod ast;
pub mod parser;

pub use ast::{AttrMap, Dom, Element, ElementName, Node, NodeId};
