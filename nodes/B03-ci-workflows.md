# B03 — ci-workflows

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | B01, B02 | none | S |

**Owns:** `.github/`, `tools/render-gallery.sh`

## Purpose

Imported from the code. CI is where Stele is built for the 486 and accepted (AGENTS.md non-negotiable 3).
It relies on B01 for the build and on B02 for the acceptance checks.

- `.github/workflows/build-substrate.yml` (on push to `ci/**` and `main`, every pull request, and manual
  dispatch) runs in the `monolith-builder` image pinned by digest. It reports the image, the Rust toolchain,
  the cross C toolchain, qemu and the audit tools, and fails only when `rustc`, `cargo` or the cross `gcc`
  is missing. It uses only `run:` steps (no checkout, no JavaScript actions).
- `.github/workflows/m0-acceptance.yml` (on push to `packet/**` and `main`, every pull request, and manual
  dispatch) has three jobs:
  - `build`, in the same pinned image: toolchain identity; `./accept.sh --tty-only` (A3 and the other host
    checks); upload of `target/release/stele` as `stele-host` (always, so a new PNG golden can be blessed
    from CI's binary); the render gallery and its `renders` artifact (always, best-effort);
    `cargo test --workspace --locked` (always); the i486 release build (an unwind shim that links
    `libgcc_eh.a` as `libunwind.a`, `-Cpanic=immediate-abort` in `RUSTFLAGS`, `--target
    targets/i486-monolith-linux-musl.json -Zbuild-std=std,panic_abort -Zjson-target-spec`); and the upload
    of `stele-i486`.
  - `accept`, after `build`, on plain `ubuntu-latest`: installs `qemu-user` and `file`, downloads
    `stele-i486` and runs `./accept.sh`: A1, A2, A4, A5z, A6a, the A6 covenant and the pending checks. The
    workflow's comments say the job has no Rust toolchain, so A3 would be pending there, but the runner
    image carries cargo, and the job also rebuilds the host binary and runs the host block (A5u aside).
  - `x11-smoke`, non-gating (`continue-on-error`): builds a host release binary, starts Xvfb, runs
    `stele --x11` on `fixtures/basic.html` for 20 seconds under a synthetic wheel storm from `xdotool`, and
    reports whether it stayed up.
- `tools/render-gallery.sh <binary> <output-dir>` renders every `fixtures/*.html` with `--headless
  --dump-png` and `--dump-text`, plus `httpforever.html` under `--color-scheme light` and `dark`, each run
  under a 30-second timeout. It writes a self-contained `index.html` that names the commit. A fixture that
  fails is logged to stderr and shown as "(render failed)". The script exits 1 only when its arguments, the
  binary, `fixtures/` or the output directory are missing.

## Tests

None. The workflows run on every pull request and push to `main`. Their results are the CI evidence that
AGENTS.md's "CI evidence" rule asks a node PR to record. No test runs `tools/render-gallery.sh`. Its output
shape (an `<img>` without `width`/`height`) is the case `tests/local_img_loading.rs` covers in the renderer.

## Changes

none: imported

## Gate

- **B03-AC-1** — `build-substrate` runs in the digest-pinned monolith-builder image and fails only when `rustc`, `cargo` or the cross `gcc` is missing. *Verify:* unpinned.
- **B03-AC-2** — `m0-acceptance` runs on every pull request, on pushes to `packet/**` and `main`, and on manual dispatch. *Verify:* unpinned.
- **B03-AC-3** — The `build` job runs `./accept.sh --tty-only` and then `cargo test --workspace --locked` in the pinned image. The unit-test step runs even when the accept step failed, and either failure fails the job. *Verify:* unpinned.
- **B03-AC-4** — The `build` job uploads the host binary `stele-host` and the `renders` gallery even when an earlier step failed. *Verify:* unpinned.
- **B03-AC-5** — The `build` job builds the i486 release binary with the canonical pipeline (unwind shim, `-Cpanic=immediate-abort`, the custom target, `-Zbuild-std=std,panic_abort`, `-Zjson-target-spec`) and uploads it as `stele-i486`. *Verify:* unpinned.
- **B03-AC-6** — The `accept` job runs `./accept.sh` on `ubuntu-latest` against the downloaded `stele-i486`, with `qemu-user` and `file` installed; it runs only when the `build` job passed. *Verify:* unpinned.
- **B03-AC-7** — `x11-smoke` never fails the workflow. *Verify:* unpinned.
- **B03-AC-8** — `tools/render-gallery.sh` renders every fixture to PNG and text, plus httpforever light and dark, writes `index.html`, and exits 0 when its preconditions hold, whatever the individual renders do. *Verify:* unpinned.

## Risks

- Every criterion is `unpinned`: CI configuration has no test here. A node that changes a workflow shows
  the change working in the CI run of its own PR.
- The image digest is written twice, in `build-substrate.yml` and in `m0-acceptance.yml`. A re-pin has to
  change both, and the toolchain pin (B01) with them. Issue #120 plans one pin file that the workflows read.
- `ubuntu-latest` moves. Its move to Ubuntu 26.04 broke A1 (issue #117). The `accept` job's result depends on
  the runner's `file` and `qemu-user` versions.
- The i486 build and its upload have no `if: always()`. When the accept step or the unit tests fail, no
  `stele-i486` is built and the `accept` job does not run, so A1, A2 and A4 report nothing for that commit.
- The `accept` job runs the host block a second time on the runner's own cargo (see Purpose), outside the
  pinned image, contrary to what the workflow's comments say.
- `m0-acceptance` does not run on a push to a `node/**` or `spec/**` branch; a node's CI comes from the
  `pull_request` trigger.
- Changing files under `.github/` needs a token with the `workflow` scope (DECISIONS D75 discusses this).
