//! External `<link rel="stylesheet">` fetch pre-pass (packet `m5-link-css`).
//!
//! `style::collect_author_sheets`/`collect_author_sheets_for_viewport`
//! (`style::author`) walk a pure `&Dom` for `<style>` blocks only — no I/O,
//! no base `Url`, by design (see that module's doc comment). A `<link
//! rel="stylesheet" href="...">` needs BOTH: the href must be resolved
//! against the document's base `Url` and actually fetched before there's any
//! CSS text to parse. That's driver-level work (real network/filesystem I/O),
//! so — mirroring `images::collect_images`'s own "walks the DOM, fetches per
//! element, driver-level rather than inside `style`/`layout`" shape — it
//! lives here, in the lib crate (so both the bin crate's `main.rs` and the
//! lib crate's own `frames.rs` can call it), not inside `style`.
//!
//! ## Document order across `<link>` AND `<style>`
//!
//! CSS cascade order matters: a `<style>` block appearing AFTER a `<link>`
//! in the source must override it on an equal-specificity tie (ordinary CSS
//! source-order semantics), and vice versa. [`collect_all_author_sheets`]
//! therefore walks the DOM ONCE, in document order, treating `<link
//! rel="stylesheet">` and `<style>` as two flavors of "the next author
//! sheet in source order" rather than collecting them in two separate
//! passes (which would silently reorder them, `<link>`s all before/after
//! every `<style>` regardless of where they actually sit in the markup).
//! Same explicit-stack, non-recursive walk `style::author::
//! collect_author_sheets` already uses (pushed in reverse child order so
//! children pop, and are thus visited, left-to-right) — deep-DOM-safe for
//! the same reason that one is.
//!
//! ## `<link media="...">`
//!
//! A stylesheet-LEVEL media gate (distinct from an in-CSS `@media { }`
//! block, which every collected sheet — `<link>`-sourced or `<style>`-
//! sourced — still gets `media::flatten_media`'d against below): if the
//! `<link>` carries a non-empty `media` attribute that doesn't match
//! `viewport_width_px` (`style::media_attr_matches`), the WHOLE sheet is
//! skipped — not even fetched, since there is no `@media`-partial-fetch
//! concept here, unlike a partial-match inside one already-fetched sheet's
//! own `@media` blocks. An absent or empty `media` attribute matches
//! unconditionally (ordinary CSS behavior: no `media` attribute means `all`).
//!
//! ## `rel` matching
//!
//! `rel` may carry multiple space-separated tokens (`rel="alternate
//! stylesheet"`); this module matches case-insensitively if `stylesheet` is
//! ANY one of them — a deliberate simplification over real browsers (which
//! treat a leading `alternate` specially, as a non-default alternate sheet);
//! documented, not a bug. A `<link>` with no `rel` at all, or a `rel` that
//! doesn't include `stylesheet` (e.g. `icon`, `preload`, `dns-prefetch`), is
//! never fetched — it isn't even charged against [`MAX_LINKS`] (see that
//! constant's own doc comment).
//!
//! ## `@import` inside a fetched sheet
//!
//! Still ignored, same as it always has been for inline `<style>` CSS
//! (`style::parser::parse` already treats `@import` as an ordinary unknown
//! at-rule, counted in `Stylesheet::ignored_at_rules` — see that module). A
//! fetched `<link>` sheet can't recurse into more HTML the way a `<script>`
//! could, so there is no cycle-detection concern here the way `frames.rs`
//! needs one for `<frame src>` — `@import` support (following ANOTHER CSS
//! fetch) is simply out of scope for this packet, not a safety hazard left
//! unhandled.
//!
//! ## Totality + bounds
//!
//! A `<link>` fetch error, an unsupported URL scheme, a missing/empty
//! `href`, or a response whose `Content-Type` header is present and clearly
//! not CSS all SKIP just that one sheet — the rest of the document's author
//! CSS (and the document itself) renders normally, never a panic, matching
//! this whole codebase's totality covenant. [`MAX_LINKS`] bounds the number
//! of DISTINCT fetch attempts one `collect_all_author_sheets` call will make,
//! so a document with thousands of `<link rel="stylesheet">`s can't drive
//! unbounded network/parse work (mirrors `images::MAX_IMAGES`'s rationale,
//! simpler here since there is no dedup/aggregate-byte-budget concern:
//! `style::parser::parse`'s own bounds already cap any ONE sheet's rule
//! count, and 32 small CSS fetches is cheap relative to the images path's
//! potentially-huge decoded pixel buffers).

