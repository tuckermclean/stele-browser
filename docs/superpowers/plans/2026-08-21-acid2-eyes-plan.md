# Acid2 eyes Plan · Spec: docs/superpowers/specs/2026-08-21-acid2-eyes-design.md (read it)

**Goal:** make `.eyes` in `fixtures/acid2.html` paint visible dark eye content instead of literal "ERROR" text
(`#eyes-a`) and a blank gap (`#eyes-b`) — two independent, root-caused bugfixes, NOT new dialect surface.
Milestone B part 1: "two dark marks," not exact geometry (spec's own OUT-of-scope list).

**Architecture (one sentence per moving part, see spec for the why):** `src/fetch/data.rs::fetch`'s `;base64`
branch percent-decodes its payload before base64-decoding it (spec Task A — fixes the innermost `<object>`'s
own `data=` PNG, a pure HTML-attribute path); `src/style/parser.rs::parse_declaration_block` (and its
`skip_to_decl_boundary` sibling) track paren/function depth while scanning for a declaration's terminating `;`,
so a raw `;` inside an unquoted `url(...)` no longer truncates the declaration (spec Task B — fixes
`#eyes-a object object object`'s and `#eyes-b`'s CSS `background: url(data:...)`, plus `.forehead`/`.chin` for
free). Both fixes are independent (different files, different code paths, either alone changes the render) —
genuine parallelism opportunity, see below. A5w's existing Acid2 golden re-bleses once both land.

**Global constraints (every task):** no new dependency; report the `stele-i486` size delta in the PR; **no
local `cargo build`/`cargo test`** — push and read `m0-acceptance`; total/no-panic on hostile input (malformed
percent-escapes, unterminated `url(`, unbalanced parens); every task starts with a failing test (visible
red→green in the commit history); pixel-verify (not eyeball) any golden this plan touches, per AGENTS.md rule 4.

**Task ordering / parallelism note:** Tasks 1 and 2 touch disjoint files (`src/fetch/data.rs` vs
`src/style/parser.rs`) and neither depends on the other's code — **no shared low-level infra to pre-assign**,
unlike Milestone A's carrier-field task. They CAN run in parallel worktrees/sessions. Task 3 (the Acid2 golden
re-bless) depends on BOTH landing first (the honest "two eyes" bar needs the innermost object's image AND the
CSS-background eye tiles — spec's own "either alone changes the render; both together are required for the
full Appendix-E composition" note) — do not attempt Task 3 until Tasks 1 and 2 are both merged to the branch
this packet ships from.

---

### Task 1 — percent-decode before base64-decode in `data:` URIs

**Files:** `src/fetch/data.rs` (`fetch`'s `is_base64` branch, `data.rs:28-32`).

**Interfaces:** no signature change — `fetch(request: &Request) -> Result<Response, FetchError>` stays as-is;
only the body computation inside it changes.

**Failing-test-first steps:**
1. Test in `data.rs`'s existing `#[cfg(test)]` module: build a payload from KNOWN raw bytes that, when
   base64-encoded, are guaranteed to contain at least one `+` and one `/` (e.g. reuse the module's own
   `encode_base64_for_test` helper, `data.rs:266-289`, on a byte sequence chosen/verified to produce both —
   `[0xFB, 0xFF, 0xBF, ...]` or similar; don't guess blind, compute it and assert the encoded string actually
   contains `+`/`/` before using it as the test's premise). Percent-escape those two characters in the encoded
   string (`+` → `%2B`, `/` → `%2F`) to build the URL, e.g. `format!("data:application/octet-stream;base64,{}",
   b64.replace('+', "%2B").replace('/', "%2F"))`. Assert `fetch(&get(&url)).unwrap().body == raw_bytes`. **Red**
   against current code (today: `Err(FetchError::Protocol("data: invalid base64"))`, since the literal `%` byte
   isn't in `b64_val`'s alphabet).
2. Test: an `=` padding character percent-escaped as `%3D` in an otherwise-normal base64 payload (mirrors
   `fixtures/acid2.html`'s own actual pattern — its payloads end `...%3D` for the padding byte) — assert it
   decodes identically to the same payload with a literal, un-escaped `=`. Red.
3. Test: a malformed percent-escape inside a base64 payload (`%ZZ`, non-hex digits) does not panic — either
   still decodes (if `percent_decode`'s literal-passthrough leaves a `%`/`Z`/`Z` sequence that HAPPENS to not
   appear, unlikely) or returns `Err(FetchError::Protocol(_))` via `decode_base64`'s existing invalid-alphabet
   path. Assert `Result`, not a panic (use `std::panic::catch_unwind` or simply call it directly — either is
   fine, this repo's existing hostile-input tests use direct calls and rely on the process not aborting).
4. Regression test: un-escaped base64 (the EXISTING `fetch_base64_image_round_trips_raw_bytes` test,
   `data.rs:228-239`) still passes unchanged post-fix — `percent_decode` on a payload with no `%` byte is
   already proven to be the identity (`data.rs:195-197`), but re-run/confirm this specific test still green
   (it's the direct regression bar for Task 1).
5. Implement (spec Task A):
   ```rust
   let body = if is_base64 {
       decode_base64(&percent_decode(payload.as_bytes()))?
   } else {
       percent_decode(payload.as_bytes())
   };
   ```
   Update the module's own top-of-file doc comment (`data.rs:1-2`) to note the percent-decode-before-base64
   step and cite RFC 3986 (percent-escaping is legal anywhere in a URI).
6. Green (CI, not local).

**Commit:** `fix(fetch): percent-decode a data: URI's base64 payload before decoding it (RFC 3986)`

---

### Task 2 — depth-aware declaration-value scanning in the CSS parser

**Files:** `src/style/parser.rs` (`parse_declaration_block`'s value-boundary scan, `parser.rs:618` and its
Semicolon-check at `parser.rs:622`; `skip_to_decl_boundary`, `parser.rs:658-666`).

**Interfaces:** no signature change on either function — both stay `(tokens: &[Token], pos: &mut usize, ...)`
internal-scan helpers; only their termination condition changes.

**Failing-test-first steps:**
1. Test in `parser.rs`'s existing `#[cfg(test)]` module: `parser::parse("div { background: red
   url(data:image/png;base64,AAAA); color: green; }")` (mirrors the module's existing
   `background_image` cascade test shape, `cascade.rs:843`, but drives it through the REAL declaration-block
   parser, not `apply_property` directly). Assert the `div`'s `ComputedStyle` has `background_color` == red
   AND `background_image == Some("data:image/png;base64,AAAA".into())`. **Red** against current code (today:
   `background_image` stays `None` — the declaration truncates at the `;` inside `url(...)`).
2. Test: same shape, but assert `color` ALSO resolved to green on the SAME element — proves the parser
   correctly resynced past the broken declaration instead of losing/corrupting what comes after it (the
   "narrowly scoped, not a wider parse corruption" claim from the spec's Finding 2 — verify it as an
   assertion, not just a comment).
3. Test: a deliberately-unrecognized leading token (e.g. a bare number where a property name is expected)
   immediately followed by a declaration containing a semicolon-bearing `url(...)`, asserting the SECOND
   (recognized) declaration still parses correctly — this exercises `skip_to_decl_boundary`'s copy of the fix,
   not `parse_declaration_block`'s main-loop copy; write it as a SEPARATE test, don't fold it into test 1/2
   (spec explicitly calls out both scan sites need the identical treatment).
4. Test: an UNTERMINATED `url(` (no closing paren before EOF/`}`) does not hang or panic — depth never returns
   to 0, but the scan still terminates at `*pos < len`/`RBrace`. Mirrors `tokenizer.rs`'s own "never panics on
   unterminated constructs" test (`tokenizer.rs:277-281`) at the parser layer instead.
5. Regression test: a plain declaration with NO functions/parens at all (`color: red;`) still terminates at
   its own `;` exactly as before (depth stays 0 throughout — sanity-check the common case isn't disturbed).
6. Implement (spec Task B): add `let mut depth = 0i32;` before each of the two scan loops
   (`parse_declaration_block`'s at `parser.rs:618` and `skip_to_decl_boundary`'s at `parser.rs:660`), track
   `Token::Function(_) | Token::LParen => depth += 1` / `Token::RParen => depth = (depth - 1).max(0)` inside
   each loop body, and change each loop's Semicolon/RBrace exit condition to only fire when `depth == 0`
   (exact shape in spec Task B). Apply identically to both functions — don't let them drift.
7. Green.

**Commit:** `fix(style): track paren/function depth when scanning a CSS declaration's value boundary`

---

### Task 3 — micro-fixtures, Acid2 re-bless, decisions record

**Files:** `fixtures/data-img-percent.html` (new), `fixtures/bg-image-semicolon.html` (new), `accept.sh` (two
new golden blocks, next free letters after `A5w`: `A5x`, `A5y`), `goldens/acid2-scrolled.png` (re-bless, from
Milestone A's own A5w), `DECISIONS.md`, `JOURNAL.md`.

**Steps (integration/golden work — the "test" is the CI render + pixel measurement, per AGENTS.md rule 4; land
as its own reviewable commit set, not folded into Tasks 1/2):**
1. Author `fixtures/data-img-percent.html`: `<img src="data:image/png;base64,<a small (e.g. 2×2 or 4×4),
   KNOWN-COLOR PNG whose base64 encoding contains at least one +, /, and =, with those three percent-escaped as
   %2B/%2F/%3D — compute this by hand/script from a real PNG, don't hand-type base64>">`. Add an `A5x` block to
   `accept.sh` modeled on A5n's own structure (`accept.sh:1186-1195`, the existing `data-img.html` gate):
   render, pixel-verify the decoded color/size BEFORE blessing, bless to `goldens/data-img-percent.png`.
2. Author `fixtures/bg-image-semicolon.html`: a `<div style="width:20px;height:20px;background:
   url(data:image/png;base64,<small PNG, escapes optional here — this fixture targets Task 2, not Task 1>)
   no-repeat;">` (inline `style=`, not a `<style>` block — spec's own point: proves the fix lives in the shared
   tokenizer/declaration-scan path). Add an `A5y` block to `accept.sh` modeled the same way: render,
   pixel-verify the background actually paints (not a blank/missing 20×20 box) before blessing, bless to
   `goldens/bg-image-semicolon.png`.
3. Push the branch with Tasks 1+2+the two new fixtures/gates; let `m0-acceptance` build and upload the
   `renders`/`stele-host` artifact.
4. **Before touching `goldens/acid2-scrolled.png`:** download the artifact, open `acid2-scrolled.png`, and
   pixel-measure it against BOTH of the spec's own bars: (a) the literal "ERROR" text glyph pixels are GONE
   from the `.eyes` region (compare against the CURRENT committed `goldens/acid2-scrolled.png` — text pixels
   present there, absent in the new render, in that region); (b) new eye-colored (dark/yellow-vs-transparent
   pattern per the PNGs' own known composition) pixel content appears within `.eyes`'s known approximate
   screen region. Script this as a connected-component or color-histogram check (same discipline as Milestone
   A's own A5w bar, `docs/superpowers/plans/2026-08-20-acid2-scroll-fixed-plan.md` Task 6 step 3) — do not
   eyeball it.
5. If the render does NOT show the expected change (e.g. still "ERROR" text, or a crash, or an unexpected
   shape), **stop and re-diagnose** (AGENTS.md rule 5, root-cause-first) — do not bless a render that doesn't
   match the diagnosed fix.
6. Bless `goldens/acid2-scrolled.png` (re-bless — the PR description states exactly what changed and why,
   citing this plan's Task 3 step 4 measurement, not just "CI is green").
7. Confirm NO other existing golden differs (`.forehead`/`.chin`'s backgrounds are a Task-2 side effect that
   COULD show up in any OTHER fixture/golden using `url(data:...)` in CSS — re-run the grep from the spec's
   Finding 2 section to reconfirm `fixtures/acid2.html` is still the ONLY file with that pattern; if it isn't
   anymore, check every OTHER affected golden too, don't assume acid2 is the sole blast radius).
8. Update `DECISIONS.md` (new entry, next free letter after D64 — spec's "Charter/decisions note": both fixes,
   the Acid2 eyes finding, the honest scope line) and `JOURNAL.md` (append on finishing this chunk, per
   AGENTS.md). Flag the "no charter amendment expected" judgment call in the PR description for the operator
   to confirm.

**Commit(s):** `test(data-uri): percent-escaped base64 payload golden (A5x)` + `test(style): url(data:) with an
internal semicolon in a CSS background golden (A5y)` + `fix(golden): re-bless acid2-scrolled.png — the eyes
render (Milestone B part 1)` (three separate commits — a reviewer should be able to see exactly which pixels
moved and why for the Acid2 re-bless specifically, separate from the two new micro-fixture goldens).

---

## Verify (whole plan, before opening the PR)
- `cargo test` green in CI (not locally) across all three tasks' new tests.
- `./accept.sh` green in CI, both host and i486 (`m0-acceptance`).
- Every re-blessed/new golden pixel-measured per AGENTS.md rule 4 — the PR description states WHAT was
  measured and WHY it's correct, not just "CI is green."
- `stele-i486` binary size delta reported against the 1,474,560-byte floppy ceiling (expect ~0: both fixes are
  a handful of lines in already-linked code, no new dependency).
- DECISIONS.md + JOURNAL.md updated; the "no charter amendment" judgment flagged for the operator.
- Confirm explicitly in the PR description: exact eye geometry/position, `background-position`/
  `background-attachment:fixed`, and `display:inline` cascade fidelity remain OUT of scope (Milestone B part 2,
  not this packet) — the bar met here is "two dark marks in the `.eyes` box," not a WaSP-reference byte-match.
