//! Shared image assets: content-addressed originals plus their previews.
//!
//! An asset is the bytes of one image file, kept byte-for-byte, together with
//! the decoded facts the board needs: format, pixel size and a small preview.
//! Assets are shared: dropping the same file twice creates two items that point
//! at one asset. Identity is the BLAKE3 checksum of the original encoded bytes,
//! so the same picture found through two paths, two file names or two imports
//! deduplicates without comparing pixels.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use rizref_codecs::{ImageBuffer, ImageFormat};
use serde::{Deserialize, Serialize};

use crate::animation::Animation;

/// The full BLAKE3 digest of `bytes`.
pub fn checksum(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Identity of an asset: the checksum of its original encoded bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AssetId([u8; 32]);

impl AssetId {
    /// Length of the raw id in bytes.
    pub const BYTE_LEN: usize = 32;
    /// Length of the hex rendering in characters.
    pub const HEX_LEN: usize = 64;

    /// Wraps a checksum as an asset id.
    pub const fn from_checksum(checksum: [u8; Self::BYTE_LEN]) -> Self {
        Self(checksum)
    }

    /// The raw checksum bytes.
    pub const fn as_bytes(&self) -> &[u8; Self::BYTE_LEN] {
        &self.0
    }

    /// The id as 64 lowercase hex digits.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(Self::HEX_LEN);
        for byte in self.0 {
            use std::fmt::Write as _;
            write!(out, "{byte:02x}").expect("writing to a String cannot fail");
        }
        out
    }

    /// Parses exactly 64 hex digits, accepting upper- and lowercase.
    ///
    /// Used when a `.rizref` document names its assets by content hash.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.as_bytes();
        if digits.len() != Self::HEX_LEN {
            return None;
        }
        let mut raw = [0u8; Self::BYTE_LEN];
        for (pair, slot) in digits.as_chunks::<2>().0.iter().zip(raw.iter_mut()) {
            *slot = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
        }
        Some(Self(raw))
    }
}

/// Value of a single hex digit, or `None` if the byte is not one.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

/// A small RGBA thumbnail of an asset, at most [`crate::preview::PREVIEW_SIDE`]
/// pixels on its longest side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewImage {
    image: ImageBuffer,
}

impl PreviewImage {
    /// Wraps an already generated preview.
    pub fn new(image: ImageBuffer) -> Self {
        Self { image }
    }

    /// The preview pixels.
    pub fn image(&self) -> &ImageBuffer {
        &self.image
    }

    /// Preview width in pixels.
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// Preview height in pixels.
    pub fn height(&self) -> u32 {
        self.image.height()
    }
}

/// One immutable original image shared by every item that references it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    id: AssetId,
    format: ImageFormat,
    checksum: [u8; AssetId::BYTE_LEN],
    encoded: Arc<[u8]>,
    pixel_width: u32,
    pixel_height: u32,
    preview: PreviewImage,
    animation: Option<Arc<Animation>>,
}

impl Asset {
    /// Builds an asset from already decoded pixels and a generated preview,
    /// computing the checksum over `encoded`.
    pub fn from_decoded(
        encoded: Arc<[u8]>,
        format: ImageFormat,
        image: &ImageBuffer,
        preview: ImageBuffer,
    ) -> Self {
        let checksum = checksum(&encoded);
        Self::from_parts(encoded, format, checksum, image, preview)
    }

    /// Builds an asset from a checksum the caller already computed.
    ///
    /// The import worker hashes the bytes off the UI thread, so the main thread
    /// never pays for a second pass over a large file.
    pub fn from_parts(
        encoded: Arc<[u8]>,
        format: ImageFormat,
        checksum: [u8; AssetId::BYTE_LEN],
        image: &ImageBuffer,
        preview: ImageBuffer,
    ) -> Self {
        Self {
            id: AssetId::from_checksum(checksum),
            format,
            checksum,
            encoded,
            pixel_width: image.width(),
            pixel_height: image.height(),
            preview: PreviewImage::new(preview),
            animation: None,
        }
    }

    /// Builds an asset from decoded facts the file already stored.
    ///
    /// Loading a board does not decode every original just to learn its size:
    /// the `.rizref` document carries the pixel dimensions, and the stored
    /// preview supplies the thumbnail. The full bitmap is decoded on demand by
    /// the renderer, exactly as it is for an asset whose bitmap was evicted.
    pub fn from_meta(
        encoded: Arc<[u8]>,
        format: ImageFormat,
        checksum: [u8; AssetId::BYTE_LEN],
        pixel_width: u32,
        pixel_height: u32,
        preview: ImageBuffer,
    ) -> Self {
        Self {
            id: AssetId::from_checksum(checksum),
            format,
            checksum,
            encoded,
            pixel_width,
            pixel_height,
            preview: PreviewImage::new(preview),
            animation: None,
        }
    }

    /// Attaches an animation, as an animated GIF import does.
    ///
    /// The asset's still image stays the first frame, so everything that does
    /// not know about animations — previews, the file format, exports — keeps
    /// working unchanged.
    pub fn with_animation(mut self, animation: Animation) -> Self {
        self.animation = Some(Arc::new(animation));
        self
    }

    /// Identity of this asset.
    pub fn id(&self) -> AssetId {
        self.id
    }

    /// The format the original bytes are in.
    pub fn format(&self) -> ImageFormat {
        self.format
    }

    /// The BLAKE3 digest of [`Asset::encoded`].
    pub fn checksum(&self) -> &[u8; AssetId::BYTE_LEN] {
        &self.checksum
    }

    /// The original encoded bytes, verbatim.
    pub fn encoded(&self) -> &Arc<[u8]> {
        &self.encoded
    }

