//! Linux framebuffer output backend (M4, last piece): paint a `MemSurface`
//! onto a real `/dev/fb0` console.
//!
//! Deliberately safe/std-only — NO `rustix`/ioctl/mmap (those need `unsafe`,
//! forbidden by this packet). Geometry comes from plain-text sysfs files
//! instead of an `ioctl(FBIOGET_VSCREENINFO)` call, and the device is a
//! sequential `std::fs::File::write` instead of an `mmap`'d framebuffer —
//! slower (no partial-redraw, one full-buffer write per call) but total, and
//! this is a one-shot "paint a page and exit" renderer, not a compositor
//! that needs the mmap'd speed.
//!
//! ## Split: testable core vs. untestable-in-CI device I/O
//!
//! There is no framebuffer device in CI (`/dev/fb0` absent on the runner, and
//! so is `/sys/class/graphics/fb0` — verified by hand against this repo's own
//! CI sandbox). So:
//! - [`parse_fb_info`] (geometry parsing) and [`convert_to_fb_bytes`] (pixel
//!   conversion) are pure functions, unit-tested thoroughly below.
//! - [`read_fb_info`] and [`write_to_device`] are the thin, un-mockable
//!   device-touching shims. They're still exercised in CI, just via their
//!   *error* path: calling them against the real (absent) default paths
//!   proves totality (`Err`, never a panic) precisely because the device
//!   isn't there.
//!
//! ## Assumed on-device pixel layouts
//!
//! - **32 bpp**: `BGRX8888` little-endian — the common x86 framebuffer
//!   layout. Byte order in memory per pixel: `[B, G, R, X]` (`X` unused,
//!   written `0`; the source alpha channel is NOT carried into it — the
//!   framebuffer has no alpha plane to blend against).
//! - **16 bpp**: `RGB565` little-endian — 5 bits red, 6 bits green, 5 bits
//!   blue, packed as `(R5 << 11) | (G6 << 5) | B5` into a `u16`, then that
//!   `u16` written low-byte-first.
//! - anything else: [`FbError::UnsupportedBpp`].

use std::fs::OpenOptions;
use std::io::Write;

use crate::surface::MemSurface;

/// Default sysfs directory for the primary framebuffer's geometry files.
pub const DEFAULT_SYSFS_DIR: &str = "/sys/class/graphics/fb0";

/// Default framebuffer device node.
pub const DEFAULT_DEVICE_PATH: &str = "/dev/fb0";

/// Parsed framebuffer geometry (brief: `FbInfo { width, height, bpp, stride }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FbInfo {
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    /// Bytes per row (may exceed `width * bytes_per_pixel(bpp)` — rows are
    /// padded to this stride).
    pub stride: u32,
}

/// Every failure mode in this module — all total, none ever panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FbError {
    /// A sysfs file or the device node couldn't be read/opened/written.
    /// Carries a human-readable description (path + underlying `io::Error`
    /// rendering) rather than the non-`Clone`/non-`PartialEq` `io::Error`
    /// itself.
    Io(String),
    /// A sysfs geometry file existed but its contents didn't parse (empty,
    /// garbage, wrong field count).
    Parse(String),
    /// A `bits_per_pixel` this module has no pixel-layout mapping for.
    UnsupportedBpp(u32),
    /// `height * stride` (the required output buffer size) doesn't fit in
    /// `usize` on this target — a hostile/garbage sysfs geometry, guarded
    /// against rather than trusted, since `usize` is only 32 bits on the
    /// i486 target this whole browser is built for.
    GeometryTooLarge,
}

impl std::fmt::Display for FbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FbError::Io(s) => write!(f, "{s}"),
            FbError::Parse(s) => write!(f, "{s}"),
            FbError::UnsupportedBpp(bpp) => write!(f, "unsupported framebuffer bits_per_pixel: {bpp}"),
            FbError::GeometryTooLarge => write!(f, "framebuffer geometry too large for this target"),
        }
    }
}

/// The pure geometry-parsing core: `virtual_size` is sysfs's own
/// `"WIDTH,HEIGHT"` shape (e.g. `"1024,768"`); `bpp_str`/`stride_str` are
/// each a single decimal integer (sysfs's `bits_per_pixel`/`stride` files).
/// Total over any input: empty/garbage/wrong-field-count text is a clean
/// `Err(FbError::Parse(_))`, never a panic.
pub fn parse_fb_info(_virtual_size: &str, _bpp_str: &str, _stride_str: &str) -> Result<FbInfo, FbError> {
    // RED stub: not yet implemented.
    Err(FbError::Parse("not yet implemented".to_string()))
}

