//! The transport seam under `http1`. `Http1Client` reads/writes over a
//! `ByteStream` rather than a concrete socket, so a different provider can
//! slot in without touching the HTTP/1.1 framing code. PR 1 ships the seam
//! and its only impl (`TcpStream`); PR 2 adds an `openssl s_client` child
//! that implements the same trait (delegated TLS — see
//! docs/superpowers/specs/2026-08-19-https-openssl-transport-design.md).

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};

/// A bidirectional byte transport for one HTTP exchange. `Read` is the
/// response side, `Write` is the request side.
pub trait ByteStream: Read + Write {
    /// Close the write half so the peer sees EOF on our request side while we
    /// keep reading the response. `TcpStream` => `shutdown(Shutdown::Write)`;
    /// PR 2's openssl child => close child stdin. NOTE: not called on the
    /// HTTP path in PR 1 (that would add a FIN-after-request — a wire change);
    /// it is the seam PR 2's transport relies on.
    fn shutdown_write(&mut self) -> io::Result<()>;
}

impl ByteStream for TcpStream {
    fn shutdown_write(&mut self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}
