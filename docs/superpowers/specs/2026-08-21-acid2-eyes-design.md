# Acid2 eyes — design

**Date:** 2026-08-21 · Milestone B, part 1 of the Acid2 geometry program (builds on Milestone A,
`docs/superpowers/specs/2026-08-20-acid2-scroll-fixed-design.md`, PR #91). Milestone A made the smiley
**compose** at the window top; this milestone is scoped to ONLY the `.eyes` subtree — the scalp, forehead,
nose bridge, mouth/chin already paint something recognizable per the task brief. **OUT OF SCOPE** (a separate,
later packet): exact eye size/position/em-fidelity, `background-position`/`background-attachment:fixed`
support, `display:inline` layout fidelity for the nested-`<object>` cascade, and any byte-match against the
WaSP reference PNG. This milestone's bar, stated honestly per the task brief: **the eyes visibly render as two
dark marks inside the red `.eyes` box**, not a blank red rectangle.

## Goal
Make `fixtures/acid2.html`'s `.eyes` subtree (lines 51–57, 130 — position:absolute; top:5em; left:3em;
background:red, containing `#eyes-a`/`#eyes-b`/`#eyes-c`) paint visible dark/eye-colored content instead of
its current bare red box.

## Non-negotiables (AGENTS.md, unchanged by this packet)
- **No JavaScript, by construction** (charter C3) — `<object>` stays inert document content; this packet adds
  no execution path, only decode/parse correctness fixes.
- **1.44 MB floppy ceiling.** Both fixes below are pure logic changes to already-shipped, dependency-free
  modules (`src/fetch/data.rs`'s in-repo base64 decoder; `src/style/parser.rs`'s declaration scanner) — no new
  crate, no new asset. Report the `stele-i486` artifact size delta in the PR.
- **CI-driven build/test.** No local `cargo build`/`cargo test`. Push, read `m0-acceptance`, download the
  `stele-host`/`renders` artifact to bless goldens.
- **Totality / no panic on hostile input.** A malformed percent-escape inside a `;base64` data: URI payload, or
  a semicolon inside an unquoted `url(...)` that never finds a matching `)`, must degrade to "this URL/value
  doesn't parse" (existing `FetchError`/`None`-return contracts), never panic.
