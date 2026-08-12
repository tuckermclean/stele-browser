# DECISIONS

Forks taken while the operator was away. Each: options, choice, why,
revisit-trigger. Newest first.

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

### D6 — cargo-auditable / cargo-audit deferred to M6
Both are absent from the image (substrate run). They serve A6/C11 (attested
provenance, audit-clean deps), which the brief scopes to M6, not M0.
**Choice:** defer; M0 gates only on A1 + A4. Revisit-trigger: M6 hardening —
install/vendor them then, or bake them into the image.
