//! The kinds of chunk a payload can be made of.
//!
//! A payload is a sequence of chunks, each introduced by a four-character tag.
//! Tags rather than numeric codes because they survive a hexdump: when a board
//! has to be inspected by hand, `META`/`SCEN`/`IMGS` say what they are, and a
//! reader that meets a tag from a newer build can skip it without guessing.
//!
//! The framing itself — tag, 64-bit length, payload, digest — is written by
//! phase 06 together with the writer. This module fixes the vocabulary now so
//! the reader, the writer and the study notes cannot drift apart.

/// Length of a chunk tag in bytes.
pub const TAG_LEN: usize = 4;

/// What a payload chunk contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChunkKind {
    /// Scene metadata as JSON; see [`crate::metadata::SceneMetadata`].
    Metadata,
    /// The serialized scene: items, transforms, groups and z-order.
    Scene,
    /// One original image, byte-for-byte as it was imported.
    ///
    /// Keeping the original bytes is a product decision, not an implementation
    /// detail: it is what preserves embedded metadata (EXIF, C2PA content
    /// credentials) that re-encoding would destroy.
    ImageBlob,
    /// The embedded board preview, so a recent-boards list can show a thumbnail
    /// without decoding the board.
    Preview,
    /// A preview of a single item, keyed by item id.
    ItemPreview,
}

impl ChunkKind {
    /// Every kind this build knows, in the order phase 06 will write them.
    pub const ALL: [Self; 5] = [
        Self::Metadata,
        Self::Scene,
        Self::ImageBlob,
        Self::Preview,
        Self::ItemPreview,
    ];

    /// The tag stored in the container.
    pub const fn tag(self) -> [u8; TAG_LEN] {
        match self {
            Self::Metadata => *b"META",
            Self::Scene => *b"SCEN",
            Self::ImageBlob => *b"IMGS",
            Self::Preview => *b"PRVW",
            Self::ItemPreview => *b"ITMP",
        }
    }

    /// The kind for `tag`, or `None` when this build does not know it.
    ///
    /// `None` is not an error: a chunk from a newer revision is skipped by its
    /// length, never interpreted.
    pub fn from_tag(tag: [u8; TAG_LEN]) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.tag() == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::{ChunkKind, TAG_LEN};

    #[test]
    fn every_kind_round_trips_through_its_tag() {
        for kind in ChunkKind::ALL {
            assert_eq!(ChunkKind::from_tag(kind.tag()), Some(kind));
        }
    }

    #[test]
    fn tags_are_distinct() {
        for (index, kind) in ChunkKind::ALL.iter().enumerate() {
            for other in &ChunkKind::ALL[index + 1..] {
                assert_ne!(
                    kind.tag(),
                    other.tag(),
                    "{kind:?} and {other:?} share a tag"
                );
            }
        }
    }

    #[test]
    fn unknown_tags_are_reported_as_unknown() {
        assert_eq!(ChunkKind::from_tag(*b"ZZZZ"), None);
        assert_eq!(
            ChunkKind::from_tag(*b"meta"),
            None,
            "tags are case sensitive"
        );
        assert_eq!(ChunkKind::from_tag(*b"META"), Some(ChunkKind::Metadata));
    }

    #[test]
    fn tags_are_frozen_by_the_file_format() {
        // These literals are part of the format: renaming one silently breaks
        // every board written by an earlier build.
        assert_eq!(ChunkKind::Metadata.tag(), *b"META");
        assert_eq!(ChunkKind::Scene.tag(), *b"SCEN");
        assert_eq!(ChunkKind::ImageBlob.tag(), *b"IMGS");
        assert_eq!(ChunkKind::Preview.tag(), *b"PRVW");
        assert_eq!(ChunkKind::ItemPreview.tag(), *b"ITMP");
        assert_eq!(TAG_LEN, 4);
    }
}
