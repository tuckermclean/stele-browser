//! Hand-rolled X11 client backend (packet/x11): enough of the core X11
//! protocol, implemented from scratch against `std::os::unix::net::UnixStream`,
//! to open a real window on a bitmap-only kdrive/Xfbdev server, blit the
//! existing pixel `Surface` into it with `PutImage`, and read back
//! keyboard/mouse events. No X11 client library, no `unsafe`, no new crates
//! — the whole wire protocol is plain byte-buffer encode/decode.
//!
//! ## Split: pure protocol (unit-tested) vs. socket I/O (manual-only)
//!
//! Same split this codebase already uses for `backend::fb` and
//! `browser::{KeyParser, GpmConnect}`: every function that turns protocol
//! *bytes* into Rust values (or Rust values into protocol bytes) is a pure,
//! panic-free function, unit-tested below with synthetic buffers — no
//! socket, no real X server required, so these tests run identically in
//! CI. [`XConnection`] is the thin, deliberately-*not*-unit-tested shim that
//! drives those pure functions over a real `UnixStream`: there is no X
//! server in CI to open a window against, so this half is manually verified
//! only (see `src/main.rs`'s `run_x11`, and the packet report for how it was
//! exercised).
//!
//! ## Wire conventions
//!
//! Every multi-byte field in every request/reply/event this module speaks
//! is little-endian (`byte-order = 'l' = 0x6c`, sent once, in the
//! connection-setup request) — the target (x86/i486) is little-endian
//! natively, so this module never handles the big-endian ('B') half of the
//! protocol; the server is trusted to honor the byte-order this client
//! declared, per spec.

use crate::layout::{Fragment, Interactive};

// =========================================================================
// Small byte-buffer helpers
// =========================================================================

/// Bytes needed to pad `n` up to the next multiple of 4 — every X11
/// variable-length field (strings, `PutImage` data, ...) is padded to a
/// 4-byte boundary.
fn pad_len(n: usize) -> usize {
    (4 - (n % 4)) % 4
}

fn get_u8(buf: &[u8], off: usize) -> Option<u8> {
    buf.get(off).copied()
}

fn get_u16_le(buf: &[u8], off: usize) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn get_u32_le(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

// =========================================================================
// DISPLAY parsing
// =========================================================================

/// Parse a `$DISPLAY` value (e.g. `":0"`, `":0.0"`, `"unix:1.0"`, empty
/// `host` meaning "local") into `(host, display_number, screen_number)`.
/// Total: anything with no `':'` is a clean `Err`, never a panic; a missing
/// screen number defaults to `0` (matching every real `DISPLAY` a desktop
/// sets — `":0"` alone is universal).
pub fn parse_display(display: &str) -> Result<(String, u16, u16), String> {
    let (host_part, rest) = display.split_once(':').ok_or_else(|| format!("invalid DISPLAY (missing ':'): {display:?}"))?;
    let (display_str, screen_str) = match rest.split_once('.') {
        Some((d, s)) => (d, s),
        None => (rest, "0"),
    };
    let display_num: u16 = display_str.parse().map_err(|_| format!("invalid DISPLAY display number: {display:?}"))?;
    let screen_num: u16 = screen_str.parse().unwrap_or(0);
    // "unix:0.0" and ":0.0" both mean "local" -- the leading host part is
    // only ever meaningful for a REMOTE display (TCP), which this client
    // (Unix-domain-socket-only, per the packet brief) never dials anyway.
    let host = if host_part.eq_ignore_ascii_case("unix") { String::new() } else { host_part.to_string() };
    Ok((host, display_num, screen_num))
}

// =========================================================================
// .Xauthority parsing
// =========================================================================

/// Read one big-endian-length-prefixed field (`u16` length, then that many
/// bytes) starting at `offset`. Total: a length that runs past the end of
/// `data` is `None`, never a panic/slice-index-out-of-bounds.
fn read_be_field(data: &[u8], offset: usize) -> Option<(&[u8], usize)> {
    let len_bytes = data.get(offset..offset + 2)?;
    let len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
    let start = offset + 2;
    let field = data.get(start..start + len)?;
    Some((field, start + len))
}

/// Extract the `MIT-MAGIC-COOKIE-1` auth cookie from a raw `.Xauthority`
/// file buffer (binary format: repeated entries of big-endian-u16-length-
/// prefixed `family, address, number, name, data`, back to back with no
/// padding between entries).
///
/// When `display` is `Some`, prefers the entry whose `number` field (the
/// display number, as ASCII digits) matches; otherwise (or if none match)
/// falls back to the FIRST `MIT-MAGIC-COOKIE-1` entry found, matching the
/// packet brief ("match display number if present else first"). `None` if
/// the buffer has no `MIT-MAGIC-COOKIE-1` entry at all, or is truncated
/// before one is found.
///
/// Total: a truncated/garbage buffer never panics — [`read_be_field`]'s
/// bounds-checked reads turn a short buffer into a clean `None`.
pub fn parse_xauthority(data: &[u8], display: Option<&str>) -> Option<Vec<u8>> {
    let mut offset = 0usize;
    let mut fallback: Option<Vec<u8>> = None;

    while offset < data.len() {
        let (_family, next) = read_be_field_u16_prefixed_family(data, offset)?;
        offset = next;
        let (_address, next) = read_be_field(data, offset)?;
        offset = next;
        let (number, next) = read_be_field(data, offset)?;
        offset = next;
        let (name, next) = read_be_field(data, offset)?;
        offset = next;
        let (auth_data, next) = read_be_field(data, offset)?;
        offset = next;

        if name == b"MIT-MAGIC-COOKIE-1" {
            let matches_display = display.is_some_and(|d| number == d.as_bytes());
            if matches_display {
                return Some(auth_data.to_vec());
            }
            if fallback.is_none() {
                fallback = Some(auth_data.to_vec());
            }
        }
    }

    fallback
}

/// The `family` field is a bare `u16` (not length-prefixed like the other
/// four fields) — this reads it and returns the SAME `(field, next_offset)`
/// shape as [`read_be_field`] so `parse_xauthority`'s loop can treat all
/// five fields uniformly.
fn read_be_field_u16_prefixed_family(data: &[u8], offset: usize) -> Option<(&[u8], usize)> {
    let field = data.get(offset..offset + 2)?;
    Some((field, offset + 2))
}

// =========================================================================
// Connection setup
// =========================================================================

/// `byte-order` value this client always sends: `'l'` (0x6c), little-endian.
pub const BYTE_ORDER_LSB_FIRST: u8 = 0x6c;

/// Encode the X11 connection-setup request: byte-order, protocol 11.0, and
/// the given auth name/data (each length-prefixed and individually padded
/// to a 4-byte boundary, per spec). `auth_name`/`auth_data` may both be
/// empty (no authentication) — the server decides whether that's accepted.
pub fn encode_setup_request(auth_name: &str, auth_data: &[u8]) -> Vec<u8> {
    let name_bytes = auth_name.as_bytes();
    let name_pad = pad_len(name_bytes.len());
    let data_pad = pad_len(auth_data.len());

    let mut out = Vec::with_capacity(12 + name_bytes.len() + name_pad + auth_data.len() + data_pad);
    out.push(BYTE_ORDER_LSB_FIRST);
    out.push(0); // unused
    out.extend_from_slice(&11u16.to_le_bytes()); // protocol-major-version
    out.extend_from_slice(&0u16.to_le_bytes()); // protocol-minor-version
    out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(&(auth_data.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // unused
    out.extend_from_slice(name_bytes);
    out.extend(std::iter::repeat(0u8).take(name_pad));
    out.extend_from_slice(auth_data);
    out.extend(std::iter::repeat(0u8).take(data_pad));
    out
}

/// One entry of the setup reply's `PIXMAP-FORMATS` list: for a given
/// drawable `depth`, how many bits each pixel occupies and what bit
/// boundary each scanline is padded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixmapFormat {
    pub depth: u8,
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
}

/// Everything this client needs out of a successful connection-setup
/// reply: enough to allocate resource IDs, address the root window with
/// its native visual/depth, know each depth's `PutImage` byte layout, and
/// respect the server's per-request size limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupInfo {
    pub resource_id_base: u32,
    pub resource_id_mask: u32,
    pub root: u32,
    pub root_visual: u32,
    pub root_depth: u8,
    pub formats: Vec<PixmapFormat>,
    /// In 4-byte units, per spec — a request (including `PutImage`'s image
    /// data) must never exceed `maximum_request_length * 4` bytes.
    pub maximum_request_length: u32,
    pub min_keycode: u8,
    pub max_keycode: u8,
}

/// Parse a connection-setup reply (the 8-byte common header, immediately
/// followed by `length*4` bytes of "additional data" — [`XConnection::connect`]
/// reads exactly that much off the socket before calling this). Only the
/// FIRST screen's (`roots[0]`) fields are read — this client only ever
/// opens a window on the first/default screen — and the per-screen
/// `ALLOWED-DEPTHS`/`VISUALTYPES` lists that trail it are never walked
/// (root visual/depth are already directly on the `SCREEN` record itself,
/// so there's nothing else in there this client needs).
///
/// Total: `success == 0` (Failed) or `2` (Authenticate, unsupported by this
/// client) is a descriptive `Err`, never treated as `Ok`; any read that
/// would run past `buf`'s end is a clean `Err` ("truncated"), never a
/// panic/OOB.
pub fn parse_setup_reply(buf: &[u8]) -> Result<SetupInfo, String> {
    let success = get_u8(buf, 0).ok_or("setup reply: empty buffer")?;
    if success == 0 {
        let reason_len = get_u8(buf, 1).unwrap_or(0) as usize;
        let reason = buf.get(8..8 + reason_len).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
        return Err(format!("X server refused the connection: {reason}"));
    }
    if success == 2 {
        return Err("X server requires further authentication, which this client does not support".to_string());
    }
    if success != 1 {
        return Err(format!("unexpected connection-setup status byte: {success}"));
    }

    let trunc = || "setup reply: truncated".to_string();

    let resource_id_base = get_u32_le(buf, 12).ok_or_else(trunc)?;
    let resource_id_mask = get_u32_le(buf, 16).ok_or_else(trunc)?;
    let vendor_len = get_u16_le(buf, 24).ok_or_else(trunc)? as usize;
    let maximum_request_length = get_u16_le(buf, 26).ok_or_else(trunc)? as u32;
    let num_formats = get_u8(buf, 29).ok_or_else(trunc)? as usize;
    let min_keycode = get_u8(buf, 34).ok_or_else(trunc)?;
    let max_keycode = get_u8(buf, 35).ok_or_else(trunc)?;

    let vendor_start = 40; // 8 (header) + 32 (fixed additional-data fields)
    let formats_start = vendor_start + vendor_len + pad_len(vendor_len);

    let mut formats = Vec::with_capacity(num_formats);
    for i in 0..num_formats {
        let off = formats_start + i * 8;
        formats.push(PixmapFormat {
            depth: get_u8(buf, off).ok_or_else(trunc)?,
            bits_per_pixel: get_u8(buf, off + 1).ok_or_else(trunc)?,
            scanline_pad: get_u8(buf, off + 2).ok_or_else(trunc)?,
        });
    }

    let screens_start = formats_start + num_formats * 8;
    let root = get_u32_le(buf, screens_start).ok_or_else(trunc)?;
    let root_visual = get_u32_le(buf, screens_start + 32).ok_or_else(trunc)?;
    let root_depth = get_u8(buf, screens_start + 38).ok_or_else(trunc)?;

    Ok(SetupInfo { resource_id_base, resource_id_mask, root, root_visual, root_depth, formats, maximum_request_length, min_keycode, max_keycode })
}

// =========================================================================
// Resource IDs
// =========================================================================

/// Allocates client-owned resource IDs (windows, GCs, ...) per spec:
/// `resource_id_base | (counter & resource_id_mask)`, `counter` incrementing
/// once per call. `counter` is `wrapping_add`ed — this client opens at most
/// a handful of resources in its lifetime, so wraparound is a theoretical
/// concern only, guarded rather than assumed away.
#[derive(Debug, Clone, Copy)]
pub struct IdAllocator {
    base: u32,
    mask: u32,
    counter: u32,
}

impl IdAllocator {
    pub fn new(base: u32, mask: u32) -> Self {
        IdAllocator { base, mask, counter: 0 }
    }

    pub fn next(&mut self) -> u32 {
        let id = self.base | (self.counter & self.mask);
        self.counter = self.counter.wrapping_add(1);
        id
    }
}

// =========================================================================
// CreateWindow / MapWindow / CreateGC
// =========================================================================

const OP_CREATE_WINDOW: u8 = 1;
const OP_MAP_WINDOW: u8 = 8;
const OP_CREATE_PIXMAP: u8 = 53;
const OP_FREE_PIXMAP: u8 = 54;
const OP_CREATE_GC: u8 = 55;
const OP_PUT_IMAGE: u8 = 72;
const OP_GET_KEYBOARD_MAPPING: u8 = 101;

/// `CreateGC` value-mask bit for `graphics-exposures`.
const GC_GRAPHICS_EXPOSURES_MASK: u32 = 0x0001_0000;

/// `CWBackPixel` value-mask bit.
pub const CW_BACK_PIXEL: u32 = 0x0000_0002;
/// `CWEventMask` value-mask bit.
pub const CW_EVENT_MASK: u32 = 0x0000_0800;

pub const EVENT_MASK_KEY_PRESS: u32 = 0x0000_0001;
pub const EVENT_MASK_BUTTON_PRESS: u32 = 0x0000_0004;
pub const EVENT_MASK_EXPOSURE: u32 = 0x0000_8000;
pub const EVENT_MASK_STRUCTURE_NOTIFY: u32 = 0x0002_0000;

/// The event mask this client's window is created with — KeyPress +
/// ButtonPress + Exposure + StructureNotify, exactly the packet brief's
/// list (nothing else: no PointerMotion, no button-release — this shell
/// has no use for them).
pub const WINDOW_EVENT_MASK: u32 = EVENT_MASK_KEY_PRESS | EVENT_MASK_BUTTON_PRESS | EVENT_MASK_EXPOSURE | EVENT_MASK_STRUCTURE_NOTIFY;

const CLASS_INPUT_OUTPUT: u16 = 1;

/// Encode a `CreateWindow` request (opcode 1) with EXACTLY the value-mask
/// this packet needs: `CWBackPixel | CWEventMask` (in that bit order — the
/// value-list must follow the mask's bits low-to-high, per spec). `parent`,
/// `visual`, `depth` are the caller's (normally the root window's own, for
/// a top-level window on the default screen).
#[allow(clippy::too_many_arguments)]
pub fn encode_create_window(wid: u32, parent: u32, depth: u8, visual: u32, x: i16, y: i16, width: u16, height: u16, back_pixel: u32, event_mask: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(40);
    out.push(OP_CREATE_WINDOW);
    out.push(depth);
    out.extend_from_slice(&10u16.to_le_bytes()); // request length: 8 fixed words + 2 value words
    out.extend_from_slice(&wid.to_le_bytes());
    out.extend_from_slice(&parent.to_le_bytes());
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // border-width
    out.extend_from_slice(&CLASS_INPUT_OUTPUT.to_le_bytes());
    out.extend_from_slice(&visual.to_le_bytes());
    out.extend_from_slice(&(CW_BACK_PIXEL | CW_EVENT_MASK).to_le_bytes());
    out.extend_from_slice(&back_pixel.to_le_bytes());
    out.extend_from_slice(&event_mask.to_le_bytes());
    out
}

/// Encode a `MapWindow` request (opcode 8).
pub fn encode_map_window(window: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.push(OP_MAP_WINDOW);
    out.push(0); // unused
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&window.to_le_bytes());
    out
}

