//! PNG decoder (P4) tests. Fixtures are generated in-test via the `png` crate's
//! own encoder so pixel comparisons can be exact (PNG is lossless).

use stele::img::png::PngDecoder;
use stele::img::{Decode, DecodeError};

/// Encode `pixels` (RGBA8, row-major, straight alpha) as a PNG with the given
/// `color_type`/`bit_depth`, optionally through a palette. Returns the bytes.
fn encode_rgba_as(
    width: u32,
    height: u32,
    pixels: &[u8],
    color_type: png::ColorType,
    bit_depth: png::BitDepth,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color_type);
        encoder.set_depth(bit_depth);

        let data: Vec<u8> = match color_type {
            png::ColorType::Rgba => pixels.to_vec(),
            png::ColorType::Rgb => pixels
                .chunks_exact(4)
                .flat_map(|px| [px[0], px[1], px[2]])
                .collect(),
            png::ColorType::GrayscaleAlpha => pixels
                .chunks_exact(4)
                .flat_map(|px| [px[0], px[3]])
                .collect(),
            png::ColorType::Grayscale => pixels
                .chunks_exact(4)
                .flat_map(|px| [px[0]])
                .collect(),
            other => panic!("test helper does not support {other:?} directly"),
        };

        let mut writer = encoder.write_header().expect("write PNG header");
        writer.write_image_data(&data).expect("write PNG data");
    }
    bytes
}

fn encode_palette(
    width: u32,
    height: u32,
    indices: &[u8],
    palette_rgb: &[u8],
    trns: Option<&[u8]>,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(palette_rgb.to_vec());
        if let Some(trns) = trns {
            encoder.set_trns(trns.to_vec());
        }
        let mut writer = encoder.write_header().expect("write PNG header");
        writer.write_image_data(indices).expect("write PNG data");
    }
    bytes
}

#[test]
fn round_trips_rgba8_exactly() {
    // 2x2: red, green, blue, semi-opaque black.
    #[rustfmt::skip]
    let pixels: Vec<u8> = vec![
        255, 0, 0, 255,    0, 255, 0, 255,
        0, 0, 255, 255,    0, 0, 0, 128,
    ];
    let bytes = encode_rgba_as(2, 2, &pixels, png::ColorType::Rgba, png::BitDepth::Eight);

    let frames = PngDecoder.decode(&bytes).expect("decode must succeed");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].delay_ms, 0);
    let img = &frames[0].image;
    assert_eq!(img.width, 2);
    assert_eq!(img.height, 2);
    assert_eq!(img.pixels, pixels);
}

#[test]
fn round_trips_opaque_rgb_by_adding_full_alpha() {
    #[rustfmt::skip]
    let pixels: Vec<u8> = vec![
        10, 20, 30, 255,   40, 50, 60, 255,
        70, 80, 90, 255,   100, 110, 120, 255,
    ];
    let bytes = encode_rgba_as(2, 2, &pixels, png::ColorType::Rgb, png::BitDepth::Eight);

    let frames = PngDecoder.decode(&bytes).expect("decode must succeed");
    assert_eq!(frames[0].image.pixels, pixels);
}

#[test]
fn round_trips_grayscale_by_replicating_into_rgb_with_full_alpha() {
    // 2x1 grayscale image: values 10 and 200.
    let bytes = encode_rgba_as(
        2,
        1,
        &[10, 10, 10, 255, 200, 200, 200, 255],
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
    );

    let frames = PngDecoder.decode(&bytes).expect("decode must succeed");
    let img = &frames[0].image;
    assert_eq!(img.width, 2);
    assert_eq!(img.height, 1);
    assert_eq!(img.pixels, vec![10, 10, 10, 255, 200, 200, 200, 255]);
}

#[test]
fn round_trips_grayscale_alpha_exactly() {
    let bytes = encode_rgba_as(
        2,
        1,
        &[10, 10, 10, 255, 200, 200, 200, 0],
        png::ColorType::GrayscaleAlpha,
        png::BitDepth::Eight,
    );

    let frames = PngDecoder.decode(&bytes).expect("decode must succeed");
    let img = &frames[0].image;
    assert_eq!(img.pixels, vec![10, 10, 10, 255, 200, 200, 200, 0]);
}

