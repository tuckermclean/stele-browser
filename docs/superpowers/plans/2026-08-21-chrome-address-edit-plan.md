# Editable address bar + reload button — Plan · Spec: docs/superpowers/specs/2026-08-21-chrome-address-edit-design.md (read it first)

**Goal:** make the `--x11` chrome's address field click-to-edit (type a URL, Enter navigates+blurs, Esc/click-
elsewhere cancels+blurs) and add a clickable reload button that triggers the reload logic that already exists
(`XIntent::Reload`, wired to `F5` since an earlier packet).

**Architecture (one sentence per moving part, see spec for the why):** a new pure module `src/backend/
address_edit.rs` (`AddressEdit { buffer, cursor, focused }`) is the CI-tested core; `src/backend/x11.rs` gets
three small, mechanical additions — `X11Key::{Home,End,Delete}`, a `column` parameter on `keysym_for_keycode`
so Shift is finally read (it's parsed off the wire already, just discarded today), and `XIntent::Edit
(EditIntent)`; `classify_x11_intent` (`src/main.rs`) grows an `address_focused: bool` parameter that changes
how `KeyPress` is routed (edit ops vs. today's global shortcuts); `chrome::ChromeLayout` gains a `reload` rect,
`chrome::ChromeState` gains `edit: Option<(&str, usize)>`, `chrome::draw` paints the reload button and (when
focused) the live buffer + a caret instead of the static URL; `run_x11`'s `Click` arm gains address-focus/blur
and reload-button hit-tests, and gains one new `XIntent::Edit(..)` match arm.

**Global constraints (every task):** no new crate dependency; no new embedded font glyphs (the reload glyph is
`'R'`, already in the embedded ASCII range); report the `stele-i486` size delta in the PR (expected near-zero —
reconfirm current headroom from the CI artifact first, spec's Non-negotiables flags JOURNAL.md as stale); no
local `cargo build`/`cargo test` — push and read `m0-acceptance`; every code task starts with a failing test;
pixel/text-verify (never eyeball) any new/changed golden.

**Task ordering / parallelism note:** Task 1 (`AddressEdit`) has zero dependency on X11/chrome code and can run
fully in parallel with Task 2 (X11 keysym extensions: Home/End/Delete/Shift-column) — both are pure, isolated
modules. Task 3 (focus-aware `classify_x11_intent` + `XIntent::Edit`) depends on Task 2 (needs `X11Key::Home/
End/Delete` and the `column`-aware lookup) but not on Task 1. Task 4 (reload button geometry/paint) is
independent of Tasks 1-3 and can run in parallel with them. Task 5 (`ChromeState`/`draw` live-buffer + caret
rendering) depends on Task 1 (needs `AddressEdit`'s shape to decide `edit`'s tuple contents) but not Task 2/3.
Task 6 (`run_x11` wiring: focus/blur/edit-op routing, reload click) depends on ALL of Tasks 1-5. Task 7
(goldens) depends on Task 6. Task 8 (docs) lands last.

---

### Task 1 — `AddressEdit`: the pure edit-buffer core

**Files:** `src/backend/address_edit.rs` (new), `src/backend/mod.rs` (`pub mod address_edit;`).

**Interfaces:** per spec §1 — `AddressEdit { buffer: String, cursor: usize, focused: bool }` with `focus(&mut
self, seed: &str)`, `blur(&mut self)`, `insert_char(&mut self, c: char)`, `backspace`, `delete_forward`,
`move_left`, `move_right`, `move_home`, `move_end`, `commit(&mut self) -> Option<String>`.

**Failing-test-first steps:**
1. Test: `focus("http://example.test/")` sets `buffer == "http://example.test/"`, `cursor == 23` (char count),
   `focused == true`. **Red** — module doesn't exist.
2. Test: `insert_char` at various cursor positions (start, middle, end) inserts at the right spot and advances
   `cursor` by 1; a run of `insert_char` calls building up a string matches expected content byte-for-byte.
3. Test: `backspace` at `cursor == 0` is a no-op (doesn't panic, doesn't change `buffer`); `backspace` at
   `cursor == buffer.chars().count()` removes the last char and decrements cursor; a `String::new()` /
   `cursor == 0` state through several stray `backspace()` calls stays at `("", 0)`.
4. Test: `delete_forward` at `cursor == buffer.chars().count()` (end) is a no-op; at `cursor == 0` on a
   non-empty buffer removes the first char, cursor unchanged (0).
5. Test: `move_left`/`move_right`/`move_home`/`move_end` clamp correctly at both boundaries — repeated
   `move_left()` calls past 0 stay at 0; repeated `move_right()` past the end stay at the end.
6. Test: `commit()` on a non-empty, non-whitespace buffer returns `Some(trimmed)` and sets `focused = false`;
   on `""`, `"   "`, and a buffer that's ONLY whitespace after trimming, returns `None` and leaves `focused`
   unchanged (still `true` if it was `true`) — three separate cases, per spec §1's "commit empty is a no-op"
   contract.
7. Test: `blur()` sets `focused = false` regardless of buffer content; does not clear/mutate `buffer` or
   `cursor` (spec's "no restore-on-cancel logic lives here" contract — the caller relies on `ChromeState`
   ignoring `AddressEdit` entirely once unfocused, not on this struct clearing itself).
8. Test: `insert_char` rejects `char::is_control()` chars (buffer unchanged, cursor unchanged) — a defensive
   totality guard, not a live bug fix (today's only caller never produces control chars, spec §1).
9. Test: `insert_char` past `MAX_LEN` (pick and document the constant, e.g. `2048`) is silently dropped — insert
   `MAX_LEN + 50` chars in a loop, assert `buffer.chars().count() == MAX_LEN` exactly, no panic.
10. Test: a totality/fuzz-style test — a long (e.g. 500-op), seeded-random sequence of every operation
    (including interleaved `focus`/`blur`/`commit` calls) never panics and always leaves `cursor <=
    buffer.chars().count()` (assert the invariant after every single op, not just at the end).
11. Test: non-ASCII chars (e.g. `é`, `€`, a CJK char) round-trip through `insert_char`/`backspace`/cursor moves
    without landing mid-codepoint or panicking (`buffer` stays valid UTF-8 by construction since all mutation
    goes through `chars()`, but pin it with an explicit assertion `String::from_utf8(buffer.clone().into_bytes())
    .is_ok()` after each op in this test, or equivalent).
12. Implement `AddressEdit` and every method. Green.

**Commit:** `feat(backend): AddressEdit — pure, totality-tested address-bar edit buffer`

---

### Task 2 — X11 keysym extensions: Home/End/Delete + Shift column

**Files:** `src/backend/x11.rs` (`X11Key` enum, `keysym_to_key`, `keysym_for_keycode`).

**Interfaces:** `X11Key` gains `Home`, `End`, `Delete` variants. `keysym_to_key` gains three match arms
(`0xff50 => Home`, `0xff57 => End`, `0xffff => Delete`). `keysym_for_keycode`'s signature grows a `column:
usize` parameter: `pub fn keysym_for_keycode(keycode: u8, min_keycode: u8, keysyms_per_keycode: u8, keysyms:
&[u32], column: usize) -> Option<u32>` — `idx = row.checked_mul(keysyms_per_keycode as usize)? +
column`, still bounds-checked against `keysyms.len()` and `column < keysyms_per_keycode as usize`. **This is a
breaking signature change** — every existing call site (today: `classify_x11_intent`, `main.rs:2208`, plus any
existing tests in `x11.rs`) must pass `column: 0` to preserve today's behavior exactly, updated in this same
task.

**Failing-test-first steps:**
1. Test: `keysym_to_key(0xff50) == Some(X11Key::Home)`, `keysym_to_key(0xff57) == Some(X11Key::End)`,
   `keysym_to_key(0xffff) == Some(X11Key::Delete)`. **Red** — variants don't exist.
2. Test: extend `keysym_for_keycode`'s existing test table (`x11.rs:1664-1669`-area) with a 2-column keysyms
   array (e.g. keycode 8 → `['a', 'A']`, keycode 9 → `['1', '!']`) and assert `keysym_for_keycode(8, 8, 2,
   &keysyms, 0) == Some('a')`, `keysym_for_keycode(8, 8, 2, &keysyms, 1) == Some('A')`, same for keycode 9.
3. Test: `keysym_for_keycode` with `column >= keysyms_per_keycode` (e.g. `column: 5` against `keysyms_per_keycode:
   2`) returns `None`, never panics/indexes out of bounds — the same totality discipline the function already
   has for an out-of-range `keycode`.
4. Update every existing call site (production code + existing tests) to pass `column: 0`, confirming no
   observable behavior change for the unfocused/no-Shift path — this IS the regression guard for "the existing
   keyboard shortcuts still work exactly as before."
5. Implement. Green.

**Commit:** `feat(x11): Home/End/Delete keysyms + Shift-aware column lookup in keysym_for_keycode`

---

### Task 3 — Focus-aware `classify_x11_intent` + `XIntent::Edit`

**Files:** `src/backend/x11.rs` (`XIntent` enum gains `Edit(EditIntent)`, new `EditIntent` enum), `src/main.rs`
(`classify_x11_intent`'s signature and body, `main.rs:2192-2221`).

**Interfaces:** per spec §3 —
```rust
pub enum EditIntent { Insert(char), Backspace, Delete, Left, Right, Home, End, Commit, Cancel }
```
`classify_x11_intent(ev, min_keycode, keysyms_per_keycode, keysyms, height, address_focused: bool) ->
Option<XIntent>` — new trailing parameter. Also reads `state` from `XEvent::KeyPress { keycode, state }` (no
longer `{ keycode, .. }`) to compute the Shift column for Task 2's `keysym_for_keycode`.

**Failing-test-first steps:**
1. Test: with `address_focused: false`, behavior is BYTE-IDENTICAL to today for every existing case — `q`/
   `Escape` → `Quit`, arrows → `ScrollBy`, `PageUp`/`PageDown` → `ScrollBy`, `F5` → `Reload`, an unmapped key →
   `None`. **Red** only in the sense that the new parameter doesn't exist yet — this test is the regression
   guard, write it FIRST against the current (5-parameter) signature's expected behavior, then watch it
   continue to pass once the 6-parameter version lands with `address_focused: false`.
2. Test: with `address_focused: true` — `X11Key::Char('q')` → `Edit(Insert('q'))`, NOT `Quit` (the collision
   fix, spec's Goal #3 — the single most important assertion in this task). `Escape` → `Edit(Cancel)`, NOT
   `Quit`. `Enter`/`X11Key::Enter` → `Edit(Commit)`. `Backspace` → `Edit(Backspace)`. `Delete` → `Edit(Delete)`
   (Task 2's new variant). `Left`/`Right`/`Home`/`End` → `Edit(Left/Right/Home/End)` respectively. `F5` → `None`
   (NOT `Reload` — typing shouldn't trigger a reload). `Up`/`Down`/`PageUp`/`PageDown` → `None` (NOT `ScrollBy`
   — typing shouldn't scroll the document). `Tab` → `None` (spec's flagged no-op).
3. Test: a Shift-held printable key (synthesize `state` with bit 0 set, e.g. `state: 0x0001`) against a 2-column
   keysyms table produces the SHIFTED char via `Edit(Insert(...))` when focused (proves Task 2's `column`
   plumbing reaches all the way through this function, not just unit-tested in isolation at the `x11.rs`
   level).
4. Update the one production call site (`main.rs`'s event-batch loop, `main.rs:1924-1927`) to pass
   `address_edit.focused` (Task 6 wires the actual `AddressEdit` instance into scope for this — for THIS task,
   it's fine if the call site temporarily hardcodes `false` or takes a local stub, since Task 6 is what threads
   the real `AddressEdit`; note explicitly in the commit which is true).
5. Implement. Green.

**Commit:** `feat(shell): focus-aware key classification — typing in the address bar no longer triggers quit/scroll/reload shortcuts`

---

### Task 4 — Reload button: `ChromeLayout.reload` + paint

**Files:** `src/backend/chrome.rs` (`ChromeLayout`, `layout`, `draw`, `draw_reload_button`).

**Interfaces:** `ChromeLayout` gains `pub reload: Rect`, computed immediately right of `back` (spec §4);
`address`'s left bound moves to `reload.x + reload.w + GAP`. `draw` gains a call to `draw_reload_button`
(mirrors `draw_attest_button`'s exact shape: guard `w == 0 || h == 0`, fill `BUTTON_COLOR`, `draw_centered_glyph
(surface, rect, 'R', INK)`).

**Failing-test-first steps:**
1. Test (mirrors `layout_attest_button_has_nonzero_size_and_does_not_overlap_siblings`): at a normal window
   size, `reload` has nonzero width/height and does not overlap `back`, `address`, `attest`, or `throbber` (4
   pairwise checks). **Red** — field doesn't exist.
2. Test (mirrors `layout_attest_button_tiny_window_never_panics_and_stays_in_bounds`): degenerate/tiny windows
   (`0x0`, `1x1`, etc. — reuse the existing test table) never panic, `reload`'s size never exceeds the window's.
3. Test: `address`'s left bound is now `>= reload.x + reload.w` (confirms the recompute, not just that `reload`
   exists in isolation) — extend `layout_back_address_throbber_sit_inside_the_top_bar` or add a sibling
   assertion.
4. Test (mirrors `draw_attest_button_guards_zero_size_rect_without_panicking`): `draw_reload_button` on a
   zero-size rect doesn't panic.
5. Test: `draw` with a normal window paints a non-background pixel somewhere inside `lay.reload` (mirrors
   `draw_address_field_is_white_or_dark_ink_never_untouched`'s probe style) — confirms the glyph actually
   painted, not just that the rect exists.
6. Implement: the new field, its `layout()` geometry, `draw_reload_button`, wiring it into `draw`.
7. Green.

**Commit:** `feat(chrome): reload button — new ChromeLayout.reload rect + paint`

---

### Task 5 — `ChromeState.edit` + live buffer/caret rendering

**Files:** `src/backend/chrome.rs` (`ChromeState`, `draw_address`).

**Interfaces:** `ChromeState` gains `pub edit: Option<(&'a str, usize)>` (live buffer, cursor char-index).
`draw_address` branches: `Some((buf, cursor))` → draw `buf` + a caret; `None` → today's exact behavior (draw
`url`, unchanged).

**Failing-test-first steps:**
1. Test: with `edit: None`, `draw_address`'s output is pixel-identical to today's (reuse/adapt the existing
   `draw_address_field_is_white_or_dark_ink_never_untouched` and `draw_a_hostile_long_url_does_not_spill_
   outside_the_address_field` tests — they must still pass unmodified, proving zero regression for the
   unfocused case). **Red** only in the sense that the struct doesn't compile yet with the new field until
   every `ChromeState` literal in the test module adds `edit: None`.
2. Test: with `edit: Some(("http://x/", 5))`, the address field shows non-background ink for the buffer text
   (same probe style as the existing url test) AND a caret-colored pixel column is present near the expected
   x-offset (compute expected offset via `Metrics::advance_str` on the first 5 chars, same helper `draw_address`
   itself will use — assert the test's independently-computed expected offset matches where the caret pixel
   actually lands, not just "a caret exists somewhere").
3. Test: a hostile long buffer (500+ chars) with `edit: Some((long, 500))` does not spill past the address
   field's right edge (mirrors the existing hostile-long-URL test, same clip guarantee, now exercised through
   the `edit` path instead of `url`).
