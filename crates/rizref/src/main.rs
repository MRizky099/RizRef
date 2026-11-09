//! RizRef — the application binary.
//!
//! # Scope
//!
//! `app::run` owns the launch sequence: arguments, logging, the panic hook,
//! the single-instance handshake, then the event loop with a window and a GPU
//! surface (phase 01). The canvas — camera, input map, quad/grid pipelines and
//! the HUD readout — arrived in phase 02; phase 03 added the image pipeline:
//! worker-thread decoding, content-addressed assets, previews, drag & drop,
//! clipboard paste and per-item sRGB textures. Phase 04 adds the editing layer:
//! selection and gestures in [`board`], the screen-space overlay (outlines,
//! handles, guides, marquee), notes rendered through cosmic-text in [`note`],
//! and the grouping/ordering/arrange commands behind one undo history. Phase 05
//! wraps the canvas in the product shell — [`ui`] (start view, menus, shortcut
//! and settings overlays), [`shortcuts`], [`settings`], [`recents`] and the
//! window modes in [`platform::window_modes`] — and publishes the command tree
//! through [`accessibility`]. Platform glue — the mutex, the args pipe, the
//! message box — lives in [`platform`], the only module allowed to call Win32
//! directly.

#![deny(unsafe_code)]
// Release builds are GUI-subsystem binaries: launching from Explorer or a jump
// list never flashes a console. `platform::console::attach_parent` restores
// terminal output for `--help`, `--version` and `inspect`. Debug builds (and
// so `cargo test`) keep the console subsystem.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accessibility;
mod app;
mod autosave;
mod board;
mod canvas;
mod clipboard;
mod contain;
mod dialogs;
mod document;
mod hud;
mod import;
mod input;
mod inspect;
mod location;
mod markdown;
mod memory;
mod note;
mod note_chrome;
mod overlay;
mod paths;
mod perf;
mod placement;
mod platform;
mod provenance;
mod recents;
mod renderer;
#[cfg(test)]
mod resource_tests;
// The `.res` writer `build.rs` uses, compiled here for the tests only.
#[cfg(test)]
#[path = "../resources.rs"]
mod resources;
mod settings;
mod shortcuts;
mod text;
mod textures;
mod ui;
mod view;

use std::process::ExitCode;

fn main() -> ExitCode {
    perf::mark_process_start();
    platform::console::attach_parent();
    app::run(std::env::args().skip(1).collect())
}
