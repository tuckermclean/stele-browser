# Working in Stele — AGENTS.md

How to be productive in this repository, for AI agents and humans doing agent-style work.
Short on purpose. It stands on its own, but points to the living sources of truth for
anything that changes over time.

Stele is a **finished document-web browser for the 486**: one static i486 musl binary,
no JavaScript by construction, must fit on a 1.44 MB floppy. If you haven't, skim
[`README.md`](README.md) for *what* it is — this file is *how you work on it*.

---

## The non-negotiables

Load-bearing. Violating one silently is worse than not touching the code at all.

1. **No JavaScript — by construction, not by policy.** The document AST (`src/dom/`) is
   a closed sum type with **no variant for executable anything** (charter C3). Don't add
   one. The heresy is *uninvited* computation; the wire is never invited.

2. **The binary must fit a 1.44 MB floppy = 1,474,560 bytes.** This is tighter than the
   CI A2 gate (2,000,000 bytes) and it is the *real* ceiling. Price every dependency,
   taffy feature, and decoder against it, and **report the size delta of anything you
   add** — measure it (the `stele-i486` CI artifact / the A2 line), don't guess.

3. **CI compiles; you usually don't.** The i486 cross-build under `-Zbuild-std` inside
   the `monolith-builder` image is heavy, and agent dev environments are typically
   resource-limited. **Default workflow: push a branch, let the `m0-acceptance` workflow
   build + test + run acceptance, and read the result.** Don't run `cargo build` /
   `cargo test` locally unless you know your environment can take it. This is how the
   project is set up to be worked, not a style preference.

4. **Goldens are byte-compared. Pixel-verify before you bless; never rubber-stamp;
   never bless away a regression.** A golden (`goldens/*.png`, `goldens/*.txt`) is a
   reference render CI diffs byte-for-byte. When a render changes:
   - **Measure the new render programmatically** (e.g. PIL / connected-component
     analysis for PNGs) and confirm it is *correct*, not merely *different*. accept.sh
     says it outright: *never bless a render you haven't verified* (brief §10).
   - If a golden changed for a reason you can't explain and justify as **more correct**,
     that's a **regression** — fix the root cause. Do **not** re-bless to turn CI green,
     and do **not** disable the check.
   - This discipline has caught real bugs here (phantom table columns, muddy sub-native
     text, auto-width overflow). It is why they didn't ship.

5. **Test-first, root-cause-first.** Every behavioral change lands with a failing test
   first (visible red→green in history). Every bug is root-caused before a fix — no
   symptom-patching, no magic offsets. If three fixes in a row don't hold, question the
   design, don't try a fourth.

6. **The dialect is amended, never drifted.** If your packet makes Stele speak something
   new — a CSS property, an element, a behavior — the **same PR** updates the charter's
   C2 adopted-amendments record (with the packet/PR reference and the size price). The
   charter is the source of truth on "What Stele Speaks"; code that outruns it turns the
   constitution into a fossil. (This rule exists because it happened: grid, floats, and
   custom properties shipped ahead of the charter and had to be back-recorded.)

---

## The build & test loop (CI-driven)

- **Canonical i486 build command:** documented and kept current at the **top of
  [`accept.sh`](accept.sh)** — read it there. Toolchain pin: [`rust-toolchain.toml`](rust-toolchain.toml).
  Builder image (pinned by digest) and CI jobs: `.github/workflows/`.
- **`./accept.sh` = definition of done** (exit 0). Checks: A1 static ELF · A2 size · A3
  tty goldens · A4 the i486 binary under `qemu-i386 -cpu 486` · A5 PNG goldens.
  `--tty-only` = just the host golden check; `--bless` = regenerate goldens.

**Typical loop:**

1. Write a failing test / add a fixture under `fixtures/`.
2. Implement.
3. Push the branch. `m0-acceptance` builds the `stele-host` and `stele-i486` binaries,
   runs `cargo test`, runs `accept.sh`, and uploads artifacts: **the binaries** and a
   **`renders/` gallery** (one PNG + txt per fixture).