use crate::dom::{Dom, Element, Node, NodeId};
use crate::fetch::file::FileFetcher;
use crate::fetch::http1::Http1Client;
use crate::fetch::{Fetch, Request, Response, Url};
use crate::style::{self, Stylesheet};

/// Upper bound on how many `<link rel="stylesheet">` fetch ATTEMPTS one
/// [`collect_all_author_sheets`] call will make. Charged the moment a
/// `<link>` passes the `rel`/`media` gates and is about to be fetched — a
/// `<link>` that isn't a stylesheet, or whose `media` attribute doesn't
/// match the viewport, costs nothing against this budget (it was never
/// going to be fetched regardless). Once exhausted, every further
/// stylesheet `<link>` is skipped without being fetched, same as any other
/// skip reason in this module. 32 is far beyond any real document-web page's
/// external stylesheet count while keeping the worst case (32 small fetches)
/// a fixed, cheap constant — see the module doc comment's Totality section.
pub const MAX_LINKS: usize = 32;

/// Walk `dom` in document order, collecting every author [`Stylesheet`] —
/// both inline `<style>` blocks AND fetched `<link rel="stylesheet" href>`
/// sheets, interleaved in the SAME document-order sequence they appear in
/// the markup — each already flattened against `viewport_width_px` (both
/// its own in-CSS `@media` blocks via [`style::parse_and_flatten`], and, for
/// `<link>`s specifically, the stylesheet-level `media` ATTRIBUTE gate — see
/// module docs). `base` resolves each `<link href>` (`Url::resolve`,
/// document-relative or absolute either way) before fetching.
///
/// This is the ONE entry point every real render site (`main.rs`'s
/// `dump_text`/`dump_png`/`render_fb_surface`, `frames.rs`'s
/// `render_single_document`) calls now — see each call site's own comment
/// for why. `style::collect_author_sheets`/`collect_author_sheets_for_viewport`
/// remain as they were (pure-DOM, `<style>`-only) for callers with no base
/// `Url`/fetch capability to give (unit tests, mainly).
///
/// Total: an empty DOM returns an empty `Vec`; every per-`<link>` failure
/// mode (bad `href`, fetch error, non-CSS response, budget exhaustion) skips
/// just that sheet — see module docs' Totality section.
#[allow(unused_variables)]
pub fn collect_all_author_sheets(dom: &Dom, base: &Url, viewport_width_px: f32) -> Vec<Stylesheet> {
    // RED (m5-link-css): `<link>` fetching not implemented yet -- this stub
    // only picks up inline <style> blocks (the pre-existing M5 behavior),
    // same as `style::collect_author_sheets_for_viewport`. Every test below
    // that exercises an ACTUAL `<link rel=stylesheet>` fetch is expected to
    // fail against this stub; see the follow-up commit for the real
    // implementation.
    style::collect_author_sheets_for_viewport(dom, viewport_width_px)
}

/// Handle one `<link>` element: gate on `rel`/`media`, then (if it passes
/// both, and the fetch budget isn't exhausted) fetch+parse+flatten its href.
/// `None` for every skip reason (not a stylesheet link, non-matching media,
/// missing/empty href, budget exhausted, fetch failure, non-CSS response) —
/// see module docs' Totality section.
fn collect_link_sheet(el: &Element, base: &Url, viewport_width_px: f32, link_fetches: &mut usize) -> Option<Stylesheet> {
    if !is_stylesheet_link(el) {
        return None;
    }
    if let Some(media) = el.attrs.get("media") {
        if !media.trim().is_empty() && !style::media_attr_matches(media, viewport_width_px) {
            return None;
        }
    }
    let href = el.attrs.get("href")?.trim();
    if href.is_empty() {
        return None;
    }
    if *link_fetches >= MAX_LINKS {
        return None;
    }
    *link_fetches += 1;

    let css = fetch_link_css(base, href)?;
    Some(style::parse_and_flatten(&css, viewport_width_px))
}

