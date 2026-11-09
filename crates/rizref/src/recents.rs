//! The start view's recent-board list.
//!
//! `%APPDATA%\RizRef\recents.json`. A recent entry carries the path, when the
//! board was last edited, its item count and optionally the board thumbnail
//! bytes, so a card can be drawn without opening the board. The save and open
//! paths record the thumbnail; this module only stores and prunes what it is
//! given.
//!
//! Paths are compared case-insensitively on Windows, because the same board can
//! be reached through different casing; duplicates would otherwise pile up.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;

/// How many recent boards the start view keeps.
pub const RECENT_LIMIT: usize = 12;

/// One recent board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    /// Absolute path to the `.rizref` file.
    pub path: String,
    /// Unix seconds of the last edit.
    pub edited_at: u64,
    /// Item count at the last edit.
    pub item_count: u32,
    /// Encoded preview bytes, when the board carried one. Written as base64;
    /// files from before phase 07 hold a JSON array of numbers, which is
    /// still read (it took about three times the space).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "preview_bytes"
    )]
    pub preview: Option<Vec<u8>>,
}

/// Serde for the preview: a base64 string out, base64 or a number array in.
mod preview_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ref_option)] // serde's `with` hands the field by reference
    pub fn serialize<S: Serializer>(value: &Option<Vec<u8>>, out: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => out.serialize_some(&super::base64_encode(bytes)),
            None => out.serialize_none(),
        }
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Base64(String),
        Numbers(Vec<u8>),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<Option<Vec<u8>>, D::Error> {
        Ok(match Option::<Stored>::deserialize(input)? {
            Some(Stored::Base64(text)) => super::base64_decode(&text),
            Some(Stored::Numbers(bytes)) => Some(bytes),
            None => None,
        })
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 §4).
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(BASE64[(triple >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64; `None` for anything malformed (a bad preview
/// only costs the card its picture).
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0_u32, 0);
    for byte in text.bytes() {
        let value = BASE64.iter().position(|c| *c == byte)? as u32;
        acc = acc << 6 | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xFF) as u8);
        }
    }
    Some(out)
}

