# Changelog

Summary of what each delivery phase landed. One section per phase; newest at the top.

## Phase 03 — Image Pipeline (done)

- `rizref-codecs`: `DecodeLimits` (100 MP default) enforced from headers before
  allocation; EXIF orientation baked in for JPEG and WebP; committed fixture suite
  (palette/16-bit PNG, baseline/progressive/grayscale JPEG, lossy+lossless WebP,
  animated GIF, BMP/TGA/PNM/QOI, multi-size ICO); malformed-input sweep; `fuzz/`
  crate with one target per codec family and a short CI fuzz job.
- `rizref-core`: content-addressed `Asset`/`AssetStore` (BLAKE3 of the original
  bytes), `ImageItem` with oriented hit-testing, `Scene` now holds items and assets,
  box-filter previews that never upscale, and a 512 px preview per asset.
- `rizref`: `crossbeam-channel` worker pool (`import.rs`) with trace spans and panic
  isolation; 2 GB LRU decoded-bitmap cache; drop-point ring packing; winit file
  drops, clipboard paste (files or bitmap) and Ctrl+O image dialog.
- Renderer: placeholder quads deleted; items draw from real per-item
  `RGBA8UnormSrgb` textures with a 512 MB LRU budget, 4096 px display cap,
  preview-first fallback and re-upload when visible again.
- Deferred by the kickoff's own allowance: OLE `IDropTarget` for virtual files
  (Outlook attachments); winit's file-drop path covers Explorer and browsers.

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
