# Diagnosis: `fixtures/css1-float-5526c.html` renders as a vertical stack

Target: W3C CSS1 §5.5.26 "display/box/float/clear test"
(`fixtures/css1-float-5526c.html`, reference `fixtures/evidence/
css1-float-5526c.reference.gif`). The reference is a Mondrian grid: a
red `<dt>` column on the left, a white `<dd>` column on the right
containing a row of yellow `<li>` cards plus a floated `<blockquote>` and
`<h1>`, all built from `float: left` / `float: right` on ordinary
block-level elements. Stele currently renders it as one tall vertical
column — every block box, in document order, full width. This document
answers the four questions the packet asked, each with file:line, then
lays out a staged plan for the real fix (NOT implemented in this packet).

## 1. Is `float`/`clear` parsed into `ComputedStyle`? Yes, fully.

- `Float` / `Clear` enums: `src/style/computed.rs:104-117`.
- `ComputedStyle.float` / `ComputedStyle.clear` fields:
  `src/style/computed.rs:314-315`.
- CSS-initial defaults (`Float::None` / `Clear::None`):
  `src/style/computed.rs:362-363`.
- Declaration parsing (`Declarations::float`/`Declarations::clear`,
  `apply_property("float"|"clear", ...)`): `src/style/value.rs:144-145`
  (struct fields), `src/style/value.rs:1246-1279` (parsing `left|right|none`
  for `float`, `left|right|both|none` for `clear`).
- These fields cascade like any other property (`Declarations::overlay`'s
  `ov!(float); ov!(clear);`, `src/style/value.rs:219-220`) and are resolved
  into every element's `ComputedStyle` unconditionally — there is no gap at
  the CSS-parsing layer. `#bar`'s `41.17%` width, the `dt`/`dl` `em`
  font sizing, and every other declaration in the fixture's `<style>` block
  parse and cascade correctly too (see point 4 below).

## 2. Does block layout do ANYTHING with `float`, or ignore it?

**It ignores it completely for block-level boxes.** `float`/`clear` are
consulted in exactly one place in the whole layout pipeline —
`src/layout/inline.rs` — and that place only ever sees **inline replaced
atoms** (`<img>`), never generic block containers like `<dt>`/`<dd>`/`<li>`/
`<blockquote>`/`<h1>`:

- `src/layout/inline.rs:1-22` (module doc): the bespoke float mechanism is
  scoped to "the classic `<p><img align=left>text...</p>` shape" — one
  floated *replaced* atom sharing an inline formatting context (IFC) with
  wrapping text.
- `src/layout/inline.rs:373` `collect_float_specs` — only walks
  `InlineRun`s, and only a `Replaced` run ever carries `style.float !=
  Float::None` in practice (`src/layout/inline.rs:291`, `r.style.float ==
  CssFloat::None` is the text-run fast path; a floated `Replaced` run is
  routed to `place_floats` instead, never emitted as a normal atom).
