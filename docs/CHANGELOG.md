# Changelog

Summary of what each delivery phase landed. One section per phase; newest at the top.

## Phase 06 — The `.rizref` file format (done)

On branch `feat/phase-06-rizref-format`. Closes issue #7. Format specification:
[`format.md`](format.md).

- `rizref-format`: the complete v1 container. A fixed 64-byte header carries a UUID v4
  scene id, revision, writer version and creation/modification times. Chunks use a
  24-byte frame (4-byte tag, compression code, stored and decompressed lengths):
  `NAME`, `THMB`, `DOC `, `BLOB`, `PREV`, `INDX` and `END!`. A fixed 80-byte footer at
  the end of a clean file points at the last index and carries its digest; the index
  lists every live chunk by offset with a BLAKE3 digest and stores its own digest in
  front of its JSON.
- `rizref-format`: content-addressed originals. Every `BLOB` is the 32-byte BLAKE3 of
  the original bytes followed by those bytes verbatim; identical images are one asset
  and one chunk. `PREV` stores a small JPEG/PNG preview per asset so a board shows
  pictures before any original is decoded.
- `rizref-format`: the scene document is format-owned JSON mirroring `rizref-core`
  (camera, scene rect, items in z-order, groups, assets, board name). Unknown members
  are preserved at every level on resave; `schema_version` refuses newer revisions;
  `assets[].embedded = false` declares a link-only asset and the reader reports it as
  missing instead of failing. Provenance paths/URLs are hints only.
- `rizref-format`: fresh saves write `<file>.rizref.tmp` and rename atomically.
  Incremental saves append only changed chunks plus a new index and footer; unchanged
  blobs are never copied. Compaction (full rewrite) triggers when the file would grow beyond
  three times its live bytes (dead space over twice live), on Save As, and whenever the existing file cannot be appended
  to (recovered, foreign or another scene).
- `rizref-format`: recovery scans the chunk stream when no valid footer exists and
  keeps the last valid index; a partially written index fails its own digest, so the
  previous generation loads. All chunk payloads are read lazily by offset.
- `rizref-format`: board thumbnails are composited from item previews with tiny-skia
  (bounding box, ≤ 512 px, JPEG when opaque and PNG with alpha); export reuses the
  same compositor for a PNG.
- `rizref`: `rizref inspect <file> [--verify]` — header, document summary, chunk table
  and digest/blob verification without opening a window. Exit codes 0/1/2.
- `rizref`: Save, Save As, Export (PNG, composited from the stored previews), Open board, Open recent and
  dropped `.rizref` files are wired; New/Open/Quit share the unsaved-changes guard. A successful save
  clears the dirty marker, updates the title and records the board (path, item count,
  thumbnail) in recents. Loading restores the camera, re-uploads previews immediately
  and streams full-resolution textures from the worker pool as items become visible.
- `rizref`: removed the phase-00 `dev_state` camera checkpoint and its temporary
  checksummed header; `SceneId::random` produces real UUID v4 ids. Asset provenance
  (origin, source path) is recorded on import and restored on open.
- Tests: 200-item mixed-codec round trip, incremental-vs-compaction growth, a
  1000-point crash matrix, fresh-save truncation matrix, digest verification and
  deliberate corruption. 499 workspace tests pass; clippy and fmt clean. A 500 MB
  board's name/thumbnail read measured 334 µs (best of 20, warm).

## Phase 05 — UI Shell (done)

Merged 2026-10-29 in PR #17 (branch `feat/phase-05-ui-shell`, merge commit
`0335105`). Closes issue #6.

- `rizref`: custom-painted UI shell instead of native menus. `ui/menu.rs` is a
  data-driven `Action`/`MenuEntry` tree (File, Edit, Image, View, Canvas, Tools, Help)
  with submenus, check marks, disabled state, accelerators read from the live shortcut
  table, mouse and full keyboard navigation, and dismissal on Escape or focus loss.
  Right-click opens it at the cursor after selecting the item underneath.
- `rizref`: `ui/paint.rs` (`Painter` over tiny-skia, scale-aware in logical pixels) and
  `ui/mod.rs` (`Shell`, toasts, mode indicators). The shell rasterises into one cached
  viewport-sized RGBA layer; a new `vs_ui`/`fs_ui` pass in `canvas.wgsl` uploads and
  draws it as a single quad only when the revision changes.
