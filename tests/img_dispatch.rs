//! `img::decode_bytes` dispatcher tests (P4): magic-byte sniffing, content-type
//! hint routing, and hint-vs-bytes mismatch fallback.

use stele::img::DecodeError;

const BASELINE_JPG: &[u8] = include_bytes!("../fixtures/p4-baseline.jpg");

fn tiny_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[10, 20, 30, 255]).unwrap();
    }
    bytes
}

fn tiny_gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[0, 0, 0]).unwrap();
        let frame = gif::Frame::from_palette_pixels(1, 1, vec![0u8], vec![9, 8, 7], None);
        encoder.write_frame(&frame).unwrap();
    }
    bytes
}

#[test]
fn sniffs_png_by_magic_with_no_hint() {
    let bytes = tiny_png();
    let frames = stele::img::decode_bytes(&bytes, None).expect("must decode by magic");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].image.pixels, vec![10, 20, 30, 255]);
}

#[test]
fn sniffs_gif_by_magic_with_no_hint() {
    let bytes = tiny_gif();
    let frames = stele::img::decode_bytes(&bytes, None).expect("must decode by magic");
    assert_eq!(frames.len(), 1);
}

#[test]
fn sniffs_jpeg_by_magic_with_no_hint() {
    let frames = stele::img::decode_bytes(BASELINE_JPG, None).expect("must decode by magic");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].image.width, 16);
}

#[test]
fn honors_correct_content_type_hint() {
    let bytes = tiny_png();
    let frames = stele::img::decode_bytes(&bytes, Some("image/png"))
        .expect("must decode using the hint");
    assert_eq!(frames.len(), 1);
}

#[test]
fn honors_content_type_hint_with_charset_parameter() {
    let bytes = tiny_png();
    let frames = stele::img::decode_bytes(&bytes, Some("image/png; charset=binary"))
        .expect("must decode using the hint, ignoring parameters");
    assert_eq!(frames.len(), 1);
}

#[test]
fn wrong_content_type_hint_falls_back_to_sniffing() {
    // Bytes are actually a PNG, but the hint claims GIF — sniffing must win.
    let bytes = tiny_png();
    let frames = stele::img::decode_bytes(&bytes, Some("image/gif"))
        .expect("must fall back to sniffing when the hint is wrong");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].image.pixels, vec![10, 20, 30, 255]);
}

#[test]
fn unknown_content_type_hint_falls_back_to_sniffing() {
    let bytes = tiny_gif();
    let frames = stele::img::decode_bytes(&bytes, Some("application/octet-stream"))
        .expect("must fall back to sniffing for an unrecognized hint");
    assert_eq!(frames.len(), 1);
}

#[test]
fn unrecognized_bytes_are_an_error_not_a_panic() {
    let result = stele::img::decode_bytes(b"not an image at all", None);
    assert!(result.is_err());
}

#[test]
fn empty_bytes_are_an_error_not_a_panic() {
    let result = stele::img::decode_bytes(&[], None);
    assert!(result.is_err());
}

#[test]
fn malformed_bytes_with_valid_magic_are_an_error_not_a_panic() {
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&[0xFF; 16]);
    let result = stele::img::decode_bytes(&bytes, None);
    assert!(matches!(result, Err(DecodeError::Malformed(_)) | Err(DecodeError::Unsupported(_))));
}
