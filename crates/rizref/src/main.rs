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

mod accessibility;
mod app;
mod board;
mod canvas;
mod clipboard;
mod dialogs;
mod document;
mod hud;
mod import;
mod input;
mod inspect;
mod markdown;
mod memory;
mod note;
mod note_chrome;
mod overlay;
mod paths;
mod placement;
mod platform;
mod provenance;
mod recents;
mod renderer;
mod settings;
mod shortcuts;
mod text;
mod textures;
mod ui;
mod view;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::run(std::env::args().skip(1).collect())
}
