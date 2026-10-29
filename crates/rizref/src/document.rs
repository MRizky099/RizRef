//! The open document's identity, file path and unsaved state.
//!
//! The title bar shows the display name and the unsaved marker; the close guard
//! uses the dirty flag; and the save/open paths use the file path. Keeping all
//! three here means the window title, the guard and the save path cannot
//! disagree about what "unsaved" or "this board" means.

use std::path::{Path, PathBuf};

/// One open board document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// File name including the `.rizref` extension.
    name: String,
    /// Where the board is stored, once it has been saved or opened.
    path: Option<PathBuf>,
    /// Whether the document has changes not written to disk.
    dirty: bool,
}

impl Document {
    /// A new, never-saved document with the next session number.
    ///
    /// Fresh boards start dirty: they have no file yet, so the asterisk is
    /// honest from the first frame and the close guard protects early work.
    pub fn untitled(number: u32) -> Self {
        Self {
            name: format!("Document{number}.rizref"),
            path: None,
            dirty: true,
        }
    }

    /// A document opened from `path`, initially clean.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Board.rizref")
            .to_owned();
        Self {
            name,
            path: Some(path),
            dirty: false,
        }
    }

    /// Whether the document has unsaved changes.
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The file path, when the document has one.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The display name including the extension.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The display name without its extension, for a board's own name field.
    pub fn stem(&self) -> &str {
        self.name
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .filter(|stem| !stem.is_empty())
            .unwrap_or(&self.name)
    }

    /// Marks the document as stored at `path`, with that file's name.
    pub fn set_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        self.name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Board.rizref")
            .to_owned();
        self.path = Some(path);
    }

    /// Marks the document as changed. Returns whether the flag actually moved.
    pub fn mark_dirty(&mut self) -> bool {
        let changed = !self.dirty;
        self.dirty = true;
        changed
    }

    /// Marks the document as saved.
    pub fn mark_saved(&mut self) -> bool {
        let changed = self.dirty;
        self.dirty = false;
        changed
    }

    /// The title shown in the custom title bar.
    ///
    /// Unsaved documents get the asterisk before the extension, matching the
    /// common editor convention: `Document1*.rizref`.
    pub fn display_title(&self) -> String {
        if !self.dirty {
            return self.name.clone();
        }
        match self.name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
                format!("{stem}*.{extension}")
            }
            _ => format!("{}*", self.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Document;

    #[test]
    fn a_new_document_is_unsaved_and_marked() {
        let document = Document::untitled(1);
        assert!(document.is_dirty());
        assert_eq!(document.display_title(), "Document1*.rizref");
    }

    #[test]
    fn saving_clears_the_marker_and_editing_restores_it() {
        let mut document = Document::untitled(2);
        assert!(document.mark_saved());
        assert!(!document.is_dirty());
        assert_eq!(document.display_title(), "Document2.rizref");
        assert!(document.mark_dirty());
        assert!(!document.mark_dirty(), "already dirty");
        assert_eq!(document.display_title(), "Document2*.rizref");
    }

    #[test]
    fn a_name_without_an_extension_gets_a_trailing_marker() {
        let document = Document {
            name: "Notes".to_owned(),
            path: None,
            dirty: true,
        };
        assert_eq!(document.display_title(), "Notes*");
    }

    #[test]
    fn an_opened_document_is_clean_and_keeps_its_path() {
        let document = Document::from_path("C:/boards/hands study.rizref");

        assert!(!document.is_dirty());
        assert_eq!(document.display_title(), "hands study.rizref");
        assert_eq!(document.stem(), "hands study");
        assert_eq!(
            document.path().expect("path"),
            std::path::Path::new("C:/boards/hands study.rizref")
        );
    }

    #[test]
    fn saving_under_a_new_path_renames_the_document() {
        let mut document = Document::untitled(3);
        document.set_path("C:/boards/hero refs.rizref");
        document.mark_saved();

        assert_eq!(document.display_title(), "hero refs.rizref");
        assert_eq!(
            document.path().expect("path").to_string_lossy(),
            "C:/boards/hero refs.rizref"
        );
    }
}
