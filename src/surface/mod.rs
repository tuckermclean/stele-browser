//! The drawing surface: one trait, three eventual backends.
//!
//! `Surface` is the only seam the backends share (brief §10: P7 tty, P9 fb, and
//! the in-memory golden surface coordinate through this and nothing else). The
//! renderer emits paint ops against `&mut dyn Surface`; each backend maps them
//! to pixels (fb, mem) or an approximation in cells (tty).

pub mod mem;

pub use mem::MemSurface;

/// A straight-alpha RGBA color — the paint primitive shared by style and paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255, a: 255 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }
}

/// An integer pixel rectangle in surface space (origin top-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// A positioned run of text to paint. The inline engine has already placed it;
/// the backend renders `text` starting at `x`, sitting on `baseline`, using the
/// font metrics from the `text` module. Firm-up (font handle, per-glyph offsets)
/// arrives with P5/P7.
#[derive(Debug, Clone, Copy)]
pub struct TextRun<'a> {
    pub text: &'a str,
    pub x: i32,
    pub baseline: i32,
    pub size_px: f32,
    pub color: Color,
}

/// The paint target. Backends implement it; the renderer only ever holds
/// `&mut dyn Surface`.
pub trait Surface {
    /// Pixel dimensions of the drawable area.
    fn size(&self) -> (u32, u32);

    /// Set a single pixel (alpha-blended over what is there).
    fn put_pixel(&mut self, x: i32, y: i32, color: Color);

    /// Fill a rectangle with a solid color (alpha-blended).
    fn fill_rect(&mut self, rect: Rect, color: Color);

    /// Copy a decoded image into the surface at `at` (scaled to `at`'s size).
    fn blit(&mut self, at: Rect, image: &crate::img::RgbaImage);

    /// Paint a positioned run of text.
    fn draw_text(&mut self, run: &TextRun);

    /// Set the current clip rectangle (in surface pixels); `None` clears it.
    /// Drawing outside the clip is suppressed. Default: no-op (backends that
    /// don't clip, e.g. tty, ignore it).
    fn set_clip(&mut self, _clip: Option<Rect>) {}
}
