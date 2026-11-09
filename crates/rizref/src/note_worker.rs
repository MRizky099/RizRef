//! Note rendering off the UI thread, and the note-texture budget.
//!
//! Laying out a long markdown note and rasterising its equations costs tens
//! to hundreds of milliseconds cold (phase 06a measured 116 ms for 4,000
//! characters, 21 ms for 20 equations): on the UI thread that is a dropped
//! frame every time a note is created, opened or reflowed. Phase 07 moves
//! every render except the live editor's onto one worker thread with its own
//! font system and [`NoteRenderer`] (layouts, equation masks and text layers
//! are cached there). The UI thread shows the note's last texture — or a
//! card-only placeholder — until the result arrives as an event.
//!
//! Requests are coalesced per note: while the worker is busy, newer requests
//! for the same note replace older ones, so dragging a reflow slider or
//! typing a background colour never builds a queue.
//!
//! [`plan_evictions`] is the 64 MiB budget for full-resolution note
//! textures: off-screen notes, least recently seen first, drop to a small
//! low-resolution copy until they are on screen again.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glam::Vec2;
use rizref_codecs::ImageBuffer;
use rizref_core::{AssetId, ItemId, NoteContent};

use crate::note::{NoteRenderer, low_resolution};
use crate::text::TextEngine;

/// Bytes of full-resolution note textures kept on the GPU.
pub const NOTE_TEXTURE_BUDGET: usize = 64 * 1024 * 1024;

/// One note to render.
#[derive(Debug, Clone)]
pub struct NoteJob {
    /// The note.
    pub note: ItemId,
    /// The texture id this render will be uploaded as.
    pub texture: AssetId,
    /// What to render.
    pub content: NoteContent,
    /// The note's world size.
    pub size: Vec2,
}

/// A finished render.
#[derive(Debug)]
pub struct NoteRendered {
    /// The note.
    pub note: ItemId,
    /// The texture id the job named.
    pub texture: AssetId,
    /// The full texture; `None` when rendering failed (a contained panic).
    pub image: Option<ImageBuffer>,
    /// The same picture at most 256 px, kept for eviction.
    pub low: Option<ImageBuffer>,
    /// Time spent on the worker.
    pub elapsed: Duration,
}

/// The note render thread.
pub struct NoteWorker {
    jobs: Option<Sender<NoteJob>>,
    thread: Option<JoinHandle<()>>,
}

impl NoteWorker {
    /// Starts the worker; `done` runs on the worker thread for each render
    /// (the application forwards it to the event loop).
    pub fn spawn(done: impl Fn(NoteRendered) + Send + 'static) -> Self {
        let (jobs, inbox) = channel::<NoteJob>();
        let thread = std::thread::Builder::new()
            .name("rizref-notes".to_owned())
            .spawn(move || run(&inbox, &done))
            .ok();
        Self {
            jobs: thread.is_some().then_some(jobs),
            thread,
        }
    }

    /// Queues a render. Returns `false` when the worker is gone.
    pub fn submit(&self, job: NoteJob) -> bool {
        self.jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok())
    }
}

impl Drop for NoteWorker {
    fn drop(&mut self) {
        // Closing the channel ends the loop after the render in hand.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(inbox: &Receiver<NoteJob>, done: &dyn Fn(NoteRendered)) {
    // Created on first use: a session without notes never loads a second
    // copy of the font database.
    let mut engine: Option<TextEngine> = None;
    let mut renderer = NoteRenderer::new();
    while let Ok(first) = inbox.recv() {
        for job in coalesce(first, inbox) {
            let engine = engine.get_or_insert_with(TextEngine::new);
            done(render(engine, &mut renderer, job));
        }
    }
}

/// `first` plus everything already queued, keeping only the newest job per
/// note, in arrival order of those newest jobs.
fn coalesce(first: NoteJob, inbox: &Receiver<NoteJob>) -> Vec<NoteJob> {
    let mut jobs = vec![first];
    jobs.extend(inbox.try_iter());
    let mut newest: HashMap<ItemId, usize> = HashMap::new();
    for (index, job) in jobs.iter().enumerate() {
        newest.insert(job.note, index);
    }
    jobs.into_iter()
        .enumerate()
        .filter(|(index, job)| newest.get(&job.note) == Some(index))
        .map(|(_, job)| job)
        .collect()
}

fn render(engine: &mut TextEngine, renderer: &mut NoteRenderer, job: NoteJob) -> NoteRendered {
    let started = Instant::now();
    let NoteJob {
        note,
        texture,
        content,
        size,
    } = job;
    let image = crate::contain::contain(|| renderer.render(engine, &content, size, None))
        .unwrap_or_else(|message| {
            tracing::warn!(%message, "note render panicked");
            None
        });
    let low = image.as_ref().map(low_resolution);
    NoteRendered {
        note,
        texture,
        image,
        low,
        elapsed: started.elapsed(),
    }
}

/// One resident note texture, as the budget sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resident {
    /// The note.
    pub note: ItemId,
    /// Bytes of its full-resolution texture; 0 for a placeholder or
    /// low-resolution copy.
    pub bytes: usize,
    /// Frame number it was last on screen.
    pub last_seen: u64,
    /// Whether it is on screen now, or being edited: never evicted.
    pub pinned: bool,
}

