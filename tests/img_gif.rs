//! GIF decoder (P4) tests — animated GIF is non-negotiable (build brief §4).
//! Fixtures are built in-test via the `gif` crate's own encoder with exact
//! indexed pixels (no NeuQuant lossiness) so composited output is checkable
//! pixel-exact.

use stele::img::gif::GifDecoder;
use stele::img::{Decode, DecodeError};

const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];
const RED: [u8; 4] = [220, 20, 20, 255];
const GREEN: [u8; 4] = [20, 220, 20, 255];
const BLUE: [u8; 4] = [20, 20, 220, 255];

/// A 4x4 3-frame animated GIF exercising:
///   frame A: full-canvas opaque red, dispose = Background, delay 5 (50ms)
///   frame B: 2x2 sub-rect at (1,1), 3 opaque green px + 1 transparent px,
///            dispose = Previous, delay 7 (70ms)
///   frame C: bottom-half (rows 2..4) opaque blue, dispose = Keep, delay 9 (90ms)
fn build_fixture() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder =
            gif::Encoder::new(&mut bytes, 4, 4, &[0, 0, 0]).expect("create GIF encoder");
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .expect("set repeat");

        let mut frame_a =
            gif::Frame::from_palette_pixels(4, 4, vec![0u8; 16], vec![220, 20, 20], None);
        frame_a.dispose = gif::DisposalMethod::Background;
        frame_a.delay = 5;
        encoder.write_frame(&frame_a).expect("write frame A");

        let mut frame_b = gif::Frame::from_palette_pixels(
            2,
            2,
            vec![0u8, 0, 1, 0],
            vec![20, 220, 20, /* green */ 0, 0, 0 /* unused/transparent */],
            Some(1),
        );
        frame_b.left = 1;
        frame_b.top = 1;
        frame_b.dispose = gif::DisposalMethod::Previous;
        frame_b.delay = 7;
        encoder.write_frame(&frame_b).expect("write frame B");

        let mut frame_c =
            gif::Frame::from_palette_pixels(4, 2, vec![0u8; 8], vec![20, 20, 220], None);
        frame_c.top = 2;
        frame_c.dispose = gif::DisposalMethod::Keep;
        frame_c.delay = 9;
        encoder.write_frame(&frame_c).expect("write frame C");
    }
    bytes
}

fn flatten(image: &stele::img::RgbaImage) -> Vec<[u8; 4]> {
    image
        .pixels
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect()
}

#[test]
fn decodes_all_frames_with_delays_in_milliseconds() {
    let bytes = build_fixture();
    let frames = GifDecoder.decode(&bytes).expect("decode must succeed");
    assert_eq!(frames.len(), 3, "must decode every animation frame");
    assert_eq!(frames[0].delay_ms, 50);
    assert_eq!(frames[1].delay_ms, 70);
    assert_eq!(frames[2].delay_ms, 90);
    for f in &frames {
        assert_eq!(f.image.width, 4);
        assert_eq!(f.image.height, 4);
    }
}

#[test]
fn frame_a_is_full_canvas_opaque_red() {
    let bytes = build_fixture();
    let frames = GifDecoder.decode(&bytes).expect("decode must succeed");
    let flat = flatten(&frames[0].image);
    for p in &flat {
        assert_eq!(*p, RED);
    }
}

#[test]
fn frame_b_composites_background_disposal_then_transparency() {
    // Frame A disposes to Background before B is drawn, so B is composited
    // onto a cleared (transparent) canvas: 3 green px + 1 still-transparent px.
    let bytes = build_fixture();
    let frames = GifDecoder.decode(&bytes).expect("decode must succeed");
    let flat = flatten(&frames[1].image);
    // Row-major 4x4: index = y*4+x
    let get = |x: usize, y: usize| flat[y * 4 + x];

    for y in 0..4 {
        for x in 0..4 {
            let expected = match (x, y) {
                (1, 1) => GREEN,
                (2, 1) => GREEN,
                (2, 2) => GREEN,
                (1, 2) => TRANSPARENT, // this sub-pixel was the transparent index
                _ => TRANSPARENT,      // rest of canvas: cleared by A's Background dispose
            };
            assert_eq!(get(x, y), expected, "mismatch at ({x},{y})");
        }
    }
}

