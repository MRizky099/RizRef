# RizRef

A fast, portable reference-image canvas for artists — collect, arrange and study
reference images on an infinite board, always available over your painting app.

RizRef is a from-scratch implementation written in **Rust** (pure Rust wherever a
reasonable crate exists), with its own scene file format, `.rizref`.

## Status

**1.0.0** — feature-complete for the first release. Progress was tracked on the issue
tracker, one issue per delivery phase:

```bash
gh issue list --repo MRizky099/RizRef --state open
```

Phase 00 (workspace, dependency policy, lint/CI policy, budgets), phase 01 (window,
Vulkan→DX12 surface, on-demand pacing, single instance, logs), phase 02 (f64 camera,
pan/zoom/rotate, grid, quad + HUD pipelines), phase 03 (worker-thread image import
with EXIF-aware decoding, BLAKE3 asset dedup, previews, an OLE drop target that also
accepts virtual files such as Outlook attachments, clipboard paste, Ctrl+O and per-item
sRGB textures), phase 04 (select/marquee, move/scale/rotate/flip, snapping with smart
guides, arrange/align/distribute, groups, notes and a coalescing undo history) and
phase 05 (custom context menus, a start view with recents, a rebindable shortcut table
with an F1 overlay, settings, a frameless auto-hiding title bar that names the open
document and marks unsaved work, an unsaved-changes close guard, always-on-top/
opacity/click-through/borderless window modes, ghost mode, color picker, canvas lock
and a UIA accessibility tree) and phase 06 (the complete `.rizref` container: header,
chunk stream, JSON scene document, content-addressed image blobs, asset previews,
board thumbnails, atomic fresh saves, incremental appends, recovery and compaction,
`rizref inspect --verify`, and Save/Open/Export wiring in the application) and phase
06a (markdown notes: a source editor with ✓/✗ chrome, rendered markdown with
`<span style>` colours and sizes, inline/display math typeset by RaTeX, a hover
toolbar with a background colour popover and auto-contrast ink, a fixed wrap width
with explicit "Reflow to box width", no note rotation, a title-bar ☰ menu, and the
additive `.rizref` note fields) and phase 06b (image provenance: browser source-URL
capture on paste and drag, Open file location / Open image URL for one selected image
with strict http/https launching, and a read-only image info dialog) and phase 07
(polish and ship: every performance budget measured and met on the reference laptop,
CPU-presented first frames while the GPU starts in the background, visible-set culling,
a parallel board load, autosave to recovery copies with a restore prompt, crash
minidumps, the Explorer thumbnail provider, per-user file-type registration and jump
list, a 14.3 MB single exe with no C runtime, a packaging script, an Inno Setup
installer and a draft-release workflow) have landed. The workspace builds, tests and
lints clean. Hand-testing of the 1.0 build against the manual test plan is the step
before tagging a release.

The sample board [`docs/demo.rizref`](docs/demo.rizref) holds four images drawn in code
(CC0) and three markdown notes; open it to see a board without collecting anything
first.

## Design principles

- **Pure Rust first.** A dependency that vendors or links C/C++ is rejected unless no
  viable Rust alternative exists; OS FFI bindings (Win32/COM declarations) are fine.
- **One portable file per board.** A `.rizref` file is self-contained: the scene,
  original image bytes (verbatim, so metadata like EXIF/C2PA survives), and an embedded
  preview thumbnail for fast browsing.
- **Crash-safe by construction.** Saves are atomic; a board never loads corrupt.
- **Always-available window.** Always-on-top, translucent, and click-through modes are
  first-class, not hacks.

## Repository layout

```
crates/
  rizref/          the application binary (window, renderer, input, platform glue)
  rizref-core/     scene model, geometry, commands/undo — no IO, no UI
  rizref-format/   .rizref container reader/writer — single source of truth
  rizref-codecs/   image decode/encode dispatch (pure-Rust codec backends)
  rizref-thumb/    Explorer thumbnail provider DLL (reads through rizref-format)
docs/
  format.md        the `.rizref` file-format specification
  demo.rizref      the sample board (CC0 images drawn in code)
  ...              local working documents (not published to this remote)
packaging/         package.ps1 (portable zip + installer), rizref.iss, README.txt
scripts/           bench-startup.ps1, check-imports.ps1, release-notes.sh
```

One line each, in the direction dependencies flow:

| Crate | Responsibility |
|---|---|
| `rizref-core` | Scene model, camera, rectangle geometry, selection and the undo history. Deterministic, no IO, no UI, no windowing. |
| `rizref-format` | The `.rizref` container — header, chunk framing, scene document, content-addressed blobs, index/footer, recovery, incremental save and board thumbnails. The only crate allowed to write board files. See [`docs/format.md`](docs/format.md). |
| `rizref-codecs` | Decodes every supported image format into one RGBA buffer and encodes PNG/JPEG. All backends are pure Rust. |
| `rizref` | The binary: window, renderer, input, clipboard, dialogs and Windows platform glue. |
| `rizref-thumb` | `rizref_thumb.dll`, the in-process COM thumbnail provider Explorer loads for `.rizref` files. Optional: the exe runs without it. |

## Dependencies

Every dependency is declared once in the root `Cargo.toml` under
`[workspace.dependencies]` and opted into per crate, so a version cannot drift
between crates. These are the pinned versions:

| Concern | Crate | Version | Notes |
|---|---|---|---|
| Windowing/input | `winit` | 0.30.13 | pure Rust; talks to `windows-sys` |
| GPU rendering | `wgpu` | 30.0.1 | Vulkan + DX12; Metal/WebGPU/GL off |
| 2D raster fallback, HUD | `tiny-skia` | 0.12.0 | pure Rust |
| Text shaping | `cosmic-text` | 0.19.0 | `fontconfig` off (it is a C library) |
| Markdown (notes) | `pulldown-cmark` | 0.13.6 | `default-features = false`: no CLI, no HTML renderer |
| CSS colours (notes) | `csscolorparser` | 0.9.0 | `std` + `named-colors` only; `<span style="color">` |
| Math (notes) | `ratex-parser` / `ratex-layout` / `ratex-types` / `ratex-render` | =0.1.15 | KaTeX-compatible, pure Rust; `embed-fonts` (KaTeX fonts, SIL OFL 1.1) |
| Maths/transforms | `glam` | 0.34.1 | pure Rust |
| PNG | `png` | 0.18.1 | pure Rust |
| JPEG | `zune-jpeg` / `jpeg-encoder` | 0.5.15 / 0.7.1 | decode / encode |
| WebP | `image-webp` | 0.2.4 | lossy + lossless decode, lossless encode |
| GIF | `gif` | 0.14.2 | every frame, composed and timed (animated on the board) |
| BMP, TGA, PNM, QOI | `image` | 0.25.10 | `default-features = false`; only those four backends |
| ICO | `ico` | 0.6.0 | largest embedded image wins |
| HEIF/HEIC | `heic-rs` | 0.1.4 | pure Rust, no `unsafe`, no build script; `parallel` off |
| AVIF | none (OS bridge) | — | Windows codec; needs the AV1 Video Extension |
| Clipboard | `arboard` | 3.7.0 | OS FFI only |
| Native dialogs | `rfd` | 0.17.2 | COM `IFileDialog` FFI |
| Import workers | `crossbeam-channel` | 0.5.17 | pure Rust, no async runtime |
| Serialization | `serde` / `serde_json` | 1.0.229 / 1.0.154 | pure Rust |
| Compression | `lz4_flex` | 0.14.0 | pure Rust |
| Hashing | `blake3` | 1.8.7 | asset identity and container checksums; `features = ["pure"]` — see below |
| Logging | `tracing` / `tracing-subscriber` | 0.1.44 / 0.3.23 | pure Rust |
| Platform FFI | `windows` | 0.62.2 | Win32/COM declarations: mutex, pipe, dialogs, OLE drop target |
| COM support | `windows-core` | 0.62.2 | required by the `#[implement]` macro; pure Rust |

### The pure-Rust rule, and how it is checked

No dependency may vendor or link C/C++. Win32/COM **declaration** crates are the only
accepted exception. `cargo tree` is audited for `*-sys`/`*_sys` crates on the Windows
target, and CI fails on anything outside the accepted list.

Audit result for the current tree (`x86_64-pc-windows-msvc`, `normal,build`):

- `windows-sys` — Win32/COM declarations only; the accepted family.
- `renderdoc-sys` — declarations for the RenderDoc capture API, loaded at runtime and
  linked against nothing. **Not removable:** `wgpu` 30 enables `wgpu-core/renderdoc`
  unconditionally for every non-wasm target, and `wgpu` exposes no feature that turns
  it off.
- `blake3` is built with `features = ["pure"]`. Without it, its build script assembles
  BLAKE3's hand-written C assembly whenever a C compiler is on `PATH`. With `pure` no C
  or assembly is compiled or linked; the build script still declares `cc` and probes one
  compiler flag on x86_64, which compiles nothing into our artifacts. If a build with
  zero C-toolchain involvement is ever required, swapping BLAKE3 for `sha2` or
  `xxhash-rust` is the way out.

