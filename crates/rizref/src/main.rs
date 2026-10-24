//! RizRef — the application binary.
//!
//! # Phase 00 scope
//!
//! What exists here is deliberately small: argument handling, logging, and a
//! startup summary that exercises every part of the pinned dependency set. The
//! window, the wgpu surface and the render loop arrive in phase 01; the platform
//! glue for the single-instance guard, the native message box and drag-and-drop
//! arrives with it, in [`platform`].
//!
//! Printing that summary is not decoration. It is the cheapest way to prove that
//! a build really links and initialises what the plan says it does — the GPU
//! backend flags, the CPU rasteriser, the font database, the clipboard, the
//! native dialogs — before any of it is wired to a window.

#![deny(unsafe_code)]

mod app;
mod clipboard;
mod dialogs;
mod hud;
mod platform;
mod renderer;
mod text;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::run(std::env::args().skip(1).collect())
}
