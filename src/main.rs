//! Stele — a document-web browser for the 486.
//!
//! With no arguments, `main` prints the M0 hello (acceptance A4's golden;
//! `goldens/m0-hello.txt`) — untouched by this packet. `--headless
//! --dump-text <path-or-url> [--cols N]` (P7, M2) drives the full
//! fetch->parse->cascade->box-tree->layout->tty pipeline and prints the
//! resulting text grid: fetch (`file://`/bare local path or `http://` via
//! the P3 fetch layer) -> `dom::parser::parse` -> `style::cascade::cascade`
//! (no author sheets yet — that's a later packet) -> `layout::box_tree::
//! build_box_tree` -> `layout::layout` -> `backend::tty::render`.
//!
//! There is, and by construction will be, no engine anywhere in this
//! program that runs code shipped by the wire (charter C3).

use std::collections::HashMap;

use stele::backend::fb;
use stele::backend::raster;
use stele::backend::tty;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::http1::Http1Client;
use stele::fetch::{Fetch, Request, Response, Url};
use stele::frames;
use stele::layout::box_tree::build_box_tree;
use stele::layout::{self, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

/// Default terminal width in character cells for `--dump-text` when
/// `--cols` isn't given.
const DEFAULT_COLS: usize = 80;

/// A tall-but-bounded viewport height for headless layout: real height is
/// always content-derived (`layout::block`'s root box stretches to content,
/// never clamped to a fixed viewport height in M2), so this value is never
/// actually load-bearing — see `layout::block::layout_tree`'s doc comments.
const HEADLESS_VIEWPORT_HEIGHT: f32 = 100_000.0;

/// Fixed viewport width (CSS px) `--dump-png` lays out at, absent any CLI
/// flag to override it (M4 pixel foundation scope: no `--width` yet — a
/// later packet can add one). 800px is a common-enough "screenshot" width
/// for a document-web page and, not coincidentally, exactly 100 columns at
/// the 8px-per-column `text::BitmapFont::vga_8x16` cell width `--dump-text`
/// already keys its own layout off of.
const DEFAULT_PNG_WIDTH: u32 = 800;

/// Hard cap on the PNG surface's content-driven height, independent of the
/// document. Mirrors `backend::tty::MAX_GRID_ROWS`'s rationale: a hostile or
/// merely huge document must not drive an unbounded `MemSurface`
/// allocation (`width * height * 4` bytes). 20,000px is a page taller than
/// any real fixture is ever going to produce (`fixtures/basic.html` is a
/// few hundred px) while keeping the worst case (`800 * 20_000 * 4` == 64MB)
/// bounded.
const MAX_PNG_HEIGHT: u32 = 20_000;

/// Fallback viewport width (CSS px) for `--render-fb` when the framebuffer's
/// own geometry (`/sys/class/graphics/fb0/virtual_size`) can't be read —
/// e.g. no `fbdev`/`vesafb`/`simplefb` driver loaded. Picked for the same
/// reason as `DEFAULT_PNG_WIDTH`: a common-enough real framebuffer width
/// (many VESA/console modes are 1024px wide or wider) that still keeps the
/// worst-case `MemSurface` allocation bounded alongside `MAX_PNG_HEIGHT`.
const DEFAULT_FB_WIDTH: u32 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    headless: bool,
    dump_text: Option<String>,
    dump_png: Option<(String, String)>,
    render_fb: Option<String>,
    cols: usize,
}

impl Default for Args {
    fn default() -> Self {
        Args { headless: false, dump_text: None, dump_png: None, render_fb: None, cols: DEFAULT_COLS }
    }
}

