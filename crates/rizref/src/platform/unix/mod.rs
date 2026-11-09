//! What Linux and macOS share: the Unix-socket single instance, winit-level
//! window modes, and the `rfd` / `arboard` paths that replace Win32 calls.
//!
//! No module here uses `unsafe`; everything is `std`, winit, `rfd` or
//! `arboard`. Where an OS feature has no counterpart yet, the function is a
//! documented stub and `platform::capabilities` says so.

pub mod clipboard_html;
pub mod crash;
pub mod drop_target;
pub mod gdi;
pub mod icon;
pub mod imaging;
pub mod message_box;
pub mod single_instance;
pub mod window_modes;
