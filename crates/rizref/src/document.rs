//! The open document's identity and unsaved state.
//!
//! Only the title-bar shell needs this today: the display name, the unsaved
//! marker and the dirty flag that guards closing the window. The `.rizref`
//! reader/writer in phase 06 will fill in a real path and clear the flag on a
//! successful save; keeping the state here means the window title, the close
//! guard and that future save path cannot disagree about what "unsaved" means.

/// One open board document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// File name including the `.rizref` extension.
    name: String,
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
            dirty: true,
        }
    }

    /// Whether the document has unsaved changes.
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Marks the document as changed. Returns whether the flag actually moved.
    pub fn mark_dirty(&mut self) -> bool {
        let changed = !self.dirty;
        self.dirty = true;
        changed
    }

    /// Marks the document as saved. Used by the phase-06 save path.
    #[allow(dead_code)]
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
            dirty: true,
        };
        assert_eq!(document.display_title(), "Notes*");
    }
}
