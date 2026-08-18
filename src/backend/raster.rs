//! The pixel-backend painter (M4, Parts 3-4): walk paint-ordered
//! `layout::Fragment`s onto a `&mut dyn Surface`, and encode a `MemSurface`
//! to PNG bytes. The pixel analog of `backend::tty::render` — same input
//! contract (a `layout::layout`-produced fragment slice, painter's-algorithm
//! order), different output (real pixels instead of a character grid).
//!
//! ## What paints, what doesn't (mirrors `backend::tty`'s own such section)
//!
//! - `FragmentKind::Box { style }`: `fill_rect`s `style.background_color`
//!   (skipped entirely when its alpha is `0` — nothing to blend), then each
//!   of the four border edges whose `BorderStyle` is `Solid` and whose
//!   `width` rounds to `> 0` px, as its own filled rect in that edge's own
//!   color (CSS borders can differ per edge; `dashed`/`dotted`/`double`/...
//!   are out of scope — brief §4 only asks `solid` to be honored).
//! - `FragmentKind::Text { text, baseline, style }`: builds a `TextRun`
//!   (`x` from the fragment's own `rect.origin.x`, `baseline` = `rect.origin.y
//!   + baseline` — the same "top of line box + offset" contract
//!   `backend::tty`'s own module docs describe for this field, just consumed
//!   here as real pixels rather than a text-mode row) and hands it to
//!   `Surface::draw_text`.
//! - `FragmentKind::Image { image }`: `blit`s `image` into the fragment's
//!   own rect (images packet, M4): `layout::block::emit` only produces this
//!   fragment kind for a `Replaced` box whose `image` field is `Some` (a
//!   successfully fetched+decoded `<img>`) — a `Replaced` with no decoded
//!   image still paints as a plain `Box` placeholder, unchanged. `blit`
//!   itself (`MemSurface::blit`) handles the scaling (nearest-neighbor, to
//!   the fragment's laid-out size) and alpha-blending; this arm is a
//!   one-line hand-off.
//!
//! ## Totality
//!
//! `paint` never panics: every fragment rect is converted through
//! [`to_pixel_rect`], which clamps non-finite/negative/huge coordinates into
//! a bounded, `i32`/`u32`-safe range before it ever reaches `Surface`'s own
//! (already-clipping) `fill_rect`/`put_pixel`/`draw_text`. An empty fragment
//! slice paints nothing. [`encode_png`] never panics either: a zero-width or
//! zero-height surface (invalid per the PNG spec's own `IHDR` constraints)
//! degrades to a single blank white pixel rather than handing the `png`
//! crate a dimension it would reject.

use std::collections::HashMap;
use std::rc::Rc;

use crate::img::RgbaImage;
use crate::layout::{Fragment, FragmentKind, Rect as LayoutRect};
use crate::style::computed::{BorderSide, BorderStyle, ComputedStyle};
use crate::surface::{Color, MemSurface, Rect as PixelRect, Surface, TextRun};

/// Paint-ordered `fragments` onto `surface`, painter's-algorithm style
/// (later fragments draw over earlier ones — already the paint order
/// `layout::layout` produces, same contract `backend::tty::render` trusts).
///
/// `bg_images` (packet bg-image) is the decoded `background-image` map from
/// `bg_images::collect_bg_images`, keyed by each box's own RAW (unresolved)
/// `style.background_image` string — see that module's doc comment for why
/// the key is the raw url rather than a resolved one (in short: so this
/// function needs no document-base `Url` parameter of its own). Pass an
/// empty map to render with background-images fully suppressed (the
/// `--no-bg-images` kill switch, wired in `main.rs`) — every `Box` fragment
/// still paints its `background_color`, just never a background image.
pub fn paint(surface: &mut dyn Surface, fragments: &[Fragment], bg_images: &HashMap<String, Rc<RgbaImage>>) {
    for fragment in fragments {
        match &fragment.kind {
            FragmentKind::Box { style } => paint_box(surface, &fragment.rect, style, bg_images),
            FragmentKind::Text { text, baseline, style } => paint_text(surface, &fragment.rect, text, *baseline, style),
            FragmentKind::Image { image } => {
                surface.blit(to_pixel_rect(&fragment.rect), image);
            }
        }
    }
}

