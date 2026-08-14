//! Generator for `fixtures/bg-tile.png` (packet bg-image's own tileable
//! asset), mirroring `tests/gen_images_fixtures.rs`'s own generator
//! convention exactly (see that file's doc comment for the full rationale:
//! deterministic, checked-in, reproducible via this generator rather than an
//! opaque binary blob).
//!
//! Not part of the normal test suite — `#[ignore]`d, so `cargo test` never
//! runs it. Run explicitly, once, to (re)produce the checked-in fixture:
//!
//! ```sh
//! cargo test --test gen_bg_image_fixture -- --ignored
//! ```
//!
//! Deliberately NOT a flat solid color (unlike `fixtures/images-red.png`,
//! reused elsewhere for plain `<img>` fixtures): a solid-color tile repeated
//! is visually indistinguishable from one big solid fill, which would defeat
//! the whole point of `goldens/bg-image.png` as a VISUAL proof of tiling
//! (packet brief: "the tiled image behind text is the visual proof" — the
//! orchestrator needs to be able to SEE the repeat, not just trust it
//! happened). A 16x16 red square framed by a 2px black border, tiled across
//! a box larger than 16x16, produces an unmistakable grid pattern.
const SIZE: u32 = 16;
const BORDER: u32 = 2;

#[test]
#[ignore = "generator, not a normal test -- run explicitly to (re)produce fixtures/bg-tile.png"]
fn generate_bg_tile_fixture_asset() {
    use stele::surface::{Color, MemSurface, Rect, Surface};

    let mut s = MemSurface::new(SIZE, SIZE, Color::rgb(220, 30, 30));
    // A 2px black frame around all four edges -- see module doc comment for
    // why a patterned (not flat) tile matters here.
    s.fill_rect(Rect { x: 0, y: 0, w: SIZE, h: BORDER }, Color::BLACK);
    s.fill_rect(Rect { x: 0, y: (SIZE - BORDER) as i32, w: SIZE, h: BORDER }, Color::BLACK);
    s.fill_rect(Rect { x: 0, y: 0, w: BORDER, h: SIZE }, Color::BLACK);
    s.fill_rect(Rect { x: (SIZE - BORDER) as i32, y: 0, w: BORDER, h: SIZE }, Color::BLACK);

    let bytes = stele::backend::raster::encode_png(&s);
    std::fs::write("fixtures/bg-tile.png", bytes).expect("write PNG fixture");
}