- `rizref`: start view (`ui/start.rs`) with wordmark, drop zone, New board / Open
  images… / Open board… buttons and recent-board cards; a responsive compact layout
  for windows under 560 px tall. `recents.rs` stores path, edited-at, item count and
  optional preview in `%APPDATA%\RizRef\recents.json`, dedupes case-insensitively,
  prunes missing files and caps at 12 entries. Opening a board and recording recents
  wait for the `.rizref` reader in phase 06.
- `rizref`: shortcut system (`shortcuts.rs`): 65 actions, 64 bound by default, exact
  modifier matching, aliases for redo, `ctrl+shift+U` for ungroup, conflicts rejected
  at bind time, and persisted overrides keyed kebab-case. `panels.rs` renders the F1
  overlay with search, selection, scrolling, Enter-to-rebind, Delete-to-reset and an
  inline conflict message, plus the pending chord prompt ("Press a chord…").
- `rizref`: `settings.rs` + settings dialog: decode cap, grid/snap/HUD/border/
  checkerboard/grayscale, canvas theme (Charcoal/Slate/Black/Paper), always-on-top,
  opacity preset and ghost opacity. `sanitize()` guards hand-edited values; saves are
  atomic; a malformed shortcut override is skipped without discarding the rest of the
  file. `paths.rs` centralises `%APPDATA%\RizRef`.
- `rizref`: `platform/window_modes.rs` composes always-on-top (`SetWindowPos`),
  uniform opacity (`WS_EX_LAYERED` + `SetLayeredWindowAttributes`), click-through
  (`WS_EX_TRANSPARENT`), borderless fullscreen and ghost mode. The pure
  `styled_ex_style` plus a state matrix test pin the `WS_EX_*` juggling, including
  ghost mode restoring the exact pre-ghost opacity/click-through state.
- `rizref`: **ghost mode** (`ctrl+shift+G`, requested after the phase review): 15 %
  window alpha (configurable 10–40 %) with whole-window click-through. Because the
  window can no longer receive input, the chord is polled through
  `GetAsyncKeyState` every 50 ms while ghost/click-through is active; the loop returns
  to `ControlFlow::Wait` otherwise. An accent border and a "GHOST MODE · Ctrl+Shift+G
  to restore" pill make the state legible. Ghost opacity is session-only, never
  persisted.
- `rizref`: **custom auto-hiding title bar** (requested after the phase was closed):
  the window is created frameless (`.with_decorations(false)`) and `ui/titlebar.rs`
  paints its own 34 px bar into the cached shell layer. The bar stays hidden until the
  pointer enters the title-bar strip; it hides again on leave, focus loss, picker arm,
  ghost/click-through activation and borderless fullscreen. Controls are the always-on-top
  pin (accent state, persisted through `ToggleAlwaysOnTop`), minimize, maximize/
  restore and close, with Windows-style hover feedback (red close). Dragging the empty
  bar calls `Window::drag_window`, double-click toggles maximize, and the left, right,
  bottom and bottom-corner edges get a 5 px `drag_resize_window` band with matching
  resize cursors. `Painter::line` draws the crisp vector glyphs.
- `rizref`: **document title and unsaved close guard** (requested after the phase was
  closed): `document.rs` carries the open document's file name and dirty flag. New
  boards are `DocumentN.rizref`; the title bar's left side shows only that name (no
  application icon) and inserts `*` before the extension while dirty —
  `Document1*.rizref`. The start view stays blank, and the display title is mirrored
  to the native window title/taskbar and the UIA root. Every board effect marks the
  document dirty; imports from the start view create the document when queued. Close
  requests from the title-bar X, `ctrl+Q`, the menu's Quit and the native
  `WM_CLOSE`/Alt+F4 all call one guarded path: a dirty document opens a modal
  "Unsaved changes" panel with Cancel (default) and Close without saving. Escape
  cancels, arrows/Tab switch, Enter confirms; UIA publishes the dialog and both
  buttons and moves focus to it. The Save choice arrives with the phase-06 writer.
- `rizref`: color picker (`I`): the surface gains `COPY_SRC`; renderer copies the 1×1
  pixel under the cursor with a 256-byte row pitch and returns RGBA after a blocking
  poll. The pick frame hides HUD and shell so the crosshair cannot be sampled; the
  toast shows `#RRGGBB` plus RGB/HSV and a click copies the hex.
- `rizref`: canvas lock (`L`) blocks item mutations while navigation still works, with
  a `LOCKED` HUD badge; HUD status also reports `GHOST` and `CLICK-THROUGH`.
- `rizref`: framebuffer effects — checkerboard backdrop and grayscale item shading in
  `canvas.wgsl`; clear colors are now converted sRGB→linear (fixes the washed-out
  phase-02 background), and grid minor/major lines fade in over zoom instead of
  saturating every pixel.