impl RecentEntry {
    /// The file name without its `.rizref` extension, or the full name.
    pub fn display_name(&self) -> String {
        Path::new(&self.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or(&self.path)
            .to_owned()
    }

    /// The absolute path as a `PathBuf`.
    pub fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    /// "Edited 12 min ago", derived from the stored timestamp.
    pub fn edited_label(&self, now_unix: u64) -> String {
        format!("Edited {}", relative_time(self.edited_at, now_unix))
    }
}

/// "just now", "12 min ago", "3 h ago", "yesterday", "4 days ago".
pub fn relative_time(then_unix: u64, now_unix: u64) -> String {
    let seconds = now_unix.saturating_sub(then_unix);
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        86_400..=172_799 => "yesterday".to_owned(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// The recent-board list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recents {
    /// Entries, newest first.
    pub entries: Vec<RecentEntry>,
}

impl Recents {
    /// Loads the list, falling back to empty on any error.
    pub fn load() -> Self {
        let path = paths::recents_file();
        match std::fs::read_to_string(&path) {
            Ok(body) => match serde_json::from_str::<Self>(&body) {
                Ok(mut recents) => {
                    recents.entries.truncate(RECENT_LIMIT);
                    recents
                }
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "recents file is not valid; starting empty"
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Writes the list atomically; errors are returned for the caller to log.
    pub fn save(&self) -> std::io::Result<()> {
        let path = paths::recents_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, body)?;
        std::fs::rename(&temporary, &path)
    }

    /// Records a board at the front, replacing an older entry for the same path.
    ///
    /// `now` is passed in rather than read here so the ordering is testable.
    pub fn record(
        &mut self,
        path: impl Into<String>,
        item_count: u32,
        preview: Option<Vec<u8>>,
        now_unix: u64,
    ) {
        let path = path.into();
        self.entries
            .retain(|entry| !entry.path.eq_ignore_ascii_case(&path));
        self.entries.insert(
            0,
            RecentEntry {
                path,
                edited_at: now_unix,
                item_count,
                preview,
            },
        );
        self.entries.truncate(RECENT_LIMIT);
    }

    /// Removes one path from the list. Returns whether it was present.
    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !entry.path.eq_ignore_ascii_case(path));
        self.entries.len() != before
    }

    /// Drops entries whose files no longer exist. Returns how many were removed.
    pub fn retain_existing(&mut self) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|entry| Path::new(&entry.path).is_file());
        before - self.entries.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The current Unix time in seconds, saturating at zero before 1970.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::{RECENT_LIMIT, Recents, relative_time};

    #[test]
    fn relative_time_reads_like_a_person_wrote_it() {
        assert_eq!(relative_time(100, 100), "just now");
        assert_eq!(relative_time(100, 159), "just now");
        assert_eq!(relative_time(100, 160), "1 min ago");
        assert_eq!(relative_time(100, 100 + 12 * 60), "12 min ago");
        assert_eq!(relative_time(100, 100 + 3 * 3_600), "3 h ago");
        assert_eq!(relative_time(100, 100 + 86_400), "yesterday");
        assert_eq!(relative_time(100, 100 + 4 * 86_400), "4 days ago");
    }

    #[test]
    fn a_recorded_board_moves_to_the_front_and_deduplicates_by_path() {
        let mut recents = Recents::default();
        recents.record("C:/boards/a.rizref", 3, None, 10);
        recents.record("C:/boards/b.rizref", 4, None, 20);
        recents.record("c:/boards/A.rizref", 5, None, 30);

        assert_eq!(recents.entries.len(), 2);
        assert_eq!(recents.entries[0].path, "c:/boards/A.rizref");
        assert_eq!(recents.entries[0].item_count, 5);
        assert_eq!(recents.entries[1].path, "C:/boards/b.rizref");
    }

    #[test]
    fn the_list_is_capped() {
        let mut recents = Recents::default();
        for index in 0..(RECENT_LIMIT + 5) {
            recents.record(format!("C:/boards/{index}.rizref"), 1, None, index as u64);
        }
        assert_eq!(recents.entries.len(), RECENT_LIMIT);
        assert_eq!(
            recents.entries[0].path,
            format!("C:/boards/{}.rizref", RECENT_LIMIT + 4)
        );
    }

    #[test]
    fn removal_is_case_insensitive_and_reports_presence() {
        let mut recents = Recents::default();
        recents.record("C:/boards/a.rizref", 1, None, 0);

        assert!(recents.remove("c:/BOARDS/A.RIZREF"));
        assert!(recents.is_empty());
        assert!(!recents.remove("c:/boards/a.rizref"));
    }

    #[test]
    fn the_display_name_drops_the_extension() {
        let mut recents = Recents::default();
        recents.record("C:/boards/hands study.rizref", 2, None, 0);
        assert_eq!(recents.entries[0].display_name(), "hands study");
    }

    #[test]
    fn the_list_round_trips_through_json() {
        let mut recents = Recents::default();
        recents.record("C:/boards/a.rizref", 3, Some(vec![1, 2, 3]), 42);

        let body = serde_json::to_string(&recents).expect("serialises");
        let back: Recents = serde_json::from_str(&body).expect("parses");

        assert_eq!(back, recents);
        assert_eq!(back.entries[0].edited_at, 42);
        assert_eq!(back.entries[0].preview, Some(vec![1, 2, 3]));
        assert!(
            body.contains("\"preview\":\"AQID\""),
            "base64 on disk: {body}"
        );
    }

    #[test]
    fn base64_round_trips_every_length_and_byte() {
        for len in 0..70 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = super::base64_encode(&bytes);
            assert_eq!(text.len() % 4, 0);
            assert_eq!(super::base64_decode(&text), Some(bytes));
        }
        assert_eq!(super::base64_encode(b"Man"), "TWFu");
        assert_eq!(super::base64_encode(b"Ma"), "TWE=");
        assert_eq!(super::base64_decode("not base64!"), None);
    }

    #[test]
    fn a_recents_file_with_number_array_previews_still_loads() {
        let body = r#"{"entries":[{"path":"C:/b.rizref","edited_at":5,"item_count":2,"preview":[255,216,255,224]}]}"#;
        let recents: Recents = serde_json::from_str(body).expect("old form parses");
        assert_eq!(recents.entries[0].preview, Some(vec![255, 216, 255, 224]));
    }
}
