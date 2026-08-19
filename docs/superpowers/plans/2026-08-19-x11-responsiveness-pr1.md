# X11 Responsiveness — PR 1 (T1–T4 + T6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `--x11` scrolling/resizing responsive under both cost models (Xfbdev and Xwayland/WSLg) by batching input, keeping frames in a server-side pixmap, batching output, and never re-fetching on resize.

**Architecture:** Split the event loop into a **pure decision layer** (`coalesce`, byte-exact encoders, `reflow_from_dom` — all CI-tested) and a **thin impure shell** (`drain_events`, the pixmap render wiring — verified by encoder tests + a non-gating xvfb smoke + operator testimonial). No timers, no async: paint immediately, once, per drained batch.

**Tech Stack:** Rust std + `rustix::event::poll` (already used by the tty shell in `main.rs`) for the non-blocking drain. Core X11 protocol only (no libX11).

**Spec:** `docs/superpowers/specs/2026-08-19-x11-responsiveness-design.md` (read alongside).

## Global Constraints

- **`--x11` only.** Do NOT touch `fb.rs`/`--fb` (one-shot render, out of scope). Do NOT touch the tty shell loop.
- **No change to paint output.** The A5 PNG surface goldens (`goldens/*.png` from the headless `dump_png` path) must stay **byte-identical** — this PR changes *how* pixels reach the window, never *what* is painted. The headless dump path is independent of `--x11`, so it should be untouched; if any A5 golden moves, that is a bug, not a re-bless.
- **No local builds** (AGENTS.md §3): implementers transcribe verbatim code + commit; CI compiles and runs `cargo test`. The X11 window path has no server in CI — its correctness rests on the byte-exact encoder tests, the pure `coalesce`/`reflow` tests, the optional xvfb smoke, and operator testimonial. State this; don't claim CI proves window pixels.
- **No timers, no frame pacing, no async.** Paint immediately, once, per drained batch.
- **Totality:** a short/malformed event read stays a clean `Err` (loop exits); `coalesce` and the encoders never panic.
- **Branch:** `packet/x11-responsiveness`, off `main`. Conventional subjects (`feat(x11):`, `test(x11):`, `refactor(x11):`, `perf(x11):`).
- **Size:** report the i486 delta from the CI `stele-i486` artifact (new encoders + fold + frame buffer add a little; measure).

## File Structure

- **Modify** `src/backend/x11.rs` — `XEvent::Expose` gains fields; `parse_event` parses them; new `XIntent` enum + pure `coalesce`; new byte-exact encoders `encode_create_pixmap`/`encode_free_pixmap` + `encode_create_gc` graphics-exposures=FALSE; new `XConnection` methods `drain_events`, `create_pixmap`, `free_pixmap`, and frame-buffering (`begin_frame`/`end_frame`). One file; it already owns all X-protocol concerns.
- **Modify** `src/main.rs` — `run_x11` uses `drain_events` → `classify` → `coalesce`; the redraw helpers (`x11_full_redraw`, `x11_scroll_to`) route through a server-side pixmap; `render_x11_page` splits into navigation + `reflow_from_dom`; a session cache struct; `--stats`/debug counters.
- **Create** `.github/workflows/` xvfb smoke step (or extend the existing workflow) — non-gating.
- **Modify** `JOURNAL.md` — packet note + before/after storm table.

---

### Task 1: Full `Expose` parse (F3)

Turn `Expose` from a unit variant into a real event carrying its region and `count`, so a series can be coalesced into one repaint. Pure + byte-exact tested.

**Files:** Modify `src/backend/x11.rs`; Modify `src/main.rs` (the one Expose match arm).

**Interfaces:**
- Produces: `XEvent::Expose { x: u16, y: u16, w: u16, h: u16, count: u16 }`.

- [ ] **Step 1: Write the failing byte-exact parse test**

Add to `x11.rs`'s `#[cfg(test)]` module (mirror `parse_event_configure_notify`):

```rust
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
```

