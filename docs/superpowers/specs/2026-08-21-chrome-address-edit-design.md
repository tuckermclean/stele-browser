# Editable address bar + reload button — design

**Date:** 2026-08-21 · Packet: `packet/chrome-address-edit`. Fills the two items the browser-chrome design
explicitly deferred (`docs/superpowers/specs/2026-08-20-browser-chrome-design.md`, "Out of scope"): *"Editable
address bar / typing a URL to navigate"* and *"reload button"* — NOT forward/tabs/bookmarks/scrollbar chrome,
still out of scope. **This document is spec + plan only — no implementation.** Companion plan:
`docs/superpowers/plans/2026-08-21-chrome-address-edit-plan.md`.

## Goal
1. Click the `--x11` chrome's address field → it becomes editable (seeded with the current URL, cursor at the
   end); type to edit; **Enter** commits (navigate + blur); **Esc** or a click elsewhere cancels (discard edits,
   blur, no navigation).
2. A new **reload** chrome button, clickable, next to `back`.
3. Ship this without regressing the one keyboard shortcut collision it creates: `q` currently quits
   (`classify_x11_intent`, `main.rs:2210`) — while the address bar is focused, `q` must type a `q`, not quit.

## The pivotal finding: keyboard input is NOT being built from scratch
The brief asked me to determine whether `run_x11` handles `KeyPress`/keycode decoding **at all** today, on the
assumption that if not, building it is this packet's bulk. **It already exists, fully, from an earlier packet**
(the `run_x11` doc comment and a code comment both say `packet/shell-keyboard`, `main.rs:2223`) — this
completely changes the packet's shape from "build X11 keyboard input" to "extend an existing, already-tested
keyboard pipeline for text editing + fix one real gap it has (Shift is parsed but never used)". Ground-truthed,
not assumed:

- **`XEvent::KeyPress { keycode: u8, state: u16 }`** is parsed in `parse_event` (`src/backend/x11.rs:707`,
  event code 2) from the wire today, unconditionally, whenever the window has focus (`SetInputFocus` is already
  sent on the first `Expose`, `main.rs:1855-1858`, `2143-2151`).
- **`GetKeyboardMapping` is already requested once at startup** (`main.rs:1862`, `encode_get_keyboard_mapping`/
  `parse_keyboard_mapping_reply` in `x11.rs:553-582`) and the reply's `(keysyms_per_keycode, keysyms)` are held
  for the whole session; a server that fails the request degrades to an inert-but-still-running keyboard
  (`main.rs:1863-1865`), not a crash.
- **`keysym_for_keycode(keycode, min_keycode, keysyms_per_keycode, keysyms) -> Option<u32>`** (`x11.rs:588-595`)
  is total (bounds-checked, `None` never panics) — but it **always reads column 0** (`idx = row *
  keysyms_per_keycode`, no column offset added). This is the one real gap: **Shift is decoded off the wire
  (`state: u16`) but discarded** — `classify_x11_intent` destructures `XEvent::KeyPress { keycode, .. }`
  (`main.rs:2207`, note the `..`) and never reads `state`. Today, physically holding Shift while typing has
  *zero* effect; only the unshifted (column-0) keysym is ever looked up.
