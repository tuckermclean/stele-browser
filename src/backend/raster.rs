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
//! - `FragmentKind::Image { .. }`: skipped. No M4 fixture emits a real
//!   `Image` fragment yet (today's block-flow pipeline paints a `Replaced`
//!   box as a plain `Box`, per `layout::block`'s own doc comment), and
//!   `MemSurface::blit` is still `todo!()` — wiring real image pixels is the
//!   NEXT packet's job. `// TODO(images packet): blit`.
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

use crate::layout::{Fragment, FragmentKind, Rect as LayoutRect};
use crate::style::computed::{BorderSide, BorderStyle, ComputedStyle};
use crate::surface::{Color, MemSurface, Rect as PixelRect, Surface, TextRun};

/// Paint-ordered `fragments` onto `surface`, painter's-algorithm style
/// (later fragments draw over earlier ones — already the paint order
/// `layout::layout` produces, same contract `backend::tty::render` trusts).
pub fn paint(surface: &mut dyn Surface, fragments: &[Fragment]) {
    for fragment in fragments {
        match &fragment.kind {
            FragmentKind::Box { style } => paint_box(surface, &fragment.rect, style),
            FragmentKind::Text { text, baseline, style } => paint_text(surface, &fragment.rect, text, *baseline, style),
            FragmentKind::Image { .. } => {
                // TODO(images packet): blit `image` into `fragment.rect`
                // once MemSurface::blit is real (see this module's docs).
            }
        }
    }
}

fn paint_box(surface: &mut dyn Surface, rect: &LayoutRect, style: &ComputedStyle) {
    let prect = to_pixel_rect(rect);
    if style.background_color.a != 0 {
        surface.fill_rect(prect, style.background_color);
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
        let fragments = vec![Fragment { rect: rect(2.0, 2.0, 4.0, 4.0), kind: FragmentKind::Box { style } }];
        paint(&mut s, &fragments);
        assert_eq!(px(&s, 3, 3), Color::rgb(10, 20, 30));
        assert_eq!(px(&s, 0, 0), Color::WHITE, "outside the box stays background");
    }

    #[test]
    fn transparent_background_paints_nothing() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = box_style(Color::TRANSPARENT, BorderSide::default());
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style } }];
        paint(&mut s, &fragments);
        assert_eq!(px(&s, 5, 5), Color::WHITE);
    }

    #[test]
    fn solid_border_paints_all_four_edges_in_the_border_color() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let border = BorderSide { width: 2.0, style: BorderStyle::Solid, color: Color::rgb(255, 0, 0) };
        let style = box_style(Color::TRANSPARENT, border);
        let fragments = vec![Fragment { rect: rect(2.0, 2.0, 6.0, 6.0), kind: FragmentKind::Box { style } }];
        paint(&mut s, &fragments);
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
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style } }];
        paint(&mut s, &fragments);
        assert_eq!(px(&s, 0, 0), Color::WHITE);
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
        let fragments = vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style } }];
        paint(&mut s, &fragments);
        assert_eq!(px(&s, 0, 0), Color::WHITE);
    }

    // ------------------------------------------------------------ paint: Text

    #[test]
    fn text_fragment_draws_ink_at_the_expected_position() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let style = ComputedStyle { color: Color::BLACK, font_size: 16.0, ..ComputedStyle::default() };
        let fragments = vec![Fragment {
            rect: rect(4.0, 0.0, 8.0, 16.0),
            kind: FragmentKind::Text { text: "A".to_string(), baseline: 12.0, style },
        }];
        paint(&mut s, &fragments);
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "expected some glyph ink to be painted");
    }

    #[test]
    fn empty_text_fragment_paints_nothing() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let style = ComputedStyle::default();
        let fragments =
            vec![Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Text { text: String::new(), baseline: 8.0, style } }];
        paint(&mut s, &fragments);
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    // ----------------------------------------------------------- paint: Image

    #[test]
    fn image_fragment_is_skipped_not_blitted() {
        // MemSurface::blit is still todo!() (next packet) -- if `paint`
        // called it, this test would panic. Skipping it is the documented
        // M4 scope call.
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let fragments =
            vec![Fragment { rect: rect(0.0, 0.0, 4.0, 4.0), kind: FragmentKind::Image { image: RgbaImage::new(2, 2) } }];
        paint(&mut s, &fragments); // must not panic
        assert_eq!(px(&s, 0, 0), Color::WHITE);
    }

    // -------------------------------------------------------------- ordering

    #[test]
    fn paint_order_wins_ties_like_the_tty_backend() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        let first = box_style(Color::rgb(255, 0, 0), BorderSide::default());
        let second = box_style(Color::rgb(0, 255, 0), BorderSide::default());
        let fragments = vec![
            Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style: first } },
            Fragment { rect: rect(0.0, 0.0, 10.0, 10.0), kind: FragmentKind::Box { style: second } },
        ];
        paint(&mut s, &fragments);
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
                Fragment { rect: rect(x, y, w, h), kind: FragmentKind::Box { style } },
                Fragment { rect: rect(x, y, w, h), kind: FragmentKind::Text { text: "z".to_string(), baseline: f32::NAN, style: text_style } },
                Fragment { rect: rect(x, y, w, h), kind: FragmentKind::Image { image: RgbaImage::new(1, 1) } },
            ];
            paint(&mut s, &fragments); // must not panic
        }
    }

    #[test]
    fn empty_fragment_list_paints_nothing() {
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        paint(&mut s, &[]);
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
}
