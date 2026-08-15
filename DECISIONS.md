# DECISIONS

Forks taken while the operator was away. Each: options, choice, why,
revisit-trigger. Newest first.

## M5 — External `<link>` CSS

### D35 — external stylesheets fetched driver-level, unified doc order with `<style>`
`<link rel=stylesheet>` was ignored. **Choice:** a new driver-level
`stylesheets::collect_all_author_sheets` (not `style::author`, mirroring
`images.rs`, since it does I/O) does ONE document-order DOM walk emitting both
`<link>` (fetched) and `<style>` sheets interleaved — so cascade source order
is correct across the two. Fetches against the doc's post-redirect `final_url`;
`rel` case-insensitive multi-token; `<link media>` = whole-sheet viewport gate
(distinct from in-CSS `@media`); `MAX_LINKS=32`; any failure skips that sheet;
`@import` inside a fetched sheet stays ignored (no HTML recursion, no cycle).
Applies on BOTH tty and png paths (author CSS affects `display` etc.). Frames
resolve `<link>` against their own frame url. Revisit: `--stats` still counts
only `<style>`-sourced ignored declarations (documented undercount); `@import`;
`rel="alternate stylesheet"` treated as a plain stylesheet (simplification).

## Rendering — border-collapse + tty table grids

### D50 — border-collapse model; tty draws box-drawing table grids
Freeze amendment: `ComputedStyle.border_collapse: BorderCollapse` (default
Separate). Collapse (from CSS, or a bare `<table border>` sans cellspacing):
spacing forced to 0 + box_tree dedups cells to top+left borders (table keeps
outer frame) → single grid lines in pixels. `<table border cellspacing=N>` opts
back to separate. The tty gained box-drawing table rendering (`─`/`│` for
`Display::Table`/`TableCell` bordered boxes; `<hr>`/non-table unchanged), plus a
4px default cell padding for bare `<table border>` so a tty separator column
always exists (no `Widget4` collision). **Why default-collapse for bare
`<table border>`:** it's what authors expect (clean grid) and fixes the
doubled-border complaint; explicit cellspacing or `border-collapse:separate`
overrides. **Known limitation:** collapsed-table tty box-drawing JUNCTIONS are
rough (`┌` where `│`/`┼` belong — top+left-only borders + naive corner logic);
readable but not pretty — proper junction resolution deferred. Full CSS
border-conflict resolution (differing adjacent borders) also deferred.

## Layout — Table cellpadding/cellspacing

### D49 — cellspacing is a border-spacing freeze amendment; cellpadding rides padding
`ComputedStyle` gains `border_spacing_x/y` (freeze amendment; defaults 8.0/0.0 =
the old `block::BORDER_SPACING_X/Y` constants, so existing tables are
byte-identical). CSS `border-spacing` + the `cellspacing` attr feed it; the table
solver (which already supported spacing) reads it off the table's own style.
`cellpadding` stamps `padding` onto cells in box_tree (mirroring the `<table
border>` stamp; author CSS wins; stops at nested tables) — no cell measure/emit
change needed because taffy border-box + the universal padding mapping already
honor a cell's padding. border-spacing resolved non-inherited (only a table's own
value is consulted). **Open (D50-to-be):** `border-collapse: collapse` for merged
grid lines, and the oversized 8px default border-spacing (a tty-cell constant
bleeding into the pixel backend; real HTML default is 2px).

## Rendering — <table border>

### D48 — <table border=N> stamps table + cell borders in box_tree
Post-cascade (like the `float` align hint; `border` is not inherited): a
`<table border=N>` gets an `Npx` solid gray outer border, and each descendant
`<td>`/`<th>` a `1px` solid gray border, via a DEPTH_CAP-bounded subtree walk
that stops at nested tables (inner `border` attr governs inner cells). Gated so
author CSS wins per-box. Done in box_tree (not the cascade) because it needs the
table→cell tree relationship, which `ElementInfo`/the cascade don't carry.
Renders in pixel/fb; tty draws no 4-side borders (v0). Follow-ups (D49):
`cellpadding`/`cellspacing`.

## Style — Presentational attributes

### D47 — HTML presentational attributes fold into the cascade as a middle tier
`resolve_declarations` gained a three-tier precedence: **UA < presentational-hint
< author** (`tier: u8` replacing `is_author: bool`; inline `style=""` still
highest). `value::presentational_hints(tag, attrs)` maps `bgcolor`→background-color,
`<font color>`→color, `<font size>`→font-size (HTML4 1–7 scale + `±N`),
block `align=`→text-align, `<body text>`→color. **Why a cascade tier, not
post-cascade mutation** (the `apply_align_float_hint` style): inherited
properties (`color`) demand it — a `<font color>` inside an already-colored
ancestor must override the inherited value, which only works if the hint
participates in the cascade/inheritance rather than checking "is the computed
value still default?" after the fact. Hints correctly lose to author CSS + inline
style, beat UA. `<img align>` stays float-only (box_tree, unchanged). Frozen
types untouched. Deferred: `<body link/vlink/alink>`, `<table border>`,
cellpadding/cellspacing, `<td width/valign/nowrap>`, `<font face>`.

## Rendering — <hr> horizontal rule

### D46 — <hr> is a top-border rule; tty draws a rule only for a sole-top border
`<hr>` gets `border-top:1px solid #808080; height:0` in the UA sheet (new
`border-top` longhand parse + top-only cascade override). The pixel/fb backend
draws it via the existing per-edge border painter (no change). The tty backend
gains its FIRST border rendering: a `─` line across a box's top row — gated to
fire ONLY when the top is the box's sole solid border. **Why the gate:** the tty
has no vertical/bottom border support, so drawing a lone top `─` over a full
4-side-bordered table cell or flex box looks broken; restricting to sole-top
borders means only genuine rules/separators (`<hr>`, decorative `border-top`
divs) render, and full-bordered boxes stay clean in tty exactly as before.
**Rebless:** kitchen-sink (already contained an `<hr>`) — rule now visible + 1px
geometry shift; table/flex unchanged. Frozen types untouched. Bounded/total.

## Layout — text-align

### D45 — text-align applied in the inline engine; <center> maps to it
`inline.rs` offsets each finalized line by an alignment delta derived from the
containing block's inherited `text_align` (Center = half the free space, Right =
all of it, clamped ≥ 0, added on top of float-exclusion offset). UA sheet maps
the presentational `<center>` element to `text-align: center`. Justify is
treated as Left in v0. **Why:** real hand-authored/vintage pages center headers
and nav via `<center>`/`text-align` (68k.news); ignoring it rendered everything
flush-left. **Invariants:** `max_width` and line content-width are unchanged, so
shrink-to-fit and flex item sizing are unaffected; left-aligned content (the
default) is byte-identical. **Rebless:** flex-polite.png footer now centers
(the fixture always declared `text-align:center`; the golden had encoded the old
ignored behavior). Attribute form (`align="center"` on block elements) not yet
mapped — follow-up.

## Layout — Block-in-inline

