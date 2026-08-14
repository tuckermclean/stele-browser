//! Stele — a document-web browser for the 486.
//!
//! This is the INTERFACE FREEZE: the small, typed, compiling core that Wave 1
//! implementers build against in parallel. Bodies are stubbed; the *shapes* are
//! the contract. Per the build brief §10, shared types change only here, in an
//! orchestrator-authored freeze packet — never inside a parallel wave packet.
//!
//! Module seams map onto the parallelization map (brief §10):
//!   - `dom`     — P1: the bespoke parser fills the arena DOM this defines.
//!   - `style`   — P2: the CSS parser + cascade produce `ComputedStyle`.
//!   - `fetch`   — P3: HTTP/file/cookies produce `Response`.
//!   - `img`     — P4: image decoders behind the `Decode` trait.
//!   - `text`    — P5: font metrics behind the `Metrics` trait.
//!   - `layout`  — P6/P8: block/inline/table over the layout-node interface.
//!   - `surface` — P7/P9: backends implement the `Surface` trait.

// The freeze is deliberately full of not-yet-called shapes. These allows are
// peeled off module-by-module as each Wave 1 seam gets wired up.
#![allow(dead_code)]

pub mod backend;
pub mod dom;
mod dom_util;
pub mod fetch;
pub mod form;
pub mod img;
pub mod layout;
pub mod style;
pub mod surface;
pub mod text;

/// The line printed by the `stele` binary. This is the M0 acceptance A4 golden
/// (`goldens/m0-hello.txt`); M1 replaces it with real rendered output, blessed
/// deliberately per the golden discipline in brief §10.
pub const HELLO_LINE: &str = "stele 0.1.0 — M0 toolchain hello: i486 binary live";
