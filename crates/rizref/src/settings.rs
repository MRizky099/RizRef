//! Persistent application settings.
//!
//! `%APPDATA%\RizRef\settings.json`, owned by this module and read/written with
//! serde. Every field carries its default, so an older file keeps working after
//! a build adds a setting, and an unknown field from a newer build is ignored
//! rather than rejected. Loading never fails the launch: a corrupt file is
//! logged and replaced by defaults in memory, then the next save heals it.
//!
//! The shortcut overrides live here too, keyed by the stable [`ShortcutId`]
//! names; [`crate::shortcuts`] owns their semantics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::shortcuts::{KeyChord, ShortcutId};

/// The canvas background presets the Canvas menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CanvasTheme {
    /// The default dark grey.
    #[default]
    Charcoal,
    /// A cooler blue-grey.
    Slate,
    /// Pure black for OLED displays.
    Black,
    /// A warm off-white for printed reference.
    Paper,
}

impl CanvasTheme {
    /// Every preset, in menu order.
    pub const ALL: [Self; 4] = [Self::Charcoal, Self::Slate, Self::Black, Self::Paper];

    /// The display name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Charcoal => "Charcoal",
            Self::Slate => "Slate",
            Self::Black => "Black",
            Self::Paper => "Paper",
        }
    }

    /// The clear colour.
    pub const fn rgba(self) -> [u8; 4] {
        match self {
            Self::Charcoal => [24, 24, 28, 255],
            Self::Slate => [30, 36, 46, 255],
            Self::Black => [0, 0, 0, 255],
            Self::Paper => [232, 226, 214, 255],
        }
    }
}

/// Everything the application remembers between launches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Upper bound on the decoded-bitmap cache, in bytes.
    pub max_decode_bytes: u64,
    /// Whether the canvas grid draws.
    pub grid_visible: bool,
    /// Whether dragging snaps to edges and centres.
    pub snap_enabled: bool,
    /// Whether the HUD readout draws.
    pub hud_visible: bool,
    /// Whether the dashed scene-rect border draws.
    pub scene_border: bool,
    /// Whether the checkerboard backdrop draws.
    pub checkerboard: bool,
    /// Whether items draw in grayscale.
    pub grayscale: bool,
    /// The canvas clear colour preset.
    pub canvas_theme: CanvasTheme,
    /// Whether the window starts always-on-top.
    pub always_on_top: bool,
    /// Window opacity for the ordinary transparency mode, 20–100 %.
    ///
    /// Stored as `canvas_opacity_percent` since phase 07, when transparency
    /// became the canvas background only. Older files wrote the
    /// whole-window `opacity_percent`; it is read into this field (the same
    /// steps, now meaning the canvas), and the next save writes the new name.
    #[serde(rename = "canvas_opacity_percent", alias = "opacity_percent")]
    pub opacity_percent: u8,
    /// Window opacity while ghost mode is active, 10–40 %.
    pub ghost_opacity_percent: u8,
    /// Rebound shortcuts; absent actions keep their defaults.
    #[serde(default, deserialize_with = "deserialize_shortcut_overrides")]
    pub shortcuts: BTreeMap<ShortcutId, KeyChord>,
}