- [ ] **Step 2: Verify it fails** — CI: `cargo test --lib backend::x11` → FAIL (Expose is a unit variant; the struct pattern won't compile).

- [ ] **Step 3: Change the variant + parser**

In `x11.rs`, change `Expose,` in `enum XEvent` to:

```rust
    Expose { x: u16, y: u16, w: u16, h: u16, count: u16 },
```

And the parse arm `EVENT_CODE_EXPOSE => Some(XEvent::Expose),` to:

```rust
        EVENT_CODE_EXPOSE => Some(XEvent::Expose {
            x: get_u16_le(buf, 8)?,
            y: get_u16_le(buf, 10)?,
            w: get_u16_le(buf, 12)?,
            h: get_u16_le(buf, 14)?,
            count: get_u16_le(buf, 16)?,
        }),
```

- [ ] **Step 4: Fix the one consumer in `main.rs`**

`run_x11`'s `xproto::XEvent::Expose =>` arm currently matches the unit variant. Change it to `xproto::XEvent::Expose { .. } =>` for now (it still does the focus-claim + `x11_full_redraw`; Task 5 will use the region). Confirm this is the only match site (grep `XEvent::Expose`).

- [ ] **Step 5: Verify + commit** — CI: `cargo test --lib backend::x11` → PASS.

```bash
git add src/backend/x11.rs src/main.rs
git commit -m "feat(x11): parse Expose region + count (was a discarded unit variant)"
```

---

### Task 2: `XIntent` + pure `coalesce` (the T1 responsiveness core)

The heart of the fix, and fully CI-testable: fold a classified intent batch so a 50-event wheel storm becomes one scroll, an Expose series becomes one repaint, a resize storm becomes one relayout — while clicks/keys keep their order.

**Files:** Modify `src/backend/x11.rs` (define `XIntent` + `coalesce` + tests).

**Interfaces:**
- Produces:
  - `pub enum XIntent { ScrollBy(i32), Resize { w: u16, h: u16 }, Expose { x: u16, y: u16, w: u16, h: u16 }, Click { x: i16, y: i16 }, Reload, Navigate, Quit }` — wait, `Navigate` is a loop-level result, NOT here. Final set: `ScrollBy(i32)`, `Resize { w: u16, h: u16 }`, `Expose { x: u16, y: u16, w: u16, h: u16 }`, `Click { x: i16, y: i16 }`, `Reload`, `Quit`.
  - `pub fn coalesce(intents: Vec<XIntent>) -> Vec<XIntent>`.

- [ ] **Step 1: Write the failing tests**

Add to `x11.rs`'s test module:

```rust
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
    fn coalesce_scroll_run_then_quit_passes_quit_through() {
        let batch = vec![XIntent::ScrollBy(60), XIntent::ScrollBy(60), XIntent::Quit];
        assert_eq!(coalesce(batch), vec![XIntent::ScrollBy(120), XIntent::Quit]);
    }
```

- [ ] **Step 2: Verify it fails** — CI: `cargo test --lib backend::x11::` → FAIL (`XIntent`/`coalesce` undefined).

- [ ] **Step 3: Implement**

Add to `x11.rs` (near `parse_event`):

```rust
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
                let x1 = (*x + *w).max(nx + nw);
                let y1 = (*y + *h).max(ny + nh);
                *x = x0;
                *y = y0;
                *w = x1 - x0;
                *h = y1 - y0;
            }
            (_, other) => out.push(other),
        }
    }
    out
}
```

- [ ] **Step 4: Verify + commit** — CI: `cargo test --lib backend::x11::coalesce` → PASS (all five).

```bash
git add src/backend/x11.rs
git commit -m "feat(x11): XIntent + pure coalesce (50-wheel-storm -> one scroll)"
```

---

### Task 3: `drain_events` (the T1 impure drain)

Batch input off the socket: block for the first event, then non-blockingly read every queued complete event. Thin; not CI-unit-tested (no server) — its output feeds the tested `coalesce`.

**Files:** Modify `src/backend/x11.rs`.

**Interfaces:**
- Consumes: `parse_event`, the existing `pending` queue + `stream`.
- Produces: `pub fn drain_events(&mut self) -> Result<Vec<XEvent>, String>`.

- [ ] **Step 1: Implement `drain_events`**

Add to `impl XConnection`, next to `next_event`. It reuses `next_event` for the blocking first read, then polls the `UnixStream` fd with a zero timeout and reads whole 32-byte events while readable. (Mirror the `rustix::event::poll` usage in `main.rs`'s tty loop: `poll(&mut [PollFd::new(&fd, PollFlags::IN)], Some(&ZERO))`.)

```rust
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
```

Note for the implementer: `UnixStream` implements `AsFd`, so `PollFd::new(&self.stream, …)` works (same as `main.rs`'s stdin poll). Do NOT add `use std::os::fd::AsFd` unless the compiler needs it (it usually does not — the bound is structural). If `poll`'s signature differs from `main.rs`'s usage, match `main.rs` exactly.

- [ ] **Step 2: Commit** (no unit test — impure socket path; proven end-to-end by the xvfb smoke + testimonial)

```bash
git add src/backend/x11.rs
git commit -m "feat(x11): drain_events — batch the socket after the first blocking event"
```

---

### Task 4: T2 encoders — `CreateGC` graphics-exposures=FALSE, `CreatePixmap`, `FreePixmap`

Pure, byte-exact. Kills the graphics-exposures flood and gives Task 5 its double-buffer.

**Files:** Modify `src/backend/x11.rs`.

**Interfaces:**
- Produces: `encode_create_gc` (now with graphics-exposures value), `pub fn encode_create_pixmap(pid, drawable, depth, width, height) -> Vec<u8>`, `pub fn encode_free_pixmap(pid) -> Vec<u8>`; `XConnection::create_pixmap`, `free_pixmap`.
- Constants: `const OP_CREATE_PIXMAP: u8 = 53; const OP_FREE_PIXMAP: u8 = 54;` and `const GC_GRAPHICS_EXPOSURES_MASK: u32 = 0x0001_0000;`.

- [ ] **Step 1: Write the failing byte-exact tests**

```rust
    #[test]
    fn encode_create_gc_disables_graphics_exposures() {
        let out = encode_create_gc(0x0040_0002, 0x0040_0001);
        assert_eq!(out[0], 55);
        assert_eq!(&out[2..4], &5u16.to_le_bytes());             // request length = 5 words
        assert_eq!(&out[4..8], &0x0040_0002u32.to_le_bytes());   // cid
        assert_eq!(&out[8..12], &0x0040_0001u32.to_le_bytes());  // drawable
        assert_eq!(&out[12..16], &0x0001_0000u32.to_le_bytes()); // value-mask: graphics-exposures
        assert_eq!(&out[16..20], &0u32.to_le_bytes());           // value: FALSE
        assert_eq!(out.len(), 20);
    }

    #[test]
    fn encode_create_pixmap_produces_correct_bytes() {
        let out = encode_create_pixmap(0x0040_0003, 0x0040_0001, 24, 1024, 768);
        assert_eq!(out[0], 53);
        assert_eq!(out[1], 24);                                   // depth
        assert_eq!(&out[2..4], &4u16.to_le_bytes());             // length = 4 words
        assert_eq!(&out[4..8], &0x0040_0003u32.to_le_bytes());   // pid
        assert_eq!(&out[8..12], &0x0040_0001u32.to_le_bytes());  // drawable
        assert_eq!(&out[12..14], &1024u16.to_le_bytes());        // width
        assert_eq!(&out[14..16], &768u16.to_le_bytes());         // height
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn encode_free_pixmap_produces_correct_bytes() {
        let out = encode_free_pixmap(0x0040_0003);
        assert_eq!(out[0], 54);
        assert_eq!(&out[2..4], &2u16.to_le_bytes());             // length = 2 words
        assert_eq!(&out[4..8], &0x0040_0003u32.to_le_bytes());   // pixmap
        assert_eq!(out.len(), 8);
    }
```

- [ ] **Step 2: Verify it fails** — CI: FAIL (new encoders undefined; old `encode_create_gc` test now wrong — UPDATE the pre-existing `encode_create_gc_produces_correct_bytes` test to match the new 20-byte form, or delete it in favor of the new `_disables_graphics_exposures` test).

- [ ] **Step 3: Implement**

Add the constants (near the other `OP_`). Replace `encode_create_gc`'s body:

```rust
pub fn encode_create_gc(cid: u32, drawable: u32) -> Vec<u8> {
    // graphics-exposures = FALSE: this client never wants NoExpose/
    // GraphicsExpose events (it repaints damaged regions from its own
    // server-side pixmap), and on non-retaining servers discarding
    // GraphicsExpose after CopyArea shows scroll garbage.
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

pub fn encode_free_pixmap(pid: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.push(OP_FREE_PIXMAP);
    out.push(0);
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&pid.to_le_bytes());
    out
}
```

Add `XConnection` methods (mirror `create_gc`):

```rust
    pub fn create_pixmap(&mut self, drawable: u32, depth: u8, width: u16, height: u16) -> Result<u32, String> {
        let pid = self.ids.next();
        self.send(&encode_create_pixmap(pid, drawable, depth, width, height))?;
        Ok(pid)
    }

    pub fn free_pixmap(&mut self, pid: u32) -> Result<(), String> {
        self.send(&encode_free_pixmap(pid))
    }
```

- [ ] **Step 4: Verify + commit** — CI: `cargo test --lib backend::x11` → PASS.

```bash
git add src/backend/x11.rs
git commit -m "feat(x11): CreatePixmap/FreePixmap encoders; CreateGC graphics-exposures=FALSE"
```

---

### Task 5: Server-side pixmap render model (T2 wiring)

Route `x11_full_redraw`/`x11_scroll_to`/Expose through a viewport-sized server pixmap. This is the impure X-path core; not CI-unit-tested (no server) — correctness rests on Task-4 encoder bytes + xvfb smoke + testimonial. **The A5 surface goldens are unaffected (headless dump path).**

**Files:** Modify `src/main.rs` (`run_x11` state + the redraw helpers).

**Interfaces:**
- Consumes: `create_pixmap`/`free_pixmap`/`copy_area`/`put_image_at` (Tasks 4 + existing).

- [ ] **Step 1: Add pixmap lifecycle to `run_x11`**

After the GC is created and the initial size is known, create a viewport-sized pixmap at the window depth: `let mut pixmap = conn.create_pixmap(window, depth, width as u16, height as u16)?;` (handle the `Result` with the loop's existing `eprintln!`+degrade style). On `ConfigureNotify` (resize), before reflow/repaint: `conn.free_pixmap(pixmap)`; recreate at the new size; reassign. (A pixmap is window-depth, window-sized — it is the back buffer.)

- [ ] **Step 2: Repoint the redraw helpers at the pixmap**

Change `x11_full_redraw` and `x11_scroll_to` to take the `pixmap` id and paint through it (the ONLY structural change; the crop/convert math is unchanged):
- `x11_full_redraw`: `put_image` the cropped viewport **into `pixmap`** (not the window), then `conn.copy_area(pixmap, window, gc, 0, 0, 0, 0, width, height)`.
- `x11_scroll_to` `Partial`: `copy_area(pixmap, pixmap, …)` for the retained rows (within the back buffer), `put_image_at(pixmap, …)` for the new strip, then one `copy_area(pixmap, window, gc, 0, 0, 0, 0, width, height)` to present.
- `x11_scroll_to` `Full`: delegates to `x11_full_redraw` (unchanged).

- [ ] **Step 3: Expose repaints from the pixmap (zero client→server image bytes)**

`run_x11`'s Expose arm (now `XEvent::Expose { x, y, w, h, .. }` after Task 1, and coalesced to one damage rect by Task 2) becomes a single `conn.copy_area(pixmap, window, gc, x as i16, y as i16, x as i16, y as i16, w, h)` — NO PutImage. (The first-Expose focus-claim stays.)

- [ ] **Step 4: Commit** (verification: encoder bytes already tested; the pixmap wiring is proven by the xvfb smoke in Task 8 + operator testimonial. No CI unit test asserts window pixels — say so in the commit body.)

```bash
git add src/main.rs
git commit -m "perf(x11): double-buffer via server-side pixmap; Expose repaints from it"
```

---

### Task 6: Frame-batched output (T4)

Coalesce a frame's requests into one write. Impure; measured by strace in the T6 report.

**Files:** Modify `src/backend/x11.rs` (frame buffer on `XConnection`) + `src/main.rs` (wrap each paint).

**Interfaces:**
- Produces: `XConnection::begin_frame(&mut self)`, `XConnection::end_frame(&mut self) -> Result<(), String>`.

- [ ] **Step 1: Add a frame buffer to `XConnection`**

Add a field `frame: Option<Vec<u8>>` (default `None`). Change `send` so that when `frame` is `Some`, requests append to the buffer instead of writing:

```rust
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
```

(`put_image_at`'s internal `send`s now accumulate — correct.) Add `frame: None` to `XConnection`'s constructor.

- [ ] **Step 2: Wrap each paint in `run_x11`**

Around each `x11_full_redraw`/`x11_scroll_to`/Expose-copy call site, bracket with `conn.begin_frame();` … `let _ = conn.end_frame();` so one drained batch → one buffer → one write.

- [ ] **Step 3: Commit**

```bash
git add src/backend/x11.rs src/main.rs
git commit -m "perf(x11): frame-buffered writes (one write per painted frame)"
```

---

### Task 7: Resize sanity — session cache + `reflow_from_dom` (T3)

Resize must generate zero network traffic. Split fetch+parse (per navigation) from reflow (per resize). The zero-fetch guarantee is **structural**: `reflow_from_dom` takes a parsed DOM, so it cannot fetch.

**Files:** Modify `src/main.rs`.

**Interfaces:**
- Produces: `fn reflow_from_dom(dom: &dom::Document, final_url: &Url, width: u32) -> Result<(MemSurface, Vec<layout::Fragment>), String>` (exact `Document` type: match `dom::parser::parse`'s return type); a session-cache struct `struct X11Session { dom: dom::Document, final_url: Url }` held by `run_x11`.

- [ ] **Step 1: Write the failing test (reflow renders from a DOM, no network)**

Add to `main.rs`'s `#[cfg(test)]` module. It parses a fixture's HTML directly (no fetch) and asserts `reflow_from_dom` produces fragments — proving the reflow path is fetch-free by construction:

```rust
    #[test]
    fn reflow_from_dom_renders_without_fetching() {
        let html = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/basic.html")).unwrap();
        let dom = stele::dom::parser::parse(&html);
        let url = Url::new("file:///fixtures/basic.html");
        let (surface, fragments) = reflow_from_dom(&dom, &url, 800).expect("reflow renders");
        assert!(!fragments.is_empty(), "basic.html must produce fragments");
        let (w, _) = stele::surface::Surface::size(&surface);
        assert_eq!(w, 800);
    }
```

- [ ] **Step 2: Verify it fails** — CI: FAIL (`reflow_from_dom` undefined).

- [ ] **Step 3: Extract `reflow_from_dom`; make `render_x11_page` call it**

Move the **width-dependent tail** of `render_x11_page` (from `collect_all_author_sheets` through `raster::paint`) into `reflow_from_dom(dom, final_url, width)`. `render_x11_page` becomes: `fetch_response` → `parse` → frameset check → `reflow_from_dom(&dom, &response.final_url, width)`, and (for the caller) also returns the parsed `dom`+`final_url` so `run_x11` can cache them. Simplest shape: add `fn load_x11_page(url, width) -> Result<(X11Session, MemSurface, Vec<Fragment>), String>` that fetches+parses+caches and calls `reflow_from_dom`; keep `render_x11_page` as a thin wrapper if other callers need it (grep — only `run_x11` calls it).

- [ ] **Step 4: Rewire `run_x11`**

- Hold `let mut session: X11Session` alongside `surface`/`fragments`.
- Navigation (initial, link click, F5): `load_x11_page` → set `session`, `surface`, `fragments`.
- `ConfigureNotify`: `reflow_from_dom(&session.dom, &session.final_url, width)` ONLY — no `fetch`, no `parse`. Set `surface`/`fragments`, clamp `scroll_y`, repaint (through the pixmap).

- [ ] **Step 5: Verify + commit** — CI: `cargo test` → the reflow test passes; existing tests green.

```bash
git add src/main.rs
git commit -m "feat(x11): resize reflows from cached DOM — zero network on ConfigureNotify"
```

---

### Task 8: Instrumentation, xvfb smoke, JOURNAL (T6)

Counters + the non-gating live smoke + the before/after report.

**Files:** Modify `src/main.rs` (counters); add xvfb CI step; `JOURNAL.md`.

- [ ] **Step 1: Debug counters**

Behind an env var (`STELE_X11_STATS`), have `run_x11` accumulate: events-drained-per-batch, `scroll_to` calls, PutImage bytes/frame, CopyArea ops, frames painted; print a summary line on quit to stderr. (A simple `struct X11Stats { … }` incremented at the call sites; gate the print on `std::env::var("STELE_X11_STATS").is_ok()`.)

- [ ] **Step 2: xvfb smoke (non-gating CI)**

Add a CI step (own job, `continue-on-error: true`) that installs `xvfb`, starts `Xvfb :99`, runs `stele --x11 file://…/fixtures/basic.html` under a short driver that maps the window, synthesizes ~50 wheel events (via `xdotool` or an `XTEST`-free canned input), and quits — asserting a clean exit (no crash/hang). If xvfb/xdotool prove flaky in one timeboxed attempt, leave the job `continue-on-error` and note it; it never gates merge.

- [ ] **Step 3: JOURNAL note + before/after table**

Append to `JOURNAL.md`: the packet summary; the strace syscall-count for the 50-wheel storm before/after (run on the dev box, put the numbers in); the `--stats` counter table (≈50 blit rounds → 1). Note the operator testimonial is pending (WSLg feel; 486/Xfbdev per-frame cost).

- [ ] **Step 4: Commit**

```bash
git add src/main.rs .github/workflows JOURNAL.md
git commit -m "feat(x11): --stats counters + non-gating xvfb smoke; journal the storm table"
```

---

## Self-Review

**1. Spec coverage (PR 1 = T1–T4 + T6):**
- T1 input coalescing → Task 1 (Expose parse) + Task 2 (`coalesce`) + Task 3 (`drain_events`). ✓
- T2 server-side frame → Task 4 (encoders, graphics-exposures=FALSE) + Task 5 (pixmap wiring, Expose-from-pixmap). ✓
- T3 resize sanity → Task 7 (`reflow_from_dom` + session cache, zero network). ✓
- T4 output batching → Task 6 (frame buffer). ✓
- T6 instrumentation + acceptance → Task 8 (counters, xvfb smoke, report). ✓
- T5 (RAM) → **PR 2**, its own plan. Correctly out of scope.

**2. Placeholder scan:** No "TBD"/"handle errors". The `Navigate`-not-here aside in Task 2's interface block is a deliberate correction, and the final `XIntent` set is stated once, consistently, in the implementation. The xvfb driver ("xdotool or canned input") is the one genuinely environment-dependent step — flagged as non-gating with a named fallback, not a silent gap.

**3. Type consistency:** `XIntent`'s variant set matches between Task 2's tests, its definition, and Task 5's consumer. `coalesce(Vec<XIntent>) -> Vec<XIntent>` consistent. `encode_create_pixmap(pid, drawable, depth, width, height)` / `create_pixmap(drawable, depth, width, height)` — note the method reorders to `(drawable, depth, w, h)` and allocates `pid` internally; the encoder takes `pid` first. Stated in both. `XEvent::Expose { x, y, w, h, count }` fields match between Task 1's parse and Task 2/5's consumers (coalesce's `Expose` drops `count` — intentional; `count` is only used loop-side to know a series continues). `reflow_from_dom(dom, final_url, width)` consistent between Task 7's test and its callers.

**4. Testability seam honored:** every CI-tested unit (Expose parse, `coalesce`, the three encoders, `reflow_from_dom`) is pure; every impure X-path unit (`drain_events`, pixmap wiring, frame buffer) is explicitly marked not-CI-unit-tested and routed to the xvfb smoke + testimonial. The A5 surface-golden invariant is restated in Global Constraints and Task 5.
