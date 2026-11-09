//! The Win32/COM half of the platform facade (phases 01–07).
//!
//! Every module here is re-exported, in part, by its namesake in
//! `crate::platform`; nothing outside the facade names these paths.

pub mod clipboard_html;
pub mod console;
pub mod crash;
pub mod drop_target;
pub mod gdi;
pub mod icon;
pub mod imaging;
pub mod launch;
pub mod message_box;
pub mod registry;
pub mod single_instance;
pub mod window_modes;
