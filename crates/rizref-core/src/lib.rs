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

pub mod animation;
pub mod arrange;
pub mod asset;
pub mod camera;
pub mod command;
pub mod frame;
pub mod geometry;
pub mod group;
pub mod item;
pub mod preview;
pub mod rich;
pub mod scene;
pub mod scene_rect;
pub mod selection;
pub mod snap;
pub mod transform;

pub use animation::{Animation, AnimationFrame};
pub use arrange::{
    Align, ArrangeMode, Orientation, SizeAxis, align, arrange, distribute, match_size,
};
pub use asset::{Asset, AssetId, AssetStore, PreviewImage, checksum};
pub use camera::Camera;
pub use command::{
    AddGroup, AddItems, CoalesceKey, Command, Compound, DissolveGroup, Effects, History,
    RemoveItems, ReorderItems, SetNoteContent, SetTransforms,
};
pub use frame::{Handle, Pivot, SelectionFrame, scale_item, scale_item_in_frame, scale_rules};
pub use geometry::Rect;
pub use group::{Group, GroupChild, GroupId, Groups};
pub use item::{
    DEFAULT_NOTE_BACKGROUND, DEFAULT_NOTE_SIZE, Item, ItemBody, ItemState, MAX_WRAP_WIDTH,
    MIN_WRAP_WIDTH, NOTE_PADDING, NoteContent, NoteFormat, TEXT_PADDING, clamp_wrap_width,
    inner_note_width,
};
pub use preview::{Fitted, PREVIEW_SIDE, fit_to_side, generate_preview};
pub use rich::{DEFAULT_TEXT_SIZE, TextSpan, TextStyle};
pub use scene::{ItemId, Scene, SceneId};
pub use scene_rect::SceneRectTracker;
pub use selection::Selection;
pub use snap::{Axis as SnapAxis, Guide, SnapResult, screen_tolerance, snap_rect};
pub use transform::{MIN_ITEM_SIZE, Transform};