/// Encode a `SetInputFocus` request (opcode 42) directing keyboard focus to
/// `window`, `revert-to = Parent`, `time = CurrentTime`. Xfbdev/kdrive runs
/// with NO window manager, so a freshly-mapped window never receives keyboard
/// focus on its own — without this, KeyPress events (including `q`/Escape)
/// never reach us and the only way out is a reboot. 12 bytes; length = 3.
///
/// IMPORTANT — caller must not send this immediately after `MapWindow`: the
/// window isn't viewable yet at that point and a real server replies
/// `BadMatch`. `main.rs`'s `run_x11` instead sends it on the FIRST `Expose`
/// event, which only arrives once the window is actually viewable.
pub fn encode_set_input_focus(window: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.push(42); // opcode: SetInputFocus
    out.push(2); // revert-to = RevertToParent
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&window.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // time = CurrentTime
    out
}

/// Encode a `CreateGC` request (opcode 55) with graphics-exposures = FALSE:
/// this client never wants NoExpose/GraphicsExpose events (it repaints
/// damaged regions from its own server-side pixmap), and on non-retaining
/// servers discarding GraphicsExpose after CopyArea shows scroll garbage.
pub fn encode_create_gc(cid: u32, drawable: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.push(OP_CREATE_GC);
    out.push(0);
    out.extend_from_slice(&5u16.to_le_bytes()); // length in 4-byte words
    out.extend_from_slice(&cid.to_le_bytes());
    out.extend_from_slice(&drawable.to_le_bytes());
    out.extend_from_slice(&GC_GRAPHICS_EXPOSURES_MASK.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // FALSE
    out
}

/// Encode a `CreatePixmap` request (opcode 53) — allocates a server-side
/// pixmap of `depth` at `width`x`height`, rooted at `drawable` (used both
/// for the double-buffer and for icon/backing-store data).
pub fn encode_create_pixmap(pid: u32, drawable: u32, depth: u8, width: u16, height: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.push(OP_CREATE_PIXMAP);
    out.push(depth);
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&pid.to_le_bytes());
    out.extend_from_slice(&drawable.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out
}

/// Encode a `FreePixmap` request (opcode 54).
pub fn encode_free_pixmap(pid: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.push(OP_FREE_PIXMAP);
    out.push(0);
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&pid.to_le_bytes());
    out
}

// =========================================================================
// PutImage
// =========================================================================

/// `PutImage`'s `format` byte for `ZPixmap` (packed pixels, one plane) —
/// the only format this client ever sends.
pub const PUT_IMAGE_FORMAT_ZPIXMAP: u8 = 2;

/// Fixed header size (bytes) of a `PutImage` request, before the image data.
const PUT_IMAGE_HEADER_BYTES: usize = 24;

/// Encode a single `PutImage` request (opcode 72, `ZPixmap`) — one
/// unbanded chunk. `data` is already in the drawable's native on-the-wire
/// pixel layout (e.g. `backend::fb::convert_to_fb_bytes`'s output), each
/// scanline already padded to the format's `scanline-pad`; this function
/// only adds the REQUEST's own trailing pad (to a 4-byte boundary) on top.
///
/// Callers with more data than fits in one server request must not call
/// this directly — use [`put_image_requests`], which bands a full image
/// into a sequence of these that each stay under the server's
/// `maximum-request-length`.
#[allow(clippy::too_many_arguments)]
pub fn encode_put_image(drawable: u32, gc: u32, width: u16, height: u16, dst_x: i16, dst_y: i16, depth: u8, data: &[u8]) -> Vec<u8> {
    let data_pad = pad_len(data.len());
    let total_words = (PUT_IMAGE_HEADER_BYTES + data.len() + data_pad) / 4;
    let mut out = Vec::with_capacity(PUT_IMAGE_HEADER_BYTES + data.len() + data_pad);
    out.push(OP_PUT_IMAGE);
    out.push(PUT_IMAGE_FORMAT_ZPIXMAP);
    out.extend_from_slice(&(total_words as u16).to_le_bytes());
    out.extend_from_slice(&drawable.to_le_bytes());
    out.extend_from_slice(&gc.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&dst_x.to_le_bytes());
    out.extend_from_slice(&dst_y.to_le_bytes());
    out.push(0); // left-pad
    out.push(depth);
    out.extend_from_slice(&0u16.to_le_bytes()); // unused
    out.extend_from_slice(data);
    out.extend(std::iter::repeat(0u8).take(data_pad));
    out
}

/// Band a full `width x total_height` `ZPixmap` image (`image_data`,
/// `row_stride` bytes per scanline — already padded to the format's
/// `scanline-pad`, matching `encode_put_image`'s own expectation) into a
/// sequence of `PutImage` requests, each one covering a contiguous run of
/// full rows, such that NO single request (header + data, rounded up to
/// its own 4-byte pad) exceeds `max_request_length_words * 4` bytes — the
/// server's advertised `maximum-request-length` (from [`SetupInfo`]).
///
/// `dst_y_base` offsets every band's `dst-y` in the drawable — band `N`
/// (rows `[N*rows_per_band, ...)` of `image_data`) lands at drawable row
/// `dst_y_base + N*rows_per_band`, not row `0`. This lets a caller blit a
/// cropped strip (e.g. a scroll's newly-revealed rows) at an arbitrary
/// window row instead of always at the top; pass `0` to reproduce the
/// original top-of-drawable behavior.
///
/// Total: `row_stride == 0` or `total_height == 0` produces no requests
/// (nothing to send) rather than dividing by zero or looping forever; a
/// `max_request_length_words` too small to fit even ONE row's header+data
/// still makes forward progress (`rows_per_band` is floored at `1`) rather
/// than looping without ever advancing `row`; `dst_y_base` is combined with
/// the running row offset via `saturating_add` so an (unrealistic)
/// caller-supplied offset near `i16::MAX` can't wrap into a bogus negative
/// `dst-y`.
#[allow(clippy::too_many_arguments)]
pub fn put_image_requests(drawable: u32, gc: u32, width: u16, total_height: u16, depth: u8, image_data: &[u8], row_stride: usize, max_request_length_words: u32, dst_y_base: i16) -> Vec<Vec<u8>> {
    if row_stride == 0 || total_height == 0 {
        return Vec::new();
    }

    let max_bytes = (max_request_length_words as usize) * 4;
    let budget = max_bytes.saturating_sub(PUT_IMAGE_HEADER_BYTES);
    let rows_per_band = (budget / row_stride).max(1);

    let mut out = Vec::new();
    let mut row: usize = 0;
    let total_height = total_height as usize;
    while row < total_height {
        let band_rows = rows_per_band.min(total_height - row);
        let start = (row * row_stride).min(image_data.len());
        let end = ((row + band_rows) * row_stride).min(image_data.len());
        let band_data = &image_data[start..end];
        let dst_y = dst_y_base.saturating_add(row.min(i16::MAX as usize) as i16);
        out.push(encode_put_image(drawable, gc, width, band_rows as u16, 0, dst_y, depth, band_data));
        row += band_rows;
    }
    out
}

