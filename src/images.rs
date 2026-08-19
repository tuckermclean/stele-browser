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
use crate::fetch::{Request, Response, Url};
use crate::img::{self, RgbaImage};

/// Upper bound on how many DISTINCT `<img src>` URLs one `collect_images`
/// call will attempt to fetch+decode. Past this many attempts, every further
/// *unseen* `src` is left undecoded (rendering as its ordinary intrinsic-size
/// placeholder) rather than continuing to spend unbounded network+decode work
/// on a hostile/pathological page (brief's "tens of thousands of `<img>`"
/// scenario). Distinct because of dedup (see module docs): an `<img>` whose
/// resolved `src` was already fetched+decoded (or already failed) earlier in
/// the same walk is a cache hit, not a new attempt, and is never blocked by
/// this cap. 256 is far beyond any real document-web page's *distinct* image
/// count while keeping the worst case (256 fetches + decodes, each already
/// bounded by the P4 decoders' own `MAX_DECODE_PIXELS` cap) a small, fixed
/// constant.
pub const MAX_IMAGES: usize = 256;

/// Aggregate ceiling, in bytes, on the total decoded pixel data
/// (`RgbaImage::pixels.len()`, summed over every DISTINCT successfully
/// decoded image) one `collect_images` call will hold onto at once. Review
/// finding (Critical): `MAX_IMAGES` bounds image COUNT and the P4 decoders'
/// own `MAX_DECODE_PIXELS` bounds EACH decode (~244MiB at the cap), but
/// neither alone bounds the AGGREGATE — up to `MAX_IMAGES` distinct images
/// each near that per-image ceiling could total tens of GiB resident at
/// once, and `panic = "abort"` gives no soft-landing from the resulting
/// allocation failure. Once a decode would push the running total over this
/// budget, it (and every further unseen `src`) is skipped for the rest of
/// this `collect_images` call — see [`collect_images_bounded`]'s "budget
/// exhausted" handling. 256 MiB: a 486-class machine has little RAM to
/// begin with, and a single image already near `MAX_DECODE_PIXELS` (~244MiB)
/// is close to this ceiling by itself — this is a coarse aggregate backstop
/// against pathological *combinations* of images, not a tight per-page
/// memory budget.
pub const MAX_TOTAL_IMAGE_BYTES: usize = 256 * 1024 * 1024;

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
///
/// Bounded by the real [`MAX_IMAGES`]/[`MAX_TOTAL_IMAGE_BYTES`] constants —
/// see [`collect_images_bounded`] (the parameterized real implementation;
/// this is a thin wrapper so tests can exercise the same dedup/budget logic
/// against small, fast bounds instead of the real 256-image/256MiB ceiling).
pub fn collect_images(dom: &Dom, base: &Url) -> HashMap<NodeId, Rc<RgbaImage>> {
    collect_images_bounded(dom, base, MAX_IMAGES, MAX_TOTAL_IMAGE_BYTES)
}

/// Real implementation, parameterized over the two resource bounds so tests
/// can exercise the same dedup/budget logic against small, fast bounds
/// (`collect_images` is the thin wrapper callers use, always passing the
/// real [`MAX_IMAGES`]/[`MAX_TOTAL_IMAGE_BYTES`] constants).
///
/// Dedup: `cache` is keyed by the RESOLVED `src` URL (as a `String` — `Url`
/// has no `Hash` impl and is a frozen type this packet may not extend), and
/// holds `Some(Rc<RgbaImage>)` for a successful decode or `None` for one
/// that failed/was skipped — either way, a `src` seen once is never
/// fetched+decoded again for the rest of this walk; every `<img>` sharing
/// that `src` shares the SAME `Rc` (or the same "gave up" outcome). This is
/// the Critical review fix: a page with `<img src="same-big.jpg">` repeated
/// hundreds of times used to trigger one independent fetch+decode per
/// occurrence (unbounded redundant network/CPU/memory); it now collapses to
/// one.
///
/// `budget` tracks two running totals across the whole walk: how many
/// DISTINCT `src`s have been attempted (`attempts`, gated by `max_images`)
/// and how many total bytes are held by every successfully cached decode so
/// far (`total_bytes`, gated by `max_total_bytes`). Once either bound is
/// hit, `exhausted` latches `true` and every further UNSEEN `src` is skipped
/// without even attempting a fetch — already-cached `src`s (dedup hits)
/// remain fully usable regardless, since they cost no new attempt and no new
/// bytes. This is the second half of the Critical fix: `MAX_IMAGES` alone
/// bounds image COUNT and the P4 decoders' own `MAX_DECODE_PIXELS` bounds
/// EACH decode, but neither bounds the AGGREGATE — many DISTINCT
/// near-`MAX_DECODE_PIXELS` images could still total tens of GiB resident at
/// once without this.
fn collect_images_bounded(
    dom: &Dom,
    base: &Url,
    max_images: usize,
    max_total_bytes: usize,
) -> HashMap<NodeId, Rc<RgbaImage>> {
    let mut out = HashMap::new();
    if dom.is_empty() {
        return out;
    }
    let mut cache: HashMap<String, Option<Rc<RgbaImage>>> = HashMap::new();
    let mut budget = Budget { attempts: 0, total_bytes: 0, max_images, max_total_bytes, exhausted: false };
    walk(dom, dom.root(), base, &mut out, &mut cache, &mut budget, 0);
    out
}