fn parse_virtual_size(s: &str) -> Result<(u32, u32), FbError> {
    let s = s.trim();
    let mut parts = s.split(',');
    let w = parts.next().ok_or_else(|| FbError::Parse(format!("virtual_size: missing width: {s:?}")))?;
    let h = parts.next().ok_or_else(|| FbError::Parse(format!("virtual_size: missing height: {s:?}")))?;
    if parts.next().is_some() {
        return Err(FbError::Parse(format!("virtual_size: too many fields: {s:?}")));
    }
    let w = w.trim().parse::<u32>().map_err(|_| FbError::Parse(format!("virtual_size: bad width: {s:?}")))?;
    let h = h.trim().parse::<u32>().map_err(|_| FbError::Parse(format!("virtual_size: bad height: {s:?}")))?;
    Ok((w, h))
}

fn parse_u32_field(s: &str, field: &str) -> Result<u32, FbError> {
    s.trim().parse::<u32>().map_err(|_| FbError::Parse(format!("{field}: not a valid integer: {s:?}")))
}

/// Read+parse `{sysfs_dir}/virtual_size`, `{sysfs_dir}/bits_per_pixel`,
/// `{sysfs_dir}/stride` into an [`FbInfo`]. Total: a missing/unreadable
/// directory (no framebuffer driver loaded — the CI-normal case) or garbage
/// contents is a clean `Err`, never a panic.
pub fn read_fb_info(sysfs_dir: &str) -> Result<FbInfo, FbError> {
    let read = |name: &str| -> Result<String, FbError> {
        let path = format!("{sysfs_dir}/{name}");
        std::fs::read_to_string(&path).map_err(|e| FbError::Io(format!("{path}: {e}")))
    };
    let virtual_size = read("virtual_size")?;
    let bpp = read("bits_per_pixel")?;
    let stride = read("stride")?;
    parse_fb_info(&virtual_size, &bpp, &stride)
}

#[inline]
fn bytes_per_pixel(bpp: u32) -> Option<u32> {
    match bpp {
        32 => Some(4),
        16 => Some(2),
        _ => None,
    }
}

/// Convert an RGBA8 `MemSurface`'s raw bytes (`surf_w * surf_h * 4` bytes,
/// row-major, `[R, G, B, A]` per pixel — `MemSurface::bytes`'s own layout)
/// into the target framebuffer's on-device byte layout for `info.bpp`.
///
/// Pure — no device access, so fully unit-testable. Produces a `Vec<u8>`
/// sized exactly `info.height * info.stride`: rows padded to `info.stride`
/// (bytes beyond the copied pixel data in a row are left `0`), and if
/// `info.stride` itself is narrower than `info.width * bytes_per_pixel`,
/// the copied width is clipped to what actually fits in a row rather than
/// overflowing into the next one.
///
/// Clipping: if the surface is larger than the framebuffer in either
/// dimension, only the top-left `min(surf_w, info.width) x
/// min(surf_h, info.height)` region is copied (documented in the module
/// docs / DECISIONS: "surface larger than fb clips, never overflows the
/// device buffer's byte math"). If the surface is smaller, the extra
/// framebuffer rows/columns are left `0` (black) — there is no prior
/// frame to preserve on a one-shot render.
///
/// Total: unsupported `bpp` is `Err(FbError::UnsupportedBpp)`; a
/// `height * stride` that doesn't fit `usize` is `Err(FbError::GeometryTooLarge)`
/// rather than an overflowing/panicking allocation.
pub fn convert_to_fb_bytes(_surface_rgba: &[u8], _surf_w: u32, _surf_h: u32, info: FbInfo) -> Result<Vec<u8>, FbError> {
    // RED stub: not yet implemented.
    let _ = bytes_per_pixel(info.bpp);
    Err(FbError::UnsupportedBpp(info.bpp))
}

