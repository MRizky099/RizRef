//! The import pipeline: file bytes in, decoded pixels and a preview out.
//!
//! Decoding runs on a small pool of standard threads, never on the UI thread.
//! The pool is coordinated by `crossbeam-channel`; there is no async runtime and
//! no dependency on winit, so the pipeline is testable on its own. The event
//! loop receives each outcome through the closure the application supplies —
//! production wires it to `EventLoopProxy::send_event`, tests to a channel.
//!
//! The original encoded bytes are carried through untouched. They are the
//! asset's identity (BLAKE3), its provenance, and later the BLOB a `.rizref`
//! file stores, so re-encoding them anywhere on this path would be a bug.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, unbounded};
use glam::Vec2;
use rizref_codecs::{CodecError, DecodeLimits, ImageBuffer, ImageFormat, decode_as_with_limits};
use rizref_core::{Animation, AnimationFrame, Fitted, checksum, fit_to_side, generate_preview};

use crate::platform::heif;
use crate::textures::MAX_TEXTURE_SIDE;

/// One image to import: a path to read, or bytes already in hand.
#[derive(Debug, Clone)]
pub struct ImportSource {
    /// Human-readable origin, used in logs and error messages.
    pub label: String,
    /// A file to read on the worker.
    pub path: Option<PathBuf>,
    /// Bytes from the clipboard or another in-memory source.
    pub bytes: Option<Arc<[u8]>>,
}

impl ImportSource {
    /// An image file on disk.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        Self {
            label,
            path: Some(path),
            bytes: None,
        }
    }

    /// Bytes that did not come from a file, such as a clipboard image.
    pub fn bytes(label: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            label: label.into(),
            path: None,
            bytes: Some(bytes.into()),
        }
    }
}

/// A request to import one image around a world-space point.
#[derive(Debug, Clone)]
pub struct ImportRequest {
    /// What to import.
    pub source: ImportSource,
    /// Where on the board the item should land.
    pub drop_world: Vec2,
    /// When the request was submitted, for the end-to-end latency trace.
    pub queued_at: Instant,
}

impl ImportRequest {
    /// A request for a source, landing around `drop_world`.
    pub fn new(source: ImportSource, drop_world: Vec2) -> Self {
        Self {
            source,
            drop_world,
            queued_at: Instant::now(),
        }
    }

    /// A request for a file path.
    pub fn path(path: impl Into<PathBuf>, drop_world: Vec2) -> Self {
        Self::new(ImportSource::path(path), drop_world)
    }

    /// A request for in-memory bytes.
    pub fn bytes(label: impl Into<String>, bytes: impl Into<Arc<[u8]>>, drop_world: Vec2) -> Self {
        Self::new(ImportSource::bytes(label, bytes), drop_world)
    }
}

/// A successfully decoded image, ready to become an [`Asset`](rizref_core::Asset).
#[derive(Debug)]
pub struct ImportedImage {
    /// The format the signature (or extension, for TGA) named.
    pub format: ImageFormat,
    /// Original encoded bytes, verbatim.
    pub encoded: Arc<[u8]>,
    /// Canonical 8-bit RGBA pixels.
    pub image: ImageBuffer,
    /// A display copy already downscaled to the GPU side cap, when the
    /// original is larger. Generated off the UI thread so a 100 MP file does
    /// not stutter the upload.
    pub display: Option<ImageBuffer>,
    /// Box-filtered thumbnail, at most 512 px on its longest side.
    pub preview: ImageBuffer,
    /// Frames for an animated GIF, composed and timed.
    pub animation: Option<Animation>,
    /// BLAKE3 of `encoded`, computed on the worker.
    pub checksum: [u8; 32],
}

/// Why an import produced no image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    /// The source label, repeated so a log line stands alone.
    pub label: String,
    /// The decoder or file-system message.
    pub reason: String,
}

/// The pool's answer for one request.
#[derive(Debug)]
pub struct ImportOutcome {
    /// The label of the source that was imported.
    pub label: String,
    /// Where the item should land.
    pub drop_world: Vec2,
    /// Submit-to-outcome time, including queueing.
    pub latency: Duration,
    /// The decoded image, or the reason it failed.
    pub result: Result<ImportedImage, ImportError>,
}

/// A fixed-size pool of decode threads.
pub struct Importer {
    sender: Option<Sender<ImportRequest>>,
    workers: Vec<JoinHandle<()>>,
}

