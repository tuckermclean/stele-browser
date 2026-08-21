# Attestation page Plan · Spec: docs/superpowers/specs/2026-08-21-attestation-modal-design.md (read it first)

**Goal:** add `about:attestations` — an in-process (no network, no filesystem), infallible-over-all-input
`about:` scheme handler serving an embedded HTML page that lists Stele's own license, its Cargo dependency
roster (real SPDX ids + real license text), and Terminus's full OFL-1.1 text — reached from the `--x11` shell
via a small chrome affordance. "Modal" is a codename; what ships is an ordinary navigated page (spec's own
naming note).

**Architecture (one sentence per moving part, see spec for the why):** a new `src/fetch/about.rs` (sibling to
`data.rs`) adds one arm to `fetch::fetch`'s scheme match (`src/fetch/mod.rs:108-114`); `main.rs`'s
`resolve_url` gets a one-line fix (`|| scheme == "about"`) without which no CLI path can reach it; a new
generator `tools/gen-attestations.py` (mirrors `tools/gen-terminus-glyphs.py`) reads the real, i486-target-
filtered Cargo dependency graph via `cargo metadata` and emits a committed `src/fetch/attestations_data.rs`
(roster + deduped license-text blocks); a hand-authored static shell `assets/attestations.html` plus Stele's
own short GPL notice plus `include_str!("../../third_party/terminus-font/OFL.TXT")` are assembled by
`about::fetch` into the final page body, using `<p>`-per-paragraph structure (never `<pre>` — `white-space:
pre` is cascaded but not yet enforced by layout, spec's Current State finding) so license-text structure
survives the engine's real (collapsing) whitespace model; `chrome.rs` gains one new `ChromeLayout` rect +
paint, `run_x11` gains one new click hit-test calling `history.navigate(Url::new("about:attestations"))`; new
CI goldens (`--dump-text`/`--dump-png about:attestations`) close the loop.

**Global constraints (every task):** no new crate dependency (the generator is a build-time-only Python
script, like `gen-terminus-glyphs.py` — nothing it does becomes a Cargo dependency of `stele` itself); report
the `stele-i486` size delta in the PR against the spec's ≈24–28 KB estimate and the 97,124 B headroom (spec
§Current State, D66); **no local `cargo build`/`cargo test`** — push and read `m0-acceptance`; every code
task starts with a failing test (visible red→green in commit history); totality — `fetch::about::fetch` never
returns `Err` for any `about:` input, mirroring `glyphs::lookup`/`translit::resolve`'s "never vanish"
contract; pixel/text-verify (never eyeball) any new golden, per AGENTS.md rule 4.

