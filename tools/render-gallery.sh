#!/usr/bin/env bash
#
# tools/render-gallery.sh - render every fixture into a downloadable gallery
# Copyright (C) 2026 Tucker McLean
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <https://www.gnu.org/licenses/>.
#
# render-gallery.sh — best-effort CI render gallery. Renders every fixture
# in fixtures/*.html through a freshly built `stele` binary (both
# --dump-png and --dump-text), plus explicit httpforever.html light/dark
# --color-scheme variants, and emits a self-contained index.html so a
# reviewer can SEE what a build renders without pulling and running the
# binary themselves.
#
# Usage:
#   ./tools/render-gallery.sh <path-to-stele-binary> <output-dir>
#
# This is a gallery, not a gate: an individual fixture failing to render
# (crash, timeout, whatever) is logged to stderr and skipped, it never
# aborts the script. The script's own exit code is 0 as long as its own
# preconditions hold (binary present+executable, fixtures/ present, output
# dir creatable) — only THOSE are treated as structural failures.

set -uo pipefail

BIN="${1:-}"
OUTDIR="${2:-}"

if [ -z "$BIN" ] || [ -z "$OUTDIR" ]; then
  echo "usage: $0 <path-to-stele-binary> <output-dir>" >&2
  exit 1
fi

if [ ! -f "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "render-gallery: binary not found or not executable: $BIN" >&2
  exit 1
fi

FIXTURES_DIR="fixtures"
if [ ! -d "$FIXTURES_DIR" ]; then
  echo "render-gallery: fixtures directory not found: $FIXTURES_DIR" >&2
  exit 1
fi

if ! mkdir -p "$OUTDIR"; then
  echo "render-gallery: could not create output dir: $OUTDIR" >&2
  exit 1
fi

# Per-render ceiling so one hostile/huge fixture can't hang the whole CI
# job. Normal fixtures render in well under a second; this is a safety
# net, not an expected budget.
TIMEOUT_SECS=30

attempted=0
renders_ok=0
renders_total=0
failures=0

INDEX="$OUTDIR/index.html"

SHA="${GITHUB_SHA:-}"
if [ -z "$SHA" ]; then
  SHA="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
fi

cat > "$INDEX" <<HTML
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Stele CI Render Gallery</title>
<style>
  :root { color-scheme: light dark; }
  * { box-sizing: border-box; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    margin: 0;
    padding: 2rem;
    background: #f5f5f7;
    color: #1d1d1f;
  }
  header { margin-bottom: 2rem; }
  h1 { margin: 0 0 0.35rem 0; font-size: 1.75rem; }
  .meta { color: #555; font-size: 0.9rem; }
  .meta code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    background: rgba(0,0,0,0.06);
    padding: 0.1rem 0.35rem;
    border-radius: 4px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 1.25rem;
  }
  .card {
    background: #fff;
    border: 1px solid #ddd;
    border-radius: 8px;
    padding: 1rem;
    box-shadow: 0 1px 3px rgba(0,0,0,0.08);
  }
  .card h2 {
    margin: 0 0 0.6rem 0;
    font-size: 0.95rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    word-break: break-all;
  }
  .card img {
    max-width: 100%;
    height: auto;
    border: 1px solid #eee;
    border-radius: 4px;
    display: block;
    margin-bottom: 0.6rem;
    background: #fff;
  }
  .card a { color: #0060df; }
  .failed { color: #b00020; font-style: italic; margin: 0 0 0.6rem 0; }
  @media (prefers-color-scheme: dark) {
    body { background: #1c1c1e; color: #f2f2f7; }
    .meta { color: #a1a1a6; }
    .meta code { background: rgba(255,255,255,0.1); }
    .card { background: #2c2c2e; border-color: #3a3a3c; box-shadow: none; }
    .card img { border-color: #3a3a3c; }
    .card a { color: #6ea8fe; }
    .failed { color: #ff6b6b; }
  }
</style>
</head>
<body>
<header>
  <h1>Stele CI Render Gallery</h1>
  <p class="meta">Rendered live by CI from commit <code>${SHA}</code> — not blessed goldens.</p>
</header>
<div class="grid">
HTML

# render_one <fixture-path> <gallery-name> [extra stele args...]
#
# Runs both --dump-png and --dump-text against $BIN for one fixture, under
# a timeout, tolerating failure of either (or both). Appends one card to
# index.html reflecting whatever actually landed on disk.
render_one() {
  local fixture="$1"
  local name="$2"
  shift 2
  local extra_args=("$@")

  attempted=$((attempted + 1))

  local png_ok=0
  local txt_ok=0
  local err

  renders_total=$((renders_total + 1))
  if err="$(timeout "${TIMEOUT_SECS}s" "$BIN" --headless --dump-png "$fixture" "$OUTDIR/$name.png" "${extra_args[@]}" 2>&1)"; then
    if [ -f "$OUTDIR/$name.png" ]; then
      png_ok=1
      renders_ok=$((renders_ok + 1))
    else
      echo "render-gallery: warning: $name: --dump-png exited 0 but produced no PNG" >&2
      failures=$((failures + 1))
    fi
  else
    echo "render-gallery: warning: $name: --dump-png failed: $(printf '%s' "$err" | tail -n1)" >&2
    rm -f "$OUTDIR/$name.png"
    failures=$((failures + 1))
  fi

  renders_total=$((renders_total + 1))
  # Redirect stdout to the .txt file while still capturing stderr into
  # $err for the failure log (fd2 -> current fd1 target, THEN fd1 -> file).
  if err="$(timeout "${TIMEOUT_SECS}s" "$BIN" --headless --dump-text "$fixture" "${extra_args[@]}" 2>&1 1>"$OUTDIR/$name.txt")"; then
    txt_ok=1
    renders_ok=$((renders_ok + 1))
  else
    echo "render-gallery: warning: $name: --dump-text failed: $(printf '%s' "$err" | tail -n1)" >&2
    rm -f "$OUTDIR/$name.txt"
    failures=$((failures + 1))
  fi

  {
    printf '  <div class="card">\n'
    printf '    <h2>%s</h2>\n' "$name"
    if [ "$png_ok" = 1 ]; then
      printf '    <img src="%s.png" alt="%s render" loading="lazy">\n' "$name" "$name"
    else
      printf '    <p class="failed">(render failed)</p>\n'
    fi
    if [ "$txt_ok" = 1 ]; then
      printf '    <p><a href="%s.txt">text dump</a></p>\n' "$name"
    fi
    printf '  </div>\n'
  } >> "$INDEX"
}

for fixture in "$FIXTURES_DIR"/*.html; do
  [ -e "$fixture" ] || continue
  name="$(basename "$fixture" .html)"
  render_one "$fixture" "$name"
done

# Explicit httpforever.html light/dark variants (in addition to the plain
# httpforever.png/.txt the loop above already produced): this fixture's own
# dark theme is reachable ONLY via --color-scheme dark (no
# prefers-color-scheme fallback in its own CSS).
if [ -f "$FIXTURES_DIR/httpforever.html" ]; then
  render_one "$FIXTURES_DIR/httpforever.html" "httpforever.light" --color-scheme light
  render_one "$FIXTURES_DIR/httpforever.html" "httpforever.dark" --color-scheme dark
fi

# Browser-chrome screenshot: fixtures/basic.html rendered INSIDE the chrome
# (address bar + back button + throbber + status bar). Uses the EXACT
# `--dump-png --chrome <src> <out>` form accept.sh's A5u gate uses (not
# render_one, which would place `--chrome` after <out>) so the blessed
# golden byte-matches what A5u compares against.
if [ -f "$FIXTURES_DIR/basic.html" ]; then
  renders_total=$((renders_total + 1))
  if "$BIN" --headless --dump-png --chrome "$FIXTURES_DIR/basic.html" "$OUTDIR/chrome-basic.png" 2>/dev/null \
      && [ -s "$OUTDIR/chrome-basic.png" ]; then
    renders_ok=$((renders_ok + 1))
  else
    echo "render-gallery: warning: chrome-basic (--chrome) render failed" >&2
    failures=$((failures + 1))
  fi
fi

cat >> "$INDEX" <<HTML
</div>
</body>
</html>
HTML

echo "render-gallery: ${renders_ok}/${renders_total} renders succeeded across ${attempted} fixture attempts, ${failures} failures" >&2

exit 0