/// The notes whose full textures to drop to their low-resolution copies so
/// the full ones fit `budget`: unpinned notes, least recently seen first,
/// until the rest fit. Pinned notes are never chosen, even over budget.
pub fn plan_evictions(residents: &[Resident], budget: usize) -> Vec<ItemId> {
    let mut total: usize = residents.iter().map(|resident| resident.bytes).sum();
    let mut candidates: Vec<&Resident> = residents
        .iter()
        .filter(|resident| !resident.pinned && resident.bytes > 0)
        .collect();
    candidates.sort_by_key(|resident| (resident.last_seen, resident.note));
    let mut evicted = Vec::new();
    for resident in candidates {
        if total <= budget {
            break;
        }
        total -= resident.bytes;
        evicted.push(resident.note);
    }
    evicted
}

#[cfg(test)]
mod tests {
    use super::{NoteJob, NoteWorker, Resident, coalesce, plan_evictions};
    use glam::Vec2;
    use rizref_core::{AssetId, ItemId, NoteContent};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn job(note: u64, text: &str) -> NoteJob {
        NoteJob {
            note: ItemId::from_raw(note),
            texture: AssetId::from_checksum([note as u8; 32]),
            content: NoteContent::markdown(text),
            size: Vec2::new(200.0, 120.0),
        }
    }

    #[test]
    fn the_worker_renders_off_thread_with_a_low_resolution_copy() {
        let (sender, results) = channel();
        let worker = NoteWorker::spawn(move |done| {
            let _ = sender.send(done);
        });
        assert!(worker.submit(job(1, "# Hello\n\nSome $x^2$ maths.")));

        let done = results
            .recv_timeout(Duration::from_secs(60))
            .expect("the worker answers");
        let image = done.image.expect("rendered");
        assert_eq!((image.width(), image.height()), (400, 240));
        let low = done.low.expect("low-resolution copy");
        assert!(low.width().max(low.height()) <= 256);
        assert_eq!(done.note, ItemId::from_raw(1));
    }

    #[test]
    fn queued_jobs_keep_only_the_newest_per_note() {
        let (sender, inbox) = channel();
        sender.send(job(2, "b1")).expect("open");
        sender.send(job(1, "a2")).expect("open");
        sender.send(job(2, "b2")).expect("open");

        let jobs = coalesce(job(1, "a1"), &inbox);

        let texts: Vec<&str> = jobs.iter().map(|job| job.content.text.as_str()).collect();
        assert_eq!(texts, ["a2", "b2"]);
    }

    fn resident(note: u64, mib: usize, last_seen: u64, pinned: bool) -> Resident {
        Resident {
            note: ItemId::from_raw(note),
            bytes: mib * 1024 * 1024,
            last_seen,
            pinned,
        }
    }

    #[test]
    fn eviction_keeps_the_budget_dropping_the_least_recently_seen_first() {
        let residents = [
            resident(1, 30, 10, false),
            resident(2, 30, 5, false),
            resident(3, 30, 20, true),
            resident(4, 10, 1, false),
        ];

        let evicted = plan_evictions(&residents, 64 * 1024 * 1024);

        // 100 MiB resident: drop note 4 (seen at 1), then note 2 (at 5).
        assert_eq!(evicted, [ItemId::from_raw(4), ItemId::from_raw(2)]);
    }

    #[test]
    fn visible_notes_are_never_evicted_even_over_budget() {
        let residents = [resident(1, 50, 1, true), resident(2, 50, 2, true)];
        assert!(plan_evictions(&residents, 64 * 1024 * 1024).is_empty());
        assert!(plan_evictions(&[resident(1, 10, 0, false)], 64 * 1024 * 1024).is_empty());
    }
}
