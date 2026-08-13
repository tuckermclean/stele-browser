//! JPEG decoder (P4) behind the frozen [`Decode`] trait — see `img/mod.rs`.
//!
//! Uses `jpeg-decoder` (default-features off: no rayon, single-threaded per
//! brief §9's no-thread-pool stance). The crate transparently handles both
//! baseline and progressive DCT coding — brief §4 requires both, and this is
//! why `jpeg-decoder` rather than a baseline-only decoder was chosen (see
//! `Cargo.toml`).
//!
//! Output pixel formats from the crate: `L8` (grayscale), `RGB24`, and
//! `CMYK32` (covers both CMYK and YCCK — the crate already undoes the YCCK
//! color transform internally). `L8`/`RGB24` are expanded to opaque RGBA8
//! here (JPEG has no alpha channel). `CMYK32` is returned as
//! [`DecodeError::Unsupported`]: real-world CMYK JPEGs are usually Adobe
//! output with inverted channel values (signaled by an APP14 marker this
//! crate parses but does not expose), and guessing wrong would silently ship
//! inverted colors — worse than falling back to alt text per brief §6 L4.
//! JPEG yields exactly one [`Frame`] with `delay_ms == 0` (no animation).

use jpeg_decoder::PixelFormat;

use super::{check_pixel_cap, Decode, DecodeError, Frame, RgbaImage};

const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// JPEG decoder using the `jpeg-decoder` crate. Handles baseline AND
/// progressive DCT (the crate dispatches on the file's own coding process).
#[derive(Debug, Default, Clone, Copy)]
pub struct JpegDecoder;

impl Decode for JpegDecoder {
    fn decode(&self, bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
        if bytes.len() < JPEG_MAGIC.len() || bytes[..JPEG_MAGIC.len()] != JPEG_MAGIC {
            return Err(DecodeError::NotThisFormat);
        }

        let mut decoder = jpeg_decoder::Decoder::new(bytes);
        let pixels = decoder.decode().map_err(map_jpeg_error)?;
        let info = decoder.info().ok_or_else(|| {
            DecodeError::Malformed("JPEG decoded with no frame metadata available".to_string())
        })?;

        let (width, height) = (u32::from(info.width), u32::from(info.height));
        check_pixel_cap(width, height)?;

        let rgba = match info.pixel_format {
            PixelFormat::RGB24 => {
                if pixels.len() < (width as usize) * (height as usize) * 3 {
                    return Err(DecodeError::Malformed(
                        "JPEG RGB24 buffer shorter than width*height*3".to_string(),
                    ));
                }
                let mut out = Vec::with_capacity(pixels.len() / 3 * 4);
                for px in pixels.chunks_exact(3) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                out
            }
            PixelFormat::L8 => {
                let mut out = Vec::with_capacity(pixels.len() * 4);
                for &gray in &pixels {
                    out.extend_from_slice(&[gray, gray, gray, 255]);
                }
                out
            }
            PixelFormat::L16 => {
                return Err(DecodeError::Unsupported(
                    "16-bit-per-sample JPEG is not supported".to_string(),
                ));
            }
            PixelFormat::CMYK32 => {
                return Err(DecodeError::Unsupported(
                    "CMYK/YCCK JPEG is not supported (ambiguous Adobe channel \
                     inversion — falls back to alt text per brief §6 L4)"
                        .to_string(),
                ));
            }
        };

        let image = RgbaImage {
            width,
            height,
            pixels: rgba,
        };
        Ok(vec![Frame {
            image,
            delay_ms: 0,
        }])
    }
}

fn map_jpeg_error(err: jpeg_decoder::Error) -> DecodeError {
    match err {
        jpeg_decoder::Error::Unsupported(feature) => {
            DecodeError::Unsupported(format!("{feature:?}"))
        }
        jpeg_decoder::Error::Format(msg) => DecodeError::Malformed(msg),
        jpeg_decoder::Error::Io(io_err) => DecodeError::Malformed(io_err.to_string()),
        jpeg_decoder::Error::Internal(err) => DecodeError::Malformed(err.to_string()),
    }
}