- Phase 07 adds **no crate**. The `windows` feature list grows by `Console`,
  `Diagnostics_Debug`, `Kernel`, `LibraryLoader`, `Registry` and
  `Shell_PropertiesSystem` (declarations for the parent-console attach, the minidump
  writer in the OS's `dbghelp.dll`, file-type registration and the thumbnail
  provider's `IInitializeWithStream`); the audit list above is unchanged. The
  plan named `minidump-writer` for crash dumps; the OS's own `MiniDumpWriteDump`
  through the existing declarations needs no dependency at all.
- Phase 06a's additions (`pulldown-cmark`, `csscolorparser`, the `ratex-*` crates and
  their trees: `ab_glyph`, `ttf-parser`, `rust-embed`, `phf`) add **no** `-sys` crate;
  the audit list above is unchanged. RaTeX brings its own `tiny-skia` 0.11 and `png`
  0.17 alongside ours (duplicate versions, pure Rust); they stay — the release exe
  is under its size budget with them (see below). The KaTeX fonts are licensed under the
  SIL Open Font License 1.1 and are credited in the About notice.

Two deliberate exclusions, both recorded rather than hidden:

- **`wgpu`'s `gles` backend is off, and the GL fallback is dropped by decision.** It pulls
  `glutin_wgl_sys`, an `-sys` crate outside the accepted family, so the backend order is
  **Vulkan → DX12** only. The pure-Rust rule wins over the third backend: for a machine
  with no usable Vulkan or DX12 driver, the intended last resort is the CPU raster path
  (`tiny-skia`, already a dependency), not OpenGL. Revisiting this means either accepting
  `glutin_wgl_sys` as an FFI-only exception or building the CPU path — both are recorded
  options, not lost work.
- **HEIF/HEIC is decoded in Rust.** `heic-rs` implements the ISOBMFF container and the
  HEVC still-picture decoder itself — grid tiles, 10-bit, `irot`/`imir` — under
  MIT OR Apache-2.0, with `#![forbid(unsafe_code)]` and no build script. It replaced the
  earlier OS bridge, which needed a Store package and only worked on Windows.
- **AVIF goes through the operating system, and that is a tooling decision, not a
  licensing one.** `image`'s AVIF support is encoder-only (decoding needs the C `dav1d`),
  and `avif-decode` — the pure-Rust option — enables `rav1d`'s `asm` feature on x86, which
  fails to build without NASM installed. Rather than make `cargo build` depend on an
  assembler, AVIF is bridged to Windows Imaging Component (`platform/imaging.rs`), which
  decodes it when the free **AV1 Video Extension** is installed. A missing codec is
  reported as the package to install, and HEIC still has the same bridge as a fallback for
  coding tools `heic-rs` declines.

## Building

```bash
cargo build --workspace        # build every crate
cargo run -p rizref            # run the application (debug build)
cargo run -p rizref -- --help  # flags this build accepts
cargo test --workspace         # run all tests
cargo run -p rizref-codecs --example make_fixtures   # regenerate committed codec fixtures
cargo run -p rizref --example make_icon               # regenerate crates/rizref/assets/rizref.ico
cargo run -p rizref-format --example make_demo_board  # regenerate docs/demo.rizref
cargo bench -p rizref-core -p rizref-format   # culling and container benches, board I/O budgets
cargo fmt --all                # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lint policy, as CI runs it
```

Fuzzing lives in the workspace-excluded `fuzz/` crate and needs nightly. CI runs one
matrix job per codec family: 60 s per target on a pull request, 600 s per target on the
nightly schedule (the phase-03 acceptance soak). Locally:

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run decode_any -- -max_total_time=600
```

Lints are workspace-wide (`[workspace.lints]`): `unsafe_code` is denied in every crate,
public items must be documented, and `todo!()`/`unimplemented!()` are denied so no
half-finished code path can ship. Win32 calls will be confined to `rizref::platform`
with narrow, documented local overrides.

CI runs `fmt`, `clippy` and `test` on `windows-latest`, plus the dependency audit above.

Release builds (`cargo build --release -p rizref -p rizref-thumb`) use fat LTO at
`opt-level = "s"` and link the C runtime statically (`.cargo/config.toml`), so
`rizref.exe` imports operating-system DLLs only — `scripts/check-imports.ps1` reads
the PE import table and fails on any C/C++ runtime. The release binary is linked for
the GUI subsystem (no console window) and reattaches to the parent console for
`--help`, `--version` and `inspect`.

## Installing and Windows integration

`rizref.exe` is the whole application: copy it anywhere and run it. Optionally:

```bash
rizref register              # .rizref -> RizRef: icon, "Open with RizRef", jump list;
                             # Explorer thumbnails when rizref_thumb.dll sits beside the exe
