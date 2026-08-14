//! Image fetch+decode pre-pass (M4 images packet): walk a parsed [`Dom`] for
//! `<img src>` elements, resolve each `src` against the document's base
//! [`Url`], fetch it, and decode it via the P4 dispatcher
//! (`img::decode_bytes`) — handing back a `NodeId -> Rc<RgbaImage>` map for
//! `layout::box_tree::build_box_tree` to thread into `Replaced` boxes.
//!
//! Driver-level (this module does real I/O — a fetch per `<img>`), so it
//! lives here rather than in `img` (P4, pure decode, no network/filesystem)
//! or `layout` (the frozen box-tree seam, which only ever consumes an
//! already-decoded image, never fetches one itself). Mirrors `frames.rs`'s
//! own "small, total, driver-level fetch helper duplicated rather than
//! shared with `main.rs::fetch_body`" convention — see that module's doc
//! comment on `fetch_body` for the rationale (this module's own copy below
//! additionally threads the `Content-Type` header through, which
//! `frames.rs`'s copy has no need for).
//!
//! ## Totality
//!
//! Never panics, and never lets one bad `<img>` sink the page: a fetch
//! error, an unsupported scheme, a malformed/truncated image, or a decode
//! that would exceed the P4 decoders' own [`crate::img::MAX_DECODE_PIXELS`]
//! cap all simply leave that `NodeId` absent from the returned map — the
//! caller (`layout::box_tree`) already treats a missing entry as "no image
//! decoded", falling back to the intrinsic-size placeholder box exactly as
//! it does today. [`MAX_IMAGES`] bounds the total number of `<img>`s this
//! pre-pass will even attempt to fetch+decode, so a page with tens of
//! thousands of `<img>` elements can't drive unbounded network/decode work;
//! [`DEPTH_CAP`] bounds the DOM walk itself against pathological nesting,
//! the same recursion-safety concern `layout::box_tree`/`dom_util`/
//! `layout::block` each already guard against their own walk for.

use std::collections::HashMap;
use std::rc::Rc;

use crate::dom::{Dom, Node, NodeId};
use crate::fetch::file::FileFetcher;
use crate::fetch::http1::Http1Client;
use crate::fetch::{Fetch, Request, Response, Url};
use crate::img::{self, RgbaImage};

/// Upper bound on how many `<img>` elements one `collect_images` call will
/// fetch+decode. Past this many successfully-decoded images, every further
/// `<img>` in the document is left out of the returned map (rendering as its
/// ordinary intrinsic-size placeholder) rather than continuing to spend
/// unbounded network+decode work on a hostile/pathological page (brief's
/// "tens of thousands of `<img>`" scenario). 256 is far beyond any real
/// document-web page's image count while keeping the worst case (256 fetches
/// + decodes, each already bounded by the P4 decoders' own
/// `MAX_DECODE_PIXELS` cap) a small, fixed constant.
pub const MAX_IMAGES: usize = 256;

/// Maximum DOM nesting depth this walk will descend into — mirrors
/// `layout::box_tree::DEPTH_CAP`/`layout::block::DEPTH_CAP`/
/// `dom_util::DEPTH_CAP` (all independently bound, since each is its own
/// recursive walk): a deeply-nested/hostile document must degrade (silently
/// stop descending) rather than blow the native call stack (a guard-page
/// fault, `panic = "abort"` gives no mitigation for that).
const DEPTH_CAP: usize = 100;

/// Walk `dom` for every `<img src>`, fetch+decode each one (resolved against
/// `base`), and return a map from that `<img>`'s [`NodeId`] to its decoded
/// frame-0 pixels. Only ever called on the pixel (`--dump-png`) render path —
/// `--dump-text` passes an empty map instead (see `main.rs`), since a tty
/// dump never paints pixels and has no use for decoded image data.
pub fn collect_images(dom: &Dom, base: &Url) -> HashMap<NodeId, Rc<RgbaImage>> {
    // RED stub (test-first): always empty, so every test below documents
    // the real behavior and fails against this stub before the real walk +
    // fetch + decode pipeline is implemented.
    let _ = (dom, base);
    HashMap::new()
}

