//! Stele — a document-web browser for the 486.
//!
//! M0 milestone: this is a toolchain hello. Its only job is to prove that a
//! binary built for the i486 floor target
//! (`targets/i486-monolith-linux-musl.json`: cpu=i486, no CMPXCHG8B, no
//! MMX/SSE, hardware x87 float) executes 486-legal code under
//! `qemu-i386 -cpu 486`. That is acceptance check A4.
//!
//! Milestone M1 replaces this `main` with the real pipeline
//! (fetch → parse → style → layout → render). There is, and by construction
//! will be, no script engine anywhere in this program: the document AST is a
//! closed sum type with no variant for executable anything (charter C3).

fn main() {
    println!("stele 0.1.0 — M0 toolchain hello: i486 binary live");
}
