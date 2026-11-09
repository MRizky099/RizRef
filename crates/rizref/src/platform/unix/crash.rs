//! Crash handling on Linux and macOS (see `platform::crash`).
//!
//! No minidump is written yet: a writer that is safe to run from a signal
//! handler is the `minidump-writer` item of the phase-08 plan. Installing
//! still prunes old dumps, so the folder obeys the same retention when dumps
//! arrive, and returns the path a dump would get. A fault leaves the session
//! marker behind, which is what makes the next launch offer recovery.

use std::path::{Path, PathBuf};

use crate::platform::crash::{dump_file_name, prune};

/// Prepares `dir` and returns the path this process's dump would use.
pub fn install(dir: &Path) -> PathBuf {
    let _ = std::fs::create_dir_all(dir);
    prune(dir);
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    dir.join(dump_file_name(unix, std::process::id()))
}

/// Ends the process the way a native fault would, for `rizref --crash-test`
/// (`SIGABRT`: the session marker stays, the next launch offers recovery).
pub fn raise_access_violation() -> ! {
    std::process::abort()
}
