# Acid2 scroll-to-fragment + viewport-anchored `position:fixed` — design

**Date:** 2026-08-20 · Milestone A only: make the Acid2 smiley **compose** at the window top (scroll the
document so `#top` sits at the viewport's top edge, and anchor `position:fixed` content to the window instead
of its DOM parent). Builds directly on `docs/superpowers/specs/2026-08-20-fixed-viewport-design.md` (the
`--viewport-height` clamp) and closes the two gaps DECISIONS D63 diagnosed honestly: fixed-viewport clamping
alone clips the face away because nothing ever scrolls the document, and even scrolled, `.picture p` (the
scalp) would move WITH the scroll because `Position::Fixed` is mapped straight onto taffy `Absolute` — anchored
to its DOM parent (`.picture`), not the window (D55 Finding A: "`Fixed` anchors to its containing block [the
body content box], not the viewport root").

**OUT OF SCOPE for this milestone** (a later one): eye-image fidelity, byte-exact float/em geometry matching
the WaSP reference PNG pixel-for-pixel, a full pass of the KILL-test. This milestone's bar is: the smiley
**composes** — the scalp, forehead, eyes, nose, smile etc. land inside the 800×600 window overlapping each
other into a face shape, not sprawled 3960px down an unscrolled document. A5-gate golden for this milestone is
pixel-*measured* (connected-component/color-region sanity, per AGENTS.md's discipline), not a byte-diff against
the official Acid2 reference bitmap.

## Non-negotiables (AGENTS.md, unchanged by this packet)
- **No JavaScript, by construction** (charter C3) — nothing here touches `src/dom/`'s closed sum type.
- **1.44 MB floppy ceiling.** New code is pure Rust logic (a lookup helper, two struct fields, a paint
  conditional, ~40 lines of CLI parsing) — no new dependency, no new asset. Report the `stele-i486` artifact
  size delta in the PR against the current floppy headroom.
- **CI-driven build/test.** No local `cargo build`/`cargo test`. Push, read `m0-acceptance`, download the
  `stele-host`/`renders` artifact to bless goldens.
- **Goldens are byte-compared; pixel-verify before blessing, never rubber-stamp.** This packet **deliberately
  changes** `goldens/pos-fixed.png` (see §3's "expected re-bless" — this is a documented correctness fix, not a
  regression) and adds one new golden. Both re-blesses must be pixel-measured, not eyeballed.
- **Totality / no panic on hostile input.** An unknown `--scroll-to <id>` (id not present in the document) must
  degrade to `scroll_y = 0` (no scroll), never panic, never crash the render — same posture every other
  CLI flag in `main.rs` already has ("missing/bad value is a no-op").
- **Test-first.** Every task below starts with a failing test.

## Current state (ground-truthed 2026-08-20 — verify against a fresh `git log` on this file's own packet)

1. **No id→fragment mapping exists.** `LayoutNode` (`src/layout/mod.rs:40-56`) carries `style`/`content`/
   `children`/`interactive` — no DOM node id, no `NodeId`. `Fragment` (`src/layout/mod.rs:116-134`) carries
   `rect`/`kind`/`interactive`/`clip` — same gap. `box_tree::build_node` (`src/layout/box_tree.rs:107-326`) has
   ~30 separate `LayoutNode { .. }` construction sites (one per element-kind branch: text, replaced, `<object>`,
   form control, `<form>`, `<a>`, `<br>`, `<details>`, list container, the generic element fallback, table
   attribute handling, pseudo-elements, markers…) and never reads `el.attrs.get("id")` at all today — only
   `style::cascade`/`style::selector` read `id` (for `#id` selector matching, e.g. `cascade.rs:743`,
   `selector.rs:200`). The DOM itself DOES carry `id` (`Element.attrs: AttrMap`, `AttrMap::get` at
   `src/dom/ast.rs:92`) — the information exists, it just never survives the DOM→`LayoutNode` translation.

2. **`emit` assigns fragment coordinates by parent-relative accumulation.** `layout::block::emit`
   (`src/layout/block.rs:2089-2146` for the `Container` arm) takes `parent_origin: Point` and computes
   `origin = parent_origin + layout.location` (`block.rs:2098`) for every node, then recurses into children
   passing `origin` (its OWN absolute position) as the child's `parent_origin` (`block.rs:2131/2135/2139/2145`,
   one call per CSS-2.1-Appendix-E paint-order bucket: negative z, static, auto/0 z, positive z). This is
   correct for `Static`/`Relative`/`Absolute` boxes (CSS 2.1's containing-block chain is genuinely parent-
   rooted, modulo the already-documented D55 CB-resolution approximation) but WRONG for `Fixed`: CSS defines
   `position:fixed`'s containing block as the **initial containing block** (viewport), not its DOM parent.
   `map_position` (`block.rs:1775-1780`) collapses `Absolute`/`Fixed` onto the SAME taffy `TPosition::Absolute`
   with a comment that says outright: *"`Fixed`'s viewport-relative containing block … is handled in layout,
   not here"* — but nothing currently does that. `built_position` (`block.rs:1785-1792`) already exists and
   cheaply recovers a `Built` node's real CSS `position` for exactly this kind of dispatch (today only used for
   z-index paint-order bucketing).
   - **Confirmed empirically:** `fixtures/pos-fixed.html`'s `position:fixed;top:0;right:0` div is a direct
     child of `<body>`. Stele's UA stylesheet gives `body` an 8px margin (`src/style/ua.rs:39`), so under
     today's parent-relative mapping the fixed box lands at **(752, 8)**, not the CSS-correct **(760, 0)**
     (800px viewport, 40×40 box) — this is the exact D55 Finding A "off by the UA 8px body margin" case,
     reproduced by reading the fixture + the UA sheet, not just re-quoting the decision.
   - **Confirmed safe:** `fixtures/httpforever.html`'s `.switcher` (also `position:fixed`) is ALSO a direct
     child of `<body>`, but that fixture's own author CSS sets `body { margin: 0; }` (`httpforever.html:102`)
     — `.switcher`'s DOM parent origin is therefore already `(0,0)`, identical to the viewport origin, so fixing
     the CB resolution is a byte-identical no-op for `goldens/httpforever.light.png`/`.dark.png`. No re-bless
     expected there (verify this claim in CI, don't take it on faith — see Task 3).
   - `fixtures/pos-nested.html` uses `position:absolute` (not `fixed`) inside a `position:relative` parent —
     entirely untouched by this packet, which only changes `Fixed`'s handling.

3. **`paint_at` already has a uniform per-fragment y-shift — the scroll primitive already exists, just not
   fixed-aware.** `backend::raster::paint_at(surface, fragments, bg_images, canvas, y_offset)`
   (`src/backend/raster.rs:105-178`) shifts EVERY fragment's `rect.origin.y` (`raster.rs:123-127`) AND its
   `clip`'s `origin.y` (`raster.rs:135-139`) by the same `y_offset`, painting the FULL fragment sequence (not a
   culled slice) so `synthesize_gap_rect`'s cross-fragment state stays correct (D54). It's already used for
   real scrolling today: `main.rs`'s interactive `--x11` shell (`paint_viewport_band`, `main.rs:1300-1315`)
   calls `raster::paint_at(&mut band, &state.fragments, &state.bg_images, Color::WHITE, -(band_page_y as f32))`
   on every scroll. That path is host-only and un-goldened (D62: "the `--x11` event loop is manual-verify-only,
   no CI") — no existing golden exercises a nonzero `y_offset`. `paint`/`--dump-png` always call
   `paint_at(.., 0.0)` (`raster.rs:85-87`, `main.rs` `dump_png_opts`), so `y_offset` is always `0.0` on every
   goldened path today — headroom to change `paint_at`'s behavior for nonzero offsets without touching a single
   existing golden.

4. **CLI (`main.rs`):** `Args` (`main.rs:68-159`) + `parse_args` (`main.rs:187-327`) already have the exact
   pattern this packet needs to extend: `--viewport-height <N>` is recognized BOTH as a standalone flag
   (`main.rs:301-310`) and inline between `--dump-png` and its two positionals (the "any slot" loop,
   `main.rs:199-262`, which must explicitly skip recognized flags or they get silently swallowed as
   positionals — see that loop's own comment, `main.rs:205-226`, which documents exactly this trap for
   `--chrome`). `build_dump_png_render` (`main.rs:726-830`) is the single fetch→parse→cascade→box-tree→layout
   pipeline shared by `dump_png_opts` and the `--chrome` path; it already branches on `viewport_height` to pick
   `layout::layout_viewport` vs `layout::layout` (`main.rs:787-790`) and to pick the canvas height
   (`main.rs:803-820`). `dump_png_opts` (`main.rs:695-704`) is the thin paint+encode wrapper that currently
   always calls `raster::paint` (`y_offset` hardcoded to `0.0` via `paint`'s own forwarding, `raster.rs:86`).

5. **Goldens/gates:** `tools/render-gallery.sh` already has a (non-goldened, gallery-only) "Acid2 in a FIXED
   800×600 viewport … for the P7 smiley measurement" block rendering `acid2.html --viewport-height 600` to
   `acid2-viewport.png` (near the file's end) — this is D63's own experiment, kept as a standing gallery
   entry. `accept.sh` gates every document-render PNG golden through `HOST_BIN` (the host build), and — per
   A5v's own comment (`accept.sh:1352-1363`) — a plain document render (no browser chrome) is "cross-build
   stable" and runs in BOTH host and i486 `accept.sh` passes, unlike A5u (chrome, host-only, text-metric float
   drift across x87). A5t (`accept.sh:1300-1316`) is Acid2's existing smoke-only check (non-empty PNG, no
   crash) — D61's explicit deferral of the real smiley golden. The four diagnostic fixtures/blocks
   (`fixtures/repro-marginbottom.html`, `fixtures/repro-acid2margin.html`, `fixtures/acid2-dbg.html`,
   `fixtures/acid2-nomb.html`, and their matching blocks in `tools/render-gallery.sh`) are NOT referenced by
   `accept.sh` at all (grepped — zero hits) — safe to delete without touching any gate.

## Design

### §1 — id→document-y lookup
Add `pub id: Option<Box<str>>` to `LayoutNode` (`mod.rs:40`), same shape/rationale as the existing
`interactive: Option<Interactive>` field (doc comment precedent at `mod.rs:44-56`). Populate it in
`box_tree::build_node` WITHOUT touching all ~30 existing `LayoutNode { .. }` literals inside it: split the
function into an unchanged `build_node_inner` (today's full body, returns `Option<LayoutNode>`, every literal
gains a trivial `id: None,`) and a thin outer `build_node` wrapper that calls it, then — only when
`dom.node(id)` is `Node::Element(el)` — overwrites `.id = el.attrs.get("id").map(|s| s.trim().to_string()...)`
on the `Some(node)` result before returning (mirroring `selector.rs:200`'s own `id` normalization — lowercase
is NOT applied to HTML `id` matching elsewhere in this codebase, so match `selector.rs`'s existing
case-sensitivity convention exactly, don't invent a new one). This is a one-function edit, not a 30-site
edit, for the *value*; the field still needs `id: None,`/`id: Some(..)` typed at every literal for the
compiler's sake (Rust struct literals are exhaustive) — mechanical, compiler-enforced (can't silently miss
one), but real: **~30 sites in `box_tree.rs` + a handful in `block.rs`/`mod.rs`'s own doc-tests, if any.**

Add a small pure helper (new fn in `layout/mod.rs`, alongside `layout`/`layout_viewport`, so it stays next to
its natural callers):
```
pub fn find_fragment_top(fragments: &[Fragment], id: &str) -> Option<f32>
```
Scans `fragments` for the FIRST `Fragment` whose `id.as_deref() == Some(id)` and whose `kind` is `FragmentKind::
Box { .. }` (an element's own box fragment — `emit`'s `Container`/`Replaced`/`Table` arms each push exactly one
such fragment per node, always BEFORE that node's children, so "first match" is unambiguous for any real
document). Returns `rect.origin.y + border_top_width` — the **padding-box top edge** (`rect.origin` is the
BORDER-box top-left per `emit`'s own `origin = parent_origin + layout.location` convention, e.g. see the
`Built::Table` arm's own `content_box_x()/y()` vs raw `layout.location` distinction at `block.rs:2250-2251` for
the same border-vs-content distinction already made elsewhere in this file); `border_top_width` comes from the
`Box` fragment's own `style.border.top.width`, `finite_nonneg`-clamped the same way `base_style` already does
(`block.rs:1618`). `None` if no fragment carries that id (never a panic — the CLI degrades to `scroll_y = 0.0`,
§4). Acid2's `#top` is `<h2 id="top">` with no border, so `border_top_width == 0` there — the padding-vs-border
distinction matters for correctness/generality, not for this specific fixture's own numbers.

### §2 — Fragment carries `is_fixed`; where it's set
Add `pub is_fixed: bool` to `Fragment` (`mod.rs:116`, next to `id` from §1) — same "small by design" posture
the `interactive`/`clip` fields' doc comments already argue for (`mod.rs:118-133`). Set it at every fragment-
push site in `emit`/`push_replaced_fragment` (`block.rs`, six `Fragment { .. }` literals: the `Container` arm's
own box push at `block.rs:2103-2108`, `push_replaced_fragment`'s two arms at `block.rs:2060-2065`, the
`Inline` arm's `Text` push at `block.rs:2160-2169`, the `Table` arm's own box push at `block.rs:2234-2239`, and
the per-cell fragment copy at `block.rs:2299-2305`) from `built_position(built) == Position::Fixed` (the
existing helper, `block.rs:1785-1792`) evaluated on the OWNING node — a `Fixed` container's text/image
descendants inherit `is_fixed = true` too (an inline run or replaced atom under a `position:fixed` block is
still part of the fixed subtree; `built_position` on the `Inline`/`Replaced` `Built` variant itself would
report `Static` per its own doc comment, so this must be threaded down as a parameter alongside `clip`, not
re-derived per-fragment from `built` alone — see §3, which needs the identical "is this subtree fixed" signal
for its own recursion anyway, so the two land together as one new `bool` parameter on `emit`). Every OTHER
existing `Fragment { .. }` construction site outside `block.rs` (test-helper literals in `backend/tty.rs`,
`backend/x11.rs`, `backend/raster.rs`'s own tests) gets a trivial `id: None, is_fixed: false,` — same
compiler-enforced mechanical note as §1.

### §3 — viewport-anchored `position:fixed`
`emit`'s signature (`block.rs:2089-2096`) gains two new read-only parameters, threaded UNCHANGED through every
recursive call (they never vary with tree depth — that's the point: the viewport is one fixed frame of
reference, not a per-ancestor one):
```
fn emit<M: Metrics>(
    built: &Built, taffy: &TaffyTree<NodeCtx>, parent_origin: Point, metrics: &M,
    out: &mut Vec<Fragment>, clip: Option<Rect>,
    viewport_origin: Point, viewport_clip: Option<Rect>,   // NEW
)
```
`layout_tree_impl` (`block.rs:372-444`, the ONE non-cell call site, `block.rs:442`) computes these ONCE, right
after `compute_layout_with_measure` (`block.rs:433-439`) and before the top-level `emit` call: `viewport_origin
= Point { x: 0.0, y: 0.0 }` (the initial containing block's origin, by definition); `viewport_clip` = `Some(Rect
{ origin: viewport_origin, size: <root's own laid-out border-box size> })` IF the ROOT's own `ComputedStyle.
overflow == Overflow::Hidden`, else `None` — i.e. exactly the same `Rect` the root's OWN `Container` arm
invocation would derive as `child_clip` for its immediate children via `intersect_clip` (`block.rs:2115-2119`),
just computed once up front instead of re-derived recursively (the root has no ancestor clip to intersect
against, so this is not a re-implementation, just hoisting the root's own case out of the general recursive
rule). `cell_content_layout`'s own isolated `emit` call (`block.rs:1558`, a table cell's private sub-layout)
passes `(Point { x: 0.0, y: 0.0 }, None)` — a table cell has no independent viewport concept, and no fixture
nests `position:fixed` inside a `<td>` (documented approximation, same posture as this file's own existing
`f.clip` re-origining comment at `block.rs:2306-2319` for the analogous "table cells aren't a scenario this
packet targets" carve-out).

Inside the `Container` arm's four recursive-call sites (`block.rs:2131/2135/2139/2145`, one per paint-order
bucket), branch per child: `if built_position(child) == Position::Fixed { emit(child, taffy, viewport_origin,
metrics, out, viewport_clip, viewport_origin, viewport_clip) } else { emit(child, taffy, origin, metrics, out,
child_clip, viewport_origin, viewport_clip) }` — i.e. a `Fixed` child's `parent_origin` becomes the VIEWPORT
origin (not `origin`, `.picture`'s/whatever-ancestor's real position) and its incoming `clip` becomes the
VIEWPORT clip (not `child_clip`, whatever ancestor `overflow:hidden` chain was in force) — while the `viewport_
origin`/`viewport_clip` PARAMETERS threaded to the recursive call are unchanged either way (a `Fixed` element's
OWN descendants use the same frame of reference its ancestors did — nesting `position:fixed` inside
`position:fixed` is vanishingly rare and CSS still roots both at the ICB).

**Why this is correct for Acid2's scalp specifically, and where it's an approximation:** taffy computed
`layout.location` for the `Fixed`-mapped-to-`Absolute` `<p>` (`.picture p { top: 9em; left: 11em; … }`, no
`right`/`bottom`) purely by adding the resolved `top`/`left` insets to ITS PARENT's padding-box origin (taffy's
own absolute-position algorithm — no dependency on the parent's SIZE unless the opposite inset is also set,
which it isn't here). So `layout.location` already numerically equals "9em from the top of whatever box taffy
treated as the containing block" — REPARENTING that same offset onto the viewport origin instead of `.picture`'s
real (scrolled-away) origin recovers the CSS-correct, viewport-true position with zero extra arithmetic.
**Honest limit:** the scalp's `width: 140%; max-width: 4em` — the `140%` is a percentage of whatever width
taffy treated as the containing block (`.picture`'s content width, per D55's parent-based CB, NOT the true
viewport width) — this packet does not correct percentage-based SIZING against the viewport, only fixed-value
POSITION. Acid2's own `max-width: 4em` clamps the scalp to a fixed 48px regardless, so this specific fixture's
sizing is unaffected by the gap — but a hypothetical `position:fixed; width: 50%` element with no `max-width`
would still size against the wrong containing block after this packet. Documented, not fixed — a real "resolve
Fixed's own box against the true ICB, not just its POSITION" fix is a follow-up (revisit trigger: a fixture that
actually needs it).

**Expected, deliberate golden churn (re-bless, not a regression):** `goldens/pos-fixed.png` moves from
`(752, 8)` to `(760, 0)` (§ Current state, point 2) — pixel-measure the diff (the 40×40 blue box's new bounding
box) before blessing. `goldens/httpforever.light.png`/`.dark.png` are expected UNCHANGED (`.switcher`'s parent
already sits at the viewport origin) — verify in CI, don't assume.

### §4 — paint-time scroll (`paint_at` becomes fixed-aware) + CLI surface
`raster::paint_at` (`raster.rs:105-178`) gates its two existing unconditional `y_offset` additions
(`raster.rs:123-127` for `rect`, `raster.rs:135-139` for `clip`) on `!fragment.is_fixed`. **This is the subtle
half of the fix, not a footnote:** `fragment.clip` for a `Fixed` fragment (post-§3) already holds the
UNSHIFTED viewport rect (`Some(Rect{origin:(0,0), size:(vw,vh)})` from §3's `viewport_clip`) — if `paint_at`
shifted that clip's `y` by `y_offset` the SAME way it shifts an ordinary fragment's clip, a scrolled render
would clip the (deliberately unmoved) fixed content against a WINDOW THAT MOVED, silently clipping it away
past a small `scroll_y`. Both the rect-shift and the clip-shift conditionals must be gated together, or the
fixed content only *looks* right at `scroll_y == 0`. Every existing caller (`paint`/`--dump-png`'s `y_offset ==
0.0`, and `--x11`'s `paint_viewport_band` on documents that today never set `is_fixed = true` because §2/§3
haven't landed pre-packet) is byte-identical: `is_fixed` is `false` for every fragment until §2/§3 exist, and
`0.0 + anything == anything` regardless of the gate, so this is additive, not a behavior change for any
existing golden.

**CLI:** new `Args.scroll_to_id: Option<String>` (`main.rs:68-159`), recognized BOTH as a standalone
`"--scroll-to"` flag (mirrors `--viewport-height`'s standalone arm, `main.rs:301-310`: missing/next-token value
is a no-op) AND inside the `--dump-png` "any slot" loop (`main.rs:227-261`) alongside `--chrome`/
`--viewport-height`, so `--dump-png --scroll-to top --viewport-height 600 <src> <out.png>` parses correctly
instead of `--scroll-to`'s value token being silently swallowed as a positional (the exact trap that loop's own
comment already documents for `--chrome`/`--viewport-height`, `main.rs:205-226`). **Effect gated on
`viewport_height` also being `Some`** — scrolling only means something inside a fixed window; `--scroll-to`
alone (no `--viewport-height`) is a documented no-op, same "unused flag combo, silent no-op" posture `chrome`
already has outside `--dump-png` (`main.rs:143-144`'s own doc comment). `build_dump_png_render`
(`main.rs:726-830`) threads `scroll_to: Option<&str>` through unchanged (fetch/parse/cascade/box-tree/layout is
IDENTICAL to the plain `--viewport-height` path — `layout::layout_viewport` doesn't need to know about
scrolling at all, per §3's design: scroll is purely a PAINT-time transform over already-correct, viewport-
anchored fragments). `dump_png_opts` (`main.rs:695-704`) computes `scroll_y = scroll_to.and_then(|id|
layout::find_fragment_top(&r.fragments, id)).unwrap_or(0.0).max(0.0)` (clamped non-negative — never scroll
"before" the document start; deliberately NOT clamped to `content_height - viewport_height` at the top end,
since `#top` may legitimately be closer to the document's end than one viewport-height, and clamping would
silently refuse the very scroll position Acid2 needs) and calls `raster::paint_at(&mut surface, &r.fragments,
&r.bg_images, Color::WHITE, -scroll_y)` instead of `raster::paint(..)` whenever `scroll_y != 0.0` (or just
always call `paint_at` with `-scroll_y`, since `-0.0 == 0.0` for `f32` and `paint` is already defined as
`paint_at(.., 0.0)` — no behavior difference, simpler code).

### §5 — golden
New fixture-free golden (reuses `fixtures/acid2.html`): `--headless --dump-png fixtures/acid2.html
goldens_out.png --viewport-height 600 --scroll-to top` at the default `DEFAULT_PNG_WIDTH` (800) → an 800×600
PNG. Add as `accept.sh` `A5w` (next free letter after `A5v`), modeled exactly on A5v's own block
(`accept.sh:1352-1383`): runs in BOTH host and i486 passes (this is the document renderer, not the chrome —
cross-build stable per A5v's own reasoning, `accept.sh:1360-1363`; the only float-heavy geometry here is the
SAME `em`-based CSS `layout::block`'s width-clamp/box-model code already handles for every other Acid2-adjacent
golden, e.g. A5c-A5g's `pos-*` goldens and A5t's Acid2 smoke test — no NEW float-determinism risk this packet
introduces). Blessed only after pixel-measuring (per AGENTS.md/brief §10) that the composed render actually
shows overlapping face-colored regions (red/black/yellow/navy) concentrated near the window origin, not the
"only intro text, face clipped away below" shape D63 documented as the FAILING case — a connected-component or
color-histogram check (e.g. "a non-trivial fraction of the yellow/black/red palette appears within the top
200px of the 600px window") is the concrete, scriptable bar for "composes," distinct from and looser than
"byte-matches the WaSP reference" (explicitly out of scope this milestone). `tools/render-gallery.sh` gets a
matching (non-gating) entry `acid2-scrolled.png` alongside the existing (unmodified) `acid2-viewport.png`
block, so a reviewer can see both the unscrolled-clipped and scrolled-composed renders side by side. `A5t`'s
comment (`accept.sh:1301-1305`) gets updated to point at A5w instead of just noting the deferral, since the
deferral this packet resolves is exactly the one A5t's own comment names.

## Testing/fixtures
- `find_fragment_top`: unit tests in `layout/mod.rs` or `tests/layout_block.rs` — id found (returns padding-top
  edge, a nonzero-border case AND a zero-border case), id absent (`None`, no panic), duplicate ids (first match
  wins — HTML validity isn't enforced elsewhere in this dialect either, so this just needs to not panic/loop).
- `is_fixed`/viewport-anchoring: extend `fixtures/pos-fixed.html`'s own coverage — a NEW unit test in
  `tests/layout_block.rs` builds a `LayoutNode` tree with a `position:relative` ancestor offset far from the
  origin (mirrors `pos-nested.html`'s "leading content pushes the relative parent away from the viewport
  origin" shape) wrapping a `position:fixed` descendant, and asserts the fixed descendant's `Fragment.rect.
  origin` equals its OWN resolved `top`/`left`, NOT `ancestor_origin + top/left`.
- `paint_at`'s fixed-aware gate: unit test in `backend/raster.rs`'s own test module — two fragments (one
  `is_fixed: true`, one `false`) at the same nominal `rect.origin.y`, painted via `paint_at(.., y_offset: -N)`;
  assert the ordinary one's PIXELS moved by `-N` and the fixed one's did NOT. A second test specifically for
  the clip-shift bug (§4): a fixed fragment under a `viewport_clip`-shaped clip, painted at a nonzero
  `y_offset`, must still be VISIBLE (not clipped away) — this is the regression this packet's own design doc
  flags as easy to get half-right.
- CLI/end-to-end: a `main.rs`-internal test (reusing the existing `decode_png_pixels`/`decode_png_dims` test
  helpers, `main.rs:3174-3181`) rendering a small synthetic fixture (not `acid2.html` — too large/slow for a
  tight unit test) with a scroll target and a fixed marker, asserting pixel colors before/after `--scroll-to`
  move the way `y_offset`'s own unit test already proved they should.
- `acid2.html` itself: the new A5w golden (§5), pixel-measured before blessing, per AGENTS.md.

## Charter/decisions note
This packet is a **C2 dialect-adjacent** fix (correcting `position:fixed`'s already-adopted containing-block
resolution — no NEW CSS property, keyword, or element), plus a **browser-capability** addition (scroll-to-
fragment rendering, in the same family as the fixed-viewport packet's own "reusable, also what the interactive
browser wants" framing, D63). Record a new DECISIONS entry (D6x, next free letter) covering: the `viewport_
origin`/`viewport_clip` `emit` parameters, the `pos-fixed.png` re-bless (with before/after coordinates), and
the honest scope line — smiley COMPOSES, does not byte-match the WaSP reference (that's the next milestone).
Update `stele-charter.md`'s C2 amendment record in the SAME PR (AGENTS.md rule 6) if a reviewer judges the
fixed-CB fix rises to "amendment" rather than "bugfix to an already-adopted D55 mapping" — flag this ambiguity
explicitly in the PR description rather than silently picking one.