4. **Bless goldens without building locally:** download the CI `stele-host` (or
   `renders`) artifact, **verify the render is correct** (measure it), copy the correct
   file into `goldens/`, commit, push. CI re-renders and byte-compares deterministically.
5. Green CI **and** verified goldens → open one PR.

---

## How work is structured: packets

**One packet = one branch = one PR.**

- Branch `packet/<short-name>`, off `main`.
- Test-first, with visible red→green commits. Conventional subjects:
  `feat(scope):` · `fix(scope):` · `test(scope):` · `chore(...):` · `size:` — often
  tagged with the milestone/decision (`T6:`, `D6`, `(#NN)`).
- Independently reviewed, CI-green, **merged one PR at a time** (squash is fine for
  noisy/experimental histories).
- **Record non-obvious forks in [`DECISIONS.md`](DECISIONS.md)** (options / choice / why
  / revisit-trigger, newest first). **Append a note to [`JOURNAL.md`](JOURNAL.md)**
  (newest at the bottom) when you finish a chunk.

Larger efforts use **subagents**: an orchestrator plans, dispatches, reviews, and merges;
implementers do the test-first work in isolated git worktrees. If you orchestrate: you
**verify** (pixel-check goldens, read the actual diff, confirm the size delta) and never
accept a subagent's "CI is green" as proof the work is *right*. If shared low-level infra
is involved, pre-assign it to ONE packet so parallel branches don't collide on merge.

---

## The workspace is shared — branch per issue, push, and check your diff

Agents here share one checkout of this repo. Runs land in it one after another, it is
periodically `reset --hard` back to `main`, and Paperclip's git sync can merge a run's
local commit into **whatever branch the checkout happens to be sitting on**. Two failure
modes follow from that, and both have already happened:

- **Work disappears.** A commit that exists only locally is not saved work — the next
  reset takes it, and a reviewer who only has `git fetch` sees nothing.
- **Work gets bundled.** A branch left over from an earlier, unrelated run accumulates
  the next run's commits. `dcx-88-shared-workspace-push-rule` shipped a docs change *and*
  an in-progress `view-source:` feature (DCX-72) in the same branch, because the feature
  was committed onto the branch that happened to be checked out. Whoever lands "the
  branch" then ships unreviewed, possibly blocked, work under an unrelated ticket.

So, for **every** change — code, docs, goldens, anything:

- **Start from `origin/main`, on a branch named for your issue.** `git fetch origin` then
  `git switch -c dcx-<n>-<slug> origin/main`. Never commit onto the branch you inherited
  from the previous run, and never assume the checked-out branch is yours.
- **Check what you are about to hand off before you hand it off.**
  `git diff --stat origin/main...HEAD` must list *only* the files your issue covers. If it
  lists anything else, you are on a contaminated branch — re-cut from `origin/main` and
  replay just your change. Say the file count and SHA in your handoff so a reviewer can
  check the same thing.
- **Push the branch to `origin` in the same heartbeat that creates the commit.** Pushing
  is part of finishing, not a release-time step. Name the pushed ref (branch **and**
  commit SHA) in your handoff — that is what makes the work inspectable.
- **Reviewers: verify the ref is fetchable from `origin`, and diff it against
  `origin/main` yourself** before reviewing content. If the ref is missing, or the diff
  is wider than the ticket, return it to its owner instead of reviewing what's there.
- **Release: never land "the branch as materialized in the shared workspace."** Land a
  pushed ref whose `origin/main...HEAD` diff matches the reviewed scope. A diff that grew
  between review and release is a stop, not a merge.
- If you need isolation from other runs in the same checkout, use a git worktree
  (`git worktree add --detach <dir> origin/main`) — the branch and push rules still apply
  to what comes out of it.
- **Never put a credential anywhere durable.** Not in `.git/config`, not in a remote URL,
  not in a commit, comment, or log line. The harness supplies git credentials to the run;
  reference them through the environment, never by value.

---

## Where the truth lives (link it, don't duplicate it)