- **`keysym_to_key(keysym) -> Option<X11Key>`** (`x11.rs:627-643`) already maps the FULL printable-ASCII range
  `0x20..=0x7e` 1:1 to `X11Key::Char(char::from_u32(keysym))`, plus `Enter`/`Backspace`/`Tab`/`Escape`/arrows/
  `PageUp`/`PageDown`/`F5`. **This needs zero changes for shifted symbols**: X11 keysyms for shifted printable
  characters (`:`, `?`, `&`, `@`, `_`, `"`, …) are themselves already in `0x20..=0x7e` (X11 keysym values for
  Latin-1-range printables are numerically identical to the character's code point) — the ONLY missing piece is
  *looking up the shifted column* in the first place, not decoding what comes back.
- **`X11Key` has no `Home`/`End`/`Delete`** (`x11.rs:606-619`) — needed for the edit-buffer's cursor-to-start/
  end and forward-delete, a small, mechanical addition (standard `keysymdef.h` values, same pattern as every
  existing arm).
- **`XIntent::Reload` already exists and is already fully wired end-to-end** (`x11.rs:731`, `main.rs:2076-2105`):
  `F5` → `classify_x11_intent` (`main.rs:2215`) → `XIntent::Reload` → the SAME `load_x11_page(history.current(),
  width)` / redraw sequence every other navigation uses. **The reload button this packet adds is a discoverable
  trigger for logic that already exists and already works — not new reload logic.** The only new code for
  reload is: a `ChromeLayout.reload` rect, its paint, and one more `x11_point_in_rect` branch in the `Click`
  arm (`main.rs:1954-2075`) that runs the *same* body `XIntent::Reload`'s arm already runs (see §4 — this
  packet duplicates that ~12-line block rather than restructuring the match, matching how `back`/`attest`/the
  viewport-link branch already each duplicate their own load/redraw sequence today; refactoring that shared
  shape is explicitly out of scope, not this packet's problem to fix).
- **`classify_x11_intent` is a pure, already-unit-tested function of `(event, min_keycode, keysyms_per_keycode,
  keysyms, height)`** with NO knowledge of chrome/focus state (`main.rs:2192`). Making keys route differently
  while the address bar is focused means this function's signature must grow a focus parameter (§3).

**Net effect on scope:** no new X11 protocol code, no new keycode↔keysym plumbing. The packet is: (a) a pure,
CI-tested `AddressEdit` struct (the real design work), (b) a small, mechanical extension of the existing keysym
table (Home/End/Delete, and reading the Shift bit), (c) threading focus state into `classify_x11_intent` and a
handful of new `XIntent` variants, (d) one new `ChromeLayout` rect + paint for reload, (e) `chrome::draw`
rendering a live buffer + caret instead of the static URL when focused, (f) a re-blessed `chrome-basic`-family
golden.

## Non-negotiables (AGENTS.md, unchanged by this packet)
- **No JavaScript, by construction** (C3) — this is chair/interaction surface (C5), not new DOM/CSS dialect; no
  charter amendment expected (§Charter note).
- **1.44 MB floppy ceiling.** Per JOURNAL.md's last size entry, `stele-i486` was **1,377,436 B (97,124 B
  headroom)** before the attestation-page packet (~24-28 KB estimated) and at least one subsequent size-squeeze
  pass (`0276f6c size: fit the i486 binary under the 1.44MB floppy via immediate-abort (#76)`, in this branch's
  own recent git log) landed on top — **JOURNAL.md was not updated for either**, so its numbers are stale.
  **Reconfirm the real current size/headroom from the CI `stele-i486` artifact before estimating this packet's
  price**, don't trust this document's numbers. This packet's own cost should be near-zero: no new crate, no
  new embedded asset/font glyphs (§4's reload glyph reuses an already-embedded ASCII character), a few hundred
  bytes of new Rust logic (`AddressEdit`, the keysym table additions, the new match arms). Report the measured
  delta in the PR regardless of how small.
- **CI-driven build/test.** No local `cargo build`/`cargo test`; push and read `m0-acceptance`; bless goldens
  only from the downloaded CI artifact, pixel-verified first.
- **Test-first.** Every code task starts with a failing test.
- **Golden discipline.** The `--chrome` golden (`goldens/chrome-basic.png`, accept.sh `A5u`) will change shape
  (new `reload` rect) and this packet likely adds a second `--chrome` variant golden showing the FOCUSED state
  — both re-blessed only after pixel-measuring the CI render, never rubber-stamped.

## Design

### 1. `AddressEdit` — the pure, CI-testable edit-buffer core
**New file, `src/backend/address_edit.rs`** (a sibling pure module to `chrome.rs`/`x11.rs`, registered in
`src/backend/mod.rs`'s `pub mod` list). Deliberately NOT added to `chrome.rs` — that module's own doc comment
scopes it to "pure layout + drawing, no event handling... no history/animation state" (`chrome.rs:1-9`);
`AddressEdit` is edit STATE, the same category as `browser::History`, just x11-shell-local rather than shared
with the tty browser (the tty browser has no address bar). Cursor indexes by **char count, not byte offset**
(URLs are near-universally ASCII, but the struct must be total over any `char`, including pasted/typed non-ASCII
punctuation — indexing by chars avoids ever landing mid-UTF-8-codepoint):

```rust
#[derive(Debug, Clone, Default)]
pub struct AddressEdit {
    pub buffer: String,
    pub cursor: usize,   // char index into `buffer`, always in 0..=buffer.chars().count()
    pub focused: bool,
}

impl AddressEdit {
    pub fn focus(&mut self, seed: &str) { .. }      // buffer = seed, cursor = end, focused = true
    pub fn blur(&mut self) { self.focused = false; } // discard is implicit -- see below
    pub fn insert_char(&mut self, c: char) { .. }    // ignores control chars except handled separately (Tab/etc. never reach this -- see classify_x11_intent's routing, §3); caps buffer length (MAX_LEN, defensive against stuck-key/hostile repeat)
    pub fn backspace(&mut self) { .. }               // no-op at cursor == 0
    pub fn delete_forward(&mut self) { .. }          // no-op at cursor == buffer.chars().count()
    pub fn move_left(&mut self) { .. }                // clamp at 0
    pub fn move_right(&mut self) { .. }               // clamp at len
    pub fn move_home(&mut self) { self.cursor = 0; }
    pub fn move_end(&mut self) { self.cursor = self.buffer.chars().count(); }
    pub fn commit(&mut self) -> Option<String> { .. } // see below
}
```

- **`focus(seed)`**: called on an address-rect click (or, later, a keyboard focus shortcut — not built here).
  Seeds `buffer` from the CURRENT navigated URL (`history.current().as_str()`, supplied by `run_x11`, not by
  this struct — `AddressEdit` never touches `Url`/`History`, keeping it a pure string/cursor machine, easy to
  unit-test without pulling in `fetch::Url`). Cursor starts at the end (common convention; select-all-on-focus
  is a plausible alternative but adds a "selection range" concept this MVP doesn't need — flagged as a
  judgment call, not built).
- **`insert_char`**: appends at `cursor`, `cursor += 1`. Filters to **printable, non-control** chars
  (`!c.is_control()`) — defends against a hostile/misbehaving `keysym_to_key` mapping or a future caller feeding
  it a stray control code; today's `keysym_to_key` only ever emits `0x20..=0x7e` printables via the `Char`
  variant so this is a belt-and-suspenders totality guard, not a live bug fix. **Caps `buffer` at a fixed
  `MAX_LEN` (e.g. 2048 chars)** — a stuck key / key-repeat storm must not grow the buffer unboundedly; inserts
  past the cap are silently dropped (still total, no panic, no reallocation blowup). This is the totality
  contract the unit tests pin (§Testing).
- **`commit()`**: trims `buffer`. If the trimmed result is **non-empty**, returns `Some(trimmed.to_string())`
  AND sets `focused = false` (Enter navigates and blurs, in one step). If the trimmed result is **empty/
  whitespace-only**, returns `None` and **leaves `focused` unchanged (stays true)** — Enter on an empty/
  blank address bar is a no-op, not a crash, not an accidental blur-with-nothing-to-navigate-to (mirrors how
  most browsers treat an empty address-bar Enter). `run_x11` only calls `history.navigate(...)` when `commit()`
  returns `Some`.
- **`blur()`**: sets `focused = false`. **No restore-on-cancel logic lives here** — and deliberately so: when
  `focused` is `false`, `chrome::draw` renders `ChromeState.url` (the real current URL, from `history.current()`,
  unaffected by whatever was mid-edit), not `AddressEdit.buffer`. So "Esc cancels" doesn't need `AddressEdit` to
  remember and restore an original string — the source of truth for the UNFOCUSED display was never touched.
  `run_x11`'s Esc/blur-elsewhere handling is simply "call `blur()`, do NOT call `commit()`" (§3). This is a
  meaningfully simpler contract than a struct that has to track "original vs. edited" internally.
- **Totality:** every operation is defined for every `(buffer, cursor)` state reachable by any sequence of
  calls — `cursor` is re-clamped to `0..=chars().count()` at the top of every mutating method (defensive against
  a future caller misusing the API, even though today's only caller, `run_x11`, always calls through the public
  methods that already maintain the invariant). No method can panic, index out of bounds, or split a UTF-8
  codepoint (all indexing goes through `chars()`/`char_indices()`, never raw byte slicing on a char-index).

### 2. X11 keyboard extension (mechanical, small — see §"pivotal finding" above)
**`src/backend/x11.rs`:**
- **`X11Key` gains `Home`, `End`, `Delete`** (`x11.rs:606-619`), mapped from `0xff50`, `0xff57`, `0xffff`
  respectively in `keysym_to_key` (`x11.rs:627-643`) — same pattern as every existing arm, no new concept.
- **Shift support: `keysym_for_keycode` gains a `column: usize` parameter** (or a new sibling function,
  implementer's call, e.g. `keysym_for_keycode_col`) — `idx = row * keysyms_per_keycode + column`, still
  bounds-checked (`None` if `column >= keysyms_per_keycode` or the computed index is out of range), still total.
  `classify_x11_intent` computes `column = if state & 0x0001 != 0 { 1 } else { 0 }` (X11's `ShiftMask` is bit 0
  of the `KeyButMask` the protocol already puts in `state`, per the X11 protocol spec — `state` has been parsed
  off the wire since `packet/shell-keyboard`, just unused until now) and passes it through. **CapsLock
  (`LockMask`, bit 1) is explicitly NOT handled** — only the live Shift key state is read; typing a URL on a
  486 floppy browser with CapsLock engaged producing wrong-case letters is an acceptable, flagged simplification
  (§Risks), not a blocker — real X servers don't swap columns for CapsLock the same way they do for letters
  anyway without locale-aware `XLookupString`, which this from-scratch client deliberately doesn't implement
  (matches the existing "minimal hardcoded printable-ASCII table, not full XKB" posture the packet brief itself
  suggested as acceptable).
- **One existing-behavior nuance, flagged not silently changed:** today, `state` is ignored, so physically
  holding Shift while pressing `q` still reads column 0 and quits (Shift has no effect at all currently). After
  this change, Shift+`q` looks up column 1 (`Q`, keysym `0x51`), which does **not** match `X11Key::Char('q')` in
  `classify_x11_intent`'s `Quit` arm (`main.rs:2210`) — so **Shift+Q stops quitting**. This is a minor, correct
  side effect of actually reading Shift for the first time, not a regression in anything this packet is asked
  to preserve; noted for the PR description.

### 3. Focus-aware `classify_x11_intent` + new `XIntent` variants
**`src/backend/x11.rs`'s `XIntent` enum** (`x11.rs:726-733`) gains edit-oriented variants, grouped under one
carrier to keep the enum's match arms manageable:
```rust
pub enum XIntent {
    ScrollBy(i32),
    Resize { w: u16, h: u16 },
    Expose { x: u16, y: u16, w: u16, h: u16 },
    Click { x: i16, y: i16 },
    Reload,
    Quit,
    Edit(EditIntent),   // NEW
}
pub enum EditIntent {
    Insert(char),
    Backspace,
    Delete,
    Left, Right, Home, End,
    Commit,
    Cancel,
}
```
**`classify_x11_intent`'s signature grows one parameter: `address_focused: bool`** (`main.rs:2192`) — sourced
from `run_x11`'s own `AddressEdit.focused` field, read fresh each batch (classification already happens once
per event batch inside the loop, `main.rs:1924-1927`, so this is a cheap, already-established pattern, not a
new kind of state threading). Behavior:
- **`address_focused == false`** (today's behavior, unchanged): `Escape`/`q` → `Quit`, arrows → scroll,
  `PageUp`/`PageDown` → scroll, `F5` → `Reload`, everything else → `None`. **Zero behavior change for the
  unfocused case** except the Shift-Q nuance above.
- **`address_focused == true`**: the SAME `KeyPress` arm branches differently —
  `X11Key::Char(c) => Some(XIntent::Edit(EditIntent::Insert(c)))`, `Backspace => Edit(Backspace)`, `Delete =>
  Edit(Delete)`, `Left/Right/Home/End => Edit(...)`, `Enter => Edit(Commit)`, `Escape => Edit(Cancel)`, and
  **everything else `None`** — critically, `q` while focused does NOT quit (it's `X11Key::Char('q')`, routed to
  `Insert('q')`), and `F5`/arrows/PageUp/PageDown are swallowed (no scroll, no reload) rather than leaking
  through to the unfocused behavior. `Tab` is mapped to `None` in both states for this MVP (no multi-field
  focus cycling exists — there's only one focusable field — so Tab has nothing to do; flagged, not built).
- **Click routing to focus/blur lives in `run_x11`, not `classify_x11_intent`** (consistent with how `back`/
  `attest`/viewport hit-testing already work — `classify_x11_intent` only ever produces the coordinate-carrying
  `XIntent::Click`, and `run_x11`'s `Click` arm does the actual rect hit-testing against `chrome::layout`,
  `main.rs:1954-2075`). New behavior added to that same `Click` arm:
  - Hit `lay.address` **while not focused** → `address_edit.focus(history.current().as_str())`. Redraw (address
    field now shows the live buffer + caret).
  - Hit `lay.address` **while already focused** → no-op for this MVP (no click-to-position-cursor; a real
    browser lets you click mid-text to move the caret there, but that needs per-glyph hit-testing inside the
    field — flagged as a reasonable, explicit scope cut, not built; typing still works, arrow keys still move
    the cursor).
  - Hit **anywhere else** (back/reload/attest/viewport, or dead chrome space) **while `address_edit.focused`**
    → `address_edit.blur()` FIRST (cancel, no commit), THEN proceed with whatever that click normally does
    (back-navigate, reload, follow a link, etc.) — matches the brief's "clicking elsewhere/viewport blurs" and
    reads as the least-surprising behavior (a click that both cancels the edit AND performs its own action,
    rather than the first click being "swallowed" just to blur).
- **`XIntent::Edit(op)`'s own match arm in `run_x11`** (new, alongside `Reload`/`Click`/etc., `main.rs:1930+`):
  routes to `address_edit`'s methods directly for `Insert`/`Backspace`/`Delete`/`Left`/`Right`/`Home`/`End`
  (each followed by a redraw of ONLY the chrome bars — a full `x11_full_redraw` is fine for a first cut given
  the address field's tiny pixel area, though implementer may choose a cheaper partial blit if `x11_scroll_to`'s
  band-blit machinery is easy to reuse; not mandated here, correctness first). `Cancel` → `address_edit.blur()`
  + redraw. `Commit` → `address_edit.commit()`; if `Some(url_str)`, resolve it (`history.current().resolve(...)`
  is for in-page relative hrefs — a typed address-bar URL should instead go through the same "make this a real
  absolute URL" handling `resolve_url` (`main.rs:380-392`) already gives CLI input; reuse `resolve_url`, not
  `Url::new` directly, so typing a bare `example.com`-shaped string gets the same passthrough/fallback treatment
  as any other Stele entry point — an implementation-time call on exactly which resolution helper applies,
  flagged, not fully pinned here) then `history.navigate(...)` + the standard load/redraw sequence (identical
  shape to the `Click`→viewport-link branch, `main.rs:2036-2069`).

### 4. Reload button — `ChromeLayout`/`chrome::draw`
**`src/backend/chrome.rs`**: `ChromeLayout` gains `pub reload: Rect`, positioned **immediately right of `back`**
(both are "page navigation" actions, grouped left, matching the back-button-then-address reading order most
browsers use) — `address`'s left bound moves from `back.x + back.w + GAP` to `reload.x + reload.w + GAP`. Sized
like `attest`/`throbber` (20px square, same `saturating_sub`/`.min()` clamp discipline every existing field
already uses — `layout`'s whole body is already written to degrade tiny/`0x0` windows to zero-size rects without
underflow, `chrome.rs:57-103`; `reload` follows the exact same recipe as `attest`'s own addition did).
`chrome::draw` gains `draw_reload_button` (mirrors `draw_attest_button`'s shape exactly: guard zero-size, fill
`BUTTON_COLOR`, `draw_centered_glyph`). **Glyph: `'R'`** — a plain, already-embedded ASCII capital letter (zero
new glyph-atlas bytes, unlike a hypothetical `↻`/`⟳` which aren't in Terminus's embedded ASCII+Latin-1 subset,
`0x20-0x7E`+`0xA0-0xFF` per the Terminus packet's own scope, D66) — the leanest option that's still legible and
consistent with `back`'s `'<'` and `attest`'s `'©'` precedent (single centered glyph in a small box, no new
drawing primitive). **Click wiring in `run_x11`**: one more `x11_point_in_rect(lay.reload, x, y)` branch in the
`Click` arm, running the identical load/redraw body `XIntent::Reload`'s own arm already runs (§"pivotal
finding" — code is duplicated here on purpose, matching this file's own established per-branch style, not
refactored into a shared helper by this packet).

### 5. Rendering — live buffer + caret when focused
**`chrome::ChromeState`** (`chrome.rs:107-118`) gains what `draw` needs to render the focused state — the
leanest shape is two new optional-by-convention fields rather than restructuring `url: &str` into an enum
(keeps every existing call site, e.g. `x11_chrome_state`, `main.rs:1730`, mostly unchanged):
```rust
pub struct ChromeState<'a> {
    pub url: &'a str,           // unchanged: the CURRENT (unfocused) URL
    pub edit: Option<(&'a str, usize)>,  // NEW: Some((live buffer, cursor char-index)) while focused, else None
    pub status: &'a str,
    pub loading: bool,
    pub throbber_frame: u8,
    pub can_go_back: bool,
}
```
`draw_address` (`chrome.rs:181-187`) branches: `Some((buf, cursor))` → draw `buf` (same
`draw_left_aligned_clipped` call, reusing all existing clip/truncation totality) THEN a **caret**: a 1-2px-wide
vertical `fill_rect` at `rect.x + PAD_X + Metrics::advance_str(&font, &buf[..byte_offset_of(cursor)], TEXT_SIZE_PX)
as i32`, full field height, `INK` color — `Metrics::advance_str` (`src/text/mod.rs:43-46`, already used
transitively via `TerminusFont`'s `Metrics` impl) gives the exact pixel offset for a monospace bitmap font
without hardcoding a cell width, matching how `draw_centered_glyph` already leans on `Metrics::ascent`/
`line_height` rather than a magic number for vertical placement. `None` → today's exact behavior (draw
`st.url`), **zero rendering change for the unfocused case** — the golden's UNFOCUSED chrome frame should be
byte-identical to before except for the new `reload` button's pixels. **Known, flagged simplification:** no
horizontal scroll-within-field when the live buffer is wider than the address rect — `set_clip` already
prevents overflow-painting (same totality guarantee `draw_left_aligned_clipped` always had), but a caret past
the visible clip edge is simply invisible rather than the field auto-scrolling to keep it in view. Acceptable
for a first cut (typed URLs are usually short; a long one is still editable, just not always visible past the
clip) — not built here, flagged for a later packet if it proves annoying in practice.

### 6. Chrome layout — final left-to-right order
`back` (nav) · `reload` (nav, NEW) · `address` (grows/shrinks to fill remaining space, now editable) · `attest`
(about:attestations) · `throbber` (load indicator). No rect changes for `attest`/`throbber`/`viewport`/`status`
beyond `address`'s left-bound recompute (§4). Every rect keeps the same non-overlap + degenerate-window-clamp
unit tests the existing fields already have (`chrome.rs`'s test module, `layout_attest_button_*`-shaped tests
are the direct template for `reload`'s own).

## What's CI-testable vs. manual-verify — the load-bearing split (per the browser-chrome design's own precedent)
**CI-testable (unit + golden):**
- `AddressEdit`'s entire operation set — insert/backspace/delete/cursor-move/commit/blur over hostile sequences
  (empty commits, whitespace-only commits, backspace-at-0, delete-at-end, insert-past-`MAX_LEN`, non-ASCII
  chars, a long randomized op sequence proving no panic/OOB ever) — this is the packet's real testable core,
  pure Rust, no X11/`Surface` involved at all.
- `keysym_to_key`'s new `Home`/`End`/`Delete` arms; `keysym_for_keycode`'s new `column` parameter (unshifted
  and shifted lookups, out-of-range column, `column >= keysyms_per_keycode`).
- `classify_x11_intent`'s focus-aware branching — a pure function, fully unit-testable by constructing raw
  `XEvent::KeyPress`/`ButtonPress` values and asserting the `XIntent` (or `None`) it produces for both
  `address_focused` states, exactly like its existing test coverage already does for the unfocused case.
- `ChromeLayout`'s new `reload` rect geometry (non-overlap with `back`/`address`/`attest`/`throbber`, tiny-
  window clamp) — same shape as `attest`'s own tests.
- `chrome::draw`'s reload-button paint and the focused-address caret/buffer rendering, via `MemSurface` pixel
  probes (bar-color-outside-widget, non-background-inside-widget, hostile-long-buffer-doesn't-spill) — same
  probe style `chrome.rs`'s existing test module already uses for `address`/`status`.
- A `--dump-png --chrome` golden re-bless (`goldens/chrome-basic.png`, `A5u`) for the new reload button in the
  UNFOCUSED state, plus a **second, NEW** `--chrome` golden variant exercising the FOCUSED state (needs a way
  to synthesize a focused `ChromeState` from the CLI screenshot path, `main.rs`'s `--chrome` flag, §Plan Task
  for the exact mechanism — e.g. a hidden/debug flag or simply constructing the `ChromeState` with `edit:
  Some((seed, cursor))` directly in that code path for the golden's sake, since the screenshot mode already
  hand-builds a `ChromeState`, `browser-chrome-design.md §2`).

**Manual-verify only (no CI, matches every other `run_x11` interaction in this codebase, per the browser-chrome
design's own established line):**
- Actually clicking the address field with a mouse and seeing it focus.
- Actually typing on a real keyboard (including Shift-modified symbols) and seeing characters appear, cursor
  move, caret render in the right place live.
- Enter navigating, Esc canceling, clicking elsewhere blurring — the full interactive loop.
- Actually clicking the reload button and seeing the page reload (the RELOAD LOGIC itself, `XIntent::Reload`'s
  body, was already manually verified when `F5` shipped — this packet only adds a second manual-verify TRIGGER
  for it, the underlying behavior is not new).

## Testing / fixtures (see plan for full task-by-task breakdown)
- `AddressEdit` unit tests (new `#[cfg(test)] mod tests` in `address_edit.rs`): every op, boundary conditions,
  a totality/fuzz-style test running a long random op sequence and asserting `cursor` never exceeds
  `buffer.chars().count()` and the function never panics.
- `x11.rs` unit tests: `Home`/`End`/`Delete` keysym mapping; `keysym_for_keycode` with `column=1` (shifted)
  against a synthetic keysyms table (mirrors the existing `parse_keyboard_mapping_reply_extracts_keysyms`/
  `keysym_for_keycode_indexes_correctly` tests, `x11.rs:1640-1669`, extended for a 2-column shifted case).
- `main.rs` unit tests (extending the existing `classify_x11_intent` coverage, if any exists there today —
  confirm at implementation time and match its style): focused vs. unfocused routing for `q`/`Escape`/`Enter`/
  arrows/`F5`/a printable char.
- `chrome.rs` unit tests: `reload` rect geometry (mirrors `layout_attest_button_*`), `draw_reload_button`
  zero-size guard, focused-buffer + caret rendering (buffer text visible, caret pixel present at the expected
  x-offset for a known short buffer + cursor position, a hostile long buffer doesn't spill past the clip).
- Golden: `goldens/chrome-basic.png` (`A5u`) re-blessed (reload button added, pixel-measured before blessing);
  a new focused-state golden (naming/slot TBD — the PNG series `A5` is at `A5x` currently, per the attestation-
  modal design's own ground-truthing, so this packet's slot is **`A5y`**, confirmed free by grep at design
  time, reconfirm at implementation time in case another packet lands first).

## Risks / unknowns not fully grounded here
- **Shift+`q` no longer quits** (§2) — a correct, minor side effect of actually reading the Shift bit for the
  first time; flagged for the PR, not a blocker.
- **CapsLock is not handled** — only live Shift state selects the keysym column; typing with CapsLock engaged
  will type the wrong case. Flagged as an accepted MVP simplification (§2), revisit if it proves genuinely
  annoying (would need real XKB locale state, a much bigger lift for a browser this size-constrained).
- **No click-to-position-cursor inside the address field, no horizontal scroll-within-field for long buffers,
  no Tab-based focus cycling** (§3, §5) — each flagged individually above as an explicit, bounded scope cut for
  this MVP, not fabricated defaults.
- **Exact URL-resolution helper for a typed/committed address-bar string** (§3, Commit handling) — whether it
  should go through `resolve_url` (the CLI-entry-point passthrough, `main.rs:380-392`) or a different/narrower
  helper is an implementation-time call, not pinned in this document; whichever is chosen must be unit-tested
  the same way `resolve_url`'s own `about:` passthrough was (`main.rs:3075`-area test, per the attestation-modal
  plan's own precedent).
- **The exact current `stele-i486` size/headroom is unconfirmed** (§Non-negotiables) — JOURNAL.md is stale
  against at least two packets that already landed on this branch's own git history. Reconfirm from the CI
  artifact before reporting this packet's delta; the delta itself should be small regardless (no new embedded
  assets, no new crate).
- **Whether a full chrome redraw per keystroke is fast enough to feel responsive** is not measured here (no
  perf number available without running the interactive shell, which is itself manual-verify-only) — flagged
  as an implementation-time judgment call; a cheaper partial-blit path is a valid follow-up if it feels laggy,
  not a blocker for landing correctness first.

## Charter / decisions note
This is a **C5 (chair / interactive shell) feature**, the same category the browser-chrome design itself
established (back/throbber/history) and the attestation-page packet's own chrome affordance followed — **no
`stele-charter.md` "What Stele Speaks" amendment expected** (no new DOM/CSS vocabulary; the address bar edits a
URL string, it doesn't change what documents can express). Record a new `DECISIONS.md` entry, **next free
letter D68** (confirmed by scanning the file at design time — D67 is the attestation page's own entry;
reconfirm at implementation time in case another packet's entry lands first), covering: (1) `AddressEdit` as a
separate pure module rather than folded into `chrome.rs` or `browser.rs`, and why; (2) the Shift-column fix and
its Shift+`q` side effect; (3) the cancel-via-non-restore design (unfocused rendering always reads
`history.current()`, never a "remembered original" inside `AddressEdit`); (4) the reload button reusing
`XIntent::Reload`'s existing logic rather than introducing new reload machinery; (5) the manual-verify scope for
the interactive click/type loop, mirroring the browser-chrome design's own established split.
