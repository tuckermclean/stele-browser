# CLAUDE.md

Claude Code (and any other AI agent): your operating manual for this repository is
**[AGENTS.md](AGENTS.md)**. Read it first.

It covers the non-negotiables you must not violate silently — no JavaScript *by
construction* (charter C3), the 1.44 MB floppy budget, the CI-driven build/test loop
(don't build the i486 target locally), and the golden discipline (pixel-verify before you
bless, never rubber-stamp, never bless away a regression) — plus the packet / test-first
workflow and where the living sources of truth live.

This stub intentionally holds no rules of its own, so guidance can't drift between two
files. Put durable working notes in **AGENTS.md** (shared with every agent); keep this
file a pointer. If you maintain separate persistent memory outside the repo, it
supplements AGENTS.md — it does not replace it.
