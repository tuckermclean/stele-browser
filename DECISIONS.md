# DECISIONS

Forks taken while the operator was away. Each: options, choice, why,
revisit-trigger. Newest first.

## P3 — Fetch (Wave 1)

### D10 — Cookie domain convention: leading `.` encodes "subdomains match"
The frozen `Cookie` shape (`domain`, `path`, `name`, `value`, `secure`) has
no separate host-only/subdomain flag, but both `header_for`'s domain-match
and the Netscape jar format's second column need one. **Choice:** when a
`Set-Cookie` carries an explicit `Domain=` attribute, store the domain with
a leading `.` (subdomain matching enabled, RFC 6265 domain-match); a
host-only cookie (no `Domain=`) stores the bare host (exact match only).
`to_netscape`'s `TRUE`/`FALSE` flag column falls straight out of
`domain.starts_with('.')` with no extra field. Revisit-trigger: never,
unless a fixture needs public-suffix-aware domain matching (not attempted
in v0 — no third-party cookies per brief §4 makes this low-risk).

### D9 — Cookie expiry: every cookie is a session cookie in v0
`Expires`/`Max-Age` are parsed off the `Set-Cookie` header (so parsing
doesn't choke on them) and then discarded — the frozen `Cookie` struct has
no field to store an expiry in, and charter C6 only requires a plain-file
jar, not eviction semantics. **Choice:** treat every stored cookie as a
session cookie; `to_netscape`'s expiration column is always `0`.
Revisit-trigger: a fixture or the Lua chair needs persistent (non-session)
cookies across restarts — then add an expiry field (a freeze-packet change,
since `Cookie` is a frozen type) and stop ignoring `Max-Age`/`Expires`.

### D8 — Bespoke HTTP/1.1 over `std::net::TcpStream`, not `httparse`
The brief (§4, §5) names `httparse` as the HTTP layer's crate. P3 needs to
land now, but the crate-vendoring apparatus (needed to bring in *any*
external crate under charter C8's "vendored + attested" rule) is being set
up separately ahead of P4. Options: (a) block P3 on vendoring landing first;
(b) hand-roll HTTP/1.1 parsing, std-only. **Choice: (b).** A bespoke
request formatter + total (never-panics) response parser — status line,
case-insensitive/folded headers, Content-Length and chunked bodies — is a
few hundred lines over `std::net::TcpStream`, unblocks the whole Wave 1
fetch packet immediately, and adds zero dependencies to `Cargo.toml`/
`Cargo.lock` (verified: both unchanged by this packet). Revisit-trigger:
once the vendoring apparatus lands and P4 needs it anyway, consider
swapping this hand-rolled parser for `httparse` if a fixture exposes a
real-world HTTP/1.1 edge case (e.g. more exotic chunk-extension syntax)
that's cheaper to get from a maintained parser than to keep hand-fixing.

### D11 — gzip deferred to a later packet
`Content-Encoding: gzip` (brief §4/§5, via `miniz_oxide`) is out of scope
for P3 for the same vendoring-not-ready-yet reason as D8. **Choice:** the
client advertises `Accept-Encoding: identity` only (never claims to accept
gzip it can't decode), and the fixture server always answers with identity
encoding, so no test in this packet exercises decompression. `Response`'s
`body` doc comment already promises "gzip already inflated" for whenever
that packet lands. Revisit-trigger: `miniz_oxide` is vendored — wire gzip
decoding into `read_response`'s body-decoding step, gated on a
`Content-Encoding: gzip` response header.

## M0 — Toolchain

### D1 — Build substrate: GitHub Actions running the monolith-builder image
Options: (a) run the image locally; (b) a fallback Debian/Ubuntu toolchain;
(c) GitHub Actions job containers. The working environment has no container
runtime and no root (can't install one), so (a) is impossible here and (b)
was explicitly ruled out by the operator (and by charter C11). GitHub-hosted
runners have Docker and run the image directly via `jobs.<id>.container`.
**Choice: (c).** The image is public on ghcr, so no registry secret is
needed. Revisit-trigger: a local/self-hosted runner with the image becomes
available, or CI minutes become a constraint.

### D2 — Toolchain pinned to nightly-2026-07-15
The image ships `nightly-2026-07-15` as the active default (rustc
1.99.0-nightly, da80ed070 2026-07-14) with `rust-src` already present.
**Choice:** pin exactly that in `rust-toolchain.toml` so the build is
reproducible and offline (no rustup download). Revisit-trigger: the image
updates its default nightly, or a needed feature forces a bump — repin then.

### D3 — Hardware x87 float, not soft-float (486SX is a known gap)
The brief's L2 ladder allows hardware float with a documented 486SX gap. The
image's musl cross toolchain (`i486-linux-musl-`) is a standard hardware-float
musl; matching it avoids a soft-float musl mismatch fight. **Choice:** target
spec carries `features: -mmx,-sse,-sse2` (cpu=i486 already implies no SSE/MMX)
and hardware x87 float — confirmed by `--print cfg` reporting `target_feature="x87"`.
Consequence: **486SX (no FPU) is unsupported in v0.1; 486DX/DX2 (with FPU),
including the myth's DX2, are supported.** Revisit-trigger: 486SX support is
ever required — then build a soft-float musl variant.

### D4 — max-atomic-width = 32 (no 64-bit atomics on a 486)
The i486 lacks CMPXCHG8B (introduced on the Pentium), so 64-bit atomics can't
be lock-free. **Choice:** target spec sets `max-atomic-width: 32` (charter C9).
`--print cfg` confirms `target_has_atomic` tops out at 32. 64-bit atomic ops
in std lower to `__atomic_*` libcalls supplied by compiler-builtins.
Revisit-trigger: link errors on `__atomic_*_8` symbols — then ensure the
compiler-builtins atomics/libatomic shim is linked.

### D5 — A4 executes the binary under qemu on the ubuntu host, not in-image
qemu is absent from the monolith-builder image (confirmed by the substrate
run), and installing it on gentoo means a slow `emerge`. The i486 binary is
fully static (crt-static), so it needs nothing from the build image at
runtime. **Choice:** two-job CI — `build` in the image, `accept` on the plain
ubuntu host where `qemu-user` is one `apt-get` away. Revisit-trigger: a reason
to execute inside the image (e.g. testing image-provided runtime bits) — then
add a static qemu-i386 to the image or the job.

### D7 — libunwind shim: alias libgcc_eh.a as libunwind.a
std's musl `unwind` crate links `-lunwind` (LLVM's libunwind), but the image's
cross toolchain is GCC-based and ships `libgcc_eh.a` (same `_Unwind_*` API)
instead. With self-contained linking off there is no bundled libunwind to fall
back on. Options: (a) build LLVM libunwind in-tree (needs llvm sources absent
from rust-src); (b) provide libunwind. **Choice: (b)** — symlink the cross
gcc's `libgcc_eh.a` (found via `-print-file-name`) as `libunwind.a` on the link
search path. Since `panic=abort` never unwinds, the `_Unwind_*` symbols resolve
but are never called. Revisit-trigger: unwinding is ever enabled, or the image
starts shipping a real libunwind — then link that instead.

### D6 — cargo-auditable / cargo-audit deferred to M6
Both are absent from the image (substrate run). They serve A6/C11 (attested
provenance, audit-clean deps), which the brief scopes to M6, not M0.
**Choice:** defer; M0 gates only on A1 + A4. Revisit-trigger: M6 hardening —
install/vendor them then, or bake them into the image.