/// Reads shortcut overrides one entry at a time, skipping malformed ones.
///
/// A typo in one chord must not discard the rest of settings.json, and it must
/// not prevent the application from launching.
fn deserialize_shortcut_overrides<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<ShortcutId, KeyChord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = BTreeMap::<ShortcutId, String>::deserialize(deserializer)?;
    let mut overrides = BTreeMap::new();
    for (action, text) in raw {
        match text.parse::<KeyChord>() {
            Ok(chord) => {
                overrides.insert(action, chord);
            }
            Err(_) => tracing::warn!(
                action = ?action,
                chord = %text,
                "ignoring an unreadable saved shortcut"
            ),
        }
    }
    Ok(overrides)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_decode_bytes: crate::memory::DEFAULT_DECODE_CACHE_BYTES,
            // Off on first launch: a reference board reads cleaner without
            // the grid and its origin axis; View → Grid turns both on.
            grid_visible: false,
            snap_enabled: true,
            hud_visible: true,
            scene_border: true,
            checkerboard: false,
            grayscale: false,
            canvas_theme: CanvasTheme::default(),
            always_on_top: false,
            opacity_percent: 100,
            ghost_opacity_percent: crate::platform::window_modes::GHOST_OPACITY_PERCENT,
            shortcuts: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Clamps values that a hand-edited file could put out of range.
    pub fn sanitize(&mut self) {
        self.max_decode_bytes = self
            .max_decode_bytes
            .clamp(128 * 1024 * 1024, 8 * 1024 * 1024 * 1024);
        self.opacity_percent = self.opacity_percent.clamp(20, 100);
        self.ghost_opacity_percent = self.ghost_opacity_percent.clamp(10, 40);
    }

    /// Loads the settings file, falling back to defaults.
    pub fn load() -> Self {
        let path = paths::settings_file();
        let mut settings = match std::fs::read_to_string(&path) {
            Ok(body) => match serde_json::from_str::<Self>(&body) {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "settings file is not valid; using defaults"
                    );
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "cannot read settings; using defaults"
                );
                Self::default()
            }
        };
        settings.sanitize();
        settings
    }

    /// Writes the settings file atomically (temp file, then rename).
    ///
    /// A failure is logged and returned; settings are a convenience, never a
    /// reason to interrupt the user's work.
    pub fn save(&self) -> std::io::Result<()> {
        let path = paths::settings_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, body)?;
        std::fs::rename(&temporary, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::{CanvasTheme, Settings};
    use crate::shortcuts::{Key, KeyChord, ShortcutId};

    #[test]
    fn the_defaults_match_the_product_decisions() {
        let settings = Settings::default();
        assert_eq!(settings.canvas_theme, CanvasTheme::Charcoal);
        assert_eq!(settings.canvas_theme.rgba(), [24, 24, 28, 255]);
        assert!(!settings.grid_visible, "grid and origin axis start hidden");
        assert!(settings.snap_enabled);
        assert!(settings.hud_visible);
        assert!(settings.scene_border);
        assert_eq!(settings.opacity_percent, 100);
        assert_eq!(settings.ghost_opacity_percent, 15);
    }

    #[test]
    fn settings_round_trip_through_json_with_shortcut_overrides() {
        let mut settings = Settings {
            grayscale: true,
            canvas_theme: CanvasTheme::Paper,
            ..Settings::default()
        };
        settings
            .shortcuts
            .insert(ShortcutId::Undo, KeyChord::ctrl(Key::U));

        let body = serde_json::to_string(&settings).expect("serialises");
        let back: Settings = serde_json::from_str(&body).expect("parses");

        assert_eq!(back, settings);
        assert_eq!(
            back.shortcuts.get(&ShortcutId::Undo),
            Some(&KeyChord::ctrl(Key::U))
        );
    }

    #[test]
    fn absent_and_unknown_fields_are_tolerated() {
        let settings: Settings =
            serde_json::from_str(r#"{"grayscale":true,"future_field":42}"#).expect("tolerated");
        assert!(settings.grayscale);
        assert!(!settings.grid_visible, "absent fields keep their default");

        // A saved choice still wins over the new default.
        let saved: Settings = serde_json::from_str(r#"{"grid_visible":true}"#).expect("parses");
        assert!(saved.grid_visible);
    }

    #[test]
    fn one_bad_shortcut_does_not_discard_the_rest() {
        let settings: Settings = serde_json::from_str(
            r#"{"grayscale":true,"shortcuts":{"undo":"NotAKey","redo":"Ctrl+Shift+U"}}"#,
        )
        .expect("the file must still load");
        assert!(settings.grayscale);
        assert!(!settings.shortcuts.contains_key(&ShortcutId::Undo));
        assert_eq!(
            settings.shortcuts.get(&ShortcutId::Redo),
            Some(&KeyChord::ctrl_shift(Key::U))
        );
    }

    #[test]
    fn sanitize_clamps_hand_edited_values() {
        let mut settings = Settings {
            opacity_percent: 0,
            ghost_opacity_percent: 250,
            max_decode_bytes: 1,
            ..Settings::default()
        };
        settings.sanitize();

        assert_eq!(settings.opacity_percent, 20);
        assert_eq!(settings.ghost_opacity_percent, 40);
        assert_eq!(settings.max_decode_bytes, 128 * 1024 * 1024);
    }

    #[test]
    fn every_theme_has_a_distinct_opaque_colour() {
        for (index, theme) in CanvasTheme::ALL.iter().enumerate() {
            assert_eq!(theme.rgba()[3], 255);
            for other in &CanvasTheme::ALL[index + 1..] {
                assert_ne!(theme.rgba(), other.rgba());
            }
        }
    }

    #[test]
    fn the_old_window_opacity_setting_becomes_the_canvas_opacity() {
        let old: Settings = serde_json::from_str(r#"{"opacity_percent": 60}"#).expect("parses");
        assert_eq!(old.opacity_percent, 60);

        let written = serde_json::to_string(&old).expect("serialises");
        assert!(
            written.contains(r#""canvas_opacity_percent":60"#),
            "{written}"
        );
        assert!(!written.contains(r#""opacity_percent""#));
        let back: Settings = serde_json::from_str(&written).expect("parses");
        assert_eq!(back.opacity_percent, 60);
    }
}