- `src/layout/inline.rs:417-422`: `clear` is explicitly documented as a
  no-op even within this narrow scope ("since floats never escape their own
  containing block's IFC, there is no OTHER block/line in scope that could
  ever need to clear past one").

Block-level boxes never reach any of this code. They go through the taffy
translation in `src/layout/block.rs` instead, and that path never reads
`cs.float`/`cs.clear` at all:

- `src/layout/block.rs:1143-1166` `base_style()` — builds a taffy `Style`
  from size/margin/padding/border only; no float/clear field exists on the
  struct it returns, and none is looked up here.
- `src/layout/block.rs:1168-1210` `apply_flex()` — flex-specific properties
  only.
- `src/layout/block.rs:1212-1237` `map_display()` — maps `Display::Block`
  (the UA-sheet default for `dt`/`dd`/`li`/`blockquote`/`h1`, `src/style/
  ua.rs:15-23` and `:65`) straight to `TDisplay::Block`. There is no
  "is this child floated" branch anywhere in this function or its caller.

And the reason a "taffy-side" fix isn't just a flag flip:
`Cargo.toml:37-43` explicitly disables taffy's own `float_layout` cargo
feature (`Cargo.toml:30-36`'s comment: *"Trimmed to what block.rs/inline.rs
actually use ... no taffy-side floats — floats are bespoke inline work,
M4 ... Default features also pull ... `float_layout` ... all dead weight
here; dropping them shrinks the binary"*). That decision was deliberate and
correct for M4's scope (inline-image floats only) — but it also means
taffy's block-layout algorithm (the one actually driving `dt`/`dd`/`li`/
`blockquote`/`h1` here) has literally no float placement code compiled in.
A block-level float engine has to be bespoke, the same way the inline one
already is.

## 3. What specifically makes the cards stack vertically instead of side-by-side?

`<dt>` and `<dd>` are siblings inside `<dl>` (`display: block` via the UA
sheet, `src/style/ua.rs:17`). Both compute `Display::Block` themselves and
become ordinary taffy child nodes of `<dl>`'s column-flex container
(`translate_container_children`, `src/layout/block.rs:754`, feeding
`base_style`/`map_display` above). Taffy's `block_layout` algorithm (the
only layout algorithm compiled in for `TDisplay::Block`, per the feature
list at `Cargo.toml:37-43`) positions block-level children in **normal
flow**: each one gets the next available vertical offset in document order,
full containing-block width unless it has its own explicit width — which
is exactly a vertical stack. `float: left`/`float: right` on `dt`/`dd`
changes nothing about that placement, because (per point 2) nothing ever
reads `cs.float` on this path. The same mechanism explains every other
symptom in the render: the five `<li>` cards inside `<ul>` stack instead of
wrapping into a row, and the floated `<blockquote>`/`<h1>` stack below the
`<ul>` instead of sitting beside/after it.

## 4. Do percentage width + `em` resolution work here?

**Yes — independently of the float gap.** Length/percentage resolution is
a separate pipeline stage (the cascade, not layout placement) and it's
unaffected by point 2/3 above:

- `em`/`%` resolution against the cascading font size:
  `src/style/cascade.rs:226-370` (`resolve_font_size`, `raw_to_px`,
  `resolve_dimension`, `resolve_lpa`, `resolve_lp` all take `font_size` and
  handle `RawLength::Em`/`RawLength::Percent`). `html { font: 10px/1
  Verdana, sans-serif }` cascades correctly into every descendant's
  `font_size`, so `dt`'s `10.638%`-of-`47em` comment in the source CSS and
  `dd`'s `34em` both resolve to the right pixel values.
  `#bar`'s `41.17%` width resolves the same way.
- Percentage widths reach taffy as real percentages, not a fallback:
  `src/layout/block.rs:1243` (`CssDimension::Percent(p) => percent(p /
  100.0)`), `:1253`, `:1262` (same for `LengthPercentage`/
  `LengthPercentageAuto`). Taffy's `block_layout`/`flexbox` algorithms
  resolve a percentage width against the used width of the *containing*
  block correctly regardless of `float` — this part of the box model was
  never in question.

So the ONLY gap is box **placement**: sizes are right, positions are wrong
(every float sits exactly where normal flow would have put it, i.e.
nowhere near its float edge).

## Safe fixes landed this packet

