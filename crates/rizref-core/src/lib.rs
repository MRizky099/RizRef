//! Scene model, geometry and command history for RizRef.
//!
//! `rizref-core` is the deterministic middle of the application:
//!
//! * it owns the scene model, the canvas view transform and the undo history;
//! * it performs no IO, opens no files, and knows nothing about `winit`, `wgpu`,
//!   the clipboard or Win32.
//!
//! That boundary keeps the whole crate unit-testable without a window, a GPU or
//! a display. The layers that build on it are `rizref-format` (persistence) and
//! `rizref` (window, renderer, input and platform glue).

#![deny(unsafe_code)]

pub mod asset;
pub mod camera;
pub mod command;
pub mod geometry;
pub mod item;
pub mod preview;
pub mod scene;
pub mod scene_rect;
pub mod selection;

pub use asset::{Asset, AssetId, AssetStore, PreviewImage, checksum};
pub use camera::Camera;
pub use command::{Command, History};
pub use geometry::Rect;
pub use item::ImageItem;
pub use preview::{Fitted, PREVIEW_SIDE, fit_to_side, generate_preview};
pub use scene::{ItemId, Scene, SceneId};
pub use scene_rect::SceneRectTracker;
pub use selection::Selection;
