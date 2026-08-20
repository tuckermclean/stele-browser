# `<object>` with nested fallback (Acid2 Packet 6) — design

**Date:** 2026-08-20 · **Status:** approved design · **Program:** Acid2 roadmap Packet 6 of 7.

## Goal
Render `<object data="…">` as its **primary representation** (the decoded `data` image) when that resolves,
else render its **fallback content** (the nested child boxes) — the classic HTML `<object>` fallback cascade,
which nests (a fallback child may itself be another `<object>`). Acid2 uses `<object>` ×3 for the face.

## Key realization (ground-truthed)
`<object>` is NOT in `box_tree::is_replaced` (only `"img"` is), so an `<object>` ALREADY renders its children
today — i.e. the **fallback path already works**. The only missing piece is rendering the object's `data` as
an image (the primary representation) and, in that case, SUPPRESSING the fallback children. Images are decoded
into a `NodeId → Rc<RgbaImage>` map (`images::collect_images`) with an entry present ONLY on successful decode,
so `images.get(object_id).is_some()` is exactly "the primary representation resolved."

## Non-negotiables
- No new dependency (reuses the P4 `data:`/fetch + image-decode pipeline). Report i486 delta. Parsing TOTAL.
- **Golden-safe:** a document with no `<object>` is unaffected. An `<object>` with no resolvable `data`
  renders its children exactly as today (no change to the existing fallback behavior).
- Test-first; no local i486 builds; PNG goldens pixel-verified (controller). No JS / C3 (an `<object>` is inert
  document content; no plugin/script execution — unsupported data simply falls back).

## Design
### 1. Fetch `<object data>` — `src/images.rs` (`walk`)
Alongside the existing `if el.name == "img" { … el.attrs.get("src") … }` branch, add: `if el.name == "object"`
→ fetch+decode `el.attrs.get("data")` with the IDENTICAL logic (resolve against base, dedup cache, budget,
`fetch_and_decode`), inserting into `out` keyed by the object's `NodeId`. (Factor the shared "resolve+decode a
URL attr into out[id]" body into a helper if clean; else duplicate the small block.) Result: an `<object>`
whose `data` decodes to an image has an `out[id]` entry; one that fails (unsupported type, bad bytes, no
`data`, budget) is absent — same success/absence contract `<img>` already uses.

### 2. Render `<object>` — `src/layout/box_tree.rs` (`build_node` element path)
Add an `<object>` branch (after the `is_replaced` check — do NOT add `object` to `is_replaced`, which would
force an empty-children Replaced and lose the fallback):
```rust
if el.name.as_str() == "object" {
    if let Some(img) = images.get(&id).cloned() {
        // Primary representation resolved → render the image, SUPPRESS fallback children.
        let mut style = style;
        apply_align_float_hint(el, &mut style);
        return Some(LayoutNode {
            style,
            content: BoxContent::Replaced { intrinsic: replaced_intrinsic(el, Some(&img)), image: Some(img) },
            children: Vec::new(),
            interactive: None,
        });
    }
    // else: fall through to the normal element path below, which builds the
    // object's CHILDREN — i.e. the fallback content (which may itself contain
    // more <object>s, handled recursively the same way). No special-casing
    // needed for nesting.
}
```
`replaced_intrinsic(el, decoded)` already derives the intrinsic size from `width`/`height` attrs (which
`<object>` also has) or the decoded image — reuse it verbatim.

### Testing / fixtures
- **Unit (CI):** `box_tree` — an `<object>` WITH a decoded image in the map → a `Replaced` node with empty
  children (fallback suppressed); an `<object>` with NO map entry → a `Container` whose children are its
  fallback content; a NESTED case (`<object><object>…</object></object>`) where the outer has no image and the
  inner does → outer is a Container containing the inner as a `Replaced`. (Build the `images` map by hand in the
  test, mirroring existing box_tree replaced-image tests.) `images` unit test for the `<object data>` fetch if
  the pipeline is unit-testable; else rely on the golden.
- **Golden micro-fixtures** (pixel-verified):
  - `object-image.html` — `<object data="data:image/png;base64,<16x16 red PNG>">FALLBACK</object>` renders the
    RED square, NOT the word "FALLBACK" (primary representation wins).
  - `object-fallback.html` — `<object data="data:,notanimage">fallback</object>` (data decodes to non-image
    bytes ⇒ no image ⇒ fallback) renders the word "fallback".
  - `object-nested.html` — `<object data="data:,bad"><object data="data:image/png;base64,<PNG>">deep</object></object>`
    — the outer object's data isn't an image, so it renders its child; the child object's `data` IS an image,
    so it renders the PNG (proving the nested fallback cascade resolves to the innermost representation).

### Charter / decisions
- Charter C2: add `<object>` with nested `data`→fallback cascade (Acid2 Packet 6).
- DECISIONS: `<object>` renders its decoded `data` image when it resolves (via the existing NodeId image map),
  else its children (fallback) — nesting is automatic because fallback children go through the same path; no
  `type`-attribute negotiation (decode-or-fallback); reuses the P4/image pipeline (no new dep).

## Out of scope (YAGNI)
- Honoring the `type`/`classid`/`codetype` attributes for representation selection (we decode-or-fallback).
- `<object>` as a nested browsing context (iframe-like) — a `data` document that is HTML, not an image, falls
  back (we render its fallback children, not the HTML document). `<param>`, `<embed>`, applet/plugin content.
