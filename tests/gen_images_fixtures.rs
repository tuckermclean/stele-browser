//! Generator for the images-packet fixture's binary image assets
//! (`fixtures/images-*.{png,gif}`, referenced by `fixtures/images.html`).
//!
//! Not part of the normal test suite — `#[ignore]`d, so `cargo test` never
//! runs it (and never touches the filesystem outside the sandboxed temp dirs
//! every other test uses). Run explicitly, once, to (re)produce the checked-
//! in fixture files:
//!
//! ```sh
//! cargo test --test gen_images_fixtures -- --ignored
//! ```
//!
//! Deterministic: every image is a solid flat color (no photographic/noisy
//! content, no timestamps, no randomness), so re-running this against the
//! same crate versions reproduces byte-identical files — the whole point of
//! keeping this as a committed generator (per the packet brief) rather than
//! opaque binary blobs with no record of how they were made.
//!
//! The fourth `images.html` asset (a JPEG) is NOT generated here —
//! `fixtures/p4-baseline.jpg` (already committed, from the P4 image-decoder
//! packet) is reused as-is.

const SIZE: u16 = 16;

#[test]
#[ignore = "generator, not a normal test -- run explicitly to (re)produce fixtures/images-*"]
fn generate_images_fixture_assets() {
    write_png();
    write_still_gif();
    write_animated_gif();
}

/// `fixtures/images-red.png`: a solid 16x16 opaque red PNG. Reuses
/// `backend::raster::encode_png` (the same deterministic encoder
/// `tests/png_golden.rs` already trusts) rather than hand-rolling PNG bytes.
fn write_png() {
    use stele::surface::{Color, MemSurface};
    let surface = MemSurface::new(SIZE as u32, SIZE as u32, Color::rgb(220, 30, 30));
    let bytes = stele::backend::raster::encode_png(&surface);
    std::fs::write("fixtures/images-red.png", bytes).expect("write PNG fixture");
}

/// `fixtures/images-blue.gif`: a solid 16x16 opaque blue, non-animated GIF
/// (one frame, `delay == 0`).
fn write_still_gif() {
    let mut buf = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut buf, SIZE, SIZE, &[]).expect("create gif encoder");
        let frame = solid_frame([30, 60, 220, 255]);
        encoder.write_frame(&frame).expect("write still gif frame");
    }
    std::fs::write("fixtures/images-blue.gif", buf).expect("write GIF fixture");
}

/// `fixtures/images-anim.gif`: a 2-frame animated GIF (solid yellow, then
/// solid green, 500ms apart, looping forever) — exercises the P4 GIF
/// decoder's multi-frame compositing (`img::gif`) and this packet's own
/// "frame 0 only" static-render rule (`images::collect_images`).
fn write_animated_gif() {
    let mut buf = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut buf, SIZE, SIZE, &[]).expect("create gif encoder");
        encoder.set_repeat(gif::Repeat::Infinite).expect("set repeat");
        for color in [[220, 200, 30, 255], [30, 200, 90, 255]] {
            let mut frame = solid_frame(color);
            frame.delay = 50; // 500ms, in GIF's 10ms units
            encoder.write_frame(&frame).expect("write animated gif frame");
        }
    }
    std::fs::write("fixtures/images-anim.gif", buf).expect("write animated GIF fixture");
}

fn solid_frame(rgba: [u8; 4]) -> gif::Frame<'static> {
    let mut pixels = vec![0u8; SIZE as usize * SIZE as usize * 4];
    for px in pixels.chunks_exact_mut(4) {
        px.copy_from_slice(&rgba);
    }
    gif::Frame::from_rgba(SIZE, SIZE, &mut pixels)
}