#[allow(dead_code)]
fn walk(dom: &Dom, id: NodeId, base: &Url, out: &mut HashMap<NodeId, Rc<RgbaImage>>, depth: usize) {
    let _ = (dom, id, base, out, depth);
}

/// Resolve `src` against `base`, fetch it, and decode frame 0. `None` on any
/// failure along the way (unresolvable/unsupported scheme, fetch error,
/// unrecognized/malformed bytes, an empty frame list) — see module docs.
/// Animated images (GIF) decode every frame; only the first is used for this
/// static render, matching the packet brief ("animated GIF -> first frame;
/// the ticking loop is a later/interactive concern").
#[allow(dead_code)]
fn fetch_and_decode(base: &Url, src: &str) -> Option<RgbaImage> {
    let url = base.resolve(src);
    let response = fetch_response(&url).ok()?;
    let content_type = response.header("content-type").map(|s| s.to_string());
    let frames = img::decode_bytes(&response.body, content_type.as_deref()).ok()?;
    frames.into_iter().next().map(|f| f.image)
}

/// Duplicated from (rather than shared with) `main.rs::fetch_body` /
/// `frames.rs::fetch_body`: same rationale as `frames.rs`'s own doc comment
/// on its copy — this is a small, total, driver-level fetch helper, and
/// three near-identical copies across `main`/`frames`/`images` cost far less
/// than reaching into the bin crate or inventing a shared "driver" module for
/// three call sites. This copy additionally returns the full [`Response`]
/// (not just the body) because [`fetch_and_decode`] needs its `Content-Type`
/// header as a decode hint (`img::decode_bytes`'s `content_type` parameter).
#[allow(dead_code)]
fn fetch_response(url: &Url) -> Result<Response, String> {
    match url.scheme().as_str() {
        "file" => FileFetcher::new().fetch(&Request::get(url.clone())).map_err(|e| format!("{e:?}")),
        "http" => Http1Client::new().fetch(&Request::get(url.clone())).map_err(|e| format!("{e:?}")),
        other => Err(format!("unsupported scheme: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::raster;
    use crate::dom;
    use crate::surface::{Color, MemSurface};

    /// Write a tiny deterministic PNG (`w`x`h`, solid `color`) to a fresh
    /// temp file and return its `file://` `Url`. Reuses `raster::encode_png`
    /// (already proven-valid PNG output, see `tests/png_golden.rs`) rather
    /// than hand-rolling PNG bytes in a test.
    fn write_temp_png(name: &str, w: u32, h: u32, color: Color) -> Url {
        let s = MemSurface::new(w, h, color);
        let bytes = raster::encode_png(&s);
        let path = std::env::temp_dir().join(format!("stele-images-test-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write temp png");
        Url::new(format!("file://{}", path.display()))
    }

    fn find_img_id(d: &dom::Dom) -> NodeId {
        fn walk(d: &dom::Dom, id: NodeId) -> Option<NodeId> {
            if let Node::Element(el) = d.node(id) {
                if el.name.as_str() == "img" {
                    return Some(id);
                }
                for &c in &el.children {
                    if let Some(found) = walk(d, c) {
                        return Some(found);
                    }
                }
            }
            None
        }
        walk(d, d.root()).expect("fixture should have an <img>")
    }

    #[test]
    fn resolves_fetches_and_decodes_an_img_src() {
        let png_url = write_temp_png("basic", 2, 2, Color::rgb(200, 50, 50));
        let html = format!(r#"<img src="{}" width="2" height="2">"#, png_url.as_str());
        let d = dom::parser::parse(&html);
        let img_id = find_img_id(&d);

        // Base URL is irrelevant here since the <img src> is already
        // absolute (`resolve` passes an absolute reference through
        // unchanged) -- any base works.
        let base = Url::new("file:///");
        let images = collect_images(&d, &base);

        let decoded = images.get(&img_id).expect("image should have decoded");
        assert_eq!((decoded.width, decoded.height), (2, 2));
        assert_eq!(&decoded.pixels[0..4], &[200, 50, 50, 255]);
    }

    #[test]
    fn resolves_a_relative_src_against_the_document_base() {
        // Write the PNG at a known path, then reference it by bare filename
        // (a relative src) resolved against a base URL pointing at that
        // same directory.
        let png_url = write_temp_png("relative", 3, 3, Color::rgb(10, 20, 30));
        let png_path = png_url.path();
        let dir = std::path::Path::new(&png_path).parent().unwrap().to_string_lossy().to_string();
        let filename = std::path::Path::new(&png_path).file_name().unwrap().to_string_lossy().to_string();
        let base = Url::new(format!("file://{dir}/index.html"));

        let html = format!(r#"<img src="{filename}">"#);
        let d = dom::parser::parse(&html);
        let img_id = find_img_id(&d);

        let images = collect_images(&d, &base);
        let decoded = images.get(&img_id).expect("relative src should resolve and decode");
        assert_eq!((decoded.width, decoded.height), (3, 3));
    }

    #[test]
    fn a_missing_file_is_skipped_not_a_panic() {
        let base = Url::new("file:///");
        let html = r#"<img src="file:///nonexistent-stele-test-image-xyz.png">"#;
        let d = dom::parser::parse(html);
        let img_id = find_img_id(&d);

        let images = collect_images(&d, &base);
        assert!(images.get(&img_id).is_none());
        assert!(images.is_empty());
    }

    #[test]
    fn malformed_image_bytes_are_skipped_not_a_panic() {
        let path = std::env::temp_dir().join(format!("stele-images-test-garbage-{}", std::process::id()));
        std::fs::write(&path, b"not a real image").expect("write garbage file");
        let url = Url::new(format!("file://{}", path.display()));

        let html = format!(r#"<img src="{}">"#, url.as_str());
        let d = dom::parser::parse(&html);
        let img_id = find_img_id(&d);

        let base = Url::new("file:///");
        let images = collect_images(&d, &base);
        assert!(images.get(&img_id).is_none());
    }

    #[test]
    fn non_img_elements_and_missing_src_never_produce_entries() {
        let d = dom::parser::parse("<div><span>hello</span><img></div>");
        let base = Url::new("file:///");
        let images = collect_images(&d, &base);
        assert!(images.is_empty());
    }

    #[test]
    fn empty_document_yields_an_empty_map() {
        let d = dom::Dom::new();
        let base = Url::new("file:///");
        let images = collect_images(&d, &base);
        assert!(images.is_empty());
    }

    #[test]
    fn image_count_is_capped_at_max_images() {
        let png_url = write_temp_png("cap", 1, 1, Color::rgb(1, 2, 3));
        let mut html = String::new();
        for _ in 0..(MAX_IMAGES + 20) {
            html.push_str(&format!(r#"<img src="{}">"#, png_url.as_str()));
        }
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");
        let images = collect_images(&d, &base);
        assert_eq!(images.len(), MAX_IMAGES, "must stop decoding past MAX_IMAGES");
    }

    #[test]
    fn deeply_nested_dom_does_not_abort_the_process() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str(r#"<img src="unreachable.png">"#);
        for _ in 0..depth {
            html.push_str("</div>");
        }
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");
        // Must return (not abort/hang); the deeply-nested <img> is past
        // DEPTH_CAP so it's never even visited (mirrors box_tree's own
        // depth-cap contract) -- an empty map is the correct, total result.
        let images = collect_images(&d, &base);
        assert!(images.is_empty());
    }
}
