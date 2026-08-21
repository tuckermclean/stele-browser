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

use stele::backend::address_edit::AddressEdit;
use stele::backend::chrome;
use stele::backend::fb;
use stele::backend::raster;
use stele::backend::tty;
use stele::browser;
use stele::dom;
use stele::fetch::{Request, Response, Url};
use stele::frames;
use stele::layout::box_tree::build_box_tree_with_pseudo;
use stele::layout::{self, Size};
use stele::style::{self, cascade};
use stele::surface::{Color, MemSurface, Rect, Surface};
use stele::text::translit;

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
/// the 8px-per-column `text::TerminusFont` (16px bucket) cell width
/// `--dump-text` already keys its own layout off of.
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
    /// `dump_text_opts`/`dump_png_opts` — packet t1d-httpforever wired the
    /// latter in; `render_fb_surface_opts`/`--render-fb` remains
    /// `--color-scheme`-unaware, out of scope for that packet too).
    /// Stamping is flag-gated rather than unconditional so the DEFAULT
    /// (no flag at all) render path stays byte-for-byte identical to every
    /// already-blessed golden — see `stamp_color_scheme`'s own doc comment
    /// for the full golden-churn rationale.
    color_scheme_given: bool,
    /// `--audit-contrast <path-or-url>` (packet T1c): a defense-in-depth
    /// GATE, not a render — lays `source` out through the same pipeline
    /// `--dump-png` uses and checks EVERY text run's already-`backend::
    /// raster::paint`-repaired foreground color against `style::contrast::
    /// CONTRAST_MIN`, printing one line per violation and exiting nonzero
    /// if any are found (`main`'s own dispatch, below). See [`audit_
    /// contrast`]'s own doc comment for why a correct implementation
    /// always reports zero.
    audit_contrast: Option<String>,
    /// `--chrome` (packet/browser-chrome T2): when set alongside
    /// `--dump-png <src> <out>`, renders the document INSIDE the browser
    /// chrome (top bar with back button/address field/throbber + bottom
    /// status bar, `backend::chrome::draw`) rather than as a bare document
    /// PNG — see [`write_dump_png_chrome_opts`]. Ignored everywhere else
    /// (`--dump-text`/`--render-fb`/`--audit-contrast`/no `--dump-png`),
    /// same "unused flag combo is a silent no-op, never an error" totality
    /// every other flag here already has. Default `false` so plain
    /// `--dump-png` (every existing golden) is completely unaffected —
    /// golden-safe per the design doc's non-negotiables.
    chrome: bool,
    /// `--viewport-height <N>` (packet/fixed-viewport, CSS px): opts a
    /// `--dump-png` render into the FIXED-viewport layout path
    /// (`layout::layout_viewport`) at `(width, N)` instead of the default
    /// content-height `layout::layout` — so `html { overflow: hidden }`
    /// clips the document into an `N`-tall window rather than sprawling to
    /// content height. `None` (the flag absent, or given with a missing/
    /// unparseable value) leaves every render path byte-identical to before
    /// this packet — same "trailing flag, no/bad value is a no-op" totality
    /// `--cols`/`--color-scheme` already have.
    viewport_height: Option<u32>,
    /// `--scroll-to <id>` (Acid2 scroll-to-fragment packet): opts a
    /// `--dump-png` render into a SCROLLED headless composite -- the final
    /// paint shifts every non-fixed fragment up so `id`'s own padding-top
    /// edge (`layout::find_fragment_top`) lands at the window's y=0, instead
    /// of the default unscrolled (`scroll_y = 0.0`) render. Effect is gated
    /// on `viewport_height` ALSO being `Some` (scrolling only means
    /// something inside a fixed window) -- `--scroll-to` alone is a
    /// documented no-op, same "unused flag combo, silent no-op" posture
    /// `chrome` already has outside `--dump-png`. `None` (flag absent, OR an
    /// id that doesn't resolve to any fragment) degrades to `scroll_y =
    /// 0.0` -- never a panic, never a hard CLI error, same totality posture
    /// every other flag here already has.
    scroll_to_id: Option<String>,
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
            audit_contrast: None,
            chrome: false,
            viewport_height: None,
            scroll_to_id: None,
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
                //
                // packet/browser-chrome T2: `--chrome` is allowed to sit in
                // ANY slot between `--dump-png` and its `<src> <out.png>`
                // pair — `--dump-png --chrome <src> <out.png>` (the form
                // accept.sh's `chrome-basic` golden uses), and equally
                // `--dump-png <src> --chrome <out.png>`. packet/fixed-
                // viewport final review: `--viewport-height <N>` gets the
                // same "any slot" treatment — `--dump-png --viewport-height
                // 120 <src> <out.png>` was silently mis-parsing `120` and
                // `<src>` as the positionals (the flag never took effect,
                // a *silent* blank-PNG failure) because this arm eagerly
                // grabs the NEXT two non-flag tokens as positionals, so any
                // recognized inline flag sitting anywhere in that stretch
                // has to be skipped explicitly here or it silently BECOMES
                // `<src>`/`<out.png>` (shifting the real positional(s)
                // over) instead of setting the flag. Both `--chrome` and
                // `--viewport-height <N>` are handled in any position here;
                // either flag AFTER the full `<src> <out.png>` pair (or
                // before `--dump-png` entirely) still works too — this loop
                // stops as soon as it has collected two positionals, so a
                // trailing flag is left untouched for the ordinary
                // standalone `"--chrome"`/`"--viewport-height"` match arms
                // below to catch (no double-consume).
                let mut j = i + 1;
                let mut chrome_seen = false;
                let mut viewport_height: Option<u32> = None;
                // Acid2 scroll-to-fragment packet: `--scroll-to <id>` gets
                // the exact same "any slot between `--dump-png` and its
                // `<src> <out.png>` pair" treatment as `--chrome`/
                // `--viewport-height` right above -- otherwise `--dump-png
                // --scroll-to top --viewport-height 600 <src> <out.png>`
                // would silently swallow `top` as `<src>` (the exact trap
                // this loop's own comment already documents for the other
                // two flags).
                let mut scroll_to: Option<String> = None;
                let mut positionals: Vec<String> = Vec::new();
                while positionals.len() < 2 && j < argv.len() {
                    if argv[j] == "--chrome" {
                        chrome_seen = true;
                        j += 1;
                    } else if argv[j] == "--viewport-height" {
                        // Value flag: skip the flag token, then try the
                        // next token as the value. Missing/non-numeric
                        // value: skip only the flag itself, leaving
                        // `viewport_height` at `None` — total, never a
                        // panic, matching the standalone arm's own
                        // "trailing flag, no/bad value is a no-op" rule.
                        j += 1;
                        if let Some(v) = argv.get(j).and_then(|s| s.parse::<u32>().ok()) {
                            viewport_height = Some(v);
                            j += 1;
                        }
                    } else if argv[j] == "--scroll-to" {
                        // Same "value flag, missing value is a no-op" rule
                        // as `--viewport-height` right above.
                        j += 1;
                        if let Some(v) = argv.get(j) {
                            scroll_to = Some(v.clone());
                            j += 1;
                        }
                    } else {
                        positionals.push(argv[j].clone());
                        j += 1;
                    }
                }
                if positionals.len() == 2 {
                    if chrome_seen {
                        out.chrome = true;
                    }
                    if let Some(v) = viewport_height {
                        out.viewport_height = Some(v);
                    }
                    if let Some(v) = scroll_to {
                        out.scroll_to_id = Some(v);
                    }
                    i = j - 1;
                    out.dump_png = Some((positionals[0].clone(), positionals[1].clone()));
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
            "--chrome" => out.chrome = true,
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
            "--audit-contrast" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.audit_contrast = Some(v.clone());
                }
                // Same "trailing flag, no value" totality as --dump-text.
            }
            "--viewport-height" => {
                i += 1;
                if let Some(v) = argv.get(i).and_then(|s| s.parse::<u32>().ok()) {
                    out.viewport_height = Some(v);
                }
                // A missing or unparseable value leaves `viewport_height` at
                // `None` (same "trailing flag, no/bad value is a no-op"
                // totality `--cols` already has) — never a panic, never a
                // hard CLI error.
            }
            "--scroll-to" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.scroll_to_id = Some(v.clone());
                }
                // A trailing `--scroll-to` with no value leaves
                // `scroll_to_id` at `None` (same "trailing flag, no value is
                // a no-op" totality `--dump-text`/`--audit-contrast` already
                // have) — never a panic, never a hard CLI error.
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
/// `file://`/`about:` pass through unchanged; anything else (no recognized
/// scheme, e.g. `fixtures/basic.html` or `/abs/path.html`) is treated as a local
/// filesystem path and turned into an absolute `file://` URL — relative
/// paths are resolved against the current working directory first, since
/// `fetch::file::file_path` expects `file:///abs/path` shaped input (a bare
/// `file://relative/path` would misparse the first path segment as a host).
fn resolve_url(raw: &str) -> Url {
    let scheme = Url::new(raw).scheme();
    // `about` passes through unresolved, same as `http`/`file` --
    // packet/attestation-modal: without this, every CLI entry point
    // (`--dump-text`, `--dump-png`, `--render-fb`, `--x11`) falls through to
    // the filesystem-path branch below and mangles `about:attestations`
    // into a bogus `file://<cwd>/about:attestations`, making the scheme
    // handler (`fetch::about`) unreachable from the CLI (design doc's
    // "Current state" finding, packet/attestation-modal).
    if scheme == "http" || scheme == "file" || scheme == "about" {
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
/// POST-redirect URL, not the request URL. `https` is served via the
/// openssl-delegated transport; any genuinely unknown scheme is a clean
/// `Err`, never a panic.
fn fetch_response(url: &Url) -> Result<Response, String> {
    stele::fetch::fetch(&Request::get(url.clone())).map_err(stele::fetch::err_to_string)
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

/// Render a [`StatsCounts`] snapshot plus `missing_glyphs` (packet
/// t2-glyph-fallback) as the one-line, deterministic summary `--stats`
/// prints, e.g. `"stele --stats: 3 ignored declarations, 1 ignored at-rule,
/// 2 media blocks, 4 missing glyphs"`. The first three fields are the exact
/// format named in the original M5 packet brief; `missing_glyphs` extends it
/// (feeds a future Provenance pane) — the count of characters that fell
/// through `text::translit::resolve`'s atlas + transliteration resolution to
/// its skip-and-count default, across the SAME document `--dump-text`/
/// `--dump-png` would actually render (see [`count_missing_glyphs`]'s own
/// doc comment). `media_at_rules` is worded as "media blocks" (matching the
/// brief's own example) rather than "media at-rules", since "block" is what
/// a reader unfamiliar with CSS at-rule terminology will recognize from
/// `@media { }`.
fn format_stats_line(counts: StatsCounts, missing_glyphs: u32) -> String {
    format!(
        "stele --stats: {} ignored declaration{}, {} ignored at-rule{}, {} media block{}, {} missing glyph{}",
        counts.ignored_declarations,
        plural(counts.ignored_declarations),
        counts.ignored_at_rules,
        plural(counts.ignored_at_rules),
        counts.media_at_rules,
        plural(counts.media_at_rules),
        missing_glyphs,
        plural(missing_glyphs),
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
    let missing_glyphs = count_missing_glyphs(&build_fragments_for_stats(source, viewport_width_px));
    eprintln!("{}", format_stats_line(aggregate_stats(&sheets), missing_glyphs));
}

/// Build the SAME `layout::Fragment` list `dump_text_opts`/`dump_png_opts`
/// would lay out for `source` at `viewport_width_px` — `--stats`'s own
/// independent read for [`count_missing_glyphs`] (packet t2-glyph-fallback),
/// same "separate pass, never threaded through the real render path" posture
/// [`print_stats`]'s own doc comment already establishes for the CSS-refusal
/// half of `--stats`. NOT the backend's own render step (`tty::render`/
/// `raster::paint` are never called here) — only `layout::layout`'s
/// fragments are needed, since [`count_missing_glyphs`] reads `Text`
/// fragment strings directly.
///
/// Total, mirroring `dump_text_opts`/`dump_png_opts`'s own totality
/// contract: a fetch failure, a frameset document (out of scope for this
/// counter, same carve-out as `blank_png`'s own frameset note), or an
/// empty/`display:none`-everything document all yield an empty `Vec` rather
/// than panicking — `count_missing_glyphs` is already total over an empty
/// slice (zero missing glyphs), so no separate error path is needed here.
fn build_fragments_for_stats(source: &str, viewport_width_px: f32) -> Vec<layout::Fragment> {
    let url = resolve_url(source);
    let Ok(response) = fetch_response(&url) else { return Vec::new() };
    let html = String::from_utf8_lossy(&response.body);
    let dom_tree = dom::parser::parse(&html);
    if frames::find_frameset(&dom_tree).is_some() {
        return Vec::new();
    }
    let author_sheets =
        stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, viewport_width_px, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let pseudo = cascade::cascade_pseudo(&dom_tree, &author_sheets, &styles);
    let Some(root) = build_box_tree_with_pseudo(&dom_tree, &styles, &HashMap::new(), &pseudo) else { return Vec::new() };
    let viewport = Size { w: viewport_width_px, h: HEADLESS_VIEWPORT_HEIGHT };
    layout::layout(&root, viewport)
}

/// Count characters that fall through `text::translit::resolve`'s atlas +
/// transliteration resolution (packet t2-glyph-fallback) to its
/// skip-and-count default, across every `Text` fragment in `fragments` —
/// the SAME per-char resolution `backend::tty::write_marker`/`backend::
/// raster::paint_text` apply when actually rendering, run here purely for
/// counting (the grid/surface `resolve`'s output would paint into is never
/// materialized). Resets the shared thread-local counter first (`text::
/// translit::reset_missing_glyph_count`) so an unrelated earlier call on
/// this thread can never leak into this total — total over an empty slice
/// (zero) and over any fragment slice, since `translit::resolve` itself is
/// total over any `&str`.
fn count_missing_glyphs(fragments: &[layout::Fragment]) -> u32 {
    translit::reset_missing_glyph_count();
    for f in fragments {
        if let layout::FragmentKind::Text { text: fragment_text, .. } = &f.kind {
            let _ = translit::resolve(fragment_text);
        }
    }
    translit::missing_glyph_count()
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
    let pseudo = cascade::cascade_pseudo(&dom_tree, &author_sheets, &styles);
    // A tty dump never paints pixels, so skip the image fetch+decode
    // pre-pass entirely (an empty map — every <img> stays its `[alt]`-style
    // placeholder) rather than paying needless network/decode cost.
    let Some(root) = build_box_tree_with_pseudo(&dom_tree, &styles, &HashMap::new(), &pseudo) else {
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
    dump_png_opts(source, false, style::ColorScheme::Light, false, None, None)
}

/// [`dump_png`]'s real implementation, parameterized over `no_bg_images`
/// (packet bg-image's `--no-bg-images` kill switch) and, as of packet
/// t1d-httpforever, `scheme`/`stamp` (packet t1b-color-scheme's
/// `--color-scheme`, wired through here the SAME way `dump_text_opts`
/// already wires them — see that function's own doc comment for the
/// stamp-vs-scheme split). `dump_png` itself is a thin wrapper always
/// passing `(false, ColorScheme::Light, false)`, keeping every existing
/// `dump_png` call site/test unchanged; `main`'s `--dump-png` branch calls
/// this directly with `args.no_bg_images`/`args.color_scheme`/
/// `args.color_scheme_given`.
///
/// This closes a gap the original t1b-color-scheme packet left open: that
/// packet scoped `--color-scheme` to `--dump-text` only (see the removed
/// doc-comment note this replaces), which meant a page like httpforever.com
/// — whose dark theme is reachable ONLY via `--color-scheme dark` (no
/// `prefers-color-scheme` fallback) — could never be PNG-screenshotted in
/// its dark theme at all. t1d-httpforever's own dark-theme fidelity fixture
/// needs exactly that, so this packet finishes the wiring.
/// `scroll_to` (Acid2 scroll-to-fragment packet): when both this AND
/// `viewport_height` are `Some`, the final paint is SCROLLED so `scroll_to`'s
/// own padding-top edge (`layout::find_fragment_top`) lands at the window's
/// y=0, instead of the default unscrolled `raster::paint` (`y_offset ==
/// 0.0`). `build_dump_png_render`'s own fetch->parse->cascade->box-tree-
/// >layout pipeline is IDENTICAL either way — `layout::layout_viewport`
/// doesn't need to know about scrolling at all; scrolling is purely a
/// PAINT-time transform over the already-correct, viewport-anchored
/// fragments Task 3 produces. Gated on `viewport_height` also being `Some`
/// (scrolling only means something inside a fixed window) — `scroll_to`
/// alone (`viewport_height: None`) is a documented no-op, same posture
/// `chrome` already has outside `--dump-png`. A `scroll_to` id that doesn't
/// resolve to any fragment (`find_fragment_top` returns `None`) degrades to
/// `scroll_y = 0.0` — the same unscrolled render as no `--scroll-to` at all,
/// never a panic.
fn dump_png_opts(
    source: &str,
    no_bg_images: bool,
    scheme: style::ColorScheme,
    stamp: bool,
    viewport_height: Option<u32>,
    scroll_to: Option<&str>,
) -> Vec<u8> {
    match build_dump_png_render(source, no_bg_images, scheme, stamp, viewport_height) {
        None => blank_png(),
        Some(r) => {
            let mut surface = MemSurface::new(DEFAULT_PNG_WIDTH, r.height, Color::WHITE);
            let scroll_y = if viewport_height.is_some() {
                scroll_to.and_then(|id| layout::find_fragment_top(&r.fragments, id)).unwrap_or(0.0).max(0.0)
            } else {
                0.0
            };
            // `paint_at(.., -0.0)` is byte-identical to `paint(..)` (`-0.0 ==
            // 0.0` for `f32`, and `paint` is already defined as `paint_at(..,
            // 0.0)`) -- always calling `paint_at` here, rather than branching
            // on `scroll_y != 0.0`, is simpler and changes no existing render.
            raster::paint_at(&mut surface, &r.fragments, &r.bg_images, Color::WHITE, -scroll_y);
            raster::encode_png(&surface)
        }
    }
}

/// The fetch->parse->cascade->box-tree->layout half of [`dump_png_opts`],
/// factored out so [`write_dump_png_chrome_opts`] (packet/browser-chrome T2)
/// can reuse the EXACT same document render — fragments, decoded background
/// images, the post-redirect `final_url`, and the content-driven height —
/// rather than duplicating this whole pipeline. `None` on any of the same
/// "degrade to blank" cases [`dump_png_opts`] always had (fetch error, empty/
/// `display:none` document, a `<frameset>` document); `dump_png_opts` itself
/// is now a thin "build, then paint+encode at the bare document width"
/// wrapper around this, so its own output is unchanged byte-for-byte —
/// same computation, just relocated.
struct DocRender {
    fragments: Vec<layout::Fragment>,
    bg_images: HashMap<String, std::rc::Rc<stele::img::RgbaImage>>,
    /// Content-driven height at `DEFAULT_PNG_WIDTH`, already clamped to
    /// `MAX_PNG_HEIGHT` — see [`dump_png_opts`]'s original doc comment for
    /// the derivation.
    height: u32,
    final_url: Url,
}

fn build_dump_png_render(
    source: &str,
    no_bg_images: bool,
    scheme: style::ColorScheme,
    stamp: bool,
    viewport_height: Option<u32>,
) -> Option<DocRender> {
    let url = resolve_url(source);
    let response = match fetch_response(&url) {
        Ok(r) => r,
        Err(_) => return None,
    };
    let html = String::from_utf8_lossy(&response.body);
    let mut dom_tree = dom::parser::parse(&html);

    if frames::find_frameset(&dom_tree).is_some() {
        return None;
    }

    // Packet t1d-httpforever: same pre-cascade `data-theme`/`data-mode` stamp
    // `dump_text_opts` applies, gated the same way (only when `--color-scheme`
    // was actually given) — see `stamp_color_scheme`'s own doc comment for
    // the full golden-churn rationale (the default, no-flag path must stay
    // byte-for-byte identical to every already-blessed PNG golden).
    if stamp {
        stamp_color_scheme(&mut dom_tree, scheme);
    }

    // M5 + m5-link-css: feed cascade every author sheet in document order —
    // inline <style> blocks AND fetched <link rel=stylesheet href> sheets,
    // resolved/fetched against `response.final_url` (same post-redirect
    // rationale the <img src> fetch below already documents). Inline
    // `style=` needs no extra wiring: cascade reads it straight off each
    // Element it already walks. M5 media: `--dump-png`'s viewport width is
    // the fixed `DEFAULT_PNG_WIDTH` (below) — flatten any `@media` (in-CSS
    // or a `<link media=...>` attribute) against THAT, evaluated against
    // `scheme` (packet t1d-httpforever: was hardcoded `Light`, now threaded
    // through from the CLI the same way `dump_text_opts` already does).
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, DEFAULT_PNG_WIDTH as f32, scheme);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let pseudo = cascade::cascade_pseudo(&dom_tree, &author_sheets, &styles);
    // Pixels matter on this path: fetch+decode every <img src> up front
    // (bounded by images::MAX_IMAGES/MAX_TOTAL_IMAGE_BYTES) so
    // build_box_tree can thread real pixel data into each Replaced box.
    // Resolved against `response.final_url` (review finding, Important),
    // NOT the pre-redirect `url`: a document-relative <img src> must
    // resolve against wherever the document actually ended up after any
    // HTTP redirect, not where it was originally requested from.
    let images = stele::images::collect_images(&dom_tree, &response.final_url);
    let Some(root) = build_box_tree_with_pseudo(&dom_tree, &styles, &images, &pseudo) else {
        return None;
    };

    let width = DEFAULT_PNG_WIDTH;
    // packet/fixed-viewport: `viewport_height` opts into the FIXED-viewport
    // layout path (`layout::layout_viewport`, docs/superpowers/specs/
    // 2026-08-20-fixed-viewport-design.md) at `(width, N)` instead of the
    // default content-height `layout::layout` at `(width,
    // HEADLESS_VIEWPORT_HEIGHT)`. `None` (the overwhelmingly common case —
    // every existing `--dump-png` golden) takes the exact call this packet
    // found here, byte-identical.
    let fragments = match viewport_height {
        Some(vh) => layout::layout_viewport(&root, Size { w: width as f32, h: vh as f32 }),
        None => layout::layout(&root, Size { w: width as f32, h: HEADLESS_VIEWPORT_HEIGHT }),
    };

    // Canvas height: content-driven by default (the tallest fragment bottom
    // edge, mirroring `backend::tty::render`'s own `rows_needed` derivation —
    // max over ALL fragments, not just Text, so a bare background Box taller
    // than its text still sizes the canvas — clamped finite/non-negative/
    // bounded the same defensive way, since a fragment rect's `size.h`/
    // `origin.y` are ultimately document/layout-controlled). When
    // `viewport_height` opted into the fixed-viewport path, the canvas is
    // the window itself (`N`, clamped into the same bound) — the whole point
    // of `layout_viewport`'s root height clamp + `overflow:hidden` clip is a
    // FIXED-size window, not a content-height canvas that happens to clip
    // its content in the first `N` rows and leave the rest blank.
    let height = match viewport_height {
        Some(vh) => vh.clamp(1, MAX_PNG_HEIGHT),
        None => {
            let mut content_bottom = 0.0f32;
            for f in &fragments {
                let y = f.rect.origin.y;
                let h = f.rect.size.h;
                if y.is_finite() && h.is_finite() {
                    content_bottom = content_bottom.max(y + h);
                }
            }
            if content_bottom.is_finite() && content_bottom > 0.0 {
                (content_bottom.ceil() as u32).clamp(1, MAX_PNG_HEIGHT)
            } else {
                1
            }
        }
    };

    // Packet bg-image: the `background-image` fetch+decode pre-pass, same
    // "resolve against `response.final_url`" rationale as the `<img src>`
    // pre-pass right above. `--no-bg-images` skips it entirely (an empty
    // map — `raster::paint` then paints every box's `background_color`
    // alone, exactly as if no box declared a `background-image` at all).
    let bg_images = if no_bg_images { HashMap::new() } else { stele::bg_images::collect_bg_images(&styles, &response.final_url) };

    Some(DocRender { fragments, bg_images, height, final_url: response.final_url })
}

/// `--dump-png <src> <out.png>`'s CLI-facing wrapper: render `source` and
/// write the PNG bytes to `out_path`. The render half ([`dump_png`]) is
/// total (never fails); the only failure mode here is the filesystem write,
/// reported as a clean `Err` rather than a panic (e.g. an unwritable
/// directory, a hostile/invalid `out_path`).
#[cfg(test)]
fn write_dump_png(source: &str, out_path: &str) -> Result<(), String> {
    write_dump_png_opts(source, out_path, false, style::ColorScheme::Light, false, None, None)
}

/// [`write_dump_png`]'s real implementation, parameterized over
/// `no_bg_images`/`scheme`/`stamp` — see [`dump_png_opts`]'s doc comment for
/// the same wrapper-over-parameterized-impl rationale and the
/// packet-t1d-httpforever `scheme`/`stamp` wiring specifically — and, as of
/// packet/fixed-viewport, `viewport_height` (see [`build_dump_png_render`]'s
/// own doc comment) and, as of the Acid2 scroll-to-fragment packet,
/// `scroll_to` (see [`dump_png_opts`]'s own doc comment).
fn write_dump_png_opts(
    source: &str,
    out_path: &str,
    no_bg_images: bool,
    scheme: style::ColorScheme,
    stamp: bool,
    viewport_height: Option<u32>,
    scroll_to: Option<&str>,
) -> Result<(), String> {
    let bytes = dump_png_opts(source, no_bg_images, scheme, stamp, viewport_height, scroll_to);
    std::fs::write(out_path, bytes).map_err(|e| format!("{e}"))
}

/// Fixed window height (px) `--dump-png --chrome` renders at, alongside the
/// same `DEFAULT_PNG_WIDTH` the plain `--dump-png` path already lays the
/// document out at. Per `backend::chrome::layout`'s own geometry, `viewport`
/// spans the FULL window width (only the top/status bars are horizontal
/// bands), so a window width equal to `DEFAULT_PNG_WIDTH` keeps the
/// document's own layout width identical with or without `--chrome` —
/// `--chrome` only ever changes what's ABOVE/BELOW the document, never its
/// own column width. 600px comfortably fits `fixtures/basic.html` (a few
/// hundred px tall) inside the viewport band without the golden needing a
/// taller window than a typical screenshot.
const CHROME_WINDOW_HEIGHT: u32 = 600;

/// `--dump-png --chrome <src> <out.png>` (packet/browser-chrome T2, spec §2):
/// render `source` INSIDE the browser chrome — a `DEFAULT_PNG_WIDTH` x
/// `CHROME_WINDOW_HEIGHT` window, the document painted into `backend::
/// chrome::layout`'s `viewport` band, the top/status bars drawn around it
/// via `backend::chrome::draw`. Reuses [`build_dump_png_render`] for the
/// document half — the SAME fetch->parse->cascade->layout pipeline (and the
/// SAME post-redirect `final_url`) plain `--dump-png` uses — so the only new
/// work here is compositing that render into the chrome window rather than
/// encoding it standalone.
///
/// Total, mirroring [`dump_png_opts`]'s own contract: a fetch error/empty
/// document/frameset (`build_dump_png_render` returning `None`) still
/// produces a well-formed chrome window — an empty viewport band framed by
/// the bars, with the address field showing the resolved (if unreachable)
/// URL — rather than falling back to a bare [`blank_png`]; `chrome::draw`
/// itself is total over any URL/status string (see its own doc comment), so
/// there is nothing here that can panic on a hostile `source`.
fn dump_png_chrome_opts(
    source: &str,
    no_bg_images: bool,
    scheme: style::ColorScheme,
    stamp: bool,
    viewport_height: Option<u32>,
) -> Vec<u8> {
    let win_w = DEFAULT_PNG_WIDTH;
    let win_h = CHROME_WINDOW_HEIGHT;
    let lay = chrome::layout(win_w, win_h);

    let render = build_dump_png_render(source, no_bg_images, scheme, stamp, viewport_height);
    let final_url = match &render {
        Some(r) => r.final_url.as_str().to_string(),
        None => resolve_url(source).as_str().to_string(),
    };

    let mut window = MemSurface::new(win_w, win_h, Color::WHITE);

    if let Some(r) = render {
        if lay.viewport.w > 0 && lay.viewport.h > 0 {
            // Paint the document into its OWN viewport-width surface first
            // (same width/paint call `dump_png_opts` uses, so this is
            // pixel-identical to what a bare `--dump-png` of the same source
            // would produce), then copy only the rows that fit inside the
            // viewport band into the window surface at `viewport.origin` —
            // this is what actually keeps the document out of the top/status
            // bars for content taller than the viewport (rather than relying
            // on `raster::paint_at`'s per-fragment clip, which resets to
            // `None` for any fragment with no `overflow:hidden` ancestor of
            // its own and would let an unclipped fragment bleed past the
            // band).
            let mut doc_surface = MemSurface::new(DEFAULT_PNG_WIDTH, r.height, Color::WHITE);
            raster::paint(&mut doc_surface, &r.fragments, &r.bg_images, Color::WHITE);

            let (doc_w, doc_h) = doc_surface.size();
            let doc_bytes = doc_surface.bytes();
            let copy_w = lay.viewport.w.min(doc_w);
            let copy_h = lay.viewport.h.min(doc_h);
            for y in 0..copy_h {
                for x in 0..copy_w {
                    let i = ((y as usize) * (doc_w as usize) + (x as usize)) * 4;
                    let color = Color::rgba(doc_bytes[i], doc_bytes[i + 1], doc_bytes[i + 2], doc_bytes[i + 3]);
                    window.put_pixel(lay.viewport.x + x as i32, lay.viewport.y + y as i32, color);
                }
            }
        }
    }

    chrome::draw(
        &mut window,
        &lay,
        &chrome::ChromeState { url: &final_url, edit: None, status: "Done", loading: false, throbber_frame: 0, can_go_back: false },
    );

    raster::encode_png(&window)
}

/// `--dump-png --chrome <src> <out.png>`'s CLI-facing wrapper — same
/// "render is total, only the filesystem write can fail" contract
/// [`write_dump_png_opts`] has, just calling [`dump_png_chrome_opts`]
/// instead of [`dump_png_opts`].
fn write_dump_png_chrome_opts(
    source: &str,
    out_path: &str,
    no_bg_images: bool,
    scheme: style::ColorScheme,
    stamp: bool,
    viewport_height: Option<u32>,
) -> Result<(), String> {
    let bytes = dump_png_chrome_opts(source, no_bg_images, scheme, stamp, viewport_height);
    std::fs::write(out_path, bytes).map_err(|e| format!("{e}"))
}

/// `--audit-contrast <path-or-url>` (packet T1c): lay `source` out through
/// the same fetch->parse->cascade->box-tree->layout pipeline `--dump-png`
/// uses (`DEFAULT_PNG_WIDTH`, `Light` color scheme — matches accept.sh's
/// own audited fixtures), then for EVERY `Text` fragment check whether the
/// REPAIRED foreground color (`style::contrast::repair_fg`, the SAME
/// function `backend::raster::paint` now wires into every real render)
/// clears `style::contrast::CONTRAST_MIN` against its own `backend::
/// raster::effective_background`.
///
/// This is a DEFENSE-IN-DEPTH GATE, not the repair itself: a correct
/// `repair_fg`/`effective_background` pair makes this always return an
/// empty `Vec` (`repair_fg`'s own doc comment proves at least one of
/// black/white always clears `CONTRAST_MIN` against ANY background), so a
/// nonempty result signals a REGRESSION in one of those two functions, not
/// a legitimately-unrepairable page. A run whose `effective_background` is
/// `None` (INDETERMINATE — its nearest containing box's visible background
/// is a real image this engine can't sample, see that function's own doc
/// comment) is SKIPPED, not flagged: `paint` itself never touches such a
/// run's color, so there's nothing repaired here to check either.
///
/// Unlike `dump_png`/`dump_text` (always total, degrading to a blank
/// render on any failure), a fetch failure here is a clean `Err` — there
/// is no pixel-sensible "audit succeeded" fallback for a page that never
/// loaded, the same posture `render_fb_surface_opts` already takes. An
/// empty/`display:none` document or a `<frameset>` (no single `layout::
/// layout` call to drive — same carve-out as `dump_png`/`dump_text`'s own
/// frames scope) both resolve to `Ok(vec![])` (nothing to audit, not a
/// failure).
///
/// NOTE (scope, future packet): this audits fragments at their RAW,
/// pre-quantization computed colors — it does NOT re-check contrast after
/// `backend::fb::convert_to_fb_bytes`'s palette quantization (the
/// `--render-fb` framebuffer path), which could in principle nudge a
/// borderline-compliant color across the threshold on very low-color-depth
/// hardware. Auditing post-quantization is a reasonable follow-up, not
/// this packet's job (brief scope).
///
/// Packet t1d-httpforever: thin wrapper always auditing at
/// `(ColorScheme::Light, false)` — keeps every existing `audit_contrast`
/// call site/test unchanged, same wrapper-over-parameterized-impl split as
/// `dump_png`/`dump_png_opts`. `main`'s `--audit-contrast` branch calls
/// [`audit_contrast_opts`] directly with `args.color_scheme`/
/// `args.color_scheme_given`, so a page whose dark theme is reachable only
/// via `--color-scheme dark` (httpforever.com's own `html[data-theme=dark]`
/// gate, no `prefers-color-scheme` fallback) can actually be audited in its
/// dark theme, not just its default light one.
#[cfg(test)]
fn audit_contrast(source: &str) -> Result<Vec<String>, String> {
    audit_contrast_opts(source, style::ColorScheme::Light, false)
}

/// [`audit_contrast`]'s real implementation, parameterized over `scheme`/
/// `stamp` — see that function's own doc comment and [`dump_png_opts`]'s for
/// the shared rationale.
fn audit_contrast_opts(source: &str, scheme: style::ColorScheme, stamp: bool) -> Result<Vec<String>, String> {
    let url = resolve_url(source);
    let response = fetch_response(&url)?;
    let html = String::from_utf8_lossy(&response.body);
    let mut dom_tree = dom::parser::parse(&html);

    if frames::find_frameset(&dom_tree).is_some() {
        return Ok(Vec::new());
    }

    if stamp {
        stamp_color_scheme(&mut dom_tree, scheme);
    }

    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, DEFAULT_PNG_WIDTH as f32, scheme);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let pseudo = cascade::cascade_pseudo(&dom_tree, &author_sheets, &styles);
    let images = stele::images::collect_images(&dom_tree, &response.final_url);
    let Some(root) = build_box_tree_with_pseudo(&dom_tree, &styles, &images, &pseudo) else {
        return Ok(Vec::new());
    };

    let viewport = Size { w: DEFAULT_PNG_WIDTH as f32, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);

    let mut violations = Vec::new();
    for (i, fragment) in fragments.iter().enumerate() {
        let layout::FragmentKind::Text { text, style: text_style, .. } = &fragment.kind else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        // `None` (packet T1c amendment) means the nearest containing box's
        // visible background is a real IMAGE this analysis can't sample —
        // `paint` itself leaves such a run's color untouched (see
        // `backend::raster::paint`'s own doc comment), so there is nothing
        // repaired here to check either; skip it rather than flag a run
        // this audit has no way to actually assess.
        let Some(effective_bg) = raster::effective_background(&fragments, i, Color::WHITE) else {
            continue;
        };
        let repaired = style::contrast::repair_fg(text_style.color, effective_bg);
        let ratio = style::contrast::contrast_ratio(repaired, effective_bg);
        if ratio < style::contrast::CONTRAST_MIN {
            let snippet: String = text.chars().take(60).collect();
            violations.push(format!(
                "contrast violation: {ratio:.2}:1 (< {min:.1}:1) at ({x:.0},{y:.0}) text={snippet:?}",
                min = style::contrast::CONTRAST_MIN,
                x = fragment.rect.origin.x,
                y = fragment.rect.origin.y,
            ));
        }
    }
    Ok(violations)
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
    // `--color-scheme`-aware — unlike `dump_png_opts` (wired by packet
    // t1d-httpforever), `--render-fb` targets real hardware framebuffers,
    // not the PNG-golden fixture corpus that packet's scope covers; wiring
    // it through here too is a reasonable follow-up, not this packet's job.
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, &response.final_url, width as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let pseudo = cascade::cascade_pseudo(&dom_tree, &author_sheets, &styles);
    let images = stele::images::collect_images(&dom_tree, &response.final_url);
    let Some(root) = build_box_tree_with_pseudo(&dom_tree, &styles, &images, &pseudo) else {
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
    raster::paint(&mut surface, &fragments, &bg_images, Color::WHITE);
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
// paint steps (same content-driven-height derivation), but returns a
// [`RenderState`] rather than a painted `Surface` -- interactive RAM is
// O(viewport), not O(document) (the tall-page doctrine): `run_x11` retains
// only the fragment list, the bg-image map, and the document height, and
// paints a viewport-sized band on demand via `paint_viewport_band`.
// `backend::x11`'s pixel hit-test needs the fragments' `Interactive::Link`
// rects for click-to-follow either way, which no painted surface could carry.
// ---------------------------------------------------------------------------

/// Cached per-navigation state `run_x11` holds alongside `state`
/// ([`RenderState`]): the parsed DOM and the (possibly redirected) URL it
/// was fetched from. A resize (`ConfigureNotify`) reflows from this cache
/// via [`reflow_from_dom`] instead of re-fetching -- see that function's doc
/// comment for the zero-network guarantee this exists to make structural.
struct X11Session {
    dom: dom::ast::Dom,
    final_url: Url,
}

/// The retained, O(1)-in-viewport render state for `--x11`: the fragment list
/// (already produced by layout) plus the bg-image map and document height.
/// Painting is DEFERRED to `paint_viewport_band` -- no whole-document surface
/// is ever allocated (the tall-page doctrine: interactive RAM is O(viewport)).
struct RenderState {
    fragments: Vec<layout::Fragment>,
    bg_images: std::collections::HashMap<String, std::rc::Rc<stele::img::RgbaImage>>,
    doc_height: u32,
}

/// Reflow an ALREADY-PARSED `dom` (fetched/parsed once by [`load_x11_page`]
/// and cached in an [`X11Session`]) into a [`RenderState`] at `width` CSS
/// px -- the width-dependent tail of the old `render_x11_page`
/// (cascade/layout/height-derivation/bg-image collection), now reusable
/// across a resize without re-fetching or re-parsing. No surface is painted
/// here at all: that's deferred to [`paint_viewport_band`], which paints
/// only the currently-visible band on demand (O(viewport) RAM, never
/// O(document)). Takes NO `Url` to fetch (only `final_url`, used to resolve
/// relative stylesheet/image/background URLs against) -- so a
/// `ConfigureNotify` resize can call this directly and the zero-network
/// guarantee is structural, not just behavioral: there is no fetch call
/// anywhere on this path for the compiler to type-check against. Total:
/// layout failure is a clean `Err`, never a panic -- `run_x11` degrades to
/// a blank page rather than propagating a panic into the event loop.
fn reflow_from_dom(dom_tree: &dom::ast::Dom, final_url: &Url, width: u32) -> Result<RenderState, String> {
    if frames::find_frameset(dom_tree).is_some() {
        return Err("frameset documents are not supported by --x11".to_string());
    }

    // Not `--color-scheme`-aware — `--x11` is the interactive shell, which
    // has no `--color-scheme` CLI flag to read at all (that flag is scoped
    // to the `--headless` dump paths); out of scope for this and the
    // t1b-color-scheme packet alike.
    let author_sheets = stele::stylesheets::collect_all_author_sheets(dom_tree, final_url, width as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(dom_tree, &author_sheets);
    let pseudo = cascade::cascade_pseudo(dom_tree, &author_sheets, &styles);
    let images = stele::images::collect_images(dom_tree, final_url);
    let Some(root) = build_box_tree_with_pseudo(dom_tree, &styles, &images, &pseudo) else {
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
    let doc_height = if content_bottom.is_finite() && content_bottom > 0.0 {
        (content_bottom.ceil() as u32).clamp(1, MAX_PNG_HEIGHT)
    } else {
        1
    };
    let bg_images = stele::bg_images::collect_bg_images(&styles, final_url);
    Ok(RenderState { fragments, bg_images, doc_height })
}

/// Paint only the page-y band `[band_page_y, band_page_y + band_h)` into a
/// fresh `band_h`-tall surface (O(viewport) RAM). Paints the FULL fragment
/// sequence at `y_offset = -band_page_y` via `raster::paint_at` (rather than
/// culling to a translated sub-slice and painting that fresh) so `raster`'s
/// stateful cross-fragment inline-gap synthesis (`synthesize_gap_rect`'s
/// `pending_box_boundary`/`last_text_end` tracking) sees every fragment
/// before the band, exactly as a whole-document paint would -- a band is
/// therefore pixel-identical to the corresponding rows of the old whole-doc-
/// surface-then-crop path. `MemSurface` clips every off-band write, so this
/// still costs O(viewport), not O(document), RAM.
fn paint_viewport_band(state: &RenderState, width: u32, band_page_y: u32, band_h: u32) -> MemSurface {
    let band_h = band_h.max(1);
    let mut band = MemSurface::new(width, band_h, Color::WHITE);
    raster::paint_at(&mut band, &state.fragments, &state.bg_images, Color::WHITE, -(band_page_y as f32));
    band
}

/// Fetch + parse `url` into a fresh [`X11Session`], then [`reflow_from_dom`]
/// it at `width` CSS px. This is the ONLY path in `run_x11` that touches
/// the network (initial load, link click, F5 reload) -- a resize
/// (`ConfigureNotify`) never calls this; it calls `reflow_from_dom`
/// directly against the already-cached `X11Session`. Total:
/// fetch/parse/layout failure is a clean `Err`, never a panic -- `run_x11`
/// degrades to a blank page rather than propagating a panic into the event
/// loop.
fn load_x11_page(url: &Url, width: u32) -> Result<(X11Session, RenderState), String> {
    let response = fetch_response(url)?;
    let html = String::from_utf8_lossy(&response.body);
    let dom_tree = dom::parser::parse(&html);

    let session = X11Session { dom: dom_tree, final_url: response.final_url };
    let state = reflow_from_dom(&session.dom, &session.final_url, width)?;
    Ok((session, state))
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

/// The wire-level plan for repainting the window after a scroll from
/// `old_scroll` to `new_scroll` (both already clamped to `[0, max_scroll]`
/// by the caller): either re-send the whole window ([`ScrollBlit::Full`]),
/// or shift the RETAINED rows server-side with a single `CopyArea` and only
/// re-send the thin newly-exposed strip ([`ScrollBlit::Partial`]).
///
/// `Full` is chosen whenever the scroll delta is `>= win_h` — at that point
/// NOTHING from the old frame is still on screen, so a `CopyArea` would just
/// be dead weight in front of a full repaint anyway.
///
/// packet/browser-chrome (T3): no longer wired into [`x11_scroll_to`] — see
/// that function's own doc comment for why (the chrome bars break the
/// "document fills the whole window" assumption this plan relies on).
/// `#[allow(dead_code)]` because the type and its computing function
/// (`scroll_blit`) stay correct and unit-tested pure logic, kept for a
/// possible future viewport-confined version rather than deleted.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollBlit {
    Full,
    Partial {
        /// `CopyArea` source window-row.
        copy_src_y: u32,
        /// `CopyArea` destination window-row.
        copy_dst_y: u32,
        /// `CopyArea` row count (0 => no `CopyArea` needed at all).
        copy_h: u32,
        /// Document row the newly-exposed strip starts at (crop source).
        strip_page_y: u32,
        /// Window row the newly-exposed strip lands at (`PutImage` dst_y_base).
        strip_dst_y: u32,
        /// Strip row count (0 => nothing new to paint, e.g. a no-op scroll).
        strip_h: u32,
    },
}

/// Compute [`ScrollBlit`] for a scroll from `old_scroll` to `new_scroll`
/// within a `win_h`-row window. Total: `new_scroll == old_scroll` (a no-op
/// scroll — e.g. Up already at the top) returns a `Partial` with `copy_h`
/// and `strip_h` both `0`, so a caller that always runs the `Partial` branch
/// sends nothing over the wire rather than needing its own separate
/// early-out. `win_h == 0` degenerates to `Full` (nothing sane to copy),
/// never a panic/div-by-zero (this function does no division at all).
/// See [`ScrollBlit`]'s own doc comment for why this is currently unused
/// outside its own unit tests.
#[allow(dead_code)]
fn scroll_blit(old_scroll: u32, new_scroll: u32, win_h: u32) -> ScrollBlit {
    if new_scroll == old_scroll {
        return ScrollBlit::Partial { copy_src_y: 0, copy_dst_y: 0, copy_h: 0, strip_page_y: new_scroll, strip_dst_y: 0, strip_h: 0 };
    }
    if new_scroll > old_scroll {
        // Scrolling DOWN: the page moves up under the window. Retained rows
        // (window rows [d, win_h) of the OLD frame) become window rows
        // [0, win_h-d) of the new frame -- CopyArea shifts them up by `d`.
        // The strip below that (window rows [win_h-d, win_h)) is newly
        // exposed page rows [new_scroll+(win_h-d), new_scroll+win_h).
        let d = new_scroll - old_scroll;
        if d >= win_h {
            return ScrollBlit::Full;
        }
        let copy_h = win_h - d;
        ScrollBlit::Partial { copy_src_y: d, copy_dst_y: 0, copy_h, strip_page_y: new_scroll + copy_h, strip_dst_y: copy_h, strip_h: d }
    } else {
        // Scrolling UP: the page moves down under the window. Retained rows
        // (window rows [0, win_h-d) of the OLD frame) become window rows
        // [d, win_h) of the new frame -- CopyArea shifts them down by `d`.
        // The strip above that (window rows [0, d)) is newly exposed page
        // rows [new_scroll, new_scroll+d).
        let d = old_scroll - new_scroll;
        if d >= win_h {
            return ScrollBlit::Full;
        }
        let copy_h = win_h - d;
        ScrollBlit::Partial { copy_src_y: 0, copy_dst_y: d, copy_h, strip_page_y: new_scroll, strip_dst_y: 0, strip_h: d }
    }
}

/// Default X11 window size (CSS px) — no `--width`/`--height` flag yet (the
/// packet brief doesn't ask for one); `ConfigureNotify` (a user resizing
/// the window) reflows to whatever size the window manager/server actually
/// grants, same as any other X11 client.
const DEFAULT_X11_WIDTH: u32 = 1024;
const DEFAULT_X11_HEIGHT: u32 = 768;

/// Pixels scrolled per arrow-key press / mouse-wheel notch.
const X11_LINE_SCROLL: u32 = 60;

/// Compose one full window-sized frame for the interactive `--x11` shell: a
/// `chrome::layout(width, height)` for placement, the document band
/// `[scroll_y, scroll_y + vh)` (`vh` = `lay.viewport.h`, NOT `height`)
/// painted via [`paint_viewport_band`] and copied into the window surface at
/// `lay.viewport`'s origin, then the top/status bars drawn over it via
/// `chrome::draw(&mut window, &lay, chrome_state)`. Same compositing shape
/// `dump_png_chrome_opts` (T2) uses for the static `--dump-png --chrome`
/// path — a viewport-sized doc render copied into a window-sized surface,
/// then the bars painted on top — just driven by a live scroll-offset band
/// instead of a whole-document paint, so it's a separate function rather
/// than a literal call-out to it (deliberately NOT shared with
/// `dump_png_chrome_opts`, which is frozen per this packet's own brief).
///
/// Total: a `0`-sized `lay.viewport` (a degenerate tiny window) just skips
/// the doc copy loop — `chrome::draw` itself is already total over any
/// window size (see its own doc comment).
fn compose_chrome_window(state: &RenderState, width: u32, height: u32, scroll_y: u32, chrome_state: &chrome::ChromeState) -> MemSurface {
    let lay = chrome::layout(width, height);
    let mut window = MemSurface::new(width, height, Color::WHITE);

    if lay.viewport.w > 0 && lay.viewport.h > 0 {
        let vh = lay.viewport.h;
        let band = paint_viewport_band(state, width, scroll_y, vh);
        let (band_w, band_h) = band.size();
        let band_bytes = band.bytes();
        let copy_w = lay.viewport.w.min(band_w);
        let copy_h = lay.viewport.h.min(band_h);
        for y in 0..copy_h {
            for x in 0..copy_w {
                let i = ((y as usize) * (band_w as usize) + (x as usize)) * 4;
                let color = Color::rgba(band_bytes[i], band_bytes[i + 1], band_bytes[i + 2], band_bytes[i + 3]);
                window.put_pixel(lay.viewport.x + x as i32, lay.viewport.y + y as i32, color);
            }
        }
    }

    chrome::draw(&mut window, &lay, chrome_state);
    window
}

/// Full re-crop + re-convert + re-`PutImage` of the whole window at
/// `scroll_y` — used for the initial paint, `Expose`'s first-focus setup,
/// a resize (`ConfigureNotify`), a scroll (see [`x11_scroll_to`]'s own doc
/// comment for why THAT no longer has its own optimized path), and anything
/// that changes the CONTENT (reload, navigate), where nothing already on
/// screen is worth retaining via `CopyArea`. Composes the document band +
/// chrome bars via [`compose_chrome_window`], then paints through the
/// server-side `pixmap` back buffer (`PutImage` into `pixmap`, then one
/// `CopyArea` from `pixmap` to `window` to present) rather than the window
/// directly — the window is NEVER `PutImage`d. Manual/interactive-only, like
/// `run_x11` itself (see its own doc comment) — takes `pixmap`/`window`/
/// `gc`/`depth`/`bpp`/`scanline_pad` as explicit params (rather than closing
/// over them) since it's called from both `run_x11` and [`x11_scroll_to`].
#[allow(clippy::too_many_arguments)]
fn x11_full_redraw(
    conn: &mut stele::backend::x11::XConnection,
    state: &RenderState,
    pixmap: u32,
    window: u32,
    gc: u32,
    depth: u8,
    bpp: u32,
    scanline_pad: u32,
    width: u32,
    height: u32,
    scroll_y: u32,
    chrome_state: &chrome::ChromeState,
) {
    let composed = compose_chrome_window(state, width, height, scroll_y, chrome_state);
    let cropped = crop_surface_rows(&composed, width, height, 0);
    let stride = x11_row_stride(width, bpp, scanline_pad);
    let fb_info = fb::FbInfo { width, height, bpp, stride };
    match fb::convert_to_fb_bytes(&cropped, width, height, fb_info) {
        Ok(bytes) => {
            if let Err(e) = conn.put_image(pixmap, gc, width as u16, height as u16, depth, &bytes, stride as usize) {
                eprintln!("stele: --x11: PutImage failed: {e}");
            }
            if let Err(e) = conn.copy_area(pixmap, window, gc, 0, 0, 0, 0, width as u16, height as u16) {
                eprintln!("stele: --x11: CopyArea (present) failed: {e}");
            }
        }
        Err(e) => eprintln!("stele: --x11: pixel conversion failed: {e}"),
    }
}

/// Repaint the window for a scroll from `old_scroll_y` to `new_scroll_y`.
///
/// packet/browser-chrome (T3): now ALWAYS a full [`x11_full_redraw`], not
/// [`scroll_blit`]'s `CopyArea`-the-retained-rows optimization. That
/// optimization's whole premise was the document filling the ENTIRE window,
/// so a vertical shift of the window's pixels was also a valid shift of the
/// document's pixels; with the chrome bars framing a `viewport` band
/// narrower than the window, a raw `CopyArea` of window rows would drag the
/// top/status bars around with the scroll (or smear document rows into
/// them) — wrong on every axis. Reusing/adapting `scroll_blit` to operate on
/// viewport-relative rows instead is a legitimate nice-to-have (the bars
/// never move, only `[TOP_H, TOP_H+vh)` needs the retained-rows treatment),
/// but per this packet's own brief, correctness beats the optimization for
/// `run_x11` (MANUAL-verify-only, no CI harness) — a full viewport-band
/// redraw is one `paint_viewport_band` call of `vh` (not `height`) rows plus
/// two cheap chrome bars, not the whole-document repaint the name might
/// suggest. `old_scroll_y` is accepted but unused (kept so call sites don't
/// need to change) — see [`scroll_blit`]'s own doc comment, which is now
/// dead code kept for its still-correct, still-tested pure logic in case a
/// future packet wants to wire up the viewport-confined version.
#[allow(clippy::too_many_arguments)]
fn x11_scroll_to(
    conn: &mut stele::backend::x11::XConnection,
    state: &RenderState,
    pixmap: u32,
    window: u32,
    gc: u32,
    depth: u8,
    bpp: u32,
    scanline_pad: u32,
    width: u32,
    height: u32,
    _old_scroll_y: u32,
    new_scroll_y: u32,
    chrome_state: &chrome::ChromeState,
) {
    x11_full_redraw(conn, state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, new_scroll_y, chrome_state);
}

/// Session-accumulated debug counters for `run_x11`, printed to stderr on
/// quit when `STELE_X11_STATS` is set (any value). Loop instrumentation
/// only -- deliberately no dedicated unit test, same as `run_x11` itself.
///
/// `batches`/`events`/`scrolls`/`frames` are counted exactly, at the site
/// where the thing actually happens. `copy_areas`/`put_image_bytes` are
/// honest approximations taken at `run_x11`'s own call sites rather than
/// threaded through [`x11_full_redraw`]/[`x11_scroll_to`] (which would mean
/// changing their signatures just to carry a counter out): `copy_areas`
/// only counts the one explicit `CopyArea` `run_x11` issues itself (the
/// `Expose` present), and `put_image_bytes` approximates each
/// [`x11_full_redraw`] call as one full-window `PutImage` of
/// `width * height * 4` bytes (BGRA8, the fb encoder's most common case --
/// it undercounts for lower bpp formats and doesn't count
/// [`x11_scroll_to`]'s partial-strip `PutImage`s at all).
#[derive(Default)]
struct X11Stats {
    /// Number of `drain_events` calls (one non-blocking poll-until-empty
    /// batch each).
    batches: u64,
    /// Total raw `XEvent`s drained across all batches (pre-coalesce).
    events: u64,
    /// Number of times a scroll intent actually moved `scroll_y` and
    /// triggered an [`x11_scroll_to`] repaint.
    scrolls: u64,
    /// Number of `CopyArea` ops `run_x11` issued directly (the `Expose`
    /// present path only -- see struct doc).
    copy_areas: u64,
    /// Approximate total `PutImage` bytes over the session -- see struct
    /// doc for the approximation.
    put_image_bytes: u64,
    /// Number of paint brackets (`begin_frame`/`end_frame` pairs).
    frames: u64,
}

/// Build the live `ChromeState` for one `run_x11` redraw from the shell's
/// own history/status/loading/throbber state — `history.current()`'s URL
/// (the display-only address bar), the caller-owned `status` line/`loading`
/// flag, `throbber_frame`, and `history.can_go_back()`. A tiny free function
/// rather than a closure so every one of `run_x11`'s several redraw sites
/// can call it without fighting the borrow checker over `state`/`session`/
/// `scroll_y`, which are ALSO mutated around the same call sites (a closure
/// capturing `history`/`status` by reference would otherwise have to
/// coexist with those other mutable borrows in scope).
fn x11_chrome_state<'a>(history: &'a browser::History, status: &'a str, loading: bool, throbber_frame: u8, edit: Option<(&'a str, usize)>) -> chrome::ChromeState<'a> {
    chrome::ChromeState { url: history.current().as_str(), edit, status, loading, throbber_frame, can_go_back: history.can_go_back() }
}

/// `packet/chrome-address-edit`: the `edit` argument every `x11_chrome_state`
/// call site now needs -- `Some((live buffer, cursor char-index))` while
/// `address_edit.focused`, `None` otherwise. A tiny, directly unit-tested
/// pure function so this plumbing has real CI coverage independent of the
/// full `run_x11` event loop itself (which stays manual-verify-only, per
/// this codebase's established "no X server in CI" posture).
fn x11_edit_arg(address_edit: &AddressEdit) -> Option<(&str, usize)> {
    if address_edit.focused {
        Some((address_edit.buffer.as_str(), address_edit.cursor))
    } else {
        None
    }
}

/// Whether window-pixel point `(x, y)` falls inside `rect` — half-open on
/// the right/bottom edges (`x < rect.x + rect.w`), matching every other
/// `Rect` hit-test convention in this codebase. Used to test an `XIntent::
/// Click`'s raw window coordinates against `chrome::layout`'s `back` rect,
/// BEFORE any viewport-offset math — the chrome bars live in window space,
/// not document space.
fn x11_point_in_rect(rect: Rect, x: i16, y: i16) -> bool {
    let (x, y) = (x as i32, y as i32);
    x >= rect.x && x < rect.x + rect.w as i32 && y >= rect.y && y < rect.y + rect.h as i32
}

/// `stele --x11 <url>`: open a real X11 window (kdrive/Xfbdev, core
/// protocol only — see `backend::x11`'s own doc comment for the wire
/// details) and drive it interactively. Each loop iteration DRAINS every
/// event currently available (`XConnection::drain_events`, a non-blocking
/// poll-until-empty batch after one blocking read) rather than acting on
/// one event at a time, classifies each raw `XEvent` to an `XIntent`
/// ([`classify_x11_intent`]), and folds the batch with
/// `xproto::coalesce` (adjacent scrolls sum, adjacent Exposes union to one
/// damage rect, only the LAST resize in a burst survives) before acting —
/// this is what keeps the loop responsive under an input burst (a
/// held-down scroll wheel, a dragged resize) instead of replaying every
/// individual event's full repaint. `Expose` repaints ONLY the damaged
/// rect, and does it with a single `CopyArea` off the server-side `pixmap`
/// back buffer (see below) — zero image bytes back to the server; on the
/// FIRST `Expose` it also claims keyboard focus (see below). Scroll
/// intents (arrow keys/PageUp/PageDown/mouse wheel) repaint via
/// [`x11_scroll_to`] (server-side `CopyArea` of the retained rows +
/// `PutImage` of only the newly-exposed strip, no re-layout); `F5`
/// reloads; a left click pixel-hit-tests the current fragment stream and,
/// on a link hit, navigates; a resize re-lays-out at the new width AND
/// recreates the pixmap at the new size (the old one is freed first); `q`/
/// Escape quits.
///
/// Every paint (initial, `Expose`, scroll, reload, navigate, resize) goes
/// through a server-side pixmap double-buffer, NOT the window directly:
/// `pixmap` is created once up front (window-depth, window-sized) and
/// recreated on every resize; [`x11_full_redraw`]/[`x11_scroll_to`]
/// `PutImage`/`CopyArea` INTO `pixmap`, then present it to `window` with
/// one final `CopyArea`. `Expose` never `PutImage`s at all — it just
/// re-presents whatever is already sitting in the back buffer.
///
/// `SetInputFocus` is deliberately NOT sent right after `MapWindow`: the
/// window isn't viewable yet at that point, and a real server replies
/// `BadMatch` (confirmed against a live server: `X BadMatch(8) on
/// SetInputFocus(major=42)`). Waiting for the first `Expose` guarantees
/// viewability — and, as a side effect, is what actually GIVES this window
/// keyboard focus on a WM-less server (Xfbdev/TinyX): under a real window
/// manager the WM already handled focus, so this is a no-op there.
///
/// Deliberately NOT unit-tested — same split as `run_browser`'s own doc
/// comment: there is no X server in CI to open a window against. Every
/// pure decision this loop makes (protocol encode/parse, keysym mapping,
/// intent classification/coalescing, pixel hit-test, scroll-repaint
/// planning) already IS unit-tested, in `backend::x11` and this module's
/// own [`scroll_blit`]; this function is thin glue over those plus
/// [`load_x11_page`]/[`reflow_from_dom`]/[`crop_surface_rows`]/[`x11_row_stride`]/
/// [`x11_full_redraw`]/[`x11_scroll_to`]/[`classify_x11_intent`]. Bounded/
/// total throughout: every socket read goes through `XConnection`'s own
/// fixed-size-buffer reads, and a page-load/reload/navigate failure prints
/// to stderr and keeps the previous frame on screen rather than panicking.
///
/// Debug counters (`STELE_X11_STATS=1`): [`X11Stats`] is accumulated for the
/// whole session and printed to stderr on quit. It's loop instrumentation
/// only -- no dedicated unit test -- see the struct doc for what each field
/// means and how it's counted.
fn run_x11(source: &str) {
    use stele::backend::x11::{self as xproto, XConnection};

    // Debug-only counters, gated on STELE_X11_STATS so a normal run pays
    // nothing but the env lookup. See X11Stats's doc comment for the
    // per-field counting rules (some fields are exact, some are honest
    // approximations taken at the run_x11 call site rather than threaded
    // through the paint helpers).
    let x11_stats_enabled = std::env::var("STELE_X11_STATS").is_ok();
    let mut stats = X11Stats::default();

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

    // SetInputFocus is sent on the first Expose (window is only guaranteed
    // viewable then), not here -- see run_x11's own doc comment for why a
    // pre-loop call gets BadMatch on a real server.
    let mut focus_set = false;

    // Best-effort: a server that fails GetKeyboardMapping still gets a
    // working (mouse-only) shell rather than a hard exit.
    let (keysyms_per_keycode, keysyms) = conn.get_keyboard_mapping().unwrap_or_else(|e| {
        eprintln!("stele: --x11: GetKeyboardMapping failed ({e}); keyboard input will be inert");
        (0, Vec::new())
    });
    let min_keycode = conn.setup.min_keycode;

    // Chrome state (packet/browser-chrome T3): `history` above already owns
    // the current URL (the address bar's content); these three are the rest
    // of what `chrome::ChromeState` needs each redraw. `throbber_frame`
    // advances by one on EVERY redraw (see `x11_chrome_state`'s call
    // sites below) -- `chrome::draw`'s own throbber only actually animates
    // while `loading` is true, so the exact cadence elsewhere is harmless.
    let mut status = String::from("Done");
    let mut loading = false;
    let mut throbber_frame: u8 = 0;

    // packet/chrome-address-edit: the address bar's own edit-buffer state
    // (focus/buffer/cursor) -- a pure `AddressEdit`, mutated directly by the
    // new `XIntent::Edit` match arm below and read by every
    // `x11_chrome_state`/`classify_x11_intent` call site (via `edit_arg`/
    // `address_edit.focused`) so the chrome always reflects whatever's
    // currently being typed.
    let mut address_edit = AddressEdit::default();

    let mut scroll_y: u32 = 0;
    let (mut session, mut state) = match load_x11_page(history.current(), width) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stele: --x11: initial page load failed: {e}");
            status = format!("Failed to load: {e}");
            (
                X11Session { dom: dom::parser::parse(""), final_url: history.current().clone() },
                RenderState { fragments: Vec::new(), bg_images: std::collections::HashMap::new(), doc_height: 1 },
            )
        }
    };

    // The server-side back buffer: window-depth, window-sized. Every paint
    // lands here first; the window only ever receives a CopyArea from it
    // (Expose) or the trailing "present" CopyArea after a PutImage into the
    // pixmap (full redraw / scroll). See x11_full_redraw/x11_scroll_to.
    let mut pixmap = match conn.create_pixmap(window, depth, width as u16, height as u16) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("stele: --x11: CreatePixmap failed: {e}");
            0
        }
    };

    throbber_frame = throbber_frame.wrapping_add(1);
    conn.begin_frame();
    x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
    let _ = conn.end_frame();
    stats.frames += 1;
    stats.put_image_bytes += width as u64 * height as u64 * 4;

    loop {
        let batch = match conn.drain_events() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("stele: --x11: connection closed: {e}");
                if x11_stats_enabled {
                    print_x11_stats(&stats);
                }
                break;
            }
        };
        stats.batches += 1;
        stats.events += batch.len() as u64;

        let intents: Vec<xproto::XIntent> = batch
            .iter()
            // packet/chrome-address-edit: `address_edit.focused`, read fresh
            // every batch, is what makes `q`/`Escape`/arrows/`F5` route to
            // Edit ops instead of the global shortcuts while typing.
            .filter_map(|ev| classify_x11_intent(ev, min_keycode, keysyms_per_keycode, &keysyms, height, address_edit.focused))
            .collect();

        let mut quit = false;
        for intent in xproto::coalesce(intents) {
            match intent {
                xproto::XIntent::Quit => {
                    quit = true;
                    break;
                }
                xproto::XIntent::ScrollBy(d) => {
                    // Scroll clamps to the VIEWPORT band's height (`vh`),
                    // not the whole window -- the doc only ever paints
                    // inside `chrome::layout`'s `viewport` rect.
                    let vh = chrome::layout(width, height).viewport.h;
                    let max_scroll = x11_max_scroll(state.doc_height, vh);
                    let old = scroll_y;
                    scroll_y = ((old as i64 + d as i64).clamp(0, max_scroll as i64)) as u32;
                    if scroll_y != old {
                        stats.scrolls += 1;
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_scroll_to(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, old, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;
                    }
                }
                xproto::XIntent::Click { x, y } => {
                    let lay = chrome::layout(width, height);

                    // packet/chrome-address-edit: "blur first, then act" --
                    // a click ANYWHERE outside the address field while it's
                    // focused cancels the in-progress edit (no commit, no
                    // navigation) before whatever that click normally does
                    // (back/reload/attest/a document link, or dead chrome
                    // space) runs below. A click INSIDE the address field
                    // while already focused is handled by the dedicated
                    // focus branch further down (a no-op for this MVP -- no
                    // click-to-position-cursor).
                    if address_edit.focused && !x11_point_in_rect(lay.address, x, y) {
                        address_edit.blur();
                        // Redraw right away so a click on DEAD chrome space
                        // (which none of the branches below act on) still
                        // visibly reverts the address field to the real
                        // current URL immediately, not just on the next
                        // unrelated repaint. A branch below that also acts
                        // on this same click (back/reload/attest/a document
                        // link) does its own further redraw(s) on top of
                        // this one -- harmless, matches this function's
                        // existing "redraw after every state change" style.
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;
                    }

                    if x11_point_in_rect(lay.back, x, y) && history.can_go_back() {
                        // Back button: pop history, then reload whatever
                        // that lands on -- same load path/status/throbber
                        // treatment as a link click or F5, just sourced from
                        // `history.back()` instead of `history.navigate()`.
                        if history.back() {
                            status = format!("Loading {}...", history.current().as_str());
                            loading = true;
                            throbber_frame = throbber_frame.wrapping_add(1);
                            conn.begin_frame();
                            x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                            let _ = conn.end_frame();
                            stats.frames += 1;
                            stats.put_image_bytes += width as u64 * height as u64 * 4;

                            match load_x11_page(history.current(), width) {
                                Ok((sess, s)) => {
                                    session = sess;
                                    state = s;
                                    scroll_y = 0;
                                    status = String::from("Done");
                                }
                                Err(e) => {
                                    eprintln!("stele: --x11: back-navigation reload failed: {e}");
                                    status = format!("Failed to load: {e}");
                                }
                            }
                            loading = false;
                            throbber_frame = throbber_frame.wrapping_add(1);
                            conn.begin_frame();
                            x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                            let _ = conn.end_frame();
                            stats.frames += 1;
                            stats.put_image_bytes += width as u64 * height as u64 * 4;
                        }
                    } else if x11_point_in_rect(lay.reload, x, y) {
                        // Reload button (packet/chrome-address-edit): the
                        // SAME load/redraw body `XIntent::Reload`'s own arm
                        // runs (design doc §4 -- "reuse, don't refactor",
                        // duplicated on purpose, matching every other
                        // branch's own duplicated load/redraw sequence in
                        // this match).
                        status = format!("Loading {}...", history.current().as_str());
                        loading = true;
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;

                        match load_x11_page(history.current(), width) {
                            Ok((sess, s)) => {
                                session = sess;
                                state = s;
                                scroll_y = 0;
                                status = String::from("Done");
                            }
                            Err(e) => {
                                eprintln!("stele: --x11: reload failed: {e}");
                                status = format!("Failed to load: {e}");
                            }
                        }
                        loading = false;
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;
                    } else if x11_point_in_rect(lay.address, x, y) && !address_edit.focused {
                        // packet/chrome-address-edit: click-to-focus, seeded
                        // with the CURRENT real URL (never whatever was
                        // mid-edit before -- there's nothing to restore,
                        // `AddressEdit::blur` never remembered anything, see
                        // its own doc comment). Chrome-only repaint (a full
                        // redraw is fine here -- correctness first, per the
                        // design doc's own "not mandated" note on a cheaper
                        // partial blit).
                        address_edit.focus(history.current().as_str());
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;
                    } else if x11_point_in_rect(lay.attest, x, y) {
                        // Attestations button (packet/attestation-modal): a
                        // fixed, well-known target URL -- no document
                        // `href` to resolve, unlike the in-page-link branch
                        // below, but otherwise the same
                        // navigate/load/redraw sequence.
                        let new_url = Url::new("about:attestations");
                        history.navigate(new_url.clone());

                        status = format!("Loading {}...", new_url.as_str());
                        loading = true;
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;

                        match load_x11_page(&new_url, width) {
                            Ok((sess, s)) => {
                                session = sess;
                                state = s;
                                scroll_y = 0;
                                status = String::from("Done");
                            }
                            Err(e) => {
                                eprintln!("stele: --x11: navigation to {new_url:?} failed: {e}");
                                status = format!("Failed to load: {e}");
                            }
                        }
                        loading = false;
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;
                    } else if y >= chrome::TOP_H as i16 && (y as u32) < chrome::TOP_H + lay.viewport.h {
                        // Inside the viewport band: hit-test in DOCUMENT
                        // coordinates, offset by the top bar's height and
                        // the current scroll position -- `x` is unchanged
                        // (the viewport spans the window's full width).
                        let doc_x = x.max(0) as f32;
                        let doc_y = (y as u32 - chrome::TOP_H) as f32 + scroll_y as f32;
                        if let Some(href) = xproto::hit_test_pixel(&state.fragments, doc_x, doc_y) {
                            let new_url = history.current().resolve(&href);
                            history.navigate(new_url.clone());

                            status = format!("Loading {}...", new_url.as_str());
                            loading = true;
                            throbber_frame = throbber_frame.wrapping_add(1);
                            conn.begin_frame();
                            x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                            let _ = conn.end_frame();
                            stats.frames += 1;
                            stats.put_image_bytes += width as u64 * height as u64 * 4;

                            match load_x11_page(&new_url, width) {
                                Ok((sess, s)) => {
                                    session = sess;
                                    state = s;
                                    scroll_y = 0;
                                    status = String::from("Done");
                                }
                                Err(e) => {
                                    eprintln!("stele: --x11: navigation to {new_url:?} failed: {e}");
                                    status = format!("Failed to load: {e}");
                                }
                            }
                            loading = false;
                            // New content -- full repaint, same reasoning as
                            // ConfigureNotify below.
                            throbber_frame = throbber_frame.wrapping_add(1);
                            conn.begin_frame();
                            x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                            let _ = conn.end_frame();
                            stats.frames += 1;
                            stats.put_image_bytes += width as u64 * height as u64 * 4;
                        }
                    }
                    // Else: a click somewhere else in the chrome bars (the
                    // address field WHILE ALREADY FOCUSED -- no click-to-
                    // position-cursor, an explicit MVP scope cut, design
                    // doc §3 -- the throbber, a disabled back button, or a
                    // tiny/degenerate window's dead space) -- display-only,
                    // ignored (the blur-first handling above already ran if
                    // it needed to).
                }
                xproto::XIntent::Reload => {
                    status = format!("Loading {}...", history.current().as_str());
                    loading = true;
                    throbber_frame = throbber_frame.wrapping_add(1);
                    conn.begin_frame();
                    x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                    let _ = conn.end_frame();
                    stats.frames += 1;
                    stats.put_image_bytes += width as u64 * height as u64 * 4;

                    match load_x11_page(history.current(), width) {
                        Ok((sess, s)) => {
                            session = sess;
                            state = s;
                            scroll_y = 0;
                            status = String::from("Done");
                        }
                        Err(e) => {
                            eprintln!("stele: --x11: reload failed: {e}");
                            status = format!("Failed to load: {e}");
                        }
                    }
                    loading = false;
                    throbber_frame = throbber_frame.wrapping_add(1);
                    conn.begin_frame();
                    x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                    let _ = conn.end_frame();
                    stats.frames += 1;
                    stats.put_image_bytes += width as u64 * height as u64 * 4;
                }
                xproto::XIntent::Edit(op) => {
                    // packet/chrome-address-edit: every op mutates
                    // `address_edit` directly. `Commit` (on a non-empty,
                    // non-whitespace buffer) additionally resolves + navigates
                    // + loads, exactly like the viewport-link/attest/reload
                    // branches above -- reusing `resolve_url` (not `Url::new`
                    // directly) so a typed bare `example.com`-shaped string
                    // gets the same passthrough/fallback treatment every other
                    // Stele entry point already gives it (design doc §3;
                    // flagged in the PR description as the pinned resolution
                    // helper). Every other op just needs the chrome bars
                    // repainted to reflect the new buffer/cursor/focus state.
                    let mut already_redrawn = false;
                    match op {
                        xproto::EditIntent::Insert(c) => address_edit.insert_char(c),
                        xproto::EditIntent::Backspace => address_edit.backspace(),
                        xproto::EditIntent::Delete => address_edit.delete_forward(),
                        xproto::EditIntent::Left => address_edit.move_left(),
                        xproto::EditIntent::Right => address_edit.move_right(),
                        xproto::EditIntent::Home => address_edit.move_home(),
                        xproto::EditIntent::End => address_edit.move_end(),
                        xproto::EditIntent::Cancel => address_edit.blur(),
                        xproto::EditIntent::Commit => {
                            if let Some(raw) = address_edit.commit() {
                                let new_url = resolve_url(&raw);
                                history.navigate(new_url.clone());

                                status = format!("Loading {}...", new_url.as_str());
                                loading = true;
                                throbber_frame = throbber_frame.wrapping_add(1);
                                conn.begin_frame();
                                x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                                let _ = conn.end_frame();
                                stats.frames += 1;
                                stats.put_image_bytes += width as u64 * height as u64 * 4;

                                match load_x11_page(&new_url, width) {
                                    Ok((sess, s)) => {
                                        session = sess;
                                        state = s;
                                        scroll_y = 0;
                                        status = String::from("Done");
                                    }
                                    Err(e) => {
                                        eprintln!("stele: --x11: navigation to {new_url:?} failed: {e}");
                                        status = format!("Failed to load: {e}");
                                    }
                                }
                                loading = false;
                                throbber_frame = throbber_frame.wrapping_add(1);
                                conn.begin_frame();
                                x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                                let _ = conn.end_frame();
                                stats.frames += 1;
                                stats.put_image_bytes += width as u64 * height as u64 * 4;
                                already_redrawn = true;
                            }
                            // `commit()` returned `None` (empty/whitespace-
                            // only buffer): a no-op, per `AddressEdit::
                            // commit`'s own contract -- `focused` stays
                            // true, nothing to navigate to. Falls through to
                            // the generic chrome-only redraw below (harmless
                            // even though nothing visibly changed).
                        }
                    }

                    if !already_redrawn {
                        throbber_frame = throbber_frame.wrapping_add(1);
                        conn.begin_frame();
                        x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                        let _ = conn.end_frame();
                        stats.frames += 1;
                        stats.put_image_bytes += width as u64 * height as u64 * 4;
                    }
                }
                xproto::XIntent::Resize { w, h } => {
                    // classify_x11_intent already screens out a 0x0
                    // ConfigureNotify (a hostile/transient geometry) before
                    // it ever becomes an XIntent -- nothing to re-check here.
                    width = w as u32;
                    height = h as u32;
                    // Create the new back buffer BEFORE freeing the old one: if the
                    // create fails, keep the old (wrong-sized but VALID) pixmap so the
                    // window keeps presenting and a later resize retries — freeing
                    // first would leave `pixmap` permanently unusable.
                    match conn.create_pixmap(window, depth, width as u16, height as u16) {
                        Ok(new_pixmap) => {
                            let _ = conn.free_pixmap(pixmap);
                            pixmap = new_pixmap;
                        }
                        Err(e) => eprintln!("stele: --x11: recreate pixmap failed, keeping old buffer: {e}"),
                    }
                    match reflow_from_dom(&session.dom, &session.final_url, width) {
                        Ok(s) => {
                            state = s;
                        }
                        Err(e) => eprintln!("stele: --x11: reflow after resize failed: {e}"),
                    }
                    // Clamp against the NEW viewport height, not the whole
                    // window -- same reasoning as the `ScrollBy` handler.
                    let vh = chrome::layout(width, height).viewport.h;
                    scroll_y = scroll_y.min(x11_max_scroll(state.doc_height, vh));
                    // Content (and the window geometry itself) changed --
                    // nothing on screen is safe to retain via CopyArea.
                    throbber_frame = throbber_frame.wrapping_add(1);
                    conn.begin_frame();
                    x11_full_redraw(&mut conn, &state, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y, &x11_chrome_state(&history, &status, loading, throbber_frame, x11_edit_arg(&address_edit)));
                    let _ = conn.end_frame();
                    stats.frames += 1;
                    stats.put_image_bytes += width as u64 * height as u64 * 4;
                }
                xproto::XIntent::Expose { x, y, w, h } => {
                    if !focus_set {
                        // Window is guaranteed viewable now (Expose only
                        // fires for a viewable window) -- see run_x11's doc
                        // comment for why this can't happen right after
                        // MapWindow.
                        if let Err(e) = conn.set_input_focus(window) {
                            eprintln!("stele: --x11: SetInputFocus failed: {e}");
                        }
                        focus_set = true;
                    }
                    // Present the damaged region straight from the back
                    // buffer -- zero image bytes on the wire.
                    conn.begin_frame();
                    if let Err(e) = conn.copy_area(pixmap, window, gc, x as i16, y as i16, x as i16, y as i16, w, h) {
                        eprintln!("stele: --x11: Expose CopyArea failed: {e}");
                    }
                    let _ = conn.end_frame();
                    stats.frames += 1;
                    stats.copy_areas += 1;
                }
            }
        }
        if quit {
            if x11_stats_enabled {
                print_x11_stats(&stats);
            }
            break;
        }
    }
}

