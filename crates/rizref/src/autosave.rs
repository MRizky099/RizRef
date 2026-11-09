//! Autosave and crash recovery.
//!
//! RizRef never writes the user's board behind their back. Autosave goes to a
//! *recovery copy* under `%LOCALAPPDATA%\RizRef\autosave`, one per scene:
//!
//! - `<scene-id>.rizref` — an ordinary board written by `rizref-format`, so
//!   the second and later autosaves are cheap incremental appends (unchanged
//!   blobs are never rewritten) and an interrupted write can never corrupt it:
//!   the container recovers to the last complete index;
//! - `<scene-id>.json` — a sidecar naming the board the copy belongs to (its
//!   path, if it has one), its display name and when it was written.
//!
//! The copy is deleted when the board is saved, when its changes are
//! discarded, and on a clean exit with nothing unsaved. Anything left behind
//! at launch therefore belongs to a session that did not end cleanly, and is
//! offered back ([`pending_recoveries`]).
//!
//! The *session marker* (`session.lock`) is the other half of the "RizRef
//! recovered" flow: it is written at launch and removed on a clean exit, so a
//! marker found at launch means the previous session crashed or was killed,
//! even when it had nothing unsaved.
//!
//! The policy ([`AutosavePolicy`]) is pure and unit-tested: save after
//! [`AutosavePolicy::every_changes`] changes, or once the oldest unsaved
//! change is [`AutosavePolicy::interval`] old.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rizref_core::SceneId;
use serde::{Deserialize, Serialize};

/// When an autosave is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutosavePolicy {
    /// Save after this many changes, however quickly they came.
    pub every_changes: u32,
    /// Save once the oldest unsaved change is this old.
    pub interval: Duration,
}

impl Default for AutosavePolicy {
    /// Every 20 changes or 60 seconds — the plan's "every N commands / 60 s".
    fn default() -> Self {
        Self {
            every_changes: 20,
            interval: Duration::from_secs(60),
        }
    }
}

/// Changes since the last autosave, and when the oldest of them happened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutosaveTracker {
    changes: u32,
    oldest: Option<Instant>,
}

impl AutosaveTracker {
    /// Records one change to the board at `now`.
    pub fn note_change(&mut self, now: Instant) {
        self.changes = self.changes.saturating_add(1);
        self.oldest.get_or_insert(now);
    }

    /// Forgets the pending changes (after an autosave, a save or a discard).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    /// Changes since the last reset.
    pub const fn changes(&self) -> u32 {
        self.changes
    }

    /// Whether an autosave is due at `now`.
    pub fn is_due(&self, policy: AutosavePolicy, now: Instant) -> bool {
        match self.oldest {
            None => false,
            Some(oldest) => {
                self.changes >= policy.every_changes
                    || now.saturating_duration_since(oldest) >= policy.interval
            }
        }
    }

    /// When the interval rule fires, for the event loop's timer.
    pub fn deadline(&self, policy: AutosavePolicy) -> Option<Instant> {
        self.oldest.map(|oldest| oldest + policy.interval)
    }
}

/// The sidecar written next to a recovery copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryInfo {
    /// Display name of the board ("Untitled 1", "Moodboard.rizref").
    pub name: String,
    /// The board file the copy belongs to; `None` for a never-saved board.
    pub original: Option<PathBuf>,
    /// When the copy was last written, in Unix seconds.
    pub saved_unix: u64,
    /// Items on the board at that time.
    pub items: usize,
}

/// One recovery copy found at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// The recovery board file.
    pub board: PathBuf,
    /// Its sidecar.
    pub sidecar: PathBuf,
    /// What the sidecar says.
    pub info: RecoveryInfo,
}

/// `%LOCALAPPDATA%\RizRef\autosave`.
pub fn autosave_dir() -> PathBuf {
    crate::paths::local_dir().join("autosave")
}

/// `%LOCALAPPDATA%\RizRef\session.lock`.
pub fn session_marker() -> PathBuf {
    crate::paths::local_dir().join("session.lock")
}

