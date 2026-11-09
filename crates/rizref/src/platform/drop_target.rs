//! Drag and drop into the board.
//!
//! * **Windows** registers its own OLE `IDropTarget` (`platform/windows/`):
//!   winit's understands only `CF_HDROP`, and RizRef also takes virtual files
//!   (Outlook attachments, files inside a compressed folder) and a browser
//!   drag's source URL. Its events arrive as [`DropEvent`]s.
//! * **Linux and macOS** keep winit's own handler: `WindowEvent::DroppedFile`
//!   carries one path per file and the app imports it at the cursor. X11
//!   (XDND) and macOS deliver file drops this way; winit 0.30 has no Wayland
//!   drag-and-drop yet, and browser source URLs on these systems are the
//!   phase-08 "Input" item. [`register`] therefore returns `None` there,
//!   which leaves winit's handler in place — the same contract Windows has
//!   when OLE cannot start.

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(windows)]
pub use super::windows::drop_target::{Registration, register};

#[cfg(unix)]
pub use super::unix::drop_target::{Registration, register};

/// One file delivered by a drag & drop, either as a path or as bytes.
#[derive(Debug, Clone)]
pub struct DropEvent {
    /// Drop position in the window's client area, physical pixels.
    ///
    /// Physical because that is what OLE and `ScreenToClient` speak; the
    /// application divides by the window's scale factor before touching the
    /// camera.
    pub client_physical: (f32, f32),
    /// Files that exist on disk.
    pub paths: Vec<PathBuf>,
    /// Files that only exist inside the drag source.
    pub virtual_files: Vec<VirtualFile>,
    /// The web address of the dragged image, when the source exposed one
    /// and the drop carries exactly one image. A browser drag that also
    /// produced a temporary cache file reports both; the importer records
    /// the URL and not the cache path.
    pub source_url: Option<String>,
}

/// A file whose contents arrived without a path, such as an Outlook attachment.
#[derive(Debug, Clone)]
pub struct VirtualFile {
    /// Name the drag source gave the file, used as its import label.
    pub name: String,
    /// The contents, exactly as delivered.
    pub bytes: Arc<[u8]>,
}
