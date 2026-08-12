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
- Decisions recorded: D1–D6 (see DECISIONS.md).
- NEXT: land the M0 packet PR, watch m0-acceptance go green (A1+A4), then the
  interface-freeze packet (dom::ast, ComputedStyle skeleton, Surface trait,
  fetch::Response, layout node interface) before Wave 1.