fn paint_box(surface: &mut dyn Surface, rect: &LayoutRect, style: &ComputedStyle, bg_images: &HashMap<String, Rc<RgbaImage>>) {
    let prect = to_pixel_rect(rect);
    if style.background_color.a != 0 {
        surface.fill_rect(prect, style.background_color);
    }
    // Background image paints ON TOP of background_color but BEHIND borders
    // and content (the box's own Text/Image children are later, separate
    // fragments in painter's-algorithm order — see `paint`'s own doc
    // comment): real CSS background painting order is background-color,
    // then background-image, then border, then content.
    if let Some(url) = style.background_image.as_deref() {
        if let Some(image) = bg_images.get(url) {
            paint_tiled_background(surface, prect, image);
        }
    }

    let top = border_px(&style.border.top);
    let right = border_px(&style.border.right);
    let bottom = border_px(&style.border.bottom);
    let left = border_px(&style.border.left);

    if let Some((w, color)) = top {
        surface.fill_rect(PixelRect { x: prect.x, y: prect.y, w: prect.w, h: w }, color);
    }
    if let Some((w, color)) = bottom {
        let h = w.min(prect.h);
        let y = prect.y + prect.h as i32 - h as i32;
        surface.fill_rect(PixelRect { x: prect.x, y, w: prect.w, h }, color);
    }
    if let Some((w, color)) = left {
        surface.fill_rect(PixelRect { x: prect.x, y: prect.y, w, h: prect.h }, color);
    }
    if let Some((w, color)) = right {
        let w2 = w.min(prect.w);
        let x = prect.x + prect.w as i32 - w2 as i32;
        surface.fill_rect(PixelRect { x, y: prect.y, w: w2, h: prect.h }, color);
    }
}

/// Tile `image` across `prect` (packet bg-image): CSS `background-repeat:
/// repeat` default only — top-left origin, natural image size, no
/// `background-size`/`background-position`/other `-repeat` variants (curated
/// scope per the packet brief; a later packet can add them). Deliberately
/// does NOT go through `Surface::blit` (which nearest-neighbor SCALES its
/// source to fill whatever `at` rect it's given): a partial edge tile passed
/// through `blit` at less than the image's natural size would get scaled
/// down and visibly distorted, not clipped. Instead this samples `image`
/// directly via `put_pixel` (both already part of the `Surface` trait this
/// module already calls — no change to the frozen `surface` module), one
/// destination pixel at a time, wrapping the source coordinate with
/// `rem_euclid` against the image's own dimensions — an exact, undistorted
/// tile repeat.
///
/// Total: a zero-size `image` or `prect` is a no-op (nothing to sample /
/// nowhere to paint). The pixel loop is bounded by intersecting `prect` with
/// `surface`'s own bounds BEFORE looping — not by `prect`'s own size alone —
/// so a hostile/huge box (bounded only by `to_pixel_rect`'s `MAX_COORD`
/// clamp, not by the surface) still costs at most one iteration per
/// on-surface pixel, never `prect.w * prect.h` unclipped iterations (which
/// could otherwise approach `MAX_COORD^2`).
fn paint_tiled_background(surface: &mut dyn Surface, prect: PixelRect, image: &RgbaImage) {
    if image.width == 0 || image.height == 0 || prect.w == 0 || prect.h == 0 {
        return;
    }
    let (sw, sh) = surface.size();
    let x0 = (prect.x as i64).max(0);
    let y0 = (prect.y as i64).max(0);
    let x1 = (prect.x as i64 + prect.w as i64).min(sw as i64);
    let y1 = (prect.y as i64 + prect.h as i64).min(sh as i64);
    if x1 <= x0 || y1 <= y0 {
        return; // box doesn't intersect the surface at all
    }

    let iw = image.width as i64;
    let ih = image.height as i64;
    // Tile phase is anchored to the box's own (unclipped) origin, not the
    // surface-clipped x0/y0 -- a box partially off-surface (or off the top-
    // left of the viewport) still tiles as if drawn from its real top-left,
    // matching CSS `background-repeat: repeat` semantics.
    let (ox, oy) = (prect.x as i64, prect.y as i64);

    for y in y0..y1 {
        let src_y = ((y - oy).rem_euclid(ih) as usize).min(image.height as usize - 1);
        let row = src_y * image.width as usize * 4;
        for x in x0..x1 {
            let src_x = ((x - ox).rem_euclid(iw) as usize).min(image.width as usize - 1);
            let idx = row + src_x * 4;
            let Some(px) = image.pixels.get(idx..idx + 4) else { continue };
            surface.put_pixel(x as i32, y as i32, Color { r: px[0], g: px[1], b: px[2], a: px[3] });
        }
    }
}

/// `Some((pixel_width, color))` for a border edge that should actually
/// paint: `style == Solid` (brief §4: only `solid` is honored in v0 — see
/// `BorderStyle`'s own doc comment), a `width` that rounds to `>= 1px`, and
/// a non-fully-transparent color. `None` (no `fill_rect` call at all) for
/// `BorderStyle::None`, a non-finite/negative width, a sub-half-pixel width
/// that rounds down to `0`, or `color.a == 0` (review Minor #2: a `Solid`
/// border whose color is fully transparent would otherwise still cost a
/// full-edge `fill_rect` that blends every pixel to a no-op — harmless
/// today, but wasted work with no visible effect, so it's rejected here
/// alongside the other "nothing would actually paint" cases).
fn border_px(side: &BorderSide) -> Option<(u32, Color)> {
    if side.style != BorderStyle::Solid {
        return None;
    }
    if !side.width.is_finite() || side.width <= 0.0 {
        return None;
    }
    if side.color.a == 0 {
        return None;
    }
    let w = side.width.round().clamp(0.0, MAX_COORD) as u32;
    if w == 0 {
        return None;
    }
    Some((w, side.color))
}