- **Goldens are byte-compared; pixel-verify before blessing, never rubber-stamp.** Both fixes are additive
  bugfixes to already-committed code — grep-confirmed (see §Current state) that no existing golden exercises
  either exact code path, so no existing golden is expected to change; the one exception (`acid2-scrolled.png`,
  Milestone A's own A5w golden) is EXPECTED to change (it will visibly gain eye content) and must be re-blessed
  only after pixel-measuring the new render.
- **Test-first.** Every task below starts with a failing test.

## Current state (ground-truthed 2026-08-21)

### The fixture, precisely
`fixtures/acid2.html:130`:
```html
<div class="eyes">
  <div id="eyes-a">
    <object data="data:application/x-unknown,ERROR">
      <object data="404" type="text/html">
        <object data="data:image/png;base64,…(a PNG with 8bit alpha containing two eyes, per the fixture's own trailing HTML comment)…">ERROR</object>
      </object>
    </object>
  </div>
  <div id="eyes-b"></div>
  <div id="eyes-c"></div>
</div>
```
CSS (`fixtures/acid2.html:51-57`): `.eyes{background:red}`; `#eyes-a{height:0;line-height:2em;text-align:right}`
(inline content paints top-most); `#eyes-a object{display:inline;vertical-align:bottom}`; `#eyes-a
object[type]{width:7.5em;height:2.5em}` (must have no effect — geometry, out of scope); `#eyes-a object object
object{border-right:solid 1em black;padding:0 12px 0 11px;background:url(data:image/png;base64,…) fixed 1px 0}`
(the innermost object, own CSS background — a SECOND, small 2×2 eye tile, distinct from its own `data=`
attribute image); `#eyes-b{float:left;width:10em;height:2em;background:fixed url(data:image/png;base64,…);
border-left:solid 1em black;border-right:solid 1em red}` (an empty `<div>` — its ENTIRE visible content is this
CSS background); `#eyes-c{display:block;background:red;border-left:2em solid yellow;width:10em;height:2em}`
(no `data:` URI at all — already renders correctly today, not implicated in either finding below).

### Finding 1 (primary root cause) — `data:` percent-escapes inside a `;base64` payload are never decoded
`src/fetch/data.rs::fetch` (`data.rs:5-40`) branches on the `;base64` flag at `data.rs:17-20` and, for a
base64 payload, calls `decode_base64(payload.as_bytes())` **directly** (`data.rs:28-29`) — the raw payload
bytes, unmodified. `decode_base64` (`data.rs:76-117`) maps each byte through `b64_val` (`data.rs:60-69`), whose
match arms cover only `A-Za-z0-9+/`; any other byte — including a literal `%` — falls to `_ => None`
(`data.rs:67`) and `decode_base64` returns `Err(FetchError::Protocol("data: invalid base64"))` at the first
such byte (`data.rs:93`).

`fixtures/acid2.html` percent-escapes `+`, `/`, and `=` inside every one of its base64 payloads (`%2B`, `%2F`,
`%3D` — 6 occurrences confirmed via `grep -c '%2F\|%3D\|%2B' fixtures/acid2.html`), a standard, real-world
`data:` URI pattern (data: URIs are URIs; RFC 3986 percent-escaping is legal at any point in a URI, and a
compliant parser percent-decodes the whole URI before applying the scheme-specific `;base64` decoding — this is
literally the pattern the actual W3C Acid2 test HTML uses, which `fixtures/acid2.html` is a copy of). Because
`decode_base64` is called on the RAW (still percent-escaped) payload, **every** `data:image/png;base64,…`
reference in `fixtures/acid2.html` fails to decode today: `.forehead`'s and `.chin`'s CSS backgrounds, both eye
CSS backgrounds (`#eyes-a object object object`, `#eyes-b`), AND — critically — the innermost `<object>`'s own
`data=` HTML **attribute** (the "two eyes" PNG itself, per the fixture's trailing comment), which is a pure DOM
attribute string, not CSS, so it is NOT touched by Finding 2 below and would otherwise decode fine.

**Consequence for the eyes specifically, traced through the already-shipped `<object>` fallback (D60,
`src/layout/box_tree.rs:177-198`, `src/images.rs:150-198`):**
1. Outer `<object data="data:application/x-unknown,ERROR">`: no `;base64` flag, percent-decodes fine to bytes
   `"ERROR"`, content-type `application/x-unknown` — `img::decode_bytes` correctly rejects this as an
   unsupported/unrecognized image type → no map entry → falls back to its child (by design, this object SHOULD
   fail — not a bug).
2. Middle `<object data="404" type="text/html">`: `404` resolves (relative to the fixture's own base URL) to a
   file that does not exist → fetch fails → no map entry → falls back to its child (also by design).
3. Inner `<object data="data:image/png;base64,…">`: this is exactly the payload Finding 1 breaks —
   `images::walk`'s `"object" => el.attrs.get("data")` branch (`images.rs:171`) resolves and calls
   `fetch_and_decode`, which now returns `None` (base64 decode error) → **no map entry for the innermost
   object either** → `box_tree::build_node`'s `<object>` branch (`box_tree.rs:178`, `if let Some(img) =
   images.get(&id)…`) takes the `else` path and falls through to the normal element path, which renders the
   object's **fallback children** — its literal text content, `"ERROR"`.

So today, the `.eyes` subtree's `#eyes-a` renders the plain-text word **"ERROR"** (`text-align:right`, painted
inside a `height:0` box per the CSS comment "contents should paint top-most") where the two-eye PNG belongs —
not a blank gap, a wrong-content bug. `#eyes-b` (an empty `<div>` whose ENTIRE content is its CSS
`background:fixed url(data:…)`) renders nothing at all — no border-box fill in the middle at all, since its own
`background-image` never resolves either (Finding 1, same payload pattern) and it declares no
`background-color` of its own (falls through to `.eyes`'s ambient red).

**Untested gap, not a regression:** `fixtures/data-img.html` (the P4 golden, `goldens/data-img.png`) and
`fixtures/object-image.html`/`object-nested.html` (the P6 goldens) all use **un-escaped** base64 payloads
(grep-confirmed — no `%` inside any base64 portion of those fixtures) — this exact percent-escaped-base64
pattern was never exercised by an existing test or golden, so this is new coverage, not a fix to something a
golden already claimed worked.

### Finding 2 (secondary, additive) — an unquoted `url(...)` with an internal `;` truncates its CSS declaration
`src/style/tokenizer.rs::tokenize` (`tokenizer.rs:41-177`) lexes the CONTENTS of an unquoted `url(...)` with
the exact same general-purpose rules as the rest of the stylesheet (there is no CSS "url-token" special
lexing state) — a raw `;` anywhere inside becomes an ordinary `Token::Semicolon` (`tokenizer.rs:164`), exactly
like a `;` between two ordinary declarations.

`src/style/parser.rs::parse_declaration_block` (`parser.rs:581-656`) finds a declaration's value by scanning
forward to the next `Token::Semicolon` or `Token::RBrace` (`parser.rs:618`: `while *pos < len && tokens[*pos]
!= Token::Semicolon && tokens[*pos] != Token::RBrace { *pos += 1; }`) — **with no paren/function-depth
tracking**, unlike the selector-parsing code earlier in the very same file (`parser.rs:448-479`, which DOES
track `LParen`/`Function` depth when skipping an unsupported functional pseudo-class — an established,
precedented pattern this code path doesn't reuse). Since `data:image/png;base64,…`'s own `;` (between the media
type and the `base64` flag) sits INSIDE an unquoted `url(...)`, `parse_declaration_block` truncates the
declaration's value tokens right there — before the url's closing `)` is ever reached.

`value::parse_url_function`/`parse_background_image_component` (`value.rs:1052-1118`) then scan the (truncated)
token list for a matching `Token::RParen`, never find one, and return `None` (`value.rs:1077-1078` /
`value.rs:1105`) — `background_image` is never set on the `ComputedStyle` for that declaration.

**Every** `url(data:...)` in the whole repository has this shape (`grep -rln "url(data:" fixtures/*.html` →
`fixtures/acid2.html` only, 4 occurrences: `.forehead`, `.chin`, `#eyes-a object object object`, `#eyes-b` —
all four have a `;base64` mediatype), so all four background-image declarations are affected. The parser
recovers cleanly afterward (`skip_to_decl_boundary`, `parser.rs:658-666`, resyncs at the next REAL `;`, which a
base64 payload never contains a literal `)` before — confirmed by tracing `#eyes-b`'s own rule, which has two
MORE declarations, `border-left`/`border-right`, AFTER the broken `background:` — both still parse correctly),
so this is narrowly scoped to "the `background-image` component of a `url(data:...)` declaration with an
internal `;` is silently dropped," not a wider parse corruption. A `background-color` listed BEFORE the `url()`
in the same shorthand (e.g. `.forehead`'s `red`, `.chin`'s `yellow`) is unaffected — it's captured before the
truncation point — which is consistent with the task brief's own observation that a red nose bridge and yellow
mouth already paint today.

**Untested gap, not a regression:** every existing `background`/`background-image` test that exercises a
`url(...)` with commas or other punctuation calls `apply_property` DIRECTLY on pre-tokenized value tokens
(`value.rs`'s `toks()` helper, `value.rs:2254-2256`, tokenizes only the isolated value string) — bypassing
`parse_declaration_block`'s boundary-scanning entirely. No existing test feeds a FULL stylesheet rule
(`selector { background: url(data:...;...) }`) through the real parser (grep-confirmed: `url(.*;.*)` appears
nowhere in `src/style/*.rs`/`tests/*.rs` outside this repo's `@font-face`/plain-url tests, none of which have
an embedded `;`). This gap was never caught because nothing exercised it end-to-end.

### What is NOT broken (verified, not assumed)
- `img::decode_bytes` / `src/img/png.rs` already supports RGBA, grayscale+alpha, and paletted-with-`tRNS` PNGs
  (`png.rs:5-7`, doc comment) — once Finding 1 is fixed, the PNG bytes themselves should decode fine; this is
  not a third blocker.
- `<object>`'s nested-fallback cascade itself (D60, `box_tree.rs:177-198`) is correct and requires NO changes —
  it already resolves to the innermost representation once that representation's bytes actually decode.
- CSS background-image PAINTING (tiling, `paint_box`, `src/backend/raster.rs:319-333`) and the fetch+decode
  pre-pass (`src/bg_images.rs`, D59-era) are both already wired and functional — `background_image` just never
  gets SET on the relevant `ComputedStyle`s today (Finding 2), so the already-working paint path never gets a
  URL to look up.
- `#eyes-c` (plain block, no `data:` URI) already renders correctly and needs no change.

## Design

Two independent, minimum-scope fixes. Either alone changes the render; both together are required to get the
CSS-2.1-Appendix-E three-layer composition (`#eyes-a`'s inline object + `#eyes-a`'s own CSS background + the
`#eyes-b` float's CSS background) instead of just the innermost `<object>`'s own primary image.

### Task A (required) — percent-decode a `;base64` payload before base64-decoding it
`src/fetch/data.rs::fetch`, the `is_base64` branch (`data.rs:28-29`): percent-decode `payload` FIRST (reuse the
existing `percent_decode` helper, `data.rs:119-136`, already used for the non-base64 branch), THEN base64-decode
the result:
```rust
let body = if is_base64 {
    decode_base64(&percent_decode(payload.as_bytes()))?
} else {
    percent_decode(payload.as_bytes())
};
```
`percent_decode` is already total (bad/truncated `%XX` passes the bytes through literally, `data.rs:119-136`,
tested at `data.rs:190-192`) and pure-ASCII-safe (operates on bytes, not chars, so no UTF-8 boundary concerns —
unlike `strip_suffix_ci`, which the doc comment at `data.rs:254-262` already flags as the char-boundary-aware
one). This is the ENTIRE code change for Task A — no new function, no new dependency, one call-site edit plus a
doc-comment update explaining why (RFC 3986: percent-escaping is legal anywhere in a URI; a `;base64` payload is
still URI content and may legitimately escape `+`/`/`/`=` to survive contexts — CSS `url()`, HTML attributes —
that could otherwise misparse those characters).

This alone: the innermost `<object>`'s own `data=` PNG (a pure HTML-attribute path, untouched by Finding 2)
decodes successfully → `images::walk` (`images.rs:150-198`) populates a map entry for it → `box_tree`'s
already-shipped `<object>` branch (`box_tree.rs:177-190`) renders it as a `Replaced` image instead of falling
back to the literal "ERROR" text. Per the fixture's own trailing comment, this PNG already **is** "a PNG with
8bit alpha containing two eyes" — Task A alone is very likely sufficient to hit this milestone's honest bar
("two dark marks"), independent of Task B.

### Task B (in scope — completes the CSS-2.1 Appendix E composition) — depth-aware declaration-value scanning
`src/style/parser.rs::parse_declaration_block`'s value-boundary scan (`parser.rs:618`) gains paren/function
depth tracking, mirroring the EXISTING pattern in the same file's selector parser (`parser.rs:448-479`):
```rust
let mut depth = 0i32;
while *pos < len
    && !(depth == 0 && (tokens[*pos] == Token::Semicolon || tokens[*pos] == Token::RBrace))
{
    match &tokens[*pos] {
        Token::Function(_) | Token::LParen => depth += 1,
        Token::RParen => depth = (depth - 1).max(0), // never goes negative on hostile/unbalanced input
        _ => {}
    }
    *pos += 1;
}
```
The loop's two exit checks below it (`*pos < len && tokens[*pos] == Token::Semicolon` at `parser.rs:622`, and
`skip_to_decl_boundary`'s own copy at `parser.rs:660/663`) get the identical depth-tracking treatment —
`skip_to_decl_boundary` (used when a declaration doesn't even have a recognizable `name: ` shape) needs the
SAME fix for the same reason: an unrecognized/malformed leading token followed by a `url(...)` with an internal
`;` would otherwise also mis-resync. An unterminated `url(` at EOF (depth never returns to 0) still terminates
the loop via `*pos < len` — total, matches the existing "tolerate unterminated constructs" posture
(`tokenizer.rs:277-281`'s own test name).

This makes `background_image` finally get SET (via the ALREADY-WORKING `parse_background_image_component` /
`bg_images::collect_bg_images` / `paint_box` pipeline — none of which need to change) for:
- `#eyes-a object object object`'s own CSS background (the eye-tile the innermost object ALSO carries as a
  background, distinct from its primary `data=` image) — makes the fixed-attachment eye tile show through, and
  gives the border/padding box its intended eye-adjacent detail.
- `#eyes-b`'s CSS background — the ONLY visible content this empty `<div>` has; without Task B it stays a
  colorless gap between its two borders.
- `.forehead`'s and `.chin`'s background images (not required for "the eyes," but the same root cause, so this
  fix reaches them too — a real, welcome, zero-extra-cost side effect, not scope creep, since it's the literal
  same code path with no eyes-specific branching).

### Explicitly OUT of scope for this packet
- `background-position` / `background-attachment: fixed` (both parsed-and-discarded today — D59's own prior
  deferral; `#eyes-a`'s/`#eyes-b`'s `fixed 1px 0` tokens are silently ignored, so the eye tiles TILE across
  their box instead of being placed/pinned at an exact offset). Geometry fidelity, not this milestone's bar.
- `display:inline` layout correctness for the 3-deep nested `<object>` cascade (whether `#eyes-a object[type]`'s
  width/height correctly have no effect, whether the objects visually stack per CSS2.1 Appendix E's exact paint
  order). Also geometry, also deferred.
- `#eyes-a`'s own `height:0` / `text-align:right` / `line-height:2em` box-model precision.
- Any pixel-exact match against the official WaSP Acid2 reference bitmap.

## Testing / fixtures

### Task A
- **Unit (`src/fetch/data.rs`):** `decode_base64` round-trips a payload containing `%2B`/`%2F`/`%3D` escapes for
  literal `+`/`/`/`=` bytes that must land inside the decoded base64 alphabet correctly (a payload hand-built so
  its RAW base64 characters include `+` and `/`, then re-encoded with those three bytes percent-escaped — assert
  `fetch(...).unwrap().body` equals the same bytes as the un-escaped control case). A malformed escape (`%ZZ`
  where hex digits are invalid) inside a base64 payload doesn't panic — falls through to `percent_decode`'s
  existing literal-passthrough behavior (`data.rs:190-192`), then either decodes (if the literal `%ZZ` chars
  happen to not appear, they will, so this should actually fail) or errors via `decode_base64`'s existing
  invalid-alphabet path — either way, `Result`, never a panic.
- **Golden micro-fixture** `fixtures/data-img-percent.html`: `<img src="data:image/png;base64,<a small PNG whose
  base64 encoding, verified by hand/script, contains at least one +, /, and = percent-escaped as %2B/%2F/%3D>">`
  — pixel-verify the decoded image's exact color/size before blessing (mirrors `data-img.html`'s own existing
  golden shape, `accept.sh` A5n).
- `fixtures/object-image.html`'s existing coverage stays as-is (unescaped payload, still exercises the
  base64-without-percent-escapes path — Task A must not regress it: add a unit test asserting un-escaped base64
  still round-trips identically post-fix, since `percent_decode` on a string with no `%` byte is already
  proven to be the identity, `data.rs:195-197`, but assert it explicitly for THIS specific call site too).

### Task B
- **Unit (`src/style/parser.rs`):** `parse("div { background: red url(data:image/png;base64,AAAA); color:
  green; }")` — assert the resulting `ComputedStyle` for `div` has BOTH `background_color == red` AND
  `background_image == Some("data:image/png;base64,AAAA")`, AND that the trailing `color: green` declaration
  (proving the parser correctly resynced past the `;`-bearing `url()` and didn't lose or corrupt anything after
  it) also applied. A second test for the `skip_to_decl_boundary` path: a deliberately-unrecognized declaration
  immediately followed by one containing a semicolon-bearing `url(...)`, asserting the SECOND declaration still
  parses (proves the malformed-declaration resync path got the same depth-tracking fix, not just the
  named-declaration path).
- **Golden micro-fixture** `fixtures/bg-image-semicolon.html`: `<div style="width:20px;height:20px;background:
  url(data:image/png;base64,<small PNG>) no-repeat;">` (inline `style=` attribute, not a `<style>` block — proves
  the fix lives in the shared tokenizer/declaration-scan path, not something `<style>`-block-specific) — pixel-
  verify the decoded image paints (not a blank/missing background) before blessing.

### `acid2.html` itself
Milestone A's existing `A5w` golden (`goldens/acid2-scrolled.png`) will visibly gain eye content once both
tasks land — this is an EXPECTED, deliberate re-bless (not a regression: today's render has literal "ERROR"
text and a gap where eyes belong; the new render has real eye-shaped content in the same region). Re-bless
ONLY after downloading the CI render artifact and pixel-measuring: (a) the "ERROR"-text glyph pixels are GONE
from the `.eyes` region, (b) new non-background-red pixel content (dark/black/yellow, matching the eye PNGs'
own known 2×2 pattern of yellow-vs-transparent corners, or the "two eyes" PNG's own decoded content) appears
within the `.eyes` box's known approximate screen region. A connected-component or color-histogram check
(same discipline as Milestone A's own A5w bar) is the concrete, scriptable pass/fail line — "looks different"
is not sufficient, per AGENTS.md rule 4.

## Charter / decisions note
Both fixes are **bugfixes to already-adopted C2 dialect surface** (the `data:` scheme, Acid2 Packet 4/D58; the
CSS `background`/`url()` shorthand, packet bg-image) — no NEW CSS property, keyword, element, or URI scheme.
Record a new DECISIONS entry (next free letter after D64) covering: (1) `data:` `;base64` payloads are now
percent-decoded before base64-decoding (RFC 3986 correctness, not new dialect surface); (2)
`parse_declaration_block`'s value-boundary scan is now paren/function-depth-aware, matching the file's own
existing selector-parsing precedent, so an unquoted `url(...)` containing a literal `;`/`,`/other-punctuator no
longer truncates its declaration; (3) the resulting Acid2 eyes finding and re-bless, with the honest scope line
this design doc opens with (two dark marks, not byte-exact geometry — that's the next milestone). No
`stele-charter.md` "What Stele Speaks" amendment is expected (AGENTS.md rule 6) — flag this judgment in the PR
description for the operator to confirm rather than silently assuming it.
