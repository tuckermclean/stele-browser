# JOURNAL

Append-only running log. Newest at the bottom.

## 2026-08-12 — Founding + M0 toolchain

- Repo founded: `tuckermclean/stele-browser` (public). Charter + build brief
  landed via PR #1; base `.gitignore` (tracks `vendor/`, ignores `target/`).
- CI substrate proven: PR #2 boots the monolith-builder image
  (`ghcr.io/tuckermclean/monolith-builder`, pinned by digest
  `sha256:c8978fe3…`, tag `20260811-b25ecd2b24a5`) on a GitHub-hosted runner
  and asserts the toolchain. Findings from the run:
    - rustc 1.99.0-nightly (da80ed070 2026-07-14), toolchain
      `nightly-2026-07-15` active/default; `rust-src` PRESENT.
    - cross `i486-linux-musl-gcc` PRESENT.
    - `qemu-i386` NOT found in image; `cargo-auditable`/`cargo-audit` absent.
- M0 packet (`packet/m0-toolchain`) authored:
    - `targets/i486-monolith-linux-musl.json` derived from the real
      `i586-unknown-linux-musl` spec (dumped via nightly): cpu=i486,
      max-atomic-width=32, relocation static, PIE off, panic=abort,
      hardware x87 float. Validated locally with
      `rustc -Zunstable-options --print cfg` (panic=abort, relocation_model=
      static, target_env=musl, crt-static, x87, atomics ≤ 32 — all as intended).
    - `rust-toolchain.toml` pins `nightly-2026-07-15` + `rust-src`.
    - `.cargo/config.toml` maps the i486 target's linker to
      `i486-linux-musl-gcc` (no global default target — host tests stay native).
    - `Cargo.toml`: `stele` bin; release profile opt-level=z, lto=fat,
      codegen-units=1, panic=abort, strip.
    - `src/main.rs`: M0 hello (no script engine, ever — C3).
    - `accept.sh`: A1 (static i386 ELF) + A4 (qemu-i386 -cpu 486 vs golden)
      live; A2 informational; A3/A5/A6/A7 PENDING against their milestones.
    - `goldens/m0-hello.txt`: the A4 golden.
    - `.github/workflows/m0-acceptance.yml`: build-in-image → accept-on-host.
- Decisions recorded: D1–D7 (see DECISIONS.md).
- **M0 ACCEPTANCE GREEN** (PR #3, run 31625819900): build-in-image → accept-on-host.
    - A1 PASS: `ELF 32-bit LSB executable, Intel 80386, statically linked, stripped`.
    - A4 PASS: `qemu-i386 -cpu 486` output matches golden — the target binary
      executes 486-legal code.
    - A2: 301,372 bytes (0.29 MB) vs 2.0 MB budget — huge headroom.
  Four real toolchain fixes were needed to get the i486 build to link, each
  caught by CI and fixed in turn:
    1. this nightly gates `.json` targets behind `-Zjson-target-spec`.
    2. self-contained musl crt/libc doesn't exist for a custom build-std target
       → drop `crt-objects-fallback`, let the cross gcc supply crt (D1-adjacent).
    3. rustc requires the self-contained object lists empty when self-contained
       is off → remove `pre/post-link-objects-fallback`.
    4. GCC musl toolchain ships `libgcc_eh.a`, not LLVM `libunwind` → alias it
       as `libunwind.a`; panic=abort never unwinds (D7).
- NEXT (after PR #3 merges): the INTERFACE FREEZE packet (orchestrator-authored,
  serial) — dom::ast (closed sum type, no script variant), style::ComputedStyle
  skeleton, Surface trait, fetch::Response, layout node interface — then Wave 1
  fans out (P1 parser · P2 CSS · P3 fetch · P4 image decoders · P5 text/metrics).
