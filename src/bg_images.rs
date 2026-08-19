//! `background-image` fetch+decode pre-pass (packet bg-image): given the
//! cascaded [`ComputedStyle`]s for a document and its base [`Url`], resolve,
//! fetch, and decode every DISTINCT `background_image` URL, handing back a
//! map from the RAW (unresolved) `background_image` string to its decoded
//! frame-0 pixels for `backend::raster::paint` to blit.
//!
//! Mirrors `images::collect_images`'s own shape and rationale closely (same
//! "driver-level fetch+decode helper" placement — this does real I/O, so it
//! can't live in `img` (P4, pure decode) or `layout`/`style` (frozen seams
//! that only ever consume already-decoded data)) with one deliberate
//! difference: `collect_images` walks the DOM directly (because an `<img>`'s
//! decoded image is threaded onto a specific [`crate::dom::NodeId`] via
//! `layout::box_tree::build_box_tree`'s frozen signature); this module
//! instead walks the already-cascaded `&[ComputedStyle]` slice (`cascade::
//! cascade`'s own return shape, one entry per `NodeId`), because
//! `background_image` is consumed at PAINT time straight off each `Box`
//! fragment's own (already-cloned) `ComputedStyle` — no DOM walk or
//! `NodeId`-keyed map needed on that side (see `backend::raster::paint_box`).
//!
//! ## Map key: the RAW url, not the resolved one
//!
//! [`collect_bg_images`]'s returned map is keyed by the RAW (unresolved)
//! `background_image` string exactly as `ComputedStyle` carries it — NOT the
//! resolved `Url`. This is a deliberate design choice (packet brief's "your
//! call, documented"): `backend::raster::paint` only gets ONE new parameter
//! (`bg_images: &HashMap<String, Rc<RgbaImage>>`, per the brief) and no base
//! `Url` — keying by the raw string lets `paint_box` look a box's own
//! `style.background_image` straight up with no re-resolution step at paint
//! time. Internally, THIS module still dedups the real fetch+decode work by
//! the RESOLVED url (via `cache`, exactly like `collect_images`'s own dedup
//! cache) — two different raw strings that happen to resolve to the same
//! resource (e.g. one relative, one absolute) still only fetch+decode once,
//! sharing one `Rc`; only the OUTPUT map's key differs from `collect_images`'s
//! `NodeId` key.
//!
//! ## Pixel-only
//!
//! Only ever called on a pixel render path (`--dump-png`/`--render-fb`) —
//! the tty backend (`backend::tty`) has no use for decoded background-image
//! pixels (a character grid can't show an image) and already renders
//! `background_color` via ANSI on its own; `--dump-text` never calls this
//! module and passes no equivalent to `backend::tty::render`.
//!
//! ## Totality
//!
//! Never panics, and one bad `background-image` never sinks the page: an
//! unresolvable/unfetchable URL, an unsupported scheme, or a malformed/
//! truncated image all simply leave that url absent from the returned map —
//! `backend::raster::paint_box` already treats a missing map entry as "no
//! image", falling back to `background_color` alone. [`MAX_BG_IMAGES`] bounds
//! how many DISTINCT background-image URLs one call will even attempt to
//! fetch+decode; the aggregate byte budget (`images::MAX_TOTAL_IMAGE_BYTES`,
//! reused rather than duplicated — see [`collect_bg_images`]'s doc comment)
//! bounds the total resident decoded-pixel memory, mirroring
//! `images::collect_images_bounded`'s own two-bound design (and its Critical-
//! finding rationale: count alone doesn't bound aggregate bytes, since each
//! decode is independently bounded by `img::MAX_DECODE_PIXELS` but many
//! distinct near-that-size images could still total unbounded memory).

use std::collections::HashMap;
use std::rc::Rc;

use crate::fetch::{Request, Response, Url};
use crate::img::{self, RgbaImage};
use crate::images::MAX_TOTAL_IMAGE_BYTES;
use crate::style::ComputedStyle;

