# B02 — acceptance-script

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | B01 | B03 | M |

**Owns:** `accept.sh`, `ci/`

## Purpose

Imported from the code. `accept.sh` is Stele's definition of done (build brief §0): exit 0 means
acceptance. It prints each check as `PASS`, `FAIL`, `WARN` or `····` (pending), coloured with ANSI codes, and
ends with `ACCEPT: ...` or `REJECT: ...`. A `FAIL` sets the exit status to 1, and the script runs every check
instead of stopping at the first failure. It relies on B01 for the build: the cargo manifest, the toolchain
pin and the i486 target.

Modes:
- `./accept.sh` runs everything below.
- `./accept.sh --tty-only` runs only the cargo-dependent block (the host checks). CI's `build` job runs it
  under the pinned toolchain, and it is the first half of the graph's gate.
- `--bless` rewrites the goldens from the current binary instead of comparing (A4, and every golden check in
  the host block). It composes with `--tty-only`.

Checks on the prebuilt i486 binary `target/i486-monolith-linux-musl/release/stele`, skipped under `--tty-only`:
- **A1**: `file -b` must say `ELF 32-bit`, `Intel 80386` and `statically linked`.
- **A4**: under `qemu-i386 -cpu 486` (or `qemu-i386-static`), the binary's output must equal
  `goldens/m0-hello.txt`.
- **A2**: the size in bytes, as a percentage of the 1,474,560-byte floppy, with the headroom. A `WARN` line
  `A2 WARN: ...` over the floppy (still a pass), a `FAIL` over 2,000,000 bytes.
- **A5z** (instructions): with a QEMU TCG instruction-count plugin, the retired instructions of
  `--headless --dump-text fixtures/kitchen-sink.html` must be at most `SPEED_INSN_BUDGET` (50,000,000).
  Without a plugin it is pending.
- **A6a**: `strings` of the binary must show no TLS or crypto implementation symbols (rustls, ring's
  crypto modules, boringssl, libcrypto, `SSL_CTX_new`, `EVP_`, `X509_verify`, embedded_tls).

The host block. Without `cargo` on `PATH` it prints `A3` as pending. With cargo it always rebuilds
`target/release/stele` with a plain `cargo build --release` (no `+toolchain`, so the pin governs), then:
- byte-compares `--headless --dump-text` and `--dump-png` renders of fixtures with their goldens:
  A3, A3b–A3p and A3r–A3z, A5a–A5s, A5v, A5w (Acid2 scrolled to `#top`) and A5x
  (`about:attestations`), and, under `--tty-only` only, A5u (`--dump-png --chrome` of `fixtures/basic.html`
  against `goldens/chrome-basic.png`). A PNG check without a golden fails and says to bless;
- A3q: `--audit-contrast` must report no violation on the fixture corpus, and on
  `fixtures/httpforever.html` under `--color-scheme dark`;
- A5t: `fixtures/acid2.html` must render to a non-empty PNG (a smoke check);
- A5z (wall-clock): the host first paint of `fixtures/kitchen-sink.html` must take at most
  `SPEED_WALLCLOCK_BUDGET_MS` (150) milliseconds, measured with `date +%s%N`.

Under `--tty-only` the script ends here with `ACCEPT (--tty-only)` or `REJECT (--tty-only)`. Otherwise it goes
on:
- **A6 covenant**: `grep -i script src/dom/ast.rs` must find nothing (charter C3).
- **A6 audit tooling**: runs `ci/ensure-audit-tools.sh` and reports its result as pending in every case.
- **A7**: pending (M6).

`ci/ensure-audit-tools.sh` makes `cargo-auditable` 0.7.6 and `cargo-audit` 0.22.2 available for the A6
audit without a rebuilt image. With both on `PATH` it exits 0. Otherwise it installs them only when
`STELE_BOOTSTRAP_AUDIT_TOOLS=1` (`cargo install --version <v> --locked`, adding `CARGO_HOME/bin` to `PATH`).
It exits 3 when the tools are absent and were not installed: the bootstrap was not requested, there is no
cargo, or the install failed. It exits 1 on anything unexpected.

## Tests

None test the script itself. `accept.sh` is the harness: the goldens it compares are in `goldens/`, the
fixtures in `fixtures/`, and the Rust golden tests under `tests/` cover the same renders through the
library. CI runs `./accept.sh --tty-only` in the `build` job and `./accept.sh` in the `accept` job (B03).
No test runs `ci/ensure-audit-tools.sh`.

