//! M4 golden: THE SCREENSHOT — the real fetch->parse->cascade->images
//! pre-pass->box-tree->layout->raster->PNG pipeline run against
//! `fixtures/images.html` (a PNG, a JPEG, a GIF, and an animated GIF in
//! normal flow, plus — as of the M4 floats + inline images packet — a fifth
//! `<img align=left>` that FLOATS with a wrapping paragraph, and a sixth
//! non-floated `<img>` sitting inline between two words), asserted exact (by
//! decoded RGBA pixels, not raw PNG bytes) against a checked-in golden
//! image. This is a PROPOSED golden (brief §10 blessing discipline, same as
//! `tests/png_golden.rs`'s own `goldens/basic.png`) — the implementer
//! regenerates it (unavoidable: this packet's own code change alters how
//! EVERY `<img>` in this fixture lays out, non-floated ones now sitting
//! inline instead of breaking flow) but never self-*trusts* it; the
//! orchestrator/reviewer visually inspects `goldens/images.png` and
//! countersigns before it's trusted.
//!
//! Unlike `tests/png_golden.rs` (which reads its fixture via `include_str!`
//! and never touches the filesystem), this test drives a REAL `file://`
//! fetch: the images pre-pass (`images::collect_images`) needs a real base
//! `Url` to resolve+fetch each `<img src>` against, and
//! `fixtures/images.html`'s `<img>`s are genuinely relative
//! (`images-red.png`, `p4-baseline.jpg`, ...) siblings on disk — exactly the
//! wiring `stele --headless --dump-png fixtures/images.html out.png` drives
//! in `main.rs`.

use std::collections::HashMap;

use stele::backend::raster;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::{Fetch, Request, Url};
use stele::images;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const GOLDEN_PNG: &[u8] = include_bytes!("../goldens/images.png");
const VIEWPORT_WIDTH: u32 = 800;

fn fixture_url() -> Url {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/images.html");
    Url::new(format!("file://{}", path.display()))
}

fn fetch_fixture_html(url: &Url) -> String {
    let body = FileFetcher::new().fetch(&Request::get(url.clone())).expect("fixture should fetch").body;
    String::from_utf8_lossy(&body).into_owned()
}

/// Render `fixtures/images.html` to PNG bytes via the real
/// fetch->parse->cascade->images->box-tree->layout->raster->PNG pipeline —
/// the same wiring `main.rs`'s `dump_png` drives, real `file://` fetch
/// included (see module docs for why this test can't use `include_str!`
/// like `tests/png_golden.rs` does).
fn render_images_fixture() -> Vec<u8> {
    let url = fixture_url();
    let html = fetch_fixture_html(&url);
    let dom_tree = dom::parser::parse(&html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let decoded = images::collect_images(&dom_tree, &url);
    let Some(root) = build_box_tree(&dom_tree, &styles, &decoded) else {
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

    let mut surface = MemSurface::new(VIEWPORT_WIDTH, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &HashMap::new(), Color::WHITE);
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
fn images_fixture_png_matches_golden_pixels() {
    let actual_bytes = render_images_fixture();
    let (aw, ah, apx) = decode(&actual_bytes);
    let (gw, gh, gpx) = decode(GOLDEN_PNG);
    assert_eq!((aw, ah), (gw, gh), "rendered PNG dimensions changed from the PROPOSED golden");
    assert_eq!(apx, gpx, "rendered PNG pixels changed from the PROPOSED golden (goldens/images.png)");
}

#[test]
fn golden_images_png_is_well_formed_and_nontrivial() {
    let (w, h, px) = decode(GOLDEN_PNG);
    assert!(w > 0 && h > 0);
    assert_eq!(px.len(), (w as usize) * (h as usize) * 4);
    assert!(px.chunks(4).any(|p| p != [255, 255, 255, 255]), "golden should not be blank");
}

/// Distinct from the pixel-exact golden comparison above: confirms the
/// pre-pass really decoded every `<img>` (the original PNG/JPEG/GIF/animated
/// GIF, plus the M4 packet's floated `<img align=left>` and non-floated
/// inline `<img>`, both reusing the already-decoded `images-red.png`/
/// `images-blue.gif` fixture assets) — a regression that silently stopped
/// decoding, re-blessed against an all-placeholder render, would slip past a
/// pixel-exact check alone.
#[test]
fn all_images_actually_decode_not_fallen_back_to_placeholders() {
    let url = fixture_url();
    let html = fetch_fixture_html(&url);
    let dom_tree = dom::parser::parse(&html);
    let decoded = images::collect_images(&dom_tree, &url);
    assert_eq!(decoded.len(), 6, "all six <img>s in fixtures/images.html should decode");
}
