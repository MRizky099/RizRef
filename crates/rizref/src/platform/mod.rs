//! Windows platform glue: everything that talks to the operating system.
//!
//! This module is where the `windows` crate is used. Three things the window
//! needs come from the OS rather than from winit:
//!
//! * **Single-instance guard.** A named mutex decides who is primary; a named
//!   pipe carries the arguments of a second launch — typically a board to open
//!   — to the running instance, which logs them and (from phase 05 on) opens
//!   them, so double-clicking a `.rizref` file never starts a second app.
//! * **Panic reporting.** A native message box next to the log file, because an
//!   application without a console has nowhere else to say that it died.
//! * **Logging location.** The user's local-appdata directory, queried from the
//!   environment at runtime — never a hardcoded path.
//!
//! Drag and drop is owned by [`drop_target`]: winit's own OLE target handles
//! only `CF_HDROP`, so this crate registers a target that also accepts the
//! virtual-file formats (Outlook attachments, files inside a compressed
//! folder). The Explorer thumbnail provider belongs here too in later phases.
//!
//! # Unsafe code
//!
//! Every crate in this workspace sets `#![deny(unsafe_code)]`. Win32 calls are
//! `unsafe`, so each function in the submodules carries a narrow, documented
//! `#[allow(unsafe_code)]` on the smallest possible item, with a safety note
//! stating the invariant that makes the call sound. Keeping that confined to
//! this module is the point: the rest of the application stays safe Rust.

pub mod clipboard_html;
pub mod drop_target;
pub mod imaging;
pub mod launch;
pub mod message_box;
pub mod single_instance;
pub mod window_modes;

pub use drop_target::{DropEvent, Registration as DropRegistration};
pub use message_box::{alert, confirm};
pub use single_instance::{SingleInstance, forward_args_to_primary, serve_forwarded_args};
