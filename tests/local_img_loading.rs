//! Regression coverage for a "local `<img>` renders blank" report
//! (packet/fix-local-img-loading).
//!
//! The report hypothesized a directory-dependent `file://` URL-resolution
//! bug (a document under an arbitrary, non-repo directory with a relative
//! `<img src>` sibling, or an absolute `file://` `<img src>`, both allegedly
//! never loading). Investigating: `stele::fetch::url`/`stele::fetch::file`
//! already resolve+extract both shapes correctly regardless of directory
//! (see `tests/fetch_url.rs`/`tests/fetch_file.rs`'s new contract cases) —
//! that hypothesis didn't hold up under reproduction.
//!
//! The REAL root cause: `layout::box_tree::build_node` already receives the
//! fetched+decoded `images: &HashMap<NodeId, Rc<RgbaImage>>` map (M4's
//! images packet, #22), but its `<img>`-without-`width`/`height`-attributes
//! path (`img_intrinsic`) never consulted it, always defaulting to a 0x0
//! intrinsic size even when the image had, in fact, already decoded
//! successfully. A 0x0 `Replaced` box has nothing for `layout`/
//! `raster::paint` to draw into — the fetch+decode fully succeeds, but the
//! image renders as blank pixels anyway. This is exactly the shape
//! `tools/render-gallery.sh` generates (`<img src="basic.png" alt="..."
//! loading="lazy">` — no `width`/`height` at all), which is why the CI
//! render gallery's own `index.html`, opened in Stele, showed every
//! thumbnail blank.
//!
//! This test drives the real fetch->parse->cascade->images pre-pass->
//! box-tree->layout->raster->PNG pipeline (mirroring `tests/images_golden.
//! rs` and `main.rs`'s `dump_png`) against documents written to an
//! ARBITRARY temp directory (never under the repo tree) with NO `width`/
//! `height` attributes on their `<img>`s, covering both a relative and an
//! absolute `file://` `src`.

use std::collections::HashMap;

use stele::backend::raster;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::{Fetch, Request, Url};
use stele::images;
use stele::layout::box_tree::build_box_tree;
use stele::layout::{self, BoxContent, LayoutNode, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

const VIEWPORT_WIDTH: u32 = 800;

/// The UA stylesheet's `body { margin: 8px; }` (`src/style/ua.rs`) adds this
/// much to a bare `<body><img></body>` document's total content height (8px
/// top + 8px bottom) on top of the `<img>`'s own intrinsic height -- the
/// page-height assertions below account for it explicitly so they pin the
/// exact expected height rather than a loose inequality.
const BODY_MARGIN_V: u32 = 16;

/// A fresh, never-repo-tree scratch directory per test (PID + a per-call
/// discriminator to avoid collisions between tests in the same process).
fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("stele-local-img-loading-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_red_png(dir: &std::path::Path, filename: &str, w: u32, h: u32) {
    let surface = MemSurface::new(w, h, Color::rgb(200, 40, 40));
    let bytes = raster::encode_png(&surface);
    std::fs::write(dir.join(filename), bytes).expect("write scratch PNG");
}

fn doc_url(dir: &std::path::Path, filename: &str) -> Url {
    Url::new(format!("file://{}", dir.join(filename).display()))
}

fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
    if matches!(node.content, BoxContent::Replaced { .. }) {
        return Some(node);
    }
    node.children.iter().find_map(find_img)
}

/// Render `url`'s document through the real pipeline, returning both the
/// box tree (to inspect the `<img>`'s intrinsic size directly) and the
/// final painted PNG bytes (to confirm real pixels, not just a
/// correctly-sized-but-unpainted box).
fn render(url: &Url) -> (LayoutNode, Vec<u8>) {
    let html = FileFetcher::new().fetch(&Request::get(url.clone())).expect("doc should fetch").body;
    let html = String::from_utf8_lossy(&html);
    let dom_tree = dom::parser::parse(&html);
    let styles = cascade::cascade(&dom_tree, &[]);
    let decoded = images::collect_images(&dom_tree, url);
    let root = build_box_tree(&dom_tree, &styles, &decoded).expect("non-empty document");
    let viewport = Size { w: VIEWPORT_WIDTH as f32, h: 100_000.0 };
    let fragments = layout::layout(&root, viewport);

    let mut content_bottom = 0.0f32;
    for f in &fragments {
        if f.rect.origin.y.is_finite() && f.rect.size.h.is_finite() {
            content_bottom = content_bottom.max(f.rect.origin.y + f.rect.size.h);
        }
    }
    let height = if content_bottom > 0.0 { content_bottom.ceil() as u32 } else { 1 };
    let mut surface = MemSurface::new(VIEWPORT_WIDTH, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &HashMap::new(), Color::WHITE);
    (root, raster::encode_png(&surface))
}

fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("valid PNG frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn has_red_pixel(px: &[u8]) -> bool {
    px.chunks(4).any(|p| p[0] > 150 && p[1] < 100 && p[2] < 100)
}