// =========================================================================
// CopyArea
// =========================================================================

const OP_COPY_AREA: u8 = 62;

/// Encode a `CopyArea` request (opcode 62): server-side copy of a
/// `width x height` rectangle from `(src_x, src_y)` on `src_drawable` to
/// `(dst_x, dst_y)` on `dst_drawable`, using `gc`. Fixed-size, 28 bytes
/// (request length 7 words) — unlike `PutImage` this never carries a
/// caller-supplied data payload, so there is no banding/pad concern here.
///
/// This is the server-side scroll primitive: retained window content that's
/// simply moving up or down a few rows never needs to cross the wire again
/// — only the newly-exposed strip does (see `main.rs`'s `scroll_blit`).
#[allow(clippy::too_many_arguments)]
pub fn encode_copy_area(src_drawable: u32, dst_drawable: u32, gc: u32, src_x: i16, src_y: i16, dst_x: i16, dst_y: i16, width: u16, height: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(28);
    out.push(OP_COPY_AREA);
    out.push(0); // unused
    out.extend_from_slice(&7u16.to_le_bytes()); // request length: 28 bytes / 4
    out.extend_from_slice(&src_drawable.to_le_bytes());
    out.extend_from_slice(&dst_drawable.to_le_bytes());
    out.extend_from_slice(&gc.to_le_bytes());
    out.extend_from_slice(&src_x.to_le_bytes());
    out.extend_from_slice(&src_y.to_le_bytes());
    out.extend_from_slice(&dst_x.to_le_bytes());
    out.extend_from_slice(&dst_y.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out
}

// =========================================================================
// GetKeyboardMapping
// =========================================================================

/// Encode a `GetKeyboardMapping` request (opcode 101).
pub fn encode_get_keyboard_mapping(first_keycode: u8, count: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.push(OP_GET_KEYBOARD_MAPPING);
    out.push(0); // unused
    out.extend_from_slice(&2u16.to_le_bytes());
    out.push(first_keycode);
    out.push(count);
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Parse a `GetKeyboardMapping` reply: `(keysyms_per_keycode, keysyms)`,
/// where `keysyms` is the flat `count * keysyms_per_keycode` list (index
/// `(keycode - min_keycode) * keysyms_per_keycode + column`). Total: a
/// buffer shorter than its own declared reply length is a clean `Err`.
pub fn parse_keyboard_mapping_reply(buf: &[u8]) -> Result<(u8, Vec<u32>), String> {
    let trunc = || "keyboard mapping reply: truncated".to_string();
    if buf.len() < 8 {
        return Err(trunc());
    }
    let keysyms_per_keycode = get_u8(buf, 1).ok_or_else(trunc)?;
    let n = get_u32_le(buf, 4).ok_or_else(trunc)? as usize;

    let mut keysyms = Vec::with_capacity(n);
    for i in 0..n {
        let off = 32 + i * 4;
        keysyms.push(get_u32_le(buf, off).ok_or_else(trunc)?);
    }
    Ok((keysyms_per_keycode, keysyms))
}

/// Look up keycode `keycode`'s first (unshifted) keysym column. `None` if
/// `keycode` is below `min_keycode`, `keysyms_per_keycode` is `0`, or the
/// computed index runs past `keysyms` — never panics/indexes out of bounds.
pub fn keysym_for_keycode(keycode: u8, min_keycode: u8, keysyms_per_keycode: u8, keysyms: &[u32]) -> Option<u32> {
    if keysyms_per_keycode == 0 || keycode < min_keycode {
        return None;
    }
    let row = (keycode - min_keycode) as usize;
    let idx = row.checked_mul(keysyms_per_keycode as usize)?;
    keysyms.get(idx).copied()
}

// =========================================================================
// keysym -> Key
// =========================================================================

/// A decoded keysym, collapsed to exactly the actions this shell needs —
/// deliberately NOT `browser::Key` (that enum is cell/tty-shaped; this one
/// is the pixel shell's own, kept local so `x11.rs` stays self-contained
/// per the packet brief).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    F5,
}

/// Map an X11 keysym (as [`keysym_for_keycode`] returns) to an [`X11Key`],
/// per the packet brief's minimal table: printable ASCII (`0x20..=0x7e`)
/// keysyms equal their own char code; the rest are the standard
/// `keysymdef.h` values for the named keys. `None` for anything else
/// (function keys other than F5, modifiers, ...) — this shell has no use
/// for them.
pub fn keysym_to_key(keysym: u32) -> Option<X11Key> {
    match keysym {
        0x20..=0x7e => char::from_u32(keysym).map(X11Key::Char),
        0xff0d => Some(X11Key::Enter),
        0xff08 => Some(X11Key::Backspace),
        0xff09 => Some(X11Key::Tab),
        0xff1b => Some(X11Key::Escape),
        0xff52 => Some(X11Key::Up),
        0xff54 => Some(X11Key::Down),
        0xff51 => Some(X11Key::Left),
        0xff53 => Some(X11Key::Right),
        0xff55 => Some(X11Key::PageUp),
        0xff56 => Some(X11Key::PageDown),
        0xffc2 => Some(X11Key::F5),
        _ => None,
    }
}

// =========================================================================
// Events
// =========================================================================

/// Every event this client cares about — everything else (KeyRelease,
/// ButtonRelease, MotionNotify, ...) collapses to [`XEvent::Other`], since
/// this shell never asked for those (they're outside `WINDOW_EVENT_MASK`
/// anyway) but a hostile/misbehaving server sending one unsolicited must
/// still parse cleanly rather than panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XEvent {
    KeyPress { keycode: u8, state: u16 },
    ButtonPress { button: u8, x: i16, y: i16 },
    Expose { x: u16, y: u16, w: u16, h: u16, count: u16 },
    ConfigureNotify { width: u16, height: u16 },
    Other,
}

const EVENT_CODE_KEY_PRESS: u8 = 2;
const EVENT_CODE_BUTTON_PRESS: u8 = 4;
const EVENT_CODE_EXPOSE: u8 = 12;
const EVENT_CODE_CONFIGURE_NOTIFY: u8 = 22;

/// Parse one 32-byte X11 event. `None` only for a buffer shorter than 32
/// bytes (never a panic/OOB read) — an unrecognized event CODE still
/// parses, as [`XEvent::Other`], since every core event is exactly 32
/// bytes regardless of type.
/// Decode a 32-byte X ERROR packet (first byte 0) into a human-readable line.
/// These are how a server reports a rejected request (BadWindow, BadMatch,
/// BadValue, ...) — the client used to swallow them silently, so a request the
/// server refused (e.g. CreateWindow/PutImage) produced a black window with no
/// clue why. Printed to stderr by [`read_reply`]/[`XConnection::next_event`].
pub fn describe_x_error(buf: &[u8]) -> String {
    if buf.len() < 11 {
        return "X error (truncated packet)".to_string();
    }
    let code = buf[1];
    let seq = u16::from_le_bytes([buf[2], buf[3]]);
    let bad = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let minor = u16::from_le_bytes([buf[8], buf[9]]);
    let major = buf[10];
    let code_name = match code {
        1 => "BadRequest", 2 => "BadValue", 3 => "BadWindow", 4 => "BadPixmap", 5 => "BadAtom",
        6 => "BadCursor", 7 => "BadFont", 8 => "BadMatch", 9 => "BadDrawable", 10 => "BadAccess",
        11 => "BadAlloc", 12 => "BadColor", 13 => "BadGC", 14 => "BadIDChoice", 15 => "BadName",
        16 => "BadLength", 17 => "BadImplementation", _ => "Bad?",
    };
    let req_name = match major {
        1 => "CreateWindow", 8 => "MapWindow", 42 => "SetInputFocus", 55 => "CreateGC",
        62 => "CopyArea", 72 => "PutImage", 101 => "GetKeyboardMapping", _ => "other",
    };
    format!("{code_name}({code}) on {req_name}(major={major},minor={minor}) seq={seq} bad=0x{bad:08x}")
}

pub fn parse_event(buf: &[u8]) -> Option<XEvent> {
    if buf.len() < 32 {
        return None;
    }
    // High bit marks a synthetic (SendEvent) event -- irrelevant to this
    // client, mask it off before matching the type.
    let code = buf[0] & 0x7f;
    match code {
        EVENT_CODE_KEY_PRESS => Some(XEvent::KeyPress { keycode: buf[1], state: get_u16_le(buf, 28)? }),
        EVENT_CODE_BUTTON_PRESS => Some(XEvent::ButtonPress { button: buf[1], x: get_u16_le(buf, 24)? as i16, y: get_u16_le(buf, 26)? as i16 }),
        EVENT_CODE_EXPOSE => Some(XEvent::Expose {
            x: get_u16_le(buf, 8)?,
            y: get_u16_le(buf, 10)?,
            w: get_u16_le(buf, 12)?,
            h: get_u16_le(buf, 14)?,
            count: get_u16_le(buf, 16)?,
        }),
        EVENT_CODE_CONFIGURE_NOTIFY => Some(XEvent::ConfigureNotify { width: get_u16_le(buf, 20)?, height: get_u16_le(buf, 22)? }),
        _ => Some(XEvent::Other),
    }
}

/// A folded, transport-level intent — the output of coalescing a drained
/// event batch. The `run_x11` loop maps each `XIntent` onto the existing
/// scroll/navigate/repaint decisions. (`Navigate` is NOT here — a click's
/// hit-test happens loop-side against the fragment stream.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XIntent {
    ScrollBy(i32),
    Resize { w: u16, h: u16 },
    Expose { x: u16, y: u16, w: u16, h: u16 },
    Click { x: i16, y: i16 },
    Reload,
    Quit,
}

