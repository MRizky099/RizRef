# Changelog

Summary of what each delivery phase landed. One section per phase; newest at the top.

## Phase 03 — Image Pipeline (done)

Merged 2026-10-29 in PR #14 (decode, drop, paste, cache) and PR #15 (HEIC in Rust,
AVIF through the OS codec, animated GIFs). Closes issues #4 and #13.

- `rizref-codecs`: `DecodeLimits` (100 MP default) enforced from headers before
  allocation; EXIF orientation baked in for JPEG and WebP; committed fixture suite
  (palette/16-bit PNG, baseline/progressive/grayscale JPEG, lossy+lossless WebP,
  animated GIF, BMP/TGA/PNM/QOI, multi-size ICO); malformed-input sweep; `fuzz/`
  crate with one target per codec family, a per-target CI matrix on nightly and a
  scheduled ten-minute soak per target. The `decode_ico` soak found a real panic in
  `ico` 0.6 (indexed pixels outside the declared palette); ICO entries are now
  validated — real dimensions for the megapixel cap plus a palette-index walk —
  before the crate runs, with the 121-byte reproducer committed as a fixture.
- `rizref-core`: content-addressed `Asset`/`AssetStore` (BLAKE3 of the original
  bytes), `ImageItem` with oriented hit-testing, `Scene` now holds items and assets,
  box-filter previews that never upscale, and a 512 px preview per asset.
- `rizref`: `crossbeam-channel` worker pool (`import.rs`) with trace spans and panic
  isolation; 2 GB LRU decoded-bitmap cache; drop-point ring packing; clipboard paste
  (files or bitmap) and Ctrl+O image dialog.
- `rizref`: OLE `IDropTarget` (`platform/drop_target.rs`) replaces winit's handler so
  drag & drop also accepts **virtual files** (`FileGroupDescriptorW` + `FileContents`:
  Outlook attachments, entries of a compressed folder), not just `CF_HDROP` paths.
  Registration degrades to winit's handler when OLE cannot initialise; descriptor
  parsing is unit-tested against the packed Win32 layout.
- `rizref-codecs`: `decode_gif_animation` returns every GIF frame already composed onto
  a canvas — offsets, transparency and `Background` disposal applied — under frame and
  pixel budgets, with a 20 ms floor on zero-delay frames.
- `rizref-core`: `Animation`/`AnimationFrame` answer "which frame is due at time t?"
  (looping, no clock of its own), and `Asset::with_animation` keeps the first frame as the
  still image so previews, export and the file format are unchanged.
- `rizref`: animated GIFs play on a one-shot `ControlFlow::WaitUntil` timer, re-uploading
  only the current frame; **a selected item freezes** and the HUD reports the frame. A
  plain left click now selects the topmost item (the seed phase 04 builds on).
- `rizref`: **HEIC/HEIF** is bridged to Windows Imaging Component
  (`platform/imaging.rs`), which reuses the HEIF Image Extension instead of bundling an
  HEVC decoder; a missing extension is reported as a package to install. The bridge is
  covered by a test that drives it with a PNG, so CI does not need the extension.
- `rizref-codecs`: **HEIC/HEIF is now decoded in Rust** by `heic-rs` (container, HEVC
  still picture, grid tiles, 10-bit, `irot`/`imir`/`clap`), under the same pixel cap as
  every other format. Two fixtures from its corpus (`still.heic`, `grid.heic`) prove the
  single-item and tiled paths, and a dedicated `decode_heif` fuzz target guards the young
  decoder. The OS bridge stays as the fallback for coding tools it declines.
- `rizref-codecs`: `ImageFormat::Avif` is now separate from `Heif` (the `ftypavif`/`avis`
  brands), so an AVIF is named as one rather than as a generic HEIF file.
- `rizref`: AVIF is decoded through the operating system codec, because the pure-Rust
  option (`avif-decode` → `rav1d` with `asm`) cannot build without NASM; a missing AV1
  Video Extension is reported by name.
- Renderer: placeholder quads deleted; items draw from real per-item
  `RGBA8UnormSrgb` textures with a 512 MB LRU budget, 4096 px display cap,
  preview-first fallback and re-upload when visible again.

## Phase 02 — Canvas & Viewport (done)

- `rizref-core`: f64 `Camera` with rotation, cursor-anchored zoom, fit/frame and
  `visible_world_rect`; incremental `SceneRectTracker`.
- `rizref`: declarative trigger→command input table; `ViewState` gestures and 80 ms
  smoothstep zoom animation; three canvas pipelines (background grid/origin/scene border,
  instanced items over an 8-layer texture array, screen-space HUD); cosmic-text +
  tiny-skia HUD readout; dev-only view persistence.

## Phase 01 — Foundations (done)

- winit window, wgpu surface (Vulkan → DX12), on-demand frame pacing (0 ms idle).
- Single-instance guard with argument forwarding; rolling file logs; panic hook.

## Phase 00 — Research & Decisions (done)

- Cargo workspace with the pure-Rust dependency policy, lints, CI and frame budgets.
- REA study of PureRef 2.1.3 (binary, `.pur` format, runtime UI) as the clean-room basis.