fn paint_text(surface: &mut dyn Surface, rect: &LayoutRect, text: &str, baseline: f32, style: &ComputedStyle) {
    if text.is_empty() {
        return;
    }
    let x = to_i32(finite_or(rect.origin.x, 0.0));
    let top_y = finite_or(rect.origin.y, 0.0);
    let bl = finite_or(baseline, 0.0);
    let baseline_px = to_i32(top_y + bl);
    let run = TextRun { text, x, baseline: baseline_px, size_px: style.font_size, color: style.color };
    surface.draw_text(&run);
}

/// Bound on any single pixel-space coordinate/dimension this module
/// produces, well inside `i32`/`u32` range even after the small arithmetic
/// (`x + w`, `y + h`) `fill_rect`/border placement does on it — see
/// [`to_pixel_rect`]'s doc comment.
const MAX_COORD: f32 = 1_000_000.0;

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

fn to_i32(v: f32) -> i32 {
    finite_or(v, 0.0).clamp(-MAX_COORD, MAX_COORD) as i32
}

/// Convert a continuous layout-space `LayoutRect` (`f32` origin/size, may be
/// non-finite/negative/huge — document-controlled) into a `Surface`-space
/// pixel `Rect`, clamped to a bounded, cast-safe range. See module docs.
fn to_pixel_rect(rect: &LayoutRect) -> PixelRect {
    let x = to_i32(rect.origin.x);
    let y = to_i32(rect.origin.y);
    let w = finite_or(rect.size.w, 0.0).clamp(0.0, MAX_COORD) as u32;
    let h = finite_or(rect.size.h, 0.0).clamp(0.0, MAX_COORD) as u32;
    PixelRect { x, y, w, h }
}

/// Encode `surface`'s RGBA8 pixels as PNG bytes (M4 Part 4). Deterministic
/// (no timestamp/text chunks — just `IHDR`/`IDAT`/`IEND`). See module docs
/// for the zero-dimension fallback.
pub fn encode_png(surface: &MemSurface) -> Vec<u8> {
    let (w, h) = surface.size();
    if w == 0 || h == 0 {
        // A zero-dimension IHDR is invalid PNG; degrade to a single blank
        // white pixel rather than handing the `png` crate a rejected
        // dimension (or, worse, panicking on it).
        return encode_png_unchecked(&MemSurface::new(1, 1, Color::WHITE));
    }
    encode_png_unchecked(surface)
}