/// Fold a classified batch: adjacent `ScrollBy` sum into one; adjacent
/// `Expose` union into one bounding box; all but the LAST `Resize` are
/// dropped; `Click`/`Reload`/`Quit` pass through in order (and break scroll/
/// expose runs). Pure — the responsiveness contract lives here.
pub fn coalesce(intents: Vec<XIntent>) -> Vec<XIntent> {
    // Drop every Resize except the last (a resize supersedes earlier sizes).
    let last_resize_idx = intents
        .iter()
        .rposition(|i| matches!(i, XIntent::Resize { .. }));
    let filtered = intents.into_iter().enumerate().filter(move |(idx, i)| {
        !matches!(i, XIntent::Resize { .. }) || Some(*idx) == last_resize_idx
    });

    let mut out: Vec<XIntent> = Vec::new();
    for (_, intent) in filtered {
        match (out.last_mut(), intent) {
            (Some(XIntent::ScrollBy(acc)), XIntent::ScrollBy(d)) => {
                *acc = acc.saturating_add(d);
            }
            (
                Some(XIntent::Expose { x, y, w, h }),
                XIntent::Expose { x: nx, y: ny, w: nw, h: nh },
            ) => {
                let x0 = (*x).min(nx);
                let y0 = (*y).min(ny);
                let x1 = (*x).saturating_add(*w).max(nx.saturating_add(nw));
                let y1 = (*y).saturating_add(*h).max(ny.saturating_add(nh));
                *x = x0;
                *y = y0;
                *w = x1.saturating_sub(x0);
                *h = y1.saturating_sub(y0);
            }
            (_, other) => out.push(other),
        }
    }
    out
}

// =========================================================================
// Pixel hit-test
// =========================================================================

/// Find the `href` of the topmost `Interactive::Link` fragment whose pixel
/// rect contains document-space point `(x, y)` (document space: unscrolled
/// — the caller adds the current scroll offset to the window-space click
/// before calling this, exactly like `backend::fb`'s callers add nothing
/// because fb has no scroll). "Topmost" = LAST matching fragment in paint
/// order (later-painted fragments sit visually on top of earlier ones,
/// same convention `raster::paint` already paints in) — for the common
/// case of non-overlapping links this is just "the" match either way.
///
/// `None` when no link fragment's rect contains the point (including when
/// `fragments` is empty, or nothing under the point is interactive at
/// all).
pub fn hit_test_pixel(fragments: &[Fragment], x: f32, y: f32) -> Option<String> {
    let mut found: Option<&str> = None;
    for f in fragments {
        if let Some(Interactive::Link { href }) = &f.interactive {
            let r = f.rect;
            let within_x = x >= r.origin.x && x < r.origin.x + r.size.w;
            let within_y = y >= r.origin.y && y < r.origin.y + r.size.h;
            if within_x && within_y {
                found = Some(href.as_ref());
            }
        }
    }
    found.map(|s| s.to_string())
}

// =========================================================================
// XConnection: the thin, manually-verified socket-I/O shim
// =========================================================================

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// A connected, handshaken X11 client. Owns the `UnixStream`, the parsed
/// [`SetupInfo`], and the [`IdAllocator`] for this session's resource IDs.
///
/// Not unit-tested (see this module's own top doc comment) — every method
/// here is a thin "encode with the pure functions above, write it, read a
/// reply/event, parse it with the pure functions above" shim. All the
/// actual logic lives in, and is tested via, the free functions above.
pub struct XConnection {
    stream: UnixStream,
    pub setup: SetupInfo,
    ids: IdAllocator,
    /// Events read off the socket while waiting for a *reply* (X interleaves
    /// events and replies on one stream — the MapNotify/Expose from an earlier
    /// MapWindow arrive BEFORE a later request's reply). Drained by
    /// [`Self::next_event`] before it touches the socket again.
    pending: VecDeque<XEvent>,
    /// When `Some`, `send` appends to this buffer instead of writing to the
    /// socket immediately — see [`Self::begin_frame`]/[`Self::end_frame`].
    frame: Option<Vec<u8>>,
}

/// Read 32-byte X packets from `stream`, queueing any *events* (first byte
/// >= 2) into `pending` and skipping *errors* (first byte 0), until a *reply*
/// (first byte 1) arrives; return the full reply (32-byte header + its
/// `length`-word body). This is THE fix for the client's original hang: a
/// reply read that naively grabbed the first 32 bytes would mistake a pending
/// MapNotify/Expose event for the reply header, read its window field as a
/// (huge) body length, and block forever on `read_exact`.
fn read_reply(stream: &mut impl Read, pending: &mut VecDeque<XEvent>) -> Result<Vec<u8>, String> {
    loop {
        let mut hdr = [0u8; 32];
        stream.read_exact(&mut hdr).map_err(|e| format!("read X packet: {e}"))?;
        match hdr[0] {
            1 => {
                let words = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                let mut body = vec![0u8; words * 4];
                stream.read_exact(&mut body).map_err(|e| format!("read X reply body: {e}"))?;
                let mut full = Vec::with_capacity(32 + body.len());
                full.extend_from_slice(&hdr);
                full.extend_from_slice(&body);
                return Ok(full);
            }
            0 => {
                // Error packet (32 bytes, no body). Surface it — a swallowed
                // error here is a rejected request and a blank window.
                eprintln!("stele: --x11: X {}", describe_x_error(&hdr));
                continue;
            }
            _ => {
                if let Some(ev) = parse_event(&hdr) {
                    pending.push_back(ev);
                }
            }
        }
    }
}

impl XConnection {
    /// Connect to the display named by `$DISPLAY` (default `:0`),
    /// authenticate with the `MIT-MAGIC-COOKIE-1` cookie from
    /// `$XAUTHORITY`/`~/.Xauthority` (best-effort: connects with no
    /// authentication at all if neither is found or parseable — some
    /// permissively-configured servers, e.g. `xhost +`, accept that), and
    /// complete the connection-setup handshake.
    pub fn connect() -> Result<XConnection, String> {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        let (host, display_num, _screen) = parse_display(&display)?;
        if !host.is_empty() {
            return Err(format!("only local (Unix-domain-socket) X displays are supported; got DISPLAY={display:?}"));
        }

        let socket_path = format!("/tmp/.X11-unix/X{display_num}");
        let mut stream = UnixStream::connect(&socket_path).map_err(|e| format!("connect {socket_path}: {e}"))?;

        let (auth_name, auth_data) = load_auth(display_num).unwrap_or_default();
        let request = encode_setup_request(&auth_name, &auth_data);
        stream.write_all(&request).map_err(|e| format!("write connection-setup request: {e}"))?;

        let mut header = [0u8; 8];
        stream.read_exact(&mut header).map_err(|e| format!("read connection-setup reply header: {e}"))?;
        let extra_len = (u16::from_le_bytes([header[6], header[7]]) as usize) * 4;
        let mut body = vec![0u8; extra_len];
        stream.read_exact(&mut body).map_err(|e| format!("read connection-setup reply body: {e}"))?;

        let mut full = Vec::with_capacity(8 + extra_len);
        full.extend_from_slice(&header);
        full.extend_from_slice(&body);
        let setup = parse_setup_reply(&full)?;
        let ids = IdAllocator::new(setup.resource_id_base, setup.resource_id_mask);

        Ok(XConnection { stream, setup, ids, pending: VecDeque::new(), frame: None })
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(buf) = self.frame.as_mut() {
            buf.extend_from_slice(bytes);
            Ok(())
        } else {
            self.stream.write_all(bytes).map_err(|e| format!("write to X server: {e}"))
        }
    }

    /// Start buffering requests; pair with `end_frame`. Nesting is a no-op
    /// (an already-open frame keeps accumulating).
    pub fn begin_frame(&mut self) {
        if self.frame.is_none() {
            self.frame = Some(Vec::with_capacity(64 * 1024));
        }
    }

    /// Flush the buffered frame in one write.
    pub fn end_frame(&mut self) -> Result<(), String> {
        if let Some(buf) = self.frame.take() {
            if !buf.is_empty() {
                self.stream.write_all(&buf).map_err(|e| format!("write frame to X server: {e}"))?;
            }
        }
        Ok(())
    }

    /// Find the [`PixmapFormat`] for `depth` (falls back to the FIRST
    /// advertised format if no exact depth match — should never happen
    /// against a real server, since the root's own depth is always in
    /// `formats`, but this stays total rather than panicking either way).
    pub fn format_for_depth(&self, depth: u8) -> Option<PixmapFormat> {
        self.setup.formats.iter().find(|f| f.depth == depth).copied().or_else(|| self.setup.formats.first().copied())
    }

    /// `CreateWindow` a top-level `InputOutput` window on the root
    /// (`self.setup.root`), at the root's own visual/depth, `back-pixel=0`,
    /// event-mask = [`WINDOW_EVENT_MASK`] — then returns its newly
    /// allocated window ID (does NOT map it; call [`Self::map_window`]
    /// separately, per the packet brief's ordering).
    pub fn create_window(&mut self, width: u16, height: u16) -> Result<u32, String> {
        let wid = self.ids.next();
        let root = self.setup.root;
        let depth = self.setup.root_depth;
        let visual = self.setup.root_visual;
        let req = encode_create_window(wid, root, depth, visual, 0, 0, width, height, 0, WINDOW_EVENT_MASK);
        self.send(&req)?;
        Ok(wid)
    }

    pub fn map_window(&mut self, window: u32) -> Result<(), String> {
        self.send(&encode_map_window(window))
    }

    /// Give keyboard focus to `window` — required under a WM-less server
    /// (Xfbdev) so KeyPress events (q/Escape/scroll keys) actually arrive.
    /// Must only be called once `window` is viewable (e.g. after the first
    /// `Expose`) — calling it right after `MapWindow` gets `BadMatch` from a
    /// real server, since the window isn't viewable yet at that point.
    pub fn set_input_focus(&mut self, window: u32) -> Result<(), String> {
        self.send(&encode_set_input_focus(window))
    }

    pub fn create_gc(&mut self, drawable: u32) -> Result<u32, String> {
        let cid = self.ids.next();
        self.send(&encode_create_gc(cid, drawable))?;
        Ok(cid)
    }

    pub fn create_pixmap(&mut self, drawable: u32, depth: u8, width: u16, height: u16) -> Result<u32, String> {
        let pid = self.ids.next();
        self.send(&encode_create_pixmap(pid, drawable, depth, width, height))?;
        Ok(pid)
    }

    pub fn free_pixmap(&mut self, pid: u32) -> Result<(), String> {
        self.send(&encode_free_pixmap(pid))
    }

    /// Blit `image_data` (a `ZPixmap` buffer at `depth`'s native layout,
    /// `row_stride` bytes/scanline) into `drawable` at `(0, 0)`, banding
    /// into as many `PutImage` requests as the server's
    /// `maximum-request-length` demands (see [`put_image_requests`]).
    pub fn put_image(&mut self, drawable: u32, gc: u32, width: u16, height: u16, depth: u8, image_data: &[u8], row_stride: usize) -> Result<(), String> {
        self.put_image_at(drawable, gc, width, height, depth, image_data, row_stride, 0)
    }

