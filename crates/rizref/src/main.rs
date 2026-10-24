//! RizRef — the application binary.
//!
//! # Scope
//!
//! `app::run` owns the launch sequence: arguments, logging, the panic hook,
//! the single-instance handshake, then the event loop with a window and a GPU
//! surface (phase 01). The canvas, input mapping and item pipeline arrive with
//! phases 02+. Platform glue — the mutex, the args pipe, the message box —
//! lives in [`platform`], the only module allowed to call Win32 directly.

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