/// The recovery board for `scene` inside `dir`.
pub fn board_path(dir: &Path, scene: SceneId) -> PathBuf {
    dir.join(format!("{}.rizref", scene.to_hex()))
}

/// The sidecar for `scene` inside `dir`.
pub fn sidecar_path(dir: &Path, scene: SceneId) -> PathBuf {
    dir.join(format!("{}.json", scene.to_hex()))
}

/// Writes the sidecar for `scene` atomically (temp file, then rename).
pub fn write_sidecar(dir: &Path, scene: SceneId, info: &RecoveryInfo) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = sidecar_path(dir, scene);
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(info).map_err(std::io::Error::other)?;
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &path)
}

/// Deletes the recovery copy of `scene` (board, sidecar, stray temp files).
/// Missing files are not an error.
pub fn discard(dir: &Path, scene: SceneId) {
    let board = board_path(dir, scene);
    let sidecar = sidecar_path(dir, scene);
    for path in [
        board.with_extension("rizref.tmp"),
        board,
        sidecar.with_extension("json.tmp"),
        sidecar,
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(path = %path.display(), %error, "cannot remove autosave"),
        }
    }
}

/// Every recovery copy in `dir` that has both a board and a readable sidecar,
/// newest first. A board without a sidecar (a crash between the two writes of
/// the very first autosave) is still offered, named after its file.
pub fn pending_recoveries(dir: &Path) -> Vec<Recovery> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Recovery> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rizref"))
        .filter_map(|board| {
            let stem = board.file_stem()?.to_str()?;
            SceneId::from_hex(stem)?;
            let sidecar = board.with_extension("json");
            let info = std::fs::read(&sidecar)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<RecoveryInfo>(&bytes).ok())
                .unwrap_or_else(|| RecoveryInfo {
                    name: "Recovered board".to_owned(),
                    original: None,
                    saved_unix: modified_unix(&board),
                    items: 0,
                });
            Some(Recovery {
                board,
                sidecar,
                info,
            })
        })
        .collect();
    found.sort_by_key(|r| std::cmp::Reverse(r.info.saved_unix));
    found
}

fn modified_unix(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// What the session marker said at launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousSession {
    /// No marker: the last session exited cleanly (or this is the first run).
    Clean,
    /// A marker was left behind: the last session crashed or was killed.
    Crashed,
}

