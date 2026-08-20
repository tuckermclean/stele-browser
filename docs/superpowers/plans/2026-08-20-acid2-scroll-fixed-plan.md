# Acid2 scroll-to-fragment + fixed-anchoring Plan · Spec: docs/superpowers/specs/2026-08-20-acid2-scroll-fixed-design.md (read it)

**Goal:** compose the Acid2 smiley at the window top — scroll a headless render so `#top`'s padding-top edge
sits at window y=0, and anchor `position:fixed` content to the viewport instead of its DOM parent — without
touching a single byte of any EXISTING golden except the one deliberate, documented `pos-fixed.png` re-bless
(spec §3). Milestone A only: compose, not byte-match the WaSP reference.

**Architecture (one sentence per moving part, see spec for the why):** `LayoutNode`/`Fragment` gain an `id`
carrier (spec §1); `Fragment` gains `is_fixed` (spec §2); `layout::block::emit` gains a `viewport_origin`/
`viewport_clip` pair threaded unchanged through recursion, consulted only for `Fixed` children (spec §3);
`backend::raster::paint_at` skips its `y_offset` shift (both `rect` AND `clip`) for `is_fixed` fragments (spec
§4); `main.rs` gains `--scroll-to <id>`, composes `layout::find_fragment_top` + the fixed-aware `paint_at`
(spec §4); one new `accept.sh` golden (spec §5).