## Changes

none: imported

## Gate

- **B02-AC-1** — A1 passes only when `file` reports a 32-bit, `Intel 80386`, statically linked ELF. *Verify:* unpinned (CI's `accept` job; listed in `gate.known_red` while issue #117 is open).
- **B02-AC-2** — A4 runs the i486 binary under `qemu-i386 -cpu 486` and compares its output with `goldens/m0-hello.txt`. *Verify:* unpinned (CI's `accept` job).
- **B02-AC-3** — A2 prints the size, percent of floppy and headroom, warns over 1,474,560 bytes and fails over 2,000,000. *Verify:* unpinned (CI's `accept` job).
- **B02-AC-4** — A5z counts retired instructions under a QEMU plugin when there is one (budget 50,000,000) and is pending without one. The host wall-clock fallback in the cargo block fails above 150 ms. *Verify:* unpinned.
- **B02-AC-5** — A6a fails when TLS or crypto implementation symbols appear in the i486 binary. *Verify:* unpinned.
- **B02-AC-6** — With cargo present, the host block rebuilds `target/release/stele` and byte-compares each listed dump with its golden (A5u only under `--tty-only`). A difference, a crash or a missing PNG golden prints a `FAIL A<n>...` line and makes the exit status 1. *Verify:* unpinned (the script is the harness; nothing tests it).
- **B02-AC-7** — Without cargo on `PATH`, the host block is pending, not failed. *Verify:* unpinned.
- **B02-AC-8** — `--tty-only` runs only the host block and ends with `ACCEPT (--tty-only)` (exit 0) or `REJECT (--tty-only)` (exit 1). *Verify:* unpinned.
- **B02-AC-9** — `--bless` writes the current renders to the goldens instead of comparing; A3q has nothing to bless and is pending. *Verify:* unpinned.
- **B02-AC-10** — The A6 covenant fails when `src/dom/ast.rs` contains `script` in any case. *Verify:* unpinned.
- **B02-AC-11** — The A6 audit-tooling line and A7 are pending whatever `ci/ensure-audit-tools.sh` returns, so they never fail the run. *Verify:* unpinned.
- **B02-AC-12** — `ci/ensure-audit-tools.sh` exits 0 with both tools on `PATH`. It exits 3 when they are absent and not installed (bootstrap not requested, no cargo, or a failed install). It installs the pinned versions only under `STELE_BOOTSTRAP_AUDIT_TOOLS=1`. *Verify:* unpinned.

## Risks

- Every criterion is `unpinned`: the script is the test harness, and no test checks the harness. A change
  to `accept.sh` that stops a comparison from failing (a `diff` that always succeeds, say) passes the gate
  unnoticed. A node that changes it should show each changed check failing on a broken input.
- A1 greps `Intel 80386`. The `file` on Ubuntu 26.04 runners prints `Intel i386`, so A1 fails in CI's
  `accept` job (issue #117, SPEC.md D5). The line is in `gate.known_red`. While it is listed, A1 checks nothing.
- A5u's golden holds CI's container path in the address bar, so A5u fails in any other checkout (SPEC.md
  D4; in `gate.known_red`).
- The script's comments say the plain `accept` CI job has no cargo, so the host block is pending there. The
  `ubuntu-latest` runner does carry cargo: on `main`'s run of 2026-11-26 the `accept` job rebuilt the host
  binary and ran the whole host block again (A5u aside). Whether that build used the pinned nightly is not
  shown in the log.
- The A5z wall-clock budget depends on the runner's speed. `SPEED_WALLCLOCK_BUDGET_MS` and
  `SPEED_INSN_BUDGET` can be overridden from the environment.
- The script writes scratch files under `/tmp` (`/tmp/stele_*`) with fixed names, so two runs on one
  machine at once can interfere.
- `ci/ensure-audit-tools.sh` falls back to `cargo install cargo-audit --no-default-features --features
  fix,vendored-openssl` when the default install fails, but cargo-audit 0.22.2 has no `vendored-openssl`
  feature, so the fallback cannot succeed (issue #122). The script and DECISIONS D74/D75 cite
  `tools/monolith-builder-audit.Dockerfile` and D73, which are not in the repository (issue #120).
- The A6 audit itself (vendored crates, a cargo-auditable binary, `cargo audit`) is not built yet (issue #118).