### D44 — an inline box containing a block descendant is not folded into an IFC
`block.rs` gains `contains_block_descendant(n, depth)` (DEPTH_CAP-bounded);
`is_inline_ish` requires `display == Inline && !contains_block_descendant(n,0)`.
So a `display:inline` element (`<font>`, `<b>`, ...) that wraps a block
(`<ol>`, `<li>`, `<div>`, `<table>`) is translated as its own box instead of
being flattened into an inline formatting context — the normal container
partitioning then splits inline runs from block children (anonymous-block
behavior it already had for direct mixed children). **Why:** real vintage/hand
HTML nests block lists inside `<font>` (68k.news), and the old fold erased all
block structure → run-on text. **Scope/limits:** this is the pragmatic
block-in-inline resolution — the whole inline wrapper blockifies rather than
CSS's precise "split the inline into before/after fragments around the block";
visually equivalent for the common wrapper case. `Text`/`Replaced` never count
as block, preserving D14 inline-replaced (`<em><img></em>`). Depth-capped for
totality (degrades to `false` past DEPTH_CAP, same as `flatten_inline`).

## UI — Wrapped-link highlight

### D43 — highlight a focusable's actual per-line cells, not its bounding box
`Focusable` carries `cell_spans` (one cell-rect per contributing fragment);
`render_frame` paints the focus highlight over each span. `rect_cells` remains
the union, still used for `hit_test` and first-visible-focus ordering (a
bounding box is fine there — clicking anywhere in a wrapped link's box still
follows it). **Why:** unioning per-line fragments into one rectangle and
filling it lit up cells the link never occupied (the "whole paragraph selected"
report), especially once block-in-inline lists (D44) made links wrap many
lines. Internal to browser.rs; no frozen change.

## UI — Editable forms + responsive resize

### D41 — text-input editing lives in ViewState; submit overrides DOM values by NodeId
Focusing a plain `<input>` (`is_text_input_kind`: text/search/email/url/tel/
password/number + empty type; excludes submit/button/checkbox/radio/hidden/
select/textarea) routes keys to `apply_key_editing`. Edit buffers are kept in
`ViewState.fields` (focusable-idx → String) with a char-offset cursor, seeded
lazily from the DOM default value, persisted across focus moves, capped at 4096
chars. Submit reuses the existing `Command::Submit(Request)` +
`form::encode_www_form` path via a new `serialize_submit_with_overrides` that
swaps in typed buffers for edited fields (matched by `control_node` NodeId), so
a whole form submits what the user typed — not just the triggering field.
**Why in ViewState, not the DOM:** keeps `apply_key` pure/total and the DOM
immutable; the shell owns transient UI state. **Trade-offs (v0):** GET only
(POST path preserved, unused here); `select`/`textarea` non-editable; Enter
doesn't activate a named submit button (activator None) so that button's
name=value isn't included on the type-then-Enter path. Follow-ups.