#[test]
fn round_trips_palette_with_transparency() {
    // Palette: 0 = opaque red, 1 = fully transparent green (via tRNS).
    let indices = vec![0u8, 1, 1, 0]; // 2x2
    let palette = vec![255, 0, 0, /* red */ 0, 255, 0 /* green */];
    let trns = vec![255u8, 0u8]; // index 0 opaque, index 1 transparent
    let bytes = encode_palette(2, 2, &indices, &palette, Some(&trns));

    let frames = PngDecoder.decode(&bytes).expect("decode must succeed");
    let img = &frames[0].image;
    assert_eq!(img.width, 2);
    assert_eq!(img.height, 2);
    #[rustfmt::skip]
    let expected: Vec<u8> = vec![
        255, 0, 0, 255,    0, 255, 0, 0,
        0, 255, 0, 0,      255, 0, 0, 255,
    ];
    assert_eq!(img.pixels, expected);
}

#[test]
fn round_trips_palette_without_transparency_as_opaque() {
    let indices = vec![0u8, 1, 2, 0]; // 2x2
    let palette = vec![
        10, 20, 30, // 0
        40, 50, 60, // 1
        70, 80, 90, // 2
    ];
    let bytes = encode_palette(2, 2, &indices, &palette, None);

    let frames = PngDecoder.decode(&bytes).expect("decode must succeed");
    let img = &frames[0].image;
    #[rustfmt::skip]
    let expected: Vec<u8> = vec![
        10, 20, 30, 255,   40, 50, 60, 255,
        70, 80, 90, 255,   10, 20, 30, 255,
    ];
    assert_eq!(img.pixels, expected);
}

#[test]
fn wrong_magic_is_not_this_format() {
    let result = PngDecoder.decode(b"GIF89a not a png at all");
    assert!(matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn empty_bytes_are_not_this_format() {
    let result = PngDecoder.decode(&[]);
    assert!(matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn truncated_signature_only_is_malformed_not_a_panic() {
    // Correct 8-byte PNG signature, nothing else.
    let sig: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let result = PngDecoder.decode(sig);
    assert!(result.is_err());
    assert!(!matches!(result, Err(DecodeError::NotThisFormat)));
}

#[test]
fn truncated_after_header_is_malformed_not_a_panic() {
    let bytes = encode_rgba_as(
        4,
        4,
        &[0u8; 4 * 4 * 4],
        png::ColorType::Rgba,
        png::BitDepth::Eight,
    );
    // Cut off partway through — after the signature/IHDR but before all IDAT.
    let truncated = &bytes[..bytes.len() - 10];
    let result = PngDecoder.decode(truncated);
    assert!(result.is_err());
}

#[test]
fn garbage_bytes_never_panic() {
    let garbage = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0xFF, 0xFF, 0xFF];
    let result = PngDecoder.decode(&garbage);
    assert!(result.is_err());
}

/// Minimal CRC32 (the PNG chunk checksum), so oversized-dimension fixtures
/// below can be hand-built without allocating an actual multi-hundred-MB
/// pixel buffer (which `png::Encoder::write_image_data` would require).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn write_chunk(bytes: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    bytes.extend_from_slice(chunk_type);
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A minimal-but-valid PNG stream: signature + IHDR(width, height, RGBA8) +
/// an empty IDAT (enough for `Reader::read_info` to locate the image-data
/// start — it does not need to actually decompress anything) + IEND. Lets
/// the oversized-dimension test below exercise the pixel cap without ever
/// allocating a real `width*height*4`-sized buffer to encode from.
fn minimal_png_header_only(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: RGBA
    ihdr.push(0); // compression method
    ihdr.push(0); // filter method
    ihdr.push(0); // interlace method
    write_chunk(&mut bytes, b"IHDR", &ihdr);
    write_chunk(&mut bytes, b"IDAT", &[]);
    write_chunk(&mut bytes, b"IEND", &[]);
    bytes
}

#[test]
fn oversized_dimensions_are_rejected_as_unsupported_before_pixel_buffer_allocation() {
    // 9000x9000 = 81,000,000 px > the 64,000,000px decode cap. This must be
    // rejected right after the (cheap) header is parsed, never attempting to
    // allocate the ~324MB RGBA8 buffer those dimensions imply.
    let bytes = minimal_png_header_only(9000, 9000);
    let result = PngDecoder.decode(&bytes);
    assert!(
        matches!(result, Err(DecodeError::Unsupported(_))),
        "expected Unsupported for an over-cap image, got {result:?}"
    );
}