/// Upper bound on how many DISTINCT (by RESOLVED url) `background-image`
/// URLs one [`collect_bg_images`] call will attempt to fetch+decode. Past
/// this many attempts, every further unseen resolved url is left undecoded
/// (that box just shows its `background_color`) rather than continuing to
/// spend unbounded network+decode work on a hostile page with a huge number
/// of distinct background images. 32 is generous for any real document-web
/// page's background-image count (most pages have zero or one) while
/// keeping the worst case (32 fetches + decodes, each already bounded by
/// `img::MAX_DECODE_PIXELS`) small and fixed — smaller than
/// `images::MAX_IMAGES` (256) since `<img>` content images are far more
/// numerous on real pages than CSS background images.
pub const MAX_BG_IMAGES: usize = 32;

/// Fetch `url` and decode frame 0, or `None` on any failure (fetch error,
/// unsupported scheme, unrecognized/malformed bytes, an empty frame list).
/// Duplicated from (rather than shared with) `images::fetch_and_decode`: same
/// "small, total, driver-level fetch helper duplicated across call sites"
/// convention that module's own doc comment (and `frames.rs`'s) already
/// establishes — three-plus near-identical copies cost far less than
/// reaching across module boundaries for a few lines of glue.
fn fetch_and_decode(url: &Url) -> Option<RgbaImage> {
    let response = fetch_response(url).ok()?;
    let content_type = response.header("content-type").map(|s| s.to_string());
    let frames = img::decode_bytes(&response.body, content_type.as_deref()).ok()?;
    frames.into_iter().next().map(|f| f.image)
}

/// The thin per-module wrapper stays (duplicated from `images::
/// fetch_response` rather than shared), but the scheme table itself is now
/// shared in `fetch::fetch`, so a new scheme lands once.
fn fetch_response(url: &Url) -> Result<Response, String> {
    crate::fetch::fetch(&Request::get(url.clone())).map_err(crate::fetch::err_to_string)
}

/// Running resource-consumption state threaded through one
/// [`collect_bg_images_bounded`] walk — mirrors `images::Budget` exactly
/// (see that struct's doc comment for the precise semantics of each field;
/// duplicated rather than shared since it's `images`-module-private).
struct Budget {
    attempts: usize,
    total_bytes: usize,
    max_images: usize,
    max_total_bytes: usize,
    exhausted: bool,
}

/// Collect, resolve, fetch, and decode every DISTINCT `background_image` URL
/// referenced anywhere in `styles` (`style::cascade::cascade`'s own return
/// shape — one [`ComputedStyle`] per `NodeId`, order doesn't matter here),
/// resolved against the document's base `base`. See module docs for the
/// map's RAW-url keying and the pixel-only/totality contracts.
///
/// Bounded by the real [`MAX_BG_IMAGES`]/[`images::MAX_TOTAL_IMAGE_BYTES`]
/// constants — [`collect_bg_images_bounded`] is the parameterized real
/// implementation, so tests can exercise the same dedup/budget logic against
/// small, fast bounds instead (mirrors `images::collect_images`'s own
/// wrapper-over-parameterized-impl shape).
pub fn collect_bg_images(styles: &[ComputedStyle], base: &Url) -> HashMap<String, Rc<RgbaImage>> {
    collect_bg_images_bounded(styles, base, MAX_BG_IMAGES, MAX_TOTAL_IMAGE_BYTES)
}