/// Running resource-consumption state threaded through one [`collect_images_bounded`]
/// walk — see that function's doc comment for the exact semantics of each
/// field.
struct Budget {
    attempts: usize,
    total_bytes: usize,
    max_images: usize,
    max_total_bytes: usize,
    exhausted: bool,
}

fn walk(
    dom: &Dom,
    id: NodeId,
    base: &Url,
    out: &mut HashMap<NodeId, Rc<RgbaImage>>,
    cache: &mut HashMap<String, Option<Rc<RgbaImage>>>,
    budget: &mut Budget,
    depth: usize,
) {
    if depth >= DEPTH_CAP {
        return;
    }
    let Node::Element(el) = dom.node(id) else { return };

    if el.name.as_str() == "img" {
        if let Some(src) = el.attrs.get("src") {
            let url = base.resolve(src);
            let key = url.as_str().to_string();

            let resolved = match cache.get(&key) {
                // Dedup hit: this exact resolved URL was already attempted
                // earlier in the walk (successfully or not) — reuse that
                // outcome, no new fetch/decode, no new budget spend.
                Some(cached) => cached.clone(),
                // Unseen URL: only spend a new attempt if the walk hasn't
                // exhausted its budget (count or bytes) yet.
                None if !budget.exhausted && budget.attempts < budget.max_images => {
                    budget.attempts += 1;
                    let decoded = fetch_and_decode(&url).map(|image| {
                        let size = image.pixels.len();
                        (image, size)
                    });
                    let result = match decoded {
                        Some((image, size)) if budget.total_bytes.saturating_add(size) <= budget.max_total_bytes => {
                            budget.total_bytes += size;
                            Some(Rc::new(image))
                        }
                        Some(_) => {
                            // This decode alone (or combined with what's
                            // already resident) would exceed the aggregate
                            // budget: discard it, and stop attempting any
                            // further UNSEEN src for the rest of this walk
                            // (already-cached srcs remain usable).
                            budget.exhausted = true;
                            None
                        }
                        None => None, // fetch/decode failure, unrelated to budget
                    };
                    cache.insert(key, result.clone());
                    result
                }
                None => None, // budget exhausted (count or bytes): skip without attempting
            };

            if let Some(rc) = resolved {
                out.insert(id, rc);
            }
        }
    }

    for &child in &el.children {
        walk(dom, child, base, out, cache, budget, depth + 1);
    }
}

/// Fetch `url` and decode frame 0. `None` on any failure along the way
/// (fetch error, unsupported scheme, unrecognized/malformed bytes, an empty
/// frame list) — see module docs. Animated images (GIF) decode every frame;
/// only the first is used for this static render, matching the packet brief
/// ("animated GIF -> first frame; the ticking loop is a later/interactive
/// concern").
fn fetch_and_decode(url: &Url) -> Option<RgbaImage> {
    let response = fetch_response(url).ok()?;
    let content_type = response.header("content-type").map(|s| s.to_string());
    let frames = img::decode_bytes(&response.body, content_type.as_deref()).ok()?;
    frames.into_iter().next().map(|f| f.image)
}

