<div align="center">

<img src="crates/rizref/assets/app_icon.png" alt="RizRef icon" width="128">

# RizRef

**A fast, portable reference-image board for artists.**
Collect, arrange and study reference images on an infinite canvas that stays on top of
your painting app.

[![CI](https://github.com/MRizky099/RizRef/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/MRizky099/RizRef/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/MRizky099/RizRef?include_prereleases&sort=semver)](https://github.com/MRizky099/RizRef/releases)
[![Version](https://img.shields.io/badge/version-1.0.0-blue)](docs/CHANGELOG.md)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](#license)
[![Rust](https://img.shields.io/badge/rust-stable%20·%202024%20edition-orange?logo=rust)](rust-toolchain.toml)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2B-0078D6?logo=windows)](#requirements)
[![Linux & macOS](https://img.shields.io/badge/Linux-preview%20·%20macOS%20in%20progress-lightgrey)](#status)

</div>

---

RizRef is written from scratch in **pure Rust** (no bundled C/C++ libraries) and saves
each board as one self-contained `.rizref` file: the layout, the original image bytes,
your notes and a thumbnail, all in one file you can copy anywhere.

## Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Keyboard and mouse](#keyboard-and-mouse)
- [Where RizRef keeps its files](#where-rizref-keeps-its-files)
- [Status](#status)
- [Building from source](#building-from-source)
- [For developers](#for-developers)
- [License](#license)
- [Author](#author)

## Features

**Collect**
- Drag images in from File Explorer, your browser, or Outlook attachments, or paste with
  `Ctrl+V` and open with `Ctrl+O`.
- PNG, JPEG, WebP, GIF (animated), BMP, TGA, PNM, QOI, ICO and HEIC/HEIF, all decoded in
  Rust. AVIF works through Windows' own codec.
- JPEG orientation from EXIF is respected. The same image imported twice is stored once.
- Each image remembers where it came from: **Open file location**, **Open image URL**
  (http/https only), and an **Image info** panel.

**Arrange**
- Infinite canvas with smooth pan, zoom (0.5 %–3200 %) and view rotation.
- Move, scale from the opposite corner (`Alt` scales from the centre), rotate, flip,
  group, align, distribute, and arrange as grid, stack, spread or hierarchy.
- Snapping with smart guides, z-order, duplicate, and a full undo/redo history.
- **Markdown notes** with headings, lists, task lists, tables, code, coloured
  `<span style>` text and LaTeX math (`$…$`, `$$…$$`), on any background colour.

**Stay out of the way**
- Frameless window with an auto-hiding title bar and button hints.
- **Always on top**, **click-through**, **borderless fullscreen** and **ghost mode**
  (`Ctrl+Shift+G` fades the window and lets clicks pass to the app underneath).
- **Canvas transparency** from 100 % to 0 %: only the canvas background fades, and
  your images stay fully opaque.
- Colour picker (`I`) that samples anywhere on screen and copies the hex code.
- Grayscale view, checkerboard, grid, scene boundary, canvas lock.

**Never lose work**
- Saves are atomic, and later saves only append what changed.
- Autosave to a separate recovery copy, offered back after a crash.
- A damaged board opens at its last good save, and `rizref inspect --verify` checks
  any board.

**Fit into Windows**
- `.rizref` thumbnails in File Explorer, a file icon, "Open with RizRef", and recent
  boards in the taskbar jump list.
- Start view with recent boards and their thumbnails. PNG export of the whole board.
- Rebindable shortcuts (`F1`), settings (`Ctrl+,`), and a UI Automation tree for
  assistive tools.

## Requirements

| | Minimum |
|---|---|
| Operating system | Windows 10 or 11, 64-bit (x64) |
| Graphics | A GPU with a **DirectX 12** or **Vulkan** driver (DX12 is tried first) |
| Disk | About 15 MB for the app, plus your boards |
| Optional | [AV1 Video Extension](https://apps.microsoft.com/detail/9MVZQVXJBQ9V) from the Microsoft Store, for AVIF images |

No runtime or redistributable is needed. The C runtime is linked in, and the exe only
uses DLLs that ship with Windows.

Linux (x86_64, X11 or Wayland, glibc 2.35 / Ubuntu 22.04 or newer, a **Vulkan**
driver) builds from source as a preview; packages arrive with phase 08c. macOS 10.13+
is in progress (see [Status](#status)). v1.0.0 will be released for all three
platforms together.

## Installation

Download from the [Releases](https://github.com/MRizky099/RizRef/releases) page.
Choose one:

**Installer** (`RizRef-<version>-x64-setup.exe`)
1. Run it. No administrator rights are needed for a per-user install.
2. Choose the folder. The default is `%LOCALAPPDATA%\Programs\RizRef`, or Program Files
   if you choose "install for all users".
3. Keep **"Open .rizref boards with RizRef"** ticked to get the file icon and Explorer
   thumbnails.

Uninstall from *Settings → Apps*. It removes the file association too. Silent install:
`RizRef-…-setup.exe /VERYSILENT /DIR="D:\Apps\RizRef"`.

**Portable** (`RizRef-<version>-x64.zip`)
1. Unzip anywhere, even a USB stick, and run `rizref.exe`.
2. Optional: associate `.rizref` files and enable thumbnails for your account:

   ```bash
   rizref register      # file icon, "Open with RizRef", jump list, Explorer thumbnails
   rizref unregister    # removes exactly what register wrote
   ```

   `register` points Windows at the exe in its current folder, so run it again if you
   move the folder.

> **First run:** release builds aren't code-signed yet, so Windows SmartScreen may say
> "Windows protected your PC". Choose **More info → Run anyway**.

If Explorer keeps showing old icons or thumbnails, run `ie4uinit -show`. If that isn't
enough, delete `%LOCALAPPDATA%\Microsoft\Windows\Explorer\iconcache_*.db` and
`thumbcache_*.db`, then restart Explorer.

## Quick start

1. Start RizRef and choose **New board**, or open the sample
   [`docs/demo.rizref`](docs/demo.rizref).
2. Drop or paste a few images. Press `F` to fit everything in view.
3. Drag to move, use the corner handles to scale and the top handle to rotate.
4. Press `N` for a note, type markdown, and press `Ctrl+Enter` to finish.
5. Press `Ctrl+Shift+T` to keep the board on top of your painting app, and
   `Ctrl+Shift+O` to fade the canvas.
6. Press `Ctrl+S` to save the board as one `.rizref` file.

Right-click anywhere (or click **☰** in the title bar) for every command.

## Keyboard and mouse

| Mouse | Action |
|---|---|
| Wheel | Zoom at the cursor |
| Middle-drag, or `Space` + drag | Pan |
| `Alt` + wheel | Rotate the view |
| Drag on empty canvas | Marquee select |
| Double-click a note | Edit it |

| Keys | Action | Keys | Action |
|---|---|---|---|
| `Ctrl+N` / `Ctrl+O` | New board / open images | `Ctrl+S` / `Ctrl+Shift+S` | Save / save as |
| `Ctrl+V` | Paste image | `Ctrl+E` | Export PNG |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo | `Ctrl+D` / `Del` | Duplicate / delete |
| `F` or `1` | Fit content | `0` / `2` | 100 % / reset rotation |
| `Shift+H` / `Shift+V` | Flip | `Ctrl+G` / `Ctrl+Shift+U` | Group / ungroup |
| `3` `4` `5` `6` | Arrange: grid, hierarchy, spread, stack | `Ctrl+Shift+Arrows` | Align |
| `N` | New note | `I` | Colour picker |
| `G` / `Ctrl+B` | Grid / scene boundary | `U` | Grayscale |
| `Ctrl+Shift+T` | Always on top | `Ctrl+Shift+O` | Cycle canvas transparency |
| `Ctrl+Shift+X` | Click-through | `Ctrl+Shift+G` | Ghost mode (works unfocused) |
| `L` | Lock canvas | `F11` | Borderless fullscreen |
| `F1` | All shortcuts, and rebinding | `Ctrl+,` | Settings |

## Where RizRef keeps its files

| What | Where |
|---|---|
| Settings, recent boards | `%APPDATA%\RizRef` |
| Logs (14 days), crash dumps, autosave recovery copies | `%LOCALAPPDATA%\RizRef` (**Help → Open logs folder**) |
| Linux: settings, recent boards | `$XDG_CONFIG_HOME/rizref` (default `~/.config/rizref`) |
| Linux: logs, autosave recovery copies | `$XDG_STATE_HOME/rizref` (default `~/.local/state/rizref`) |
| macOS: settings, autosave / logs | `~/Library/Application Support/RizRef` / `~/Library/Logs/RizRef` |

Command line: `rizref --help`, `rizref --version`, `rizref <board.rizref>`,
`rizref inspect <board.rizref> [--verify]`, and `-v` for debug logging.
`RIZREF_GPU_BACKEND=dx12|vulkan` forces one GPU backend. Canvas-only transparency
needs DX12; on Vulkan it fades the whole window instead.

## Status

| Phase | Scope | State |
|---|---|---|
| 00–07 | Windows app: canvas, import, editing, UI shell, `.rizref` format, notes, provenance, performance, packaging | ✅ merged |
| 08a | Portability layer + Linux (X11 + Wayland): per-OS platform glue, XDG directories, `rizref register` / `thumbnail`, CI on Windows, Linux and macOS | ✅ done ([#26](https://github.com/MRizky099/RizRef/pull/26)) |
| 08b | macOS 10.13+ (Metal, Finder integration, ⌘ shortcuts) | 🚧 next |
| 08c | Installer + portable build for every OS, the three-platform release workflow | ⏳ after 08b |
| **v1.0.0** | One release for Windows, Linux and macOS | ⏳ after phase 08 |

Each phase is one GitHub issue and one PR, summarised in
[`docs/CHANGELOG.md`](docs/CHANGELOG.md). The file format is specified in
[`docs/format.md`](docs/format.md).

## Building from source

You need stable [Rust](https://rustup.rs) (2024 edition; `rust-toolchain.toml` pins the
channel) and, on Windows, the MSVC linker from *Visual Studio Build Tools* ("Desktop
development with C++"). Only its linker is used; no C/C++ is compiled.

```bash
git clone https://github.com/MRizky099/RizRef.git
cd RizRef
cargo run -p rizref --release                         # build and run
cargo build --release -p rizref -p rizref-thumb       # exe + Explorer thumbnail DLL
cargo test --workspace                                # all tests
```

Packaging (portable zip + installer; the installer needs
[Inno Setup 6](https://jrsoftware.org/isinfo.php)):

```bash
powershell -File packaging/package.ps1
```

## For developers

### Design principles

- **Pure Rust first.** A dependency that vendors or links C/C++ is rejected unless no
  viable Rust alternative exists. OS FFI bindings (Win32/COM declarations) are fine.
- **One portable file per board.** A `.rizref` file is self-contained: the scene,
  original image bytes (verbatim, so metadata like EXIF/C2PA survives), and an embedded
  preview thumbnail for fast browsing.
- **Crash-safe by construction.** Saves are atomic, and a board never loads corrupt.
- **Always-available window.** Always-on-top, translucent, and click-through modes are
  first-class, not hacks.

### Repository layout

```
crates/
  rizref/          the application binary (window, renderer, input, platform glue)
  rizref-core/     scene model, geometry, commands/undo — no IO, no UI
  rizref-format/   .rizref container reader/writer — single source of truth
  rizref-codecs/   image decode/encode dispatch (pure-Rust codec backends)
  rizref-thumb/    Explorer thumbnail provider DLL (reads through rizref-format)
docs/
  format.md        the `.rizref` file-format specification
  CHANGELOG.md     what each phase delivered
  demo.rizref      the sample board (CC0 images drawn in code)
packaging/         package.ps1 (portable zip + installer), rizref.iss, README.txt
scripts/           bench-startup.ps1, check-imports.ps1, release-notes.sh
```

In the direction dependencies flow:

| Crate | Responsibility |
|---|---|
| `rizref-core` | Scene model, camera, rectangle geometry, selection and the undo history. Deterministic: no IO, no UI, no windowing. |
| `rizref-format` | The `.rizref` container: header, chunk framing, scene document, content-addressed blobs, index/footer, recovery, incremental save and board thumbnails. The only crate allowed to write board files. See [`docs/format.md`](docs/format.md). |
| `rizref-codecs` | Decodes every supported image format into one RGBA buffer and encodes PNG/JPEG. All backends are pure Rust. |
| `rizref` | The binary: window, renderer, input, clipboard, dialogs and Windows platform glue. |
| `rizref-thumb` | `rizref_thumb.dll`, the in-process COM thumbnail provider Explorer loads for `.rizref` files. Optional: the exe runs without it. |

### Development commands

```bash
cargo run -p rizref -- --help                         # flags this build accepts
cargo run -p rizref-codecs --example make_fixtures    # regenerate committed codec fixtures
cargo run -p rizref --example make_icon               # regenerate both .ico files from assets/*.png
cargo run -p rizref-format --example make_demo_board  # regenerate docs/demo.rizref
cargo bench -p rizref-core -p rizref-format           # culling and container benches, board I/O budgets
cargo fmt --all                                       # formatting
cargo clippy --workspace --all-targets -- -D warnings # lint policy, as CI runs it
```

Fuzzing lives in the workspace-excluded `fuzz/` crate and needs nightly. CI runs one
matrix job per codec family: 60 s per target on a pull request, 600 s per target on the
nightly schedule. Locally:

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run decode_any -- -max_total_time=600
```

Lints are workspace-wide (`[workspace.lints]`):
- `unsafe_code` is denied in every crate.
- Public items must be documented.
- `todo!()` and `unimplemented!()` are denied.
- OS calls are confined to `rizref::platform` (`windows/`, `unix/`, `linux/`, `macos/`).

CI runs `fmt`, `clippy`, `test` and the dependency audit on `windows-latest`,
`ubuntu-22.04` and `macos-14` (with an `x86_64-apple-darwin` cross-check), plus the fuzz
matrix.

Release builds use fat LTO at `opt-level = "s"` and link the C runtime statically
(`.cargo/config.toml`), so `rizref.exe` imports operating-system DLLs only.
`scripts/check-imports.ps1` reads the PE import table and fails on any C/C++ runtime.

### Releasing

Pushing a tag `vX.Y.Z` that matches the workspace version runs
`.github/workflows/release.yml`. It tests, packages, signs when a certificate is
configured, and creates a **draft** GitHub release with notes generated from the
conventional commits (`scripts/release-notes.sh`). A maintainer reviews and publishes
the draft by hand; the workflow never publishes. Signing activates once the secrets
`RIZREF_SIGN_PFX_BASE64` and `RIZREF_SIGN_PASSWORD` exist.

### Dependencies

Every dependency is declared once in the root `Cargo.toml` under
`[workspace.dependencies]` and opted into per crate, so a version can't drift between
crates. These are the pinned versions:

| Concern | Crate | Version | Notes |
|---|---|---|---|
| Windowing/input | `winit` | 0.30.13 | pure Rust; `windows-sys` on Windows, X11/Wayland through `dlopen` on Linux, `objc2` on macOS |
| GPU rendering | `wgpu` | 30.0.1 | DX12 + Vulkan; Metal/WebGPU/GL off |
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
| AVIF | none (OS bridge) | — | Windows codec; needs the AV1 Video Extension. Not supported on Linux/macOS yet (phase-08 decision) |
| Clipboard | `arboard` | 3.7.0 | OS FFI only; `wayland-data-control` on Linux (`wl-clipboard-rs`, pure Rust) |
| Native dialogs | `rfd` | 0.17.2 | COM `IFileDialog` FFI; xdg-desktop-portal over D-Bus on Linux (`ashpd`/`zbus`, not GTK) |
| Import workers | `crossbeam-channel` | 0.5.17 | pure Rust, no async runtime |
| Serialization | `serde` / `serde_json` | 1.0.229 / 1.0.154 | pure Rust |
| Compression | `lz4_flex` | 0.14.0 | pure Rust |
| Hashing | `blake3` | 1.8.7 | asset identity and container checksums; `features = ["pure"]` (see below) |
| Logging | `tracing` / `tracing-subscriber` | 0.1.44 / 0.3.23 | pure Rust |
| Platform FFI | `windows` | 0.62.2 | Windows only. Win32/COM declarations: mutex, pipe, dialogs, OLE drop target, registry, shell |
| COM support | `windows-core` | 0.62.2 | Windows only. Required by the `#[implement]` macro; pure Rust |
| Fallback UI font | `ratex-katex-fonts` | =0.1.15 | Linux/macOS: KaTeX_SansSerif (SIL OFL 1.1), already embedded for math, registered as the last-resort face |

#### The pure-Rust rule, and how it is checked

No dependency may vendor or link C/C++. Win32/COM **declaration** crates are the only
accepted exception. `cargo tree` is audited for `*-sys`/`*_sys` crates on the Windows
target, and CI fails on anything outside the accepted list.

Audit result for the current tree (`x86_64-pc-windows-msvc`, `normal,build`):

- `windows-sys`: Win32/COM declarations only; the accepted family.
- `renderdoc-sys`: declarations for the RenderDoc capture API, loaded at runtime and
  linked against nothing. **Not removable:** `wgpu` 30 enables `wgpu-core/renderdoc`
  unconditionally for every non-wasm target, with no feature to turn it off.
- `blake3` is built with `features = ["pure"]`. Without it, its build script assembles
  BLAKE3's hand-written C assembly whenever a C compiler is on `PATH`. With `pure`,
  nothing is compiled or linked. The build script still probes one compiler flag on
  x86_64, which adds nothing to our artifacts.

Phase 06a's additions (`pulldown-cmark`, `csscolorparser`, the `ratex-*` crates) and
phase 07 add **no** `-sys` crate. Phase 07 only grows the `windows` feature list (crash
dumps through the OS's `dbghelp.dll`, registration, the thumbnail provider).

Linux and macOS (phase 08a; `scripts/audit-native.sh <triple> [build-dir]`, run by CI
for `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin` and `x86_64-apple-darwin`). The
accepted family grows by declaration-only and `dlopen` crates, as the phase-08 plan
states:

| Target | `-sys` crate | Why it is accepted |
|---|---|---|
| Linux | `wayland-sys` | libwayland reached through `dlopen` (feature checked by the script); never linked |
| Linux | `linux-raw-sys` | `rustix`'s system-call declarations, pure Rust |
| macOS | `core-foundation-sys` | CoreFoundation declarations; the framework ships with the OS |
| macOS | `objc-sys` | Objective-C runtime declarations (`objc2` 0.5, via winit and AccessKit); `libobjc` ships with the OS |
| all | `renderdoc-sys`, the `windows-sys` family | as above (Win32 crates appear only as host-side build-tool dependencies off Windows) |

`wayland-backend` lists `cc` as a build dependency but compiles its C log shim only with
its `log` feature, which nothing enables; the script fails if it ever is. Beyond names,
the script fails if any build script left an object file or static library in its
`OUT_DIR` (BLAKE3's empty flag probe excepted), which catches C compiled by a crate
whose name does not end in `-sys`.

Deliberate exclusions, recorded rather than hidden:

- **`wgpu`'s `gles` backend is off on Windows.** It pulls `glutin_wgl_sys`, an `-sys`
  crate outside the accepted family. The backend order is **DX12 → Vulkan**.
- **HEIF/HEIC is decoded in Rust** by `heic-rs` (ISOBMFF + HEVC still pictures, grid
  tiles, 10-bit, `irot`/`imir`), with `#![forbid(unsafe_code)]` and no build script.
- **AVIF goes through Windows Imaging Component.** The pure-Rust decoder path
  (`rav1d`) enables x86 assembly that needs NASM to build. Rather than make
  `cargo build` depend on an assembler, AVIF uses the OS codec when the free AV1 Video
  Extension is installed, and a missing codec is reported as the package to install.

### Performance budgets

Measured on the reference machine: a 2015 laptop with an Intel Core i7-4720HQ, 12 GB RAM,
an NVIDIA GeForce GTX 950M and Windows 10 Pro 22H2. Release build, 600 × 400 window,
60 Hz display. Every number comes from a committed, repeatable harness.

| Budget | Target | Measured | Harness |
|---|---|---|---|
| Cold start to interactive canvas, 0 items | < 300 ms | **139 ms** median (first GPU frame ~0.7–1 s; the CPU-painted shell takes input before it) | `scripts/bench-startup.ps1` |
| Pan/zoom, input to presented frame | < 16 ms | **0.54 ms** median, 0.77 ms p95 frame work; frames present every 16.5 ms (vsync) | `rizref --bench-frames=600 <board>` |
| Load a 100-item, ~200 MB board | < 1 s | **330 ms** `load`; 905 ms to the first frame with previews on a 327 MB board | `cargo bench -p rizref-format --bench board_io` |
| Save that board | < 500 ms incremental | **93–96 ms** | `board_io` |
| Release binary size | < 15 MB, no bundled C runtime | **14.6 MB**, OS DLLs only | `scripts/check-imports.ps1` |
| Note re-render, 4,000 characters | < 8 ms | **7.1–8.1 ms**, off the UI thread | `cargo test -p rizref --release -- --ignored note_budgets` |

## License

Copyright (c) 2026 Muhammad Rizky Abdillah ([@MRizky099](https://github.com/MRizky099)).

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in RizRef, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.

The KaTeX fonts compiled into the binary for note math are licensed under the SIL Open
Font License 1.1 (credited in **Help → About RizRef**).

## Author

**Muhammad Rizky Abdillah** · GitHub [@MRizky099](https://github.com/MRizky099)

Bug reports and ideas are welcome in [Issues](https://github.com/MRizky099/RizRef/issues).
