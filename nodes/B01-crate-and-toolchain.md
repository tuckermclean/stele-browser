# B01 — crate-and-toolchain

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | B02, B03, F05 | S |

**Owns:** `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/`, `targets/`, `.gitignore`

## Purpose

Imported from the code. These files define how the one crate builds, for the host and for the i486 floor.

- `Cargo.toml`: the package `stele` (edition 2021, GPL-3.0-or-later, `publish = false`) with a library
  `stele` (`src/lib.rs`) and a binary `stele` (`src/main.rs`). Five dependencies: `gif` 0.13,
  `jpeg-decoder` 0.3 without default features (no rayon thread pool), `png` 0.17, `rustix` 1.1.4 with only
  `termios`, `std` and `event`, and `taffy` 0.13 trimmed to `std`, `taffy_tree`, `flexbox`, `block_layout`,
  `float_layout`, `content_size` and `grid`. The comments record why each feature is on and its size price.
  `[profile.release]` is size-first: `opt-level = "z"`, fat LTO, one codegen unit, `panic = "abort"`
  (charter C4), `strip = true`.
- `Cargo.lock`: the resolved dependency graph (lockfile version 4). CI builds and tests with `--locked`.
- `rust-toolchain.toml`: the one pinned rustc (charter C9), `nightly-2026-07-15`, profile `minimal`, with
  `rust-src` for `-Zbuild-std`.
- `.cargo/config.toml`: the linker `i486-linux-musl-gcc` for the target `i486-monolith-linux-musl`, and
  deliberately no `[build] target`, so host tests build for the host. `[env]` sets
  `RUST_MIN_STACK = 16777216`, a 16 MiB stack for libtest's test threads, so the deep-nesting probes do not
  overflow in a debug build.
- `targets/i486-monolith-linux-musl.json`: the custom i486 target. 32-bit x86, `cpu: i486`, MMX and SSE
  off (hardware x87 float), musl, static relocation and no PIE, static CRT by default, `panic-strategy: abort`,
  inline stack probes, frame pointers always.
- `.gitignore`: ignores `/target/`, `/target-i486/`, `**/*.rs.bk`, editor files, `/.env` and
  `/.cargo/registry/`. It leaves `vendor/` tracked on purpose (charter C8/C11).

The i486 release build itself (`cargo build --release --target targets/i486-monolith-linux-musl.json
-Zbuild-std=std,panic_abort -Zjson-target-spec` with `-Cpanic=immediate-abort` in `RUSTFLAGS`) is run by CI
(B03) and documented in `accept.sh`'s header (B02).

## Tests

No test reads these files directly. The integration tests exercise them by building: `tests/stats_cli.rs`
runs the binary through `CARGO_BIN_EXE_stele`, the integration tests import the library as `stele`,
`tests/layout_block_floats.rs` and `tests/grid_golden.rs` use the taffy features, and the inline test
`layout::box_tree::tests::deeply_nested_dom_does_not_abort_and_returns` (`src/layout/box_tree.rs`) needs
the test-thread stack. The i486 target, the toolchain pin and the release profile are exercised only in CI
(the `build` job's i486 build, and A1, A2 and A4 in the `accept` job).

## Changes

none: imported

## Gate

- **B01-AC-1** — The binary target is named `stele`, so Cargo provides `CARGO_BIN_EXE_stele` to the integration tests. *Verify:* `cargo test --locked --jobs 2 --test stats_cli -- --test-threads=2` exits 0.
- **B01-AC-2** — The library target is named `stele`, so the integration tests import it as `stele`. *Verify:* `cargo test --locked --jobs 2 --test layout_block_floats -- --test-threads=2` exits 0.
- **B01-AC-3** — taffy is built with its `float_layout` feature, which block layout's `float`/`clear` mapping needs. *Verify:* `cargo test --locked --jobs 2 --test layout_block_floats -- --test-threads=2` exits 0.
- **B01-AC-4** — taffy is built with its `grid` feature, which the CSS Grid layout path needs. *Verify:* `cargo test --locked --jobs 2 --test grid_golden -- --test-threads=2` exits 0.
- **B01-AC-5** — `Cargo.lock` agrees with `Cargo.toml`: a `--locked` build does not need to change the lockfile. *Verify:* `cargo test --locked --jobs 2 --test stats_cli -- --test-threads=2` exits 0.
- **B01-AC-6** — Test threads get a 16 MiB stack (`RUST_MIN_STACK` in `.cargo/config.toml`), so the depth-capped recursion probe does not overflow in a debug build. *Verify:* `cargo test --locked --jobs 2 --lib layout::box_tree::tests::deeply_nested_dom_does_not_abort_and_returns -- --test-threads=2` exits 0.
- **B01-AC-7** — The dependency set is `gif`, `jpeg-decoder` (no default features), `png`, `rustix` (`termios`, `std`, `event`) and `taffy` with only the seven features listed above. *Verify:* unpinned.
- **B01-AC-8** — The release profile is `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`. *Verify:* unpinned (its effect shows only in CI's A2 size line).
- **B01-AC-9** — `rust-toolchain.toml` pins `nightly-2026-07-15` with `rust-src`, profile `minimal`. *Verify:* unpinned.
- **B01-AC-10** — `.cargo/config.toml` sets the linker `i486-linux-musl-gcc` for `i486-monolith-linux-musl` and sets no `[build] target`. *Verify:* unpinned.
- **B01-AC-11** — The target JSON describes a 32-bit i486 musl target without MMX/SSE, statically relocated, CRT-static by default, with `panic-strategy: abort`. *Verify:* unpinned (CI's A1 and A4 check the binary built from it).
- **B01-AC-12** — `.gitignore` ignores the build directories `/target/` and `/target-i486/` and does not ignore `vendor/`. *Verify:* unpinned.

## Risks

- B01-AC-4 and B01-AC-5 are partly checked by `--locked`. Dropping `grid` also removes a package from the
  lockfile, so a `--locked` run fails on the lockfile before it compiles. A run without `--locked` fails on
  the missing grid types.
- `RUST_MIN_STACK` sizes only libtest's threads. The comment calls the default stack "marginal": a larger
  stack frame in `build_node_inner` could need a larger value. Release builds are not affected.
- The toolchain pin is shared with the monolith-builder image's default nightly. A re-pin of the image
  (issue #120) and this file have to move together. No test catches a drift; CI's i486 build does.
- No local test exercises the target JSON. A change to it is judged only by CI's i486 build, A1, A2 and A4.
  Its `metadata.description` says the 486SX (no FPU) is a documented gap.
- Every dependency or feature added here is priced against the 1.44 MB floppy (SPEC.md principle 2), and
  changes the generated attestation data (`src/fetch/attestations_data.rs`, from
  `tools/gen-attestations.py`). A Protolith dependency (SPEC.md D1) would land here. The attestation
  ceremony (issue #118) adds a `[source]` replacement to `.cargo/config.toml` and a committed `vendor/`.