4. Test: `edit: Some(("", 0))` (empty buffer, freshly focused with nothing typed over a blank seed — edge case)
   still draws a caret at the field's left edge without panicking.
5. Implement: `ChromeState.edit`, `draw_address`'s branch, the caret `fill_rect` using `Metrics::advance_str`.
6. Green.

**Commit:** `feat(chrome): render the live address-edit buffer + cursor caret when focused`

---

### Task 6 — `run_x11` wiring: focus/blur, edit-op routing, reload click

**Files:** `src/main.rs` (`run_x11`, `x11_chrome_state`, the `Click` match arm `main.rs:1954-2075`, the new
`XIntent::Edit` match arm, the event-batch `classify_x11_intent` call site).

**Interfaces:** `run_x11` gains a `let mut address_edit = AddressEdit::default();` alongside `history`/`status`/
`loading`. `x11_chrome_state` (`main.rs:1730`) gains an `edit` parameter/computation: `if address_edit.focused {
Some((address_edit.buffer.as_str(), address_edit.cursor)) } else { None }`. The `classify_x11_intent` call site
passes `address_edit.focused` (closing Task 3's stub). The `Click` arm gains, in order: (a) if
`address_edit.focused` and the click is NOT inside `lay.address`, call `address_edit.blur()` before falling
into the existing back/reload/attest/viewport branches (spec §3's "blur first, then act" rule); (b) a new `else
if x11_point_in_rect(lay.reload, x, y)` branch running the same body `XIntent::Reload`'s arm runs (§4/spec's
"reuse, don't refactor" call); (c) a new `else if x11_point_in_rect(lay.address, x, y) && !address_edit.focused`
branch calling `address_edit.focus(history.current().as_str())` + a chrome-only redraw. A new `XIntent::Edit(op)`
arm dispatches to `address_edit`'s methods; `EditIntent::Commit` additionally calls `address_edit.commit()`,
and on `Some(url_str)` resolves it (spec §3 — pin the exact resolution helper at this task, document the choice
in the commit) and runs the standard navigate/load/redraw sequence.

**Failing-test-first steps (integration-shaped — `run_x11` itself is manual-verify, so these tests target the
PURE helpers this task touches, not the loop):**
1. Test: `x11_chrome_state` (or an equivalent small pure helper, if the implementer extracts one) returns
   `edit: Some(..)` when `address_edit.focused` and `edit: None` otherwise — a direct unit test of the new
   plumbing, independent of the full event loop.
2. Test: whatever URL-resolution helper Task 6 picks for `EditIntent::Commit` is unit-tested directly (e.g. if
   it reuses `resolve_url`, this is likely already covered by Task 5's/earlier `resolve_url` tests — if a new
   narrower helper is introduced instead, it gets its own test here: a bare host-shaped string, an absolute
   `http://`/`https://` string, and an empty string all resolve to something sane, never panic).
3. Implement the `run_x11` wiring described above.
4. Green (Rust unit tests for the pure pieces). **Manual-verify against a real X server** (per AGENTS.md's own
   established `run_x11`-is-manual-verify posture, spec's own CI-vs-manual split): click the address bar, type
   a URL (including at least one Shift-modified character, e.g. typing `HTTPS://EXAMPLE.COM` or a `:`/`?`), hit
   Enter and confirm navigation; repeat and hit Esc, confirm the address bar reverts to the real current URL
   with no navigation; click away instead of Esc, confirm the same cancel behavior; click the reload button and
   confirm the page reloads; press `q` while the address bar is NOT focused and confirm it still quits; type
   `q` while the address bar IS focused and confirm it inserts a `q`, does not quit.

**Commit:** `feat(shell): wire the address-bar edit buffer and reload button into run_x11`

---

### Task 7 — Goldens (`--chrome` re-bless + new focused-state golden)

**Files:** `goldens/chrome-basic.png` (existing, re-blessed), a new golden for the focused state (name/path TBD
at implementation, e.g. `goldens/chrome-address-focused.png`), `accept.sh` (the `A5u` block gets a new sibling
block for the focused variant — spec's §Testing flags **`A5y`** as the next free PNG letter, confirmed free by
grep at design time; **reconfirm at implementation time**, per the attestation-modal design's own precedent for
letter-slot collisions across parallel packets), `src/main.rs`'s `--chrome` screenshot path (needs a way to
construct a FOCUSED `ChromeState` for the new golden — e.g. a debug/test-only flag or CLI argument that seeds
`edit: Some((seed_url, cursor))`; pin the exact mechanism at this task, it's a small addition to the existing
`--chrome` screenshot code, `browser-chrome-design.md §2`).

**Steps (integration/golden work — do NOT fold into Tasks 1-6's commits, per AGENTS.md rule 4):**
1. Decide and implement the minimal `--chrome`-mode addition needed to render a focused-state screenshot
   (§Files above) — this is the one small piece of NEW production code in this task, everything else is golden
   plumbing.
2. Push the branch with Tasks 1-6 landed; let `m0-acceptance` build and upload `stele-host`/`renders`.
3. **Before blessing:** download the artifact, render both `--dump-png --chrome` (unfocused, existing fixture)
   and the new focused-state variant. Pixel-measure (not eyeball): the reload button occupies its expected
   rect and isn't blank/all-background; the focused variant shows the seeded buffer text AND a caret-colored
   column at the expected x-offset; neither render shows an obvious garbage/crash-shaped artifact.
4. Add/extend the `accept.sh` blocks (bless-if-absent + diff-if-present, mirroring `A5u`'s/`A5x`'s exact shape).
5. Bless both goldens from the CI-verified render only.
6. If either render doesn't match the read-through in step 3, stop and re-diagnose — do not bless past it.

**Commit:** `test(golden): re-bless chrome-basic for the reload button + add a focused-address-bar golden`

---

### Task 8 — Docs: `DECISIONS.md`, `JOURNAL.md`, size-delta report

**Files:** `DECISIONS.md` (new entry), `JOURNAL.md` (new entry — and note, per the spec's own Non-negotiables
finding, that JOURNAL.md is ALREADY missing entries for at least the size-squeeze-floppy packet (#76) and
possibly others that landed between the attestation-page entry and this branch — flag this staleness in the new
entry rather than silently deepening it further, mirroring how the attestation-modal packet back-filled its own
missing Terminus entry).

**Steps:**
1. `DECISIONS.md`: new entry, **D68** (reconfirm the next free letter against the file's actual state at
   implementation time — D67 is the attestation page's own entry as of this design), covering the 5 points in
   spec's own Charter/decisions note: `AddressEdit`'s module placement, the Shift-column fix + its Shift+`q`
   side effect, the cancel-via-non-restore design, the reload button reusing existing `Reload` logic, and the
   manual-verify scope for the interactive loop.
2. `JOURNAL.md`: append an entry (newest at the bottom) — what shipped, the real CI-measured `stele-i486` size
   delta (reported against the CURRENT actual headroom read from the CI artifact, not this plan's/spec's stale
   JOURNAL-derived numbers), and an explicit note flagging JOURNAL.md's own pre-existing staleness gap (found
   during this packet's ground-truthing) for whoever next has spare cycles to back-fill it — do not attempt to
   back-fill packets unrelated to this one in this same commit (scope discipline; flag, don't silently absorb
   unrelated cleanup).
3. Report the actual CI-measured `stele-i486` size delta in the PR description against the 1,474,560-byte
   floppy ceiling and whatever headroom figure Task 8 step 2 establishes as current.

**Commit:** `docs: DECISIONS/JOURNAL entries for the editable address bar + reload button`

---

## Verify (whole plan, before opening the PR)
- `cargo test` green in CI (not locally) across all eight tasks' new/updated tests.
- `./accept.sh` green in CI, both host and i486 (`m0-acceptance`), including the two golden blocks (re-blessed
  `A5u`, new `A5y` or whatever letter Task 7 actually lands on).
- Both goldens pixel-verified per AGENTS.md rule 4 — not bulk-blessed blind.
- `stele-i486` binary size delta reported in the PR against the actual, CI-reconfirmed current headroom (not
  the stale JOURNAL.md figures this plan cites for context only).
- **The collision fix confirmed by test, not assumed:** `classify_x11_intent` with `address_focused: true` maps
  `X11Key::Char('q')` to `Edit(Insert('q'))`, never `Quit` (Task 3, step 2) — this is the single assertion that
  proves Goal #3 from the spec.
- **Zero behavior change for the unfocused path confirmed by test:** Task 3 step 1's regression guard, Task 5
  step 1's unchanged-`draw_address`-output guard, Task 2 step 4's `column: 0` parity update.
- `AddressEdit` confirmed total (no panic, no OOB, cursor always `<= buffer.chars().count()`) over the Task 1
  step 10 fuzz-style test and the explicit boundary tests (steps 2-9).
- The reload button confirmed to trigger the SAME code path `F5` already does (Task 6, code-review-level check
  — not a new reload implementation) — call this out explicitly in the PR description.
- `DECISIONS.md` (new D68) + `JOURNAL.md` (new entry, plus the flagged-not-fixed staleness note) updated; the
  Shift+`q`, CapsLock, no-click-to-position-cursor, no-scroll-within-field, and URL-resolution-helper judgment
  calls all flagged explicitly in the PR description for the operator, per the spec's own Risks section.
- Confirm explicitly in the PR description: the interactive click/type loop itself (focus, typing, Enter,
  Esc, reload click) is manual-verify only (no X11 test harness in this repo) — every PURE helper it calls
  (`AddressEdit`, `classify_x11_intent`, `keysym_for_keycode`, `chrome::layout`/`draw`) IS CI-tested, consistent
  with every other `run_x11` interaction in this codebase.
