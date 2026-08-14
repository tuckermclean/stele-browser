# DECISIONS

Forks taken while the operator was away. Each: options, choice, why,
revisit-trigger. Newest first.

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
