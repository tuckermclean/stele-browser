# <object> nested fallback (Acid2 Packet 6) Plan
> SUB-SKILL: superpowers:subagent-driven-development. Spec: docs/superpowers/specs/2026-08-20-object-fallback-design.md (read it).
Global: no new dep; 1.44MB floppy (report i486 delta); parse TOTAL; golden-safe (no <object> ⇒ unchanged; an
<object> w/o resolvable data renders children exactly as today); no local i486 builds; PNG goldens pixel-
verified (controller); no JS/C3.

### Task 1: fetch <object data> + render object (primary-or-fallback) — src/images.rs + src/layout/box_tree.rs + tests
Per spec §1: in `images::walk`, add an `el.name=="object"` branch fetching+decoding `el.attrs.get("data")` with
the SAME logic as the `<img src>` branch, inserting into `out` keyed by the object's NodeId. Per spec §2: in
`box_tree::build_node`'s element path (after the `is_replaced` check, do NOT add object to is_replaced), add an
`<object>` branch: if `images.get(&id)` is Some → `Replaced { intrinsic: replaced_intrinsic(el, Some(&img)),
image: Some(img) }` with empty children (fallback suppressed); else fall through to the normal element path
(renders children = fallback; nesting automatic). Tests (box_tree, hand-built images map): object-with-image →
Replaced+empty children; object-without → Container of fallback children; nested (outer no image, inner has) →
outer Container containing inner Replaced. cargo test --lib. Commit: `feat(layout): <object> renders decoded data image or nested fallback (Acid2 P6)`.

### Task 2: fixtures + accept.sh + controller bless
`object-image.html` (`<object data="data:image/png;base64,<16x16 red PNG>">FALLBACK</object>` → red square, not
"FALLBACK"), `object-fallback.html` (`<object data="data:,notanimage">fallback</object>` → word "fallback"),
`object-nested.html` (`<object data="data:,bad"><object data="data:image/png;base64,<PNG>">deep</object></object>`
→ the inner PNG). Wire accept.sh A5q/A5r/A5s (mirror A5k). Controller renders+pixel-verifies+blesses. Use the
same 16x16 red PNG base64 as fixtures/data-img.html (grep it). Commit: `test(css): <object> fallback fixtures + accept.sh (Acid2 P6)`.

### Task 3: charter + DECISIONS + JOURNAL (+ i486 size).
