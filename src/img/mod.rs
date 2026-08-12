//! Image decoding behind one trait. P4 (Wave 1) implements GIF (incl. animated
//! — non-negotiable, it is 1996), JPEG (baseline + progressive), and PNG behind
//! [`Decode`], each vetted to build for the i486 target.

/// A decoded, straight-alpha RGBA8 image.
#[derive(Debug, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32) -> Self {
        RgbaImage {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        }
    }
}

/// One frame of a (possibly animated) image. A still image decodes to exactly
/// one frame with `delay_ms == 0`.
#[derive(Debug, Clone)]
pub struct Frame {
    pub image: RgbaImage,
    /// Inter-frame delay for animation (GIF); 0 for stills.
    pub delay_ms: u16,
}

/// Decode encoded bytes into one or more frames. One decoder per format
/// implements this; the image pipeline picks by sniffing/`Content-Type`.
pub trait Decode {
    /// Decode all frames. Animated GIF yields many; JPEG/PNG yield one.
    fn decode(&self, bytes: &[u8]) -> Result<Vec<Frame>, DecodeError>;
}

#[derive(Debug, Clone)]
pub enum DecodeError {
    /// The bytes are not this decoder's format.
    NotThisFormat,
    /// The bytes are this format but malformed.
    Malformed(String),
    /// The format is recognized but a needed feature is unimplemented; the
    /// caller should fall back to `alt` text (brief §6, L4).
    Unsupported(String),
}
