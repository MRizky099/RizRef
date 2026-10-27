//! RizRef — the application binary.
//!
//! # Scope
//!
//! `app::run` owns the launch sequence: arguments, logging, the panic hook,
//! the single-instance handshake, then the event loop with a window and a GPU
//! surface (phase 01). The canvas — camera, input map, quad/grid pipelines and
//! the HUD readout — arrived in phase 02; the item pipeline comes with phase
//! 03. Platform glue — the mutex, the args pipe, the message box — lives in
//! [`platform`], the only module allowed to call Win32 directly.

#![deny(unsafe_code)]

mod app;
mod canvas;
mod clipboard;
mod dev_state;
mod dialogs;
mod hud;
mod input;
mod platform;
mod renderer;
mod text;
mod view;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::run(std::env::args().skip(1).collect())
}
