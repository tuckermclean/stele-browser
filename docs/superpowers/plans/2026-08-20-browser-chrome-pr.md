# Browser chrome Plan · Spec: docs/superpowers/specs/2026-08-20-browser-chrome-design.md (read it)
Global: no new dep; 1.44MB floppy; no JS/C3; total (hostile URL/status never panics); golden-safe (chrome
opt-in via --x11 / --chrome; plain --dump-png unchanged); no local i486 builds; goldens pixel-verified.

### T1: src/backend/chrome.rs (pure module + unit tests)
Per spec §1: TOP_H=28, STATUS_H=18; `ChromeLayout{top,back,address,throbber,viewport,status: Rect}` +
`layout(win_w,win_h)->ChromeLayout` (viewport = window minus bars; tiny-window clamp to 0, no underflow);
`ChromeState{url,status:&str, loading:bool, throbber_frame:u8, can_go_back:bool}`; `draw(&mut dyn Surface,
&ChromeLayout,&ChromeState)` — gray bars, back button box+arrow (dim if !can_go_back), white address field +
url text (set_clip to the field, left-aligned/truncated), throbber (frame or idle dot), status text; does NOT
touch viewport. Register `pub mod chrome;` in backend. Unit tests: layout geometry (no overlap, viewport=win−bars,
tiny clamp); draw into MemSurface (bars are bar color; address field white w/ dark ink; long url doesn't draw
outside its field). cargo test --lib. Commit: `feat(chrome): pure chrome layout + draw module (address/back/throbber/status)`.

### T2: --chrome screenshot mode + golden
Per spec §2: a `--chrome` flag; with `--dump-png <src> <out>` render the page into `layout(win).viewport`
(offset+clip fragments to viewport) + `chrome::draw` the bars (url=final_url, status="Done", loading=false).
Fixture-less (reuse fixtures/basic.html): add accept.sh A5-style golden `chrome-basic.png` = `--dump-png --chrome
fixtures/basic.html`. Controller pixel-verifies (bars + URL + doc in viewport) + blesses. Commit: `feat(main): --chrome screenshot render + golden`.

### T3: run_x11 integration (history + back + throbber + status + viewport-offset doc paint) — MANUAL verify
Per spec §3: back-stack Vec<Url>; paint doc band into viewport region (offset TOP_H, clip); chrome::draw live
state; ButtonPress in back rect + can_go_back → pop+load+redraw; link hit-test offset by TOP_H; status shows
loading url/"Done"; throbber frame advances per redraw while loading. Address bar display-only. Manual (run_x11
untested in CI). cargo test --lib (pure bits) + cargo check. Commit: `feat(x11): draw chrome + history/back in run_x11`.

### T4: charter (C5 note) + DECISIONS + JOURNAL (+ i486 size).