impl Importer {
    /// Starts a pool sized `max(1, available_parallelism - 1)`.
    pub fn new(notify: impl Fn(ImportOutcome) + Send + Sync + 'static) -> Self {
        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        Self::with_workers(available.saturating_sub(1).max(1), notify)
    }

    /// Starts a pool with an explicit worker count, for tests and diagnostics.
    pub fn with_workers(
        count: usize,
        notify: impl Fn(ImportOutcome) + Send + Sync + 'static,
    ) -> Self {
        let (sender, receiver) = unbounded::<ImportRequest>();
        let notify: Arc<dyn Fn(ImportOutcome) + Send + Sync> = Arc::new(notify);
        let mut workers = Vec::with_capacity(count.max(1));

        for index in 0..count.max(1) {
            let receiver = receiver.clone();
            let notify = Arc::clone(&notify);
            match std::thread::Builder::new()
                .name(format!("rizref-import-{index}"))
                .spawn(move || {
                    while let Ok(request) = receiver.recv() {
                        let outcome = process(request);
                        notify(outcome);
                    }
                }) {
                Ok(handle) => workers.push(handle),
                Err(error) => {
                    tracing::error!(%error, index, "cannot start an import worker");
                }
            }
        }

        Self {
            sender: Some(sender),
            workers,
        }
    }

    /// Queues a request. Returns `false` when the pool has already shut down.
    pub fn submit(&self, request: ImportRequest) -> bool {
        match self.sender.as_ref() {
            Some(sender) => sender.send(request).is_ok(),
            None => false,
        }
    }

