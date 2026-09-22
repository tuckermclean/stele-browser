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
