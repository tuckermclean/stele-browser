//! GIF decoder (P4) behind the frozen [`Decode`] trait — see `img/mod.rs`.
//!
//! Animated GIF is non-negotiable per brief §4 ("it is 1996"): every frame is
//! decoded and composited onto a running full-canvas RGBA buffer honoring the
//! GIF89a disposal methods, so each returned [`Frame`] is a complete,
//! ready-to-blit image — never a raw sub-rect delta. Per-frame delay
//! (centiseconds in the format) is converted to milliseconds.
//!
//! ## Compositing algorithm
//! The `gif` crate hands back each frame's *own* sub-rectangle (its
//! `left`/`top`/`width`/`height` and RGBA pixels — [`gif::ColorOutput::RGBA`]
//! is requested so transparency is already baked into alpha, no manual
//! palette/`tRNS` lookups needed). This decoder maintains a `canvas` the size
//! of the logical screen and, for each frame in order:
//!
//! 1. Applies the *previous* frame's disposal method (if any) to prepare the
//!    canvas: [`DisposalMethod::Background`] clears the previous frame's
//!    rectangle to transparent; [`DisposalMethod::Previous`] restores the
//!    canvas to a snapshot taken right before that previous frame was drawn;
//!    [`DisposalMethod::Keep`]/[`DisposalMethod::Any`] leave the canvas as-is
//!    (matching how browsers treat "no disposal specified").
//! 2. If *this* frame's own disposal is `Previous`, snapshots the canvas now
//!    (before drawing), so step 1 has something to restore to once the next
//!    frame is processed.
//! 3. Composites the frame's pixels onto the canvas at its rectangle. GIF
//!    transparency is binary (no partial alpha): a source pixel with alpha 0
//!    leaves the underlying canvas pixel untouched; anything else replaces
//!    it outright.
//! 4. Clones the canvas as this frame's output image.
//!
//! Still (non-animated) GIFs are just the one-frame case of the same loop.

use std::num::NonZeroU64;

use super::{check_pixel_cap, Decode, DecodeError, Frame, RgbaImage, MAX_DECODE_PIXELS};

const GIF_MAGIC_87A: &[u8] = b"GIF87a";
const GIF_MAGIC_89A: &[u8] = b"GIF89a";

/// GIF decoder using the `gif` crate, compositing every frame onto a
/// full-canvas buffer per the disposal-method rules above.
#[derive(Debug, Default, Clone, Copy)]
pub struct GifDecoder;

impl Decode for GifDecoder {
    fn decode(&self, bytes: &[u8]) -> Result<Vec<Frame>, DecodeError> {
        if !(bytes.starts_with(GIF_MAGIC_87A) || bytes.starts_with(GIF_MAGIC_89A)) {
            return Err(DecodeError::NotThisFormat);
        }

        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        // The `gif` crate defaults to a 50MB-per-frame memory limit (~12.5M
        // px at 4 bytes/px), stricter than and independent of our own
        // `check_pixel_cap` (64M px). Left at its default, a legitimate
        // frame under our advertised cap could still be silently rejected
        // (as `Malformed`, from the crate's own "image is too large" error)
        // before `check_pixel_cap` ever runs. Raise it to match our budget
        // so `check_pixel_cap` — run below, right after `read_info`, before
        // any frame is decoded — is the one authoritative gate.
        let frame_byte_cap = NonZeroU64::new(MAX_DECODE_PIXELS * 4)
            .expect("MAX_DECODE_PIXELS * 4 is a nonzero constant");
        options.set_memory_limit(gif::MemoryLimit::Bytes(frame_byte_cap));
        let mut decoder = options.read_info(bytes).map_err(map_gif_error)?;

        let (canvas_width, canvas_height) =
            (u32::from(decoder.width()), u32::from(decoder.height()));
        check_pixel_cap(canvas_width, canvas_height)?;

        let mut canvas = vec![0u8; (canvas_width as usize) * (canvas_height as usize) * 4];
        let mut frames = Vec::new();

        // Disposal bookkeeping for the *previous* iteration's frame.
        let mut prev_dispose = gif::DisposalMethod::Keep;
        let mut prev_rect: Option<(u32, u32, u32, u32)> = None;
        let mut prev_snapshot: Option<Vec<u8>> = None;

        while let Some(frame) = decoder.read_next_frame().map_err(map_gif_error)? {
            if let Some((left, top, w, h)) = prev_rect {
                match prev_dispose {
                    gif::DisposalMethod::Background => {
                        clear_rect(&mut canvas, canvas_width, canvas_height, left, top, w, h);
                    }
                    gif::DisposalMethod::Previous => {
                        if let Some(snapshot) = prev_snapshot.take() {
                            canvas = snapshot;
                        }
                    }
                    gif::DisposalMethod::Keep | gif::DisposalMethod::Any => {}
                }
            }

            let (left, top, w, h) = (
                u32::from(frame.left),
                u32::from(frame.top),
                u32::from(frame.width),
                u32::from(frame.height),
            );

            prev_snapshot = if frame.dispose == gif::DisposalMethod::Previous {
                Some(canvas.clone())
            } else {
                None
            };

            composite_rect(
                &mut canvas,
                canvas_width,
                canvas_height,
                left,
                top,
                w,
                h,
                frame.buffer.as_ref(),
            );

            frames.push(Frame {
                image: RgbaImage {
                    width: canvas_width,
                    height: canvas_height,
                    pixels: canvas.clone(),
                },
                delay_ms: frame.delay.saturating_mul(10),
            });

            prev_dispose = frame.dispose;
            prev_rect = Some((left, top, w, h));
        }

        if frames.is_empty() {
            return Err(DecodeError::Malformed(
                "GIF stream contains no image frames".to_string(),
            ));
        }

        Ok(frames)
    }
}

/// Clears an axis-aligned rectangle of `canvas` (RGBA8, row-major,
/// `canvas_width x canvas_height`) to fully transparent black, clipping to
/// canvas bounds.
fn clear_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    left: u32,
    top: u32,
    w: u32,
    h: u32,
) {
    for y in 0..h {
        let cy = top + y;
        if cy >= canvas_height {
            break;
        }
        for x in 0..w {
            let cx = left + x;
            if cx >= canvas_width {
                break;
            }
            let idx = ((cy * canvas_width + cx) as usize) * 4;
            canvas[idx..idx + 4].copy_from_slice(&[0, 0, 0, 0]);
        }
    }
}

/// Composites `src` (RGBA8, row-major, `w x h`) onto `canvas` at
/// `(left, top)`, clipping to canvas bounds. GIF transparency is binary: a
/// zero-alpha source pixel leaves the destination untouched; any other pixel
/// replaces it outright.
#[allow(clippy::too_many_arguments)]
fn composite_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    left: u32,
    top: u32,
    w: u32,
    h: u32,
    src: &[u8],
) {
    for y in 0..h {
        let cy = top + y;
        if cy >= canvas_height {
            break;
        }
        for x in 0..w {
            let cx = left + x;
            if cx >= canvas_width {
                break;
            }
            let src_idx = ((y * w + x) as usize) * 4;
            if src_idx + 4 > src.len() {
                continue;
            }
            if src[src_idx + 3] == 0 {
                continue; // transparent: leave the canvas pixel as-is
            }
            let dst_idx = ((cy * canvas_width + cx) as usize) * 4;
            canvas[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
        }
    }
}

fn map_gif_error(err: gif::DecodingError) -> DecodeError {
    DecodeError::Malformed(err.to_string())
}
