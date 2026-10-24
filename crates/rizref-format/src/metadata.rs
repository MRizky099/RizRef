//! The board metadata payload.
//!
//! Stored as JSON in the `META` chunk — not in the compact binary the scene
//! itself will use — because this is the part of a file that has to stay
//! inspectable and forgiving: fields added by a newer build are ignored rather
//! than lost, a missing field falls back to its default instead of failing the
//! load, and a board whose scene payload cannot be read still says what it is.
//!
//! Nothing here contains the board's identity: that lives in the header, which
//! is what lets the metadata payload be rewritten without touching the framing.

use serde::{Deserialize, Serialize};

use crate::error::FormatError;

/// What a board knows about itself besides the scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneMetadata {
    /// Revision of this payload, bumped whenever a field changes meaning.
    #[serde(default = "SceneMetadata::written_schema")]
    pub schema: u16,
    /// Where the imported images came from.
    ///
    /// Board-relative wherever possible, so a board folder can be copied to
    /// another machine — or another drive letter — and still make sense.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Free-form note shown in the board's info panel.
    #[serde(default)]
    pub note: String,
}

impl SceneMetadata {
    /// Revision this build writes.
    pub const SCHEMA: u16 = 1;

    /// The schema revision, in the form `#[serde(default = …)]` needs.
    #[must_use]
    pub const fn written_schema() -> u16 {
        Self::SCHEMA
    }

    /// Metadata for a new, empty board.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serializes to UTF-8 JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        serde_json::to_vec(self).map_err(|error| FormatError::Metadata {
            reason: error.to_string(),
        })
    }

    /// Parses UTF-8 JSON, applying defaults for fields that are absent.
    pub fn from_json(bytes: &[u8]) -> Result<Self, FormatError> {
        serde_json::from_slice(bytes).map_err(|error| FormatError::Metadata {
            reason: error.to_string(),
        })
    }
}

impl Default for SceneMetadata {
    fn default() -> Self {
        Self {
            schema: Self::SCHEMA,
            sources: Vec::new(),
            note: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SceneMetadata;

    #[test]
    fn metadata_round_trips_through_json() {
        let metadata = SceneMetadata {
            schema: SceneMetadata::SCHEMA,
            sources: vec![
                "board/images/hands-01.png".to_owned(),
                "board/images/hands-02.jpg".to_owned(),
            ],
            note: "left hand studies, warm light".to_owned(),
        };

        let encoded = metadata.to_json().expect("serializes");

        assert_eq!(
            SceneMetadata::from_json(&encoded).expect("parses"),
            metadata
        );
    }

    #[test]
    fn a_new_board_has_the_current_schema_and_nothing_else() {
        let metadata = SceneMetadata::new();

        assert_eq!(metadata.schema, SceneMetadata::SCHEMA);
        assert!(metadata.sources.is_empty());
        assert!(metadata.note.is_empty());
    }

    #[test]
    fn a_field_from_a_newer_build_is_ignored_rather_than_fatal() {
        let json = br#"{
            "schema": 1,
            "sources": ["board/images/ref.png"],
            "note": "",
            "colour_management": "display-p3"
        }"#;

        let metadata = SceneMetadata::from_json(json).expect("unknown fields must be tolerated");

        assert_eq!(metadata.sources, vec!["board/images/ref.png".to_owned()]);
    }

    #[test]
    fn absent_fields_fall_back_to_their_defaults() {
        let metadata = SceneMetadata::from_json(b"{}").expect("an empty object is valid");

        assert_eq!(metadata.schema, SceneMetadata::SCHEMA);
        assert!(metadata.note.is_empty());
    }

    #[test]
    fn malformed_json_is_reported_as_a_metadata_error() {
        let error = SceneMetadata::from_json(b"{ not json").expect_err("must not parse");

        assert!(
            error.to_string().contains("cannot read scene metadata"),
            "unexpected message: {error}"
        );
    }
}
