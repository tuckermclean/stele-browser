# Box constraints (Acid2 Packet 5) Implementation Plan
> REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Spec:
> docs/superpowers/specs/2026-08-20-box-constraints-design.md (read it for full design per feature).

**Goal:** min/max-width/height clamping (taffy-native) + `overflow:hidden` paint clipping + `background-position`.
**Global constraints:** no new dep; 1.44MB floppy (report i486 delta); parse TOTAL (bad→initial, no panic);
golden-safe (each inert at initial value); no local i486 builds; PNG goldens pixel-verified (controller); no JS/C3.

### Task 1: min/max-width/height (taffy plumbing) — computed.rs/value.rs/cascade.rs/block.rs
Add `min_width,max_width,min_height,max_height: Dimension` (default `Auto`; non-inherited) to ComputedStyle;
parse `min-/max-width/height` like `width`/`height` (`max-*:none`→`Dimension::Auto`; `min-*:auto`→`Auto`);
cascade own; in `base_style` (block.rs, next to `size:` at line 1557) add
`min_size: TSize{width:map_dimension(cs.min_width),height:map_dimension(cs.min_height)}` and same for
`max_size`. Taffy clamps. Tests: parse + a base_style mapping test (a `Dimension::Px(80)` min maps to taffy
min_size length(80)). CI: cargo test --lib. Commit: `feat(css): min/max-width/height via taffy min_size/max_size (Acid2 P5)`.

### Task 2: overflow:hidden clipping — computed.rs/value.rs/cascade.rs/layout/mod.rs/block.rs/raster.rs
Per spec §2: `Overflow{Visible,Hidden}` (scroll/auto→Hidden for static paint); parse `overflow`/`-x`/`-y`
(either clipping ⇒ Hidden); add `pub clip: Option<Rect>` to `Fragment`; `emit` threads a `clip` param,
intersecting the container's border box when `overflow==Hidden`, stamping each Fragment.clip; raster intersects
each fragment's draw with `fragment.clip` (empty ⇒ skip); tty ignores clip. Tests: parse; emit stamps clip on a
descendant of overflow:hidden (None otherwise); intersect geometry. Commit: `feat(layout): overflow:hidden paint clipping via per-fragment clip rect (Acid2 P5)`.

### Task 3: background-position — computed.rs/value.rs/cascade.rs/raster.rs
Per spec §3: `background_position: (LengthPercentage,LengthPercentage)` default `(Percent(0),Percent(0))`;
parse keywords(left/center/right,top/center/bottom→0/50/100%) + length/percentage (1 val⇒x, y=center); cascade
own; raster offsets the bg-IMAGE tile anchor (raster.rs ~363) by the resolved position (percentage against
box_size−image_size, length direct). Tests: parse totality. Commit: `feat(css): background-position offsets the background image (Acid2 P5)`.

### Task 4: fixtures + accept.sh + controller bless
`fixtures/bc-minmax.html` (width:40;min-width:80 clamped up; width:300;max-width:100 clamped down),
`bc-overflow.html` (overflow:hidden 60x60 parent clips a 200x200 child), `bc-bg-position.html` (a data: PNG bg
positioned right/bottom or 20px 10px). Wire accept.sh A5o/A5p/A5q (mirror A5k). Controller renders + pixel-
verifies + blesses. Commit: `test(css): box-constraint micro-fixtures + accept.sh (Acid2 P5)`.

### Task 5: charter + DECISIONS + JOURNAL (+ i486 size)
Charter C2 amendment; DECISIONS entry (per spec §Charter/decisions); JOURNAL P5 entry with size delta.
