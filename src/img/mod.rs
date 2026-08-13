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

pub mod gif;
pub mod jpeg;
pub mod png;

/// Resource-abuse guard shared by every decoder: reject decoded images whose
/// pixel count would risk an oversized allocation on a 486-class machine,
/// rather than letting a hostile/malformed `width`/`height` pair try to
/// allocate gigabytes. 64M pixels is comfortably above any real document-web
/// image while still catching decompression-bomb-style dimensions.
pub(crate) const MAX_DECODE_PIXELS: u64 = 64_000_000;

/// Checked `width * height` against [`MAX_DECODE_PIXELS`], also rejecting the
/// degenerate zero-sized case. Shared by the PNG/JPEG/GIF decoders.
pub(crate) fn check_pixel_cap(width: u32, height: u32) -> Result<(), DecodeError> {
    if width == 0 || height == 0 {
        return Err(DecodeError::Malformed(format!(
            "zero-sized image ({width}x{height})"
        )));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_DECODE_PIXELS {
        return Err(DecodeError::Unsupported(format!(
            "image dimensions {width}x{height} ({pixels} px) exceed the \
             {MAX_DECODE_PIXELS}px decode cap"
        )));
    }
    Ok(())
}

/// Which of the three Wave-1 formats a byte stream is, per magic numbers
/// (brief §4: GIF incl. animated, JPEG baseline+progressive, PNG).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Gif,
    Jpeg,
    Png,
}

/// Sniff `bytes` by magic number. `None` if none of the three formats match.
pub fn sniff_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(ImageFormat::Png)
    } else {
        None
    }
}

fn decode_as(format: ImageFormat, bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
    match format {
        ImageFormat::Gif => gif::GifDecoder.decode(bytes),
        ImageFormat::Jpeg => jpeg::JpegDecoder.decode(bytes),
        ImageFormat::Png => png::PngDecoder.decode(bytes),
    }
}

/// Decode `bytes` into frames, picking a decoder by `content_type` hint
/// (e.g. an HTTP `Content-Type` header) when present, else by magic-byte
/// sniffing. A hint that turns out to be wrong (the bytes are not actually
/// that format) falls back to sniffing rather than failing outright — servers
/// lie about content types constantly, and this is 1996-web software.
pub fn decode_bytes(bytes: &[u8], content_type: Option<&str>) -> Result<Vec<Frame>, DecodeError> {
    let hinted = content_type.and_then(|ct| {
        let ct = ct.split(';').next().unwrap_or(ct).trim().to_ascii_lowercase();
        match ct.as_str() {
            "image/gif" => Some(ImageFormat::Gif),
            "image/jpeg" | "image/jpg" | "image/pjpeg" => Some(ImageFormat::Jpeg),
            "image/png" => Some(ImageFormat::Png),
            _ => None,
        }
    });

    if let Some(format) = hinted {
        match decode_as(format, bytes) {
            Err(DecodeError::NotThisFormat) => { /* hint was wrong; fall through to sniff */ }
            result => return result,
        }
    }

    match sniff_format(bytes) {
        Some(format) => decode_as(format, bytes),
        None => Err(DecodeError::Malformed(
            "unrecognized image format (bad magic bytes)".to_string(),
        )),
    }
}
