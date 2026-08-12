//! Stele — a document-web browser for the 486.
//!
//! Arg parsing, backend selection, and headless mode land here as milestones
//! arrive. For now `main` is the M0 hello whose output is acceptance check A4:
//! proof that the i486 binary executes 486-legal code under `qemu-i386 -cpu 486`.
//! There is, and by construction will be, no engine anywhere in this program
//! that runs code shipped by the wire (charter C3).

fn main() {
    println!("{}", stele::HELLO_LINE);
}