/// Does this `<link>`'s `rel` attribute name it a stylesheet? Case-
/// insensitive, space-separated token match — see module docs.
fn is_stylesheet_link(el: &Element) -> bool {
    match el.attrs.get("rel") {
        Some(rel) => rel.split_ascii_whitespace().any(|tok| tok.eq_ignore_ascii_case("stylesheet")),
        None => false,
    }
}

/// Resolve `href` against `base`, fetch it, and return its body as CSS text
/// — `None` on any failure (unsupported scheme, fetch error, a
/// `Content-Type` that's present and clearly not CSS). A body decodes lossily
/// as UTF-8 (mirrors `main.rs::dump_text`'s own HTML decoding — this
/// codebase doesn't do charset sniffing anywhere) rather than failing on
/// non-UTF-8 bytes outright.
fn fetch_link_css(base: &Url, href: &str) -> Option<String> {
    let url = base.resolve(href);
    let response = fetch_response(&url).ok()?;
    if let Some(content_type) = response.header("content-type") {
        if !content_type.to_ascii_lowercase().contains("css") {
            return None;
        }
    }
    Some(String::from_utf8_lossy(&response.body).into_owned())
}

/// Duplicated from (rather than shared with) `main.rs`/`frames.rs`/
/// `images.rs`'s own copies of this exact helper — same rationale as each of
/// theirs: a small, total, driver-level fetch helper, and one more near-
/// identical copy costs far less than inventing a shared "driver" module for
/// four call sites (see `images.rs::fetch_response`'s own doc comment, which
/// already made this call for three).
fn fetch_response(url: &Url) -> Result<Response, String> {
    match url.scheme().as_str() {
        "file" => FileFetcher::new().fetch(&Request::get(url.clone())).map_err(|e| format!("{e:?}")),
        "http" => Http1Client::new().fetch(&Request::get(url.clone())).map_err(|e| format!("{e:?}")),
        other => Err(format!("unsupported scheme: {other}")),
    }
}

