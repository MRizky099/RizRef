//! Windows platform glue: everything that talks to the operating system.
//!
//! This module is where the `windows` crate is used, and where phase 01 adds the
//! three things the window needs from the OS rather than from winit:
//!
//! * **Single-instance guard.** A named mutex; a second launch forwards its
//!   arguments — typically a board to open — to the running instance and exits,
//!   so double-clicking a `.rizref` file does not start a second board.
//! * **Named-pipe or window-message hand-off** of those arguments.
//! * **Panic reporting.** A native message box next to the log file, because an
//!   application without a console has nowhere else to say that it died.
//!
//! Drag-and-drop (`IDropTarget`) belongs here too, as does the opt-in per-monitor
//! DPI awareness declaration.
//!
//! # Unsafe code
//!
//! Every crate in this workspace sets `#![deny(unsafe_code)]`. Win32 calls are
//! `unsafe`, so the functions added here will each carry a narrow, documented
//! `#[allow(unsafe_code)]` on the smallest possible item, with a safety note
//! stating the invariant that makes the call sound. Keeping that confined to
//! this module is the point: the rest of the application stays safe Rust that
//! `#![forbid]`-style rules can be applied to later.
//!
//! Nothing is implemented yet — the module is here so the rule above has a
//! written home before the first `unsafe` block appears.