| Question | Source of truth |
|---|---|
| What may Stele render? What are the principles? | [`stele-charter.md`](stele-charter.md) — C1–C11, "What Stele Speaks" |
| Process, milestones, definition of done | [`stele-build-brief.md`](stele-build-brief.md) |
| Why is this code shaped like this? | [`DECISIONS.md`](DECISIONS.md) + the heavy doc-comments in the source |
| What happened, in order? | [`JOURNAL.md`](JOURNAL.md) |
| Exact build flags / toolchain / size gate | [`accept.sh`](accept.sh) header · [`rust-toolchain.toml`](rust-toolchain.toml) · [`Cargo.toml`](Cargo.toml) |
| Current state / release summary | [`REPORT.md`](REPORT.md) |

Doc-comments in `src/` are unusually heavy **on purpose** — a function's comment often
explains the CSS rule, the taffy interaction, *and* the packet that touched it. Read
them; keep them true when you change the code beneath them. If your agent harness has
persistent memory, record the non-obvious project facts you learn (size headroom, build
gotchas, review lessons) there too.

---

## Keeping this document alive (the anti-fossil clause)

This file rots the moment it lies. Rules to keep it honest:

- **Link volatile facts; don't inline them.** Toolchain version, exact build flags, live
  size numbers, dialect scope → point to their source (`rust-toolchain.toml`,
  `accept.sh`, `Cargo.toml`, the charter). If you find a hardcoded number here that's now
  wrong, replace it with a pointer to where it's measured.
- **Update docs in the same PR as the change.** New workflow step, new gate, new
  non-negotiable, renamed directory → edit README/AGENTS in that PR. A doc PR that trails
  the code is how fossils form.
- **Date-stamp any "current state" claim** that's a snapshot, and say what to verify it
  against.
- **If you learned something the hard way to be productive here, it belongs in this
  file.** AGENTS.md is the distilled answer to "what do I wish I'd known before touching
  this repo." Grow it deliberately; prune it when it's wrong.

---

## Building a node

The graph regime: `SPEC.md`, `docs/graph.yaml` and the node specs in `nodes/` plan the work; a node is
built test-first by role agents. The non-negotiables above hold for every node (rule 6 as `SPEC.md`
principle 6 words it: the amendment lands in the spec PR that plans the node). Where this section and
the packet sections above differ, this section governs node work; the packet sections govern work
outside the graph.

- **Building.** Build a node with `/noodlebot:node <ID>`, on a branch `node/<ID>-<name>` off
  `origin/main`. A spec change goes on `spec/<topic>`.
- **Authority order.** `SPEC.md`, then `docs/graph.yaml`, then the node spec `nodes/<ID>-<name>.md`,
  then this file. When two disagree, the earlier one wins and the later one is fixed by a spec PR. The
  charter (`stele-charter.md`) stays the constitution that `SPEC.md` points to.
- **Roles.** The plugin's tester writes the failing tests, the implementer makes them pass, the
  reviewer judges the result. This repository has no role agents of its own. The role guard reads
  `.noodlebot/roles.yaml`: it adds `goldens/**` and `src/**/*_tests.rs` to the default test paths, and
  this section's spec files to the spec paths.
- **Spec checkers.** `python3 -I <plugin dir>/skills/node/check-regime.py` and
  `python3 -I <plugin dir>/skills/node/check-coverage.py`, from the repository root, must print
  `PROBLEMS 0` before a spec PR is opened.
- **Gate command.** The whole suite, as CI's `build` job runs it, and it runs in CI on the PR
  (`.github/workflows/m0-acceptance.yml`):
  `./accept.sh --tty-only; a=$?; cargo test --workspace --locked; t=$?; [ $a -eq 0 ] && [ $t -eq 0 ]`.
  Its failure lines are `FAIL A<n>...` (accept.sh) and `test <path> ... FAILED` (libtest), which the
  graph's `gate.fail_pattern` matches. A summary line such as `test result: FAILED. ...` names no test
  and is not matched.
