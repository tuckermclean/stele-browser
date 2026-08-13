//! JPEG decoder (P4) tests. `jpeg-decoder` is decode-only, so fixtures are
//! small checked-in JPEGs (fixtures/p4-baseline.jpg, fixtures/p4-progressive.jpg
//! — a 16x16 image, left half red-ish, right half blue-ish, encoded once as
//! baseline and once as progressive DCT). JPEG is lossy: assert dimensions
//! and approximate sampled colors, not exact bytes.

use stele::img::jpeg::JpegDecoder;
use stele::img::{Decode, DecodeError};

const BASELINE_JPG: &[u8] = include_bytes!("../fixtures/p4-baseline.jpg");
const PROGRESSIVE_JPG: &[u8] = include_bytes!("../fixtures/p4-progressive.jpg");

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