/// Prints [`X11Stats`]'s one-line summary to stderr. Split out of
/// [`run_x11`] purely so both quit paths (a clean `Quit` intent and the
/// connection-closed error path) can share the same print without
/// duplicating the format string.
fn print_x11_stats(stats: &X11Stats) {
    eprintln!(
        "stele x11 stats: {} batches, {} events, {} scrolls, {} frames, {} copy_areas, {} put_image_bytes",
        stats.batches, stats.events, stats.scrolls, stats.frames, stats.copy_areas, stats.put_image_bytes
    );
}

/// Classify one raw [`xproto::XEvent`] into an [`xproto::XIntent`] for
/// [`xproto::coalesce`] to fold, using the keyboard map fetched once in
/// `run_x11` (`min_keycode`/`keysyms_per_keycode`/`keysyms`), the CURRENT
/// window `height` (needed for `PageUp`/`PageDown`'s scroll delta), and
/// `address_focused` (`packet/chrome-address-edit`, `run_x11`'s own
/// `AddressEdit.focused`, read fresh per batch — see `run_x11`'s call
/// site). `None` for events that carry no actionable intent (an unmapped
/// keycode, an unrecognized key, a non-wheel/non-button-1 click,
/// `XEvent::Other`) -- same "silently ignore" behavior the old
/// one-event-at-a-time loop had.
///
/// `address_focused == false`: BYTE-IDENTICAL to this function's behavior
/// before this packet, except one deliberate, flagged side effect —
/// `state`'s Shift bit (`ShiftMask`, bit 0) is now actually read (it used to
/// be parsed off the wire and discarded) to select `keysym_for_keycode`'s
/// column, so **Shift+`q` no longer quits**: column 1 for `q`'s keycode is
/// `Q` (keysym `0x51`), which doesn't match the `Quit` arm's `X11Key::
/// Char('q')` pattern. Holding Shift had zero effect before this packet, so
/// this is a minor correctness fix (Shift now does something), not a
/// preserved-behavior regression — see the design doc §2/Risks.
///
/// `address_focused == true`: the SAME `KeyPress` arm routes to
/// `XIntent::Edit(EditIntent)` instead — typing `q` inserts a `q` rather
/// than quitting (the collision this whole packet exists to fix), `Escape`
/// cancels the edit instead of quitting, `Enter` commits, and the global
/// scroll/reload/quit shortcuts (`Up`/`Down`/`PageUp`/`PageDown`/`F5`/`q`/
/// `Escape`) are all swallowed (`None`) rather than leaking through --
/// typing in the address bar must never scroll the document or reload it
/// out from under the edit. `Tab` maps to `None` in both states (no
/// multi-field focus cycling exists yet, design doc §3).
fn classify_x11_intent(ev: &stele::backend::x11::XEvent, min_keycode: u8, keysyms_per_keycode: u8, keysyms: &[u32], height: u32, address_focused: bool) -> Option<stele::backend::x11::XIntent> {
    use stele::backend::x11::{self as xproto, EditIntent, XEvent, XIntent, X11Key};
    match ev {
        XEvent::ButtonPress { button: 4, .. } => Some(XIntent::ScrollBy(-(X11_LINE_SCROLL as i32))),
        XEvent::ButtonPress { button: 5, .. } => Some(XIntent::ScrollBy(X11_LINE_SCROLL as i32)),
        XEvent::ButtonPress { button: 1, x, y } => Some(XIntent::Click { x: *x, y: *y }),
        XEvent::ButtonPress { .. } => None,
        XEvent::Expose { x, y, w, h, .. } => Some(XIntent::Expose { x: *x, y: *y, w: *w, h: *h }),
        XEvent::ConfigureNotify { width, height: eh } => {
            if *width == 0 || *eh == 0 {
                None
            } else {
                Some(XIntent::Resize { w: *width, h: *eh })
            }
        }
        XEvent::KeyPress { keycode, state } => {
            // ShiftMask (bit 0 of the KeyButMask `state`) selects column 1
            // -- the one real behavior change from reading `state` at all
            // (see this function's own doc comment above).
            let column = if state & 0x0001 != 0 { 1 } else { 0 };
            let sym = xproto::keysym_for_keycode(*keycode, min_keycode, keysyms_per_keycode, keysyms, column)?;
            let key = xproto::keysym_to_key(sym)?;

            if address_focused {
                return match key {
                    X11Key::Char(c) => Some(XIntent::Edit(EditIntent::Insert(c))),
                    X11Key::Backspace => Some(XIntent::Edit(EditIntent::Backspace)),
                    X11Key::Delete => Some(XIntent::Edit(EditIntent::Delete)),
                    X11Key::Left => Some(XIntent::Edit(EditIntent::Left)),
                    X11Key::Right => Some(XIntent::Edit(EditIntent::Right)),
                    X11Key::Home => Some(XIntent::Edit(EditIntent::Home)),
                    X11Key::End => Some(XIntent::Edit(EditIntent::End)),
                    X11Key::Enter => Some(XIntent::Edit(EditIntent::Commit)),
                    X11Key::Escape => Some(XIntent::Edit(EditIntent::Cancel)),
                    // Global shortcuts (scroll/reload) and Tab are all
                    // swallowed while typing -- see doc comment above.
                    X11Key::Up | X11Key::Down | X11Key::PageUp | X11Key::PageDown | X11Key::F5 | X11Key::Tab => None,
                };
            }

            match key {
                X11Key::Escape | X11Key::Char('q') => Some(XIntent::Quit),
                X11Key::Up => Some(XIntent::ScrollBy(-(X11_LINE_SCROLL as i32))),
                X11Key::Down => Some(XIntent::ScrollBy(X11_LINE_SCROLL as i32)),
                X11Key::PageUp => Some(XIntent::ScrollBy(-(height as i32))),
                X11Key::PageDown => Some(XIntent::ScrollBy(height as i32)),
                X11Key::F5 => Some(XIntent::Reload),
                _ => None,
            }
        }
        XEvent::Other => None,
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
    // Not `--color-scheme`-aware — same rationale as `reflow_from_dom`'s
    // identical note just above: the interactive shell has no
    // `--color-scheme` CLI flag to read (scoped to the `--headless` dump
    // paths only).
    let author_sheets = stele::stylesheets::collect_all_author_sheets(&dom_tree, final_url, viewport_width, style::ColorScheme::Light);
    let styles = cascade::cascade(&dom_tree, &author_sheets);
    let pseudo = cascade::cascade_pseudo(&dom_tree, &author_sheets, &styles);
    let fragments = match build_box_tree_with_pseudo(&dom_tree, &styles, &HashMap::new(), &pseudo) {
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
/// whichever of the two live schemes it names — same shared `fetch::fetch`
/// table as [`fetch_response`], just over a caller-built `Request`
/// (method/body already set by `form::serialize_submit`) instead of a fresh
/// `GET`.
fn fetch_request(req: &Request) -> Result<Response, String> {
    stele::fetch::fetch(req).map_err(stele::fetch::err_to_string)
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
            // packet/browser-chrome T2: `--chrome` composes the document
            // into the browser chrome window instead of a bare document
            // PNG; plain `--dump-png` (the default, every existing golden)
            // takes the unchanged path below.
            let result = if args.chrome {
                write_dump_png_chrome_opts(
                    &source,
                    &out_path,
                    args.no_bg_images,
                    args.color_scheme,
                    args.color_scheme_given,
                    args.viewport_height,
                )
            } else {
                write_dump_png_opts(
                    &source,
                    &out_path,
                    args.no_bg_images,
                    args.color_scheme,
                    args.color_scheme_given,
                    args.viewport_height,
                    args.scroll_to_id.as_deref(),
                )
            };
            if let Err(e) = result {
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
        if let Some(source) = args.audit_contrast {
            match audit_contrast_opts(&source, args.color_scheme, args.color_scheme_given) {
                Ok(violations) if violations.is_empty() => {
                    println!("stele: --audit-contrast: 0 violations");
                }
                Ok(violations) => {
                    for line in &violations {
                        println!("{line}");
                    }
                    eprintln!("stele: --audit-contrast: {} violation(s)", violations.len());
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("stele: --audit-contrast failed: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }
        eprintln!("stele: --headless requires --dump-text <path-or-url>, --dump-png <path-or-url> <out.png>, --render-fb <path-or-url>, or --audit-contrast <path-or-url>");
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
    fn reflow_from_dom_renders_without_fetching() {
        // Parses the fixture's HTML directly (no fetch) and asserts
        // `reflow_from_dom` produces fragments -- proving the resize path
        // is fetch-free BY CONSTRUCTION: `reflow_from_dom` takes a parsed
        // `Dom`, not a `Url`, so there is no fetch call on this path at all.
        let html = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/basic.html")).unwrap();
        let dom_tree = dom::parser::parse(&html);
        let url = Url::new("file:///fixtures/basic.html");
        let state = reflow_from_dom(&dom_tree, &url, 800).expect("reflow renders");
        assert!(!state.fragments.is_empty(), "basic.html must produce fragments");
    }

    #[test]
    fn paint_viewport_band_allocates_a_viewport_sized_surface_not_the_document() {
        // A very tall document scrolled far down must still paint into a
        // band-HEIGHT surface (O(viewport)), never a document-height one.
        let html = format!("<html><body>{}</body></html>", "<p>line</p>".repeat(4000));
        let dom = stele::dom::parser::parse(&html);
        let state = reflow_from_dom(&dom, &Url::new("file:///tall.html"), 800).expect("reflow");
        assert!(state.doc_height > 5000, "the fixture must be genuinely tall (was {})", state.doc_height);
        let band = paint_viewport_band(&state, 800, 4000, 768);
        assert_eq!(stele::surface::Surface::size(&band), (800, 768), "band surface must be viewport-sized regardless of doc height");
    }

    #[test]
    fn reflow_from_dom_returns_state_without_a_document_surface() {
        // Structural O(viewport) guarantee: reflow no longer returns a MemSurface
        // at all -- it returns render state; painting is deferred to band paints.
        let html = "<html><body><p>hi</p></body></html>";
        let dom = stele::dom::parser::parse(html);
        let state = reflow_from_dom(&dom, &Url::new("file:///x.html"), 800).expect("reflow");
        assert!(!state.fragments.is_empty());
        assert!(state.doc_height >= 1);
    }

    #[test]
    fn full_scroll_of_a_tall_document_only_ever_paints_viewport_bands() {
        // 68k.news-scale stand-in: a very tall document.
        let html = format!("<html><body>{}</body></html>", "<p>paragraph</p>".repeat(6000));
        let dom = stele::dom::parser::parse(&html);
        let state = reflow_from_dom(&dom, &Url::new("file:///tall.html"), 800).expect("reflow");
        assert!(state.doc_height > 10_000, "fixture must be much taller than a viewport (was {})", state.doc_height);

        let viewport_h = 768u32;
        let max_scroll = state.doc_height.saturating_sub(viewport_h);
        // Walk the whole document in viewport steps; every painted band must be
        // exactly viewport-height -- the peak surface allocation is O(viewport),
        // never the ~doc_height*width*4 the old whole-document MemSurface took.
        let mut y = 0u32;
        while y <= max_scroll {
            let band = paint_viewport_band(&state, 800, y, viewport_h);
            assert_eq!(stele::surface::Surface::size(&band), (800, viewport_h));
            y += viewport_h;
        }
        // And the final clamped band.
        let last = paint_viewport_band(&state, 800, max_scroll, viewport_h);
        assert_eq!(stele::surface::Surface::size(&last), (800, viewport_h));
    }

    // ----------------------------------------------------------- scroll_blit

    #[test]
    fn scroll_blit_scrolling_down_copies_up_and_paints_the_bottom_strip() {
        // win_h=768, old=100 -> new=160 (d=60): retained rows shift up by
        // 60 (CopyArea src_y=60 -> dst_y=0, 708 rows); the newly-exposed
        // bottom 60-row strip is page rows [160+708, 160+768) = [868, 928).
        let blit = scroll_blit(100, 160, 768);
        assert_eq!(
            blit,
            ScrollBlit::Partial { copy_src_y: 60, copy_dst_y: 0, copy_h: 708, strip_page_y: 868, strip_dst_y: 708, strip_h: 60 }
        );
    }

    #[test]
    fn scroll_blit_scrolling_up_copies_down_and_paints_the_top_strip() {
        // win_h=768, old=160 -> new=100 (d=60): retained rows shift down by
        // 60 (CopyArea src_y=0 -> dst_y=60, 708 rows); the newly-exposed top
        // 60-row strip is page rows [100, 160).
        let blit = scroll_blit(160, 100, 768);
        assert_eq!(
            blit,
            ScrollBlit::Partial { copy_src_y: 0, copy_dst_y: 60, copy_h: 708, strip_page_y: 100, strip_dst_y: 0, strip_h: 60 }
        );
    }

    #[test]
    fn scroll_blit_jump_at_or_past_win_h_is_full() {
        // A jump of exactly win_h, and one well past it, both leave nothing
        // from the old frame on screen -- no CopyArea is worth doing.
        assert_eq!(scroll_blit(0, 768, 768), ScrollBlit::Full);
        assert_eq!(scroll_blit(0, 5000, 768), ScrollBlit::Full);
        assert_eq!(scroll_blit(5000, 0, 768), ScrollBlit::Full); // upward jump too
    }

    #[test]
    fn scroll_blit_no_op_scroll_paints_nothing() {
        // Up already at the top (saturating_sub clamps to the same value) --
        // must not CopyArea or PutImage anything.
        let blit = scroll_blit(0, 0, 768);
        assert_eq!(blit, ScrollBlit::Partial { copy_src_y: 0, copy_dst_y: 0, copy_h: 0, strip_page_y: 0, strip_dst_y: 0, strip_h: 0 });
    }

    #[test]
    fn scroll_blit_small_window_and_small_delta_stay_bounded() {
        // A window shorter than a typical line-scroll step still produces a
        // sane (bounded, non-panicking) plan.
        let blit = scroll_blit(0, 10, 20);
        assert_eq!(blit, ScrollBlit::Partial { copy_src_y: 10, copy_dst_y: 0, copy_h: 10, strip_page_y: 20, strip_dst_y: 10, strip_h: 10 });
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

    // ---- packet/fixed-viewport: --viewport-height CLI parsing -----------

    #[test]
    fn parse_args_reads_viewport_height() {
        let a = parse_args(&args(&["--headless", "--dump-png", "x.html", "out.png", "--viewport-height", "600"]));
        assert_eq!(a.viewport_height, Some(600));
    }

    #[test]
    fn parse_args_viewport_height_defaults_to_none_when_absent() {
        let a = parse_args(&args(&["--headless", "--dump-png", "x.html", "out.png"]));
        assert_eq!(a.viewport_height, None);
    }

    #[test]
    fn parse_args_viewport_height_missing_or_non_numeric_value_is_a_no_op_not_a_panic() {
        let a = parse_args(&args(&["--viewport-height"]));
        assert_eq!(a.viewport_height, None);
        let a2 = parse_args(&args(&["--viewport-height", "not-a-number"]));
        assert_eq!(a2.viewport_height, None);
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

    // ---- t1d-httpforever: --color-scheme end-to-end via --dump-png ---------
    //
    // `dump_text_opts` already proved (above) that `scheme`/`stamp` toggle
    // which `<p>`s are `display: block` vs `display: none`. `fixtures/
    // color-scheme.html`'s three paragraphs stack vertically with no fixed
    // height anywhere in the document, so which ones are visible changes the
    // laid-out content height — and `dump_png_opts`'s own canvas height is
    // content-derived (see its "content-driven height" comment) — so a
    // correct color-scheme wiring must change the PNG's pixel dimensions
    // between light and dark, not just its dump_text string. This is the
    // pixel-path equivalent of `color_scheme_dark_flag_toggles_visible_text_
    // via_stamped_attribute_and_media_query` above.

    #[test]
    fn dump_png_color_scheme_default_matches_the_pre_t1d_render() {
        // Golden-churn safety: the DEFAULT (no --color-scheme given) render
        // path must be byte-identical to the pre-t1d two-argument call --
        // same rationale as dump_text's own
        // `color_scheme_default_does_not_stamp_and_matches_the_pre_t1b_golden_path`.
        let via_opts = dump_png_opts("fixtures/basic.html", false, style::ColorScheme::Light, false, None, None);
        let golden: &[u8] = include_bytes!("../goldens/basic.png");
        assert_eq!(via_opts.as_slice(), golden, "the default (unstamped, Light) PNG path must not have changed");
    }

    #[test]
    fn dump_png_color_scheme_dark_stamped_changes_the_rendered_pixels() {
        let light = dump_png_opts("fixtures/color-scheme.html", false, style::ColorScheme::Light, false, None, None);
        let dark = dump_png_opts("fixtures/color-scheme.html", false, style::ColorScheme::Dark, true, None, None);
        assert_ne!(light, dark, "--color-scheme dark (stamped) must change which paragraphs paint, hence the PNG bytes");

        let (_, light_h) = decode_png_dims(&light);
        let (_, dark_h) = decode_png_dims(&dark);
        assert_ne!(light_h, dark_h, "toggling which <p>s are display:none must change the content-driven canvas height");
    }

    #[test]
    fn dump_png_color_scheme_dark_without_stamping_still_reads_media_queries() {
        // `stamp = false` proves the two mechanisms (media evaluation vs
        // attribute stamp) are independently wired on the pixel path too --
        // mirrors dump_text's own
        // `color_scheme_dark_without_stamping_only_affects_media_queries_not_attribute_selectors`.
        let dark_no_stamp = dump_png_opts("fixtures/color-scheme.html", false, style::ColorScheme::Dark, false, None, None);
        let light = dump_png_opts("fixtures/color-scheme.html", false, style::ColorScheme::Light, false, None, None);
        assert_ne!(dark_no_stamp, light, "prefers-color-scheme is scheme-driven independent of stamping, so the render must still differ");
    }

    #[test]
    fn write_dump_png_opts_threads_color_scheme_through_to_disk() {
        let dir = std::env::temp_dir().join(format!("stele-write-dump-png-color-scheme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let out = dir.join("dark.png");
        write_dump_png_opts("fixtures/color-scheme.html", &out.to_string_lossy(), false, style::ColorScheme::Dark, true, None, None).expect("write should succeed");
        let on_disk = std::fs::read(&out).expect("png should be written");
        assert_eq!(on_disk, dump_png_opts("fixtures/color-scheme.html", false, style::ColorScheme::Dark, true, None, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_url_passes_through_http_and_file_schemes() {
        assert_eq!(resolve_url("http://example.com/x").as_str(), "http://example.com/x");
        assert_eq!(resolve_url("file:///abs/path.html").as_str(), "file:///abs/path.html");
    }

    #[test]
    fn resolve_url_passes_through_about_scheme() {
        // packet/attestation-modal: without this, `about:attestations`
        // resolves to a bogus `file://<cwd>/about:attestations` and no CLI
        // entry point can ever reach `fetch::about`.
        assert_eq!(resolve_url("about:attestations").as_str(), "about:attestations");
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

    // ---------------------------------------------------- classify_x11_intent
    // packet/chrome-address-edit, Task 3: a synthetic 2-column keysym table
    // (min_keycode 8, keysyms_per_keycode 2) covering every key this
    // function's own doc comment discusses -- column 0 unshifted, column 1
    // shifted, matching the real GetKeyboardMapping layout Task 2's
    // `keysym_for_keycode` indexes into.

    /// keycode -> (column 0 keysym, column 1 keysym), row-ordered starting
    /// at `min_keycode = 8`.
    fn classify_intent_keymap() -> (u8, u8, Vec<u32>) {
        let min_keycode = 8u8;
        let keysyms_per_keycode = 2u8;
        let rows: Vec<(u32, u32)> = vec![
            (0x71, 0x51),     // 8: 'q' / 'Q'
            (0xff1b, 0xff1b), // 9: Escape
            (0xff52, 0xff52), // 10: Up
            (0xff54, 0xff54), // 11: Down
            (0xff55, 0xff55), // 12: PageUp
            (0xff56, 0xff56), // 13: PageDown
            (0xffc2, 0xffc2), // 14: F5
            (0xff0d, 0xff0d), // 15: Enter
            (0xff08, 0xff08), // 16: Backspace
            (0xffff, 0xffff), // 17: Delete
            (0xff51, 0xff51), // 18: Left
            (0xff53, 0xff53), // 19: Right
            (0xff50, 0xff50), // 20: Home
            (0xff57, 0xff57), // 21: End
            (0xff09, 0xff09), // 22: Tab
        ];
        let mut keysyms = Vec::with_capacity(rows.len() * 2);
        for (a, b) in rows {
            keysyms.push(a);
            keysyms.push(b);
        }
        (min_keycode, keysyms_per_keycode, keysyms)
    }

    fn key_press(keycode: u8, shift: bool) -> stele::backend::x11::XEvent {
        stele::backend::x11::XEvent::KeyPress { keycode, state: if shift { 0x0001 } else { 0x0000 } }
    }

    #[test]
    fn classify_x11_intent_unfocused_matches_pre_packet_behavior_exactly() {
        use stele::backend::x11::XIntent;
        let (min_kc, per_kc, keysyms) = classify_intent_keymap();
        let height = 600u32;
        let cl = |kc: u8| classify_x11_intent(&key_press(kc, false), min_kc, per_kc, &keysyms, height, false);

        assert_eq!(cl(8), Some(XIntent::Quit), "'q'");
        assert_eq!(cl(9), Some(XIntent::Quit), "Escape");
        assert_eq!(cl(10), Some(XIntent::ScrollBy(-(X11_LINE_SCROLL as i32))), "Up");
        assert_eq!(cl(11), Some(XIntent::ScrollBy(X11_LINE_SCROLL as i32)), "Down");
        assert_eq!(cl(12), Some(XIntent::ScrollBy(-(height as i32))), "PageUp");
        assert_eq!(cl(13), Some(XIntent::ScrollBy(height as i32)), "PageDown");
        assert_eq!(cl(14), Some(XIntent::Reload), "F5");
        assert_eq!(cl(200), None, "unmapped keycode");
    }

    #[test]
    fn classify_x11_intent_focused_routes_to_edit_and_q_no_longer_quits() {
        use stele::backend::x11::{EditIntent, XIntent};
        let (min_kc, per_kc, keysyms) = classify_intent_keymap();
        let height = 600u32;
        let cl = |kc: u8| classify_x11_intent(&key_press(kc, false), min_kc, per_kc, &keysyms, height, true);

        // The collision fix -- the single most important assertion in this
        // task (design doc Goal #3): 'q' while focused types, never quits.
        assert_eq!(cl(8), Some(XIntent::Edit(EditIntent::Insert('q'))), "'q' must insert, not quit, while focused");
        assert_eq!(cl(9), Some(XIntent::Edit(EditIntent::Cancel)), "Escape cancels, doesn't quit");
        assert_eq!(cl(15), Some(XIntent::Edit(EditIntent::Commit)), "Enter commits");
        assert_eq!(cl(16), Some(XIntent::Edit(EditIntent::Backspace)));
        assert_eq!(cl(17), Some(XIntent::Edit(EditIntent::Delete)));
        assert_eq!(cl(18), Some(XIntent::Edit(EditIntent::Left)));
        assert_eq!(cl(19), Some(XIntent::Edit(EditIntent::Right)));
        assert_eq!(cl(20), Some(XIntent::Edit(EditIntent::Home)));
        assert_eq!(cl(21), Some(XIntent::Edit(EditIntent::End)));

        // Global shortcuts and Tab are swallowed while typing.
        assert_eq!(cl(14), None, "F5 must not reload while typing");
        assert_eq!(cl(10), None, "Up must not scroll while typing");
        assert_eq!(cl(11), None, "Down must not scroll while typing");
        assert_eq!(cl(12), None, "PageUp must not scroll while typing");
        assert_eq!(cl(13), None, "PageDown must not scroll while typing");
        assert_eq!(cl(22), None, "Tab is a flagged no-op in both states");
    }

    #[test]
    fn classify_x11_intent_shift_column_reaches_through_to_edit_insert() {
        // Proves Task 2's `column` plumbing reaches all the way through this
        // function, not just unit-tested in isolation at the x11.rs level:
        // Shift+'q' (state bit 0 set) must look up column 1 ('Q', keysym
        // 0x51) and, while focused, insert the SHIFTED char.
        use stele::backend::x11::{EditIntent, XIntent};
        let (min_kc, per_kc, keysyms) = classify_intent_keymap();
        let out = classify_x11_intent(&key_press(8, true), min_kc, per_kc, &keysyms, 600, true);
        assert_eq!(out, Some(XIntent::Edit(EditIntent::Insert('Q'))));
    }

    // ------------------------------------------------------- x11_edit_arg
    // packet/chrome-address-edit, Task 6: a direct unit test of the new
    // `ChromeState.edit` plumbing, independent of the full `run_x11` event
    // loop (which stays manual-verify-only -- no X server in CI).

    #[test]
    fn x11_edit_arg_is_none_when_unfocused_and_some_when_focused() {
        let mut a = AddressEdit::default();
        assert_eq!(x11_edit_arg(&a), None, "a freshly-defaulted AddressEdit is unfocused");

        a.focus("http://example.test/");
        assert_eq!(x11_edit_arg(&a), Some(("http://example.test/", "http://example.test/".chars().count())));

        a.insert_char('x');
        assert_eq!(x11_edit_arg(&a), Some(("http://example.test/x", "http://example.test/x".chars().count())));

        a.blur();
        assert_eq!(x11_edit_arg(&a), None, "blurred, even with a non-empty buffer, must report None");
    }

    #[test]
    fn resolve_url_used_by_edit_commit_never_panics_on_typed_address_bar_shapes() {
        // The exact helper `EditIntent::Commit` resolves a typed address
        // through (Task 6) -- a bare host-shaped string, an absolute
        // http(s):// string, and an empty string must all resolve to
        // something sane, never panic. (`AddressEdit::commit` itself already
        // guarantees the string handed to `resolve_url` is trimmed and
        // non-empty, but this helper is tested standalone against the wider
        // set of shapes `resolve_url` itself documents.)
        let _ = resolve_url("example.com");
        let _ = resolve_url("http://example.com/path");
        let _ = resolve_url("https://example.com/path");
        let _ = resolve_url(""); // never reached via `EditIntent::Commit` (AddressEdit::commit trims to non-empty first), but resolve_url itself must still stay total over it.
        assert_eq!(resolve_url("about:attestations").as_str(), "about:attestations");
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

    #[test]
    fn parse_args_dump_png_no_chrome_leaves_chrome_false() {
        let a = parse_args(&args(&["--dump-png", "fixtures/basic.html", "/tmp/out.png"]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert!(!a.chrome);
    }

    #[test]
    fn parse_args_dump_png_chrome_before_src_sets_chrome_and_positionals() {
        let a = parse_args(&args(&["--dump-png", "--chrome", "fixtures/basic.html", "/tmp/out.png"]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert!(a.chrome);
    }

    #[test]
    fn parse_args_dump_png_chrome_between_src_and_out_sets_chrome_and_positionals() {
        let a = parse_args(&args(&["--dump-png", "fixtures/basic.html", "--chrome", "/tmp/out.png"]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert!(a.chrome);
    }

    #[test]
    fn parse_args_dump_png_chrome_after_out_sets_chrome_and_positionals() {
        let a = parse_args(&args(&["--dump-png", "fixtures/basic.html", "/tmp/out.png", "--chrome"]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert!(a.chrome);
    }

    // packet/fixed-viewport final review: `--viewport-height` inline within
    // `--dump-png`'s <src> <out> stretch must not get mis-parsed as a
    // positional (the footgun this fix closes).

    #[test]
    fn parse_args_dump_png_viewport_height_before_src_sets_flag_and_positionals() {
        let a = parse_args(&args(&["--dump-png", "--viewport-height", "120", "fixtures/basic.html", "/tmp/out.png"]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert_eq!(a.viewport_height, Some(120));
    }

    #[test]
    fn parse_args_dump_png_chrome_and_viewport_height_before_src_sets_both_and_positionals() {
        let a = parse_args(&args(&[
            "--dump-png",
            "--chrome",
            "--viewport-height",
            "90",
            "fixtures/basic.html",
            "/tmp/out.png",
        ]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert!(a.chrome);
        assert_eq!(a.viewport_height, Some(90));
    }

    #[test]
    fn parse_args_dump_png_viewport_height_after_out_still_sets_flag_and_positionals() {
        let a = parse_args(&args(&[
            "--dump-png",
            "fixtures/basic.html",
            "/tmp/out.png",
            "--viewport-height",
            "120",
        ]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert_eq!(a.viewport_height, Some(120));
    }

    #[test]
    fn parse_args_dump_png_no_viewport_height_leaves_it_none() {
        let a = parse_args(&args(&["--dump-png", "fixtures/basic.html", "/tmp/out.png"]));
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
        assert_eq!(a.viewport_height, None);
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

    /// packet/fixed-viewport: `--viewport-height` opts `--dump-png` into the
    /// fixed-viewport layout path (`layout::layout_viewport`) — the encoded
    /// PNG's height is the requested window height, not the content-driven
    /// height the default (no-flag) path above produces. `fixtures/
    /// basic.html`'s default content height is well over 10px, so a 10px
    /// window genuinely exercises the clamp rather than coincidentally
    /// matching it.
    #[test]
    fn dump_png_with_viewport_height_clamps_the_encoded_png_to_the_requested_height() {
        let default_bytes = dump_png_opts("fixtures/basic.html", false, style::ColorScheme::Light, false, None, None);
        let (_, default_h) = decode_png_dims(&default_bytes);
        assert!(default_h > 10, "fixture must be taller than the clamp under test for this to be meaningful");

        let clamped_bytes = dump_png_opts("fixtures/basic.html", false, style::ColorScheme::Light, false, Some(10), None);
        assert!(clamped_bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        let (w, h) = decode_png_dims(&clamped_bytes);
        assert_eq!(w, DEFAULT_PNG_WIDTH);
        assert_eq!(h, 10);
    }

    // -------------------------------------------------------- --scroll-to (Acid2 scroll-to-fragment packet)

    /// A small synthetic fixture with (a) a `position:fixed` marker pinned
    /// to the top-left corner, (b) a 400px spacer, (c) a `<div id="mark">`
    /// partway down -- written to a temp file, same pattern
    /// `dump_png_applies_an_external_link_stylesheet` already uses (a
    /// `file://`-free literal path, `dump_png`'s own fetch resolves a bare
    /// filesystem path directly). `body { margin: 0 }` keeps the geometry
    /// exact (no UA 8px margin to account for).
    fn scroll_to_fixture_dir() -> std::path::PathBuf {
        // Unique dir per call. Three tests share this helper and cargo runs
        // them in parallel; a per-process shared path let one test's
        // truncate+rewrite (`fs::write`) race another test's two reads — the
        // second read seeing a half-written fixture, rendering a different
        // (shorter) PNG and failing an `assert_eq`. An atomic counter gives
        // each caller its own isolated fixture directory.
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("stele-scroll-to-{}-{}", std::process::id(), seq));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        std::fs::write(
            dir.join("scroll-to.html"),
            r#"<!doctype html><body style="margin:0">
<div style="position:fixed;top:0;left:0;width:20px;height:20px;background:rgb(10,20,220)"></div>
<div style="height:400px"></div>
<div id="mark" style="width:50px;height:50px;background:rgb(220,30,10)"></div>
</body>"#,
        )
        .expect("write fixture html");
        dir
    }

    /// The composition this packet delivers, end to end: `#mark` (well
    /// below the fold) scrolls into view at the window's top, WHILE the
    /// `position:fixed` marker stays pinned at the top-left corner, unmoved
    /// by the scroll. Both assertions land in one test since that's the
    /// actual behavior being delivered (design's own framing). Red against
    /// pre-Task-5 `main.rs` (the flag doesn't exist / isn't parsed / isn't
    /// wired into the paint call).
    #[test]
    fn scroll_to_composes_the_target_at_top_with_a_fixed_marker_unmoved() {
        let dir = scroll_to_fixture_dir();
        let src = dir.join("scroll-to.html").to_string_lossy().to_string();

        let bytes = dump_png_opts(&src, false, style::ColorScheme::Light, false, Some(200), Some("mark"));
        let (w, h) = decode_png_dims(&bytes);
        assert_eq!((w, h), (DEFAULT_PNG_WIDTH, 200));
        let pixels = decode_png_pixels(&bytes);
        let row_bytes = w as usize * 4;
        let band = |y0: usize, y1: usize| -> &[u8] { &pixels[y0 * row_bytes..(y1 * row_bytes).min(pixels.len())] };

        // (i) #mark's red scrolled into view near the window's top -- its
        // padding-top edge lands at exactly y=0 (find_fragment_top's own
        // contract), so scan generously (the top 60 rows of a 200px window)
        // rather than pinning an exact row.
        assert!(
            band(0, 60).chunks(4).any(|p| p == [220, 30, 10, 255]),
            "#mark's own background color should appear near the top of the scrolled window"
        );
        // (ii) the fixed marker's blue is STILL at the top-left corner,
        // unmoved by the scroll.
        assert!(
            band(0, 20).chunks(4).any(|p| p == [10, 20, 220, 255]),
            "the position:fixed marker's own background color must still be visible near the top, unmoved by the scroll"
        );
    }

    #[test]
    fn parse_args_reads_scroll_to() {
        let a = parse_args(&args(&["--dump-png", "--scroll-to", "mark", "--viewport-height", "200", "src.html", "out.png"]));
        assert_eq!(a.scroll_to_id, Some("mark".to_string()));
        assert_eq!(a.dump_png, Some(("src.html".to_string(), "out.png".to_string())));
        assert_eq!(a.viewport_height, Some(200));
    }

    /// The exact "swallow-as-positional" trap this loop's own comment
    /// documents for `--chrome`/`--viewport-height`: `--scroll-to`'s VALUE
    /// token, sitting between `--dump-png` and its `<src> <out.png>` pair,
    /// must not get silently consumed as one of the two positionals.
    #[test]
    fn parse_args_scroll_to_after_the_dump_png_positionals_still_parses() {
        let a = parse_args(&args(&["--dump-png", "src.html", "out.png", "--scroll-to", "mark", "--viewport-height", "200"]));
        assert_eq!(a.scroll_to_id, Some("mark".to_string()));
        assert_eq!(a.dump_png, Some(("src.html".to_string(), "out.png".to_string())));
        assert_eq!(a.viewport_height, Some(200));
    }

    /// `--scroll-to` alone (no `--viewport-height`) is a documented no-op:
    /// the render must be byte-identical to a call with NEITHER flag.
    #[test]
    fn scroll_to_without_viewport_height_is_a_no_op() {
        let dir = scroll_to_fixture_dir();
        let src = dir.join("scroll-to.html").to_string_lossy().to_string();

        let neither = dump_png_opts(&src, false, style::ColorScheme::Light, false, None, None);
        let scroll_to_only = dump_png_opts(&src, false, style::ColorScheme::Light, false, None, Some("mark"));
        assert_eq!(neither, scroll_to_only, "--scroll-to with no --viewport-height must render exactly like neither flag");
    }

    /// A `--scroll-to` id that doesn't resolve to any fragment degrades to
    /// `scroll_y = 0.0` -- byte-identical to an ordinary `--viewport-height`
    /// -only render (`find_fragment_top`'s own `None` case, Task 2) --
    /// never a panic, never an error.
    #[test]
    fn scroll_to_unknown_id_degrades_to_an_unscrolled_viewport_render() {
        let dir = scroll_to_fixture_dir();
        let src = dir.join("scroll-to.html").to_string_lossy().to_string();

        let viewport_only = dump_png_opts(&src, false, style::ColorScheme::Light, false, Some(200), None);
        let unknown_id = dump_png_opts(&src, false, style::ColorScheme::Light, false, Some(200), Some("nonexistent-id"));
        assert_eq!(viewport_only, unknown_id, "an unresolvable --scroll-to id must degrade to scroll_y = 0.0, not panic");
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
        let with_images = dump_png_opts("fixtures/bg-image.html", false, style::ColorScheme::Light, false, None, None);
        let without_images = dump_png_opts("fixtures/bg-image.html", true, style::ColorScheme::Light, false, None, None);

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
        assert_eq!(
            format_stats_line(counts, 4),
            "stele --stats: 3 ignored declarations, 1 ignored at-rule, 2 media blocks, 4 missing glyphs"
        );
    }

    #[test]
    fn format_stats_line_pluralizes_singular_and_plural_correctly() {
        assert_eq!(
            format_stats_line(StatsCounts::default(), 0),
            "stele --stats: 0 ignored declarations, 0 ignored at-rules, 0 media blocks, 0 missing glyphs"
        );
        assert_eq!(
            format_stats_line(StatsCounts { ignored_declarations: 1, ignored_at_rules: 1, media_at_rules: 1 }, 1),
            "stele --stats: 1 ignored declaration, 1 ignored at-rule, 1 media block, 1 missing glyph"
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
        assert_eq!(
            format_stats_line(counts, 0),
            "stele --stats: 2 ignored declarations, 1 ignored at-rule, 0 media blocks, 0 missing glyphs"
        );
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

    // -------------------------------------- missing-glyph counter (t2-glyph-fallback)

    #[test]
    fn count_missing_glyphs_is_zero_for_an_empty_fragment_slice() {
        assert_eq!(count_missing_glyphs(&[]), 0);
    }

    #[test]
    fn count_missing_glyphs_counts_across_multiple_text_fragments() {
        use stele::layout::{Point, Rect, Size as LSize};
        use stele::style::ComputedStyle;
        let text_fragment = |s: &str| layout::Fragment {
            rect: Rect { origin: Point { x: 0.0, y: 0.0 }, size: LSize { w: 8.0, h: 16.0 } },
            kind: layout::FragmentKind::Text { text: s.to_string(), baseline: 12.0, style: ComputedStyle::default() },
            interactive: None, clip: None, id: None, is_fixed: false,
        };
        // One emoji (missing) in the first fragment, an ASCII-only second
        // fragment (nothing missing), a CJK pair (two missing) in the third.
        let fragments = vec![text_fragment("hi \u{1F600}"), text_fragment("plain text"), text_fragment("\u{65E5}\u{672C}")];
        assert_eq!(count_missing_glyphs(&fragments), 3);
    }

    #[test]
    fn count_missing_glyphs_ignores_box_and_image_fragments() {
        use stele::img::RgbaImage;
        use stele::layout::{Point, Rect, Size as LSize};
        use stele::style::ComputedStyle;
        let fragments = vec![
            layout::Fragment {
                rect: Rect { origin: Point { x: 0.0, y: 0.0 }, size: LSize { w: 10.0, h: 10.0 } },
                kind: layout::FragmentKind::Box { style: ComputedStyle::default() },
                interactive: None, clip: None, id: None, is_fixed: false,
            },
            layout::Fragment {
                rect: Rect { origin: Point { x: 0.0, y: 0.0 }, size: LSize { w: 32.0, h: 32.0 } },
                kind: layout::FragmentKind::Image { image: RgbaImage::new(1, 1) },
                interactive: None, clip: None, id: None, is_fixed: false,
            },
        ];
        assert_eq!(count_missing_glyphs(&fragments), 0, "non-Text fragments carry no char content to count");
    }

    #[test]
    fn build_fragments_for_stats_is_empty_on_a_fetch_failure_not_a_panic() {
        assert!(build_fragments_for_stats("fixtures/does-not-exist-nope.html", 640.0).is_empty());
    }

    #[test]
    fn stats_pipeline_reports_missing_glyphs_for_a_real_document() {
        // fixtures/punctuation.html (packet t2-glyph-fallback) has genuinely
        // unmappable characters by design (see that fixture's own comments)
        // -- proves the wiring end to end, not just the pure counting helper.
        let fragments = build_fragments_for_stats("fixtures/punctuation.html", 640.0);
        assert!(!fragments.is_empty(), "fixture should lay out real content");
        assert!(count_missing_glyphs(&fragments) > 0, "fixtures/punctuation.html should report at least one missing glyph");
    }

    // ------------------------------------------------- --audit-contrast (T1c)

    #[test]
    fn parse_args_recognizes_audit_contrast() {
        let a = parse_args(&args(&["--headless", "--audit-contrast", "fixtures/basic.html"]));
        assert!(a.headless);
        assert_eq!(a.audit_contrast.as_deref(), Some("fixtures/basic.html"));
    }

    #[test]
    fn parse_args_audit_contrast_with_no_trailing_value_is_a_no_op_not_a_panic() {
        let a = parse_args(&args(&["--headless", "--audit-contrast"]));
        assert_eq!(a.audit_contrast, None);
    }

    #[test]
    fn audit_contrast_reports_zero_violations_on_a_clean_black_on_white_fixture() {
        let violations = audit_contrast("fixtures/basic.html").expect("basic.html should render");
        assert!(violations.is_empty(), "expected no contrast violations, got: {violations:?}");
    }

    #[test]
    fn audit_contrast_reports_zero_violations_on_the_kitchen_sink_fixture() {
        // The densest real fixture: a dark `.banner` with white text, a
        // `<pre>`/`.flexrow` with pale backgrounds, default black body text
        // -- every one of these already clears CONTRAST_MIN today, and this
        // packet's `repair_fg` must never turn a compliant color INTO a
        // violation.
        let violations = audit_contrast("fixtures/kitchen-sink.html").expect("kitchen-sink.html should render");
        assert!(violations.is_empty(), "expected no contrast violations, got: {violations:?}");
    }

    #[test]
    fn audit_contrast_reports_zero_violations_on_the_presentational_fixture() {
        // Its `<font color="red">` on the default white canvas is the
        // closest-to-the-floor real color pair in the whole fixture corpus
        // (~4.0:1, just above CONTRAST_MIN's 3.0:1) -- confirms the audit
        // doesn't false-positive right at the edge.
        let violations = audit_contrast("fixtures/presentational.html").expect("presentational.html should render");
        assert!(violations.is_empty(), "expected no contrast violations, got: {violations:?}");
    }

    #[test]
    fn audit_contrast_reports_zero_violations_on_a_background_image_only_box() {
        // fixtures/bg-image.html's `.tile` sets `color: #ffffff` but only a
        // `background-image` (no `background-color` of its own) --
        // `raster::effective_background` returns `None` (indeterminate: a
        // real image it can't sample) for this run, and this audit skips
        // any run it can't assess rather than flagging it -- so this must
        // report clean, same as every other fixture.
        let violations = audit_contrast("fixtures/bg-image.html").expect("bg-image.html should render");
        assert!(violations.is_empty(), "expected no contrast violations, got: {violations:?}");
    }

    #[test]
    fn audit_contrast_on_a_fetch_failure_is_a_clean_err_not_a_panic() {
        let result = audit_contrast("fixtures/does-not-exist-nope.html");
        assert!(result.is_err());
    }

    #[test]
    fn audit_contrast_on_a_frameset_document_is_empty_not_an_error() {
        // Same carve-out as dump_text/dump_png: a <frameset> document has
        // no single layout::layout call for this audit to drive.
        let violations = audit_contrast("fixtures/frames.html").expect("frameset documents degrade cleanly, never error");
        assert!(violations.is_empty());
    }
}
