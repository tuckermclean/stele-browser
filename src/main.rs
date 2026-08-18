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
use std::os::unix::net::UnixStream;
use std::path::Path;

use stele::backend::fb;
use stele::backend::raster;
use stele::backend::tty;
use stele::browser;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::http1::Http1Client;
use stele::fetch::{Fetch, Request, Response, Url};
use stele::frames;
use stele::layout::box_tree::build_box_tree;
use stele::layout::{self, Size};
use stele::style::{self, cascade};
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
    /// `--x11 <path-or-url>` (packet/x11): launch the interactive pixel
    /// shell against a real X11 window (`run_x11`) instead of the tty
    /// shell. Independent of `headless`/`source` -- checked first in
    /// `main`, same "first recognized mode wins" posture as `--dump-text`/
    /// `--dump-png`/`--render-fb` already have under `--headless`.
    x11: Option<String>,
    cols: usize,
    /// `--stats` (M5, C2 "count what we refuse"): print the aggregated
    /// author-CSS-refusal counts to STDERR after cascading — see
    /// [`print_stats`]'s doc comment. Only consulted alongside `--dump-text`/
    /// `--dump-png`, per the packet brief.
    stats: bool,
    /// `--no-bg-images` (packet bg-image): the kill switch — when set, the
    /// pixel paths (`--dump-png`/`--render-fb`) skip the `background-image`
    /// fetch+decode pre-pass entirely (an empty map, see
    /// [`dump_png_opts`]/[`render_fb_surface_opts`]) rather than fetching
    /// and painting any of them; every box still shows its
    /// `background_color`. Default `false` (bg-images ON) — a hostile page
    /// stuffing huge/numerous images into `background-image` is exactly the
    /// worst-case this flag exists to let a user nuke in one shot, but
    /// that's an opt-IN degradation, not the default. `--dump-text`/the
    /// interactive shell never consult this at all (see `bg_images` module
    /// docs: pixel-only, the tty backend has no use for decoded image
    /// pixels).
    no_bg_images: bool,
    /// packet/shell-keyboard: the first bare (non-`--flag`) argument, e.g.
    /// `stele fixtures/basic.html` or `stele http://example.com/` — when
    /// `headless` is `false` and this is `Some`, `main` launches the
    /// interactive shell on it (see [`run_browser`]) instead of falling
    /// back to the M0 hello. Not consulted at all in `--headless` mode
    /// (those paths read `dump_text`/`dump_png`/`render_fb` directly).
    source: Option<String>,
    /// `--color-scheme <light|dark|auto>` (packet t1b-color-scheme): the
    /// resolved [`style::ColorScheme`] every headless render path evaluates
    /// `prefers-color-scheme` media queries against (see
    /// `style::media::Feature::PrefersColorScheme`) — ALWAYS consulted,
    /// even when the flag is absent (absent defaults to `Light`, the same
    /// "no OS/JS signal" posture `ColorScheme::parse`'s `auto` arm
    /// documents). `"auto"` and any unrecognized value both resolve to
    /// `Light` too (`ColorScheme::parse` is total and fails closed to the
    /// honest default — never a panic, never a hard CLI error).
    color_scheme: style::ColorScheme,
    /// Whether `--color-scheme` was actually given on the command line, as
    /// opposed to `color_scheme` merely holding its default `Light` —
    /// separate from `color_scheme` itself because it gates a SECOND,
    /// independent behavior: the pre-cascade `data-theme`/`data-mode`
    /// root-attribute stamp (`stamp_color_scheme`, called from
    /// `dump_text_opts`/`dump_png_opts`/`render_fb_surface_opts`).
    /// Stamping is flag-gated rather than unconditional so the DEFAULT
    /// (no flag at all) render path stays byte-for-byte identical to every
    /// already-blessed golden — see `stamp_color_scheme`'s own doc comment
    /// for the full golden-churn rationale.
    color_scheme_given: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            headless: false,
            dump_text: None,
            dump_png: None,
            render_fb: None,
            x11: None,
            cols: DEFAULT_COLS,
            stats: false,
            no_bg_images: false,
            source: None,
            color_scheme: style::ColorScheme::Light,
            color_scheme_given: false,
        }
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
            "--x11" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.x11 = Some(v.clone());
                }
            }
            "--cols" => {
                i += 1;
                if let Some(v) = argv.get(i).and_then(|s| s.parse::<usize>().ok()) {
                    out.cols = v;
                }
            }
            "--stats" => out.stats = true,
            "--no-bg-images" => out.no_bg_images = true,
            "--color-scheme" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.color_scheme = style::ColorScheme::parse(v);
                    out.color_scheme_given = true;
                }
                // A trailing `--color-scheme` with no value leaves both
                // fields at their defaults, same "missing value is a no-op,
                // never a panic" totality `--dump-text`/`--cols` already have.
            }
            other => {
                // packet/shell-keyboard: the first bare token (doesn't start
                // with `--`) is the interactive-mode source. Only the FIRST
                // one is captured — `stele fixtures/basic.html extra-noise`
                // still launches on `fixtures/basic.html` rather than
                // silently overwriting it with `extra-noise`, matching this
                // function's overall "ignore anything past what's
                // recognized" posture.
                if out.source.is_none() && !other.starts_with("--") {
                    out.source = Some(other.to_string());
                }
            }
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

// ---------------------------------------------------------------------------
// `--stats` (M5, C2 "count what we refuse"): every collected author sheet
// already carries `ignored_declarations`/`ignored_at_rules`/`media_at_rules`
// counters (`style::parser::Stylesheet` — P2/M5, "feeds the future
// Provenance pane / --stats"); this is the CLI surface that finally sums and
// prints them. STDERR only, never stdout — a `--dump-text`/`--dump-png`
// golden must never see this line, so it's wired as a side effect
// independent of `dump_text`/`dump_png`'s own return values (see
// `print_stats`'s doc comment below for why it re-fetches/re-parses rather
// than threading a flag through those two functions).
// ---------------------------------------------------------------------------

/// One aggregated `--stats` snapshot: what Stele's CSS layer refused,
/// summed across every collected author `<style>` sheet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StatsCounts {
    ignored_declarations: u32,
    ignored_at_rules: u32,
    media_at_rules: u32,
}

/// Sum each `Stylesheet`'s own counters across `sheets`. Total: an empty
/// slice (no author CSS at all) yields all-zero counts, matching
/// `StatsCounts::default()`.
fn aggregate_stats(sheets: &[style::Stylesheet]) -> StatsCounts {
    let mut counts = StatsCounts::default();
    for s in sheets {
        counts.ignored_declarations += s.ignored_declarations;
        counts.ignored_at_rules += s.ignored_at_rules;
        counts.media_at_rules += s.media_at_rules;
    }
    counts
}

/// `"s"` for anything but exactly `1` — keeps [`format_stats_line`]'s output
/// grammatical (`"1 ignored at-rule"`, not `"1 ignored at-rules"`).
fn plural(n: u32) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Render a [`StatsCounts`] snapshot as the one-line, deterministic summary
/// `--stats` prints, e.g. `"stele --stats: 3 ignored declarations, 1
/// ignored at-rule, 2 media blocks"` — the exact format named in the packet
/// brief. `media_at_rules` is worded as "media blocks" (matching the brief's
/// own example) rather than "media at-rules", since "block" is what a reader
/// unfamiliar with CSS at-rule terminology will recognize from `@media { }`.
fn format_stats_line(counts: StatsCounts) -> String {
    format!(
        "stele --stats: {} ignored declaration{}, {} ignored at-rule{}, {} media block{}",
        counts.ignored_declarations,
        plural(counts.ignored_declarations),
        counts.ignored_at_rules,
        plural(counts.ignored_at_rules),
        counts.media_at_rules,
        plural(counts.media_at_rules),
    )
}

/// `--stats`'s CLI-facing driver: fetch + parse `source` (mirroring
/// `dump_text`/`dump_png`'s own fetch step — a fetch failure degrades to an
/// empty document rather than propagating an error, same totality contract),
/// collect every author sheet at `viewport_width_px` exactly like the real
/// render path does, and print the aggregated line to STDERR.
///
/// A SEPARATE fetch+parse+collect pass from `dump_text`/`dump_png` (rather
/// than threading a `stats: bool` through either) — deliberately: those two
/// functions' return values (the golden-compared text/PNG bytes) must stay
/// byte-for-byte identical whether or not `--stats` was passed, and the
/// cheapest way to GUARANTEE that (rather than merely test it) is to give
/// `--stats` its own independent read of the document. The extra fetch/parse
/// is cheap relative to a CLI invocation's own process-startup cost, and
/// `--stats` is diagnostic tooling, not a hot path.
///
/// Total: a fetch failure or a document with no author CSS at all still
/// prints an all-zero line (`aggregate_stats(&[])` is `StatsCounts::default()`)
/// rather than panicking or silently skipping the line.
///
/// Not `--color-scheme`-aware (packet t1b-color-scheme): always collects at
/// `ColorScheme::Light` — this is diagnostic refusal-counting tooling, not a
/// rendered-output path, and a `prefers-color-scheme`-gated `@media` block's
/// declarations are still counted (just under whichever branch is inert)
/// regardless of which scheme flattens it in. Threading `--color-scheme`
/// through here too is a reasonable follow-up, not required for this packet.
fn print_stats(source: &str, viewport_width_px: f32) {
    let url = resolve_url(source);
    let sheets = match fetch_body(&url) {
        Ok(body) => {
            let html = String::from_utf8_lossy(&body);
            let dom_tree = dom::parser::parse(&html);
            style::collect_author_sheets_for_viewport(&dom_tree, viewport_width_px, style::ColorScheme::Light)
        }
        Err(_) => Vec::new(),
    };
    eprintln!("{}", format_stats_line(aggregate_stats(&sheets)));
}

