//! Platform glue: everything that talks to the operating system.
//!
//! # Layout (phase 08)
//!
//! This module is a facade of free functions and plain types. Each submodule
//! named here (`single_instance`, `window_modes`, `launch`, …) holds the part
//! that is the same everywhere — wire formats, mode policy, argument
//! validation, file naming — together with its tests, and re-exports the
//! operating-system half from one of:
//!
//! * `windows` (`platform/windows/`) — the Win32/COM code of phases
//!   01–07, unchanged in behaviour;
//! * `unix` (`platform/unix/`) — what Linux and macOS share: the
//!   Unix-domain-socket single instance, winit-level window modes, the
//!   `rfd`/`arboard` paths;
//! * `linux` (`platform/linux/`) and `macos` (`platform/macos/`) — what
//!   differs between the two: launching (`xdg-open` / `open`) and file-type
//!   registration (XDG desktop entries / the app bundle).
//!
//! There are no traits: every function has one body per `#[cfg]`, so a call
//! site reads the same on every system and a missing counterpart is a
//! compile error on that target, not a runtime surprise. A feature an OS
//! cannot offer is a documented stub that reports so through
//! [`capabilities`], and the UI greys the command out instead of offering
//! something that silently does nothing.
//!
//! # Unsafe code
//!
//! Every crate in this workspace sets `#![deny(unsafe_code)]`. OS calls are
//! `unsafe`, so each function in the OS submodules carries a narrow,
//! documented `#[allow(unsafe_code)]` on the smallest possible item, with a
//! safety note stating the invariant that makes the call sound. The Unix
//! backends need none: they are built on `std`, winit, `rfd` and `arboard`.

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
compile_error!("RizRef supports Windows, Linux and macOS");

#[cfg(windows)]
mod windows;

#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

pub mod capabilities;
pub mod clipboard_html;
pub mod console;
pub mod crash;
pub mod drop_target;
pub mod gdi;
pub mod icon;
pub mod imaging;
pub mod launch;
pub mod message_box;
pub mod register;
pub mod single_instance;
pub mod window_modes;

pub use drop_target::{DropEvent, Registration as DropRegistration};
pub use message_box::alert;
pub use single_instance::{SingleInstance, forward_args_to_primary, serve_forwarded_args};