#[test]
fn frame_c_restores_previous_disposal_then_draws_bottom_half() {
    // Frame B disposes to Previous, restoring the canvas to its state from
    // right before B was drawn (fully transparent, post-A's Background
    // disposal) before C is composited.
    let bytes = build_fixture();
    let frames = GifDecoder.decode(&bytes).expect("decode must succeed");
    let flat = flatten(&frames[2].image);
    let get = |x: usize, y: usize| flat[y * 4 + x];

    for y in 0..2 {
        for x in 0..4 {
            assert_eq!(get(x, y), TRANSPARENT, "top half must stay transparent at ({x},{y})");
        }
    }
    for y in 2..4 {
        for x in 0..4 {
            assert_eq!(get(x, y), BLUE, "bottom half must be blue at ({x},{y})");
        }
    }
}

#[test]
fn single_frame_gif_yields_one_frame() {
    let mut bytes = Vec::new();
    {
        let mut encoder =
            gif::Encoder::new(&mut bytes, 2, 2, &[0, 0, 0]).expect("create GIF encoder");
        let frame = gif::Frame::from_palette_pixels(2, 2, vec![0u8; 4], vec![1, 2, 3], None);
        encoder.write_frame(&frame).expect("write frame");
    }
    let frames = GifDecoder.decode(&bytes).expect("decode must succeed");
    assert_eq!(frames.len(), 1);
}

#[test]
fn wrong_magic_is_not_this_format() {
    let result = GifDecoder.decode(b"\x89PNG\r\n\x1a\nnot a gif");
    assert!(matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn empty_bytes_are_not_this_format() {
    let result = GifDecoder.decode(&[]);
    assert!(matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn truncated_after_magic_is_malformed_not_a_panic() {
    let result = GifDecoder.decode(b"GIF89a");
    assert!(result.is_err());
    assert!(!matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn truncated_mid_stream_is_malformed_not_a_panic() {
    let bytes = build_fixture();
    let truncated = &bytes[..bytes.len() - 20];
    let result = GifDecoder.decode(truncated);
    assert!(result.is_err());
}

#[test]
fn garbage_after_valid_magic_never_panics() {
    let mut garbage = b"GIF89a".to_vec();
    garbage.extend_from_slice(&[0xFF; 64]);
    let result = GifDecoder.decode(&garbage);
    assert!(result.is_err());
}

#[test]
fn oversized_canvas_is_rejected_as_unsupported_before_any_frame_is_read() {
    // 9000x9000 = 81,000,000 px > the 64,000,000px decode cap. The Logical
    // Screen Descriptor alone declares this; the actual frame is a trivial
    // 1x1 pixel, so this stays a tiny fixture. Must be rejected right after
    // `read_info` (header-only), never attempting to allocate a full
    // 9000x9000 RGBA canvas.
    let mut bytes = Vec::new();
    {
        let mut encoder =
            gif::Encoder::new(&mut bytes, 9000, 9000, &[0, 0, 0]).expect("create GIF encoder");
        let frame = gif::Frame::from_palette_pixels(1, 1, vec![0u8], vec![1, 2, 3], None);
        encoder.write_frame(&frame).expect("write frame");
    }
    let result = GifDecoder.decode(&bytes);
    assert!(
        matches!(result, Err(DecodeError::Unsupported(_))),
        "expected Unsupported for an over-cap canvas, got {result:?}"
    );
}

#[test]
fn legitimate_large_image_under_our_cap_is_not_rejected_by_gifs_stricter_default_limit() {
    // Regression: the `gif` crate defaults to a 50MB-per-frame memory limit
    // (~12.5M px at 4 bytes/px RGBA output), independent of and stricter
    // than our own 64M px `check_pixel_cap`. Before explicitly raising the
    // crate's limit to match our budget, a legitimate image comfortably
    // under our advertised cap (13.2M px here) was silently rejected by the
    // crate's own default instead. This canvas is fully covered by one
    // opaque frame, so the encoded GIF still compresses to a tiny fixture.
    let width: u16 = 4000;
    let height: u16 = 3300; // 13,200,000 px: > gif's default ~12.5M px limit, < our 64M px cap.
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, width, height, &[0, 0, 0])
            .expect("create GIF encoder");
        let pixels = vec![0u8; (width as usize) * (height as usize)];
        let frame =
            gif::Frame::from_palette_pixels(width, height, pixels, vec![200, 100, 50], None);
        encoder.write_frame(&frame).expect("write frame");
    }

    let frames = GifDecoder
        .decode(&bytes)
        .expect("an image under our own pixel cap must not be rejected");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].image.width, u32::from(width));
    assert_eq!(frames[0].image.height, u32::from(height));
    // Spot-check a pixel rather than the full 52MB buffer.
    assert_eq!(&frames[0].image.pixels[0..4], &[200, 100, 50, 255]);
}