/// The thin per-module wrapper stays (this one returns the full [`Response`]
/// for its `Content-Type` decode hint — [`fetch_and_decode`] needs it for
/// `img::decode_bytes`'s `content_type` parameter), but the scheme table
/// itself is now shared in `fetch::fetch`, so a new scheme lands once.
fn fetch_response(url: &Url) -> Result<Response, String> {
    crate::fetch::fetch(&Request::get(url.clone())).map_err(crate::fetch::err_to_string)
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
        find_all_img_ids(d).into_iter().next().expect("fixture should have an <img>")
    }

    fn find_all_img_ids(d: &dom::Dom) -> Vec<NodeId> {
        fn walk(d: &dom::Dom, id: NodeId, out: &mut Vec<NodeId>) {
            if let Node::Element(el) = d.node(id) {
                if el.name.as_str() == "img" {
                    out.push(id);
                }
                for &c in &el.children {
                    walk(d, c, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(d, d.root(), &mut out);
        out
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

    // ------------------------------------------------------ dedup + budget
    //
    // Review finding (Critical): MAX_IMAGES alone bounds image COUNT and the
    // P4 decoders' own MAX_DECODE_PIXELS bounds EACH decode, but nothing
    // bounded the AGGREGATE resident memory across many DISTINCT images, and
    // a repeated identical `src` used to trigger one independent
    // fetch+decode per occurrence (redundant network/CPU, and -- pre-dedup
    // -- redundant memory too). These three tests pin the fix: (1) the same
    // `src` referenced many times decodes exactly ONCE and is shared by
    // `Rc` across every referencing `<img>`; (2) a tiny `max_images` bound
    // still caps DISTINCT decode attempts (dedup doesn't defeat the count
    // cap); (3) a tiny aggregate byte budget stops decoding further DISTINCT
    // images once exhausted, while an already-cached (dedup) hit still
    // resolves regardless.

    #[test]
    fn repeated_src_decodes_once_and_is_shared_across_every_referencing_img() {
        // Same `src`, referenced far more times than MAX_IMAGES would allow
        // as independent attempts -- if this weren't deduped, the walk would
        // cap out at MAX_IMAGES entries (the pre-fix behavior); with dedup,
        // EVERY occurrence resolves (one real decode, shared by Rc).
        let png_url = write_temp_png("repeated", 1, 1, Color::rgb(1, 2, 3));
        let mut html = String::new();
        let occurrences = MAX_IMAGES + 20;
        for _ in 0..occurrences {
            html.push_str(&format!(r#"<img src="{}">"#, png_url.as_str()));
        }
        let d = dom::parser::parse(&html);
        let img_ids = find_all_img_ids(&d);
        assert_eq!(img_ids.len(), occurrences);

        let base = Url::new("file:///");
        let images = collect_images(&d, &base);

        assert_eq!(images.len(), occurrences, "every occurrence of the repeated src should decode, not just MAX_IMAGES of them");
        let first = images.get(&img_ids[0]).expect("first occurrence should decode");
        for id in &img_ids {
            let rc = images.get(id).expect("every occurrence should decode");
            assert!(Rc::ptr_eq(rc, first), "every occurrence of the same src should share ONE decoded Rc, not decode independently");
        }
    }

    #[test]
    fn distinct_images_beyond_max_images_are_skipped() {
        // A tiny max_images bound (via the parameterized real
        // implementation) still caps how many DISTINCT srcs get decoded --
        // dedup collapses repeats to one attempt, but distinct URLs are
        // still real, separate attempts subject to the count cap.
        let urls: Vec<Url> = (0..5).map(|i| write_temp_png(&format!("distinct-cap-{i}"), 1, 1, Color::rgb(1, 2, 3))).collect();
        let html: String = urls.iter().map(|u| format!(r#"<img src="{}">"#, u.as_str())).collect();
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let images = collect_images_bounded(&d, &base, 2, MAX_TOTAL_IMAGE_BYTES);
        assert_eq!(images.len(), 2, "must stop attempting new distinct srcs past max_images");
    }

    #[test]
    fn distinct_images_beyond_the_aggregate_byte_budget_are_skipped_and_cached_hits_still_work() {
        // Each 2x2 RGBA image decodes to exactly 16 bytes of pixels. A
        // budget of 16 fits exactly one distinct image; a second, DISTINCT
        // image would push the running total over budget and must be
        // skipped (rendering as its ordinary placeholder) -- and, per the
        // review finding, decoding must STOP there: a third distinct image
        // is skipped too, even though nothing has checked whether it alone
        // would individually fit. A later repeat of the FIRST (already
        // budgeted, cached) src must still resolve -- exhaustion blocks new
        // attempts, not cache hits.
        let fits = write_temp_png("budget-fits", 2, 2, Color::rgb(9, 9, 9));
        let too_big_1 = write_temp_png("budget-over-1", 2, 2, Color::rgb(8, 8, 8));
        let too_big_2 = write_temp_png("budget-over-2", 2, 2, Color::rgb(7, 7, 7));
        let html = format!(
            r#"<img src="{}"><img src="{}"><img src="{}"><img src="{}">"#,
            fits.as_str(),
            too_big_1.as_str(),
            too_big_2.as_str(),
            fits.as_str(), // repeat of the first, already-cached src
        );
        let d = dom::parser::parse(&html);
        let img_ids = find_all_img_ids(&d);
        assert_eq!(img_ids.len(), 4);
        let base = Url::new("file:///");

        let one_image_bytes = 2 * 2 * 4; // RgbaImage::pixels.len() for a 2x2 RGBA image
        let images = collect_images_bounded(&d, &base, MAX_IMAGES, one_image_bytes);

        assert!(images.get(&img_ids[0]).is_some(), "the first (budget-fitting) image should decode");
        assert!(images.get(&img_ids[1]).is_none(), "the second (over-budget) distinct image must be skipped");
        assert!(images.get(&img_ids[2]).is_none(), "decoding must stop after budget exhaustion, not just skip the one over-budget image");
        let repeat = images.get(&img_ids[3]).expect("a repeat of an already-cached, under-budget src must still resolve");
        assert!(Rc::ptr_eq(repeat, images.get(&img_ids[0]).unwrap()), "the repeat should share the cached Rc, not re-decode");
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