#[test]
fn relative_img_src_with_no_size_attrs_decodes_and_paints_at_an_arbitrary_directory() {
    let dir = scratch_dir("relative-nosize");
    write_red_png(&dir, "pic.png", 5, 5);
    std::fs::write(
        dir.join("doc.html"),
        r#"<!doctype html><html><body><img src="pic.png" alt="red"></body></html>"#,
    )
    .expect("write doc");

    let url = doc_url(&dir, "doc.html");
    let (root, png_bytes) = render(&url);

    let img = find_img(&root).expect("img box present");
    match img.content {
        BoxContent::Replaced { intrinsic, ref image } => {
            assert!(image.is_some(), "the relative src should have fetched+decoded");
            assert_eq!(intrinsic.w, 5.0, "intrinsic width should come from the decoded image, not default to 0");
            assert_eq!(intrinsic.h, 5.0, "intrinsic height should come from the decoded image, not default to 0");
        }
        _ => panic!("expected a Replaced box"),
    }

    let (w, h, px) = decode_png(&png_bytes);
    assert_eq!(
        (w, h),
        (VIEWPORT_WIDTH, 5 + BODY_MARGIN_V),
        "the page height should reflect the decoded image (plus the UA body margin), not collapse to just the margin"
    );
    assert!(has_red_pixel(&px), "the decoded red image should actually be painted, not rendered blank");
}

#[test]
fn absolute_file_url_img_src_with_no_size_attrs_decodes_and_paints() {
    let dir = scratch_dir("absolute-nosize");
    write_red_png(&dir, "pic.png", 6, 4);
    let abs_src = doc_url(&dir, "pic.png");
    std::fs::write(
        dir.join("doc.html"),
        format!(r#"<!doctype html><html><body><img src="{}" alt="red"></body></html>"#, abs_src.as_str()),
    )
    .expect("write doc");

    let url = doc_url(&dir, "doc.html");
    let (root, png_bytes) = render(&url);

    let img = find_img(&root).expect("img box present");
    match img.content {
        BoxContent::Replaced { intrinsic, ref image } => {
            assert!(image.is_some(), "the absolute file:// src should have fetched+decoded");
            assert_eq!(intrinsic.w, 6.0);
            assert_eq!(intrinsic.h, 4.0);
        }
        _ => panic!("expected a Replaced box"),
    }

    let (w, h, px) = decode_png(&png_bytes);
    assert_eq!((w, h), (VIEWPORT_WIDTH, 4 + BODY_MARGIN_V));
    assert!(has_red_pixel(&px), "the decoded red image should actually be painted, not rendered blank");
}

/// Positive control mirroring the report's own "this ALREADY works" case
/// (`fixtures/images.html`, explicit `width`/`height` on every `<img>`) —
/// confirms an explicit attribute still wins and nothing regressed for the
/// already-working shape, at an arbitrary (non-repo) directory this time.
#[test]
fn relative_img_src_with_explicit_size_attrs_still_uses_the_attributes() {
    let dir = scratch_dir("relative-explicit-size");
    write_red_png(&dir, "pic.png", 5, 5);
    std::fs::write(
        dir.join("doc.html"),
        r#"<!doctype html><html><body><img src="pic.png" width="40" height="20" alt="red"></body></html>"#,
    )
    .expect("write doc");

    let url = doc_url(&dir, "doc.html");
    let (root, png_bytes) = render(&url);

    let img = find_img(&root).expect("img box present");
    match img.content {
        BoxContent::Replaced { intrinsic, .. } => {
            assert_eq!(intrinsic.w, 40.0);
            assert_eq!(intrinsic.h, 20.0);
        }
        _ => panic!("expected a Replaced box"),
    }

    let (_, _, px) = decode_png(&png_bytes);
    assert!(has_red_pixel(&px));
}

/// A relative sibling nested one directory deeper (`images/pic.png`),
/// covering the multi-segment relative-path shape alongside the flat one
/// above — directory-independence should hold for both.
#[test]
fn relative_img_src_in_a_nested_subdirectory_decodes_and_paints() {
    let dir = scratch_dir("relative-nested");
    let sub = dir.join("images");
    std::fs::create_dir_all(&sub).expect("create nested dir");
    write_red_png(&sub, "pic.png", 3, 3);
    std::fs::write(
        dir.join("doc.html"),
        r#"<!doctype html><html><body><img src="images/pic.png" alt="red"></body></html>"#,
    )
    .expect("write doc");

    let url = doc_url(&dir, "doc.html");
    let (root, png_bytes) = render(&url);

    let img = find_img(&root).expect("img box present");
    match img.content {
        BoxContent::Replaced { intrinsic, ref image } => {
            assert!(image.is_some());
            assert_eq!(intrinsic.w, 3.0);
            assert_eq!(intrinsic.h, 3.0);
        }
        _ => panic!("expected a Replaced box"),
    }

    let (_, _, px) = decode_png(&png_bytes);
    assert!(has_red_pixel(&px));
}