    /// Same as [`Self::put_image`], but the image lands at drawable row
    /// `dst_y_base` instead of always `0` — used to blit a cropped scroll
    /// strip at whatever window row it belongs at (see [`put_image_requests`]).
    #[allow(clippy::too_many_arguments)]
    pub fn put_image_at(&mut self, drawable: u32, gc: u32, width: u16, height: u16, depth: u8, image_data: &[u8], row_stride: usize, dst_y_base: i16) -> Result<(), String> {
        let max_len = self.setup.maximum_request_length;
        for req in put_image_requests(drawable, gc, width, height, depth, image_data, row_stride, max_len, dst_y_base) {
            self.send(&req)?;
        }
        Ok(())
    }

    /// Server-side `CopyArea`: copy a `width x height` rectangle from
    /// `(src_x, src_y)` on `src_drawable` to `(dst_x, dst_y)` on
    /// `dst_drawable`. Used to shift retained window content on a scroll
    /// instead of re-sending it (see [`encode_copy_area`]).
    #[allow(clippy::too_many_arguments)]
    pub fn copy_area(&mut self, src_drawable: u32, dst_drawable: u32, gc: u32, src_x: i16, src_y: i16, dst_x: i16, dst_y: i16, width: u16, height: u16) -> Result<(), String> {
        self.send(&encode_copy_area(src_drawable, dst_drawable, gc, src_x, src_y, dst_x, dst_y, width, height))
    }

    /// `GetKeyboardMapping` for every keycode the server advertises
    /// (`min_keycode..=max_keycode`) in one round-trip.
    pub fn get_keyboard_mapping(&mut self) -> Result<(u8, Vec<u32>), String> {
        let min_kc = self.setup.min_keycode;
        let max_kc = self.setup.max_keycode;
        let count = max_kc.saturating_sub(min_kc).saturating_add(1);
        self.send(&encode_get_keyboard_mapping(min_kc, count))?;
        // Demux: events (MapNotify/Expose from the earlier MapWindow) can sit
        // ahead of this reply on the stream — queue them, don't mistake one for
        // the reply header. See `read_reply`.
        let full = read_reply(&mut self.stream, &mut self.pending)?;
        parse_keyboard_mapping_reply(&full)
    }

    /// Block for the next 32-byte event and parse it. A malformed/short
    /// read is a clean `Err` (e.g. the server closed the connection); an
    /// event this client doesn't recognize parses as `Ok(XEvent::Other)`
    /// rather than erroring — see [`parse_event`].
    pub fn next_event(&mut self) -> Result<XEvent, String> {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Ok(ev);
            }
            let mut buf = [0u8; 32];
            self.stream.read_exact(&mut buf).map_err(|e| format!("read X event: {e}"))?;
            match buf[0] {
                // An unexpected reply in the event stream (has a body) — drain
                // its body so we stay byte-aligned, then keep reading.
                1 => {
                    let words = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
                    let mut body = vec![0u8; words * 4];
                    self.stream.read_exact(&mut body).map_err(|e| format!("drain X reply body: {e}"))?;
                }
                0 => {
                    eprintln!("stele: --x11: X {}", describe_x_error(&buf));
                    return Ok(XEvent::Other); // error packet: 32 bytes, no body
                }
                _ => return Ok(parse_event(&buf).unwrap_or(XEvent::Other)),
            }
        }
    }

    /// Block for the next event, then drain every event already queued on the
    /// socket (non-blocking) into one batch. The batch feeds `coalesce`, so a
    /// burst of wheel/resize/expose events collapses to one paint. Returns at
    /// least one event. A short/failed read is a clean `Err`.
    pub fn drain_events(&mut self) -> Result<Vec<XEvent>, String> {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        const ZERO: Timespec = Timespec { tv_sec: 0, tv_nsec: 0 };

        let mut batch = vec![self.next_event()?];
        // First, anything already demuxed into `pending`.
        while let Some(ev) = self.pending.pop_front() {
            batch.push(ev);
        }
        // Then everything sitting on the socket right now.
        loop {
            let mut fds = [PollFd::new(&self.stream, PollFlags::IN)];
            let n = poll(&mut fds, Some(&ZERO)).map_err(|e| format!("poll X socket: {e}"))?;
            if n == 0 || !fds[0].revents().contains(PollFlags::IN) {
                break;
            }
            // A byte is ready; read one whole event (blocking only for the
            // remaining bytes of an event whose first byte already arrived).
            batch.push(self.next_event()?);
        }
        Ok(batch)
    }
}

/// Best-effort `MIT-MAGIC-COOKIE-1` lookup: `$XAUTHORITY` if set, else
/// `~/.Xauthority`; `None` on any failure (unset `$HOME`, unreadable file,
/// no matching entry) — [`XConnection::connect`] falls back to no
/// authentication rather than treating this as fatal, since some servers
/// accept that.
fn load_auth(display_num: u16) -> Option<(String, Vec<u8>)> {
    let path = std::env::var("XAUTHORITY").ok().map(PathBuf::from).or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".Xauthority")))?;
    let data = std::fs::read(&path).ok()?;
    let cookie = parse_xauthority(&data, Some(&display_num.to_string()))?;
    Some(("MIT-MAGIC-COOKIE-1".to_string(), cookie))
}

