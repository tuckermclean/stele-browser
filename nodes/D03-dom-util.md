# D03 — dom-util

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01 | L05 | S |

**Owns:** `src/dom_util.rs`

## Purpose

Imported from the code. Small, crate-private DOM-walk helpers shared by the form submission serializer
(`src/form.rs`) and the form-control renderer (`src/layout/box_tree.rs`), and read by `src/browser.rs` and
`src/frames.rs`. They were factored out after two copies of the same walk drifted (`depth > CAP` in one,
`depth >= CAP` in the other). The module is `pub(crate)` only (`src/lib.rs` declares `mod dom_util`), the one
module of the library that integration tests cannot name, and it holds no submission or rendering rules.

- `DEPTH_CAP = 100`: the depth bound of every recursive walk here. `src/form.rs`, `src/browser.rs` and
  `src/frames.rs` bound their own walks with it too. Hostile markup can nest a form control's content
  thousands deep, and a native stack overflow aborts the process whatever `panic = "abort"` does.
- `is_checked(el)` and `is_disabled(el)`: whether the element carries `checked` or `disabled`, whatever the
  value (presence is what counts).
- `node_checked(dom, id)`: `dom.node(id)` for an id in range, `None` otherwise, for ids that do not come from a
  root walk (a stale activator, an id from another document). `Dom::node` itself panics on them (D01).
- `collect_options(dom, el, depth)`: every `option` under `el`, depth first, in document order, looking
  through any other element in between (`optgroup`); for each, its `value` attribute if present, its text
  trimmed, and whether it carries `selected`. Bounded by `DEPTH_CAP` from the `depth` it is given.
- `collect_text(dom, el)`: the concatenated text of every `Text` descendant of `el`, bounded by `DEPTH_CAP`.
  For a `textarea` (RCDATA in D02) it is the one decoded child; for `button` and `option` it also reads text
  inside markup.

## Tests

None in this file. The helpers are tested through their callers: `tests/form_submit.rs` (disabled, checked,
select and textarea controls, deep nesting, out-of-range ids, through `stele::form::serialize_submit`), and
the form-control rendering tests of `src/layout/box_tree.rs` and the goldens.

## Changes

none: imported

## Gate

- **D03-AC-1** — `is_disabled`: a control that carries `disabled` is left out of the form submission. *Verify:* `cargo test --locked --jobs 2 --test form_submit unnamed_disabled_and_unchecked_controls_are_excluded -- --test-threads=2` exits 0.
- **D03-AC-2** — `is_checked`: a checkbox or radio button that carries `checked` is drawn with its mark, and one without it is drawn blank. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 layout::box_tree::tests::checkbox_shows_x_when_checked_and_blank_when_not layout::box_tree::tests::radio_shows_star_when_checked_and_blank_when_not` exits 0.
- **D03-AC-3** — `node_checked` returns `None` for an out-of-range id, so a form id or activator past the arena does not panic and submits a bare GET to the base URL. *Verify:* `cargo test --locked --jobs 2 --test form_submit out_of_range_form_id_and_activator_never_panic -- --test-threads=2` exits 0.
- **D03-AC-4** — `collect_options` reports each `option`'s `value` attribute, so a select submits the value of its selected option (or of the first). *Verify:* `cargo test --locked --jobs 2 --test form_submit select_ -- --test-threads=2` exits 0.
- **D03-AC-5** — `collect_options` reports whether each `option` carries `selected`, so a select is drawn with its selected option's text. *Verify:* `cargo test --locked --jobs 2 --lib layout::box_tree::tests::select_shows_selected_option_text -- --test-threads=2` exits 0.
- **D03-AC-6** — `collect_text` returns the text of an element's `Text` children: a `textarea` is drawn with its content and a `button` with its text. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 layout::box_tree::tests::textarea_shows_short_text_verbatim layout::box_tree::tests::button_element_text_beats_value_beats_default` exits 0.
- **D03-AC-7** — `collect_options` trims each option's text. *Verify:* unpinned.
- **D03-AC-8** — `collect_options` finds `option`s inside an `optgroup`, or any other element, between the control and the option. *Verify:* unpinned.
- **D03-AC-9** — `collect_text` also reads the text inside nested markup (a `button`'s `<b>label</b>`). *Verify:* unpinned.
- **D03-AC-10** — `collect_options` and `collect_text` stop descending at `DEPTH_CAP` (100) levels. *Verify:* unpinned (no test nests a control's content that deep).

## Risks

- The helpers are tested only through `src/form.rs` and the box tree. A change here is judged by
  `tests/form_submit.rs` and the render tests, so a criterion for a new helper needs a caller or a
  public path to test it (`AGENTS.md`: a test through a private item is a question to the owner).
- `DEPTH_CAP` is shared by name with `src/form.rs`, `src/browser.rs` and `src/frames.rs`, which bound other
  walks with it, and matches `layout::box_tree`'s and `layout::block`'s own caps by value only. Changing it
  here changes all of those walks.
- `collect_text` and `collect_options` silently stop at the cap: a control nested deeper than 100 levels
  loses its options or text without an error.
- `collect_options` recurses through every non-`option` element, not only `optgroup`, so an `option` wrapped
  in any markup still counts.