**Global constraints (every task):** no new dependency; report the `stele-i486` size delta in the PR; **no
local `cargo build`/`cargo test`** — push and read `m0-acceptance`; total/no-panic on hostile input (missing
id, missing flag value, non-UTF8-ish edge cases already handled elsewhere in `main.rs`'s parser); every task
starts with a failing test (visible red→green in the commit history); pixel-verify (not eyeball) any golden
this plan touches, per AGENTS.md rule 4.

**Task ordering / collision note:** Task 1 touches the `LayoutNode`/`Fragment` struct DEFINITIONS plus every
construction-site literal across `src/layout/mod.rs`, `src/layout/box_tree.rs`, `src/layout/block.rs`,
`src/backend/tty.rs`, `src/backend/x11.rs`, `src/backend/raster.rs` (test helpers) — this is the "shared
low-level infra" AGENTS.md says to pre-assign to ONE task/session, not split across parallel agents. **Task 1
must land (be committed) before Task 2 starts** — every later task's new code reads the fields Task 1 adds.
Tasks 2-5 are then strictly sequential (each reads/extends the previous task's surface); there is no genuine
parallelism opportunity in this feature (a scroll render pixel needs the id lookup, the fixed-anchoring fix,
AND the fixed-aware paint gate all present at once to produce a correct pixel) — do not split this plan across
concurrent worktrees.

---

### Task 1 — carrier fields: `LayoutNode.id`, `Fragment.id`, `Fragment.is_fixed`

**Files:** `src/layout/mod.rs` (struct defs), `src/layout/box_tree.rs` (`build_node`/`build_node_inner` split +
~30 literal sites), `src/layout/block.rs` (6 `Fragment { .. }` literals in `emit`/`push_replaced_fragment`,
sets `is_fixed`/`id` for real), `src/backend/tty.rs`, `src/backend/x11.rs`, `src/backend/raster.rs` (test-helper
`Fragment { .. }` literals — mechanical `id: None, is_fixed: false,`), any `tests/layout_*.rs` file that
constructs `LayoutNode`/`Fragment` directly (grep `LayoutNode {`/`Fragment {` in `tests/` first).

**Interfaces:**
```rust
// src/layout/mod.rs
pub struct LayoutNode { .. , pub id: Option<Box<str>> }
pub struct Fragment { .. , pub id: Option<Box<str>>, pub is_fixed: bool }
```

**Failing-test-first steps:**
1. Add a test in `src/layout/box_tree.rs`'s existing `#[cfg(test)]` module: parse `<div id="x">hi</div>`,
   build the box tree, assert the resulting `LayoutNode`'s (or its first Container-shaped descendant's) `.id ==
   Some("x".into())`. Red (field doesn't exist yet — won't compile).
2. Add a test in `src/layout/block.rs`'s existing `#[cfg(test)]` module (near `base_style_maps_position_and_
   inset_to_taffy`, `block.rs:2531`): a `LayoutNode` tree with a `<div id="target">` runs through
   `layout::block::layout_tree`; assert exactly one output `Fragment` has `.id == Some("target".into())` and it
   is the `Box` fragment (not a descendant text run). Red.
3. Add a test in the same module: a `position: fixed` node's own `Box` fragment has `is_fixed == true`; a
   sibling `position: static` node's does not. Red.
4. Implement: add both fields to the two structs; split `box_tree::build_node` into `build_node_inner`
   (today's body verbatim, `id: None,` added to every literal) + a thin `build_node` wrapper that calls it and
   overwrites `.id` from `el.attrs.get("id")` when the source DOM node is `Node::Element` (spec §1); in
   `block.rs`'s six `Fragment { .. }` sites, set `is_fixed: built_position(built) == Position::Fixed` (or the
   threaded-down equivalent once Task 3 exists — for THIS task, before `emit`'s signature changes, it's fine to
   derive it locally from `built`/the owning style, since the viewport-threading param doesn't exist yet;
   Task 3 will replace this local derivation with the threaded parameter for inline/replaced descendants of a
   fixed container — leave a `// TODO(Task 3)` comment marking the two spots that need it) and `id` copied from
   the source `LayoutNode.id` the same way `interactive` already is. Fix every other construction site the
   compiler flags (mechanical — cannot silently miss one, Rust struct literals are exhaustive).
5. Green: run the three new tests (CI, not local — push and check `cargo test` in `m0-acceptance`).

**Commit:** `feat(layout): carry element id + is_fixed provenance onto LayoutNode/Fragment`

---

### Task 2 — `layout::find_fragment_top`

**Files:** `src/layout/mod.rs` (new pure fn, near `layout`/`layout_viewport`).

**Interfaces:**
```rust
pub fn find_fragment_top(fragments: &[Fragment], id: &str) -> Option<f32>
```

**Failing-test-first steps:**
1. Test: build a tiny tree via `box_tree`/hand-built `LayoutNode`s with a `<div id="target">` whose computed
   style has a top border (e.g. `border-top-width: 4px`) inside some ordinary flow offset; lay it out via
   `layout::layout`; assert `find_fragment_top(&fragments, "target") == Some(expected_padding_top_y)` where
   `expected_padding_top_y = border_box_top_y + 4.0`. Red (fn doesn't exist).
2. Test: same tree, `find_fragment_top(&fragments, "nonexistent") == None` (never panics). Red→trivially green
   once implemented, but write it BEFORE the implementation anyway (totality is a first-class requirement here,
   not an afterthought).
3. Test: a zero-border element — `expected_padding_top_y == border_box_top_y` (the common case; Acid2's own
   `#top` has no border, so this is the path that fixture actually exercises).
4. Implement per spec §1: linear scan, first `Fragment` with `id.as_deref() == Some(id)` AND
   `matches!(kind, FragmentKind::Box { .. })`, return `rect.origin.y + finite_nonneg(style.border.top.width)`.
5. Green.

**Commit:** `feat(layout): find_fragment_top — resolve an element id to its document-space padding-top edge`

---

### Task 3 — viewport-anchored `position:fixed` in `emit`

**Files:** `src/layout/block.rs` (`emit`'s signature + its 4 recursive call sites in the `Container` arm, the 2
top-level call sites in `layout_tree_impl`/`cell_content_layout`, the two `// TODO(Task 3)` spots from Task 1).

**Interfaces:**
```rust
fn emit<M: Metrics>(
    built: &Built, taffy: &TaffyTree<NodeCtx>, parent_origin: Point, metrics: &M,
    out: &mut Vec<Fragment>, clip: Option<Rect>,
    viewport_origin: Point, viewport_clip: Option<Rect>,
)
```

**Failing-test-first steps:**
1. Test in `block.rs`'s test module: a tree shaped like `fixtures/pos-nested.html` (a `position:relative`
   ancestor pushed away from the origin by leading flow content) wrapping a `position:fixed;top:Npx;left:Mpx`
   descendant with no `right`/`bottom`; lay out via `layout::block::layout_tree`; assert the fixed descendant's
   `Fragment.rect.origin == Point { x: M, y: N }` (viewport-relative), NOT
   `ancestor_origin + Point { x: M, y: N }` (today's wrong, parent-relative answer). Red against current code.
2. Test: same shape, but nest the fixed descendant two levels deep (fixed inside relative inside relative)
   to prove `viewport_origin`/`viewport_clip` really pass through UNCHANGED regardless of depth, not just
   one level.
3. Test: a document whose root has `overflow: hidden` at a clamped `--viewport-height`-style height (reuse
   `layout::layout_viewport`) containing a `position:fixed` element positioned INSIDE the viewport bounds —
   assert its `Fragment.clip == Some(viewport_rect)` (not `None`, not some intermediate ancestor's clip) — this
   is the clip half of §3, load-bearing for Task 4's own clip-gate test to have something real to exercise.
4. Implement: compute `viewport_origin`/`viewport_clip` once in `layout_tree_impl` right after
   `compute_layout_with_measure` (spec §3); pass `(Point { x: 0.0, y: 0.0 }, None)` from `cell_content_layout`;
   thread both through every recursive `emit` call; in the `Container` arm's four paint-order buckets, branch
   per child on `built_position(child) == Position::Fixed` to swap `parent_origin`→`viewport_origin` and
   `clip`→`viewport_clip` for that ONE recursive call (the threaded `viewport_origin`/`viewport_clip`
   parameters passed to the recursive call itself stay the ambient ones, unchanged). Resolve the two
   `// TODO(Task 3)` spots from Task 1's `is_fixed` derivation to read the now-threaded signal instead of
   re-deriving locally, so an inline text run or replaced atom under a `position:fixed` container also reports
   `is_fixed == true` (spec §2's own note that `built_position` alone reports `Static` for `Inline`/`Replaced`
   variants).
5. Green: the 3 new tests, PLUS confirm by reading (not running) `fixtures/pos-fixed.html` and
   `fixtures/httpforever.html` against the spec's own worked-out coordinates (752,8)→(760,0) and "no change"
   respectively — these get pixel-verified for real in Task 6's golden pass, not asserted as unit tests here.

**Commit:** `feat(layout): position:fixed anchors to the viewport, not its DOM parent (D55 Finding A)`

---

### Task 4 — `paint_at` becomes `is_fixed`-aware

**Files:** `src/backend/raster.rs` (`paint_at`'s two `y_offset` sites: `raster.rs:123-127` for `rect`,
`raster.rs:135-139` for `clip`).

**Failing-test-first steps:**
1. Test in `raster.rs`'s test module: two `Fragment`s at the same nominal `rect.origin.y = 100.0` — one
   `is_fixed: true`, one `is_fixed: false` — painted via `paint_at(&mut surface, &fragments, &empty_map,
   Color::WHITE, -50.0)`. Assert (via direct pixel inspection on the `MemSurface`, the same pattern
   `raster.rs`'s existing tests already use) that the NON-fixed fragment's pixels landed at `y = 50` and the
   FIXED fragment's landed at `y = 100` (unmoved). Red against current code (both would land at `y = 50`
   today).
2. Test: a `Fragment` with `is_fixed: true` carrying `clip: Some(Rect { origin: (0,0), size: (W,H) })` (a
   viewport-shaped clip, per Task 3's own clip test), painted at `y_offset = -50.0`, with the fragment's OWN
   `rect` positioned such that it would be clipped away IF the clip rect were (wrongly) shifted by `-50` too but
   IS visible if the clip stays at `(0,0)-(W,H)` unshifted. Assert its pixels ARE visible in the output surface.
   This is the exact regression the design doc calls out as easy to get half-right — write it explicitly, don't
   fold it into test 1.
3. Implement: gate both `r.origin.y += y_offset` (rect) and `c.origin.y += y_offset` (clip) on `!fragment.
   is_fixed`.
4. Green. Confirm (by reading, since `y_offset` is always `0.0` on every existing goldened path) that no
   existing golden's paint call is affected — note this explicitly in the PR description rather than assuming
   it silently.

**Commit:** `fix(raster): paint_at does not scroll position:fixed content (rect or its clip)`

---

### Task 5 — CLI: `--scroll-to <id>`

**Files:** `src/main.rs` (`Args` struct + `Default` impl, `parse_args`'s standalone-flag arm + the `--dump-png`
"any slot" loop, `build_dump_png_render`, `dump_png_opts`).

**Interfaces:**
```rust
struct Args { .. , scroll_to_id: Option<String> }
fn build_dump_png_render(source: &str, no_bg_images: bool, scheme: style::ColorScheme, stamp: bool,
    viewport_height: Option<u32>, scroll_to: Option<&str>) -> Option<DocRender>   // scroll_to param added
fn dump_png_opts(source: &str, no_bg_images: bool, scheme: style::ColorScheme, stamp: bool,
    viewport_height: Option<u32>, scroll_to: Option<&str>) -> Vec<u8>            // scroll_to param added
```

**Failing-test-first steps:**
1. Test in `main.rs`'s test module (reuse `decode_png_pixels`/`decode_png_dims`, `main.rs:3174-3181`): write a
   small synthetic HTML fixture to a temp file (or construct one as a `file://` `data:`-free literal, matching
   however nearby tests already source fixtures — check the existing pattern used by `viewport_height`'s own
   tests before inventing a new one) with (a) a tall spacer, (b) a `<div id="mark">` partway down with a
   distinct background color, and (c) a `position:fixed;top:0;left:0` marker with a DIFFERENT distinct color.
   Render via `dump_png_opts(.., viewport_height: Some(200), scroll_to: Some("mark"))`; decode pixels; assert
   (i) the `#mark` div's color now appears near the TOP of the 200px window (it scrolled into view) and (ii)
   the fixed marker's color is STILL at the top-left corner (unmoved by the scroll) — both assertions in one
   test, since that's the actual composition this packet delivers. Red (flag doesn't exist / isn't parsed).
2. Test: `parse_args(&["--dump-png", "--scroll-to", "mark", "--viewport-height", "200", "src.html",
   "out.png"])` (and a second ordering with the flags in different slots, mirroring the existing `--chrome`/
   `--viewport-height` "any slot" tests if present nearby) parses `scroll_to_id == Some("mark".into())` AND the
   two positionals correctly (the exact swallow-as-positional trap the loop's own comment already warns about).
3. Test: `--scroll-to` given WITHOUT `--viewport-height` is a documented no-op (render is byte-identical to
   the same source with neither flag) — encodes spec §4's "gated on viewport_height also being Some" rule as an
   actual assertion, not just a comment.
4. Test: `--scroll-to nonexistent-id` (with `--viewport-height` present) degrades to `scroll_y = 0.0` (renders
   like an ordinary `--viewport-height`-only call) — never panics, matches Task 2's own `None` case.
5. Implement: add `scroll_to_id` to `Args`; standalone `"--scroll-to"` arm (mirrors `--viewport-height`,
   `main.rs:301-310`); extend the `--dump-png` any-slot loop (`main.rs:227-261`) with a third recognized inline
   flag; thread `scroll_to: Option<&str>` through `build_dump_png_render`/`dump_png_opts` (the fetch/parse/
   cascade/box-tree/layout pipeline itself is UNCHANGED — only the final paint call in `dump_png_opts` differs:
   compute `scroll_y` via `layout::find_fragment_top(&r.fragments, id).unwrap_or(0.0).max(0.0)` when both
   `scroll_to` and `viewport_height` are `Some`, else `0.0`; call `raster::paint_at(&mut surface, &r.fragments,
   &r.bg_images, Color::WHITE, -scroll_y)`).
6. Green.

**Commit:** `feat(main): --scroll-to <id> — headless render scrolled so an element's top edge hits the window top`

---

### Task 6 — Acid2 golden + gallery + fixture cleanup

**Files:** `accept.sh` (new `A5w` block + `A5t`'s comment update), `tools/render-gallery.sh` (new
`acid2-scrolled.png` entry + removal of the four diagnostic blocks), delete `fixtures/repro-marginbottom.html`,
`fixtures/repro-acid2margin.html`, `fixtures/acid2-dbg.html`, `fixtures/acid2-nomb.html`.

**Steps (this task is integration/golden work, not unit-TDD — the "test" is the CI render + pixel measurement,
per AGENTS.md rule 4; still land it as its own reviewable commit, not folded into Task 5):**
1. Add `A5w` to `accept.sh` immediately after `A5v` (`accept.sh:1352-1383`), modeled on A5v's own structure
   (spec §5): `"$HOST_BIN" --headless --dump-png fixtures/acid2.html /tmp/stele_acid2scroll.png
   --viewport-height 600 --scroll-to top`, NOT inside the `TTY_ONLY` guard (runs in both host and i486 passes,
   per spec §5's cross-build-stable reasoning) — `--bless` copies to `goldens/acid2-scrolled.png`, otherwise
   `cmp -s` against it. Update `A5t`'s own comment (`accept.sh:1301-1305`) to reference A5w instead of a bare
   "deferred" note.
2. Push the branch, let `m0-acceptance` build + upload the `renders`/`stele-host` artifact.
3. **Before blessing:** download the artifact, open `acid2-scrolled.png`, and pixel-measure it (per AGENTS.md
   rule 4 — connected-component or color-histogram check, e.g. a script asserting the yellow/black/red/navy
   face palette occupies a non-trivial area within the top ~200px of the 600px window) — confirm it shows the
   COMPOSED shape (spec §5), not D63's documented failure shape (intro text only, face clipped below). If it
   still doesn't compose, this is a genuine finding, not a rubber-stamp opportunity — stop and re-diagnose
   (root-cause, per AGENTS.md rule 5) rather than blessing a wrong render.
4. Also pixel-measure the Task 3 re-bless candidates: `goldens/pos-fixed.png` (expect the 40×40 box's bounding
   box moved from `(752,8)-(792,48)` to `(760,0)-(800,40)`) and confirm `goldens/httpforever.light.png`/
   `.dark.png` render BYTE-IDENTICAL to their current committed versions (if they differ, STOP — that means the
   "`.switcher`'s parent already sits at the origin" ground-truth claim in the spec was wrong, and the
   discrepancy needs root-causing before blessing anything).
5. Bless: `pos-fixed.png` (re-bless, documented reason in the commit message), `acid2-scrolled.png` (new).
   `httpforever.*.png` should need NO re-bless — if `accept.sh` reports them differing, treat that as a
   blocking finding per step 4, not something to silently re-bless away (AGENTS.md rule 4's core discipline).
6. Add `tools/render-gallery.sh`'s `acid2-scrolled.png` entry (gallery-only, non-gating) alongside the existing
   unmodified `acid2-viewport.png` block near the file's end.
7. Delete the four diagnostic fixtures and their matching blocks in `tools/render-gallery.sh` (the
   `repro-marginbottom`/`repro-acid2margin` loop, and the standalone `acid2-nomb`/`acid2-dbg` blocks) — grep
   `accept.sh` first to reconfirm zero references (already verified zero during design; reconfirm at
   implementation time in case something changed) before deleting.
8. Update `DECISIONS.md` (new entry, spec's "Charter/decisions note") and `JOURNAL.md` (AGENTS.md rule: append
   on finishing a chunk). Flag the C2-amendment-or-not ambiguity (spec's own note) in the PR description for
   the operator to rule on, rather than silently picking one.

**Commit(s):** `test(acid2): scrolled-to-#top window golden (A5w)` + `fix(golden): re-bless pos-fixed.png —
viewport-anchored fixed positioning (D55 Finding A)` + `chore(fixtures): remove positioning-fidelity diagnostic
repros` (three separate commits, each independently reviewable — don't squash the re-bless into the new-golden
commit, a reviewer should be able to see exactly which pixels moved and why for each).

---

## Verify (whole plan, before opening the PR)
- `cargo test` green in CI (not locally) across all six tasks' new tests.
- `./accept.sh` green in CI, both host and i486 (`m0-acceptance`).
- Every re-blessed/new golden pixel-measured per AGENTS.md rule 4 — the PR description states WHAT was
  measured and WHY it's correct, not just "CI is green."
- `stele-i486` binary size delta reported against the 1,474,560-byte floppy ceiling.
- DECISIONS.md + JOURNAL.md updated; charter C2 ambiguity flagged for the operator.
