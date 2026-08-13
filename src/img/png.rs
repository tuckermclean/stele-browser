//! PNG decoder (P4) behind the frozen [`Decode`] trait — see `img/mod.rs`.
//!
//! Uses the `png` crate with `Transformations::EXPAND | STRIP_16 | ALPHA`,
//! which normalizes every PNG color type (RGB, RGBA, grayscale,
//! grayscale+alpha, palette incl. `tRNS`) and bit depth down to exactly two
//! possible decoded shapes: 8-bit grayscale+alpha or 8-bit RGBA (see
//! `Reader::output_color_type` in the `png` crate for why only those two
//! survive with `ALPHA` set). Both are normalized here to straight-alpha
//! RGBA8. PNG's alpha is already straight (not premultiplied), matching
//! [`RgbaImage`]'s contract.
//!
//! APNG animation chunks (`acTL`/`fcTL`/`fdAT`), if present, are ignored —
//! only the default (first) image is decoded, always as one [`Frame`] with
//! `delay_ms == 0`. Full APNG playback is out of brief §4's scope for P4.

use super::{check_pixel_cap, Decode, DecodeError, Frame, RgbaImage};

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// PNG decoder using the `png` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct PngDecoder;

impl Decode for PngDecoder {
    fn decode(&self, bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
        if bytes.len() < PNG_MAGIC.len() || bytes[..PNG_MAGIC.len()] != PNG_MAGIC {
            return Err(DecodeError::NotThisFormat);
        }

        let mut decoder = png::Decoder::new(bytes);
        decoder.set_transformations(
            png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
        );

        let mut reader = decoder.read_info().map_err(map_png_error)?;

        let (width, height) = reader.info().size();
        check_pixel_cap(width, height)?;

        let mut buf = vec![0u8; reader.output_buffer_size()];
        let output_info = reader.next_frame(&mut buf).map_err(map_png_error)?;
        let (color_type, _bit_depth) = reader.output_color_type();
        buf.truncate(output_info.buffer_size());

        let pixels = match color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::GrayscaleAlpha => {
                let mut out = Vec::with_capacity(buf.len() * 2);
                for px in buf.chunks_exact(2) {
                    let (gray, alpha) = (px[0], px[1]);
                    out.extend_from_slice(&[gray, gray, gray, alpha]);
                }
                out
            }
            other => {
                return Err(DecodeError::Unsupported(format!(
                    "PNG color type {other:?} after normalization is not \
                     RGBA/GrayscaleAlpha (unexpected — please file a bug)"
                )));
            }
        };

        let image = RgbaImage {
            width: output_info.width,
            height: output_info.height,
            pixels,
        };
        Ok(vec![Frame {
            image,
            delay_ms: 0,
        }])
    }
}

fn map_png_error(err: png::DecodingError) -> DecodeError {
    DecodeError::Malformed(err.to_string())
}