See the PR body / commit history for `is_item`'s `node.style.display ==
Display::Block` guard in `src/layout/box_tree.rs`'s
`build_list_container_node` (list-marker suppression for a non-block-
display `<li>`) — unrelated to the float gap, landed separately as the
packet's "TINY, obviously-correct fix" deliverable. **Note**: the literal
"`li{display:block}` should suppress the marker" scenario from the W3C
fixture itself (`fixtures/css1-float-5526c.html`'s own `li{display:block;
/* i.e., suppress marker */ ...}`) is NOT fixable this way in Stele — see
that commit's own doc comment for why (Stele's UA sheet already computes
`display: block` for every `<li>`, `src/style/ua.rs:65`, since Stele has no
`Display::ListItem` variant at all; there is no `ComputedStyle` signal that
distinguishes "author explicitly wrote `display: block`" from "that's just
the UA default", so `fixtures/css1-float-5526c.html`'s own `<li>` markers
will still show a `*` in Stele's render until a real list-item-tracking
packet lands). This is a known, explicitly-deferred gap, not an oversight.

## Float engine roadmap (NOT implemented this packet)

A block-level float engine is a real layout-algorithm addition, not a safe
overnight fix — this is a staged plan for a FUTURE, reviewed packet.

### Stage 0 — new minimal fixtures + tests first

Before touching `fixtures/css1-float-5526c.html` at all, add small,
hand-written fixtures exercising ONE float behavior at a time, mirroring
`tests/layout_floats.rs`'s existing M4-inline-image discipline (real
parse→cascade→box_tree→layout pipeline, asserting `Fragment` rects — no
pixel golden yet):

1. Two `float: left` block siblings placed side-by-side (not stacked).
2. A `float: right` block sibling placed at the containing block's right
   edge.
3. One `float: left` block + one normal-flow sibling: the normal-flow
   sibling's content wraps beside the float (reuses/extends the exclusion
   math `inline::line_exclusion` already has for the inline-image case).
4. A `clear: both` block below two floats: pushed down past both floats'
   bottom edges (CSS 2.1 §9.5.2 "clearance").
5. Nested float contexts: a floated block (like `<dd>`) that ITSELF
   contains further floats (like `<dd>`'s `<ul>` of floated `<li>`s) — the
   inner floats must resolve against the *inner* containing block's width,
   not the outer one's.

Each of these should be its own tiny fixture + integration test, landed and
green independently, before the next is attempted.

### Stage 1 — block-level float collection + placement pass

Generalize the existing inline-float mechanism
(`inline::collect_float_specs` / `inline::place_floats` / `inline::
line_exclusion`, `src/layout/inline.rs:373-515`) from "floated `InlineRun`
atoms within one IFC" to "floated `LayoutNode` block children within one
block formatting context (BFC)". Concretely: in
`translate_container_children` (`src/layout/block.rs:754`), partition a
container's block-level children into normal-flow (`float: none`) and
floated (`float: left|right`) before translation. Two implementation
options to evaluate, not pre-decided:

- **(a) Bespoke pre-pass, taffy stays float-blind**: compute each floated
  child's rect directly (mirroring `place_floats`'s edge-stacking algorithm
  extended to left+right collision, which M4 explicitly deferred —
  `src/layout/inline.rs:397-399`), emit it as an absolutely-positioned taffy
  leaf (`taffy::Style::position = Position::Absolute` with explicit
  `inset`), and shrink normal-flow siblings' available width for whichever
  rows/lines fall inside the float's vertical span (needs a taffy
  measure-function hook — the same seam `inline::layout_runs` already uses
  for IFC leaves — so normal-flow BLOCK children, not just inline lines,
  can be told "your available width is narrower here").
- **(b) Re-enable taffy's `float_layout` feature**: turn the cargo feature
  back on (`Cargo.toml:37-43`) and let taffy's own float algorithm place
  these children. Needs an audit of what taffy 0.13's `float_layout`
  actually implements (CSS float semantics vary in fidelity across engines)
  and its binary-size cost (the whole reason it was excluded, `Cargo.toml:
  30-36`) before committing to it over (a).

This stage is the one that actually needs a design decision + prototype
before implementation; everything below assumes whichever of (a)/(b) is
chosen.

### Stage 2 — `clear` for block-level boxes

Once Stage 1 tracks "float bottom per side per BFC", a normal-flow block
child with `clear: left|right|both` needs its own top offset pushed down
past the relevant float(s)' bottom edge before taffy computes its position
— exactly CSS 2.1 §9.5.2 clearance. This is the block-level counterpart to
the no-op documented at `src/layout/inline.rs:417-422`; that inline-only
deferral stays correct and unchanged (a `clear` inside one IFC still can't
escape it), this is strictly the NEW block-level case.

### Stage 3 — shrink-to-fit width for floats with no explicit width

Not needed by `fixtures/css1-float-5526c.html` (every floated element in it
has an explicit `width`), but needed for real-world content. Defer until a
fixture actually needs it, matching the precedent
`inline.rs`'s own M4 scope-cut already set (`src/layout/inline.rs:397-399`,
"no M4 fixture needs left+right floats colliding in one paragraph").

### Stage 4 — attempt `fixtures/css1-float-5526c.html` end to end

Only once Stages 0-2 are individually green: render the full fixture,
compare against `fixtures/evidence/css1-float-5526c.reference.gif` via
PROGRAMMATIC pixel analysis (per this project's own "verify goldens with
pixel analysis" discipline — never bless by eyeballing), and bless a golden
only when close. Full pixel-identity may never be reached (font
rasterization and form-widget chrome are explicitly excused by the test
itself) — the target is structural/positional fidelity: red column left,
white column right, yellow card row, not stacked.

### Expected golden churn

**None of the EXISTING goldens/fixtures use `float` on a block-level
(non-replaced) element** — a repo-wide check
(`grep -rn float fixtures/*.html`) shows the only current `float`/`align=
left` usage is `<img align=left>` in `fixtures/images.html` and
`fixtures/kitchen-sink.html`, both purely inline-image floats that keep
going through `inline.rs`'s existing (untouched) mechanism. So Stage 1-3
landing should cause **zero churn** to any currently-blessed golden; churn
is confined to whatever NEW fixtures/goldens the float packet itself adds,
plus (Stage 4, much later) `fixtures/css1-float-5526c.html` gaining its
first golden.

### Test strategy summary

- Unit tests for the new block-float placement function(s), same style as
  `inline.rs`'s own `#[cfg(test)]` module (edge cases: zero floats, a float
  wider than its containing block, `MAX_FLOATS`-style caps against hostile
  input — the existing `MAX_FLOATS` precedent at `src/layout/inline.rs:92-99`
  should be mirrored for block-level floats too, for the same total-on-
  hostile-input reason).
- Integration tests per Stage-0 fixture (`tests/layout_floats.rs` or a new
  `tests/layout_block_floats.rs`), asserting `Fragment` rects the same way
  `tests/layout_floats.rs` already does for inline floats.
- `fixtures/css1-float-5526c.html` stays UNBLESSED until Stage 4, verified
  by pixel-diffing the rendered PNG against `css1-float-5526c.reference.gif`
  programmatically before any golden is committed.

## Follow-up (packet/acid1-coherence): the real remaining bug was `font`, not floats

Stages 0-4 above all landed (`packet/block-floats` #67, `spike/taffy-
float-layout` #65): taffy's own `float_layout` is wired in
(`Cargo.toml`'s `taffy` feature list; `layout::block::base_style` maps
`ComputedStyle.float`/`.clear` onto it via `map_float`/`map_clear`), and
`goldens/css1-float-5526c.png` already exists and — visually — already
shows the reference's Mondrian-grid *shape*: a red `dt` column on the
left, two rows of three alternating yellow/black cards on the right,
correct text content and card colors (`li#bar`/`li#baz`'s ID-selector
background overrides both resolve correctly). The float engine itself
works.

What's still off is **proportion**, not placement: a pixel-measurement
pass (`python3`/PIL, comparing `goldens/css1-float-5526c.png` against
this file's own reference GIF) found the rendered `dt` column running
~22% narrower than its declared `10.638%` should produce, and `#bar`'s
`41.17%`-wide card running short by almost exactly the same ratio — two
independent percentage-based widths, both shrunk by roughly the same
factor. Isolating the cause with small hand-written fixtures (not
committed — see `apply_font_shorthand`'s own doc comment in
`src/style/value.rs` for the summary) traced it to a single root cause
upstream of any float/percentage math at all:

**`html { font: 10px/1 Verdana, sans-serif }` — the `font` SHORTHAND —
was not recognized by `style::value::apply_property` at all.** Every
`font-*` LONGHAND (`font-family`, `font-size`, `font-weight`,
`font-style`) had its own match arm; the shorthand itself did not, so it
fell to the catch-all `_ => false` and was silently dropped as an ignored
declaration (charter C2's "ignore what you don't understand" treaty,
applied here to a property this engine was never taught to understand in
the first place). The practical effect: `<html>`'s font-size never left
the UA default of 16px, so **every `em` length in the entire document**
— including this fixture's own `dt{width:10.638%}`/`dd{width:34em}`/
`#bar{width:41.17%}`, all of which resolve against an `em`-sized
ancestor width — computed 1.6x (16px/10px) too large against the
`--dump-png` pipeline's fixed 800px viewport (`src/main.rs`'s
`DEFAULT_PNG_WIDTH`). That inflation, combined with the fixed viewport,
is what produced the specific non-uniform-looking shortfalls the pixel
measurements found (some boxes hit viewport-width limits that only bite
at the wrong 16px/em scale; a purely percentage-relative measurement
inside one box isn't affected the same way a box whose ancestor's
declared `Nem` width interacts with a fixed-px viewport is).

Fixed by adding a real (if curated, matching this file's `border`/
`background` shorthand-parsing precedent) `apply_font_shorthand` — see
its doc comment in `src/style/value.rs` for the exact grammar subset
covered (`[style] [weight] size[/line-height] family`) and what's
deliberately left out (`font-variant`, CSS2.1 system-font keywords —
nothing in this repo uses either). No other fixture in the repo uses the
`font` shorthand (`grep -rn 'font:' fixtures/*.html` — only this one), so
this fix is scoped: it cannot change any other golden's render.

## Follow-up 2 (packet/acid1-nested-floats): the "nested floats collapse
## into one vertical band" symptom is NOT present in current `main` —
## root-caused and closed with evidence, not a new block-float engine

This packet was scoped around a specific reported symptom: with the
`font` shorthand fix applied, the outer `dt`/`dd` floats sit side by side
correctly, but the `<li>`/`<blockquote>`/`<h1>` cards *inside* the
floated `<dd>` were reported to collapse into a single vertical band
(around x≈21-50%) instead of flowing left-to-right and wrapping into
rows the way `dd`'s own 34em inner width should force. Investigating this
with evidence (both static analysis of the vendored taffy 0.13 source and
a live CI render, per this project's own "verify goldens with pixel
analysis" discipline) found that **this symptom does not reproduce on
current `main` (`f6a59c0`)** — it was already closed, as a side effect,
by an earlier packet. This section documents the trail so the "why" is
on record rather than silently rediscovered later.

### 1. Does taffy 0.13's `float_layout` even support a float nested
### beneath a floated ancestor?

Yes, by construction — a floated box always gets its OWN fresh
`BlockFormattingContext`, not the parent's:

- `taffy-0.13.0/src/compute/block.rs`'s `generate_item_list` marks a
  floated child `is_in_same_bfc: false` (float excluded via
  `is_not_floated` in the flag's conjunction, around line 767).
- A floated item is laid out via `tree.perform_child_layout(...)` (the
  ordinary GENERIC recursive entry point, `compute/block.rs` ~line 956),
  never `tree.compute_block_child_layout(...)` (the block-context-
  threading call same-BFC children get, ~line 1166). Generic recursion
  passes no `BlockContext`, so `compute_block_layout`
  (`compute/block.rs:401-420`) takes its `_ => { let mut root_bfc =
  BlockFormattingContext::new(); ... }` arm: a brand-new float context,
  scoped to and sized from THIS float's own resolved border-box width
  (`compute/block.rs:897-901`, `if block_ctx.is_bfc_root() {
  block_ctx.set_width(container_outer_width); ...}`).
- An ordinary (non-floated, non-table, non-scroll-container) block child
  — e.g. `<ul>`, the wrapper between `dd` and its `<li>`s — is
  `is_in_same_bfc: true`, so it's laid out via
  `compute_block_child_layout` with the PARENT's `BlockContext` passed
  through (`sub_context`, `compute/block.rs:93-109`), contributing
  `insets` from its own margin/border/padding (zero for a bare `<ul>`)
  but never resetting the shared `FloatContext`'s width
  (`is_bfc_root()` is already `false` for it). So floats inside `<ul>`
  register against the SAME `FloatContext` `<dd>` created, at `<dd>`'s
  own inner content width — exactly the CSS 2.1 §9.4 rule ("floats
  establish a new BFC; ordinary descendants don't") and exactly what
  `fixtures/css1-float-5526c.html`'s `dd > ul > li` shape needs.

### 2. Is this wired correctly on Stele's side?

Yes — checked end to end, not just at the `base_style` call site:

- `src/layout/block.rs`'s `base_style` (~line 1484) sets `float`/`clear`
  on EVERY node's taffy `Style` unconditionally (display-independent, see
  its own doc comment), so `dt`/`dd`/`li`/`blockquote`/`h1` all reach
  taffy with the right `Float`/`Clear` value — this is the wiring
  `packet/block-floats` (commit `88c404c`, already on `main` before this
  packet started) landed, together with re-enabling taffy's own
  `float_layout` cargo feature (`Cargo.toml`, `packet/block-floats`
  comment).
- `<li>` is `display: block` in this fixture (`li{display:block; /* i.e.,
  suppress marker */ ...}`), so it is never folded into an inline-
  formatting-context leaf by `is_inline_ish`
  (`src/layout/block.rs:953-978`: a `Container` is only inline-ish when
  its OWN `display == Inline`) — each `<li>` becomes its own real taffy
  block node, individually floatable, exactly like the passing
  `nested_floats_resolve_against_inner_containing_block_width` test's
  `<div>`s.
- `build_list_container_node` (`src/layout/box_tree.rs:963-1009`, landed
  by `packet/display-list-item`) correctly suppresses the marker for this
  exact case (`is_item = tag_is_li && node.style.display ==
  Display::ListItem`, false here since the author CSS overrides to
  `Block`) and otherwise pushes each `<li>` node through unmodified — no
  extra wrapper is interposed between `<ul>` and its `<li>` children that
  could break the BFC-sharing chain above.

### 3. Empirical confirmation (not just static reading)

- `tests/layout_block_floats.rs` already had
  `nested_floats_resolve_against_inner_containing_block_width` (from
  `packet/block-floats`), proving a float DIRECTLY inside a floated
  parent wraps at the parent's inner width. Its own module doc flagged
  taffy's "TODO: handle nested blocks with different widths" comment
  (`compute/block.rs:896`) as an unexplored rough edge for the ONE shape
  it deliberately did not cover: a floated child nested beneath a
  **plain, non-floated wrapper** (`<dd> > <ul> > <li>`, not `<dd> >
  <li>`) — precisely `fixtures/css1-float-5526c.html`'s own shape.
- This packet added `nested_floats_beneath_a_plain_wrapper_still_wrap_at_
  inner_width` (same file) to close exactly that gap: four 40px floats
  inside an unsized `<div>` wrapper inside a 150px floated parent. Landed
  as a "probe" commit, pushed to CI ahead of any fix — **it passed
  immediately**, with no production code changes beyond the font-
  shorthand cherry-pick. That is the direct evidence the wrapper shape
  was never actually broken on `main`.
- The real fixture, rendered by the actual CI-built `stele-host` binary
  (`stele --headless --dump-png fixtures/css1-float-5526c.html`,
  reproduced locally byte-for-byte from the same artifact) and analyzed
  programmatically (PIL/`scipy.ndimage` connected-component labeling,
  never eyeballed): the yellow cards form TWO separate components on row
  1 (`x ≈ 21.2-26.1%` and `45.5-50.4%` of the 800px canvas, `y ≈ 12.8-
  33.1%`) plus a third on row 2 (`x ≈ 35.6-41.8%`, `y ≈ 34.6-57.4%`) —
  not one merged band. Re-expressed relative to the page's own non-
  background bounding box (matching how the reference's proportions were
  measured against its 531px window rather than Stele's fixed 800px
  `--dump-png` viewport), the top row spans `x ≈ 32.4%-81.0%` versus the
  W3C reference GIF's own measured `x ≈ 27.5%-89.5%` for the same row —
  the same shape (two cards near the row's outer edges, a black card
  between them), close in extent, not pixel-identical (expected: font
  metrics, `dt`'s content-box em-scale, and viewport width all differ
  from the reference's own rendering environment, all explicitly excused
  by the test itself). Row order/content from `--dump-text` matches the
  reference exactly: row 1 = "the way" / "the world ends, bang(), whimper()"
  / "i grow old"; row 2 = "pluot?" / "bar maids," / "sing to me, erbarme
  dich".

### Conclusion

The nested-float collapse this packet was scoped to fix does not exist
on current `main` — it was already closed by `packet/block-floats`
(`88c404c`) re-enabling and correctly wiring taffy 0.13's own
`float_layout` block algorithm, which (per §1 above) already generalizes
correctly to a float nested beneath a plain wrapper inside another float,
with no Stele-side gap (per §2) and no taffy-side gap for this shape
(per §3's new passing test). This packet's actual, necessary change is
narrower than originally scoped: re-apply the `font` shorthand fix
(`a15555c`, required — without it every `em` in the fixture, including
its own float widths, is 1.6x too large against the fixed 800px
`--dump-png` viewport) and bless `goldens/css1-float-5526c.png` off the
now-coherent render, with the pixel evidence above in place of a rewrite
of the block-level float placement engine. The `content-box` sizing
change from the prior `packet/acid1-coherence` attempt (`b88f9cd`) is
deliberately NOT part of this packet — it regressed this exact fixture
(columns stacked vertically instead of side by side) when tried before,
and nothing in this packet's evidence trail shows it's needed.