**Task ordering / parallelism note:** Task 1 (scheme handler + `resolve_url` fix, serving a PLACEHOLDER body)
and Task 2 (the attestation-data generator) are independent of each other and can run in parallel worktrees —
Task 1 doesn't need real content to prove the dispatch/totality contract, Task 2 doesn't need the fetch
handler to exist to prove the generator's output is correct Rust. Task 3 (assemble the real page body) depends
on BOTH landing first. Task 4 (chrome affordance) depends only on Task 1 (needs `about:attestations` to be a
valid navigable URL; doesn't need the real page content). Task 5 (goldens) depends on Task 3. Task 6 (docs)
lands last, after Task 5's real size delta is known.

---

### Task 1 — `about:` scheme handler + `resolve_url` fix (placeholder content)

**Files:** `src/fetch/about.rs` (new), `src/fetch/mod.rs` (`fetch()`'s match, `mod.rs:108-114`), `src/main.rs`
(`resolve_url`, `main.rs:380-392`).

**Interfaces:** `pub fn fetch(request: &Request) -> Result<Response, FetchError>` in `about.rs` — same free-
function shape as `data::fetch`, infallible (always returns `Ok`). `fetch::fetch` gains `"about" =>
about::fetch(request),`. `resolve_url`'s scheme check becomes `scheme == "http" || scheme == "file" || scheme
== "about"`.

**Failing-test-first steps:**
1. Test (`about.rs`): `fetch(&Request::get(Url::new("about:attestations")))` returns `Ok(Response { status:
   200, .. })` with `content-type` header `"text/html"` and a NON-empty body. **Red** — module doesn't exist.
2. Test (`about.rs`): the body contains a placeholder marker string unique to this task (e.g.
   `"Attestations (placeholder)"`), swapped for the real content in Task 3 — proves the dispatch path end to
   end before the real content exists, same "prove the seam, then fill it" order Task 1/2 of the Terminus plan
   used for `terminus_glyphs.rs`.
3. Test (`about.rs`): a table of hostile/edge `about:` inputs — `"about:"` (bare), `"about:blank"`,
   `"about:xyz"`, `"about:ATTESTATIONS"` (case — decide and assert: scheme is already lowercased by
   `Url::scheme()`, but the PATH is not; pick and test whether `about:Attestations` is treated as unknown, and
   document that choice), a very long garbage path, non-ASCII — every one returns `Ok(Response{status:200,..})`
   with the small unknown-page body, NEVER `Err`, NEVER a panic.
4. Test (`mod.rs::dispatch_tests`, alongside the existing `fetch_routes_file_scheme_to_the_file_fetcher`-style
   tests): `fetch(&Request::get(Url::new("about:attestations")))` dispatches to `about::fetch`, not
   `UnsupportedScheme` — proves the new match arm, mirroring `fetch_rejects_unknown_scheme`'s own pattern for
   what it does NOT match.
5. Test (`main.rs`): extend `resolve_url_passes_through_http_and_file_schemes` (`main.rs:3075`, or a new
   sibling test) with `resolve_url("about:attestations").as_str() == "about:attestations"`. **Red** — today
   it resolves to a bogus `file://<cwd>/about:attestations`.
6. Implement: `about.rs`'s `fetch`, the `mod.rs` match arm, `resolve_url`'s one-line fix. Green.

**Commit:** `feat(fetch): about: scheme handler (placeholder body) + resolve_url passthrough`

---

### Task 2 — `tools/gen-attestations.py` + generated dependency/license data

**Files:** `tools/gen-attestations.py` (new), `src/fetch/attestations_data.rs` (new, GENERATED — header
comment says so, points back at the tool, mirrors `terminus_glyphs.rs`'s own header style).

**Interfaces:**
```rust
pub struct DepEntry { pub name: &'static str, pub version: &'static str, pub spdx: &'static str, pub license_block: usize }
pub const DEPS: &[DepEntry] = &[ /* real roster, i486-target-filtered, proc-macro/build-only deps excluded */ ];
pub const LICENSE_BLOCKS: &[&str] = &[ /* content-hash-deduped license texts, each pre-split into blank-line-delimited paragraphs by the generator (spec §Design 4 — no runtime dependency on white-space:pre) */ ];
```
(exact shape is an implementation detail for whoever authors the tool — the constraint is: every `DEPS` entry
resolves to a valid `LICENSE_BLOCKS` index, and every block's text is already paragraph-split, per spec §4.)

**Failing-test-first steps:**
1. Before writing the tool: resolve and record (in the tool's own top-of-file doc comment, mirroring
   `gen-terminus-glyphs.py`'s own "Upstream source" section) the exact i486 target triple to filter
   `cargo metadata` against (read it from `targets/`/`rust-toolchain.toml`/`accept.sh` — don't guess) and a
   pinned source (repo tag/commit + SHA-256) for `taffy`'s missing `LICENSE-MIT` (spec's flagged risk — `taffy`
   0.13.0 ships no license file in its published crate).
2. Add a Rust test module (in `attestations_data.rs` itself or a sibling `#[cfg(test)]` file) that pins a
   small, HAND-VERIFIED subset before the generator exists to produce it: `DEPS` contains an entry for
   `taffy` with `spdx == "MIT"`; `DEPS` contains an entry for `slotmap` with `spdx == "Zlib"`; `DEPS` does
   **NOT** contain `serde_derive`/`syn`/`quote`/`proc-macro2`/`unicode-ident`/`version_check` (the excluded
   build-only crates, spec's explicit scoping call) or `windows-sys`/`windows-link` (the Windows-only-cfg
   crates that never compile for the i486 target, spec's Current State finding). **Red** — module doesn't
   exist.
3. Test: `rustix`'s, `linux-raw-sys`'s, and `serde`'s `DEPS` entries share the SAME `license_block` index
   (proving the confirmed 3-way content-hash dedup, spec §Design 2, actually happened, not just theoretically
   possible).
4. Test: every `LICENSE_BLOCKS` entry, when split on blank lines, yields more than one paragraph for any text
   longer than ~500 bytes (a structural sanity check that the generator's paragraph-splitting actually ran,
   not a no-op pass-through of one giant blob).
5. Test: `DEPS.len()` matches the real, hand-counted i486-target runtime dependency count from spec's Current
   State trace (5 direct + the transitive runtime set, minus Windows-only and build-only — recompute and pin
   the exact number when the tool is authored, don't copy the spec's prose count blindly since it was derived
   by hand, not by `cargo metadata`).
6. Author `tools/gen-attestations.py`: run `cargo metadata --filter-platform <triple>`, walk the resolved
   graph from the `stele` root node, exclude proc-macro/build-dependency-only nodes and any node unreachable
   under the filtered platform, read each surviving package's `license` field + on-disk `LICENSE*` file(s)
   from the registry cache (falling back to the pinned `taffy` source from step 1 when absent), content-hash
   every license text to dedupe, paragraph-split each text on blank lines, emit `src/fetch/attestations_data.rs`
   with a GENERATED-file header (tool name, `cargo metadata` command used, regeneration instructions, explicit
   note that hand edits will be overwritten).
7. Run the tool for real, commit the generated file. Green (CI, not local).

**Commit:** `feat(fetch): generate the Cargo dependency/license roster (tools/gen-attestations.py)`

---

### Task 3 — assemble the real page body

**Files:** `assets/attestations.html` (new, hand-authored static shell), `src/fetch/about.rs` (real body
assembly, replacing Task 1's placeholder).

**Interfaces:** `about::fetch`'s body-building step becomes: read the static shell, Stele's own short GPL
notice (a `const &str` in `about.rs` itself — short enough not to need its own file), `attestations_data::DEPS`
rendered as a `<ul><li>` roster, `attestations_data::LICENSE_BLOCKS` rendered as `<p>`-per-paragraph sections
(one section per DISTINCT block, cross-referenced from the roster — e.g. "see the MIT text below" rather than
repeating the same MIT text 13+ times inline), and `include_str!("../../third_party/terminus-font/OFL.TXT")`
also paragraph-split into `<p>`s at render-assembly time (a small helper function, NOT baked into the
generator, since `OFL.TXT` is hand-copied verbatim, not itself generated).

**Failing-test-first steps:**
1. Test (`about.rs`): the real body contains the OFL-1.1's own distinctive phrase ("Reserved Font Name") as a
   substring — proves `OFL.TXT` actually made it into the page, not just referenced.
2. Test (`about.rs`): the real body contains at least one `DEPS` entry's exact `name`/`version` pair as a
   substring (e.g. `"taffy 0.13.0"` or however the roster renders it) — proves the generated roster actually
   made it in, not a stale placeholder.
3. Test (`about.rs`): the real body, parsed through `dom::parser::parse`, produces a DOM with at least one
   `<h1>`, at least one `<ul>` with more than 5 `<li>` children (roster sanity — catches an empty/truncated
   roster), and **zero `<pre>` elements** (asserts the "no `<pre>` reliance" design decision was actually
   followed, not just intended — a cheap, durable regression guard against someone "helpfully" wrapping
   license text back into a `<pre>` later and silently collapsing it).
4. Test (`about.rs`): the assembled body is valid enough UTF-8 HTML that `dom::parser::parse` doesn't produce
   a degenerate/empty tree (a totality smoke test, mirroring the fuzz-totality discipline `REPORT.md`
   describes for the rest of the pipeline).
5. Implement: `assets/attestations.html`'s static shell + `about.rs`'s real assembly logic (string
   concatenation/`format!` — no new runtime abstraction needed, this runs at most once per navigation).
6. Green.

**Commit:** `feat(fetch): assemble the real about:attestations page body`

---

### Task 4 — chrome affordance (manual-verify wiring; CI-testable geometry/paint)

**Files:** `src/backend/chrome.rs` (`ChromeLayout`, `layout`, `draw`, `chrome.rs:33-90`+), `src/main.rs`
(`run_x11`'s `ButtonPress` handling, `main.rs:1948-1993`).

**Interfaces:** `ChromeLayout` gains one new `pub` field (a `Rect`, name TBD at implementation — e.g. `attest`)
computed in `layout()` alongside `back`/`throbber`, clamped with the same `saturating_sub`/`.min()` discipline
every other field already uses. `chrome::draw` paints it (reuse `draw_centered_glyph`/`fill_rect`, no new
primitive). `run_x11`'s click handler gains one more `if x11_point_in_rect(lay.<new-field>, x, y)` branch,
alongside the existing `lay.back` check, calling `history.navigate(Url::new("about:attestations"))`.

**Failing-test-first steps:**
1. Test (`chrome.rs`): at a normal window size (mirrors `layout_normal_window_matches_the_spec_geometry`,
   `chrome.rs`'s existing test), the new rect has nonzero width/height and does NOT overlap `back`, `address`,
   or `throbber` (an explicit non-overlap assertion — 4 rect-intersection checks). **Red** — field doesn't
   exist.
2. Test (`chrome.rs`): at a degenerate `0x0`/tiny window, the new rect collapses to zero size without
   panicking (mirrors every existing `ChromeLayout` field's own totality test).
3. Test (`chrome.rs`): `draw` with the new rect present doesn't panic on an empty/zero-size `Surface` (mirror
   existing `draw_*` guard-clause tests, e.g. `draw_back_button`'s `rect.w == 0 || rect.h == 0` early-out).
4. Implement: the new `ChromeLayout` field + its `layout()` geometry + `draw`'s paint call + `run_x11`'s click
   branch.
5. Green (Rust tests). Manual-verify the click itself against a real X server (per AGENTS.md's own
   established `run_x11`-is-manual-verify posture, spec §Design 5) — not automatable in this repo today.

**Commit:** `feat(chrome): attestations affordance — new chrome-bar button navigates to about:attestations`

---

### Task 5 — goldens (`--dump-text`/`--dump-png about:attestations`)

**Files:** `goldens/attestations.txt` (new), `goldens/attestations.png` (new), `accept.sh` (new golden
blocks).

**Steps (integration/golden work — do NOT fold into Tasks 1-4's commits, per AGENTS.md rule 4's own
discipline):**
1. **Resolve the `accept.sh` letter-exhaustion finding (spec's Current State) before writing the block**: the
   tty (`A3`) letter suffix is fully exhausted (`A3`–`A3z`, all 26 in use, grep-confirmed) — pick and justify
   a naming scheme for the new tty golden (a two-character extension, e.g. `A3aa`, is the least surprising
   given the existing convention's shape; a fresh gate letter is the alternative — state the choice in the
   commit, don't silently collide with an existing block). The PNG (`A5`) series still has `x`/`y`/`z` free —
   use `A5x` for the PNG golden, no exhaustion issue there.
2. Push the branch with Tasks 1-4 landed; let `m0-acceptance` build and upload the `stele-host`/`renders`
   artifacts.
3. **Before blessing:** download the artifact, run (or read the gallery's render of) `--dump-text
   about:attestations --cols 80` and `--dump-png about:attestations out.png`. Read the actual text output —
   confirm the GPL notice, the dependency roster (spot-check a few real names/versions), and the OFL text's
   "Reserved Font Name" phrase are all present and legible, no missing-glyph tofu (`translit`'s "N missing
   glyphs" stat, if `--stats` is run alongside, should be zero or explainable). Pixel-measure the PNG (not
   eyeball) — confirm it's not blank/all-white, has a reasonable non-trivial content height, and doesn't show
   an obvious crash-shaped artifact.
4. Add the `accept.sh` blocks (bless-if-absent + diff-if-present, mirroring every existing `A3`/`A5` block's
   exact shape) for both new goldens, using `about:attestations` as the literal `<src>` argument (no fixture
   file needed — `resolve_url` now passes it straight to `fetch::fetch`, Task 1).
5. Bless both goldens from the CI-verified render (never a local render, AGENTS.md rule 3).
6. If the render doesn't match the read-through in step 3 (garbage text, blank PNG, missing sections), stop
   and re-diagnose — do not bless past it (AGENTS.md rule 4).

**Commit:** `test(golden): about:attestations tty + PNG goldens`

---

### Task 6 — docs: `REPORT.md` refresh, `DECISIONS.md`, `JOURNAL.md`, size-delta report

**Files:** `REPORT.md` (Dependencies & licenses table, `REPORT.md:100-109`; License section,
`REPORT.md:162-174`), `DECISIONS.md` (new entry), `JOURNAL.md` (new entry), `third_party/terminus-font/NOTICE.md`
(small update — its own text currently says attribution "lands via the forthcoming attestation modal"; update
that phrase now that it has landed).

**Steps:**
1. `REPORT.md`'s Dependencies & licenses table: fix the stale `font8x8_basic`/`font8x8_ext_latin` rows
   (`REPORT.md:108-109` — WRONG today, `font8x8` was removed by the Terminus packet, spec's Current State
   finding) to a single **Terminus (191-glyph subset)** row (matches the Terminus plan's own Task 7, which
   this packet's ground-truthing discovered was never actually applied); add the full runtime dependency
   roster this packet's generator now produces (Task 2), so `REPORT.md`'s hand-maintained table and the
   in-binary generated roster describe the same ground truth going forward — note in the PR that keeping them
   in sync is a manual step unless a future packet generates `REPORT.md`'s table too (explicitly out of scope
   here — flag, don't silently defer).
2. `REPORT.md`'s closing License section (`REPORT.md:162-174`) and its final italicized "Fonts:" line: update
   from "font8x8_basic (Public Domain)" to Terminus's real attribution; add a sentence noting the new
   `about:attestations` page as the user-facing, in-binary version of this same information.
3. `third_party/terminus-font/NOTICE.md`: update the "lands via the forthcoming attestation modal" line (D66's
   own phrasing) to reflect that it has now landed, with a pointer to `about:attestations`.
4. `DECISIONS.md`: new entry, next free letter after D66 (**D67** as of this writing — reconfirm against the
   current file before writing), covering the 5 points in the spec's own Charter/decisions note: the `about:`
   scheme's infallibility contract, the embed-real-license-texts-not-summaries call + measured byte cost, the
   generated-roster-over-hand-table choice, the chrome affordance's manual-verify scope, and the `resolve_url`
   fix.
5. `JOURNAL.md`: append an entry (newest at the bottom) — what shipped, the real `stele-i486` size delta
   (Task 5's CI artifact, reported against this design's ≈24–28 KB estimate and the 97,124 B headroom), and a
   note that this packet ALSO fixed `REPORT.md`'s pre-existing font8x8 staleness (found during ground-
   truthing, not this packet's original scope, but adjacent and cheap to fix in the same PR) — **also note,
   if true at implementation time, that `JOURNAL.md` itself had no entry for the Terminus font packet's own
   landing** (this session's ground-truthing found `DECISIONS.md` has D65/D66 but `JOURNAL.md`'s last entry
   predates them) — back-fill a short Terminus entry here too if still missing, so the two ledgers stay
   consistent (AGENTS.md's own "keep this document alive" spirit).
6. Report the ACTUAL CI-measured `stele-i486` size delta in the PR description, against both the spec's
   estimate and the 1,474,560-byte floppy ceiling.

**Commit:** `docs: REPORT.md refresh + DECISIONS/JOURNAL entries for the attestation page`

---

## Verify (whole plan, before opening the PR)
- `cargo test` green in CI (not locally) across all six tasks' new/updated tests.
- `./accept.sh` green in CI, both host and i486 (`m0-acceptance`), including the two new golden blocks.
- Both new goldens pixel/text-verified per AGENTS.md rule 4 — not bulk-blessed blind.
- `stele-i486` binary size delta reported against the spec's ≈24–28 KB estimate and the 97,124 B / 1,474,560 B
  floppy ceiling — confirm actual headroom remaining, don't just repeat the design's projection.
- `resolve_url("about:attestations")` passthrough confirmed by test, not assumed.
- `fetch::about::fetch` confirmed total (never `Err`) over the hostile-input table in Task 1 step 3.
- The embedded page confirmed to contain **zero `<pre>` elements** (Task 3 step 3) — the concrete, durable
  check that the `white-space: pre` gap was designed around, not silently reintroduced.
- `DECISIONS.md` (new D67) + `JOURNAL.md` (new entry, plus the Terminus back-fill if still missing) +
  `REPORT.md` (refreshed) all updated; the "short GPL notice sufficiency" and "no charter amendment" judgment
  calls both flagged explicitly in the PR description for the operator, per the spec's own Risks section.
- Confirm explicitly in the PR description: the chrome affordance's click-to-navigate wiring is manual-verify
  only (no X11 test harness in this repo) — its geometry/paint ARE CI-golden-tested; the click itself is not,
  consistent with every other `run_x11` interaction in this codebase.
