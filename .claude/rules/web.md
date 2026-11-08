---
paths:
  - "crates/web/**"
---

# Web app

- Styling lives in `crates/web/public/style.css`: tokens on `:root` (`--surface`, `--accent`, `--ok/--warn/--bad` …), components use only tokens. A new page reuses `.panel`, `.tile`, `.chip`, `table.grid`, `.seg-group`.
- Pages that hold a chart (Overview, Latency) stay mounted across the 3 s device poll: the App renders them by a `Memo` of (view, loaded), and they read `ctl.devices` inside closures. A page rendered inside `move || devices.get()` is rebuilt every poll, so a uPlot chart there would lose its zoom.
- Charts: `crates/web/public/charts.js` (uPlot, vendored in `public/vendor/uplot`) is called through `wasm_bindgen` (`pages.rs` `chart_latency`) with JSON from `hist::chart_json`. Zoom comes back as `dc-zoom`/`dc-unzoom` DOM events.
- The hash is the address: a `hashchange` listener moves the view; links may use plain `href="#…"`.
- `python3` string replace on Rust after `cargo fmt` silently misses reformatted text: assert every replacement.
- The device poll sets `devices` only when the answer differs; any view inside `move || devices.get()` still rebuilds on a real change, so state held in its DOM (a scroll box's position) must live in a signal and be put back (`Expanded.scroll` in Routing).