rizref unregister            # remove exactly what register wrote
rizref register --machine    # for every user (an elevated installer)
```

Registration writes only `HKCU\Software\Classes` (no administrator rights). The
optional installer (`packaging/rizref.iss`, Inno Setup 6) installs per user to
`%LOCALAPPDATA%\Programs\RizRef` and runs the same command.
`packaging/package.ps1` builds both and the portable zip.

Settings and recents live in `%APPDATA%\RizRef`; logs, crash minidumps
(`crashes\`) and autosave recovery copies (`autosave\`) in `%LOCALAPPDATA%\RizRef`
(**Help → Open logs folder**). `-v`/`--verbose` logs at debug level; logs rotate daily
and 14 are kept. `RIZREF_GPU_BACKEND=dx12` (or `vulkan`) restricts the GPU walk to one
backend, for a machine whose Vulkan driver misbehaves.

### Releasing

Pushing a tag `vX.Y.Z` that matches the workspace version runs
`.github/workflows/release.yml`: test, package, sign when a certificate is configured,
and create a **draft** GitHub release with notes generated from the conventional
commits since the previous tag (`scripts/release-notes.sh`). A maintainer reviews and
publishes the draft by hand; the workflow never publishes. Code signing activates once
the repository secrets `RIZREF_SIGN_PFX_BASE64` and `RIZREF_SIGN_PASSWORD` exist — no
certificate exists yet, so artifacts are unsigned and SmartScreen will warn on first
run.

## Performance budgets

The phase-00 targets, measured in phase 07 on the reference machine: a 2015 laptop,
Intel Core i7-4720HQ, 12 GB RAM, NVIDIA GeForce GTX 950M (the only Vulkan/DX12
adapter — its Intel HD 4600 is offered by neither API), Windows 10 Pro 22H2, release
build of `feat/phase-07-polish-and-ship`, default 600 × 400 window, 60 Hz display. Every number comes from a
committed, repeatable harness.

| Budget | Target | Measured | Harness |
|---|---|---|---|
| Cold start to interactive canvas, 0 items | < 300 ms | **130 ms** median, 226 ms worst (20 warm launches); first GPU frame 1,038 ms | `scripts/bench-startup.ps1` |
| Pan/zoom latency, input to presented frame | < 16 ms | **0.59 ms** median, 0.96 ms p95 frame work on a 100-item board; 1.04 ms p95 on 500 items / 1.6 GB; frames present every 16.5 ms (vsync) | `rizref --bench-frames=600 <board>` |
| Loading a 100-item board (~200 MB) — first interactive frame | < 1 s | **905 ms** to the first frame with previews, on a *327 MB* 100-item board; the format's `load` alone takes 300 ms for 210 MB | `--bench-frames` (`board_load_ms`); `cargo bench -p rizref-format --bench board_io` |
| Saving that board | < 500 ms incremental | **96 ms** after moving an item, **121 ms** after adding a 2 MiB image | `board_io` |
| Release binary size | < 15 MB, single exe, no bundled C runtimes | **14.28 MB**; imports OS DLLs only | `scripts/check-imports.ps1` |

Notes on reading the table:

- **Cold start** is window-to-first-frame with the GPU still starting: until the
  device is ready (on this laptop the NVIDIA driver needs ~740 ms to create it), frames
  are the CPU-painted shell — start view, title bar, menus — copied to the window with
  GDI, and the window takes input. Images draw from the first GPU frame. Before phase
  07 the first frame waited for the GPU: 1,059 ms median.
- **Frame work** is the frame's CPU time minus the time spent waiting on the swapchain
  (acquire and present), i.e. the latency a change adds before the next vsync. The
  plan asks for an integrated GPU; this machine has none usable, so the figures are on
  a 2015 discrete GPU. One run showed a single 295 ms frame (out of 1,800 recorded on
  the 100-item board), not reproduced in the other runs.
- **Load** needed two fixes to fit: each original is now hashed once instead of twice,
  and blob verification and preview decoding run on all cores.
- **Size** came from the profile: thin LTO at `opt-level` 3 was 18.37 MB; fat LTO at
  `opt-level = "s"` is 13.96 MB before the static C runtime (+0.25 MB). The runtime
  cost (`cargo run --release -p rizref-codecs --example decode_bench`, 12 MP image):
  JPEG decode 139 → 154 ms, PNG 116 → 154 ms. `opt-level = "z"` (12.52 MB) was
  rejected: JPEG decode 496 ms.
- Culling 500 items costs 5.8 µs per frame (`cargo bench -p rizref-core`).

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in RizRef, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.

The KaTeX fonts compiled into the binary for note math are licensed under the SIL Open
Font License 1.1 (credited in About).
