//! Packet bg-image golden: THE SCREENSHOT — the real fetch->parse->cascade->
//! bg-images pre-pass->box-tree->layout->raster->PNG pipeline run against
//! `fixtures/bg-image.html` (a `.tile` box with a `background-image: url(...)`
//! tiled behind its own text content), asserted exact (by decoded RGBA
//! pixels, not raw PNG bytes) against a checked-in golden image. This is a
//! PROPOSED golden (brief §10 blessing discipline, same as every other
//! golden in this repo) — the implementer never self-blesses; the
//! orchestrator/reviewer visually inspects `goldens/bg-image.png` (the tiled
//! image visible BEHIND the "Tiled bg behind this text" text — proof of paint
//! layering) and countersigns before it's trusted.
//!
//! Also proves the `--no-bg-images` kill switch (packet brief): rendering the
//! SAME fixture with an empty `bg_images` map (what `main.rs`'s
//! `--no-bg-images` wires to) produces a DIFFERENTLY-colored, image-free
//! result — the `.tile` box shows no red tile pixels at all.
//!
//! Real `file://` fetch (like `tests/images_golden.rs`, not
//! `tests/png_golden.rs`'s `include_str!` shortcut): `bg_images::
//! collect_bg_images` needs a real base `Url` to resolve+fetch
//! `images-red.png` against, exactly the wiring `stele --headless --dump-png
//! fixtures/bg-image.html out.png` drives in `main.rs`.

use std::collections::HashMap;

use stele::backend::raster;
use stele::bg_images;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::{Fetch, Request, Url};
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/bg-image.png");
const VIEWPORT_WIDTH: u32 = 800;

fn fixture_url() -> Url {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/bg-image.html");
    Url::new(format!("file://{}", path.display()))
}

fn fetch_fixture_html(url: &Url) -> String {
    let body = FileFetcher::new().fetch(&Request::get(url.clone())).expect("fixture should fetch").body;
    String::from_utf8_lossy(&body).into_owned()
}

/// Render `fixtures/bg-image.html` via the real
/// fetch->parse->cascade->bg-images->box-tree->layout->raster->PNG pipeline
/// (the same wiring `main.rs`'s `dump_png_opts` drives). `no_bg_images`
/// mirrors `main.rs`'s own kill switch: `true` passes an empty map instead
/// of the real decoded pre-pass output, exactly like `--no-bg-images`.
fn render_bg_image_fixture(no_bg_images: bool) -> Vec<u8> {
    let url = fixture_url();
    let html = fetch_fixture_html(&url);
    let dom_tree = dom::parser::parse(&html);
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &url, VIEWPORT_WIDTH as f32);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return raster::encode_png(&MemSurface::new(1, 1, Color::WHITE));
    };
    let viewport = Size { w: VIEWPORT_WIDTH as f32, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);

    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let (y, h) = (f.rect.origin.y, f.rect.size.h);
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let height = if content_bottom > 0.0 { content_bottom.ceil() as u32 } else { 1 };

    let bg_images = if no_bg_images { HashMap::new() } else { bg_images::collect_bg_images(&styles, &url) };

    let mut surface = MemSurface::new(VIEWPORT_WIDTH, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &bg_images);
    raster::encode_png(&surface)
}

fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("valid PNG frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn bg_image_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_bg_image_fixture(false);
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/bg-image.png)");
}

#[test]
fn golden_bg_image_png_is_well_formed_and_shows_the_tiled_image() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    // fixtures/bg-tile.png is a 16x16 red square framed by a 2px black
    // border (tests/gen_bg_image_fixture.rs's own doc comment -- deliberately
    // patterned, not flat, so tiling is visually obvious) -- the golden must
    // show both colors, not just a blank/placeholder box.
    assert!(px.chunks(4).any(|p| p == [220, 30, 30, 255]), "golden should show the tiled background image's red fill");
    assert!(px.chunks(4).any(|p| p == [0, 0, 0, 255]), "golden should show the tiled background image's black frame");
    // And the white "Tiled bg behind this text" ink over the red tile.
    assert!(px.chunks(4).any(|p| p == [255, 255, 255, 255]));
}

/// The `--no-bg-images` proof: the SAME fixture, rendered with an empty
/// `bg_images` map, must be pixel-different from the real golden AND must
/// show no red tile pixels anywhere (the `.tile` box falls back to its
/// `background_color`, which this fixture never sets -- so the box paints
/// as fully transparent, i.e. the plain white page background shows
/// through).
#[test]
fn no_bg_images_produces_a_distinct_image_free_render() {
    let with_images = render_bg_image_fixture(false);
    let without_images = render_bg_image_fixture(true);
    assert_ne!(with_images, without_images, "--no-bg-images must change the render");

    let (_, _, without_px) = decode(&without_images);
    assert!(
        without_px.chunks(4).all(|p| p != [220, 30, 30, 255]),
        "--no-bg-images: no red background-image pixels should appear anywhere"
    );
}

/// Distinct from the pixel-exact golden comparison: confirms the pre-pass
/// really decoded the background image (a regression that silently stopped
/// decoding, re-blessed against an all-placeholder-color render, would slip
/// past a pixel-exact check alone).
#[test]
fn the_bg_image_actually_decodes_not_fallen_back_to_color_only() {
    let url = fixture_url();
    let html = fetch_fixture_html(&url);
    let dom_tree = dom::parser::parse(&html);
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &url, VIEWPORT_WIDTH as f32);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let images = bg_images::collect_bg_images(&styles, &url);
    assert_eq!(images.len(), 1, "fixtures/bg-image.html declares exactly one distinct background-image url");
}
