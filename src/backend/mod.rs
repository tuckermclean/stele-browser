//! Renderer backends that consume `layout::Fragment`s (brief §10: P7 tty,
//! P9 fb). The tty backend is the first end-to-end target: a deterministic
//! character-grid dump, no display required, cheap to golden-test.

pub mod tty;