// =========================================================================
// Tests -- every pure function above, driven by synthetic buffers. No
// socket, no real X server: see this module's top doc comment for why
// XConnection itself carries no dedicated test.
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Point, Rect, Size};

    // --------------------------------------------------------- parse_display

    #[test]
    fn parse_display_plain_local() {
        assert_eq!(parse_display(":0").unwrap(), (String::new(), 0, 0));
    }

    #[test]
    fn parse_display_with_screen() {
        assert_eq!(parse_display(":1.2").unwrap(), (String::new(), 1, 2));
    }

    #[test]
    fn parse_display_unix_prefix_is_local() {
        assert_eq!(parse_display("unix:0.0").unwrap(), (String::new(), 0, 0));
    }

    #[test]
    fn parse_display_rejects_missing_colon() {
        assert!(parse_display("nocolon").is_err());
    }

    #[test]
    fn parse_display_rejects_non_numeric_display() {
        assert!(parse_display(":abc").is_err());
    }

    // ------------------------------------------------------ .Xauthority parse

    /// Build one synthetic `.Xauthority` entry: family(u16 BE) + four
    /// length-prefixed (u16 BE) fields.
    fn xauth_entry(family: u16, address: &[u8], number: &str, name: &str, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&family.to_be_bytes());
        for field in [address, number.as_bytes(), name.as_bytes(), data] {
            out.extend_from_slice(&(field.len() as u16).to_be_bytes());
            out.extend_from_slice(field);
        }
        out
    }

    #[test]
    fn parse_xauthority_extracts_the_cookie_matching_display_number() {
        let cookie0 = [1u8; 16];
        let cookie1 = [2u8; 16];
        let mut buf = xauth_entry(0x0100, b"host", "0", "MIT-MAGIC-COOKIE-1", &cookie0);
        buf.extend_from_slice(&xauth_entry(0x0100, b"host", "1", "MIT-MAGIC-COOKIE-1", &cookie1));

        assert_eq!(parse_xauthority(&buf, Some("1")), Some(cookie1.to_vec()));
        assert_eq!(parse_xauthority(&buf, Some("0")), Some(cookie0.to_vec()));
    }

    #[test]
    fn parse_xauthority_falls_back_to_first_entry_when_no_display_given_or_matched() {
        let cookie0 = [9u8; 16];
        let cookie1 = [8u8; 16];
        let mut buf = xauth_entry(0x0100, b"host", "0", "MIT-MAGIC-COOKIE-1", &cookie0);
        buf.extend_from_slice(&xauth_entry(0x0100, b"host", "1", "MIT-MAGIC-COOKIE-1", &cookie1));

        assert_eq!(parse_xauthority(&buf, None), Some(cookie0.to_vec()));
        assert_eq!(parse_xauthority(&buf, Some("99")), Some(cookie0.to_vec()));
    }

    #[test]
    fn parse_xauthority_skips_non_cookie_entries() {
        let cookie = [7u8; 16];
        let mut buf = xauth_entry(0x0100, b"host", "0", "XDM-AUTHORIZATION-1", &[0u8; 16]);
        buf.extend_from_slice(&xauth_entry(0x0100, b"host", "0", "MIT-MAGIC-COOKIE-1", &cookie));
        assert_eq!(parse_xauthority(&buf, None), Some(cookie.to_vec()));
    }

    #[test]
    fn parse_xauthority_on_empty_or_truncated_buffer_is_none_not_a_panic() {
        assert_eq!(parse_xauthority(&[], None), None);
        let full = xauth_entry(0x0100, b"host", "0", "MIT-MAGIC-COOKIE-1", &[1u8; 16]);
        for cut in 0..full.len() {
            let _ = parse_xauthority(&full[..cut], None); // must not panic
        }
    }

    // ----------------------------------------------------- setup request enc

    #[test]
    fn encode_setup_request_produces_exact_bytes() {
        let out = encode_setup_request("MIT-MAGIC-COOKIE-1", &[0xAAu8; 16]);

        assert_eq!(out[0], 0x6c); // byte-order 'l'
        assert_eq!(out[1], 0); // unused
        assert_eq!(&out[2..4], &11u16.to_le_bytes()); // major
        assert_eq!(&out[4..6], &0u16.to_le_bytes()); // minor
        assert_eq!(&out[6..8], &18u16.to_le_bytes()); // name len
        assert_eq!(&out[8..10], &16u16.to_le_bytes()); // data len
        assert_eq!(&out[10..12], &0u16.to_le_bytes()); // unused

        // name (18 bytes) + 2 pad bytes -> 20
        assert_eq!(&out[12..30], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&out[30..32], &[0u8, 0u8]); // pad

        // data (16 bytes, already a multiple of 4 -> no pad)
        assert_eq!(&out[32..48], &[0xAAu8; 16]);
        assert_eq!(out.len(), 48);
    }

    #[test]
    fn encode_setup_request_with_empty_auth_pads_correctly() {
        let out = encode_setup_request("", &[]);
        assert_eq!(out.len(), 12);
        assert_eq!(&out[6..8], &0u16.to_le_bytes());
        assert_eq!(&out[8..10], &0u16.to_le_bytes());
    }

    // -------------------------------------------------------- setup reply parse

    /// Build a synthetic, well-formed `Success` connection-setup reply with
    /// one screen and `formats.len()` pixmap formats.
    fn synth_setup_reply(base: u32, mask: u32, root: u32, root_visual: u32, root_depth: u8, max_req_len: u16, min_kc: u8, max_kc: u8, formats: &[PixmapFormat]) -> Vec<u8> {
        let vendor = b"Stele Test Vendor";
        let vendor_pad = pad_len(vendor.len());

        let mut extra = Vec::new();
        extra.extend_from_slice(&0u32.to_le_bytes()); // release-number
        extra.extend_from_slice(&base.to_le_bytes());
        extra.extend_from_slice(&mask.to_le_bytes());
        extra.extend_from_slice(&0u32.to_le_bytes()); // motion-buffer-size
        extra.extend_from_slice(&(vendor.len() as u16).to_le_bytes());
        extra.extend_from_slice(&max_req_len.to_le_bytes());
        extra.push(1); // number of ROOTS
        extra.push(formats.len() as u8);
        extra.push(0); // image-byte-order
        extra.push(0); // bitmap-format-bit-order
        extra.push(32); // bitmap-format-scanline-unit
        extra.push(32); // bitmap-format-scanline-pad
        extra.push(min_kc);
        extra.push(max_kc);
        extra.extend_from_slice(&0u32.to_le_bytes()); // unused
        extra.extend_from_slice(vendor);
        extra.extend(std::iter::repeat(0u8).take(vendor_pad));

        for f in formats {
            extra.push(f.depth);
            extra.push(f.bits_per_pixel);
            extra.push(f.scanline_pad);
            extra.extend_from_slice(&[0u8; 5]); // unused
        }

        // SCREEN record (only the fields this parser reads are meaningful;
        // the rest are zeroed).
        extra.extend_from_slice(&root.to_le_bytes());
        extra.extend_from_slice(&0u32.to_le_bytes()); // default-colormap
        extra.extend_from_slice(&0u32.to_le_bytes()); // white-pixel
        extra.extend_from_slice(&0u32.to_le_bytes()); // black-pixel
        extra.extend_from_slice(&0u32.to_le_bytes()); // current-input-masks
        extra.extend_from_slice(&1024u16.to_le_bytes()); // width-in-pixels
        extra.extend_from_slice(&768u16.to_le_bytes()); // height-in-pixels
        extra.extend_from_slice(&0u16.to_le_bytes()); // width-mm
        extra.extend_from_slice(&0u16.to_le_bytes()); // height-mm
        extra.extend_from_slice(&0u16.to_le_bytes()); // min-installed-maps
        extra.extend_from_slice(&0u16.to_le_bytes()); // max-installed-maps
        extra.extend_from_slice(&root_visual.to_le_bytes());
        extra.push(0); // backing-stores
        extra.push(0); // save-unders
        extra.push(root_depth);
        extra.push(0); // number of DEPTHS (none, for this synthetic reply)

        let extra_words = extra.len().div_ceil(4);
        let extra_padded_len = extra_words * 4;
        extra.extend(std::iter::repeat(0u8).take(extra_padded_len - extra.len()));

        let mut out = Vec::with_capacity(8 + extra.len());
        out.push(1); // success
        out.push(0); // unused
        out.extend_from_slice(&11u16.to_le_bytes()); // major
        out.extend_from_slice(&0u16.to_le_bytes()); // minor
        out.extend_from_slice(&(extra_words as u16).to_le_bytes());
        out.extend_from_slice(&extra);
        out
    }

    #[test]
    fn parse_setup_reply_extracts_all_fields() {
        let formats = [PixmapFormat { depth: 24, bits_per_pixel: 32, scanline_pad: 32 }, PixmapFormat { depth: 16, bits_per_pixel: 16, scanline_pad: 16 }];
        let buf = synth_setup_reply(0x0040_0000, 0x001F_FFFF, 0x0000_0042, 0x0000_0099, 24, 65535, 8, 255, &formats);

        let info = parse_setup_reply(&buf).expect("well-formed reply parses");
        assert_eq!(info.resource_id_base, 0x0040_0000);
        assert_eq!(info.resource_id_mask, 0x001F_FFFF);
        assert_eq!(info.root, 0x0000_0042);
        assert_eq!(info.root_visual, 0x0000_0099);
        assert_eq!(info.root_depth, 24);
        assert_eq!(info.maximum_request_length, 65535);
        assert_eq!(info.min_keycode, 8);
        assert_eq!(info.max_keycode, 255);
        assert_eq!(info.formats, formats.to_vec());
    }

    #[test]
    fn parse_setup_reply_failed_connection_is_a_descriptive_err() {
        let reason = b"not authorized";
        let mut out = Vec::new();
        out.push(0); // Failed
        out.push(reason.len() as u8);
        out.extend_from_slice(&11u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(reason.len().div_ceil(4) as u16).to_le_bytes());
        out.extend_from_slice(reason);
        out.extend(std::iter::repeat(0u8).take(pad_len(reason.len())));

        let err = parse_setup_reply(&out).unwrap_err();
        assert!(err.contains("not authorized"), "{err}");
    }

    #[test]
    fn parse_setup_reply_authenticate_status_is_an_err() {
        let out = [2u8, 0, 0, 0, 0, 0, 0, 0];
        assert!(parse_setup_reply(&out).is_err());
    }

    #[test]
    fn parse_setup_reply_truncated_buffer_never_panics() {
        let formats = [PixmapFormat { depth: 24, bits_per_pixel: 32, scanline_pad: 32 }];
        let full = synth_setup_reply(1, 1, 1, 1, 24, 1, 8, 255, &formats);
        for cut in 0..full.len() {
            let _ = parse_setup_reply(&full[..cut]); // must not panic
        }
    }

    // ------------------------------------------------------------- IdAllocator

    #[test]
    fn id_allocator_masks_and_increments() {
        let mut ids = IdAllocator::new(0x0040_0000, 0x001F_FFFF);
        assert_eq!(ids.next(), 0x0040_0000);
        assert_eq!(ids.next(), 0x0040_0001);
        assert_eq!(ids.next(), 0x0040_0002);
    }

    #[test]
    fn id_allocator_wraps_within_mask() {
        let mut ids = IdAllocator::new(0x1000_0000, 0x0000_0001);
        assert_eq!(ids.next(), 0x1000_0000); // counter 0 & mask 1 = 0
        assert_eq!(ids.next(), 0x1000_0001); // counter 1 & mask 1 = 1
        assert_eq!(ids.next(), 0x1000_0000); // counter 2 & mask 1 = 0 again
    }

    // ------------------------------------------------------ CreateWindow enc

    #[test]
    fn encode_create_window_produces_correct_opcode_length_and_fields() {
        let out = encode_create_window(0x0040_0001, 0x0000_0042, 24, 0x0000_0099, 0, 0, 800, 600, 0, WINDOW_EVENT_MASK);

        assert_eq!(out[0], 1); // opcode
        assert_eq!(out[1], 24); // depth
        assert_eq!(&out[2..4], &10u16.to_le_bytes()); // length: 8 + 2 value words
        assert_eq!(&out[4..8], &0x0040_0001u32.to_le_bytes()); // wid
        assert_eq!(&out[8..12], &0x0000_0042u32.to_le_bytes()); // parent
        assert_eq!(&out[12..14], &0i16.to_le_bytes()); // x
        assert_eq!(&out[14..16], &0i16.to_le_bytes()); // y
        assert_eq!(&out[16..18], &800u16.to_le_bytes()); // width
        assert_eq!(&out[18..20], &600u16.to_le_bytes()); // height
        assert_eq!(&out[20..22], &0u16.to_le_bytes()); // border-width
        assert_eq!(&out[22..24], &1u16.to_le_bytes()); // class InputOutput
        assert_eq!(&out[24..28], &0x0000_0099u32.to_le_bytes()); // visual
        assert_eq!(&out[28..32], &(CW_BACK_PIXEL | CW_EVENT_MASK).to_le_bytes()); // value-mask
        assert_eq!(&out[32..36], &0u32.to_le_bytes()); // back-pixel value
        assert_eq!(&out[36..40], &WINDOW_EVENT_MASK.to_le_bytes()); // event-mask value
        assert_eq!(out.len(), 40);
    }

    #[test]
    fn encode_set_input_focus_produces_correct_bytes() {
        // opcode 42, revert-to Parent (2), length 3, window, CurrentTime (0)
        assert_eq!(
            encode_set_input_focus(0x0140_0001),
            vec![42, 2, 3, 0, 0x01, 0x00, 0x40, 0x01, 0, 0, 0, 0]
        );
    }

    #[test]
    fn describe_x_error_decodes_a_badmatch_on_createwindow() {
        let mut e = [0u8; 32];
        e[0] = 0; // error
        e[1] = 8; // BadMatch
        e[2..4].copy_from_slice(&1u16.to_le_bytes()); // seq
        e[4..8].copy_from_slice(&0x0040_0001u32.to_le_bytes()); // bad value
        e[10] = 1; // major opcode = CreateWindow
        let s = describe_x_error(&e);
        assert!(s.contains("BadMatch"), "{s}");
        assert!(s.contains("CreateWindow"), "{s}");
    }

    #[test]
    fn read_reply_queues_events_that_precede_the_reply() {
        // Reproduces the original hang: a reply preceded on the stream by the
        // MapNotify/Expose events an earlier MapWindow generated. The MapNotify
        // packet's bytes[4..8] are huge (a window id) — the OLD code read those
        // as a reply body length and blocked on a ~16MB read_exact forever.
        use std::io::Cursor;
        let mut wire = Vec::new();
        let mut expose = [0u8; 32];
        expose[0] = EVENT_CODE_EXPOSE; // 12
        wire.extend_from_slice(&expose);
        let mut mapnotify = [0u8; 32];
        mapnotify[0] = 19; // MapNotify (unhandled -> Other), huge "length" field:
        mapnotify[4..8].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());
        wire.extend_from_slice(&mapnotify);
        let mut reply = [0u8; 32];
        reply[0] = 1; // reply
        reply[4..8].copy_from_slice(&1u32.to_le_bytes()); // 1 word = 4 body bytes
        wire.extend_from_slice(&reply);
        wire.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);

        let mut cur = Cursor::new(wire);
        let mut pending = VecDeque::new();
        let full = read_reply(&mut cur, &mut pending).unwrap();
        assert_eq!(full.len(), 36, "reply = 32 header + 4 body");
        assert_eq!(full[0], 1);
        assert_eq!(&full[32..36], &[0xaa, 0xbb, 0xcc, 0xdd]);
        // Both preceding events were queued (not swallowed / misread):
        assert_eq!(pending.pop_front(), Some(XEvent::Expose { x: 0, y: 0, w: 0, h: 0, count: 0 }));
        assert_eq!(pending.pop_front(), Some(XEvent::Other)); // MapNotify
        assert!(pending.is_empty());
    }

    #[test]
    fn encode_map_window_produces_correct_bytes() {
        let out = encode_map_window(0x0040_0001);
        assert_eq!(out[0], 8);
        assert_eq!(&out[2..4], &2u16.to_le_bytes());
        assert_eq!(&out[4..8], &0x0040_0001u32.to_le_bytes());
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn encode_create_gc_disables_graphics_exposures() {
        let out = encode_create_gc(0x0040_0002, 0x0040_0001);
        assert_eq!(out[0], 55);
        assert_eq!(&out[2..4], &5u16.to_le_bytes()); // request length = 5 words
        assert_eq!(&out[4..8], &0x0040_0002u32.to_le_bytes()); // cid
        assert_eq!(&out[8..12], &0x0040_0001u32.to_le_bytes()); // drawable
        assert_eq!(&out[12..16], &0x0001_0000u32.to_le_bytes()); // value-mask: graphics-exposures
        assert_eq!(&out[16..20], &0u32.to_le_bytes()); // value: FALSE
        assert_eq!(out.len(), 20);
    }

    #[test]
    fn encode_create_pixmap_produces_correct_bytes() {
        let out = encode_create_pixmap(0x0040_0003, 0x0040_0001, 24, 1024, 768);
        assert_eq!(out[0], 53);
        assert_eq!(out[1], 24); // depth
        assert_eq!(&out[2..4], &4u16.to_le_bytes()); // length = 4 words
        assert_eq!(&out[4..8], &0x0040_0003u32.to_le_bytes()); // pid
        assert_eq!(&out[8..12], &0x0040_0001u32.to_le_bytes()); // drawable
        assert_eq!(&out[12..14], &1024u16.to_le_bytes()); // width
        assert_eq!(&out[14..16], &768u16.to_le_bytes()); // height
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn encode_free_pixmap_produces_correct_bytes() {
        let out = encode_free_pixmap(0x0040_0003);
        assert_eq!(out[0], 54);
        assert_eq!(&out[2..4], &2u16.to_le_bytes()); // length = 2 words
        assert_eq!(&out[4..8], &0x0040_0003u32.to_le_bytes()); // pixmap
        assert_eq!(out.len(), 8);
    }

    // ------------------------------------------------------------- PutImage

    #[test]
    fn encode_put_image_produces_correct_header_and_padded_data() {
        let data = vec![1u8, 2, 3, 4, 5]; // 5 bytes -> 3 pad bytes
        let out = encode_put_image(0x0040_0001, 0x0040_0002, 2, 1, 0, 5, 24, &data);

        assert_eq!(out[0], 72); // opcode
        assert_eq!(out[1], PUT_IMAGE_FORMAT_ZPIXMAP);
        let expected_words = ((24 + 5 + 3) / 4) as u16;
        assert_eq!(&out[2..4], &expected_words.to_le_bytes());
        assert_eq!(&out[4..8], &0x0040_0001u32.to_le_bytes()); // drawable
        assert_eq!(&out[8..12], &0x0040_0002u32.to_le_bytes()); // gc
        assert_eq!(&out[12..14], &2u16.to_le_bytes()); // width
        assert_eq!(&out[14..16], &1u16.to_le_bytes()); // height
        assert_eq!(&out[16..18], &0i16.to_le_bytes()); // dst-x
        assert_eq!(&out[18..20], &5i16.to_le_bytes()); // dst-y
        assert_eq!(out[20], 0); // left-pad
        assert_eq!(out[21], 24); // depth
        assert_eq!(&out[24..29], &data[..]);
        assert_eq!(&out[29..32], &[0u8; 3]); // request pad
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn put_image_requests_bands_a_large_image_under_the_request_limit() {
        // 10 rows, 16 bytes/row (160 bytes total) -- force a small
        // max-request-length so it must band into multiple requests.
        let row_stride = 16usize;
        let total_height = 10u16;
        let mut data = Vec::with_capacity(row_stride * total_height as usize);
        for row in 0..total_height as u8 {
            data.extend(std::iter::repeat(row).take(row_stride));
        }

        // max-request-length small enough that only ~2 rows fit per
        // request (24-byte header + 2*16 = 56 bytes -> 14 words; give 15
        // words of budget so exactly 2 rows fit, never 3: 24+3*16=72B=18w > 15w).
        let max_words = 15u32;
        let max_bytes = max_words as usize * 4;

        let requests = put_image_requests(0x1, 0x2, 4, total_height, 24, &data, row_stride, max_words, 0);

        assert!(requests.len() > 1, "expected banding into multiple requests");

        let mut reconstructed = Vec::new();
        let mut expected_dst_y: i16 = 0;
        for req in &requests {
            // Every request must fit under the server's limit.
            assert!(req.len() <= max_bytes, "request of {} bytes exceeds max {max_bytes}", req.len());
            // Parse back the header fields to confirm banding correctness.
            let dst_y = i16::from_le_bytes([req[18], req[19]]);
            assert_eq!(dst_y, expected_dst_y);
            let height = u16::from_le_bytes([req[14], req[15]]);
            expected_dst_y += height as i16;
            let data_start = 24;
            let data_len = (height as usize) * row_stride;
            reconstructed.extend_from_slice(&req[data_start..data_start + data_len]);
        }
        assert_eq!(reconstructed, data, "banded requests must reconstruct the original image byte-for-byte");
        assert_eq!(expected_dst_y as u16, total_height);
    }

    #[test]
    fn put_image_requests_single_band_when_it_fits() {
        let row_stride = 8usize;
        let data = vec![0xAAu8; row_stride * 4];
        let requests = put_image_requests(0x1, 0x2, 2, 4, 24, &data, row_stride, 4096, 0);
        assert_eq!(requests.len(), 1);
    }

    #[test]
    fn put_image_requests_zero_stride_or_height_produces_no_requests() {
        assert!(put_image_requests(1, 2, 4, 4, 24, &[0u8; 16], 0, 4096, 0).is_empty());
        assert!(put_image_requests(1, 2, 4, 0, 24, &[], 8, 4096, 0).is_empty());
    }

    #[test]
    fn put_image_requests_pathologically_small_limit_still_makes_progress() {
        // Even a max-request-length that can't fit a single row must still
        // terminate (rows_per_band floored at 1), not loop forever.
        let row_stride = 100usize;
        let data = vec![0u8; row_stride * 3];
        let requests = put_image_requests(1, 2, 25, 3, 24, &data, row_stride, 1, 0);
        assert_eq!(requests.len(), 3); // one row per request
    }

    #[test]
    fn put_image_requests_dst_y_base_offsets_every_band() {
        // Same banding shape as the "bands a large image" test above, but
        // with a non-zero dst_y_base: every band's dst-y must land at
        // `dst_y_base + <row offset within the image>`, not `<row offset>`
        // alone -- this is what lets a scroll strip land partway down the
        // window instead of always at the top.
        let row_stride = 16usize;
        let total_height = 10u16;
        let data = vec![0u8; row_stride * total_height as usize];
        let max_words = 15u32; // same budget as above: ~2 rows/band
        let dst_y_base: i16 = 500;

        let requests = put_image_requests(0x1, 0x2, 4, total_height, 24, &data, row_stride, max_words, dst_y_base);
        assert!(requests.len() > 1, "expected banding into multiple requests");

        let mut expected_dst_y = dst_y_base;
        for req in &requests {
            let dst_y = i16::from_le_bytes([req[18], req[19]]);
            assert_eq!(dst_y, expected_dst_y);
            let height = u16::from_le_bytes([req[14], req[15]]);
            expected_dst_y += height as i16;
        }
    }

    #[test]
    fn put_image_requests_dst_y_base_zero_matches_old_top_of_drawable_behavior() {
        let row_stride = 8usize;
        let data = vec![0xAAu8; row_stride * 4];
        let requests = put_image_requests(0x1, 0x2, 2, 4, 24, &data, row_stride, 4096, 0);
        assert_eq!(requests.len(), 1);
        let dst_y = i16::from_le_bytes([requests[0][18], requests[0][19]]);
        assert_eq!(dst_y, 0);
    }

    // -------------------------------------------------------------- CopyArea

    #[test]
    fn encode_copy_area_produces_exact_bytes() {
        let out = encode_copy_area(0x0040_0001, 0x0040_0001, 0x0040_0002, 0, 60, 0, 0, 1024, 708);

        assert_eq!(out[0], 62); // opcode
        assert_eq!(out[1], 0); // unused
        assert_eq!(&out[2..4], &7u16.to_le_bytes()); // request length
        assert_eq!(&out[4..8], &0x0040_0001u32.to_le_bytes()); // src-drawable
        assert_eq!(&out[8..12], &0x0040_0001u32.to_le_bytes()); // dst-drawable
        assert_eq!(&out[12..16], &0x0040_0002u32.to_le_bytes()); // gc
        assert_eq!(&out[16..18], &0i16.to_le_bytes()); // src-x
        assert_eq!(&out[18..20], &60i16.to_le_bytes()); // src-y
        assert_eq!(&out[20..22], &0i16.to_le_bytes()); // dst-x
        assert_eq!(&out[22..24], &0i16.to_le_bytes()); // dst-y
        assert_eq!(&out[24..26], &1024u16.to_le_bytes()); // width
        assert_eq!(&out[26..28], &708u16.to_le_bytes()); // height
        assert_eq!(out.len(), 28);
    }

    #[test]
    fn encode_copy_area_negative_coordinates_round_trip() {
        // src/dst coordinates are signed per spec (a drawable can be copied
        // from/to a point that's technically off-window); confirm negative
        // values encode losslessly rather than being silently clamped.
        let out = encode_copy_area(1, 2, 3, -5, -10, 7, 0, 1, 1);
        assert_eq!(i16::from_le_bytes([out[16], out[17]]), -5);
        assert_eq!(i16::from_le_bytes([out[18], out[19]]), -10);
        assert_eq!(i16::from_le_bytes([out[20], out[21]]), 7);
    }

    // ------------------------------------------------------ GetKeyboardMapping

    #[test]
    fn encode_get_keyboard_mapping_produces_correct_bytes() {
        let out = encode_get_keyboard_mapping(8, 248);
        assert_eq!(out[0], 101);
        assert_eq!(&out[2..4], &2u16.to_le_bytes());
        assert_eq!(out[4], 8);
        assert_eq!(out[5], 248);
        assert_eq!(out.len(), 8);
    }

    #[test]
    fn parse_keyboard_mapping_reply_extracts_keysyms() {
        let keysyms_per_keycode = 2u8;
        let keysyms: Vec<u32> = vec![0x61, 0x41, 0x62, 0x42]; // 'a','A','b','B'
        let mut buf = Vec::new();
        buf.push(1); // reply
        buf.push(keysyms_per_keycode);
        buf.extend_from_slice(&0u16.to_le_bytes()); // sequence number
        buf.extend_from_slice(&(keysyms.len() as u32).to_le_bytes()); // reply length
        buf.extend_from_slice(&[0u8; 24]); // unused
        for k in &keysyms {
            buf.extend_from_slice(&k.to_le_bytes());
        }

        let (per, out_syms) = parse_keyboard_mapping_reply(&buf).expect("well-formed reply parses");
        assert_eq!(per, 2);
        assert_eq!(out_syms, keysyms);
    }

    #[test]
    fn parse_keyboard_mapping_reply_truncated_is_a_clean_err() {
        assert!(parse_keyboard_mapping_reply(&[1, 2, 0, 0, 5, 0, 0, 0]).is_err());
    }

    #[test]
    fn keysym_for_keycode_indexes_correctly() {
        let keysyms = vec![0x61, 0x41, 0x62, 0x42]; // keycode 8 -> [a,A], keycode 9 -> [b,B]
        assert_eq!(keysym_for_keycode(8, 8, 2, &keysyms), Some(0x61));
        assert_eq!(keysym_for_keycode(9, 8, 2, &keysyms), Some(0x62));
        assert_eq!(keysym_for_keycode(7, 8, 2, &keysyms), None); // below min
        assert_eq!(keysym_for_keycode(20, 8, 2, &keysyms), None); // past the end
    }

    // ------------------------------------------------------------ keysym->Key

    #[test]
    fn keysym_to_key_maps_printable_ascii() {
        assert_eq!(keysym_to_key(0x61), Some(X11Key::Char('a'))); // 'a'
        assert_eq!(keysym_to_key(0x20), Some(X11Key::Char(' ')));
        assert_eq!(keysym_to_key(0x7e), Some(X11Key::Char('~')));
    }

    #[test]
    fn keysym_to_key_maps_named_keys() {
        assert_eq!(keysym_to_key(0xff0d), Some(X11Key::Enter));
        assert_eq!(keysym_to_key(0xff08), Some(X11Key::Backspace));
        assert_eq!(keysym_to_key(0xff09), Some(X11Key::Tab));
        assert_eq!(keysym_to_key(0xff1b), Some(X11Key::Escape));
        assert_eq!(keysym_to_key(0xff52), Some(X11Key::Up));
        assert_eq!(keysym_to_key(0xff54), Some(X11Key::Down));
        assert_eq!(keysym_to_key(0xff51), Some(X11Key::Left));
        assert_eq!(keysym_to_key(0xff53), Some(X11Key::Right));
        assert_eq!(keysym_to_key(0xff55), Some(X11Key::PageUp));
        assert_eq!(keysym_to_key(0xff56), Some(X11Key::PageDown));
        assert_eq!(keysym_to_key(0xffc2), Some(X11Key::F5));
    }

    #[test]
    fn keysym_to_key_unknown_is_none() {
        assert_eq!(keysym_to_key(0x0), None);
        assert_eq!(keysym_to_key(0xdead), None);
    }

    // -------------------------------------------------------------- events

    fn synth_event(code: u8, byte1: u8, fields: &[(usize, &[u8])]) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[0] = code;
        buf[1] = byte1;
        for (off, bytes) in fields {
            buf[*off..*off + bytes.len()].copy_from_slice(bytes);
        }
        buf
    }

    #[test]
    fn parse_event_key_press() {
        let buf = synth_event(2, 38, &[(28, &100u16.to_le_bytes())]); // keycode 38, state 100
        assert_eq!(parse_event(&buf), Some(XEvent::KeyPress { keycode: 38, state: 100 }));
    }

    #[test]
    fn parse_event_button_press() {
        let buf = synth_event(4, 1, &[(24, &42u16.to_le_bytes()), (26, &99u16.to_le_bytes())]);
        assert_eq!(parse_event(&buf), Some(XEvent::ButtonPress { button: 1, x: 42, y: 99 }));
    }

    #[test]
    fn parse_event_expose() {
        let buf = synth_event(12, 0, &[]);
        assert_eq!(parse_event(&buf), Some(XEvent::Expose { x: 0, y: 0, w: 0, h: 0, count: 0 }));
    }

    #[test]
    fn parse_event_expose_carries_region_and_count() {
        // Expose event (code 12): x@8, y@10, width@12, height@14, count@16.
        let mut buf = [0u8; 32];
        buf[0] = 12;
        buf[8..10].copy_from_slice(&40u16.to_le_bytes());   // x
        buf[10..12].copy_from_slice(&50u16.to_le_bytes());  // y
        buf[12..14].copy_from_slice(&300u16.to_le_bytes()); // width
        buf[14..16].copy_from_slice(&200u16.to_le_bytes()); // height
        buf[16..18].copy_from_slice(&3u16.to_le_bytes());   // count
        assert_eq!(
            parse_event(&buf),
            Some(XEvent::Expose { x: 40, y: 50, w: 300, h: 200, count: 3 })
        );
    }

    #[test]
    fn parse_event_configure_notify() {
        let buf = synth_event(22, 0, &[(20, &640u16.to_le_bytes()), (22, &480u16.to_le_bytes())]);
        assert_eq!(parse_event(&buf), Some(XEvent::ConfigureNotify { width: 640, height: 480 }));
    }

    #[test]
    fn parse_event_masks_off_the_send_event_bit() {
        let buf = synth_event(12 | 0x80, 0, &[]);
        assert_eq!(parse_event(&buf), Some(XEvent::Expose { x: 0, y: 0, w: 0, h: 0, count: 0 }));
    }

    #[test]
    fn parse_event_unknown_code_is_other_not_none() {
        let buf = synth_event(200, 0, &[]);
        assert_eq!(parse_event(&buf), Some(XEvent::Other));
    }

    #[test]
    fn parse_event_short_buffer_is_none() {
        assert_eq!(parse_event(&[1, 2, 3]), None);
    }

    // -------------------------------------------------------------- coalesce

    #[test]
    fn coalesce_sums_a_wheel_storm_into_one_scroll() {
        let batch = vec![XIntent::ScrollBy(60); 50];
        assert_eq!(coalesce(batch), vec![XIntent::ScrollBy(3000)]);
    }

    #[test]
    fn coalesce_preserves_click_order_between_scroll_runs() {
        let batch = vec![
            XIntent::ScrollBy(60), XIntent::ScrollBy(60),
            XIntent::Click { x: 10, y: 20 },
            XIntent::ScrollBy(-60), XIntent::ScrollBy(-60),
        ];
        assert_eq!(
            coalesce(batch),
            vec![XIntent::ScrollBy(120), XIntent::Click { x: 10, y: 20 }, XIntent::ScrollBy(-120)]
        );
    }

    #[test]
    fn coalesce_keeps_only_the_last_resize() {
        let batch = vec![
            XIntent::Resize { w: 800, h: 600 },
            XIntent::Resize { w: 900, h: 650 },
            XIntent::Resize { w: 1024, h: 768 },
        ];
        assert_eq!(coalesce(batch), vec![XIntent::Resize { w: 1024, h: 768 }]);
    }

    #[test]
    fn coalesce_unions_an_expose_series() {
        let batch = vec![
            XIntent::Expose { x: 10, y: 10, w: 20, h: 20 }, // covers (10,10)-(30,30)
            XIntent::Expose { x: 50, y: 5,  w: 10, h: 40 }, // covers (50,5)-(60,45)
        ];
        // Union bounding box: x 10..60, y 5..45 => x=10,y=5,w=50,h=40.
        assert_eq!(coalesce(batch), vec![XIntent::Expose { x: 10, y: 5, w: 50, h: 40 }]);
    }

    #[test]
    fn coalesce_expose_union_saturates_on_huge_coords_no_panic() {
        // A hostile/buggy server could send Expose coords whose x+w overflows
        // u16; the union must saturate, not panic (totality).
        let batch = vec![
            XIntent::Expose { x: 60000, y: 60000, w: 60000, h: 60000 },
            XIntent::Expose { x: 0, y: 0, w: 10, h: 10 },
        ];
        let out = coalesce(batch);
        assert_eq!(out, vec![XIntent::Expose { x: 0, y: 0, w: 65535, h: 65535 }]);
    }

    #[test]
    fn coalesce_scroll_run_then_quit_passes_quit_through() {
        let batch = vec![XIntent::ScrollBy(60), XIntent::ScrollBy(60), XIntent::Quit];
        assert_eq!(coalesce(batch), vec![XIntent::ScrollBy(120), XIntent::Quit]);
    }

    // --------------------------------------------------------- hit_test_pixel

    fn link_fragment(x: f32, y: f32, w: f32, h: f32, href: &str) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: crate::layout::FragmentKind::Box { style: crate::style::ComputedStyle::default() },
            interactive: Some(Interactive::Link { href: href.into() }), clip: None,
        }
    }

    fn plain_fragment(x: f32, y: f32, w: f32, h: f32) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: crate::layout::FragmentKind::Box { style: crate::style::ComputedStyle::default() },
            interactive: None, clip: None,
        }
    }

    #[test]
    fn hit_test_pixel_inside_link_rect_returns_href() {
        let fragments = vec![plain_fragment(0.0, 0.0, 800.0, 20.0), link_fragment(10.0, 30.0, 100.0, 20.0, "/about")];
        assert_eq!(hit_test_pixel(&fragments, 50.0, 40.0), Some("/about".to_string()));
    }

    #[test]
    fn hit_test_pixel_outside_link_rect_returns_none() {
        let fragments = vec![link_fragment(10.0, 30.0, 100.0, 20.0, "/about")];
        assert_eq!(hit_test_pixel(&fragments, 5.0, 40.0), None); // left of rect
        assert_eq!(hit_test_pixel(&fragments, 50.0, 60.0), None); // below rect
        assert_eq!(hit_test_pixel(&fragments, 200.0, 40.0), None); // right of rect
    }

    #[test]
    fn hit_test_pixel_on_rect_edges() {
        let fragments = vec![link_fragment(10.0, 30.0, 100.0, 20.0, "/edge")];
        assert_eq!(hit_test_pixel(&fragments, 10.0, 30.0), Some("/edge".to_string())); // top-left inclusive
        assert_eq!(hit_test_pixel(&fragments, 109.9, 49.9), Some("/edge".to_string())); // just inside bottom-right
        assert_eq!(hit_test_pixel(&fragments, 110.0, 30.0), None); // right edge exclusive
        assert_eq!(hit_test_pixel(&fragments, 10.0, 50.0), None); // bottom edge exclusive
    }

    #[test]
    fn hit_test_pixel_no_interactive_fragments_returns_none() {
        let fragments = vec![plain_fragment(0.0, 0.0, 100.0, 100.0)];
        assert_eq!(hit_test_pixel(&fragments, 50.0, 50.0), None);
    }

    #[test]
    fn hit_test_pixel_overlapping_links_returns_the_topmost_paint_order() {
        // Later fragment (painted on top) wins when both rects overlap the
        // click point.
        let fragments = vec![link_fragment(0.0, 0.0, 100.0, 100.0, "/behind"), link_fragment(0.0, 0.0, 100.0, 100.0, "/front")];
        assert_eq!(hit_test_pixel(&fragments, 50.0, 50.0), Some("/front".to_string()));
    }

    #[test]
    fn hit_test_pixel_empty_fragments_returns_none() {
        assert_eq!(hit_test_pixel(&[], 0.0, 0.0), None);
    }
}