/// Claims the session marker at `path`, reporting whether a previous session
/// left one behind. The marker holds this process id, for the logs.
pub fn claim_session(path: &Path) -> PreviousSession {
    let previous = if path.exists() {
        PreviousSession::Crashed
    } else {
        PreviousSession::Clean
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(path, std::process::id().to_string()) {
        tracing::warn!(path = %path.display(), %error, "cannot write the session marker");
    }
    previous
}

/// Removes the session marker on a clean exit.
pub fn release_session(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// The recovery offer's two body lines for `recovery`, given the current
/// time: what was saved and when, then whose work it is.
pub fn prompt_text(recovery: &Recovery, now_unix: u64) -> (String, String) {
    let age = now_unix.saturating_sub(recovery.info.saved_unix);
    let when = match age {
        0..=59 => "less than a minute ago".to_owned(),
        60..=3599 => format!("{} min ago", age / 60),
        3600..=86_399 => format!("{} h ago", age / 3600),
        _ => format!("{} days ago", age / 86_400),
    };
    // The note stays one short line (the dialog does not wrap): the board's
    // file name, not its full path, which the restored title shows anyway.
    let target = recovery.info.original.as_ref().map_or_else(
        || "a board that was never saved".to_owned(),
        |p| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        },
    );
    (
        format!(
            "\"{}\" ({} items, autosaved {when}) was not saved.",
            recovery.info.name, recovery.info.items
        ),
        format!("RizRef did not close properly. It belongs to {target}."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(byte: u8) -> SceneId {
        SceneId::from_bytes([byte; SceneId::BYTE_LEN])
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rizref-autosave-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn nothing_is_due_without_a_change() {
        let tracker = AutosaveTracker::default();
        let now = Instant::now();

        assert!(!tracker.is_due(AutosavePolicy::default(), now + Duration::from_secs(3600)));
        assert_eq!(tracker.deadline(AutosavePolicy::default()), None);
    }

    #[test]
    fn the_change_count_triggers_a_save() {
        let policy = AutosavePolicy {
            every_changes: 3,
            interval: Duration::from_secs(60),
        };
        let mut tracker = AutosaveTracker::default();
        let now = Instant::now();

        tracker.note_change(now);
        tracker.note_change(now);
        assert!(!tracker.is_due(policy, now));
        tracker.note_change(now);
        assert!(tracker.is_due(policy, now));

        tracker.reset();
        assert_eq!(tracker.changes(), 0);
        assert!(!tracker.is_due(policy, now));
    }

    #[test]
    fn the_oldest_change_sets_the_interval_deadline() {
        let policy = AutosavePolicy::default();
        let mut tracker = AutosaveTracker::default();
        let start = Instant::now();

        tracker.note_change(start);
        tracker.note_change(start + Duration::from_secs(30));

        assert_eq!(
            tracker.deadline(policy),
            Some(start + Duration::from_secs(60))
        );
        assert!(!tracker.is_due(policy, start + Duration::from_secs(59)));
        assert!(tracker.is_due(policy, start + Duration::from_secs(60)));
    }

    #[test]
    fn recoveries_are_listed_newest_first_and_discarded_cleanly() {
        let dir = temp_dir("list");
        for (byte, saved, name) in [(1_u8, 100_u64, "old"), (2, 300, "new")] {
            std::fs::write(board_path(&dir, scene(byte)), b"board").expect("board");
            write_sidecar(
                &dir,
                scene(byte),
                &RecoveryInfo {
                    name: name.to_owned(),
                    original: Some(PathBuf::from(format!("C:\\boards\\{name}.rizref"))),
                    saved_unix: saved,
                    items: 3,
                },
            )
            .expect("sidecar");
        }
        // Neither a stray file nor a board with a non-id name is a recovery.
        std::fs::write(dir.join("notes.txt"), b"x").expect("stray");
        std::fs::write(dir.join("not-an-id.rizref"), b"x").expect("stray board");

        let found = pending_recoveries(&dir);
        let names: Vec<&str> = found.iter().map(|r| r.info.name.as_str()).collect();
        assert_eq!(names, ["new", "old"]);

        discard(&dir, scene(2));
        let names: Vec<String> = pending_recoveries(&dir)
            .into_iter()
            .map(|r| r.info.name)
            .collect();
        assert_eq!(names, ["old"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_board_without_its_sidecar_is_still_offered() {
        let dir = temp_dir("orphan");
        std::fs::write(board_path(&dir, scene(9)), b"board").expect("board");

        let found = pending_recoveries(&dir);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].info.name, "Recovered board");
        assert_eq!(found[0].info.original, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_session_marker_detects_an_unclean_exit() {
        let dir = temp_dir("marker");
        let marker = dir.join("session.lock");

        assert_eq!(claim_session(&marker), PreviousSession::Clean);
        // The process "crashes": the marker is never released.
        assert_eq!(claim_session(&marker), PreviousSession::Crashed);
        release_session(&marker);
        assert_eq!(claim_session(&marker), PreviousSession::Clean);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Grows a board forever, appending to its recovery copy: the child half
    /// of the kill test below.
    fn autosave_forever(dir: &Path) -> ! {
        use rizref_codecs::{ImageBuffer, ImageFormat};
        use rizref_core::{Asset, Camera, Scene};
        use std::sync::Arc;

        let id = scene(0x5a);
        let path = board_path(dir, id);
        let mut scene = Scene::new(id);
        let mut round = 0_u32;
        loop {
            round += 1;
            // A fresh 1 MiB "original" each round, so every append writes a
            // blob chunk — the long write a kill is most likely to land in.
            let bytes: Vec<u8> = (0..1_048_576_u32)
                .map(|i| (i.wrapping_mul(2_654_435_761).wrapping_add(round) >> 13) as u8)
                .collect();
            let pixels = ImageBuffer::filled(8, 8, [round as u8, 40, 90, 255]);
            let asset = Asset::from_decoded(
                Arc::from(bytes),
                ImageFormat::Png,
                &pixels,
                ImageBuffer::filled(4, 4, [round as u8, 40, 90, 255]),
            );
            let (asset_id, _) = scene.insert_asset(asset);
            let offset = round as f32 * 12.0;
            scene.insert_image(asset_id, glam::Vec2::splat(offset), glam::Vec2::splat(64.0));
            scene.insert_note(
                format!("round {round}"),
                glam::Vec2::new(offset, -80.0),
                glam::Vec2::splat(60.0),
            );
            let request = rizref_format::SaveRequest::new(&scene, Camera::default(), "soak");
            rizref_format::save(&path, &request).expect("autosave in the child");
            write_sidecar(
                dir,
                id,
                &RecoveryInfo {
                    name: "soak".to_owned(),
                    original: None,
                    saved_unix: round.into(),
                    items: scene.items().len(),
                },
            )
            .expect("sidecar in the child");
        }
    }

    /// The phase-07 acceptance check, as a test: kill the process at an
    /// arbitrary moment during autosave (`TerminateProcess`, the Windows
    /// `kill -9`) and the recovery copy left behind must still load — to the
    /// last complete autosave, never to a corrupt board.
    #[test]
    fn a_killed_autosave_always_leaves_a_loadable_copy() {
        const CHILD: &str = "RIZREF_AUTOSAVE_SOAK_DIR";
        if let Some(dir) = std::env::var_os(CHILD) {
            autosave_forever(Path::new(&dir));
        }

        let exe = std::env::current_exe().expect("test binary");
        let mut loaded_rounds = Vec::new();
        for (attempt, delay_ms) in [180_u64, 260, 330, 410, 520, 640].into_iter().enumerate() {
            let dir = temp_dir(&format!("kill{attempt}"));
            let mut child = std::process::Command::new(&exe)
                .args([
                    "autosave::tests::a_killed_autosave_always_leaves_a_loadable_copy",
                    "--exact",
                    "--test-threads=1",
                ])
                .env(CHILD, &dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn the autosave child");
            std::thread::sleep(Duration::from_millis(delay_ms));
            child.kill().expect("kill the child");
            let _ = child.wait();

            let path = board_path(&dir, scene(0x5a));
            if path.exists() {
                let loaded = rizref_format::load(&path)
                    .unwrap_or_else(|error| panic!("attempt {attempt}: corrupt copy: {error}"));
                let images = loaded.scene.items().iter().filter(|i| !i.is_note()).count();
                assert_eq!(
                    images * 2,
                    loaded.scene.items().len(),
                    "attempt {attempt}: every complete round has one image and one note"
                );
                loaded_rounds.push(images);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        assert!(
            loaded_rounds.iter().any(|&rounds| rounds > 0),
            "at least one attempt must have autosaved before the kill: {loaded_rounds:?}"
        );
    }

    #[test]
    fn the_prompt_names_the_board_and_its_age() {
        let recovery = Recovery {
            board: PathBuf::from("x.rizref"),
            sidecar: PathBuf::from("x.json"),
            info: RecoveryInfo {
                name: "Moodboard".to_owned(),
                original: None,
                saved_unix: 1_000,
                items: 12,
            },
        };

        let (message, note) = prompt_text(&recovery, 1_000 + 125);

        assert!(
            message.contains("\"Moodboard\" (12 items, autosaved 2 min ago)"),
            "{message}"
        );
        assert!(note.contains("never saved"), "{note}");

        // A saved board is named by its file name only, so the line fits.
        let saved = Recovery {
            info: RecoveryInfo {
                original: Some(PathBuf::from("F:/work/deep/folder/board1.rizref")),
                ..recovery.info.clone()
            },
            ..recovery
        };
        let (_, note) = prompt_text(&saved, 1_000);
        assert!(note.ends_with("It belongs to board1.rizref."), "{note}");
    }
}