/// Concatenate a `<style>` element's direct `Text` children's raw content —
/// duplicated from `style::author::style_text` (private to that module);
/// same small helper, kept local rather than making that one `pub` for a
/// single external caller.
fn style_text(dom: &Dom, style_id: NodeId) -> String {
    let mut css = String::new();
    if let Node::Element(el) = dom.node(style_id) {
        for &child in &el.children {
            if let Node::Text(t) = dom.node(child) {
                css.push_str(t);
            }
        }
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::style::cascade;
    use crate::style::computed::Display;
    use crate::surface::Color;

    fn find(d: &dom::Dom, tag: &str) -> dom::NodeId {
        fn walk(d: &dom::Dom, id: dom::NodeId, tag: &str) -> Option<dom::NodeId> {
            let el = d.node(id).element()?;
            if el.name.as_str() == tag {
                return Some(id);
            }
            for &c in &el.children {
                if let Some(found) = walk(d, c, tag) {
                    return Some(found);
                }
            }
            None
        }
        walk(d, d.root(), tag).unwrap_or_else(|| panic!("no <{tag}> found"))
    }

    /// Write a temp `.css` file with `content` and return its `file://`
    /// `Url` — mirrors `images.rs::write_temp_png`'s own pattern for a
    /// deterministic, disk-backed fetch target in a unit test.
    fn write_temp_css(name: &str, content: &str) -> Url {
        let path = std::env::temp_dir().join(format!("stele-stylesheets-test-{}-{name}.css", std::process::id()));
        std::fs::write(&path, content).expect("write temp css");
        Url::new(format!("file://{}", path.display()))
    }

    #[test]
    fn empty_dom_yields_no_sheets() {
        let d = dom::Dom::new();
        let base = Url::new("file:///");
        assert!(collect_all_author_sheets(&d, &base, 320.0).is_empty());
    }

    #[test]
    fn no_link_or_style_yields_no_sheets() {
        let d = dom::parser::parse("<p>hello</p>");
        let base = Url::new("file:///");
        assert!(collect_all_author_sheets(&d, &base, 320.0).is_empty());
    }

    // ---- external sheet applies --------------------------------------------

    #[test]
    fn external_link_stylesheet_applies_through_a_real_file_fetch() {
        let css_url = write_temp_css("basic", "p { color: red }");
        let html = format!(r#"<link rel="stylesheet" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 1);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn external_link_href_resolves_relative_to_the_document_base() {
        let css_url = write_temp_css("relative", "p { color: red }");
        let css_path = css_url.path();
        let dir = std::path::Path::new(&css_path).parent().unwrap().to_string_lossy().to_string();
        let filename = std::path::Path::new(&css_path).file_name().unwrap().to_string_lossy().to_string();
        let base = Url::new(format!("file://{dir}/index.html"));

        let html = format!(r#"<link rel="stylesheet" href="{filename}"><p>x</p>"#);
        let d = dom::parser::parse(&html);

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 1);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(255, 0, 0));
    }

    // ---- ordering: link then style, style wins -----------------------------

    #[test]
    fn a_later_style_block_overrides_an_earlier_link_stylesheet() {
        let css_url = write_temp_css("order", "p { color: red }");
        let html = format!(r#"<link rel="stylesheet" href="{}"><style>p {{ color: blue }}</style><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 2);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255), "the later <style> should win the source-order tie over the earlier <link>");
    }

    #[test]
    fn a_later_link_stylesheet_overrides_an_earlier_style_block() {
        let css_url = write_temp_css("order2", "p { color: blue }");
        let html = format!(r#"<style>p {{ color: red }}</style><link rel="stylesheet" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 2);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255), "the later <link> should win the source-order tie over the earlier <style>");
    }

    // ---- <link media="..."> ------------------------------------------------

    #[test]
    fn link_media_attribute_gates_the_whole_sheet_by_viewport() {
        let css_url = write_temp_css("media", "p { display: none }");
        let html = format!(r#"<link rel="stylesheet" href="{}" media="(max-width: 500px)"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let narrow = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(narrow.len(), 1, "narrow viewport should match the link's media query and fetch it");
        let narrow_styles = cascade::cascade(&d, &narrow);
        assert_eq!(narrow_styles[find(&d, "p")].display, Display::None);

        let wide = collect_all_author_sheets(&d, &base, 1024.0);
        assert!(wide.is_empty(), "wide viewport should never even fetch a non-matching-media link");
    }

    #[test]
    fn link_with_no_media_attribute_always_applies() {
        let css_url = write_temp_css("no-media", "p { color: red }");
        let html = format!(r#"<link rel="stylesheet" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        for width in [100.0, 320.0, 1024.0, 4000.0] {
            let sheets = collect_all_author_sheets(&d, &base, width);
            assert_eq!(sheets.len(), 1, "no media attribute should apply at every width ({width})");
        }
    }

    // ---- rel matching --------------------------------------------------------

    #[test]
    fn link_rel_icon_is_never_fetched_as_a_stylesheet() {
        let css_url = write_temp_css("icon", "p { display: none }");
        let html = format!(r#"<link rel="icon" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert!(sheets.is_empty());
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].display, Display::Block, "an unmatched rel must never apply the sheet's rules");
    }

    #[test]
    fn link_rel_missing_is_never_fetched() {
        let css_url = write_temp_css("norel", "p { display: none }");
        let html = format!(r#"<link href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");
        assert!(collect_all_author_sheets(&d, &base, 320.0).is_empty());
    }

    #[test]
    fn link_rel_stylesheet_matches_case_insensitively_and_among_multiple_tokens() {
        let css_url = write_temp_css("multi-rel", "p { color: red }");
        let html = format!(r#"<link rel="Alternate STYLESHEET" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");
        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 1);
    }

    // ---- fetch failure ---------------------------------------------------

    #[test]
    fn a_missing_href_file_is_skipped_not_a_panic() {
        let html = r#"<link rel="stylesheet" href="file:///nonexistent-stele-link-css-xyz.css"><style>p { color: blue }</style><p>x</p>"#;
        let d = dom::parser::parse(html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        // The missing link is skipped, but the sibling <style> still applies.
        assert_eq!(sheets.len(), 1);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn an_empty_href_is_skipped_not_a_panic() {
        let html = r#"<link rel="stylesheet" href=""><p>x</p>"#;
        let d = dom::parser::parse(html);
        let base = Url::new("file:///");
        assert!(collect_all_author_sheets(&d, &base, 320.0).is_empty());
    }

    #[test]
    fn a_link_with_no_href_at_all_is_skipped_not_a_panic() {
        let html = r#"<link rel="stylesheet"><p>x</p>"#;
        let d = dom::parser::parse(html);
        let base = Url::new("file:///");
        assert!(collect_all_author_sheets(&d, &base, 320.0).is_empty());
    }

    #[test]
    fn an_unsupported_scheme_href_is_skipped_not_a_panic() {
        let html = r#"<link rel="stylesheet" href="ftp://example.com/x.css"><p>x</p>"#;
        let d = dom::parser::parse(html);
        let base = Url::new("file:///");
        assert!(collect_all_author_sheets(&d, &base, 320.0).is_empty());
    }

    #[test]
    fn a_non_css_content_type_response_is_skipped() {
        // Write a file whose bytes look like HTML, not CSS -- the sniff is
        // purely via Content-Type (file:// fetches carry none, so THIS test
        // can't exercise that path; it's exercised for real via the http1
        // fetcher in a higher-level integration test instead). Documented
        // here as a gap for the file:// scheme specifically: file:// has no
        // Content-Type header at all, so this module's content-type check is
        // a pure no-op (never skips) on that scheme -- only meaningful over
        // http. This test instead pins the file:// behavior: garbage CSS
        // text still parses (total, not "not CSS" rejected) since there is
        // no header to sniff.
        let css_url = write_temp_css("garbage", "<html>not css</html>");
        let html = format!(r#"<link rel="stylesheet" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");
        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 1, "file:// has no Content-Type to sniff, so this text is parsed (total) rather than rejected");
    }

    // ---- MAX_LINKS cap -----------------------------------------------------

    #[test]
    fn max_links_caps_the_number_of_link_fetches() {
        let mut html = String::new();
        let urls: Vec<Url> = (0..MAX_LINKS + 10)
            .map(|i| {
                let url = write_temp_css(&format!("cap-{i}"), &format!(".c{i} {{ color: red }}"));
                html.push_str(&format!(r#"<link rel="stylesheet" href="{}">"#, url.as_str()));
                url
            })
            .collect();
        let _ = urls;
        html.push_str("<p>x</p>");
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), MAX_LINKS, "must stop fetching new <link> stylesheets past MAX_LINKS");
    }

    #[test]
    fn max_links_does_not_block_style_blocks() {
        // Budget exhaustion on <link> fetches must not prevent a <style>
        // block elsewhere in the document from being collected -- the
        // budget is scoped to <link> fetch ATTEMPTS only.
        let mut html = String::new();
        for i in 0..MAX_LINKS + 5 {
            let url = write_temp_css(&format!("noblock-{i}"), &format!(".c{i} {{ color: red }}"));
            html.push_str(&format!(r#"<link rel="stylesheet" href="{}">"#, url.as_str()));
        }
        html.push_str("<style>p { color: blue }</style><p>x</p>");
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), MAX_LINKS + 1, "MAX_LINKS link sheets plus the one <style> sheet");
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    // ---- @import stays ignored ---------------------------------------------

    #[test]
    fn at_import_inside_a_fetched_sheet_is_ignored_not_followed() {
        let css_url = write_temp_css("import", "@import url(nonexistent-nested.css); p { color: red }");
        let html = format!(r#"<link rel="stylesheet" href="{}"><p>x</p>"#, css_url.as_str());
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");

        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].ignored_at_rules, 1, "@import is an ignored at-rule, not followed as a nested fetch");
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(255, 0, 0), "the rest of the sheet after @import still applies");
    }

    // ---- totality on deep/hostile markup -----------------------------------

    #[test]
    fn deeply_nested_dom_with_links_does_not_abort() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str(r#"<link rel="stylesheet" href="unreachable.css">"#);
        for _ in 0..depth {
            html.push_str("</div>");
        }
        let d = dom::parser::parse(&html);
        let base = Url::new("file:///");
        // Must return (not abort/hang/panic) -- the unreachable href fails
        // to fetch regardless of depth, so an empty Vec is correct either
        // way; the real assertion here is "did not crash."
        let sheets = collect_all_author_sheets(&d, &base, 320.0);
        assert!(sheets.is_empty());
    }
}