/// Parse `argv` (already stripped of `argv[0]`) into [`Args`]. Total: total
/// over any input, no std dependency beyond `String`/`str` (brief: "don't
/// pull clap"). Unrecognized flags are ignored rather than erroring — a
/// headless text browser for hostile/1996-era fixtures should degrade, not
/// hard-fail, on an unexpected argument.
fn parse_args(argv: &[String]) -> Args {
    let mut out = Args::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--headless" => out.headless = true,
            "--dump-text" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.dump_text = Some(v.clone());
                }
            }
            "--dump-png" => {
                // Two positional args follow: <src> <out.png>. A missing
                // trailing value (either or both absent) leaves `dump_png`
                // at `None` rather than partially populating it — matching
                // `--dump-text`'s own "trailing flag, no value" totality.
                let src = argv.get(i + 1).cloned();
                let out_path = argv.get(i + 2).cloned();
                if let (Some(src), Some(out_path)) = (src, out_path) {
                    i += 2;
                    out.dump_png = Some((src, out_path));
                }
            }
            "--render-fb" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.render_fb = Some(v.clone());
                }
            }
            "--cols" => {
                i += 1;
                if let Some(v) = argv.get(i).and_then(|s| s.parse::<usize>().ok()) {
                    out.cols = v;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Resolve a CLI-supplied source into a fetchable [`Url`]: `http://`/
/// `file://` pass through unchanged; anything else (no recognized scheme,
/// e.g. `fixtures/basic.html` or `/abs/path.html`) is treated as a local
/// filesystem path and turned into an absolute `file://` URL — relative
/// paths are resolved against the current working directory first, since
/// `fetch::file::file_path` expects `file:///abs/path` shaped input (a bare
/// `file://relative/path` would misparse the first path segment as a host).
fn resolve_url(raw: &str) -> Url {
    let scheme = Url::new(raw).scheme();
    if scheme == "http" || scheme == "file" {
        return Url::new(raw);
    }
    let path = std::path::Path::new(raw);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    Url::new(format!("file://{}", abs.display()))
}

/// Fetch `url` over whichever of the two live schemes it names, returning
/// the full [`Response`] (not just the body) — `dump_png` needs
/// `Response::final_url` (see its own doc comment: review finding,
/// Important) to resolve document-relative `<img src>`s against the
/// POST-redirect URL, not the request URL. Every other scheme (including
/// `https`, which this build never serves — no TLS, ever, per the charter)
/// is a clean `Err`, never a panic.
fn fetch_response(url: &Url) -> Result<Response, String> {
    match url.scheme().as_str() {
        "file" => FileFetcher::new().fetch(&Request::get(url.clone())).map_err(|e| format!("{e:?}")),
        "http" => Http1Client::new().fetch(&Request::get(url.clone())).map_err(|e| format!("{e:?}")),
        other => Err(format!("unsupported scheme: {other}")),
    }
}

/// `fetch_response`'s body only — `dump_text` has no use for `final_url`
/// (it never fetches images), so it keeps this simpler shape.
fn fetch_body(url: &Url) -> Result<Vec<u8>, String> {
    fetch_response(url).map(|r| r.body)
}

/// Drive the full headless pipeline for `--dump-text`. Total: a fetch
/// error, non-UTF-8 body (lossily recovered), empty document, or
/// `display: none` root all resolve to a clean empty string rather than a
/// panic — the caller prints whatever comes back verbatim.
///
/// Frames (packet `frames`): if the fetched document's `<html>` contains a
/// `<frameset>` anywhere (`stele::frames::find_frameset`), this routes to
/// the frames renderer (`stele::frames::render`) INSTEAD of the ordinary
/// cascade->box-tree->layout->tty chain below — a frameset document has no
/// `<body>` to run that chain over; each `<frame src>` gets its own
/// independent instance of it, recursively, driven from `frames.rs`. See
/// that module's docs for the full design (track sizing, compositing,
/// totality bounds).
fn dump_text(source: &str, cols: usize) -> String {
    let url = resolve_url(source);
    let body = match fetch_body(&url) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    let html = String::from_utf8_lossy(&body);
    let dom_tree = dom::parser::parse(&html);

    if let Some(frameset_id) = frames::find_frameset(&dom_tree) {
        return frames::render(&url, &dom_tree, frameset_id, cols).to_text();
    }

    let styles = cascade::cascade(&dom_tree, &[]);
    // A tty dump never paints pixels, so skip the image fetch+decode
    // pre-pass entirely (an empty map — every <img> stays its `[alt]`-style
    // placeholder) rather than paying needless network/decode cost.
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

/// A minimal valid single white pixel, PNG-encoded — the clean fallback
/// `dump_png` returns for a fetch error, an empty/`display:none` document,
/// or a frameset (pixel rendering of `<frameset>` documents is out of scope
/// for this packet; frames get real pixels in a later one). Mirrors
/// `dump_text`'s own "clean empty string, never a panic" totality contract,
/// just in PNG-shaped terms (there's no equivalent of an empty string for a
/// raster image — a 1x1 blank canvas is the smallest well-formed PNG this
/// module ever needs to produce, and `raster::encode_png` itself keys off
/// exactly this same "1x1 white" fallback for a zero-dimension surface).
fn blank_png() -> Vec<u8> {
    raster::encode_png(&MemSurface::new(1, 1, Color::WHITE))
}

/// Drive the full headless pixel pipeline for `--dump-png`: fetch, parse,
/// cascade, box-tree, layout at a fixed-width/content-height viewport, paint
/// fragments onto a `MemSurface`, and PNG-encode it. Total, mirroring
/// `dump_text`'s own contract: a fetch error, empty document, or frameset
/// document all resolve to [`blank_png`] rather than a panic. Frames: same
/// scope call as `blank_png`'s doc comment — a `<frameset>` document has no
/// single `layout::layout` call to drive (see `dump_text`'s own frames
/// carve-out), and wiring the frames compositor to pixels is a follow-up,
/// not this packet's job.
fn dump_png(source: &str) -> Vec<u8> {
    let url = resolve_url(source);
    let response = match fetch_response(&url) {
        Ok(r) => r,
        Err(_) => return blank_png(),
    };
    let html = String::from_utf8_lossy(&response.body);
    let dom_tree = dom::parser::parse(&html);

    if frames::find_frameset(&dom_tree).is_some() {
        return blank_png();
    }

    let styles = cascade::cascade(&dom_tree, &[]);
    // Pixels matter on this path: fetch+decode every <img src> up front
    // (bounded by images::MAX_IMAGES/MAX_TOTAL_IMAGE_BYTES) so
    // build_box_tree can thread real pixel data into each Replaced box.
    // Resolved against `response.final_url` (review finding, Important),
    // NOT the pre-redirect `url`: a document-relative <img src> must
    // resolve against wherever the document actually ended up after any
    // HTTP redirect, not where it was originally requested from.
    let images = stele::images::collect_images(&dom_tree, &response.final_url);
    let Some(root) = build_box_tree(&dom_tree, &styles, &images) else {
        return blank_png();
    };

    let width = DEFAULT_PNG_WIDTH;
    let viewport = Size { w: width as f32, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);

    // Content-driven height: the tallest fragment bottom edge, mirroring
    // `backend::tty::render`'s own `rows_needed` derivation (max over ALL
    // fragments, not just Text, so a bare background Box taller than its
    // text still sizes the canvas) — clamped finite/non-negative/bounded
    // the same defensive way, since a fragment rect's `size.h`/`origin.y`
    // are ultimately document/layout-controlled.
    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let y = f.rect.origin.y;
        let h = f.rect.size.h;
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let height = if content_bottom.is_finite() && content_bottom > 0.0 {
        (content_bottom.ceil() as u32).clamp(1, MAX_PNG_HEIGHT)
    } else {
        1
    };

    let mut surface = MemSurface::new(width, height, Color::WHITE);
    raster::paint(&mut surface, &fragments);
    raster::encode_png(&surface)
}

/// `--dump-png <src> <out.png>`'s CLI-facing wrapper: render `source` and
/// write the PNG bytes to `out_path`. The render half ([`dump_png`]) is
/// total (never fails); the only failure mode here is the filesystem write,
/// reported as a clean `Err` rather than a panic (e.g. an unwritable
/// directory, a hostile/invalid `out_path`).
fn write_dump_png(source: &str, out_path: &str) -> Result<(), String> {
    let bytes = dump_png(source);
    std::fs::write(out_path, bytes).map_err(|e| format!("{e}"))
}

/// Drive the fetch->parse->cascade->(image pre-pass)->box_tree->layout->paint
/// pipeline for `--render-fb`, laying out at a fixed `width` (the
/// framebuffer's own width when known, [`DEFAULT_FB_WIDTH`] otherwise — see
/// [`render_fb`]) and a content-driven height, mirroring [`dump_png`]'s own
/// viewport/height derivation. Unlike `dump_png` (which is total and always
/// returns *some* PNG, even a blank one), this returns `Err` on a fetch
/// failure, an empty/`display:none` document, or a frameset document: there
/// is no pixel-sensible "blank screen" fallback to paint onto real hardware
/// the way there's a trivial 1x1 blank PNG to encode, and the CLI layer
/// ([`render_fb`]) reports whichever of these `Err`s comes back rather than
/// silently painting nothing.
fn render_fb_surface(source: &str, width: u32) -> Result<MemSurface, String> {
    let url = resolve_url(source);
    let response = fetch_response(&url)?;
    let html = String::from_utf8_lossy(&response.body);
    let dom_tree = dom::parser::parse(&html);

    if frames::find_frameset(&dom_tree).is_some() {
        return Err("frameset documents are not supported by --render-fb".to_string());
    }

    let styles = cascade::cascade(&dom_tree, &[]);
    let images = stele::images::collect_images(&dom_tree, &response.final_url);
    let Some(root) = build_box_tree(&dom_tree, &styles, &images) else {
        return Err("empty document (nothing to render)".to_string());
    };

    let viewport = Size { w: width as f32, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);

    // Content-driven height -- same derivation as dump_png's own (see its
    // doc comment for the full rationale).
    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let y = f.rect.origin.y;
        let h = f.rect.size.h;
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let height = if content_bottom.is_finite() && content_bottom > 0.0 {
        (content_bottom.ceil() as u32).clamp(1, MAX_PNG_HEIGHT)
    } else {
        1
    };

    let mut surface = MemSurface::new(width, height, Color::WHITE);
    raster::paint(&mut surface, &fragments);
    Ok(surface)
}

/// `--render-fb <src>`'s CLI-facing driver: render `source` to a
/// `MemSurface` sized to the real framebuffer's width (read from
/// `backend::fb::DEFAULT_SYSFS_DIR`; falls back to [`DEFAULT_FB_WIDTH`] and
/// reports the geometry error to stderr if sysfs is unreadable -- e.g. no fb
/// driver loaded), convert it to the device's own pixel layout, and write it
/// to `backend::fb::DEFAULT_DEVICE_PATH`.
///
/// Total: every failure mode (fetch error, empty/frameset document,
/// unreadable framebuffer geometry, unsupported `bits_per_pixel`, an absent
/// or unwritable `/dev/fb0`) is a clean `Err(String)`, never a panic --
/// this is the path brief calls out as un-integration-testable in CI (no
/// `/dev/fb0` on the runner), so its device-facing half is only ever
/// exercised via this same error path there.
fn render_fb(source: &str) -> Result<(), String> {
    let fb_info = fb::read_fb_info(fb::DEFAULT_SYSFS_DIR);
    let width = match &fb_info {
        Ok(info) => info.width,
        Err(e) => {
            eprintln!("stele: framebuffer geometry unavailable ({e}); using default width {DEFAULT_FB_WIDTH}");
            DEFAULT_FB_WIDTH
        }
    };

    let surface = render_fb_surface(source, width)?;
    let info = fb_info.map_err(|e| e.to_string())?;
    let (surf_w, surf_h) = stele::surface::Surface::size(&surface);
    let bytes = fb::convert_to_fb_bytes(surface.bytes(), surf_w, surf_h, info).map_err(|e| e.to_string())?;
    fb::write_to_device(&bytes, fb::DEFAULT_DEVICE_PATH).map_err(|e| e.to_string())
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        println!("{}", stele::HELLO_LINE);
        return;
    }

    let args = parse_args(&argv);
    if args.headless {
        if let Some(source) = args.dump_text {
            println!("{}", dump_text(&source, args.cols));
            return;
        }
        if let Some((source, out_path)) = args.dump_png {
            if let Err(e) = write_dump_png(&source, &out_path) {
                eprintln!("stele: --dump-png failed: {e}");
            }
            return;
        }
        if let Some(source) = args.render_fb {
            if let Err(e) = render_fb(&source) {
                eprintln!("stele: no framebuffer (/dev/fb0): {e}");
                std::process::exit(1);
            }
            return;
        }
        eprintln!("stele: --headless requires --dump-text <path-or-url>, --dump-png <path-or-url> <out.png>, or --render-fb <path-or-url>");
        return;
    }

    // No recognized mode: fall back to the M0 hello rather than erroring —
    // keeps `stele --nonsense` from ever panicking (totality applies to the
    // CLI surface too).
    println!("{}", stele::HELLO_LINE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_defaults_when_empty() {
        let a = parse_args(&[]);
        assert_eq!(a, Args::default());
    }

    #[test]
    fn parse_args_reads_headless_dump_text_and_cols() {
        let a = parse_args(&args(&["--headless", "--dump-text", "fixtures/basic.html", "--cols", "40"]));
        assert!(a.headless);
        assert_eq!(a.dump_text.as_deref(), Some("fixtures/basic.html"));
        assert_eq!(a.cols, 40);
    }

    #[test]
    fn parse_args_defaults_cols_to_80_when_not_given() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html"]));
        assert_eq!(a.cols, DEFAULT_COLS);
    }

    #[test]
    fn parse_args_ignores_unrecognized_flags_rather_than_failing() {
        let a = parse_args(&args(&["--wat", "--headless"]));
        assert!(a.headless);
    }

    #[test]
    fn parse_args_trailing_flag_with_missing_value_does_not_panic() {
        let a = parse_args(&args(&["--dump-text"]));
        assert_eq!(a.dump_text, None);
        let a2 = parse_args(&args(&["--cols"]));
        assert_eq!(a2.cols, DEFAULT_COLS);
    }

    #[test]
    fn parse_args_non_numeric_cols_falls_back_to_default() {
        let a = parse_args(&args(&["--cols", "not-a-number"]));
        assert_eq!(a.cols, DEFAULT_COLS);
    }

    #[test]
    fn resolve_url_passes_through_http_and_file_schemes() {
        assert_eq!(resolve_url("http://example.com/x").as_str(), "http://example.com/x");
        assert_eq!(resolve_url("file:///abs/path.html").as_str(), "file:///abs/path.html");
    }

    #[test]
    fn resolve_url_turns_a_bare_relative_path_into_an_absolute_file_url() {
        let url = resolve_url("fixtures/basic.html");
        assert_eq!(url.scheme(), "file");
        assert!(url.as_str().ends_with("fixtures/basic.html"));
        assert!(url.path().starts_with('/'), "resolved to an absolute path: {}", url.path());
    }

    #[test]
    fn resolve_url_turns_a_bare_absolute_path_into_a_file_url() {
        let url = resolve_url("/abs/path.html");
        assert_eq!(url.scheme(), "file");
        assert_eq!(url.path(), "/abs/path.html");
    }

    #[test]
    fn dump_text_over_file_fetch_matches_the_tty_golden() {
        let golden = include_str!("../goldens/basic.tty.txt");
        let text = dump_text("fixtures/basic.html", 80);
        assert_eq!(text, golden.trim_end_matches('\n'));
    }

    #[test]
    fn dump_text_on_a_missing_file_is_a_clean_empty_string_not_a_panic() {
        assert_eq!(dump_text("fixtures/does-not-exist-nope.html", 80), "");
    }

    #[test]
    fn dump_text_on_an_unsupported_scheme_is_a_clean_empty_string() {
        assert_eq!(dump_text("ftp://example.com/x", 80), "");
    }

    #[test]
    fn narrower_cols_clip_wide_lines_more_than_default() {
        let narrow = dump_text("fixtures/basic.html", 10);
        let wide = dump_text("fixtures/basic.html", 80);
        assert_ne!(narrow, wide);
        for line in narrow.lines() {
            assert!(line.chars().count() <= 10, "line exceeds requested cols: {line:?}");
        }
    }

    // ------------------------------------------------------------- --dump-png

    #[test]
    fn parse_args_reads_dump_png_source_and_out_path() {
        let a = parse_args(&args(&["--headless", "--dump-png", "fixtures/basic.html", "/tmp/out.png"]));
        assert!(a.headless);
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
    }

    #[test]
    fn parse_args_dump_png_missing_out_path_does_not_panic_or_partially_set() {
        let a = parse_args(&args(&["--dump-png", "fixtures/basic.html"]));
        assert_eq!(a.dump_png, None);
    }

    fn decode_png_dims(bytes: &[u8]) -> (u32, u32) {
        let decoder = png::Decoder::new(bytes);
        let reader = decoder.read_info().expect("dump_png must always produce a valid PNG");
        (reader.info().width, reader.info().height)
    }

    fn decode_png_pixels(bytes: &[u8]) -> Vec<u8> {
        let decoder = png::Decoder::new(bytes);
        let mut reader = decoder.read_info().expect("dump_png must always produce a valid PNG");
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("valid PNG frame");
        buf.truncate(info.buffer_size());
        buf
    }

    /// A tiny one-shot HTTP fixture server (self-contained here rather than
    /// reused from `tests/support` — that helper lives outside `src/` and is
    /// out of reach for this bin crate's own `#[cfg(test)]` module) serving
    /// exactly the routes the redirect regression test below needs: `/go`
    /// 302-redirects to `/sub/page.html`, which references a RELATIVE `<img
    /// src="pic.png">`; `/sub/pic.png` serves a tiny solid-red PNG. The
    /// `/sub/` path segment is deliberate: resolving the relative `pic.png`
    /// against the correct (post-redirect) base `/sub/page.html` lands on
    /// `/sub/pic.png` (served, red); resolving it against the WRONG
    /// (pre-redirect) base `/go` instead lands on `/pic.png` (deliberately
    /// unserved here — a 404) — so this test actually distinguishes the two
    /// bases, unlike a same-directory redirect would. Loops accepting
    /// connections forever on a background thread; the test process exits
    /// long before the OS needs the socket back.
    fn spawn_redirect_image_server() -> std::net::SocketAddr {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect fixture server");
        let addr = listener.local_addr().expect("local_addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");

                let (status, extra_headers, content_type, body): (&str, Vec<(&str, String)>, &str, Vec<u8>) = match path {
                    "/go" => ("302 Found", vec![("Location", "/sub/page.html".to_string())], "text/plain", Vec::new()),
                    "/sub/page.html" => (
                        "200 OK",
                        Vec::new(),
                        "text/html",
                        b"<img src=\"pic.png\" width=\"2\" height=\"2\" alt=\"red\">".to_vec(),
                    ),
                    "/sub/pic.png" => {
                        let s = MemSurface::new(2, 2, Color::rgb(200, 30, 30));
                        ("200 OK", Vec::new(), "image/png", raster::encode_png(&s))
                    }
                    // Deliberately unserved: this is where the relative
                    // `pic.png` would wrongly resolve to if collect_images
                    // used the pre-redirect `/go` as its base URL.
                    _ => ("404 Not Found", Vec::new(), "text/plain", b"not found".to_vec()),
                };

                let mut out = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in &extra_headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = stream.write_all(out.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        addr
    }

    /// Review finding (Important): `dump_png` used to resolve `<img src>`
    /// against the PRE-redirect request URL (`fetch_body` discarded
    /// `Response::final_url`), so a document reached via an HTTP redirect
    /// had its document-relative image sources resolve against the wrong
    /// base and 404 — a wrong (not a crashing) render. `/go` redirects to
    /// `/page.html`, whose relative `pic.png` only resolves correctly
    /// against the post-redirect URL; this must show the decoded red pixel,
    /// not a bare placeholder box.
    #[test]
    fn dump_png_resolves_relative_img_src_against_the_post_redirect_final_url() {
        let addr = spawn_redirect_image_server();
        let bytes = dump_png(&format!("http://{addr}/go"));
        let pixels = decode_png_pixels(&bytes);
        assert!(
            pixels.chunks(4).any(|p| p == [200, 30, 30, 255]),
            "expected the redirected page's relative <img> to have decoded (red pixel present)"
        );
    }

    #[test]
    fn dump_png_over_file_fetch_produces_a_valid_png_at_the_default_width() {
        let bytes = dump_png("fixtures/basic.html");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        let (w, h) = decode_png_dims(&bytes);
        assert_eq!(w, DEFAULT_PNG_WIDTH);
        assert!(h > 0, "content-driven height should be nonzero for a real document");
    }

    #[test]
    fn dump_png_on_a_missing_file_is_a_clean_blank_png_not_a_panic() {
        let bytes = dump_png("fixtures/does-not-exist-nope.html");
        assert_eq!(bytes, blank_png());
    }

    #[test]
    fn dump_png_on_an_unsupported_scheme_is_a_clean_blank_png() {
        let bytes = dump_png("ftp://example.com/x");
        assert_eq!(bytes, blank_png());
    }

    #[test]
    fn dump_png_on_a_frameset_document_is_a_clean_blank_png() {
        // Pixel rendering of framesets is out-of-scope for this packet (see
        // dump_png's doc comment) -- must degrade cleanly, not panic.
        let bytes = dump_png("fixtures/frames.html");
        assert_eq!(bytes, blank_png());
    }

    #[test]
    fn write_dump_png_writes_valid_png_bytes_to_disk() {
        let out = std::env::temp_dir().join(format!("stele-test-{}.png", std::process::id()));
        let out_str = out.to_string_lossy().to_string();
        write_dump_png("fixtures/basic.html", &out_str).expect("write should succeed");
        let on_disk = std::fs::read(&out).expect("file should exist");
        assert_eq!(on_disk, dump_png("fixtures/basic.html"));
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn write_dump_png_to_an_unwritable_path_is_a_clean_err_not_a_panic() {
        let result = write_dump_png("fixtures/basic.html", "/nonexistent-dir-xyz/out.png");
        assert!(result.is_err());
    }

    #[test]
    fn blank_png_is_a_valid_1x1_png() {
        let bytes = blank_png();
        let (w, h) = decode_png_dims(&bytes);
        assert_eq!((w, h), (1, 1));
    }

    // ------------------------------------------------------------ --render-fb

    #[test]
    fn parse_args_reads_render_fb_source() {
        let a = parse_args(&args(&["--headless", "--render-fb", "fixtures/basic.html"]));
        assert!(a.headless);
        assert_eq!(a.render_fb.as_deref(), Some("fixtures/basic.html"));
    }

    #[test]
    fn parse_args_render_fb_missing_value_does_not_panic_or_partially_set() {
        let a = parse_args(&args(&["--headless", "--render-fb"]));
        assert!(a.headless);
        assert_eq!(a.render_fb, None);
    }

    /// The core of the totality contract this packet exists to prove:
    /// there is no `/dev/fb0` (and no `/sys/class/graphics/fb0`) in this
    /// sandbox, mirroring the CI runner exactly. `render_fb` must still
    /// degrade to a clean `Err`, never a panic/abort, even though it drives
    /// the full fetch->parse->cascade->box-tree->layout->paint pipeline
    /// before ever touching the (absent) device.
    #[test]
    fn render_fb_on_a_real_document_with_no_framebuffer_device_is_a_clean_err() {
        let result = render_fb("fixtures/basic.html");
        assert!(result.is_err(), "expected Err: no /dev/fb0 in this environment");
    }

    #[test]
    fn render_fb_on_a_missing_file_is_a_clean_err_not_a_panic() {
        let result = render_fb("fixtures/does-not-exist-nope.html");
        assert!(result.is_err());
    }

    #[test]
    fn render_fb_on_an_unsupported_scheme_is_a_clean_err() {
        let result = render_fb("ftp://example.com/x");
        assert!(result.is_err());
    }

    #[test]
    fn render_fb_on_a_frameset_document_is_a_clean_err_not_a_panic() {
        // Mirrors dump_png's own frameset carve-out (pixel rendering of
        // <frameset> documents is out of scope here too), just surfaced as
        // an Err instead of a blank-PNG fallback -- see render_fb_surface's
        // doc comment for why there's no pixel-sensible blank-screen
        // fallback to paint onto real hardware.
        let result = render_fb("fixtures/frames.html");
        assert!(result.is_err());
    }

    #[test]
    fn render_fb_surface_paints_a_real_document_at_the_requested_width() {
        let surface = render_fb_surface("fixtures/basic.html", 640).expect("basic.html renders");
        assert_eq!(stele::surface::Surface::size(&surface).0, 640);
        assert!(stele::surface::Surface::size(&surface).1 > 0);
    }

    #[test]
    fn render_fb_surface_on_a_missing_file_is_a_clean_err_not_a_panic() {
        let result = render_fb_surface("fixtures/does-not-exist-nope.html", 640);
        assert!(result.is_err());
    }
}
