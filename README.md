# RizRef

A fast, portable reference-image canvas for artists — collect, arrange and study
reference images on an infinite board, always available over your painting app.

RizRef is a from-scratch implementation written in **Rust** (pure Rust wherever a
reasonable crate exists), with its own scene file format, `.rizref`.

## Status

Early development. Progress is tracked on the issue tracker, one issue per delivery
phase:

```bash
gh issue list --repo MRizky099/RizRef --state open
```

Phase 00 (workspace, dependency policy, lint/CI policy, budgets), phase 01 (window,
Vulkan→DX12 surface, on-demand pacing, single instance, logs), phase 02 (f64 camera,
pan/zoom/rotate, grid, quad + HUD pipelines) and phase 03 (worker-thread image import
with EXIF-aware decoding, BLAKE3 asset dedup, previews, an OLE drop target that also
accepts virtual files such as Outlook attachments, clipboard paste, Ctrl+O and per-item
sRGB textures) have landed. The workspace builds, tests and lints clean.

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
docs/
  ...              local working documents (not published to this remote)
```

One line each, in the direction dependencies flow:

| Crate | Responsibility |
|---|---|
| `rizref-core` | Scene model, camera, rectangle geometry, selection and the undo history. Deterministic, no IO, no UI, no windowing. |
| `rizref-format` | The `.rizref` container — header, checksums, chunk vocabulary, metadata. The only crate allowed to write board files. |
| `rizref-codecs` | Decodes every supported image format into one RGBA buffer and encodes PNG/JPEG. All backends are pure Rust. |
| `rizref` | The binary: window, renderer, input, clipboard, dialogs and Windows platform glue. |

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
| Maths/transforms | `glam` | 0.34.1 | pure Rust |
| PNG | `png` | 0.18.1 | pure Rust |
| JPEG | `zune-jpeg` / `jpeg-encoder` | 0.5.15 / 0.7.1 | decode / encode |
| WebP | `image-webp` | 0.2.4 | lossy + lossless decode, lossless encode |
| GIF | `gif` | 0.14.2 | every frame, composed and timed (animated on the board) |
| BMP, TGA, PNM, QOI | `image` | 0.25.10 | `default-features = false`; only those four backends |
| ICO | `ico` | 0.6.0 | largest embedded image wins |
| Clipboard | `arboard` | 3.7.0 | OS FFI only |
| Native dialogs | `rfd` | 0.17.2 | COM `IFileDialog` FFI |
| Import workers | `crossbeam-channel` | 0.5.17 | pure Rust, no async runtime |
| Serialization | `serde` / `serde_json` | 1.0.229 / 1.0.154 | pure Rust |
| Compression | `lz4_flex` | 0.14.0 | pure Rust |asset identity and container checksums
| Hashing | `blake3` | 1.8.7 | `features = ["pure"]` — see below |
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

Two deliberate exclusions, both recorded rather than hidden:

- **`wgpu`'s `gles` backend is off, and the GL fallback is dropped by decision.** It pulls
  `glutin_wgl_sys`, an `-sys` crate outside the accepted family, so the backend order is
  **Vulkan → DX12** only. The pure-Rust rule wins over the third backend: for a machine
  with no usable Vulkan or DX12 driver, the intended last resort is the CPU raster path
  (`tiny-skia`, already a dependency), not OpenGL. Revisiting this means either accepting
  `glutin_wgl_sys` as an FFI-only exception or building the CPU path — both are recorded
  options, not lost work.
- **HEIF/HEIC is decoded through the operating system, not in Rust.** No mature pure-Rust
  decoder exists, so `rizref-codecs` keeps reporting it as unsupported while the
  application bridges it to Windows Imaging Component
  (`crates/rizref/src/platform/heif.rs`). WIC decodes HEVC-coded HEIF when the free
  **HEIF Image Extensions** package is installed — it ships with Windows 11 and installs
  from the Store on Windows 10 — and the failure mode is a message naming that package
  rather than "corrupt file". The bridge lives with the other platform glue, so the
  codec crate's dependency set is unchanged; AVIF remains recognised but undecoded.

## Building

```bash
cargo build --workspace        # build every crate
cargo run -p rizref            # print what this build contains (no window yet)
cargo run -p rizref -- --help  # flags this build accepts
cargo test --workspace         # run all tests
cargo run -p rizref-codecs --example make_fixtures   # regenerate committed codec fixtures
cargo bench -p rizref-core -p rizref-format   # skeleton benchmarks
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

## Performance budgets

These are the phase-00 targets. They are **not measured yet**: no number here is a
claim, and nothing in the repository asserts them. Phase 07 verifies them on a
documented machine and updates this section with the measurements.

| Budget | Target |
|---|---|
| Cold start to interactive canvas, 0 items | < 300 ms |
| Pan/zoom latency, input to presented frame | < 16 ms (60 Hz) on an integrated GPU |
| Loading a 100-item board (mixed JPEG/PNG, ~200 MB originals) — first interactive frame | < 1 s, using the embedded previews, with full decode streamed in the background |
| Saving that board | < 500 ms incremental (unchanged blobs copied, not re-encoded) |
| Release binary size | < 15 MB, single exe, no bundled C runtimes |

What exists today is the measurement scaffolding, not the measurements:
`cargo bench -p rizref-core -p rizref-format` times the per-item maths (camera mapping,
rectangle culling, hit-testing) and the container primitives (header encode/decode,
BLAKE3, LZ4). Cold start, frame latency, board load, save and binary size need the full
application, so phase 07 owns them.

## License

Not yet decided; a license will be added before the first public release.