fn encode_png_unchecked(surface: &MemSurface) -> Vec<u8> {
    let (w, h) = surface.size();
    // `MemSurface`'s own documented invariant (`pixels: width * height * 4`
    // bytes, RGBA8) is exactly what makes `write_image_data` infallible here
    // in practice (see below). Review fix (Minor #3): that invariant was
    // previously only asserted in a comment; a `debug_assert_eq!` makes a
    // future change that breaks it (on either side: `MemSurface` or this
    // function) fail loudly in debug/test builds instead of silently
    // encoding a corrupt PNG (or writing partial bytes) with no signal. Not
    // a `assert!`/panic in release: the release-profile totality contract
    // (`panic = "abort"`, brief's "no reachable panic" bar) shouldn't gate
    // on an invariant that's cheap to verify here in debug and otherwise
    // unreachable given `MemSurface::new`'s own construction.
    debug_assert_eq!(
        surface.bytes().len(),
        (w as usize) * (h as usize) * 4,
        "MemSurface invariant violated: bytes().len() must be width * height * 4 (RGBA8)"
    );
    let mut buf = Vec::new();
    let mut encoder = png::Encoder::new(&mut buf, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = encoder.write_header() {
        // Both `write_header` and `write_image_data` are fallible only for
        // I/O errors or a size mismatch; writing into a `Vec<u8>` with
        // exactly `w * h * 4` bytes (MemSurface's own invariant, asserted
        // above) can't hit either in practice. Degrading to whatever bytes
        // were written so far (rather than unwrapping) keeps this function
        // panic-free regardless (release builds have no debug_assert to
        // catch a violation, so this fallback is the real backstop there).
        let _ = writer.write_image_data(surface.bytes());
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::img::RgbaImage;
    use crate::layout::{Point, Size};
    use crate::style::computed::{BorderSide, BorderStyle, Edges};

    fn box_style(bg: Color, border: BorderSide) -> ComputedStyle {
        ComputedStyle { background_color: bg, border: Edges::all(border), ..ComputedStyle::default() }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> LayoutRect {
        LayoutRect { origin: Point { x, y }, size: Size { w, h } }
    }

    fn px(s: &MemSurface, x: i32, y: i32) -> Color {
        let (w, _h) = s.size();
        let i = ((y as usize) * (w as usize) + (x as usize)) * 4;
        let b = s.bytes();
        Color { r: b[i], g: b[i + 1], b: b[i + 2], a: b[i + 3] }
    }

    // ------------------------------------------------------------- paint: Box

    #[test]
    fn box_fragment_fills_its_background_color() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = box_style(Color::rgb(10, 20, 30), BorderSide::default());
        let fragments = vec![Fragment { rect: rect(2.0, 2.0, 4.0, 4.0), kind: FragmentKind::Box { style }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        assert_eq!(px(&s, 3, 3), Color::rgb(10, 20, 30));
        assert_eq!(px(&s, 0, 0), Color::WHITE, "outside the box stays background");
    }

    #[test]
    fn transparent_background_paints_nothing() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = box_style(Color::TRANSPARENT, BorderSide::default());
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        assert_eq!(px(&s, 5, 5), Color::WHITE);
    }

    #[test]
    fn solid_border_paints_all_four_edges_in_the_border_color() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let border = BorderSide { width: 2.0, style: BorderStyle::Solid, color: Color::rgb(255, 0, 0) };
        let style = box_style(Color::TRANSPARENT, border);
        let fragments = vec![Fragment { rect: rect(2.0, 2.0, 6.0, 6.0), kind: FragmentKind::Box { style }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        // top edge
        assert_eq!(px(&s, 4, 2), Color::rgb(255, 0, 0));
        // left edge
        assert_eq!(px(&s, 2, 4), Color::rgb(255, 0, 0));
        // right edge (box spans x in [2,8), 2px border -> last 2 cols [6,8))
        assert_eq!(px(&s, 7, 4), Color::rgb(255, 0, 0));
        // bottom edge (box spans y in [2,8), 2px border -> last 2 rows [6,8))
        assert_eq!(px(&s, 4, 7), Color::rgb(255, 0, 0));
        // interior stays untouched (transparent bg -> background shows through)
        assert_eq!(px(&s, 4, 4), Color::WHITE);
    }

    #[test]
    fn none_style_border_paints_nothing_even_with_nonzero_width() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let border = BorderSide { width: 3.0, style: BorderStyle::None, color: Color::rgb(255, 0, 0) };
        let style = box_style(Color::TRANSPARENT, border);
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        assert_eq!(px(&s, 0, 0), Color::WHITE);
    }

    /// packet/hr-rule: `<hr>`'s UA shape is a ZERO-content-height box with
    /// ONLY a solid top border (right/bottom/left stay `BorderSide::
    /// default()`, unlike `box_style`'s helper above which uses `Edges::
    /// all`) -- confirms `paint_box`/`border_px` already handle this
    /// asymmetric, zero-height case correctly with no code change: the top
    /// edge is its own independent `fill_rect` sized by ITS OWN border
    /// width, not gated on `rect.h` being nonzero, so a 0-height box still
    /// paints a visible 1px line spanning its full content width.
    #[test]
    fn zero_height_box_with_only_a_solid_top_border_paints_a_horizontal_rule_line() {
        let mut s = MemSurface::new(10, 4, Color::WHITE);
        let gray = Color::rgb(0x80, 0x80, 0x80);
        let top = BorderSide { width: 1.0, style: BorderStyle::Solid, color: gray };
        let style = ComputedStyle {
            background_color: Color::TRANSPARENT,
            border: Edges { top, right: BorderSide::default(), bottom: BorderSide::default(), left: BorderSide::default() },
            ..ComputedStyle::default()
        };
        // hr's own box: full width, zero content height (matches `height: 0`
        // in the UA sheet -- border adds its own 1px on top of that).
        let fragments = vec![Fragment { rect: rect(0.0, 1.0, 10.0, 0.0), kind: FragmentKind::Box { style }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        for x in 0..10 {
            assert_eq!(px(&s, x, 1), gray, "row 1 (the box's own y) should be the gray rule line at col {x}");
        }
        assert_eq!(px(&s, 0, 0), Color::WHITE, "row above the rule must be untouched");
        assert_eq!(px(&s, 0, 2), Color::WHITE, "row below the rule must be untouched (no bottom/side edges)");
    }

    /// Review fix (Minor #2): a `Solid` border whose color is fully
    /// transparent (`a == 0`) should short-circuit in `border_px` rather
    /// than reaching a `fill_rect` call that blends every edge pixel to a
    /// no-op. Behaviorally identical either way (harmless, just wasteful
    /// pre-fix) -- this pins the no-op result.
    #[test]
    fn solid_border_with_fully_transparent_color_paints_nothing() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let border = BorderSide { width: 3.0, style: BorderStyle::Solid, color: Color::rgba(255, 0, 0, 0) };
        let style = box_style(Color::TRANSPARENT, border);
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        assert_eq!(px(&s, 0, 0), Color::WHITE);
    }

    // ------------------------------------------------------- paint: bg-image

    fn solid_rgba_image(w: u32, h: u32, color: [u8; 4]) -> RgbaImage {
        let mut image = RgbaImage::new(w, h);
        image.pixels = color.repeat((w * h) as usize);
        image
    }

    fn box_style_with_bg_image(url: &str) -> ComputedStyle {
        ComputedStyle { background_color: Color::WHITE, background_image: Some(url.into()), ..ComputedStyle::default() }
    }

    #[test]
    fn box_with_a_resolvable_bg_image_tiles_it_across_the_box_rect() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = box_style_with_bg_image("tile.png");
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("tile.png".to_string(), Rc::new(solid_rgba_image(2, 2, [10, 200, 30, 255])));
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 6.0, 6.0), kind: FragmentKind::Box { style }, interactive: None }];

        paint(&mut s, &fragments, &bg_images);

        // Every pixel inside the 6x6 box should be the tile color (a 2x2
        // tile repeated evenly across 6x6 divides exactly).
        for y in 0..6 {
            for x in 0..6 {
                assert_eq!(px(&s, x, y), Color::rgb(10, 200, 30), "pixel ({x},{y}) should show the tiled bg image");
            }
        }
        // Outside the box, the surface background is untouched.
        assert_eq!(px(&s, 8, 8), Color::WHITE);
    }

    #[test]
    fn bg_image_tiling_wraps_at_the_image_boundary_not_stretched() {
        // A 2x2 tile across a 5x5 box (doesn't divide evenly) must repeat
        // (wrap), not scale/stretch to fill -- pin one full second tile
        // period plus a clipped partial tile at the box's far edge, all
        // still showing the SAME tile color (a stretch/scale bug would show
        // a blended/different color at the seam).
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = box_style_with_bg_image("tile.png");
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("tile.png".to_string(), Rc::new(solid_rgba_image(2, 2, [1, 2, 3, 255])));
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 5.0, 5.0), kind: FragmentKind::Box { style }, interactive: None }];

        paint(&mut s, &fragments, &bg_images);

        for y in 0..5 {
            for x in 0..5 {
                assert_eq!(px(&s, x, y), Color::rgb(1, 2, 3));
            }
        }
    }

    #[test]
    fn unresolved_bg_image_url_falls_back_to_background_color_only() {
        // The style names a url, but the map has no entry for it (e.g. the
        // fetch/decode failed) -- must paint only background_color, no
        // panic, no partial image.
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = ComputedStyle { background_color: Color::rgb(5, 5, 5), background_image: Some("missing.png".into()), ..ComputedStyle::default() };
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style }, interactive: None }];

        paint(&mut s, &fragments, &HashMap::new());

        assert_eq!(px(&s, 5, 5), Color::rgb(5, 5, 5));
    }

    #[test]
    fn no_bg_image_declared_paints_only_background_color() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = box_style(Color::rgb(1, 2, 3), BorderSide::default());
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("unrelated.png".to_string(), Rc::new(solid_rgba_image(2, 2, [255, 0, 0, 255])));
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style }, interactive: None }];

        paint(&mut s, &fragments, &bg_images);

        assert_eq!(px(&s, 5, 5), Color::rgb(1, 2, 3), "no background_image on this style, so no bg-image map lookup should apply");
    }

    /// Toggle proof (packet brief: "assert that `--no-bg-images` produces a
    /// DIFFERENT render"): passing an EMPTY map (what `main.rs` does for
    /// `--no-bg-images`) instead of a populated one changes the painted
    /// pixels for the exact same fragments -- the same box shows its
    /// background_color alone instead of the tiled image.
    #[test]
    fn empty_bg_images_map_renders_differently_from_a_populated_one() {
        let style = box_style_with_bg_image("tile.png");
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 6.0, 6.0), kind: FragmentKind::Box { style }, interactive: None }];

        let mut with_image = MemSurface::new(10, 10, Color::WHITE);
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("tile.png".to_string(), Rc::new(solid_rgba_image(2, 2, [10, 200, 30, 255])));
        paint(&mut with_image, &fragments, &bg_images);

        let mut without_image = MemSurface::new(10, 10, Color::WHITE);
        paint(&mut without_image, &fragments, &HashMap::new()); // --no-bg-images

        assert_ne!(with_image.bytes(), without_image.bytes());
        assert_eq!(px(&without_image, 3, 3), Color::WHITE, "--no-bg-images: box shows only its (here, default transparent) background_color");
    }

    #[test]
    fn text_painted_over_a_bg_image_still_shows_paint_order() {
        // Box fragment (with its bg image) painted first, Text fragment for
        // the same rect painted second -- painter's-algorithm order (see
        // `paint`'s own doc comment) means the text's ink must still show,
        // not get hidden behind the tiled image painted after it.
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let box_style = box_style_with_bg_image("tile.png");
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("tile.png".to_string(), Rc::new(solid_rgba_image(2, 2, [0, 0, 255, 255])));
        let text_style = ComputedStyle { color: Color::BLACK, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: box_style }, interactive: None },
            Fragment { rect: rect(4.0, 0.0, 8.0, 16.0), kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style: text_style }, interactive: None },
        ];

        paint(&mut s, &fragments, &bg_images);

        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "text ink must still be visible painted over the bg image");
    }

    #[test]
    fn zero_size_bg_image_or_box_is_a_no_op_not_a_panic() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("empty.png".to_string(), Rc::new(RgbaImage::new(0, 0)));
        bg_images.insert("zero-box.png".to_string(), Rc::new(solid_rgba_image(2, 2, [9, 9, 9, 255])));
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 5.0, 5.0), kind: FragmentKind::Box { style: box_style_with_bg_image("empty.png") }, interactive: None },
            Fragment { rect: rect(0.0, 0.0, 0.0, 0.0), kind: FragmentKind::Box { style: box_style_with_bg_image("zero-box.png") }, interactive: None },
        ];
        paint(&mut s, &fragments, &bg_images); // must not panic
        assert_eq!(px(&s, 2, 2), Color::WHITE);
    }

    #[test]
    fn huge_box_with_a_bg_image_is_bounded_by_the_surface_not_a_hang() {
        // Layout can hand paint an enormous (but MAX_COORD-clamped) box rect
        // -- the tiling loop must be bounded by the actual (small) surface,
        // not by the box's own huge nominal size, or this test would hang.
        let mut s = MemSurface::new(8, 8, Color::WHITE);
        let style = box_style_with_bg_image("tile.png");
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        bg_images.insert("tile.png".to_string(), Rc::new(solid_rgba_image(2, 2, [4, 5, 6, 255])));
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 500_000.0, 500_000.0), kind: FragmentKind::Box { style }, interactive: None }];

        paint(&mut s, &fragments, &bg_images); // must return promptly, not hang

        assert_eq!(px(&s, 4, 4), Color::rgb(4, 5, 6));
    }

    #[test]
    fn hostile_many_bg_image_boxes_do_not_panic() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let mut bg_images: HashMap<String, Rc<RgbaImage>> = HashMap::new();
        for i in 0..200 {
            bg_images.insert(format!("tile-{i}.png"), Rc::new(solid_rgba_image(2, 2, [1, 1, 1, 255])));
        }
        let fragments: Vec<Fragment> = (0..200)
            .map(|i| Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: box_style_with_bg_image(&format!("tile-{i}.png")) }, interactive: None })
            .collect();
        paint(&mut s, &fragments, &bg_images); // must not panic
    }

    // ------------------------------------------------------------ paint: Text

    #[test]
    fn text_fragment_draws_ink_at_the_expected_position() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let style = ComputedStyle { color: Color::BLACK, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![Fragment {
            rect: rect(4.0, 0.0, 8.0, 16.0),
            kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style },
            interactive: None,
        }];
        paint(&mut s, &fragments, &HashMap::new());
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "expected some glyph ink to be painted");
    }

    #[test]
    fn empty_text_fragment_paints_nothing() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = ComputedStyle::default();
        let fragments =
            vec![Fragment {
                rect: rect(0.0, 0.0, 10.0, 10.0),
                kind: FragmentKind::Text { text: String::new(), baseline: 8.0, style },
                interactive: None,
            }];
        paint(&mut s, &fragments, &HashMap::new());
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    // ----------------------------------------------------------- paint: Image

    #[test]
    fn image_fragment_is_blitted_into_its_rect() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let mut image = RgbaImage::new(2, 2);
        image.pixels = [255u8, 0, 0, 255].repeat(4); // solid opaque red, 2x2
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 4.0, 4.0), kind: FragmentKind::Image { image }, interactive: None }];
        paint(&mut s, &fragments, &HashMap::new());
        assert_eq!(px(&s, 0, 0), Color::rgb(255, 0, 0));
        assert_eq!(px(&s, 3, 3), Color::rgb(255, 0, 0));
        assert_eq!(px(&s, 5, 5), Color::WHITE, "outside the image's rect stays background");
    }

    #[test]
    fn fully_transparent_image_fragment_paints_nothing() {
        // Distinct from the skip-entirely M2 scope call this replaces:
        // `paint` now always calls `blit`, but a fully transparent source
        // image (RgbaImage::new's own default: zeroed pixels, alpha 0)
        // still results in no visible change, via blit's own alpha blend
        // (put_pixel is a no-op for alpha 0) -- not because `paint` special-
        // cased it.
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let fragments =
            vec![Fragment {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                kind: FragmentKind::Image { image: RgbaImage::new(2, 2) },
                interactive: None,
            }];
        paint(&mut s, &fragments, &HashMap::new()); // must not panic
        assert_eq!(px(&s, 0, 0), Color::WHITE);
    }

    // -------------------------------------------------------------- ordering

    #[test]
    fn paint_order_wins_ties_like_the_tty_backend() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let first = box_style(Color::rgb(255, 0, 0), BorderSide::default());
        let second = box_style(Color::rgb(0, 255, 0), BorderSide::default());
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style: first }, interactive: None },
            Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style: second }, interactive: None },
        ];
        paint(&mut s, &fragments, &HashMap::new());
        assert_eq!(px(&s, 5, 5), Color::rgb(0, 255, 0));
    }

    // --------------------------------------------------------------- totality

    #[test]
    fn degenerate_rects_never_panic() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let degenerate = [
            (f32::NAN, f32::NAN, f32::NAN, f32::NAN),
            (f32::INFINITY, f32::INFINITY, f32::INFINITY, f32::INFINITY),
            (f32::NEG_INFINITY, f32::NEG_INFINITY, -1.0, -1.0),
            (f32::MAX, f32::MAX, f32::MAX, f32::MAX),
            (-1.0, -1.0, -5.0, -5.0),
        ];
        for (x, y, w, h) in degenerate {
            let border = BorderSide { width: f32::NAN, style: BorderStyle::Solid, color: Color::BLACK };
            let style = box_style(Color::rgb(1, 2, 3), border);
            let text_style = ComputedStyle { font_size: f32::NAN, ..ComputedStyle::default() };
            let fragments = vec![
                Fragment { rect: rect(x, y, w, h), kind: FragmentKind::Box { style }, interactive: None },
                Fragment {
                    rect: rect(x, y, w, h),
                    kind: FragmentKind::Text { text: "z".to_string(), baseline: f32::NAN, style: text_style },
                    interactive: None,
                },
                Fragment { rect: rect(x, y, w, h), kind: FragmentKind::Image { image: RgbaImage::new(1, 1) }, interactive: None },
            ];
            paint(&mut s, &fragments, &HashMap::new()); // must not panic
        }
    }

    #[test]
    fn empty_fragment_list_paints_nothing() {
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        paint(&mut s, &[], &HashMap::new());
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    // ------------------------------------------------------------- encode_png

    #[test]
    fn encode_png_round_trips_pixel_data() {
        let mut s = MemSurface::new(3, 2, Color::WHITE);
        s.fill_rect(PixelRect { x: 0, y: 0, w: 1, h: 1 }, Color::rgb(10, 20, 30));
        let bytes = encode_png(&s);
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "should start with the PNG magic bytes");

        let decoder = png::Decoder::new(bytes.as_slice());
        let mut reader = decoder.read_info().expect("valid PNG");
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("valid PNG frame");
        assert_eq!(info.width, 3);
        assert_eq!(info.height, 2);
        assert_eq!(&buf[..info.buffer_size()], s.bytes());
    }

    #[test]
    fn encode_png_on_a_zero_dimension_surface_is_a_blank_pixel_not_a_panic() {
        let s = MemSurface::new(0, 0, Color::WHITE);
        let bytes = encode_png(&s); // must not panic
        assert!(!bytes.is_empty());
        let decoder = png::Decoder::new(bytes.as_slice());
        let reader = decoder.read_info().expect("valid PNG even for the zero-dim fallback");
        assert!(reader.info().width >= 1);
        assert!(reader.info().height >= 1);
    }

    #[test]
    fn encode_png_is_deterministic() {
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.fill_rect(PixelRect { x: 1, y: 1, w: 2, h: 2 }, Color::BLACK);
        assert_eq!(encode_png(&s), encode_png(&s));
    }

    // ------------------------------------------------ effective_background

    #[test]
    fn effective_background_nested_boxes_picks_the_innermost_opaque_one() {
        let mut fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: box_style(Color::rgb(10, 10, 10), BorderSide::default()) }, interactive: None },
            Fragment { rect: rect(2.0, 2.0, 10.0, 10.0), kind: FragmentKind::Box { style: box_style(Color::rgb(20, 20, 20), BorderSide::default()) }, interactive: None },
        ];
        fragments.push(Fragment {
            rect: rect(3.0, 3.0, 4.0, 4.0),
            kind: FragmentKind::Text { text: "x".to_string(), baseline: 3.0, style: ComputedStyle::default() },
            interactive: None,
        });
        let eff = effective_background(&fragments, 2, Color::WHITE);
        assert_eq!(eff, Color::rgb(20, 20, 20), "the innermost (nearer, later-painted) opaque box wins");
    }

    #[test]
    fn effective_background_skips_a_transparent_box_and_falls_to_the_next_opaque_ancestor() {
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: box_style(Color::rgb(5, 5, 5), BorderSide::default()) }, interactive: None },
            Fragment { rect: rect(2.0, 2.0, 10.0, 10.0), kind: FragmentKind::Box { style: box_style(Color::TRANSPARENT, BorderSide::default()) }, interactive: None },
            Fragment {
                rect: rect(3.0, 3.0, 4.0, 4.0),
                kind: FragmentKind::Text { text: "x".to_string(), baseline: 3.0, style: ComputedStyle::default() },
                interactive: None,
            },
        ];
        let eff = effective_background(&fragments, 2, Color::WHITE);
        assert_eq!(eff, Color::rgb(5, 5, 5), "a transparent box must not shadow the next opaque ancestor beneath it");
    }

    #[test]
    fn effective_background_with_no_containing_box_falls_back_to_canvas() {
        let fragments = vec![Fragment {
            rect: rect(3.0, 3.0, 4.0, 4.0),
            kind: FragmentKind::Text { text: "x".to_string(), baseline: 3.0, style: ComputedStyle::default() },
            interactive: None,
        }];
        assert_eq!(effective_background(&fragments, 0, Color::WHITE), Color::WHITE);
        assert_eq!(effective_background(&fragments, 0, Color::rgb(1, 2, 3)), Color::rgb(1, 2, 3), "must thread the caller's own canvas through, not hardcode white");
    }

    #[test]
    fn effective_background_ignores_a_box_painted_after_the_text() {
        let fragments = vec![
            Fragment {
                rect: rect(0.0, 0.0, 20.0, 20.0),
                kind: FragmentKind::Text { text: "x".to_string(), baseline: 3.0, style: ComputedStyle::default() },
                interactive: None,
            },
            Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: box_style(Color::rgb(1, 2, 3), BorderSide::default()) }, interactive: None },
        ];
        let eff = effective_background(&fragments, 0, Color::WHITE);
        assert_eq!(eff, Color::WHITE, "a box painted AFTER this text must not count as its background");
    }

    #[test]
    fn effective_background_a_box_not_containing_the_text_origin_is_skipped() {
        let fragments = vec![
            Fragment { rect: rect(50.0, 50.0, 5.0, 5.0), kind: FragmentKind::Box { style: box_style(Color::rgb(9, 9, 9), BorderSide::default()) }, interactive: None },
            Fragment {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                kind: FragmentKind::Text { text: "x".to_string(), baseline: 3.0, style: ComputedStyle::default() },
                interactive: None,
            },
        ];
        let eff = effective_background(&fragments, 1, Color::WHITE);
        assert_eq!(eff, Color::WHITE, "a box that doesn't geometrically contain the text's own origin must not count");
    }

    #[test]
    fn effective_background_out_of_range_index_is_canvas_not_a_panic() {
        let fragments: Vec<Fragment> = Vec::new();
        assert_eq!(effective_background(&fragments, 5, Color::WHITE), Color::WHITE);
    }

    // ---------------------------------------------------- paint: contrast repair

    #[test]
    fn paint_repairs_illegible_white_text_over_the_default_white_canvas() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let text_style = ComputedStyle { color: Color::WHITE, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![Fragment {
            rect: rect(0.0, 0.0, 16.0, 16.0),
            kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style: text_style },
            interactive: None,
        }];
        paint(&mut s, &fragments, &HashMap::new(), Color::WHITE);
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "white text on the white canvas must be repaired to black ink");
    }

    #[test]
    fn paint_leaves_already_legible_black_on_white_text_unchanged() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let text_style = ComputedStyle { color: Color::BLACK, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![Fragment {
            rect: rect(0.0, 0.0, 16.0, 16.0),
            kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style: text_style },
            interactive: None,
        }];
        paint(&mut s, &fragments, &HashMap::new(), Color::WHITE);
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "already-legible black-on-white text must still render");
    }

    #[test]
    fn paint_repairs_white_text_against_its_own_boxs_light_background_not_just_the_canvas() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let light_box = box_style(Color::rgb(240, 240, 240), BorderSide::default());
        let text_style = ComputedStyle { color: Color::WHITE, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: light_box }, interactive: None },
            Fragment {
                rect: rect(0.0, 0.0, 16.0, 16.0),
                kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style: text_style },
                interactive: None,
            },
        ];
        paint(&mut s, &fragments, &HashMap::new(), Color::WHITE);
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "white text over a near-white BOX background (not the canvas) must still be repaired");
    }

    #[test]
    fn paint_keeps_white_text_white_over_a_dark_opaque_box() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let dark_box = box_style(Color::rgb(10, 10, 10), BorderSide::default());
        let text_style = ComputedStyle { color: Color::WHITE, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 20.0, 20.0), kind: FragmentKind::Box { style: dark_box }, interactive: None },
            Fragment {
                rect: rect(0.0, 0.0, 16.0, 16.0),
                kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style: text_style },
                interactive: None,
            },
        ];
        paint(&mut s, &fragments, &HashMap::new(), Color::WHITE);
        // The box fills the whole 20x20 surface, so any white pixel found
        // can only be glyph ink (never leftover canvas) -- proves the
        // already-legible white-on-dark text passed through unrepaired.
        let count_white_ink = s.bytes().chunks(4).filter(|p| p == &[255, 255, 255, 255]).count();
        assert!(count_white_ink > 0, "white text already legible over a dark box must render unchanged");
    }
}
