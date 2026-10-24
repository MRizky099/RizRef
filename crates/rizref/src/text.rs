//! Text shaping for the HUD and for notes.
//!
//! One `FontSystem` is created for the process and reused: building the font
//! database means reading the system's font registry and, on Windows, opening
//! every face it lists. Doing that per frame is the difference between a HUD
//! that costs microseconds and one that costs milliseconds.
//!
//! Phase 00 proves the stack loads and reports how many faces it found. Shaping
//! HUD labels and rendering note text is phase 04/05 work.

use cosmic_text::FontSystem;

/// Owns the font database and the shaping cache.
pub struct TextEngine {
    fonts: FontSystem,
}

impl TextEngine {
    /// Loads the system font database.
    ///
    /// Never fails: `cosmic-text` falls back to an empty database when it cannot
    /// read the font registry, and an empty database still renders the built-in
    /// fallback. A board full of images should not refuse to open because a font
    /// directory is unreadable.
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
        }
    }

    /// How many font faces are available.
    ///
    /// Reported at startup and worth having in a bug report: "the labels are
    /// boxes" is almost always a font database that came up empty.
    pub fn font_count(&self) -> usize {
        self.fonts.db().len()
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::TextEngine;

    #[test]
    fn the_system_font_database_is_not_empty() {
        // A machine with no readable fonts at all is broken enough that this
        // failing is more useful than the HUD silently drawing nothing.
        assert!(TextEngine::new().font_count() > 0);
    }

    #[test]
    fn creating_an_engine_twice_is_allowed() {
        // Guards against a hidden process-wide singleton: the font system will
        // have to be constructible per document once boards can be reopened.
        let first = TextEngine::new();
        let second = TextEngine::new();

        assert_eq!(first.font_count(), second.font_count());
    }
}