/// Packet t1b-color-scheme: stamp `data-theme="<scheme>"` and
/// `data-mode="<scheme>"` onto the document's root `<html>` element, BEFORE
/// cascade runs — the no-JS approximation of what a real theme-toggle
/// script would do for pages like httpforever.com, whose dark mode is
/// gated entirely on `html[data-theme="dark"]` with no `@media
/// (prefers-color-scheme)` fallback at all. This is an explicit,
/// user-invoked, DOCUMENTED approximation: it sets standard theming hooks
/// the page's own author already wired up for exactly this purpose, not
/// arbitrary invented content, and never runs any of the page's own script
/// (charter C3 is untouched — no JS is parsed or executed anywhere in this
/// pipeline).
///
/// Only called when `--color-scheme` was actually given on the command
/// line (`Args::color_scheme_given`) — the default (no flag) render path
/// never stamps anything, so it stays byte-for-byte identical to every
/// already-blessed golden. (An earlier design considered stamping `light`
/// unconditionally, since `light` IS the honest default; this packet
/// chose the more conservative flag-gated behavior instead, since verifying
/// "no golden churn" for the unconditional-stamp design would require
/// running the full golden suite, which this environment's write-code/
/// push/let-CI-verify workflow doesn't do before opening the PR.)
///
/// Total: delegates entirely to `Dom::set_attribute` (itself total over any
/// node id — see its own doc comment), so this is too, including against
/// `Dom::new()`'s trivial one-node arena or a `Dom` whose root somehow isn't
/// an `Element` at all.
fn stamp_color_scheme(dom_tree: &mut dom::Dom, scheme: style::ColorScheme) {
    let value = match scheme {
        style::ColorScheme::Light => "light",
        style::ColorScheme::Dark => "dark",
    };
    let root = dom_tree.root();
    dom_tree.set_attribute(root, "data-theme", value);
    dom_tree.set_attribute(root, "data-mode", value);
}

/// [`dump_text`]'s real implementation, parameterized over `scheme`
/// (packet t1b-color-scheme's `--color-scheme`) and `stamp` (whether to
/// pre-cascade-stamp `data-theme`/`data-mode` on the root element — see
/// [`stamp_color_scheme`]'s own doc comment for why that's gated
/// separately from `scheme` itself). `dump_text` is a thin wrapper always
/// passing `(ColorScheme::Light, false)`, keeping every existing
/// `dump_text` call site/test unchanged — mirrors `dump_png`/
/// `dump_png_opts`'s own wrapper-over-parameterized-impl split for
/// `--no-bg-images`. `main`'s `--dump-text` branch calls this directly
/// with `args.color_scheme`/`args.color_scheme_given`.
///
/// Total: a fetch error, non-UTF-8 body (lossily recovered), empty
/// document, or `display: none` root all resolve to a clean empty string
/// rather than a panic — the caller prints whatever comes back verbatim.
///
/// Frames (packet `frames`): if the fetched document's `<html>` contains a
/// `<frameset>` anywhere (`stele::frames::find_frameset`), this routes to
/// the frames renderer (`stele::frames::render`) INSTEAD of the ordinary
/// cascade->box-tree->layout->tty chain below — a frameset document has no
/// `<body>` to run that chain over; each `<frame src>` gets its own
/// independent instance of it, recursively, driven from `frames.rs`.
/// `stamp_color_scheme` only ever touches a TOP-level `<html>` (frameset
/// documents route away before it's called — a `<frameset>` document has
/// no single root author-CSS target the way an ordinary document's `<html>`
/// is), so a frameset document's `--color-scheme` support is scoped to the
/// `prefers-color-scheme` media feature `scheme` still threads into
/// `frames::render`, not the attribute stamp. See that module's docs for
/// the frames pipeline's own design (track sizing, compositing, totality
/// bounds).
#[cfg(test)]
fn dump_text(source: &str, cols: usize) -> String {
    dump_text_opts(source, cols, style::ColorScheme::Light, false)
}

fn dump_text_opts(source: &str, cols: usize, scheme: style::ColorScheme, stamp: bool) -> String {
    let url = resolve_url(source);
    // Fetch the full Response (not just the body) — m5-link-css: `<link
    // href>` stylesheets must resolve against the POST-redirect URL, same
    // "review finding, Important" `dump_png`/`render_fb_surface` already
    // apply to `<img src>` (see `stele::images::collect_images`'s call
    // sites) — a document-relative `<link href>` has to resolve against
    // wherever the document actually ended up, not where it was requested.
    let response = match fetch_response(&url) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    let html = String::from_utf8_lossy(&response.body);
    let mut dom_tree = dom::parser::parse(&html);

    if let Some(frameset_id) = frames::find_frameset(&dom_tree) {
        return frames::render(&url, &dom_tree, frameset_id, cols, scheme).to_text();
    }

    if stamp {
        stamp_color_scheme(&mut dom_tree, scheme);
    }

    // M5 + m5-link-css: feed cascade every author sheet in document order —
    // inline <style> blocks AND fetched <link rel=stylesheet href> sheets
    // (stele::stylesheets::collect_all_author_sheets), interleaved by source
    // position. Inline `style=` needs no extra wiring: cascade reads it
    // straight off each Element it already walks. M5 media: viewport WIDTH
    // for a tty dump is `cols * 8px` (the tty cell width) — the collector
    // flattens any `@media` in those sheets (both in-CSS `@media` blocks and
    // a `<link media=...>` attribute) against that width before cascade ever
    // runs. --dump-text DOES want author CSS fetched (it affects
    // `display:none` etc.), unlike the image pre-pass below, which is
    // pixel-only and skipped here.
    let viewport_width = cols as f32 * 8.0;
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, viewport_width, scheme);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    // A tty dump never paints pixels, so skip the image fetch+decode
    // pre-pass entirely (an empty map — every <img> stays its `[alt]`-style
    // placeholder) rather than paying needless network/decode cost.
    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return String::new();
    };
    let viewport = Size { w: viewport_width, h: HEADLESS_VIEWPORT_HEIGHT };
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
#[cfg(test)]
fn dump_png(source: &str) -> Vec<u8> {
    dump_png_opts(source, false)
}

/// [`dump_png`]'s real implementation, parameterized over `no_bg_images`
/// (packet bg-image's `--no-bg-images` kill switch) — `dump_png` itself is a
/// thin wrapper always passing `false` (bg-images ON), keeping every
/// existing `dump_png` call site/test unchanged; `main`'s `--dump-png`
/// branch calls this directly with `args.no_bg_images`.
fn dump_png_opts(source: &str, no_bg_images: bool) -> Vec<u8> {
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

    // M5 + m5-link-css: feed cascade every author sheet in document order —
    // inline <style> blocks AND fetched <link rel=stylesheet href> sheets,
    // resolved/fetched against `response.final_url` (same post-redirect
    // rationale the <img src> fetch below already documents). Inline
    // `style=` needs no extra wiring: cascade reads it straight off each
    // Element it already walks. M5 media: `--dump-png`'s viewport width is
    // the fixed `DEFAULT_PNG_WIDTH` (below) — flatten any `@media` (in-CSS
    // or a `<link media=...>` attribute) against THAT. Not `--color-scheme`-
    // aware (packet t1b-color-scheme scoped that flag to `--dump-text` only
    // — see that packet's PR for the rationale): always `Light` here.
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, DEFAULT_PNG_WIDTH as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
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

    // Packet bg-image: the `background-image` fetch+decode pre-pass, same
    // "resolve against `response.final_url`" rationale as the `<img src>`
    // pre-pass right above. `--no-bg-images` skips it entirely (an empty
    // map — `raster::paint` then paints every box's `background_color`
    // alone, exactly as if no box declared a `background-image` at all).
    let bg_images = if no_bg_images { HashMap::new() } else { stele::bg_images::collect_bg_images(&styles, &response.final_url) };

    let mut surface = MemSurface::new(width, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &bg_images);
    raster::encode_png(&surface)
}

/// `--dump-png <src> <out.png>`'s CLI-facing wrapper: render `source` and
/// write the PNG bytes to `out_path`. The render half ([`dump_png`]) is
/// total (never fails); the only failure mode here is the filesystem write,
/// reported as a clean `Err` rather than a panic (e.g. an unwritable
/// directory, a hostile/invalid `out_path`).
#[cfg(test)]
fn write_dump_png(source: &str, out_path: &str) -> Result<(), String> {
    write_dump_png_opts(source, out_path, false)
}

