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