    /// Decoded pixel width.
    pub fn pixel_width(&self) -> u32 {
        self.pixel_width
    }

    /// Decoded pixel height.
    pub fn pixel_height(&self) -> u32 {
        self.pixel_height
    }

    /// The embedded thumbnail shown while the full texture is not resident.
    pub fn preview(&self) -> &PreviewImage {
        &self.preview
    }

    /// The animation this asset plays, when it is animated.
    pub fn animation(&self) -> Option<&Arc<Animation>> {
        self.animation.as_ref()
    }
}

/// Content-addressed storage for the board's assets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetStore {
    assets: HashMap<AssetId, Asset>,
}

impl AssetStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts `asset` unless an asset with the same checksum is already
    /// present, returning its id and whether this call actually inserted it.
    pub fn intern(&mut self, asset: Asset) -> (AssetId, bool) {
        let id = asset.id();
        match self.assets.entry(id) {
            std::collections::hash_map::Entry::Occupied(_) => (id, false),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(asset);
                (id, true)
            }
        }
    }

    /// Replaces the asset with the same id, returning the previous one.
    ///
    /// The only caller is a background re-decode that attaches an animation to
    /// an asset loaded from a file. Content addressing makes the replacement
    /// safe: same id means same original bytes.
    pub fn replace(&mut self, asset: Asset) -> Option<Asset> {
        self.assets.insert(asset.id(), asset)
    }

    /// The asset with `id`, if present.
    pub fn get(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(&id)
    }

    /// Whether `id` is stored.
    pub fn contains(&self, id: AssetId) -> bool {
        self.assets.contains_key(&id)
    }

    /// Number of distinct assets.
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    /// Whether no asset is stored.
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    /// Every stored asset, in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (&AssetId, &Asset)> {
        self.assets.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{Asset, AssetId, AssetStore};
    use rizref_codecs::{ImageBuffer, ImageFormat};
    use std::sync::Arc;

    fn image() -> ImageBuffer {
        ImageBuffer::filled(4, 3, [10, 20, 30, 255])
    }

    fn asset(bytes: &[u8]) -> Asset {
        let pixels = image();
        Asset::from_decoded(
            Arc::from(bytes),
            ImageFormat::Png,
            &pixels,
            ImageBuffer::filled(2, 1, [1, 2, 3, 255]),
        )
    }

    #[test]
    fn the_same_bytes_intern_once_and_report_the_first_insert() {
        let mut store = AssetStore::new();

        let (first, new_first) = store.intern(asset(b"same file"));
        let (second, new_second) = store.intern(asset(b"same file"));

        assert_eq!(first, second);
        assert!(new_first);
        assert!(!new_second, "the second intern must not replace anything");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn different_bytes_are_different_assets() {
        let mut store = AssetStore::new();

        let (a, _) = store.intern(asset(b"one"));
        let (b, _) = store.intern(asset(b"two"));

        assert_ne!(a, b);
        assert_eq!(store.len(), 2);
        assert!(store.contains(a) && store.contains(b));
    }

    #[test]
    fn the_id_is_the_checksum_of_the_original_bytes() {
        let bytes = b"an original image";
        let asset = asset(bytes);

        assert_eq!(asset.id(), AssetId::from_checksum(super::checksum(bytes)));
        assert_eq!(asset.checksum(), &super::checksum(bytes));
        assert_eq!(asset.id().to_hex().len(), AssetId::HEX_LEN);
        assert_eq!(asset.encoded().as_ref(), bytes);
    }

    #[test]
    fn an_asset_keeps_the_decoded_size_and_the_preview() {
        let asset = asset(b"sized");

        assert_eq!((asset.pixel_width(), asset.pixel_height()), (4, 3));
        assert_eq!(asset.format(), ImageFormat::Png);
        assert_eq!((asset.preview().width(), asset.preview().height()), (2, 1));
    }

    #[test]
    fn asset_ids_round_trip_through_hex() {
        let id = AssetId::from_checksum(super::checksum(b"an original image"));

        let hex = id.to_hex();

        assert_eq!(hex.len(), AssetId::HEX_LEN);
        assert_eq!(AssetId::from_hex(&hex), Some(id));
        assert_eq!(AssetId::from_hex(&hex.to_uppercase()), Some(id));
        assert_eq!(AssetId::from_hex(""), None);
        assert_eq!(AssetId::from_hex(&"0".repeat(63)), None);
        assert_eq!(AssetId::from_hex(&"z".repeat(64)), None);
    }

    #[test]
    fn an_asset_can_be_built_from_stored_facts_without_a_full_decode() {
        let bytes = b"stored original";
        let checksum = super::checksum(bytes);
        let asset = Asset::from_meta(
            Arc::from(&bytes[..]),
            ImageFormat::Jpeg,
            checksum,
            1920,
            1080,
            ImageBuffer::filled(2, 1, [1, 2, 3, 255]),
        );

        assert_eq!(asset.id(), AssetId::from_checksum(checksum));
        assert_eq!((asset.pixel_width(), asset.pixel_height()), (1920, 1080));
        assert_eq!(asset.format(), ImageFormat::Jpeg);
        assert_eq!(asset.encoded().as_ref(), bytes);
        assert_eq!(asset.preview().width(), 2);
    }

    #[test]
    fn a_caller_computed_checksum_is_not_recomputed() {
        let checksum = [7_u8; 32];
        let pixels = image();
        let asset = Asset::from_parts(
            Arc::from(&b"not hashed here"[..]),
            ImageFormat::Jpeg,
            checksum,
            &pixels,
            ImageBuffer::filled(1, 1, [0, 0, 0, 0]),
        );

        assert_eq!(asset.id(), AssetId::from_checksum(checksum));
        assert_eq!(asset.checksum(), &checksum);
    }
}