/// Write `bytes` (already device-formatted, e.g. by [`convert_to_fb_bytes`])
/// to `device_path` in full, from offset 0. Total: an absent device, a
/// permission-denied open, or a short/failed write is a clean
/// `Err(FbError::Io(_))`, never a panic — this is precisely the path that
/// can't be exercised for real in CI (no `/dev/fb0` on the runner), so its
/// only CI coverage is this same error path, deliberately.
///
/// No `mmap`; this is a one-shot "paint and exit" render, not a compositor,
/// so there's no need for the speed (or the `unsafe`) an `mmap`'d
/// framebuffer would bring — and no restore-on-exit either, for the same
/// reason (nothing owns the screen afterward to hand it back to).
pub fn write_to_device(bytes: &[u8], device_path: &str) -> Result<(), FbError> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(device_path)
        .map_err(|e| FbError::Io(format!("{device_path}: {e}")))?;
    file.write_all(bytes).map_err(|e| FbError::Io(format!("{device_path}: write failed: {e}")))
}

/// The end-to-end fbdev output path: read geometry from `sysfs_dir`, convert
/// `surface`'s pixels to the device's layout, and write them to
/// `device_path`. Total: any failure at any stage (geometry unreadable,
/// unsupported bpp, device unwritable) is a clean `Err`, never a panic.
pub fn render_to_device(surface: &MemSurface, sysfs_dir: &str, device_path: &str) -> Result<(), FbError> {
    let info = read_fb_info(sysfs_dir)?;
    let (w, h) = crate::surface::Surface::size(surface);
    let bytes = convert_to_fb_bytes(surface.bytes(), w, h, info)?;
    write_to_device(&bytes, device_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::{Color, Surface};

    // --------------------------------------------------------- geometry parsing

    #[test]
    fn parse_fb_info_reads_a_well_formed_sysfs_triple() {
        let info = parse_fb_info("1024,768", "32", "4096").expect("well-formed geometry parses");
        assert_eq!(info, FbInfo { width: 1024, height: 768, bpp: 32, stride: 4096 });
    }

    #[test]
    fn parse_fb_info_trims_trailing_newlines_like_real_sysfs_files() {
        // Real sysfs files end in "\n" — std::fs::read_to_string keeps it.
        let info = parse_fb_info("1024,768\n", "16\n", "2048\n").expect("trailing newlines are tolerated");
        assert_eq!(info, FbInfo { width: 1024, height: 768, bpp: 16, stride: 2048 });
    }

    #[test]
    fn parse_fb_info_rejects_garbage_virtual_size() {
        assert!(parse_fb_info("garbage", "32", "4096").is_err());
        assert!(parse_fb_info("", "32", "4096").is_err());
        assert!(parse_fb_info("1024", "32", "4096").is_err());
        assert!(parse_fb_info("1024,768,90", "32", "4096").is_err());
    }

    #[test]
    fn parse_fb_info_rejects_garbage_bpp_or_stride() {
        assert!(parse_fb_info("1024,768", "", "4096").is_err());
        assert!(parse_fb_info("1024,768", "thirty-two", "4096").is_err());
        assert!(parse_fb_info("1024,768", "32", "").is_err());
        assert!(parse_fb_info("1024,768", "32", "not-a-number").is_err());
    }

    #[test]
    fn parse_fb_info_never_panics_on_hostile_input() {
        // A grab-bag of hostile strings; the only contract is "no panic".
        let hostile = ["\0", ",", ",,", "-1,-1", "99999999999999999999,1", "1,1,1,1,1"];
        for s in hostile {
            let _ = parse_fb_info(s, "32", "4096");
            let _ = parse_fb_info("1024,768", s, "4096");
            let _ = parse_fb_info("1024,768", "32", s);
        }
    }

    // ------------------------------------------------------- read_fb_info (I/O)

    #[test]
    fn read_fb_info_on_a_missing_sysfs_dir_is_a_clean_err_not_a_panic() {
        // The real default path -- absent in this sandbox and in CI (no fb
        // driver loaded on the runner), which is exactly the case this
        // packet must degrade cleanly on.
        let result = read_fb_info(DEFAULT_SYSFS_DIR);
        assert!(result.is_err(), "expected Err since {DEFAULT_SYSFS_DIR} has no fb driver in this environment");
    }

    #[test]
    fn read_fb_info_on_an_explicitly_bogus_dir_is_a_clean_err() {
        let result = read_fb_info("/nonexistent-sysfs-dir-xyz/fb0");
        assert!(result.is_err());
    }

    #[test]
    fn read_fb_info_reads_real_files_from_a_tmp_dir() {
        let dir = std::env::temp_dir().join(format!("stele-fb-test-{}-a", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create tmp sysfs dir");
        std::fs::write(dir.join("virtual_size"), "640,480\n").unwrap();
        std::fs::write(dir.join("bits_per_pixel"), "16\n").unwrap();
        std::fs::write(dir.join("stride"), "1280\n").unwrap();

        let info = read_fb_info(dir.to_str().unwrap()).expect("well-formed tmp sysfs dir parses");
        assert_eq!(info, FbInfo { width: 640, height: 480, bpp: 16, stride: 1280 });

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------- pixel conversion

    /// A hand-built 2x2 RGBA surface: top-left red, top-right green,
    /// bottom-left blue, bottom-right white.
    fn swatch_2x2() -> MemSurface {
        let mut s = MemSurface::new(2, 2, Color::BLACK);
        s.put_pixel(0, 0, Color::rgb(255, 0, 0));
        s.put_pixel(1, 0, Color::rgb(0, 255, 0));
        s.put_pixel(0, 1, Color::rgb(0, 0, 255));
        s.put_pixel(1, 1, Color::rgb(255, 255, 255));
        s
    }

    #[test]
    fn convert_32bpp_produces_exact_bgrx_bytes_no_padding() {
        let surf = swatch_2x2();
        let info = FbInfo { width: 2, height: 2, bpp: 32, stride: 8 }; // 2px * 4B = 8B, no pad
        let out = convert_to_fb_bytes(surf.bytes(), 2, 2, info).expect("32bpp is supported");

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            // row 0: red, green
            0, 0, 255, 0,   0, 255, 0, 0,
            // row 1: blue, white
            255, 0, 0, 0,   255, 255, 255, 0,
        ];
        assert_eq!(out, expected);
        assert_eq!(out.len(), (info.height * info.stride) as usize);
    }

    #[test]
    fn convert_16bpp_produces_exact_rgb565_bytes_no_padding() {
        let surf = swatch_2x2();
        let info = FbInfo { width: 2, height: 2, bpp: 16, stride: 4 }; // 2px * 2B = 4B, no pad
        let out = convert_to_fb_bytes(surf.bytes(), 2, 2, info).expect("16bpp is supported");

        // Hand-computed RGB565:
        //   red   (255,0,0)   -> R5=31,G6=0,B5=0  -> 0b11111_000000_00000 = 0xF800 -> LE [0x00, 0xF8]
        //   green (0,255,0)   -> R5=0,G6=63,B5=0   -> 0b00000_111111_00000 = 0x07E0 -> LE [0xE0, 0x07]
        //   blue  (0,0,255)   -> R5=0,G6=0,B5=31   -> 0b00000_000000_11111 = 0x001F -> LE [0x1F, 0x00]
        //   white (255,255,255) -> R5=31,G6=63,B5=31 -> 0xFFFF -> LE [0xFF, 0xFF]
        let expected: Vec<u8> = vec![
            0x00, 0xF8, 0xE0, 0x07, // row 0: red, green
            0x1F, 0x00, 0xFF, 0xFF, // row 1: blue, white
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn convert_pads_rows_when_surface_is_narrower_than_stride() {
        let surf = swatch_2x2();
        // stride is 16 bytes/row but only 2px (8B) of real data per row.
        let info = FbInfo { width: 2, height: 2, bpp: 32, stride: 16 };
        let out = convert_to_fb_bytes(surf.bytes(), 2, 2, info).expect("32bpp is supported");
        assert_eq!(out.len(), 32);

        // Row 0: red, green, then 8 bytes of padding.
        assert_eq!(&out[0..8], &[0, 0, 255, 0, 0, 255, 0, 0]);
        assert_eq!(&out[8..16], &[0u8; 8]);
        // Row 1: blue, white, then 8 bytes of padding.
        assert_eq!(&out[16..24], &[255, 0, 0, 0, 255, 255, 255, 0]);
        assert_eq!(&out[24..32], &[0u8; 8]);
    }

    #[test]
    fn convert_unsupported_bpp_is_a_clean_err() {
        let surf = swatch_2x2();
        let info = FbInfo { width: 2, height: 2, bpp: 24, stride: 6 };
        let result = convert_to_fb_bytes(surf.bytes(), 2, 2, info);
        assert_eq!(result, Err(FbError::UnsupportedBpp(24)));
    }

    #[test]
    fn convert_clips_a_surface_larger_than_the_framebuffer_instead_of_overflowing() {
        // A 4x4 surface onto a 2x2 "device" -- must clip to the top-left
        // 2x2, produce exactly height*stride bytes, and never panic/overflow
        // indexing into the smaller output buffer.
        let mut surf = MemSurface::new(4, 4, Color::BLACK);
        surf.put_pixel(0, 0, Color::rgb(255, 0, 0));
        surf.put_pixel(1, 0, Color::rgb(0, 255, 0));
        surf.put_pixel(0, 1, Color::rgb(0, 0, 255));
        surf.put_pixel(1, 1, Color::rgb(255, 255, 255));
        // Pixels outside the 2x2 top-left corner (e.g. (3,3)) are bright
        // magenta -- must NOT show up anywhere in the clipped output.
        surf.put_pixel(3, 3, Color::rgb(255, 0, 255));

        let info = FbInfo { width: 2, height: 2, bpp: 32, stride: 8 };
        let out = convert_to_fb_bytes(surf.bytes(), 4, 4, info).expect("32bpp is supported");
        assert_eq!(out.len(), 16);

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            0, 0, 255, 0,   0, 255, 0, 0,
            255, 0, 0, 0,   255, 255, 255, 0,
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn convert_a_surface_smaller_than_the_framebuffer_leaves_the_rest_zeroed() {
        let surf = swatch_2x2();
        let info = FbInfo { width: 4, height: 4, bpp: 32, stride: 16 };
        let out = convert_to_fb_bytes(surf.bytes(), 2, 2, info).expect("32bpp is supported");
        assert_eq!(out.len(), 64);

        // Row 0: red, green, then black (0,0,0,0) x2.
        assert_eq!(&out[0..8], &[0, 0, 255, 0, 0, 255, 0, 0]);
        assert_eq!(&out[8..16], &[0u8; 8]);
        // Rows 2 and 3 (untouched device rows) are entirely zero.
        assert_eq!(&out[32..64], &[0u8; 32]);
    }

    #[test]
    fn convert_stride_narrower_than_full_row_clips_the_row_rather_than_overflowing() {
        // 3 logical px wide, but stride only fits 2 32bpp px (8B) per row --
        // the 3rd column must be dropped, not overflow into the next row.
        let mut surf = MemSurface::new(3, 1, Color::BLACK);
        surf.put_pixel(0, 0, Color::rgb(1, 2, 3));
        surf.put_pixel(1, 0, Color::rgb(4, 5, 6));
        surf.put_pixel(2, 0, Color::rgb(7, 8, 9));

        let info = FbInfo { width: 3, height: 1, bpp: 32, stride: 8 };
        let out = convert_to_fb_bytes(surf.bytes(), 3, 1, info).expect("32bpp is supported");
        assert_eq!(out.len(), 8);
        assert_eq!(out, vec![3, 2, 1, 0, 6, 5, 4, 0]);
    }

    // ------------------------------------------------------------- totality

    #[test]
    fn write_to_device_on_the_real_default_device_path_is_a_clean_err_not_a_panic() {
        // /dev/fb0 is absent in this sandbox and in CI -- exactly the case
        // this function must degrade cleanly on.
        let result = write_to_device(&[0u8; 16], DEFAULT_DEVICE_PATH);
        assert!(result.is_err(), "expected Err since {DEFAULT_DEVICE_PATH} doesn't exist in this environment");
    }

    #[test]
    fn write_to_device_on_an_explicitly_bogus_path_is_a_clean_err() {
        let result = write_to_device(&[0u8; 16], "/nonexistent-dir-xyz/fb0");
        assert!(result.is_err());
    }

    #[test]
    fn render_to_device_with_absent_sysfs_and_device_is_a_clean_err_not_a_panic() {
        let surface = MemSurface::new(4, 4, Color::WHITE);
        let result = render_to_device(&surface, DEFAULT_SYSFS_DIR, DEFAULT_DEVICE_PATH);
        assert!(result.is_err());
    }

    #[test]
    fn render_to_device_with_real_geometry_but_absent_device_is_a_clean_err() {
        // Geometry parses fine (tmp sysfs dir), but the device write still
        // fails cleanly since there's no real /dev/fb0 in this environment.
        let dir = std::env::temp_dir().join(format!("stele-fb-test-{}-b", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create tmp sysfs dir");
        std::fs::write(dir.join("virtual_size"), "4,4").unwrap();
        std::fs::write(dir.join("bits_per_pixel"), "32").unwrap();
        std::fs::write(dir.join("stride"), "16").unwrap();

        let surface = MemSurface::new(4, 4, Color::WHITE);
        let result = render_to_device(&surface, dir.to_str().unwrap(), "/nonexistent-dir-xyz/fb0");
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
