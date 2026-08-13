//! JPEG decoder (P4) tests. `jpeg-decoder` is decode-only, so fixtures are
//! small checked-in JPEGs (fixtures/p4-baseline.jpg, fixtures/p4-progressive.jpg
//! — a 16x16 image, left half red-ish, right half blue-ish, encoded once as
//! baseline and once as progressive DCT). JPEG is lossy: assert dimensions
//! and approximate sampled colors, not exact bytes.

use stele::img::jpeg::JpegDecoder;
use stele::img::{Decode, DecodeError};

const BASELINE_JPG: &[u8] = include_bytes!("../fixtures/p4-baseline.jpg");
const PROGRESSIVE_JPG: &[u8] = include_bytes!("../fixtures/p4-progressive.jpg");
// A genuine small CMYK JPEG (Pillow-encoded 8x8, `Image.new("CMYK", ...)`),
// so the CMYK32 `Unsupported` arm is exercised against a real decode rather
// than an early-exit on truncated bytes.
const CMYK_JPG: &[u8] = include_bytes!("../fixtures/p4-cmyk.jpg");

fn approx(actual: u8, expected: u8, tolerance: i16) {
    let diff = (actual as i16 - expected as i16).abs();
    assert!(
        diff <= tolerance,
        "expected ~{expected}, got {actual} (tolerance {tolerance})"
    );
}

#[test]
fn decodes_baseline_jpeg_dimensions_and_regions() {
    let frames = JpegDecoder.decode(BASELINE_JPG).expect("baseline JPEG must decode");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].delay_ms, 0);
    let img = &frames[0].image;
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 16);
    assert_eq!(img.pixels.len(), 16 * 16 * 4);

    // Sample the left region (encoded red-ish) and right region (blue-ish).
    let left_idx = ((8 * 16 + 2) * 4) as usize;
    let right_idx = ((8 * 16 + 13) * 4) as usize;
    approx(img.pixels[left_idx], 220, 30);
    approx(img.pixels[left_idx + 1], 20, 30);
    approx(img.pixels[left_idx + 2], 20, 30);
    approx(img.pixels[left_idx + 3], 255, 0);

    approx(img.pixels[right_idx], 20, 30);
    approx(img.pixels[right_idx + 1], 20, 30);
    approx(img.pixels[right_idx + 2], 220, 30);
    approx(img.pixels[right_idx + 3], 255, 0);
}

#[test]
fn decodes_progressive_jpeg_dimensions_and_regions() {
    let frames = JpegDecoder
        .decode(PROGRESSIVE_JPG)
        .expect("progressive JPEG must decode");
    assert_eq!(frames.len(), 1);
    let img = &frames[0].image;
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 16);

    let left_idx = ((8 * 16 + 2) * 4) as usize;
    let right_idx = ((8 * 16 + 13) * 4) as usize;
    approx(img.pixels[left_idx], 220, 30);
    approx(img.pixels[left_idx + 2], 20, 30);
    approx(img.pixels[right_idx], 20, 30);
    approx(img.pixels[right_idx + 2], 220, 30);
}

#[test]
fn wrong_magic_is_not_this_format() {
    let result = JpegDecoder.decode(b"GIF89a not a jpeg");
    assert!(matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn empty_bytes_are_not_this_format() {
    let result = JpegDecoder.decode(&[]);
    assert!(matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn truncated_after_magic_is_malformed_not_a_panic() {
    let result = JpegDecoder.decode(&[0xFF, 0xD8, 0xFF]);
    assert!(result.is_err());
    assert!(!matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn truncated_mid_stream_is_malformed_not_a_panic() {
    let truncated = &BASELINE_JPG[..BASELINE_JPG.len() / 2];
    let result = JpegDecoder.decode(truncated);
    assert!(result.is_err());
}

#[test]
fn garbage_after_valid_magic_never_panics() {
    let mut garbage = vec![0xFF, 0xD8, 0xFF];
    garbage.extend_from_slice(&[0x00; 128]);
    let result = JpegDecoder.decode(&garbage);
    assert!(result.is_err());
}

/// Builds a truncated **progressive** (SOF2) JPEG that only contains a frame
/// header declaring `width`x`height` — no Huffman tables, no scan data.
/// `jpeg_decoder::Decoder::read_info` parses just far enough to learn the
/// declared dimensions without allocating anything proportional to them;
/// `decode()` would go on to read further markers (and, for a real
/// progressive stream, allocate a full coefficient buffer sized off those
/// dimensions) before ever hitting our pixel cap.
fn truncated_progressive_sof_claiming(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8]; // SOI
    bytes.push(0xFF);
    bytes.push(0xC2); // SOF2: progressive DCT, Huffman
    let len: u16 = 2 + 1 + 2 + 2 + 1 + 3; // length field + precision + h + w + ncomp + 1 component
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.push(8); // sample precision
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.push(1); // 1 component (grayscale)
    bytes.push(1); // component id
    bytes.push(0x11); // sampling factors
    bytes.push(0); // quant table selector
    bytes
}

#[test]
fn oversized_progressive_frame_header_is_rejected_before_decode_allocates() {
    // CRITICAL regression: 12000x12000 = 144,000,000 px > our 64,000,000px
    // cap. Before reordering to `read_info` -> cap check -> `decode`, this
    // truncated-right-after-SOF fixture made `decode()` run first and fail
    // on the missing Huffman tables/scan data (an IO/Format error, mapped to
    // `Malformed`) — the cap check that should have rejected it outright
    // never ran. A real (non-truncated) file at these dimensions would have
    // let `decode()` allocate multi-hundred-MB coefficient buffers first,
    // an uncatchable OOM abort on our `panic = "abort"` target. With the fix,
    // `read_info` alone is enough to learn the declared dimensions and reject
    // via the pixel cap without ever calling `decode()`.
    let bytes = truncated_progressive_sof_claiming(12000, 12000);
    let result = JpegDecoder.decode(&bytes);
    assert!(
        matches!(result, Err(DecodeError::Unsupported(_))),
        "expected Unsupported for an over-cap declared frame size, got {result:?}"
    );
}

#[test]
fn cmyk_jpeg_is_unsupported_not_silently_wrong_colors() {
    // See img/jpeg.rs doc comment: real-world CMYK JPEGs are usually
    // Adobe-inverted and this crate doesn't expose the APP14 marker needed
    // to detect that, so we refuse rather than guess.
    //
    // (A 16-bit-per-sample (`PixelFormat::L16`) fixture would exercise the
    // adjacent `Unsupported` arm, but that's a lossless-JPEG-only pixel
    // depth with no practical encoder available here (Pillow can't produce
    // one); left uncovered by a fixture rather than forcing something
    // flaky — the arm itself is a direct, obviously-correct `Unsupported`
    // return with no logic to regress.)
    let result = JpegDecoder.decode(CMYK_JPG);
    assert!(
        matches!(result, Err(DecodeError::Unsupported(_))),
        "expected Unsupported for CMYK, got {result:?}"
    );
}