### D42 — responsive resize via poll timeout + dirty-gated redraw (no signals)
`run_browser`'s `poll` uses a 250ms `Timespec` timeout instead of blocking
forever; the loop re-queries `tcgetwinsize` each tick and rebuilds page +
`clamp_scroll` only when the size changed, so a resize reflows within ~250ms
with no keypress — no `SIGWINCH` handler, no `unsafe` (the brief's constraint).
A `dirty` flag (set on real input or a detected resize) gates the clear+redraw,
so idle timeout ticks do NOT flicker or busy-spin. **Why polling not signals:**
signal handlers need `unsafe`/a self-pipe and fight the totality discipline; a
cheap 4×/sec ioctl poll is simpler and total. **Cost:** resize triggers a full
`load_page` (re-fetch) at the new width — acceptable since resize is rare;
re-layout-without-refetch is a follow-up. `clamp_scroll` is unit-tested; the
loop itself is the manually-pty-verified thin half.

## UI — tty readability (contrast)

### D40 — tty defers to the terminal theme by default and guarantees legibility
The tty backend no longer dumps raw CSS colors onto the terminal. `to_ansi`
routes each cell through `resolve_cell_colors(fg, bg)`:
- **No author background** (`bg.a==0`): emit terminal-default bg (49); emit
  terminal-default fg (39) for unset OR near-black/near-white foregrounds
  (they'd vanish on a same-theme terminal); pass through chromatic mid-tones.
- **Author background** (`bg.a!=0`): emit it, and force the foreground legible —
  keep the author fg only at WCAG contrast ≥ 4.5:1, else black/white by bg
  luminance.
**Why:** the web assumes a white canvas we don't have; honoring author text
colors verbatim produced black-on-black on dark terminals (unstyled pages,
errors, httpforever). This makes the browser default to the user's own terminal
theme (always readable) while defending against pages that specify illegible
color pairs — same "protect the user from unfriendly pages" stance as
`--no-bg-images` (D39). **Tradeoff:** luminance is non-gamma (as specified); it
over-forces contrast (safe — only costs author-gray fidelity, e.g. `#333` on
`#eee` snaps to black). Follow-up: gamma-corrected luminance with retuned
near-black/near-white thresholds so author grays pass through. Pixel/fb
backends are unaffected (they paint a real canvas). Regression guard: golden
`tty-color.ansi` asserts the emitted SGR, not just characters.

## UI — Background-image

### D39 — background-image tiled in pixel backends; one frozen field + side-map
`background-image: url(...)` + the `background` shorthand url now render.
**Freeze amendment:** one field, `ComputedStyle.background_image:
Option<Box<str>>` (raw url; not inherited — resolved like `background_color`).
The decoded image reaches the painter via a driver-level SIDE-MAP
(`bg_images::collect_bg_images -> HashMap<raw-url, Rc<RgbaImage>>`, mirroring
`images.rs`), NOT another fragment/LayoutNode amendment — `raster::paint` takes
a `bg_images` param (impl sig). **Bounded:** dedup by resolved url,
`MAX_BG_IMAGES=32` + the shared aggregate-byte budget; decode failure → falls
back to background-color. **Reasonable, not fussy:** repeat-only, top-left
origin, no `background-size`/`-position`/other `-repeat` variants (deferred);
tiling is a bespoke `put_pixel` loop clipped to the surface (a hostile box
can't blow up). **Pixel-only** (`--dump-png`/`--render-fb`); tty shows
background-color via ANSI, not images. **`--no-bg-images`** flag (default OFF =
on) skips the pre-pass → color-only render. Revisit: background-size/position,
non-repeat, bg-image in the fb-interactive shell.

## UI — Mouse (gpm + xterm)

### D38 — first-class mouse via bespoke gpm socket + xterm SGR
Mouse works on a bare Linux console (the 486 target, via **gpm**) and in
terminal emulators (xterm SGR). **gpm choice:** speak the protocol directly
over a `std` `UnixStream` to `/dev/gpmctl` — NO libgpm, no FFI, no `unsafe`
(charter "own the wire" ethos). Exact wire: `Gpm_Connect` 16B
(eventMask/defaultMask/minMod/maxMod u16 + pid/vc i32), `Gpm_Event` 28B
(buttons/modifiers u8, vc u16, dx/dy/x/y i16, type/clicks/margin i32, wdx/wdy
i16) — LE, size-checked. VC from `/proc/self/fd/0` (`/dev/ttyN`→N, else `0`
best-effort). **Auto-detect:** gpm if `/dev/gpmctl` connects, else enable xterm
SGR (`\e[?1000h\e[?1006h`), never both; restored on quit. **Behavior:** left
click → `hit_test`(viewport→page via scroll) → `enter_command` (follow/submit,
same as Enter); wheel = 3 lines; right/middle/release/move = no-op. rustix
`"event"` feature added for `poll(2)`; cross-builds + runs on i486 (CI). Pure
parsing/`apply_mouse` unit-tested; the socket+poll loop is manual (no gpm in
CI). Revisit: VC fallback if a real gpmd rejects `vc:0`; hover-highlight;
form-field editing.

## UI — Interactive shell (keyboard)

### D37 — the interactive terminal browser; rustix for raw mode
`stele <url>` (bare positional, no `--headless`) launches an interactive
terminal UI. **Split:** all logic is pure + unit-tested in `browser.rs`
(focusables, `hit_test`, key parsing, `apply_key`, scroll/focus/reveal,
history, `render_frame`); only `main::run_browser` (raw-mode enter/restore,
terminal size, the blocking read→draw loop) is manual (no tty in CI —
pty-verified). **Model:** arrows/PgUp-PgDn SCROLL (not element-nav);
Tab/Shift-Tab focus with wrap + auto-scroll-to-reveal; focus-on-load = first
visible focusable; Enter follows a link (`Url::resolve`) or submits a
submit/image control (`form::serialize_submit`, default values — text/checkbox
editing deferred); Backspace back (no forward stack, v0); F5/`r` reload;
`q`/Ctrl-C quit; yellow fixed highlight. **Dep:** `rustix` (charter substrate),
`default-features=false` + `["termios","std"]` — linux_raw backend, no `unsafe`
in our code; cross-compiles + runs on i486 (CI-verified). Raw mode clears
ICANON|ECHO|ISIG (Ctrl-C as a byte) but keeps OPOST. Restore runs on normal
quit; `panic=abort` skips it on a genuine panic, so the loop is written total.
Known edge: two adjacent links with identical `href` merge into one focusable;
Fragment carries no NodeId, so control→form resolution is a best-effort DOM
walk that degrades to no-op. Revisit: (c2) mouse; form-field editing.

## UI — Colored tty render

### D36 — TextGrid carries fg/bg; to_ansi() for color, to_text() unchanged
The interactive shell needs a colored terminal render, and `<body>`/region
backgrounds should show in tty (previously D17: `Box` painted nothing there).
**Choice:** `TextGrid`'s cell became `Cell { ch, fg, bg }` (pub(crate); default
fg black, bg transparent). `Box` fills `background_color` into its cells; text
sets `fg` from `style.color` and inherits the box `bg` (paint order draws Box
before its Text). A NEW `to_ansi()` emits 24-bit SGR, run-length-optimized (one
escape per color change, reset per line); the EXISTING `to_text()` is unchanged
(chars only) so every blessed tty golden stays byte-identical — color is purely
additive. Borders still unrendered in tty (deferred). Also: the CSS
`background` shorthand now yields `background_color` (first color-shaped token,
url()-contents skipped; no-color → unapplied per C2). Revisit: ASCII box-drawing
borders; the shell consumes `to_ansi`.

## UI — Interactive provenance

### D34 — Fragment/LayoutNode carry interactive provenance (link href, controls)
The interactive shell must know, per rendered region, if it's a link (and its
`href`) or a form control — without re-deriving from the DOM at paint time.
**Choice:** a freeze amendment adding `Interactive { Link{href} |
FormControl{kind,name,form_action} }` + an `interactive: Option<Interactive>`
field on both `Fragment` and `LayoutNode` (and, as a non-frozen carrier,
`inline::InlineRun`, since wrapped link text folds into one taffy leaf). Raw
unresolved `href` (resolution is the shell's job). Zero behavior change —
painters ignore it, all goldens byte-identical. Revisit: the shell reads
`Fragment.interactive` for hit-testing/highlight/follow/submit.

### D34b — parallel packets MUST use worktree isolation
Two subagents were dispatched into the SHARED working tree concurrently (this
amendment + `<link>` CSS); they raced on `src/layout/*` and `frames.rs`. Both
recovered (the second self-isolated into a git worktree), but the rule is now
explicit: **any parallel packet work uses `isolation: worktree`** so subagents
never share an editable tree. (They also both touched `frames.rs`, forcing a
serial merge + rebase.)

## M6 — List markers

### D33 — list-item markers synthesized in box_tree (ASCII bullets)
`<ul>/<ol>/<li>` had no markers. `box_tree` prepends a synthesized marker
`Text` per `<li>` (the details/form-placeholder stand-in convention), keyed on
the nearest `<ul>`/`<ol>` parent. **ASCII glyphs** (`* `/`o `/`# ` for
disc/circle/square, `N. ` decimal, base-26 alpha) — not Unicode `•`, because
the bitmap font is ASCII-only so a Unicode bullet would be a tofu box in the
PNG; ASCII keeps both PNG and tty outputs legible and per-variant-distinct.
`list-style-type: none` → no marker; `<ol start>` honored; nested lists count
per-list (restart at 1); `display:none` `<li>` doesn't consume an ordinal.
`lower/upper-roman` absent from the frozen `ListStyleType` enum (not deferred —
not in the dialect). Revisit: Unicode bullet glyphs if the font ever gains
non-ASCII coverage; roman numerals if the enum is amended.

## M6 — Hardening core

### D32 — kitchen-sink coverage, mutation fuzz, `<br>` via sentinel, A2 hard gate
Release hardening. `kitchen-sink.html` exercises the whole dialect in one page
(A5 coverage golden, blessed). A hand-rolled deterministic mutation fuzz
(`tests/fuzz_totality.rs`, seeded xorshift64*, ~4400 iters over HTML-mutation /
random-blob / random-CSS / image-decode) drives the full pipeline asserting no
panic — `cargo-fuzz` isn't in the image, so hand-rolled per brief §6. It found
exactly one bug (`<br>` a no-op) and nothing else. **`<br>` fix:** `box_tree`
rides a `LINE_BREAK_SENTINEL` (U+E000 PUA) inside the frozen `BoxContent::Text`,
recognized by `inline` as a forced break — avoids a freeze amendment; tradeoff:
literal U+E000 in real content misrenders as a break (cosmetic, never a panic);
a `BoxContent::LineBreak` amendment is the clean alternative. A2 (2MB size) is
now a hard gate (met with huge headroom, ~542KB). Known pre-existing gaps
(documented, not regressions): list-item markers unimplemented; `<pre>`
whitespace collapses. Revisit: list markers (next), the `<br>` sentinel→variant
if it ever bites, the A5 speed budget (brief defines A5 as <50M instrs/<150ms —
this packet's checks are coverage, not speed; speed instrumentation is a
separate M6 item).

## M5 — Dialect completeness

### D31 — details/summary collapse, noscript shown, entities verified, --stats
Curated-dialect finish, all in impl (no frozen-type change):
- **`<details>`**: `open` absent → collapsed, only the first direct-child
  `<summary>` built (rest dropped from the box tree, not just visually hidden);
  `open` present → expanded. No `<summary>` → synthesized `"Details"` label.
  Disclosure marker `> `/`v ` (ASCII, deterministic) glued as a synthetic
  leading Text box (same mechanism as form-control placeholders). Interactivity
  (click-toggle) is out of scope (no interactive shell).
- **`<noscript>`**: UA `display:block` (was accidentally-visible via the
  CSS-initial `inline`; now intentional). No-JS by construction → its content
  always renders.
- **entities**: all HTML 4.01 named + decimal + hex numeric decode correctly
  (no P1 bug); unknown passes through literally. Known frozen-layout gap: NBSP
  (U+00A0) decodes right but the inline tokenizer collapses it to a space
  (`char::is_whitespace` includes NBSP) — flagged for a later inline tweak, not
  forced in this packet's scope.
- **`--stats`**: stderr-only `N ignored declaration(s), N ignored at-rule(s),
  N media block(s)` aggregated across author sheets, computed in an independent
  fetch+parse pass so `dump_text`/`dump_png` stdout can never be perturbed.
  Surfaces charter C2 (ignore-unknown, but COUNT).
Revisit: cookie-jar file persistence (C6) still to wire (HTTP-only); the NBSP
inline behavior; details keyboard/click toggle when an interactive shell exists.

## M5 — @media

### D30 — @media evaluated in a viewport pre-pass (no frozen-signature change)
`@media` blocks were parsed-then-discarded (only `media_at_rules` counted).
**Choice:** the parser now STORES them (`Stylesheet.media_rules`: condition +
nested rules, sharing the global rule `order` counter), and a `flatten_media(
sheet, viewport)` pre-pass evaluates each query against the render viewport,
producing a media-free `Stylesheet` that `cascade` consumes unchanged — so the
frozen `parse`/`cascade` signatures stay put (`cascade` diff is literally
zero). Sharing one `order` counter across top-level and `@media`-body rules
makes a matching block cascade exactly as if written inline (later `@media`
beats earlier equal-specificity), no positional fixups. **Supported subset:**
media types `all`/`screen`, width features `(min-width|max-width|width: px)`,
`and`, comma=OR; `print`, `not`, unknown features/types, and malformed
conditions **fail closed** (never match), never panic. **Viewport width** per
render mode: tty `cols*8`, png 800, fb device width, frames per-region.
`min/max-height` out of scope (all modes lay out at content-driven unbounded
height). Revisit: `<link>` external CSS (still deferred), height/orientation
features, `!important`.

## M5 — flex-polite (flexbox pixel-green)

### D29 — flexbox renders to pixels; whitespace-only text is not a flex item
Flexbox already laid out (taffy `apply_flex`) but had never been styled +
rendered; `flex-polite.html` (a modern no-JS blog layout via an author
`<style>` block) proves it pixel-green. One real bug fixed: per CSS Flexbox §4,
a flex container child that is a `Text` node of only collapsible whitespace
(ubiquitous from source newlines/indentation between flex children) must NOT
generate an anonymous flex item — `translate_container_children`'s flex branch
was making one, doubling `gap`. Fixed with an `is_whitespace_only_text` skip
(only in the flex branch; block flow is unaffected). The golden test renders
through the REAL author-CSS pipeline (`collect_author_sheets` + `cascade(dom,
&sheets)`, matching `main::dump_png`), not the legacy hardcoded
`cascade(dom, &[])` helper — so flex CSS actually applies; a companion test
asserts flex is load-bearing. Revisit: meaningful (non-whitespace) text in a
flex container still correctly becomes an anonymous flex item.

## M5 — Author CSS

### D28 — author `<style>` + inline `style=` wired into the cascade
The render pipeline had always called `cascade(dom, &[])` and ignored inline
`style=`, so no page CSS applied. **Choices:** `<style>` blocks are collected
(`style::collect_author_sheets`, a new module, explicit-stack DOM walk) into
author `Stylesheet`s in document order and passed to the existing (frozen)
`cascade` signature; inline `style="..."` is modeled as a `Declarations`
overlay applied LAST in `visit` per element — the highest-precedence origin
(inline beats any non-`!important` author/UA rule regardless of selector
specificity), read straight off the `Element` so `cascade`'s signature stays
frozen. Reuses the already-total P2 parser (`parse_inline` is a thin
`parse_declaration_block` wrapper); malformed CSS is ignored (C2), never a
panic. **`<link rel=stylesheet>` external CSS is deferred** — it needs a fetch
pre-pass analogous to `images::collect_images`; noted for a follow-up.
Revisit-trigger: a fixture needs external stylesheets, `!important`, or
per-element ignored-declaration reporting (the `--stats` counter, M5).

## M4 — fbdev backend

### D27 — framebuffer via sysfs + file write (safe, std-only, no unsafe/deps)
The real hardware output path deliberately avoids `rustix`/ioctl/mmap (which
require `unsafe`). **Choice:** read fb geometry from **sysfs** text files
(`/sys/class/graphics/fb0/{virtual_size,bits_per_pixel,stride}`) and write
pixels to `/dev/fb0` via a plain `std::fs::File` — no `unsafe`, no new deps,
one syscall-per-file instead of an ioctl (irrelevant for a one-shot renderer).
A pure `convert_to_fb_bytes` (mem-Surface RGBA8 → fb layout) is the testable
core: **32bpp = BGRX8888** little-endian (`[B,G,R,X]`, X=0, no device alpha),
**16bpp = RGB565** little-endian, other bpp → `Err`. Totality: `height*stride`
computed in `u64` and range-checked into `usize` (`GeometryTooLarge` guards the
32-bit i486 target); columns clipped by `stride/bpp` so writes can't overflow a
row; a surface larger than the fb clips, smaller leaves the rest black
(one-shot, no prior frame); absent/garbage `/dev/fb0`/sysfs → clean `Err`, never
a panic (this error path is what CI actually exercises, since no framebuffer
exists on the runner). Default width fallback `1024` when sysfs is unreadable.
Revisit: exact RGB bitfield offsets would need `FBIOGET_VSCREENINFO` (ioctl) —
if a device with a non-standard channel layout ever matters, add it then
(would introduce localized `unsafe`); the standard 16/32bpp assumption covers
vesafb/simplefb/efifb, i.e. essentially all real fbcon setups.

## M4 — Floats + inline images

### D26 — bespoke float layout + inline replaced atoms (closes D14)
Taffy has no floats, so `img align=left`/`right` and inline `<img>` are
hand-rolled in the inline engine (charter §158). Choices:
- **`<img align>` → `float` presentational hint** in box_tree, applied only
  when the cascaded `float` is still `None` (author CSS wins); `left`/`right`
  only (`top/middle/bottom` are vertical-align, ignored).
- **Inline replaced atom:** a non-floated `Replaced` occupies its intrinsic
  size on the line and wraps like an unbreakable word (baseline convention:
  ascent=height, descent=0). Closes D14's "grandchild `Replaced` dropped".
- **Float model (scoped):** a floated `Replaced` is placed at the block's
  left/right edge at the IFC top (y=0, NOT the true source line-y — a
  documented simplification good for the `<p><img align=left>text…</p>` shape);
  same-side floats stack, oversized floats clamp to full width; `layout_runs`
  excludes `[float.y, float.y+h)` from overlapping lines (offset + shortened for
  left, shortened for right) so text wraps, returning to full width below.
  **Cross-block float continuation is deferred**, and `clear` is a no-op in
  this scope (floats never escape their own IFC). Revisit at a later CSS pass
  if a fixture needs multi-block floats / real `clear`.
- **Totality:** the line-breaker is a single bounded `for` over finite clusters
  (no `while`/retry → structurally no float-spin hang); `MAX_FLOATS=256`,
  `MAX_DIM=1e6` clamp every dimension (width AND the atom paint height — the
  latter added as defense-in-depth so nothing downstream inherits an unclamped
  value even though frozen `blit` already clips).

## M4 — Images

### D25 — image pipeline: Replaced carries the decoded image; bounded fetch/decode
`<img>` rendering wires the P4 decoders into the render path. Choices:
- **Freeze amendment:** `BoxContent::Replaced` gains `image: Option<Rc<RgbaImage>>`
  — the seam so a decoded image reaches the frozen `FragmentKind::Image`. `Rc`
  (single-threaded) so `LayoutNode` clones during layout don't copy pixel
  buffers; `block::emit` clones the inner `RgbaImage` once into the fragment
  (forced by the frozen by-value `Image { image }`). `None` → placeholder, no
  behavior change (`basic.png` byte-identical).
- **Driver-level pre-pass with I/O:** `images::collect_images(dom, final_url)`
  fetches+decodes each `<img src>` (frame 0 of animated GIF) — decode gated to
  the `--dump-png` path (tty passes an empty map, no fetch). Base is the
  document's FINAL (post-redirect) URL, not the request URL.
- **Bounded against image bombs:** `MAX_IMAGES=256` (count) + dedup decodes by
  resolved URL (repeated `src` decodes once, shared `Rc`) + `MAX_TOTAL_IMAGE_BYTES
  =256MiB` aggregate-resident budget (halt decoding past it → placeholders).
  Together these bound both the same-src-×N and distinct-huge-images attacks
  (the P4 `MAX_DECODE_PIXELS` bounds each single decode). Every fetch/decode
  failure → skip → placeholder, never a panic. `blit` is fully clipped (i64
  edge math, bounds-checked source reads, zero-size guards). Revisit: animated
  GIF ticking (interactive), image caching across documents, real image-button
  coordinates — all later.

## M4 — Pixel foundation

### D24 — embedded PD bitmap font; raster painter; bounded pixel paths
The fb-render path (P5/P9 glyph-raster deferral coming due). Choices:
- **Font:** embedded `font8x8_basic` (128 ASCII glyphs, **public domain**,
  from github.com/dhepper/font8x8) as a compiled-in atlas — the brief's "embed
  one bitmap-friendly font" over shipping a TTF. 8×8 glyphs drawn
  baseline-aligned within the 16px `vga_8x16` line box; non-ASCII → a tofu
  fallback box (never a panic). License noted in REPORT.md. Revisit: a fixture
  needs Latin-1/box-drawing beyond ASCII → embed a fuller PD page.
- **Pixel paths are bounded** (`panic=abort` + hostile input): `draw_glyph`
  clamps one glyph to `MAX_GLYPH_PX=1024` per axis AND early-returns O(1) when
  the glyph's screen bbox doesn't intersect the surface (else a long document
  at a large `font-size` is `O(chars·1024²)` wasted work on off-screen glyphs).
  `--dump-png` fixes width at 800px and clamps height to `MAX_PNG_HEIGHT=20_000`
  (64MB surface ceiling — the `tty::MAX_GRID_ROWS` analog). All raster
  coordinates clamped to ±1e6 before reaching the `Surface`.
- **Golden discipline for PIXELS:** PNG goldens can't be eyeballed by a
  text-only reviewer, so the ORCHESTRATOR views the rendered PNG and
  countersigns it visually; the golden test decodes both PNGs and compares RGBA
  arrays (robust to encoder metadata), not raw bytes. `MemSurface::blit` stays
  `todo!()` until the images packet (never called for image-free fixtures).

## M3 — Frames

### D23 — framesets as a driver-level recursive render; bounded against frame bombs
`<frameset>` support lives ABOVE the single-document pipeline (`src/frames.rs` +
main.rs routing), reusing `layout()`/`tty::render` per frame — no frozen-type
change. Choices:
- **`<frame>` is void.** Added `"frame"` to the parser's `VOID_ELEMENTS`
  (implementation, not frozen `ast.rs`): real 1996 framesets write
  `<frame src=x>` with no end tag, and without this they mis-nest into a
  single cell. `<frameset>` stays a container.
- **Track sizing:** fixed-px tracks first, then percentages of the remainder,
  then star (`*`/`2*`) split the rest by weight; absent attr = one 100% track;
  malformed → `1*`. Cell units reuse the tty 8×16 cell. Column width is
  load-bearing (bounds wrapping); a frame taller than its row track CLIPS at
  the boundary (faithful to a frame's fixed scrollable viewport), rather than
  growing the row.
- **Incremental compositing:** the viewport canvas is sized from track math
  alone, then each cell is rendered → blitted → dropped, so peak memory is the
  bounded canvas + one in-flight child, not all `MAX_TOTAL_FRAMES` grids at
  once.
- **Frame-bomb bounds:** `MAX_FRAME_DEPTH=6`, `MAX_TOTAL_FRAMES=128` (a single
  global budget threaded by `&mut` through the recursion), `MAX_TRACKS_PER_DIMENSION=32`,
  `MAX_TRACK_VALUE` clamp, and a same-URL-on-fetch-path cycle check. The cycle
  check compares `Url::as_str()` without percent-decoding or case-folding, so
  `a%2ehtml`/`A.html` could evade it — but `MAX_FRAME_DEPTH` is an
  UNCONDITIONAL backstop, so no evasion can produce a hang, only bounded
  redundant work. Revisit: wire real URL normalization into `fetch::Url` (a
  broader concern) if a fixture needs it.
- **`<noframes>` never rendered** (Stele renders real frames); frame
  `scrolling`/`noresize`/`marginwidth`/`frameborder` ignored (v0). No
  separator/border drawn between frames (same reasoning as `Box` fragments
  painting nothing in tty).

## M3 — Forms

### D22 — form submit as a pure serializer; controls render as tty text placeholders
`form::serialize_submit` implements HTML 4.01 §17.13.2 as a PURE function (no
event wiring — a no-JS browser never runs `onsubmit`; a future interactive
shell calls this on the activating control). Form controls render as
synthesized TEXT (not empty boxes, which are invisible in the tty backend per
D17). v0 choices:
- **Blank text field filler is `_`, not spaces** — the inline engine collapses
  whitespace runs unconditionally in v1, so a space-padded field would collapse
  to one char. Placeholder glyphs: `[value]`/`[____]` inputs (password masked
  `*`), `[x]`/`[ ]` checkbox, `(*)`/`( )` radio, `[ label ]` buttons,
  `[ text v]` select, textarea first line truncated. Controls get
  `display:inline` (UA sheet) so they flow after their labels.
- **`type=file` → filename only, no multipart**; **`type=image` → plain named
  submit** (no click coordinates exist in a no-mouse/no-JS browser — treated
  like a plain submit when it's the activator, documented rather than faking
  `.x`/`.y`); **checkbox/radio absent `value` → `on`** (universal browser
  practice). `<select multiple>` contributes one pair per selected option;
  single-select contributes the selected (else first) option. GET replaces the
  action's query; POST sends the urlencoded body. Self-rolled percent-encoding
  (space→`+`, unreserved kept, else `%XX`). Revisit: a fixture needs multipart
  file upload (needs an interactive file picker — out of scope for the document
  web) or real image-button coordinates (needs a pointer).
- **Shared DOM-walk helpers live in crate-private `dom_util`** (one
  `DEPTH_CAP=100`, OOB-guarded `node_checked`), used by both `form` and
  `layout::box_tree`, so the two can't drift.

## M3 — Table layout

### D21 — tables as a bespoke measure-leaf; bounded + cached per-cell measurement
`Display::Table` subtrees are laid out by `layout::table_layout` + a new
`layout::block` measure-leaf (parallel to the inline engine): grid
auto-placement → taffy intrinsic-size measurement of each cell's
min/max-content + height → `solve_table` → cells painted at solved rects.
Forks taken:
- **Cost is bounded on hostile input.** Each cell costs several taffy
  sub-layouts (min-content + max-content + height, re-derived in `emit`), which
  the 262K *placement* cap (D18/`solve_table`) never accounted for — a wide
  table (tens of thousands of `<td>`s) would hang for minutes. **Choice:** a
  dedicated `MAX_TABLE_MEASURED_CELLS = 2_000` checked cheaply (via
  `place_grid`'s cell count) BEFORE any measurement; over-cap tables degrade
  to plain block layout (cells as stacked blocks — total, cheap), same as an
  over-`TABLE_DEPTH_CAP` table. Plus a `RefCell` cache (keyed on
  `available_width`, since column resolution depends on it) so `emit` reuses
  measure's solved layout instead of recomputing (~7×→~3× per cell). Real 1996
  tables (hundreds of cells) are never affected. Revisit: raise the cap or add
  a `table-layout: fixed` fast path if a real fixture needs a bigger table.
- **Nested-table depth cap `TABLE_DEPTH_CAP = 2`** (empirically: cost
  multiplies per nesting level — a budget of 8 caused a 15-min runaway). One
  table nested in a cell renders; deeper degrades to block. Revisit: never for
  the document web.
- **Border-spacing is a fixed constant** (8px h / 0px v), not CSS-driven —
  `ComputedStyle` has no `border-spacing` field (frozen). Revisit: a fixture
  needs `border-spacing`/`cellspacing` → freeze-amend a field.
- **M3 simplifications (deferred):** `table-layout: fixed`, `<caption>`/
  `<colgroup>`/`<col>`, `vertical-align` (cells top-align, not middle), and
  author `<style>` stylesheets (not yet collected into the cascade anywhere in
  the codebase — a separate gap, pre-existing). Revisit each at its milestone.

## M3 — Freeze amendment: table cell spans

### D20 — `BoxContent::TableCell` carries colspan/rowspan into LayoutNode
`colspan`/`rowspan` are HTML attributes on `<td>`/`<th>`, but `box_tree`
produces `LayoutNode`s that carry only `ComputedStyle` + `BoxContent` +
children — attributes are dropped. The table column solver (P8) needs the
spans to build the grid, and (unlike inline content) the layout engine can't
recover them. **Options:** (a) add a field to `LayoutNode` (breaks every
literal constructor across the tests); (b) add a `BoxContent` variant carrying
the spans. **Choice: (b)** — `BoxContent::TableCell { colspan: u16, rowspan:
u16 }`, additive, touched only by the code that builds/consumes cells (existing
`Container`/`Text`/`Replaced` literals unaffected). `box_tree` populates it for
`display: table-cell` nodes, defaulting missing/unparseable/zero to 1 and
clamping to HTML's limits (colspan ≤ 1000, rowspan ≤ 65534) so downstream holds
only sane values. Until the table-layout packet, a `TableCell` translates
exactly like a `Container` (stacked block) in all three `layout::block`
matches. Revisit: never — this is the stable carrier the table-layout packet
reads.

## M3 — Freeze amendment: CSS table Display values

### D19 — table display values land as a marker, block-fallback until integration
Real table layout needs the layout engine to RECOGNIZE table subtrees, but the
frozen `Display` enum had no table values and `LayoutNode` carries no table
marker. **Options:** (a) recognize tables purely by element tag in box_tree;
(b) add `display: table/table-row/table-cell` values (CSS-idiomatic, "element
semantics live in the UA sheet" per charter). **Choice: (b)** — a sanctioned,
additive freeze amendment (brief §10): `Display::{Table, TableRow, TableCell,
TableRowGroup}` appended (existing variants/order/initial-value unchanged), the
value parser + UA sheet wired. To keep the tree green with zero regressions,
`block::map_display` (the only exhaustive `Display` match) maps all four to
taffy `Block` for now — so a `<table>` currently renders as stacked block boxes
(visually wrong, but total and green). Explicit arms, no catch-all `_`, so the
real table-layout packet gets compiler-guided TODOs. Revisit: the next packet
wires `solve_table` (cell min/max measurement → column widths → fixed flex
bases) behind these markers. Note: `table` is left in the generic block
selector group and overridden to `display:table` by UA source order (equal
specificity, later wins) — standard cascade behavior, cascade-tested.

## P8 — Table column solver (Wave 2)

### D18 — standalone pre-placed table solver; bounded on hostile specs
`layout::table::solve_table` is the CSS 2.1 §17.5.2.2 auto-layout column/row
solver, built standalone (integration deferred to M3 + a freeze amendment).
Forks taken:
- **Pre-placed, not auto-flowing.** Cells arrive with explicit
  `col`/`row`/`colspan`/`rowspan` and already-measured `min_content`/
  `max_content`/`intrinsic_height`; the solver does NOT measure text or
  auto-flow cells into free slots (that's the box-tree caller's job at
  integration). Overlapping/out-of-range cells are skipped (→ `Rect::default()`,
  preserving 1:1 output correspondence with `spec.cells`), never a panic.
- **Bounded on hostile input.** Per-dimension `MAX_GRID_DIM=4096` AND a grid-
  area cap `MAX_GRID_CELLS=262_144`; placement uses an occupied-slot bitset
  (not an O(n²) scan) plus a `placement_budget` capping total slot-reads, so
  total work is O(grid area) regardless of `cells.len()` — a huge hostile
  `<table>` can't hang the process (not just can't panic). All float inputs
  sanitized (NaN/±inf/negative → 0); all float OUTPUTS scrubbed to finite too,
  so extreme magnitudes can't leak inf/NaN.
- **v1 algorithm choices (documented):** over-constrained (`available >=
  sum_max`) → columns at max, leftover NOT stretched/distributed; colspan
  spanning-excess distributed proportional to each column's `max_i` (even
  split if the max weights are zero), with a final `max_i >= min_i` clamp;
  rowspan excess split evenly across spanned rows (no row "max" analog);
  border-spacing between adjacent cells only (`n-1` gaps), no outer edge.
  Revisit at M3 integration if a fixture needs table-width stretch or the
  fixed-layout algorithm (`table-layout: fixed`).

## P7 — tty render pipeline (Wave 2 / M2)

### D17 — tty grid bounds + cell-mapping scope calls
The tty backend renders fragments into a character grid; forks taken to keep
it total and honest:
- **Grid allocation is bounded on BOTH axes.** `MAX_GRID_ROWS=10_000`
  (derived from layout) already bounded rows; `MAX_GRID_COLS=2_000` now bounds
  columns, which are directly user/attacker-controlled via `--cols` (a single
  flag drove a multi-GB allocation → capacity-overflow/OOM abort under
  `panic=abort`, on ANY document). Clamp happens first thing in `render`;
  `rows_needed==0` short-circuits before sizing a row (`vec![elem;n]` evaluates
  `elem` once regardless of `n`). Worst case grid = 2000×10000×4B = 80MB —
  bounded, and 2000 cols is far past any real terminal. Revisit: a fixture
  legitimately needs a wider grid (none should).
- **One column per char, ignoring per-glyph advance (known limitation,
  deferred).** `write_marker` advances one grid column per `char`, but
  `BitmapFont::advance` scales with font-size (h1@32px is 16px/char). A single
  uniformly-styled run alone on its line (every heading/para in basic.html) is
  fine; two `Text` fragments of different font-size sharing one line box would
  misalign. Inherent to mapping size-scaled layout onto a fixed 8px tty cell;
  documented in code, not fixed. Revisit: a fixture mixes inline font sizes on
  one line — then either lay tty out with a uniform cell metric or snap runs.
- **A3 acceptance runs under the pinned toolchain.** `accept.sh` A3 (tty-golden
  diff) is cargo-dependent; it runs in the CI `build` job (pinned
  `nightly-2026-07-15`) via `--tty-only`, with no `+nightly` override (charter
  C9), and degrades to PENDING where cargo is absent (the `accept` job) rather
  than failing. Goldens are blessed only via `accept.sh --bless` after an
  independent reviewer countersigns (brief §10) — an implementer never blesses
  their own.
## Hardening — recursion totality (cascade + parser)

### D15 — cascade made total by an explicit-stack rewrite, not a depth cap
`style::cascade`'s recursive `visit` had no depth bound and `SIGABRT`ed
(stack overflow) on a DOM nested ~3000 deep — reachable before layout, so
layout's `DEPTH_CAP` (D14) didn't protect it. **Options:** (a) reuse the
D14 depth-cap-and-degrade pattern (cap at 100, fill deeper nodes with a
degraded inherited/default style); (b) rewrite the walk iteratively with an
explicit heap stack. **Choice: (b).** Unlike layout — where taffy's own
recursion also has to be bounded, so a cap is unavoidable and >100-deep
boxes are genuinely pathological to render — cascade is pure tree walking we
fully own, so an explicit-stack (`Vec<Frame>`, `Enter`/`Exit`) rewrite
removes the crash with **zero correctness loss**: every node at any depth
still resolves its true inherited style, rather than degrading past a cap.
Semantics preserved exactly (ancestors chain, parent propagation, text-takes-
parent, source order). Revisit-trigger: none expected; if a profiler ever
shows the per-node `ComputedStyle` clones into frames hurt on the 486, switch
frames to carry an index/handle instead of an owned style.

### D16 — parser left as-is (already total at depth); guard-tested
`dom::parser` was audited for the same crash and found **already total**: it
drives an explicit `Vec`-backed open-element stack, not per-nesting-level
recursion, so 5000 nested (or 5000 unclosed) tags parse without overflow.
**Choice:** change nothing, add totality guard tests so a future refactor
that reintroduced recursion would be caught. Revisit-trigger: a parser change
that adds a recursive descent over nesting depth — re-audit then.

## P6 — Layout: block flow + inline engine (Wave 2)

### D14 — taffy flex substrate; layout is total via a depth cap; M2 scope calls
The layout engine is charter §158's "solvers over a flex substrate": **taffy
0.13** supplies block/flex box-math (block flow = degenerate column flex),
while inline layout is bespoke off measure-function leaves. Forks taken:
- **taffy trimmed hard.** `default-features = false`, features
  `["std","taffy_tree","flexbox","block_layout","content_size"]`. Dropped
  `grid`/`float_layout`/`detailed_layout_info`/`calc` (and transitive
  `smallvec`): no CSS grid in the dialect, our floats are bespoke-inline (M4)
  not taffy-side, no `calc()`. Serves the A2 ≤2MB budget and the C8 vendoring
  surface. Revisit: if a fixture ever needs CSS grid (it won't — out of
  dialect) or taffy-native floats.
- **Layout is total via `DEPTH_CAP = 100`.** The recursive tree walk
  (`translate`/`flatten_inline`/`emit`) plus taffy's own `compute_layout` had
  no depth bound; ~200 nested `Container`s overflowed the stack — a
  guard-page `SIGABRT` that `panic="abort"` cannot catch (uncatchable process
  death), reachable from a few hundred bytes of hostile/generated HTML.
  **Choice:** cap descent at depth 100 (well under the ~180–200 empirical
  floor, with margin for taffy's frames and the musl i486 stack); over-deep
  subtrees degrade to empty boxes rather than crashing (fallback-ladder
  ethos: keep the pipeline green). Capping `translate` bounds taffy's compute
  and `emit` for free (both only see the capped tree). Revisit: if a real
  document legitimately nests >100 deep (none should — that's already
  pathological), raise the cap or move to an explicit-stack iterative walk.
- **M2 scope simplifications (all bounded, none silent data loss on M2
  fixtures):** margin-collapsing not implemented (each block's margins apply
  independently); `Replaced` paints as a placeholder `Box` (no pixel data
  until P9); inline elements get no own background/border box (only block
  boxes paint); mixed block+inline children fold each maximal inline run into
  one taffy leaf (an approximation of CSS anonymous block boxes — nothing
  dropped); empty/whitespace-only text → zero lines; `white-space: pre`
  parsed but not honored (always collapses). **Deferred to M4** (with the
  image + float work, when `Replaced` carries pixels): a non-floated inline
  `<img>` between text sits inline instead of breaking flow, and a `Replaced`
  grandchild nested in an inline container gets a box instead of being
  dropped. No M2 fixture (basic.html) exercises inline images. Revisit each
  at its milestone.

## P4 — Image decoders (Wave 1)

### D13 — Decode caps & unsupported-format policy for hostile image bytes
Image bytes are attacker-controlled and the target is `panic=abort` (any
reachable panic/OOM is a hard, uncatchable crash), so the decoders enforce a
budget and degrade — never trust a decoder-declared dimension. **Choices:**
(a) a shared 64M-px cap (`MAX_DECODE_PIXELS`) rejects decompression-bomb
dimensions as `DecodeError::Unsupported` *before* any proportional allocation;
critically, JPEG checks it against the SOF frame header (`read_info`) **before**
`decode()`, because a progressive JPEG allocates its full coefficient buffer off
the untrusted width×height up front (a review Critical — a crafted 25-byte file
otherwise drove a multi-GB allocation). (b) The `gif` crate's own per-frame
memory limit is pinned to `MAX_DECODE_PIXELS*4` bytes so `check_pixel_cap`, not
the crate's stricter 50MB default, is the single authoritative gate. (c) Formats
recognized-but-unimplemented are `DecodeError::Unsupported(_)` so the caller
falls back to `alt` text (brief §6 L4), never a panic or garbage pixels:
**CMYK/YCCK JPEG** (no color-managed CMYK→RGB in v0) and 16-bit `L16` JPEG are
Unsupported; **APNG** is not decoded (only GIF animation is required by the
brief). Revisit-trigger: a real fixture needs a bigger cap, CMYK JPEG, or APNG —
raise the cap / add a color transform / add an APNG path then; low urgency for
the document web of 1996.

## P3 — Fetch (Wave 1)

### D12 — Cookie `Domain=` validated against the responding host; no PSL
Review caught a Critical bug: `parse_set_cookie` stored an explicit
`Domain=` attribute verbatim with no check against who actually sent it, so
a response from `attacker.test` could plant a cookie scoped to
`example.com` (cross-origin cookie injection), and `Domain=com` was
accepted as a supercookie for every `*.com` site. **Fix (RFC 6265 §5.3 step
6):** a `Domain=` cookie is now accepted only if the responding host
domain-matches it (`domain_matches(candidate, url.host())`); otherwise the
whole cookie is rejected (never silently downgraded to host-only). Since v0
has no public-suffix list, a second heuristic guard rejects any `Domain=`
value with no embedded dot (`com`, `localhost`, ...) outright — this is a
heuristic, not full PSL coverage: a two-label public suffix like `co.uk`
would still slip through. Also fixed in the same pass: `domain_matches`
now requires exact-string equality (not RFC 6265 §5.1.3 suffix matching)
whenever either side of the comparison is an IP-literal, so a stored
`.127.0.0.1` can never suffix-match `foo.127.0.0.1`. Revisit-trigger: a
fixture needs real public-suffix-aware matching (e.g. rejecting
`Domain=co.uk` specifically) — vendor a PSL table then; low urgency since
v0 has no third-party cookies to begin with (brief §4).

### D10 — Cookie domain convention: leading `.` encodes "subdomains match"
The frozen `Cookie` shape (`domain`, `path`, `name`, `value`, `secure`) has
no separate host-only/subdomain flag, but both `header_for`'s domain-match
and the Netscape jar format's second column need one. **Choice:** when a
`Set-Cookie` carries an explicit `Domain=` attribute, store the domain with
a leading `.` (subdomain matching enabled, RFC 6265 domain-match); a
host-only cookie (no `Domain=`) stores the bare host (exact match only).
`to_netscape`'s `TRUE`/`FALSE` flag column falls straight out of
`domain.starts_with('.')` with no extra field. Revisit-trigger: never,
unless a fixture needs public-suffix-aware domain matching (not attempted
in v0 — no third-party cookies per brief §4 makes this low-risk).

### D9 — Cookie expiry: every cookie is a session cookie in v0
`Expires`/`Max-Age` are parsed off the `Set-Cookie` header (so parsing
doesn't choke on them) and then discarded — the frozen `Cookie` struct has
no field to store an expiry in, and charter C6 only requires a plain-file
jar, not eviction semantics. **Choice:** treat every stored cookie as a
session cookie; `to_netscape`'s expiration column is always `0`.
Revisit-trigger: a fixture or the Lua chair needs persistent (non-session)
cookies across restarts — then add an expiry field (a freeze-packet change,
since `Cookie` is a frozen type) and stop ignoring `Max-Age`/`Expires`.

### D8 — Bespoke HTTP/1.1 over `std::net::TcpStream`, not `httparse`
The brief (§4, §5) names `httparse` as the HTTP layer's crate. P3 needs to
land now, but the crate-vendoring apparatus (needed to bring in *any*
external crate under charter C8's "vendored + attested" rule) is being set
up separately ahead of P4. Options: (a) block P3 on vendoring landing first;
(b) hand-roll HTTP/1.1 parsing, std-only. **Choice: (b).** A bespoke
request formatter + total (never-panics) response parser — status line,
case-insensitive/folded headers, Content-Length and chunked bodies — is a
few hundred lines over `std::net::TcpStream`, unblocks the whole Wave 1
fetch packet immediately, and adds zero dependencies to `Cargo.toml`/
`Cargo.lock` (verified: both unchanged by this packet). Revisit-trigger:
once the vendoring apparatus lands and P4 needs it anyway, consider
swapping this hand-rolled parser for `httparse` if a fixture exposes a
real-world HTTP/1.1 edge case (e.g. more exotic chunk-extension syntax)
that's cheaper to get from a maintained parser than to keep hand-fixing.

### D11 — gzip deferred to a later packet
`Content-Encoding: gzip` (brief §4/§5, via `miniz_oxide`) is out of scope
for P3 for the same vendoring-not-ready-yet reason as D8. **Choice:** the
client advertises `Accept-Encoding: identity` only (never claims to accept
gzip it can't decode), and the fixture server always answers with identity
encoding, so no test in this packet exercises decompression. `Response`'s
`body` doc comment already promises "gzip already inflated" for whenever
that packet lands. Revisit-trigger: `miniz_oxide` is vendored — wire gzip
decoding into `read_response`'s body-decoding step, gated on a
`Content-Encoding: gzip` response header.

## M0 — Toolchain

### D1 — Build substrate: GitHub Actions running the monolith-builder image
Options: (a) run the image locally; (b) a fallback Debian/Ubuntu toolchain;
(c) GitHub Actions job containers. The working environment has no container
runtime and no root (can't install one), so (a) is impossible here and (b)
was explicitly ruled out by the operator (and by charter C11). GitHub-hosted
runners have Docker and run the image directly via `jobs.<id>.container`.
**Choice: (c).** The image is public on ghcr, so no registry secret is
needed. Revisit-trigger: a local/self-hosted runner with the image becomes
available, or CI minutes become a constraint.

### D2 — Toolchain pinned to nightly-2026-07-15
The image ships `nightly-2026-07-15` as the active default (rustc
1.99.0-nightly, da80ed070 2026-07-14) with `rust-src` already present.
**Choice:** pin exactly that in `rust-toolchain.toml` so the build is
reproducible and offline (no rustup download). Revisit-trigger: the image
updates its default nightly, or a needed feature forces a bump — repin then.

### D3 — Hardware x87 float, not soft-float (486SX is a known gap)
The brief's L2 ladder allows hardware float with a documented 486SX gap. The
image's musl cross toolchain (`i486-linux-musl-`) is a standard hardware-float
musl; matching it avoids a soft-float musl mismatch fight. **Choice:** target
spec carries `features: -mmx,-sse,-sse2` (cpu=i486 already implies no SSE/MMX)
and hardware x87 float — confirmed by `--print cfg` reporting `target_feature="x87"`.
Consequence: **486SX (no FPU) is unsupported in v0.1; 486DX/DX2 (with FPU),
including the myth's DX2, are supported.** Revisit-trigger: 486SX support is
ever required — then build a soft-float musl variant.

### D4 — max-atomic-width = 32 (no 64-bit atomics on a 486)
The i486 lacks CMPXCHG8B (introduced on the Pentium), so 64-bit atomics can't
be lock-free. **Choice:** target spec sets `max-atomic-width: 32` (charter C9).
`--print cfg` confirms `target_has_atomic` tops out at 32. 64-bit atomic ops
in std lower to `__atomic_*` libcalls supplied by compiler-builtins.
Revisit-trigger: link errors on `__atomic_*_8` symbols — then ensure the
compiler-builtins atomics/libatomic shim is linked.

### D5 — A4 executes the binary under qemu on the ubuntu host, not in-image
qemu is absent from the monolith-builder image (confirmed by the substrate
run), and installing it on gentoo means a slow `emerge`. The i486 binary is
fully static (crt-static), so it needs nothing from the build image at
runtime. **Choice:** two-job CI — `build` in the image, `accept` on the plain
ubuntu host where `qemu-user` is one `apt-get` away. Revisit-trigger: a reason
to execute inside the image (e.g. testing image-provided runtime bits) — then
add a static qemu-i386 to the image or the job.

### D7 — libunwind shim: alias libgcc_eh.a as libunwind.a
std's musl `unwind` crate links `-lunwind` (LLVM's libunwind), but the image's
cross toolchain is GCC-based and ships `libgcc_eh.a` (same `_Unwind_*` API)
instead. With self-contained linking off there is no bundled libunwind to fall
back on. Options: (a) build LLVM libunwind in-tree (needs llvm sources absent
from rust-src); (b) provide libunwind. **Choice: (b)** — symlink the cross
gcc's `libgcc_eh.a` (found via `-print-file-name`) as `libunwind.a` on the link
search path. Since `panic=abort` never unwinds, the `_Unwind_*` symbols resolve
but are never called. Revisit-trigger: unwinding is ever enabled, or the image
starts shipping a real libunwind — then link that instead.

### D6 — cargo-auditable / cargo-audit deferred to M6
Both are absent from the image (substrate run). They serve A6/C11 (attested
provenance, audit-clean deps), which the brief scopes to M6, not M0.
**Choice:** defer; M0 gates only on A1 + A4. Revisit-trigger: M6 hardening —
install/vendor them then, or bake them into the image.
