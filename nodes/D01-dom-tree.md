# D01 — dom-tree

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | D02, D03, S04, S05, S06, S07, S08 | S |

**Owns:** `src/dom/mod.rs`, `src/dom/ast.rs`

## Purpose

Imported from the code. The document object model: the closed, arena-allocated tree that the parser (D02)
fills and every later stage (style, layout, forms, frames, images, the browser) reads.

- `src/dom/mod.rs` declares the public modules `ast` and `parser` and re-exports `AttrMap`, `Dom`, `Element`,
  `ElementName`, `Node` and `NodeId`, so the rest of the crate and the integration tests write
  `stele::dom::{Dom, Node, NodeId}`.
- `src/dom/ast.rs` is the covenant's enforcement point (charter C3, `SPEC.md` principle 1). `Node` has exactly
  two variants, `Element(Element)` and `Text(String)`: nothing a page becomes can carry code. accept.sh's A6
  covenant (B02) fails when the word `script`, in any case, appears in this file, so even its comments avoid
  it.
- `Dom` is a flat `Vec<Node>` arena; a node refers to others by index (`NodeId = usize`). `Dom::new()` seeds
  node 0, an `html` element, as the root. `new_element` and `new_text` allocate a node and return its id
  without attaching it; `append_child` attaches a child under an element and panics when the parent is a
  `Text` node. `node` and `node_mut` index the arena directly, so an out-of-range id panics.
- `Element` holds its `name`, `attrs` and `children` (ids, in order). `ElementName` is an interned name,
  trimmed and folded to ASCII lowercase; which tags are blocks or replaced is not known here but in the
  user-agent stylesheet and the style layer.
- `AttrMap` keeps attributes in source order. `get` compares names ignoring ASCII case. `set` is the
  parse-time tag-soup rule: the first value for a name wins and later ones are ignored. `set_overwrite`
  replaces an existing value (case-insensitively) or appends a new pair. Names are stored trimmed and
  lowercased.
- `Dom::set_attribute(id, name, value)` is `set_overwrite` on one element, and a no-op, never a panic, when
  `id` is out of range or names a `Text` node. `main.rs`'s `stamp_color_scheme` uses it to stamp
  `data-theme`/`data-mode` on the root before the cascade.

## Tests

The inline module `dom::ast::tests` in `src/dom/ast.rs` (8 tests: the arena, `AttrMap::set`,
`AttrMap::set_overwrite`, `Dom::set_attribute`). The integration tests import the re-exports
(`tests/form_submit.rs` uses `stele::dom::{Dom, Node, NodeId}`). The A6 covenant grep is in `accept.sh`
and runs only in CI's `accept` job.

## Changes

none: imported

## Gate

- **D01-AC-1** — `ElementName::new` folds a name to ASCII lowercase, and the arena reads back the tree built with `new_element`, `new_text` and `append_child` under the `html` root. *Verify:* `cargo test --locked --jobs 2 --lib dom::ast::tests::arena_builds_and_reads_back_a_small_tree -- --test-threads=2` exits 0.
- **D01-AC-2** — `AttrMap::get` finds a name in any ASCII case, and `AttrMap::set` keeps the first value for a name and ignores later ones. *Verify:* `cargo test --locked --jobs 2 --lib dom::ast::tests::attrs_are_case_insensitive_and_first_wins -- --test-threads=2` exits 0.
- **D01-AC-3** — `AttrMap::set_overwrite` replaces an existing value whatever the case of the name, without adding a second pair, and appends a pair when the name is absent. *Verify:* `cargo test --locked --jobs 2 --lib dom::ast::tests::set_overwrite -- --test-threads=2` exits 0.
- **D01-AC-4** — `Dom::set_attribute` overwrites an element's attribute or adds it, does not panic on an out-of-range id, and leaves a `Text` node as it was. *Verify:* `cargo test --locked --jobs 2 --lib dom::ast::tests::dom_set_attribute -- --test-threads=2` exits 0.
- **D01-AC-5** — `stele::dom` re-exports `Dom`, `Node` and `NodeId` (with `AttrMap`, `Element`, `ElementName`) and the public module `parser`. *Verify:* `cargo test --locked --jobs 2 --test form_submit -- --test-threads=2` exits 0.
- **D01-AC-6** — `Node` has only the variants `Element` and `Text`, and `src/dom/ast.rs` does not contain the word `script` in any case. *Verify:* unpinned (accept.sh's A6 covenant line, which runs only in CI's `accept` job).
- **D01-AC-7** — `append_child` panics when the parent is a `Text` node; `node` and `node_mut` panic on an out-of-range id. *Verify:* unpinned.

## Risks

- The covenant grep is textual. Adding the word to a comment or a doc-comment in `src/dom/ast.rs` fails
  CI's `accept` job even though no variant changed, and a new variant named otherwise is caught only by
  review. A node that touches this file keeps `Node` to two variants (`SPEC.md` principle 1).
- `node()` and `node_mut()` panic on an out-of-range id, and the binary is built with `panic = "abort"`.
  Callers that hold ids not taken from a root walk go through `dom_util::node_checked` (D03).
- `AttrMap` is a `Vec` with a linear, case-folding lookup: cheap for the few attributes of 1996 markup, but
  every lookup is O(attributes).
- `Dom::default()` is `Dom::new()`, so `is_empty()` is never true for a `Dom` built through the API.
- Every module of the crate imports these types; a change to their shape reaches style, layout, forms,
  frames, images and the browser.