/// Real implementation, parameterized over the two resource bounds (see
/// [`collect_bg_images`]). `cache` dedups the real fetch+decode work by
/// RESOLVED url (a `String`, since `Url` has no `Hash` impl and is frozen);
/// the returned `out` map is keyed by the RAW url instead (see module docs).
fn collect_bg_images_bounded(
    styles: &[ComputedStyle],
    base: &Url,
    max_images: usize,
    max_total_bytes: usize,
) -> HashMap<String, Rc<RgbaImage>> {
    let mut out = HashMap::new();
    let mut cache: HashMap<String, Option<Rc<RgbaImage>>> = HashMap::new();
    let mut budget = Budget { attempts: 0, total_bytes: 0, max_images, max_total_bytes, exhausted: false };

    for style in styles {
        let Some(raw_url) = style.background_image.as_deref() else { continue };
        if out.contains_key(raw_url) {
            // This exact raw url string already resolved successfully
            // earlier in this same walk -- no new work needed.
            continue;
        }

        let resolved_key = base.resolve(raw_url).as_str().to_string();
        let resolved = match cache.get(&resolved_key) {
            // Dedup hit: this resolved url was already attempted (whether it
            // succeeded or not) by an earlier, different raw url string —
            // reuse that outcome, no new fetch/decode, no new budget spend.
            Some(cached) => cached.clone(),
            None if !budget.exhausted && budget.attempts < budget.max_images => {
                budget.attempts += 1;
                let decoded = fetch_and_decode(&base.resolve(raw_url)).map(|image| {
                    let size = image.pixels.len();
                    (image, size)
                });
                let result = match decoded {
                    Some((image, size)) if budget.total_bytes.saturating_add(size) <= budget.max_total_bytes => {
                        budget.total_bytes += size;
                        Some(Rc::new(image))
                    }
                    Some(_) => {
                        // This decode alone (or combined with what's already
                        // resident) would exceed the aggregate budget:
                        // discard it, and stop attempting any further unseen
                        // resolved url for the rest of this walk (already-
                        // cached ones remain usable).
                        budget.exhausted = true;
                        None
                    }
                    None => None, // fetch/decode failure, unrelated to budget
                };
                cache.insert(resolved_key, result.clone());
                result
            }
            None => None, // budget exhausted (count or bytes): skip without attempting
        };

        if let Some(rc) = resolved {
            out.insert(raw_url.to_string(), rc);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::raster;
    use crate::surface::{Color, MemSurface};

    /// Write a tiny deterministic PNG (`w`x`h`, solid `color`) to a fresh
    /// temp file and return its `file://` `Url` — mirrors
    /// `images::tests::write_temp_png` exactly (same rationale: reuse
    /// `raster::encode_png`, already proven-valid PNG output, rather than
    /// hand-rolling PNG bytes in a test).
    fn write_temp_png(name: &str, w: u32, h: u32, color: Color) -> Url {
        let s = MemSurface::new(w, h, color);
        let bytes = raster::encode_png(&s);
        let path = std::env::temp_dir().join(format!("stele-bg-images-test-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write temp png");
        Url::new(format!("file://{}", path.display()))
    }

    fn style_with_bg(url: &str) -> ComputedStyle {
        ComputedStyle { background_image: Some(url.into()), ..ComputedStyle::default() }
    }

    #[test]
    fn fetches_and_decodes_a_file_url_background_image() {
        let png_url = write_temp_png("basic", 2, 2, Color::rgb(200, 50, 50));
        let styles = vec![style_with_bg(png_url.as_str())];
        let base = Url::new("file:///");

        let images = collect_bg_images(&styles, &base);
        let decoded = images.get(png_url.as_str()).expect("bg image should have decoded");
        assert_eq!((decoded.width, decoded.height), (2, 2));
        assert_eq!(&decoded.pixels[0..4], &[200, 50, 50, 255]);
    }

    #[test]
    fn resolves_a_relative_url_against_the_document_base() {
        let png_url = write_temp_png("relative", 3, 3, Color::rgb(10, 20, 30));
        let png_path = png_url.path();
        let dir = std::path::Path::new(&png_path).parent().unwrap().to_string_lossy().to_string();
        let filename = std::path::Path::new(&png_path).file_name().unwrap().to_string_lossy().to_string();
        let base = Url::new(format!("file://{dir}/index.html"));

        let styles = vec![style_with_bg(&filename)];
        let images = collect_bg_images(&styles, &base);
        let decoded = images.get(filename.as_str()).expect("relative bg-image url should resolve and decode");
        assert_eq!((decoded.width, decoded.height), (3, 3));
    }

    #[test]
    fn no_background_image_declared_yields_no_entry_not_a_panic() {
        let styles = vec![ComputedStyle::default()];
        let base = Url::new("file:///");
        let images = collect_bg_images(&styles, &base);
        assert!(images.is_empty());
    }

    #[test]
    fn a_missing_file_is_skipped_not_a_panic() {
        let styles = vec![style_with_bg("file:///nonexistent-stele-bg-test-xyz.png")];
        let base = Url::new("file:///");
        let images = collect_bg_images(&styles, &base);
        assert!(images.is_empty());
    }

    #[test]
    fn malformed_image_bytes_are_skipped_not_a_panic() {
        let path = std::env::temp_dir().join(format!("stele-bg-images-test-garbage-{}", std::process::id()));
        std::fs::write(&path, b"not a real image").expect("write garbage file");
        let url = Url::new(format!("file://{}", path.display()));

        let styles = vec![style_with_bg(url.as_str())];
        let base = Url::new("file:///");
        let images = collect_bg_images(&styles, &base);
        assert!(images.is_empty());
    }

    #[test]
    fn empty_styles_slice_yields_an_empty_map() {
        let images = collect_bg_images(&[], &Url::new("file:///"));
        assert!(images.is_empty());
    }

    // ------------------------------------------------------ dedup + budget

    #[test]
    fn repeated_raw_url_decodes_once_and_is_shared_by_rc() {
        let png_url = write_temp_png("repeated", 1, 1, Color::rgb(1, 2, 3));
        let occurrences = MAX_BG_IMAGES + 5;
        let styles: Vec<ComputedStyle> = (0..occurrences).map(|_| style_with_bg(png_url.as_str())).collect();
        let base = Url::new("file:///");

        let images = collect_bg_images(&styles, &base);
        // One raw url string -> one map entry, regardless of how many
        // ComputedStyles reference it (unlike images::collect_images, whose
        // output is keyed per-NodeId; here the map key IS the raw url, so
        // repeats collapse to the same single entry by construction).
        assert_eq!(images.len(), 1);
        assert!(images.contains_key(png_url.as_str()));
    }

    #[test]
    fn distinct_urls_beyond_max_images_are_skipped() {
        let urls: Vec<Url> = (0..5).map(|i| write_temp_png(&format!("distinct-cap-{i}"), 1, 1, Color::rgb(1, 2, 3))).collect();
        let styles: Vec<ComputedStyle> = urls.iter().map(|u| style_with_bg(u.as_str())).collect();
        let base = Url::new("file:///");

        let images = collect_bg_images_bounded(&styles, &base, 2, MAX_TOTAL_IMAGE_BYTES);
        assert_eq!(images.len(), 2, "must stop attempting new distinct urls past max_images");
    }

    #[test]
    fn distinct_urls_beyond_the_aggregate_byte_budget_are_skipped_and_cached_hits_still_work() {
        // Each 2x2 RGBA image decodes to exactly 16 bytes of pixels.
        let fits = write_temp_png("budget-fits", 2, 2, Color::rgb(9, 9, 9));
        let too_big_1 = write_temp_png("budget-over-1", 2, 2, Color::rgb(8, 8, 8));
        let too_big_2 = write_temp_png("budget-over-2", 2, 2, Color::rgb(7, 7, 7));
        let styles = vec![
            style_with_bg(fits.as_str()),
            style_with_bg(too_big_1.as_str()),
            style_with_bg(too_big_2.as_str()),
        ];
        let base = Url::new("file:///");

        let one_image_bytes = 2 * 2 * 4;
        let images = collect_bg_images_bounded(&styles, &base, MAX_BG_IMAGES, one_image_bytes);

        assert!(images.get(fits.as_str()).is_some(), "the first (budget-fitting) image should decode");
        assert!(images.get(too_big_1.as_str()).is_none(), "the second (over-budget) distinct image must be skipped");
        assert!(images.get(too_big_2.as_str()).is_none(), "decoding must stop after budget exhaustion, not just skip the one over-budget image");
    }

    #[test]
    fn hostile_many_distinct_bg_image_styles_do_not_panic_and_stay_bounded() {
        let styles: Vec<ComputedStyle> = (0..500).map(|i| style_with_bg(&format!("file:///unreachable-{i}.png"))).collect();
        let base = Url::new("file:///");
        let images = collect_bg_images(&styles, &base); // must not panic / hang
        assert!(images.is_empty(), "every url here is unreachable, so nothing should decode");
    }
}