    /// Number of workers that started successfully.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}

impl Drop for Importer {
    fn drop(&mut self) {
        // Closing the channel is what ends each worker loop.
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Reads, decodes, hashes and previews one request.
fn process(request: ImportRequest) -> ImportOutcome {
    let ImportRequest {
        source,
        drop_world,
        queued_at,
    } = request;
    let label = source.label.clone();

    let result =
        std::panic::catch_unwind(AssertUnwindSafe(|| import(&source))).unwrap_or_else(|payload| {
            Err(ImportError {
                label: label.clone(),
                reason: format!("decoder panicked: {}", panic_message(payload.as_ref())),
            })
        });

    let latency = queued_at.elapsed();
    match &result {
        Ok(image) => tracing::debug!(
            label = %label,
            format = ?image.format,
            width = image.image.width(),
            height = image.image.height(),
            latency_ms = latency.as_secs_f64() * 1000.0,
            "import finished"
        ),
        Err(error) => tracing::warn!(
            label = %label,
            reason = %error.reason,
            latency_ms = latency.as_secs_f64() * 1000.0,
            "import failed"
        ),
    }

    ImportOutcome {
        label,
        drop_world,
        latency,
        result,
    }
}

fn import(source: &ImportSource) -> Result<ImportedImage, ImportError> {
    let bytes = match (&source.bytes, &source.path) {
        (Some(bytes), _) => Arc::clone(bytes),
        (None, Some(path)) => std::fs::read(path)
            .map(Arc::from)
            .map_err(|error| ImportError {
                label: source.label.clone(),
                reason: format!("cannot read {}: {error}", path.display()),
            })?,
        (None, None) => {
            return Err(ImportError {
                label: source.label.clone(),
                reason: "the import source has neither bytes nor a path".to_owned(),
            });
        }
    };

    let format = format_for(&bytes, source.path.as_deref()).ok_or_else(|| ImportError {
        label: source.label.clone(),
        reason: CodecError::Unrecognized.to_string(),
    })?;

    let _decode_span = tracing::info_span!(
        "import.decode",
        label = %source.label,
        format = ?format,
        bytes = bytes.len()
    )
    .entered();
    let limits = DecodeLimits::default();
    // HEIF is the one format this build cannot decode in Rust; the OS codec
    // does it instead (see `platform::heif`).
    let (image, animation) = if format == ImageFormat::Heif {
        let image = heif::decode(&bytes, limits).map_err(|error| ImportError {
            label: source.label.clone(),
            reason: error.to_string(),
        })?;
        (image, None)
    } else {
        let image = decode_as_with_limits(&bytes, format, limits).map_err(|error| ImportError {
            label: source.label.clone(),
            reason: error.to_string(),
        })?;
        let animation = if format == ImageFormat::Gif {
            animation_of(&bytes, limits, &source.label)
        } else {
            None
        };
        (image, animation)
    };

    let checksum = checksum(&bytes);

    // The preview span is separate so a slow box filter is distinguishable
    // from a slow codec in the log. The display copy is made here too: it is
    // the texture the board samples, and downscaling it on the UI thread would
    // stall the first frame after a large drop.
    let (display, preview) = {
        let _preview_span = tracing::info_span!(
            "import.preview",
            label = %source.label,
            width = image.width(),
            height = image.height()
        )
        .entered();
        let display = match fit_to_side(&image, MAX_TEXTURE_SIDE) {
            Fitted::Original(_) => None,
            Fitted::Resized(resized) => Some(resized),
        };
        (display, generate_preview(&image))
    };

    Ok(ImportedImage {
        format,
        encoded: bytes,
        image,
        display,
        preview,
        animation,
        checksum,
    })
}

/// Decodes a GIF's frames, or `None` when it is a still image.
///
/// A one-frame GIF is deliberately *not* wrapped in an animation: there is
/// nothing to play, and treating it as animated would put a pointless clock on
/// the frame budget.
fn animation_of(bytes: &[u8], limits: DecodeLimits, label: &str) -> Option<Animation> {
    match rizref_codecs::decode_gif_animation(bytes, limits) {
        Ok(frames) if frames.len() > 1 => {
            tracing::debug!(label, frames = frames.len(), "animated gif decoded");
            let frames = frames
                .into_iter()
                .map(|frame| AnimationFrame::new(frame.image, frame.delay))
                .collect();
            Animation::new(frames)
        }
        Ok(_) => None,
        Err(error) => {
            // The still frame is already in hand; a failed frame walk only
            // costs the animation, not the import.
            tracing::warn!(label, %error, "cannot read the gif's frames; showing the first");
            None
        }
    }
}

/// Sniffs the format, falling back to the extension for TGA (no signature).
fn format_for(bytes: &[u8], path: Option<&Path>) -> Option<ImageFormat> {
    rizref_codecs::sniff(bytes).or_else(|| {
        path.and_then(Path::extension)
            .and_then(|extension| extension.to_str())
            .and_then(ImageFormat::from_extension)
    })
}

fn panic_message(payload: &dyn std::any::Any) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{ImportRequest, Importer};
    use glam::Vec2;
    use rizref_codecs::{ImageBuffer, ImageFormat};
    use std::sync::mpsc;
    use std::time::Duration;

    fn png() -> Vec<u8> {
        let image = ImageBuffer::filled(8, 4, [20, 40, 60, 255]);
        rizref_codecs::encode_png(&image).expect("encode test png")
    }

    #[test]
    fn a_worker_decodes_bytes_and_generates_a_preview() {
        let (notify, outcomes) = mpsc::channel();
        let importer = Importer::with_workers(1, move |outcome| {
            notify.send(outcome).expect("test receiver lives");
        });

        assert!(importer.submit(ImportRequest::bytes(
            "clipboard.png",
            png(),
            Vec2::new(3.0, 4.0)
        )));

        let outcome = outcomes
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker answers");
        let image = outcome.result.expect("valid png");
        assert_eq!(image.format, ImageFormat::Png);
        assert_eq!((image.image.width(), image.image.height()), (8, 4));
        assert_eq!((image.preview.width(), image.preview.height()), (8, 4));
        assert!(image.display.is_none(), "small images need no display copy");
        assert_eq!(image.checksum, rizref_core::checksum(&image.encoded));
        assert_eq!(outcome.drop_world, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn a_missing_file_is_reported_through_the_outcome_not_a_panic() {
        let (notify, outcomes) = mpsc::channel();
        let importer = Importer::with_workers(1, move |outcome| {
            notify.send(outcome).expect("test receiver lives");
        });

        importer.submit(ImportRequest::path(
            "this-file-does-not-exist.png",
            Vec2::ZERO,
        ));

        let outcome = outcomes
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker answers");
        let error = outcome.result.expect_err("no such file");
        assert!(error.reason.contains("cannot read"));
    }

    #[test]
    fn unrecognised_bytes_fail_cleanly() {
        let (notify, outcomes) = mpsc::channel();
        let importer = Importer::with_workers(1, move |outcome| {
            notify.send(outcome).expect("test receiver lives");
        });

        importer.submit(ImportRequest::bytes(
            "notes.txt",
            b"not an image".to_vec(),
            Vec2::ZERO,
        ));

        let outcome = outcomes
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker answers");
        assert!(outcome.result.is_err());
    }

    #[test]
    fn the_pool_has_the_requested_workers() {
        let importer = Importer::with_workers(2, |_| {});
        assert_eq!(importer.worker_count(), 2);
    }
}