- `rizref`: accessibility (`accessibility.rs`) via AccessKit: Window → a "Window
  controls" group (always-on-top switch, minimize, maximize/restore, close) → MenuBar
  → menu/submenu leaves with names, accelerators, checkable toggle state, disabled
  state and click actions routed back into `Action`. While the close guard is open the
  tree also carries the "Unsaved changes" dialog with its Cancel and Close without
  saving buttons, and focus moves to the dialog. Verified with a UIAutomation client
  (same tree Inspect.exe shows), unlike PureRef's empty tree.
- Deferred: glyph-atlas text pipeline (shell uses one cached layer), tray icon,
  dragging the window by empty canvas background, opacity slider (presets are used),
  board save/open and recents recording (phase 06). Replacing a dirty board through
  New/Open will reuse the close guard once phase 06 owns the document lifecycle.
- Verification: 462 workspace tests, clippy and rustfmt clean. Live checks covered
  the start view, context menu and submenu navigation, F1 rebinding persistence,
  settings dialog, ghost mode style/alpha flip and global-chord restore, color picker
  values against screenshots, and the UIA tree. The title bar was checked live:
  hidden/revealed screenshots, pin flipping `WS_EX_TOPMOST` and persisting, drag and
  edge resize changing `GetWindowRect`, double-click maximize/restore, ghost mode
  keeping it hidden, close exiting, and UIA exposing all four controls. The document
  feature was checked live: blank start-view title, `Document1*.rizref` in the custom
  bar and native title after New board, the unsaved dialog from the title-bar X and
  from `WM_CLOSE`, Cancel and Escape keeping the app open, Close without saving
  exiting, and UIA listing the dialog and both buttons.

## Phase 04 — Items & Manipulation (done)

Merged 2026-10-29 in PR #16 (branch `feat/phase-04-items-and-manipulation`,
merge commit `f1c5724`). Closes issue #5.

- `rizref-core`: `Transform` (centre/size/rotation, oriented hit-test, exact
  separating-axis rectangle test), a unified `Item` with `ItemBody::Image`/`Note` and
  UV-mirror flips, `Groups` (exclusive parentage, recursive with a depth guard),
  `Snapping` with guides and screen-pixel tolerance, and pure `arrange`/`align`/
  `distribute`/`match_size` functions.
- `rizref-core`: one command stack for every mutation — `SetTransforms`, `AddItems`,
  `RemoveItems`, `ReorderItems`, `SetNoteText`, `AddGroup`, `DissolveGroup` — each
  returning `Effects` (geometry/content/added/removed/order/groups). History depth
  200; key-repeat nudges coalesce inside 100 ms, gestures stay discrete. Undo-all
  reproduces the serialised scene byte-for-byte (session high-water marks are not
  serialised).
- `rizref`: new `Board` controller — click/shift-click selection with group expansion,
  marquee (exact against rotated items), move, eight-handle scale (centre pivot,
  `alt` = opposite corner, `shift` = aspect), rotate (15° steps), flips, nudges,
  duplicate/delete, z-order, grouping, arrange/align/distribute, note creation and
  text commits. The scripted 20-item acceptance test drives it through synthetic
  pointer events and checks exact geometry through undo and redo.
- `rizref`: screen-space overlay pipeline (a fourth pass in `canvas.wgsl`) for item
  outlines, hover, the selection frame, handles, rotate handle, marquee and smart
  guides. Handles stay 9 px and guides 1 px at 5% and 3200% zoom.
- `rizref`: notes — cosmic-text `Editor` sessions (caret, arrows, delete/backspace,
  Enter, Ctrl+A, text paste), rounded-card rasterisation through tiny-skia, and
  content-addressed textures so identical notes share one GPU texture. `N` creates a
  note; double-click edits; Escape or clicking outside commits one undo step.
- `rizref`: HUD readout now includes the selection count and bounding size; the cursor
  switches between move/grab/resize/rotate/text/crosshair; the input table gained the
  item shortcuts (`ctrl+A/D/G/Z/Y`, `del`, arrows, `pgup/pgdn/home/end`, `shift+H/V`,
  `ctrl+shift+W/H/R`, `3/4/5/6`, `ctrl+shift+arrows`, `ctrl+shift+,/.`).
- Verification: 393 workspace tests, clippy and rustfmt clean; the offscreen GPU test
  renders the overlay pipeline; an 8-second smoke launch creates the window and
  Vulkan surface without a validation error.

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