/// [`write_dump_png`]'s real implementation, parameterized over
/// `no_bg_images` — see [`dump_png_opts`]'s doc comment for the same
/// wrapper-over-parameterized-impl rationale.
fn write_dump_png_opts(source: &str, out_path: &str, no_bg_images: bool) -> Result<(), String> {
    let bytes = dump_png_opts(source, no_bg_images);
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
#[cfg(test)]
fn render_fb_surface(source: &str, width: u32) -> Result<MemSurface, String> {
    render_fb_surface_opts(source, width, false)
}

/// [`render_fb_surface`]'s real implementation, parameterized over
/// `no_bg_images` — see [`dump_png_opts`]'s doc comment for the same
/// wrapper-over-parameterized-impl rationale.
fn render_fb_surface_opts(source: &str, width: u32, no_bg_images: bool) -> Result<MemSurface, String> {
    let url = resolve_url(source);
    let response = fetch_response(&url)?;
    let html = String::from_utf8_lossy(&response.body);
    let dom_tree = dom::parser::parse(&html);

    if frames::find_frameset(&dom_tree).is_some() {
        return Err("frameset documents are not supported by --render-fb".to_string());
    }

    // M5 + m5-link-css: feed cascade every author sheet in document order —
    // inline <style> blocks AND fetched <link rel=stylesheet href> sheets,
    // resolved/fetched against `response.final_url` (same post-redirect
    // rationale the <img src> fetch below already documents). Inline
    // `style=` needs no extra wiring: cascade reads it straight off each
    // Element it already walks. M5 media: `--render-fb`'s viewport width is
    // the real framebuffer width (`width` param) — flatten any `@media`
    // (in-CSS or a `<link media=...>` attribute) against that. Not
    // `--color-scheme`-aware — see `dump_png_opts`'s identical note above.
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, width as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
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

    // Packet bg-image: see `dump_png_opts`'s own doc comment for the exact
    // same rationale (this is `--render-fb`'s copy of that same pre-pass).
    let bg_images = if no_bg_images { HashMap::new() } else { stele::bg_images::collect_bg_images(&styles, &response.final_url) };

    let mut surface = MemSurface::new(width, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &bg_images);
    Ok(surface)
}

/// `--render-fb <src>`'s pipeline, parameterized by the sysfs geometry
/// directory and device node path: render `source` to a `MemSurface` sized
/// to the framebuffer's width (read from `sysfs_dir`; falls back to
/// [`DEFAULT_FB_WIDTH`] and reports the geometry error to stderr if sysfs is
/// unreadable -- e.g. no fb driver loaded), convert it to the device's own
/// pixel layout, and write it to `device_path`.
///
/// Total: every failure mode (fetch error, empty/frameset document,
/// unreadable framebuffer geometry, unsupported `bits_per_pixel`, an absent
/// or unwritable device) is a clean `Err(String)`, never a panic.
///
/// Parameterized (rather than hardcoding `backend::fb::DEFAULT_SYSFS_DIR`/
/// `DEFAULT_DEVICE_PATH` here) so tests can drive the full pipeline against
/// scratch paths deterministically -- whether the REAL `/sys/class/graphics/
/// fb0` and `/dev/fb0` exist on the host running the test suite must never
/// change a test's pass/fail (some CI/build containers do have a real or
/// passed-through fb0, some don't). [`render_fb`] is this closed over the
/// real defaults, for [`main`] to call.
#[cfg(test)]
fn render_fb_to(source: &str, sysfs_dir: &Path, device_path: &Path) -> Result<(), String> {
    render_fb_to_opts(source, sysfs_dir, device_path, false)
}

/// [`render_fb_to`]'s real implementation, parameterized over `no_bg_images`
/// — see [`dump_png_opts`]'s doc comment for the same wrapper-over-
/// parameterized-impl rationale.
fn render_fb_to_opts(source: &str, sysfs_dir: &Path, device_path: &Path, no_bg_images: bool) -> Result<(), String> {
    let fb_info = fb::read_fb_info_from(sysfs_dir);
    let width = match &fb_info {
        Ok(info) => info.width,
        Err(e) => {
            eprintln!("stele: framebuffer geometry unavailable ({e}); using default width {DEFAULT_FB_WIDTH}");
            DEFAULT_FB_WIDTH
        }
    };

    let surface = render_fb_surface_opts(source, width, no_bg_images)?;
    let info = fb_info.map_err(|e| e.to_string())?;
    let (surf_w, surf_h) = stele::surface::Surface::size(&surface);
    let bytes = fb::convert_to_fb_bytes(surface.bytes(), surf_w, surf_h, info).map_err(|e| e.to_string())?;
    fb::write_to_device(device_path, &bytes).map_err(|e| e.to_string())
}

/// `--render-fb <src>`'s CLI-facing driver: [`render_fb_to`] closed over the
/// real `backend::fb::DEFAULT_SYSFS_DIR`/`DEFAULT_DEVICE_PATH`. Not itself
/// unit-tested for the same reason `backend::fb::read_fb_info`/
/// `write_to_device`'s zero-argument forms aren't: there's no
/// environment-independent assertion to make about a call that touches
/// whatever real hardware happens to be present, or not, on this machine --
/// [`render_fb_to`] carries the real test coverage. This is the path the
/// brief calls out as un-integration-testable in CI (no `/dev/fb0`
/// guaranteed on any given runner).
#[cfg(test)]
fn render_fb(source: &str) -> Result<(), String> {
    render_fb_opts(source, false)
}

/// [`render_fb`]'s real implementation, parameterized over `no_bg_images` —
/// `main`'s `--render-fb` branch calls this directly with
/// `args.no_bg_images`.
fn render_fb_opts(source: &str, no_bg_images: bool) -> Result<(), String> {
    render_fb_to_opts(source, Path::new(fb::DEFAULT_SYSFS_DIR), Path::new(fb::DEFAULT_DEVICE_PATH), no_bg_images)
}

// ---------------------------------------------------------------------------
// packet/x11: the pixel-shell page-render pipeline. Mirrors
// `render_fb_surface_opts`'s own fetch->parse->cascade->box-tree->layout->
// paint steps (same content-driven-height derivation) but ALSO returns the
// raw `Fragment` stream alongside the painted `Surface` -- `backend::x11`'s
// pixel hit-test needs the fragments' `Interactive::Link` rects for
// click-to-follow, which the painted pixels alone don't carry.
// ---------------------------------------------------------------------------

/// Render `url` (already resolved -- see `browser::History::current`) into
/// a full-document `MemSurface` at `width` CSS px, alongside the
/// `Fragment`s that produced it. Total: fetch/parse/layout failure is a
/// clean `Err`, never a panic -- `run_x11` degrades to a blank page rather
/// than propagating a panic into the event loop.
fn render_x11_page(url: &Url, width: u32) -> Result<(MemSurface, Vec<layout::Fragment>), String> {
    let response = fetch_response(url)?;
    let html = String::from_utf8_lossy(&response.body);
    let dom_tree = dom::parser::parse(&html);

    if frames::find_frameset(&dom_tree).is_some() {
        return Err("frameset documents are not supported by --x11".to_string());
    }

    // Not `--color-scheme`-aware — see `dump_png_opts`'s identical note.
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, width as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let images = stele::images::collect_images(&dom_tree, &response.final_url);
    let Some(root) = build_box_tree(&dom_tree, &styles, &images) else {
        return Err("empty document (nothing to render)".to_string());
    };

    let viewport = Size { w: width as f32, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);

    // Content-driven height -- same derivation `dump_png_opts`/
    // `render_fb_surface_opts` already use.
    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let y = f.rect.origin.y;
        let h = f.rect.size.h;
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let height = if content_bottom.is_finite() && content_bottom > 0.0 { (content_bottom.ceil() as u32).clamp(1, MAX_PNG_HEIGHT) } else { 1 };

    let bg_images = stele::bg_images::collect_bg_images(&styles, &response.final_url);

    let mut surface = MemSurface::new(width, height, Color::WHITE);
    raster::paint(&mut surface, &fragments, &bg_images);
    Ok((surface, fragments))
}

/// Window pixel width, the drawable's bits-per-pixel, and the format's
/// `scanline-pad` (bits) -> the byte stride `PutImage`'s `ZPixmap` data
/// must use per scanline (rounded UP to `scanline_pad_bits`, per spec —
/// the same padding convention `backend::fb::FbInfo.stride` already
/// encodes, just derived here from the X11 setup reply's `PixmapFormat`
/// instead of `/sys/class/graphics/fb0`'s own `stride` file).
fn x11_row_stride(width: u32, bpp: u32, scanline_pad_bits: u32) -> u32 {
    let pad = scanline_pad_bits.max(8);
    let bits = width.saturating_mul(bpp);
    let padded_bits = bits.div_ceil(pad) * pad;
    padded_bits / 8
}

/// Crop `win_height` rows starting at document row `scroll_y` out of
/// `surface`'s full-document RGBA8 pixels, into a fresh `width *
/// win_height * 4`-byte buffer ready for [`fb::convert_to_fb_bytes`].
/// `backend::fb::convert_to_fb_bytes` alone has no scroll-offset concept
/// (it always clips to the top-left corner) — this is the scroll step in
/// front of it. Rows/columns past `surface`'s own bounds (document shorter
/// than the window, or narrower after a resize) are left `0` (white would
/// need painting; `0`/black-transparent is what an X11 `PutImage` outside
/// real content shows, matching every other "surface smaller than target"
/// convention in this codebase).
fn crop_surface_rows(surface: &MemSurface, width: u32, win_height: u32, scroll_y: u32) -> Vec<u8> {
    let (sw, sh) = stele::surface::Surface::size(surface);
    let bytes = surface.bytes();
    // Fill with opaque WHITE, not black: any window area past the rendered
    // surface (a page shorter than the window, or narrower than its width)
    // is the page canvas, which is white — a zero-fill made the whole area
    // below a short page render as a black window.
    let mut out = vec![0xffu8; (width as usize) * (win_height as usize) * 4];
    let dst_row_bytes = width as usize * 4;
    let src_row_bytes = sw as usize * 4;
    let copy_w_bytes = (width.min(sw) as usize) * 4;

    for row in 0..win_height {
        let src_y = scroll_y + row;
        if src_y >= sh {
            break;
        }
        let dst_off = row as usize * dst_row_bytes;
        let src_off = src_y as usize * src_row_bytes;
        if src_off + copy_w_bytes <= bytes.len() && dst_off + copy_w_bytes <= out.len() {
            out[dst_off..dst_off + copy_w_bytes].copy_from_slice(&bytes[src_off..src_off + copy_w_bytes]);
        }
    }
    out
}

/// How many document pixel rows past the bottom of the window are still
/// scrollable, given the full document height `doc_h` and the current
/// window `win_h` — `0` when the whole document already fits.
fn x11_max_scroll(doc_h: u32, win_h: u32) -> u32 {
    doc_h.saturating_sub(win_h.min(doc_h))
}

/// Default X11 window size (CSS px) — no `--width`/`--height` flag yet (the
/// packet brief doesn't ask for one); `ConfigureNotify` (a user resizing
/// the window) reflows to whatever size the window manager/server actually
/// grants, same as any other X11 client.
const DEFAULT_X11_WIDTH: u32 = 1024;
const DEFAULT_X11_HEIGHT: u32 = 768;

/// Pixels scrolled per arrow-key press / mouse-wheel notch.
const X11_LINE_SCROLL: u32 = 60;

/// `stele --x11 <url>`: open a real X11 window (kdrive/Xfbdev, core
/// protocol only — see `backend::x11`'s own doc comment for the wire
/// details) and drive it interactively: `Expose` repaints the current
/// frame; arrow keys/PageUp/PageDown/mouse wheel scroll (re-cropping +
/// re-`PutImage`ing, no re-layout); `F5` reloads; a left click
/// pixel-hit-tests the current fragment stream and, on a link hit,
/// navigates; `ConfigureNotify` (a resize) re-lays-out at the new width;
/// `q`/Escape quits.
///
/// Deliberately NOT unit-tested — same split as `run_browser`'s own doc
/// comment: there is no X server in CI to open a window against. Every
/// pure decision this loop makes (protocol encode/parse, keysym mapping,
/// pixel hit-test) already IS unit-tested, in `backend::x11`; this
/// function is thin glue over those plus this module's own
/// `render_x11_page`/`crop_surface_rows`/`x11_row_stride`. Bounded/total
/// throughout: every socket read goes through `XConnection`'s own
/// fixed-size-buffer reads, and a page-load/reload/navigate failure prints
/// to stderr and keeps the previous frame on screen rather than panicking.
fn run_x11(source: &str) {
    use stele::backend::x11::{self as xproto, XConnection};

    let mut history = browser::History::new(resolve_url(source));

    let mut conn = match XConnection::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("stele: --x11: failed to connect to the X server: {e}");
            std::process::exit(1);
        }
    };

    let depth = conn.setup.root_depth;
    let Some(format) = conn.format_for_depth(depth) else {
        eprintln!("stele: --x11: X server advertised no usable pixmap formats");
        std::process::exit(1);
    };
    let bpp = format.bits_per_pixel as u32;
    let scanline_pad = format.scanline_pad as u32;
    eprintln!(
        "stele: --x11: connected — root_depth={} root_visual=0x{:x} bpp={} scanline_pad={} max_request_len={}",
        depth, conn.setup.root_visual, bpp, scanline_pad, conn.setup.maximum_request_length
    );

    let mut width = DEFAULT_X11_WIDTH;
    let mut height = DEFAULT_X11_HEIGHT;

    let window = match conn.create_window(width as u16, height as u16) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("stele: --x11: CreateWindow failed: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = conn.map_window(window) {
        eprintln!("stele: --x11: MapWindow failed: {e}");
        std::process::exit(1);
    }
    let gc = match conn.create_gc(window) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("stele: --x11: CreateGC failed: {e}");
            std::process::exit(1);
        }
    };

    // No window manager under Xfbdev, so nothing else will hand us keyboard
    // focus — do it ourselves, or q/Escape/scroll keys never arrive and the
    // only escape from the mapped window is a reboot. Best-effort.
    if let Err(e) = conn.set_input_focus(window) {
        eprintln!("stele: --x11: SetInputFocus failed: {e}");
    }

    // Best-effort: a server that fails GetKeyboardMapping still gets a
    // working (mouse-only) shell rather than a hard exit.
    let (keysyms_per_keycode, keysyms) = conn.get_keyboard_mapping().unwrap_or_else(|e| {
        eprintln!("stele: --x11: GetKeyboardMapping failed ({e}); keyboard input will be inert");
        (0, Vec::new())
    });
    let min_keycode = conn.setup.min_keycode;

    let mut scroll_y: u32 = 0;
    let (mut surface, mut fragments) = match render_x11_page(history.current(), width) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stele: --x11: initial page load failed: {e}");
            (MemSurface::new(width, 1, Color::WHITE), Vec::new())
        }
    };

    let redraw = |conn: &mut XConnection, surface: &MemSurface, width: u32, height: u32, scroll_y: u32| {
        let cropped = crop_surface_rows(surface, width, height, scroll_y);
        let stride = x11_row_stride(width, bpp, scanline_pad);
        let fb_info = fb::FbInfo { width, height, bpp, stride };
        match fb::convert_to_fb_bytes(&cropped, width, height, fb_info) {
            Ok(bytes) => {
                if let Err(e) = conn.put_image(window, gc, width as u16, height as u16, depth, &bytes, stride as usize) {
                    eprintln!("stele: --x11: PutImage failed: {e}");
                }
            }
            Err(e) => eprintln!("stele: --x11: pixel conversion failed: {e}"),
        }
    };

    redraw(&mut conn, &surface, width, height, scroll_y);

    loop {
        let event = match conn.next_event() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("stele: --x11: connection closed: {e}");
                break;
            }
        };

        match event {
            xproto::XEvent::Expose => redraw(&mut conn, &surface, width, height, scroll_y),

            xproto::XEvent::ConfigureNotify { width: w, height: h } => {
                if w == 0 || h == 0 {
                    continue; // a hostile/transient 0-sized geometry -- nothing sane to render
                }
                width = w as u32;
                height = h as u32;
                match render_x11_page(history.current(), width) {
                    Ok((s, f)) => {
                        surface = s;
                        fragments = f;
                    }
                    Err(e) => eprintln!("stele: --x11: reflow after resize failed: {e}"),
                }
                let (_, doc_h) = stele::surface::Surface::size(&surface);
                scroll_y = scroll_y.min(x11_max_scroll(doc_h, height));
                redraw(&mut conn, &surface, width, height, scroll_y);
            }

            xproto::XEvent::KeyPress { keycode, .. } => {
                let Some(sym) = xproto::keysym_for_keycode(keycode, min_keycode, keysyms_per_keycode, &keysyms) else { continue };
                let Some(key) = xproto::keysym_to_key(sym) else { continue };
                let (_, doc_h) = stele::surface::Surface::size(&surface);
                let max_scroll = x11_max_scroll(doc_h, height);

                match key {
                    xproto::X11Key::Escape | xproto::X11Key::Char('q') => break,
                    xproto::X11Key::Up => {
                        scroll_y = scroll_y.saturating_sub(X11_LINE_SCROLL);
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    xproto::X11Key::Down => {
                        scroll_y = (scroll_y + X11_LINE_SCROLL).min(max_scroll);
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    xproto::X11Key::PageUp => {
                        scroll_y = scroll_y.saturating_sub(height);
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    xproto::X11Key::PageDown => {
                        scroll_y = (scroll_y + height).min(max_scroll);
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    xproto::X11Key::F5 => {
                        match render_x11_page(history.current(), width) {
                            Ok((s, f)) => {
                                surface = s;
                                fragments = f;
                                scroll_y = 0;
                            }
                            Err(e) => eprintln!("stele: --x11: reload failed: {e}"),
                        }
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    _ => {}
                }
            }

            xproto::XEvent::ButtonPress { button, x, y } => {
                let (_, doc_h) = stele::surface::Surface::size(&surface);
                let max_scroll = x11_max_scroll(doc_h, height);
                match button {
                    4 => {
                        // Wheel up.
                        scroll_y = scroll_y.saturating_sub(X11_LINE_SCROLL);
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    5 => {
                        // Wheel down.
                        scroll_y = (scroll_y + X11_LINE_SCROLL).min(max_scroll);
                        redraw(&mut conn, &surface, width, height, scroll_y);
                    }
                    1 => {
                        let doc_x = x.max(0) as f32;
                        let doc_y = y.max(0) as f32 + scroll_y as f32;
                        if let Some(href) = xproto::hit_test_pixel(&fragments, doc_x, doc_y) {
                            let new_url = history.current().resolve(&href);
                            history.navigate(new_url.clone());
                            match render_x11_page(&new_url, width) {
                                Ok((s, f)) => {
                                    surface = s;
                                    fragments = f;
                                    scroll_y = 0;
                                }
                                Err(e) => eprintln!("stele: --x11: navigation to {new_url:?} failed: {e}"),
                            }
                            redraw(&mut conn, &surface, width, height, scroll_y);
                        }
                    }
                    _ => {}
                }
            }

            xproto::XEvent::Other => {}
        }
    }
}

// ---------------------------------------------------------------------------
// packet/shell-keyboard: the interactive shell. `stele::browser` (P7) owns
// every actual DECISION (scroll/focus/key-parsing/history/frame rendering) —
// pure and unit-tested there. What lives here is deliberately thin and NOT
// unit-tested (the packet brief's own split: "this is NOT
// end-to-end CI-testable" — no terminal in CI): raw-mode termios, terminal
// size, the blocking read/draw loop, and gluing `stele::browser::Page`
// together from the SAME fetch->parse->cascade->box-tree->layout pipeline
// `dump_text`/`dump_png` already drive (see `build_page_from_dom`'s doc
// comment for why it doesn't just reuse `dump_text` itself).
// ---------------------------------------------------------------------------

/// Build a [`browser::Page`] from an already-fetched+parsed `dom_tree`,
/// mirroring `dump_text`'s own cascade->box-tree->layout steps exactly
/// (same author-sheet collection, same viewport-width-from-cols convention)
/// but keeping `dom_tree`/`styles`/`fragments` alive long enough to hand all
/// three to `browser::Page::build` — `dump_text` can't be reused directly
/// here since it throws those away and returns only the final rendered
/// string. `final_url` is the document's OWN url (post-redirect, when
/// fetched) — both the base `<link>`/`<img>`/`<a href>` resolution uses AND
/// the `Page`'s own `url` field (what `Enter`-on-link resolves against,
/// what the status line prints).
fn build_page_from_dom(dom_tree: dom::Dom, final_url: &Url, cols: usize) -> browser::Page {
    let viewport_width = cols as f32 * 8.0;
    // Not `--color-scheme`-aware (packet t1b-color-scheme scoped that flag
    // to `--dump-text` only, same as the other pixel/interactive paths —
    // see `dump_png_opts`'s identical note).
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, final_url, viewport_width, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let fragments = match build_box_tree(&dom_tree, &styles, &HashMap::new()) {
        Some(root) => layout::layout(&root, Size { w: viewport_width, h: HEADLESS_VIEWPORT_HEIGHT }),
        None => Vec::new(),
    };
    browser::Page::build(dom_tree, &styles, &fragments, cols, final_url.clone())
}

/// A minimal, always-buildable [`browser::Page`] reporting a load failure —
/// goes through the SAME real pipeline as any other page (not a hand-built
/// `Fragment` list) so it behaves identically in the shell (scrollable,
/// has a status line, etc.). Used whenever a fetch/submit fails: the shell
/// must never crash or hang on a bad URL/network error, just show something
/// and stay driveable (Backspace still works to get back out).
fn error_page(url: &Url, message: &str, cols: usize) -> browser::Page {
    let html = format!("<p>Could not load {}</p><p>{}</p>", url.as_str(), message);
    let dom_tree = dom::parser::parse(&html);
    build_page_from_dom(dom_tree, url, cols)
}

/// Fetch + parse `url` into a [`browser::Page`] at `cols` columns; a fetch
/// error degrades to [`error_page`] rather than propagating — matching
/// `dump_text`/`dump_png`'s own "never a panic, never an unhandled `Err`
/// bubbling up to `main`" totality contract, just with a visible on-screen
/// error instead of a blank string.
fn load_page(url: &Url, cols: usize) -> browser::Page {
    match fetch_response(url) {
        Ok(response) => {
            let html = String::from_utf8_lossy(&response.body);
            let dom_tree = dom::parser::parse(&html);
            build_page_from_dom(dom_tree, &response.final_url, cols)
        }
        Err(e) => error_page(url, &e, cols),
    }
}

/// Dispatch a form-submission [`Request`] (`browser::Command::Submit`) over
/// whichever of the two live schemes it names — same scheme dispatch as
/// [`fetch_response`], just over a caller-built `Request` (method/body
/// already set by `form::serialize_submit`) instead of a fresh `GET`.
fn fetch_request(req: &Request) -> Result<Response, String> {
    match req.url.scheme().as_str() {
        "file" => FileFetcher::new().fetch(req).map_err(|e| format!("{e:?}")),
        "http" => Http1Client::new().fetch(req).map_err(|e| format!("{e:?}")),
        other => Err(format!("unsupported scheme: {other}")),
    }
}

/// Query the real terminal size via `TIOCGWINSZ` (`rustix::termios::
/// tcgetwinsize`), falling back to 80x24 (brief's own named fallback) when
/// stdout isn't a tty, the ioctl fails, or reports a degenerate `0x0` (some
/// pty setups do this before the first resize event).
fn terminal_size() -> (usize, usize) {
    match rustix::termios::tcgetwinsize(std::io::stdout()) {
        Ok(ws) if ws.ws_col > 0 && ws.ws_row > 0 => (ws.ws_col as usize, ws.ws_row as usize),
        _ => (80, 24),
    }
}

// ---------------------------------------------------------------------------
// packet/shell-mouse (c2): mouse input, the thin/manual half. `stele::browser`
// owns every actual DECISION (SGR-sequence bytes, the gpm wire layout,
// apply_mouse's click/wheel semantics) -- pure and unit-tested there (see
// that module's "Mouse input"/"gpm client protocol" section doc comments).
// What lives here is the genuinely un-CI-testable glue this packet's brief
// calls out: connecting to the real `/dev/gpmctl` socket, writing/erasing
// the xterm SGR mouse-reporting escape sequences, and poll()ing stdin +
// (when connected) the gpm fd together in the shell's read/draw loop.
// ---------------------------------------------------------------------------

/// The gpm daemon's control socket -- present only on a bare Linux VT with
/// `gpm` actually running (never in CI, and never over ssh/tmux/a plain
/// xterm). Connecting here, or failing to, IS the auto-detect switch this
/// packet's brief asks for: success -> use gpm, and skip xterm mouse
/// reporting entirely ("do NOT also enable xterm mouse"); any failure
/// (socket absent, connection refused, gpm not running) -> fall back to
/// xterm SGR mouse reporting (see [`run_browser`]).
const GPM_SOCKET_PATH: &str = "/dev/gpmctl";

/// `\e[?1000h` enables X10/normal mouse button-event reporting; `\e[?1006h`
/// switches the coordinate ENCODING to SGR (unambiguous past column/row
/// 223 -- the legacy encoding alone packs `Cx+32`/`Cy+32` into a single
/// byte each, which a wide terminal or a tall document easily exceeds).
/// Both are xterm-standard private modes, supported by xterm itself, most
/// VTE-based terminals, and tmux/screen passthrough -- exactly the
/// "terminal emulators (so it also works over SSH)" path the packet brief
/// asks for as the non-gpm fallback.
const XTERM_MOUSE_ENABLE: &str = "\x1b[?1000h\x1b[?1006h";
/// The exact inverse of [`XTERM_MOUSE_ENABLE`], written on exit so a plain
/// keyboard-only terminal session behaves normally after the shell quits --
/// an un-disabled mouse mode otherwise leaks raw click/drag escape
/// sequences into whatever runs next in that terminal (most obviously: the
/// shell prompt underneath).
const XTERM_MOUSE_DISABLE: &str = "\x1b[?1000l\x1b[?1006l";

/// Best-effort virtual-console number for `Gpm_Connect.vc`. gpm's own
/// clients derive this from the controlling tty (`ttyname()` + parsing the
/// trailing digits off `/dev/ttyN`) -- this build has no `libc`/`unsafe`
/// FFI to call `ttyname_r` with (charter: no `unsafe` in this packet's own
/// code), so it reads the SAME information a different, safe way: the
/// `/proc/self/fd/0` symlink, which the kernel itself resolves to stdin's
/// underlying device path. `/dev/ttyN` (a bare VT device) -> `N`; anything
/// else (`/dev/pts/N` over ssh/tmux/a GUI terminal, or an unreadable
/// `/proc`, e.g. a `/proc` not mounted) has no VC number at all -- `0`,
/// which the packet brief names as a plausible "the console this
/// connection is on" sentinel some gpm builds accept, is returned instead
/// of failing the whole connect outright.
///
/// DOCUMENTED best-effort, per the packet brief's own call for this: if a
/// real system's `gpmd` rejects `vc: 0` (some configurations require the
/// exact real VC number), this is the one function to adjust -- e.g. by
/// reading `/sys/class/tty/tty0/active` (`"ttyN\n"`) as a second fallback,
/// which wasn't added here to keep the sysfs-parsing surface (and its own
/// failure modes) no bigger than this packet strictly needs.
fn derive_vc() -> i32 {
    match std::fs::read_link("/proc/self/fd/0") {
        Ok(path) => path.to_str().and_then(|s| s.strip_prefix("/dev/tty")).and_then(|digits| digits.parse::<i32>().ok()).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Connect to the gpm daemon and complete its handshake: open
/// [`GPM_SOCKET_PATH`] as a `UnixStream`, then write a `Gpm_Connect` record
/// (`browser::GpmConnect::to_bytes` -- see that struct's doc comment for
/// the exact wire layout) with `event_mask: GPM_EVENT_MASK_ALL` (every
/// event type), `default_mask: 0` / `min_mod: 0` (nothing passed through to
/// a default handler, no modifier floor), `max_mod: 0xFFFF` (accept any
/// modifier combination), our own `pid`, and [`derive_vc`]'s best-effort VC.
///
/// `None` for anything short of a fully successful connect+handshake: no
/// `/dev/gpmctl` (not on a VT, or gpm isn't running), a connection error,
/// or a failed write -- every one of these is exactly the auto-detect
/// signal [`run_browser`] uses to fall back to xterm mouse reporting
/// instead, never a panic or a hard failure of the whole shell.
fn connect_gpm() -> Option<UnixStream> {
    use std::io::Write;
    let mut stream = UnixStream::connect(GPM_SOCKET_PATH).ok()?;
    let connect =
        browser::GpmConnect { event_mask: browser::GPM_EVENT_MASK_ALL, default_mask: 0, min_mod: 0, max_mod: 0xFFFF, pid: std::process::id() as i32, vc: derive_vc() };
    stream.write_all(&connect.to_bytes()).ok()?;
    Some(stream)
}

/// The interactive shell's raw-mode read/draw loop -- see this module's own
/// section doc comment for the pure/thin split. NOT unit-tested (brief:
/// "this is NOT end-to-end CI-testable — no terminal in CI"); manually
/// driven by a human (see the packet report for exact steps).
///
/// Raw mode clears THREE local-mode flags, not just the two the brief names
/// (`ICANON`, `ECHO`): also `ISIG`. Without `ISIG` off, Ctrl-C would be
/// consumed by the tty driver as `SIGINT` (killing the process outside our
/// own control flow, skipping the termios restore below) instead of
/// arriving as a literal `0x03` byte on stdin -- which is exactly what
/// `browser::Key::CtrlC` (the packet's own "Ctrl-C -> Quit" key) needs to
/// see. This is a minimal, deliberate raw mode, NOT `Termios::make_raw`'s
/// full `cfmakeraw` (which also disables `OPOST`): output post-processing
/// stays ON, so a bare `\n` in `render_frame`'s output still becomes `\r\n`
/// on the wire -- turning it off would stair-step every drawn line one
/// column further right each row.
///
/// Totality caveat (brief, and worth repeating here): `panic = "abort"`
/// means an actual panic anywhere in this loop skips the termios restore
/// below entirely (no unwind, no `Drop`, no cleanup) and leaves the user's
/// terminal in raw mode. Every function this loop calls is written to be
/// total (see each one's own doc comment), so a NORMAL quit (`q`/Ctrl-C)
/// always restores cleanly -- but there is, by construction of
/// `panic = "abort"`, no safety net for a genuine bug that panics anyway.
fn run_browser(source: &str) {
    use std::io::{Read, Write};
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    use rustix::termios::{LocalModes, OptionalActions};

    // packet/shell-forms (responsive-resize follow-up): `poll` blocks for AT
    // MOST this long, so a terminal resize with no keypress still gets
    // picked up promptly (see the `'outer` loop's own resize-handling
    // comment) instead of only on the user's next keystroke. 250ms is
    // frequent enough to feel responsive, infrequent enough that 4
    // `tcgetwinsize` ioctls/sec while idle is noise, not a busy-spin.
    const POLL_TIMEOUT: Timespec = Timespec { tv_sec: 0, tv_nsec: 250_000_000 };

    let orig_termios = match rustix::termios::tcgetattr(std::io::stdin()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("stele: stdin is not a terminal ({e}); the interactive shell needs a real tty");
            return;
        }
    };
    let mut raw = orig_termios.clone();
    raw.local_modes.remove(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG);
    if let Err(e) = rustix::termios::tcsetattr(std::io::stdin(), OptionalActions::Now, &raw) {
        eprintln!("stele: failed to enter raw mode ({e})");
        return;
    }

    // packet/shell-forms: `cols`/`rows`/`content_rows` are now `mut` --
    // re-queried on every trip around the `'outer` loop below (see that
    // loop's own resize-handling comment) so the shell re-flows when the
    // terminal is resized, instead of staying pinned to whatever size it
    // happened to be at launch.
    let (mut cols, mut rows) = terminal_size();
    let mut content_rows = rows.saturating_sub(1).max(1);

    let mut history = browser::History::new(resolve_url(source));
    let mut page = load_page(history.current(), cols);
    let mut view = browser::ViewState::initial(&page, cols, content_rows);
    let mut parser = browser::KeyParser::new();

    // packet/shell-mouse: auto-detect, gpm first. `connect_gpm` is a total
    // best-effort attempt (see its own doc comment) -- when it succeeds we
    // ONLY use gpm (never also turn on xterm mouse reporting, per the
    // packet brief); when it fails (no /dev/gpmctl, not on a VT, gpm not
    // running) we fall back to xterm SGR mouse reporting instead. Neither
    // present is still a fully working shell -- exactly c1's keyboard-only
    // behavior, untouched.
    let mut gpm = connect_gpm();
    let use_xterm_mouse = gpm.is_none();
    if use_xterm_mouse {
        print!("{XTERM_MOUSE_ENABLE}");
    }

    print!("\x1b[?25l"); // hide cursor
    let _ = std::io::stdout().flush();

    let mut buf = [0u8; 256];
    let mut gpm_buf: Vec<u8> = Vec::new();
    let mut gpm_read_buf = [0u8; 256];

    // packet/shell-forms (responsive-resize follow-up): redraw ONLY when
    // something actually changed -- a resize, or real input having been
    // handled -- never on a bare timeout tick. Without this, a 250ms
    // `poll` timeout (needed so a resize gets noticed without a keypress --
    // see below) would turn into a full clear+redraw 4x/sec even while the
    // terminal sits idle, which reads as visible flicker. `true` here so
    // the very first frame still draws before the first `poll`.
    let mut dirty = true;

    'outer: loop {
        // Honor a terminal resize. Re-queried every iteration (cheap: one
        // `ioctl`) rather than once at startup; rebuilding is skipped
        // entirely unless the size actually changed (`!=` below), so an
        // unchanged terminal costs nothing extra here beyond the ioctl
        // itself. There's still no `SIGWINCH` handler (the packet brief's
        // own constraint -- no signal handler, no `unsafe`), so this can't
        // be woken by the resize itself; instead `poll` below is given a
        // short timeout (`POLL_TIMEOUT`) specifically so this check re-runs
        // on its own within a fraction of a second, not only when the next
        // keystroke/mouse event happens to arrive. Manually verified (this
        // whole loop is the un-CI-testable thin half of the packet split --
        // see the module's own section doc comment); the pure clamp this
        // triggers, `browser::clamp_scroll`, IS unit-tested in
        // `browser.rs`.
        let (new_cols, new_rows) = terminal_size();
        if (new_cols, new_rows) != (cols, rows) {
            cols = new_cols;
            rows = new_rows;
            content_rows = rows.saturating_sub(1).max(1);
            page = load_page(history.current(), cols);
            let mut next_view = browser::ViewState { cols, rows: content_rows, ..view };
            if next_view.focus.is_some_and(|idx| idx >= page.focusables.len()) {
                next_view.focus = None;
            }
            view = browser::clamp_scroll(next_view, &page);
            dirty = true;
        }

        if dirty {
            let frame = browser::render_frame(&page, &view);
            print!("\x1b[H\x1b[2J{frame}");
            let _ = std::io::stdout().flush();
            dirty = false;
        }

        // Watch stdin AND (when connected) the gpm socket at once -- a
        // single blocking stdin read (c1's own loop) can't see gpm events
        // arriving on a SEPARATE fd, hence `poll` (this packet's own reason
        // for the new rustix "event" feature). A bounded timeout (rather
        // than c1's original `None`/block-forever) is what makes the
        // resize check above actually responsive -- see its own comment;
        // `Ok(0)` (nothing ready before the deadline) falls through to the
        // `stdin_ready`/`gpm_ready` checks below exactly like a real but
        // empty result, both `false`, so this loop just goes around again
        // (re-checking the terminal size) without reading or drawing
        // anything -- an idle terminal costs one `ioctl` + one `poll` per
        // tick, never a redraw.
        let stdin = std::io::stdin();
        let (stdin_ready, gpm_ready) = {
            let mut poll_fds = vec![PollFd::new(&stdin, PollFlags::IN)];
            if let Some(g) = gpm.as_ref() {
                poll_fds.push(PollFd::new(g, PollFlags::IN));
            }
            if poll(&mut poll_fds, Some(&POLL_TIMEOUT)).is_err() {
                break; // a poll error -- exit cleanly, restore below still runs
            }
            let stdin_ready = poll_fds[0].revents().contains(PollFlags::IN);
            // HUP/ERR too: a dead gpm socket must be noticed even though it
            // will never again report POLLIN (see the read arm below, which
            // drops `gpm` on any read failure).
            let gpm_ready = poll_fds.get(1).is_some_and(|p| p.revents().intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR));
            (stdin_ready, gpm_ready)
        };

        if !stdin_ready && !gpm_ready {
            continue; // a bare timeout tick -- nothing ready, nothing to redraw
        }
        dirty = true; // real input is about to be handled -- redraw once it is

        let mut events: Vec<browser::InputEvent> = Vec::new();

        if stdin_ready {
            let n = match stdin.lock().read(&mut buf) {
                Ok(0) | Err(_) => break, // EOF or a read error: exit cleanly, restore below still runs
                Ok(n) => n,
            };
            events.extend(parser.feed(&buf[..n]));
        }

        if gpm_ready {
            if let Some(g) = gpm.as_mut() {
                match g.read(&mut gpm_read_buf) {
                    Ok(0) | Err(_) => {
                        // gpm went away mid-session (daemon restarted/
                        // killed) -- fall back to keyboard-only for the
                        // rest of it rather than spin-poll a dead fd
                        // forever. Switching to xterm mouse reporting this
                        // late isn't attempted: we can't tell from here
                        // whether the terminal underneath even supports
                        // it, and gpm dying under a live shell is rare
                        // enough not to be worth that extra complexity.
                        gpm = None;
                    }
                    Ok(n) => {
                        gpm_buf.extend_from_slice(&gpm_read_buf[..n]);
                        while gpm_buf.len() >= browser::GPM_EVENT_SIZE {
                            if let Some(gpm_event) = browser::parse_gpm_event(&gpm_buf[..browser::GPM_EVENT_SIZE]) {
                                if let Some(mouse) = browser::gpm_event_to_mouse(&gpm_event) {
                                    events.push(browser::InputEvent::Mouse(mouse));
                                }
                            }
                            gpm_buf.drain(0..browser::GPM_EVENT_SIZE);
                        }
                    }
                }
            }
        }

        for event in events {
            let (next_view, cmd) = match event {
                browser::InputEvent::Key(key) => browser::apply_key(key, view, &page),
                browser::InputEvent::Mouse(mouse) => browser::apply_mouse(mouse, view, &page),
            };
            view = next_view;
            match cmd {
                browser::Command::None => {}
                browser::Command::Navigate(url) => {
                    history.navigate(url.clone());
                    page = load_page(&url, cols);
                    view = browser::ViewState::initial(&page, cols, content_rows);
                }
                browser::Command::Submit(req) => {
                    let target = req.url.clone();
                    page = match fetch_request(&req) {
                        Ok(response) => {
                            let html = String::from_utf8_lossy(&response.body);
                            let dom_tree = dom::parser::parse(&html);
                            history.navigate(response.final_url.clone());
                            build_page_from_dom(dom_tree, &response.final_url, cols)
                        }
                        Err(e) => error_page(&target, &e, cols),
                    };
                    view = browser::ViewState::initial(&page, cols, content_rows);
                }
                browser::Command::Back => {
                    if history.back() {
                        page = load_page(history.current(), cols);
                        view = browser::ViewState::initial(&page, cols, content_rows);
                    }
                }
                browser::Command::Reload => {
                    page = load_page(history.current(), cols);
                    view = browser::ViewState::initial(&page, cols, content_rows);
                }
                browser::Command::Quit => break 'outer,
            }
        }
    }

    if use_xterm_mouse {
        print!("{XTERM_MOUSE_DISABLE}");
    }
    let _ = rustix::termios::tcsetattr(std::io::stdin(), OptionalActions::Now, &orig_termios);
    print!("\x1b[?25h\x1b[0m\n");
    let _ = std::io::stdout().flush();
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        println!("{}", stele::HELLO_LINE);
        return;
    }

    let args = parse_args(&argv);
    if let Some(source) = args.x11 {
        run_x11(&source);
        return;
    }
    if args.headless {
        if let Some(source) = args.dump_text {
            // --stats goes to STDERR, printed before the golden-compared
            // stdout line -- see `print_stats`'s doc comment for why this is
            // an independent fetch+parse pass rather than threading a flag
            // through `dump_text` itself.
            if args.stats {
                print_stats(&source, args.cols as f32 * 8.0);
            }
            println!("{}", dump_text_opts(&source, args.cols, args.color_scheme, args.color_scheme_given));
            return;
        }
        if let Some((source, out_path)) = args.dump_png {
            if args.stats {
                print_stats(&source, DEFAULT_PNG_WIDTH as f32);
            }
            if let Err(e) = write_dump_png_opts(&source, &out_path, args.no_bg_images) {
                eprintln!("stele: --dump-png failed: {e}");
            }
            return;
        }
        if let Some(source) = args.render_fb {
            if let Err(e) = render_fb_opts(&source, args.no_bg_images) {
                eprintln!("stele: no framebuffer (/dev/fb0): {e}");
                std::process::exit(1);
            }
            return;
        }
        eprintln!("stele: --headless requires --dump-text <path-or-url>, --dump-png <path-or-url> <out.png>, or --render-fb <path-or-url>");
        return;
    }

    // packet/shell-keyboard: `stele <path-or-url>` with no `--headless` and
    // no dump flag launches the interactive shell. `--headless`'s own
    // branches above already returned, so reaching here with a `source`
    // unambiguously means "plain interactive invocation".
    if let Some(source) = args.source {
        run_browser(&source);
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
    fn crop_surface_rows_pads_below_content_with_white_not_black() {
        // A 2x1 black surface cropped into a 2x3 window: row 0 is the surface,
        // rows 1-2 are canvas padding, which must be WHITE (not a black fill).
        let surf = MemSurface::new(2, 1, Color::BLACK);
        let out = crop_surface_rows(&surf, 2, 3, 0);
        assert_eq!(out.len(), 2 * 3 * 4);
        assert_eq!(&out[0..8], &[0, 0, 0, 255, 0, 0, 0, 255], "row 0 is the black surface");
        assert!(out[8..].iter().all(|&b| b == 0xff), "rows below content must be white canvas");
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

    // ---- T1b: --color-scheme CLI parsing --------------------------------

    #[test]
    fn parse_args_reads_color_scheme_dark() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html", "--color-scheme", "dark"]));
        assert_eq!(a.color_scheme, style::ColorScheme::Dark);
        assert!(a.color_scheme_given);
    }

    #[test]
    fn parse_args_reads_color_scheme_light() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html", "--color-scheme", "light"]));
        assert_eq!(a.color_scheme, style::ColorScheme::Light);
        assert!(a.color_scheme_given);
    }

    #[test]
    fn parse_args_color_scheme_auto_resolves_to_light() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html", "--color-scheme", "auto"]));
        assert_eq!(a.color_scheme, style::ColorScheme::Light);
        assert!(a.color_scheme_given);
    }

    #[test]
    fn parse_args_color_scheme_defaults_to_light_and_not_given_when_absent() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html"]));
        assert_eq!(a.color_scheme, style::ColorScheme::Light);
        assert!(!a.color_scheme_given, "the default must be distinguishable from an explicit --color-scheme light");
    }

    #[test]
    fn parse_args_color_scheme_bad_value_falls_back_to_light_not_a_panic() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html", "--color-scheme", "nonsense"]));
        assert_eq!(a.color_scheme, style::ColorScheme::Light);
        assert!(a.color_scheme_given, "the flag WAS given on the command line, even though its value was garbage");
    }

    #[test]
    fn parse_args_color_scheme_trailing_flag_with_missing_value_does_not_panic() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html", "--color-scheme"]));
        assert_eq!(a.color_scheme, style::ColorScheme::Light);
        assert!(!a.color_scheme_given);
    }

    // ---- T1b: pre-cascade data-theme/data-mode root stamp -----------------

    #[test]
    fn stamp_color_scheme_sets_data_theme_and_data_mode_on_the_root_element() {
        let mut dom_tree = dom::parser::parse("<html><body><p>t</p></body></html>");
        stamp_color_scheme(&mut dom_tree, style::ColorScheme::Dark);
        let root = dom_tree.root();
        let el = dom_tree.node(root).element().expect("root is an element");
        assert_eq!(el.attrs.get("data-theme"), Some("dark"));
        assert_eq!(el.attrs.get("data-mode"), Some("dark"));
    }

    #[test]
    fn stamp_color_scheme_is_total_on_the_trivial_default_dom() {
        let mut dom_tree = dom::Dom::new(); // just the seeded <html> root, no children
        stamp_color_scheme(&mut dom_tree, style::ColorScheme::Light);
        let el = dom_tree.node(dom_tree.root()).element().unwrap();
        assert_eq!(el.attrs.get("data-theme"), Some("light"));
    }

    #[test]
    fn stamped_color_scheme_attribute_makes_an_attribute_selector_match_end_to_end() {
        let mut dom_tree = dom::parser::parse("<html><body><p>t</p></body></html>");
        stamp_color_scheme(&mut dom_tree, style::ColorScheme::Dark);
        let sheet = stele::style::parser::parse(r#"html[data-theme="dark"] p { color: red; }"#);
        let styles = cascade::cascade(&dom_tree, std::slice::from_ref(&sheet));

        fn find_p(d: &dom::Dom, id: dom::NodeId) -> Option<dom::NodeId> {
            let el = d.node(id).element()?;
            if el.name.as_str() == "p" {
                return Some(id);
            }
            for &c in &el.children {
                if let Some(f) = find_p(d, c) {
                    return Some(f);
                }
            }
            None
        }
        let p = find_p(&dom_tree, dom_tree.root()).expect("p present");
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    // ---- T1b: --color-scheme end-to-end via --dump-text --------------------

    #[test]
    fn color_scheme_default_does_not_stamp_and_matches_the_pre_t1b_golden_path() {
        // Golden-churn safety: the DEFAULT (no --color-scheme given) render
        // path must be byte-identical to calling the pre-T1b `dump_text`
        // wrapper -- see `Args::color_scheme_given`'s doc comment for why
        // stamping is flag-gated rather than unconditional.
        let via_opts = dump_text_opts("fixtures/basic.html", 80, style::ColorScheme::Light, false);
        let golden = include_str!("../goldens/basic.tty.txt");
        assert_eq!(via_opts, golden.trim_end_matches('\n'));
    }

    #[test]
    fn color_scheme_dark_flag_toggles_visible_text_via_stamped_attribute_and_media_query() {
        let light = dump_text_opts("fixtures/color-scheme.html", 80, style::ColorScheme::Light, false);
        assert!(light.contains("LIGHT MODE TEXT"));
        assert!(!light.contains("DARK MODE TEXT"));
        assert!(!light.contains("MEDIA DARK TEXT"));

        let dark = dump_text_opts("fixtures/color-scheme.html", 80, style::ColorScheme::Dark, true);
        assert!(!dark.contains("LIGHT MODE TEXT"), "the [data-theme=dark]-gated light-mode text should be hidden");
        assert!(dark.contains("DARK MODE TEXT"), "attribute-selector-gated content should show under --color-scheme dark");
        assert!(dark.contains("MEDIA DARK TEXT"), "prefers-color-scheme-media-gated content should show under --color-scheme dark");
    }

    #[test]
    fn color_scheme_dark_without_stamping_only_affects_media_queries_not_attribute_selectors() {
        // `stamp = false` proves the two mechanisms (media evaluation vs
        // attribute stamp) are independently wired, not accidentally coupled.
        let dark_no_stamp = dump_text_opts("fixtures/color-scheme.html", 80, style::ColorScheme::Dark, false);
        assert!(dark_no_stamp.contains("LIGHT MODE TEXT"), "without stamping, the [data-theme=dark] rule never applies");
        assert!(!dark_no_stamp.contains("DARK MODE TEXT"));
        assert!(dark_no_stamp.contains("MEDIA DARK TEXT"), "prefers-color-scheme is scheme-driven independent of stamping");
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

    /// m5-link-css: `fixtures/link-css.html` exercises the real
    /// fetch->parse->cascade(WITH `<link>`-fetched author sheets)->box-tree->
    /// layout->tty pipeline end to end — `dump_text` now fetches
    /// `fixtures/link-css.css` (a REAL `file://` fetch, same as the
    /// document's own fetch) via `stele::stylesheets::collect_all_author_sheets`
    /// rather than that `<link>` sitting inert. PROPOSED golden (brief §10
    /// blessing discipline, same as `basic.tty.txt`/`author-css.tty.txt`):
    /// generated by this packet's implementer, never self-blessed — see the
    /// packet report for the countersign/bless request. The fixture
    /// demonstrates: (1) the external `<link>` sheet applying at all
    /// (`p.hidden { display: none }` removes the second paragraph, mirroring
    /// `author-css.html`'s own `display`-based proof, since color has no
    /// tty-visible effect); (2) document order across `<link>` AND `<style>`
    /// — a LATER inline `<style>` block overrides an EARLIER `<link>`'s
    /// `.overridden` rule (both same specificity), proving the collector
    /// interleaves them by source position rather than always ordering one
    /// kind before the other; (3) a non-stylesheet `<link rel="icon">`
    /// pointing at a nonexistent file causes no failure (never fetched at
    /// all, since its `rel` doesn't match `stylesheet`).
    #[test]
    fn dump_text_over_file_fetch_matches_the_link_css_golden() {
        let golden = include_str!("../goldens/link-css.tty.txt");
        let text = dump_text("fixtures/link-css.html", 80);
        assert_eq!(text, golden.trim_end_matches('\n'));
    }

    #[test]
    fn link_css_hidden_paragraph_is_actually_removed_by_the_external_sheet() {
        let text = dump_text("fixtures/link-css.html", 80);
        assert!(!text.contains("the external"), "the linked display:none paragraph should not appear in the dump");
    }

    #[test]
    fn link_css_later_style_block_wins_the_source_order_tie_over_the_earlier_link() {
        let text = dump_text("fixtures/link-css.html", 80);
        assert!(
            text.contains("Visible again"),
            "the paragraph re-enabled by the later <style> block should stay visible, overriding the earlier <link>'s rule"
        );
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

    // -------------------------------------------------------- bg-image (--no-bg-images)

    #[test]
    fn parse_args_recognizes_no_bg_images_flag() {
        let a = parse_args(&args(&["--headless", "--dump-png", "fixtures/basic.html", "/tmp/out.png", "--no-bg-images"]));
        assert!(a.no_bg_images);
    }

    #[test]
    fn parse_args_no_bg_images_defaults_to_false() {
        let a = parse_args(&args(&["--headless", "--dump-text", "fixtures/basic.html"]));
        assert!(!a.no_bg_images);
    }

    /// `dump_png` (the default, no-flag entry point every existing test
    /// above already calls) must render WITH background-images — a fixture
    /// whose only visible pixels come from a `background-image` (not a
    /// `background-color`) should show that image's colors.
    #[test]
    fn dump_png_default_paints_background_images() {
        let bytes = dump_png("fixtures/bg-image.html");
        let pixels = decode_png_pixels(&bytes);
        assert!(
            pixels.chunks(4).any(|p| p == [220, 30, 30, 255]),
            "expected the tiled red background-image to show by default"
        );
    }

    /// The kill switch: `dump_png_opts(.., true)` (what `--no-bg-images`
    /// wires to) must render a DIFFERENT, image-free result for the exact
    /// same fixture — the `.tile` box shows only its `background_color`
    /// (unset here, so transparent -> the page's white canvas), never the
    /// red tile pixels.
    #[test]
    fn no_bg_images_flag_produces_a_distinct_image_free_render() {
        let with_images = dump_png_opts("fixtures/bg-image.html", false);
        let without_images = dump_png_opts("fixtures/bg-image.html", true);

        assert_ne!(with_images, without_images, "--no-bg-images must change the render");

        let without_pixels = decode_png_pixels(&without_images);
        assert!(
            without_pixels.chunks(4).all(|p| p != [220, 30, 30, 255]),
            "--no-bg-images: no red background-image pixels should appear anywhere"
        );
    }

    /// m5-link-css: the pixel path (`dump_png`) fetches `<link>` sheets too
    /// — unlike `--dump-text`'s golden above (which proves it via
    /// `display:none`, since color has no tty-visible effect), this fixture
    /// proves it via a `background-color` that only PIXELS can show, cross-
    /// checking against a sibling document with no `<link>` at all so the
    /// comparison isolates the `<link>` fetch specifically (not just "some
    /// author CSS applied").
    #[test]
    fn dump_png_applies_an_external_link_stylesheet() {
        let dir = std::env::temp_dir().join(format!("stele-dump-png-link-css-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        std::fs::write(dir.join("style.css"), "body { background-color: rgb(10, 200, 30); }").expect("write css");
        std::fs::write(
            dir.join("with-link.html"),
            r#"<!doctype html><link rel="stylesheet" href="style.css"><body>x</body>"#,
        )
        .expect("write html");
        std::fs::write(dir.join("without-link.html"), r#"<!doctype html><body>x</body>"#).expect("write html");

        let with_link = dump_png(&dir.join("with-link.html").to_string_lossy());
        let without_link = dump_png(&dir.join("without-link.html").to_string_lossy());

        let with_link_pixels = decode_png_pixels(&with_link);
        let without_link_pixels = decode_png_pixels(&without_link);
        assert!(
            with_link_pixels.chunks(4).any(|p| p == [10, 200, 30, 255]),
            "the <link>-sourced background-color should appear in the painted PNG"
        );
        assert!(
            !without_link_pixels.chunks(4).any(|p| p == [10, 200, 30, 255]),
            "sanity: the sibling document with no <link> at all must not happen to already have this color"
        );
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

    /// A path this test can be certain doesn't exist, regardless of host/CI
    /// environment (mirrors `backend::fb::tests::guaranteed_absent_path`;
    /// duplicated here rather than shared since it's test-only and this is
    /// a separate crate target from the lib).
    fn guaranteed_absent_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("stele-render-fb-absent-{tag}-{}-{nanos}", std::process::id()))
    }

    /// The core of the totality contract this packet exists to prove: with
    /// NO real framebuffer sysfs geometry or device node available (a
    /// guaranteed-absent scratch path, not the real `/sys/class/graphics/
    /// fb0`/`/dev/fb0` -- whether those exist on the host running this test
    /// must never change its pass/fail), `render_fb_to` still degrades to a
    /// clean `Err`, never a panic/abort, even though it drives the full
    /// fetch->parse->cascade->box-tree->layout->paint pipeline first.
    #[test]
    fn render_fb_to_with_guaranteed_absent_sysfs_and_device_is_a_clean_err_not_a_panic() {
        let sysfs_dir = guaranteed_absent_path("sysfs");
        let device_path = guaranteed_absent_path("device");
        let result = render_fb_to("fixtures/basic.html", &sysfs_dir, &device_path);
        assert!(result.is_err());
    }

    /// The positive end-to-end path: a real fixture, real (scratch) sysfs
    /// geometry, and a real (scratch, writable) device file -- the whole
    /// --render-fb pipeline actually succeeds and writes bytes, with no
    /// real hardware anywhere in the loop.
    #[test]
    fn render_fb_to_with_real_tmp_geometry_and_a_writable_tmp_device_succeeds() {
        let sysfs_dir = guaranteed_absent_path("sysfs-ok");
        std::fs::create_dir_all(&sysfs_dir).expect("create tmp sysfs dir");
        std::fs::write(sysfs_dir.join("virtual_size"), "64,64").unwrap();
        std::fs::write(sysfs_dir.join("bits_per_pixel"), "32").unwrap();
        std::fs::write(sysfs_dir.join("stride"), "256").unwrap(); // 64px * 4B, no padding

        let device_path = guaranteed_absent_path("device-ok");
        std::fs::write(&device_path, []).expect("create scratch device file");

        let result = render_fb_to("fixtures/basic.html", &sysfs_dir, &device_path);
        assert!(result.is_ok(), "{result:?}");

        let on_disk = std::fs::read(&device_path).expect("scratch device file should exist");
        assert_eq!(on_disk.len(), 64 * 256, "expected exactly height*stride bytes written");

        let _ = std::fs::remove_dir_all(&sysfs_dir);
        let _ = std::fs::remove_file(&device_path);
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

    // ------------------------------------------------------------------ --stats

    #[test]
    fn parse_args_reads_stats_flag() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html", "--stats"]));
        assert!(a.stats);
    }

    #[test]
    fn parse_args_stats_defaults_to_false() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html"]));
        assert!(!a.stats);
    }

    #[test]
    fn aggregate_stats_is_all_zero_for_no_sheets() {
        let counts = aggregate_stats(&[]);
        assert_eq!(counts, StatsCounts::default());
        assert_eq!(counts.ignored_declarations, 0);
        assert_eq!(counts.ignored_at_rules, 0);
        assert_eq!(counts.media_at_rules, 0);
    }

    #[test]
    fn aggregate_stats_sums_across_multiple_sheets() {
        // Mirrors the packet brief's own worked example: 2 unknown
        // declarations + 1 @import in one sheet, plus an @media block (never
        // itself an "ignored" thing -- it's a supported construct, just
        // counted separately) in a second sheet.
        let sheet_a = style::parser::parse("p { flibbertigibbet: 1; color: red; wobble: 2; } @import url(x.css);");
        let sheet_b = style::parser::parse("@media (min-width: 500px) { p { color: blue; } }");
        let counts = aggregate_stats(&[sheet_a, sheet_b]);
        assert_eq!(counts.ignored_declarations, 2);
        assert_eq!(counts.ignored_at_rules, 1);
        assert_eq!(counts.media_at_rules, 1);
    }

    #[test]
    fn format_stats_line_matches_the_documented_shape() {
        let counts = StatsCounts { ignored_declarations: 3, ignored_at_rules: 1, media_at_rules: 2 };
        assert_eq!(format_stats_line(counts), "stele --stats: 3 ignored declarations, 1 ignored at-rule, 2 media blocks");
    }

    #[test]
    fn format_stats_line_pluralizes_singular_and_plural_correctly() {
        assert_eq!(
            format_stats_line(StatsCounts::default()),
            "stele --stats: 0 ignored declarations, 0 ignored at-rules, 0 media blocks"
        );
        assert_eq!(
            format_stats_line(StatsCounts { ignored_declarations: 1, ignored_at_rules: 1, media_at_rules: 1 }),
            "stele --stats: 1 ignored declaration, 1 ignored at-rule, 1 media block"
        );
    }

    #[test]
    fn stats_pipeline_end_to_end_aggregates_a_real_documents_author_css() {
        // The same real pipeline print_stats drives (fetch -> parse ->
        // collect_author_sheets_for_viewport), minus the fetch hop -- proves
        // the wiring, not just the pure aggregation/formatting helpers above.
        let html = "<style>p { flibbertigibbet: 1; color: red; wobble: 2; } @import url(x.css);</style><p>hi</p>";
        let dom_tree = dom::parser::parse(html);
        let sheets = style::collect_author_sheets_for_viewport(&dom_tree, 640.0, style::ColorScheme::Light);
        let counts = aggregate_stats(&sheets);
        assert_eq!(counts.ignored_declarations, 2);
        assert_eq!(counts.ignored_at_rules, 1);
        assert_eq!(counts.media_at_rules, 0);
        assert_eq!(format_stats_line(counts), "stele --stats: 2 ignored declarations, 1 ignored at-rule, 0 media blocks");
    }

    #[test]
    fn stats_pipeline_is_all_zero_for_a_document_with_no_author_css() {
        let dom_tree = dom::parser::parse("<p>hello</p>");
        let sheets = style::collect_author_sheets_for_viewport(&dom_tree, 640.0, style::ColorScheme::Light);
        assert_eq!(aggregate_stats(&sheets), StatsCounts::default());
    }

    #[test]
    fn stats_pipeline_is_all_zero_on_a_fetch_failure_not_a_panic() {
        // print_stats itself (rather than the pure aggregate/format halves
        // above) must degrade cleanly on a fetch error, same totality
        // contract as dump_text/dump_png -- exercised via stderr capture is
        // impractical in-process, so this just proves it doesn't panic; the
        // "all zeros" claim is separately proven by the compiled-binary CLI
        // test in tests/stats_cli.rs.
        print_stats("fixtures/does-not-exist-nope.html", 640.0);
    }
}