- **CI's output.** accept.sh colours its `PASS`/`FAIL`/`WARN` words with ANSI escape codes, and every
  line of a fetched GitHub Actions job log starts with a timestamp. `fail_pattern` matches a line once
  the colour codes (`\x1b\[[0-9;]*m`) and the timestamp prefix (`^\S+Z `) are stripped. Run the gate
  locally with `CI=true` and the agent session's variables (for example `CLAUDECODE`, `AI_AGENT`) unset.
- **Quick gate.** In a session, instead of the whole gate, run the gate's own runner on the red targets
  and on what the change touches: `cargo test --locked --jobs 2 --test <stem> ... -- --test-threads=2`
  for each touched `tests/<stem>.rs`; `cargo test --locked --jobs 2 --lib <module::path> --
  --test-threads=2` for a touched module under `src/` (`--bin stele <filter>` for `src/main.rs`); and
  `CARGO_BUILD_JOBS=2 ./accept.sh --tty-only` when a render path, a fixture or a golden is touched.
  A `--lib` filter that matches no test runs nothing (`src/fetch/url.rs`, say, is tested in
  `tests/fetch_url.rs`), so check the `test result` line counts what you meant. It prints the same
  `test <path> ... FAILED` and `FAIL A<n>...` lines. The quick gate builds for the host only; the i486
  build stays in CI (non-negotiable 3).
- **Bounded workers.** `cargo test` takes `--jobs 2` (build jobs) and `-- --test-threads=2` (test
  threads); keep both on a local run. accept.sh's own `cargo build --release` takes
  `CARGO_BUILD_JOBS=2` in the environment.
- **Known red.** The graph's `gate.known_red` lists the tests that fail today, by the name their
  failure line prints. A failing test that is not listed is a failure of the change. A listed failure
  stays until a node fixes it, and that node removes it from the list. Why each entry is listed is in
  the graph's comments and `SPEC.md`'s open decisions.
- **Owns.** A node changes only the files it owns (its `owns` globs) and its direct deps' files, and
  places every file it adds where the graph's globs cover it. A node that adds a test or fixture under
  `tests/`, `fixtures/` or `goldens/` depends on the node that owns them.
- **Red target.** A command that exits non-zero before the node's change and 0 after, normally
  `cargo test --locked --jobs 2 --test <stem> -- --test-threads=2` for the node's own test file, or
  `CARGO_BUILD_JOBS=2 ./accept.sh --tty-only` for a new or changed golden.
- **Tests and fixtures.** New tests go in `tests/<topic>.rs` against the library's public API (every
  module but `dom_util` is `pub`), or through the binary (`CARGO_BIN_EXE_stele`, as
  `tests/stats_cli.rs` does); shared helpers are in `tests/support/`. Fixtures are in `fixtures/`
  (HTML and images), goldens in `goldens/` (`.txt` and `.png`, byte-compared). Existing inline
  `#[cfg(test)]` modules inside `src/` stay where they are: an implementer never edits a
  `#[cfg(test)]` block, and the reviewer checks that. `src/**/*_tests.rs` files are tests. A golden is
  blessed only after its render is measured and verified (non-negotiable 4).
- **Spec files:** `SPEC.md`, `docs/graph.yaml`, `nodes/`, `stele-charter.md`, `stele-build-brief.md`, `DECISIONS.md`, `README.md`, `REPORT.md`, `CLAUDE.md` (a trailing slash marks a folder); a spec PR changes only these.
- **Dialect and decisions.** A node that makes Stele speak something new needs its C2 amendment (with
  the size price) in the charter first: it lands in the spec PR that plans the node. A decision for
  `DECISIONS.md` is recorded in a spec PR too. `JOURNAL.md` is frozen history; the merged node PRs and
  the graph are the log.
- **Porting rules.** None.
- **CI evidence.** In the node's PR, record from the `m0-acceptance` run on the PR's head commit: the
  gate's result (the `build` job's A3 and unit-test steps), the `accept` job's result (A1, A2, A4,
  A5z, A6a), the `stele-i486` size from the A2 line and its delta from `main`'s, and the red target's
  output before and after. A changed golden also records how its render was measured.
