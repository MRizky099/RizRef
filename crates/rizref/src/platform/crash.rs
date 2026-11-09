//! Crash reports: where they go, what they are called, how many are kept.
//!
//! A Rust panic is reported by the panic hook in `app` on every OS. What the
//! hook cannot see is a fault below Rust — an access violation inside a GPU
//! driver, a stack overflow in a decoder:
//!
//! * **Windows** writes a minidump from the unhandled-exception filter
//!   (`platform/windows/crash.rs`, dbghelp — part of every install);
//! * **Linux and macOS** do not write a dump yet (`platform/unix/crash.rs`):
//!   a signal-safe writer is the `minidump-writer` work the phase-08 plan
//!   lists. The session marker left behind by a fault still makes the next
//!   launch say "RizRef recovered" and offer the autosave.
//!
//! Dumps go to the `crashes` folder of the per-OS state directory
//! ([`crashes_dir`]), the newest [`DUMPS_KEPT`] are kept.

use std::path::{Path, PathBuf};

#[cfg(windows)]
pub use super::windows::crash::{install, raise_access_violation};

#[cfg(unix)]
pub use super::unix::crash::{install, raise_access_violation};

/// How many dumps to keep; older ones are pruned at install time.
pub const DUMPS_KEPT: usize = 10;

/// The `crashes` folder of the state directory (`%LOCALAPPDATA%\RizRef\crashes`
/// on Windows).
pub fn crashes_dir() -> PathBuf {
    crate::paths::local_dir().join("crashes")
}

/// The dump file name for one process: unique per launch, sortable by time.
pub fn dump_file_name(unix_seconds: u64, pid: u32) -> String {
    format!("rizref-{unix_seconds:011}-{pid}.dmp")
}

/// Which dump files to delete so `keep` newest remain. Pure; names sort
/// chronologically thanks to the zero-padded timestamp.
pub fn stale_dumps(names: &[String], keep: usize) -> Vec<String> {
    let mut dumps: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("rizref-") && n.ends_with(".dmp"))
        .collect();
    dumps.sort_unstable();
    dumps.reverse();
    dumps.into_iter().skip(keep).cloned().collect()
}

/// Deletes all but the newest [`DUMPS_KEPT`] dumps in `dir`.
pub(crate) fn prune(dir: &Path) {
    let names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    for name in stale_dumps(&names, DUMPS_KEPT) {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::{dump_file_name, stale_dumps};

    #[test]
    fn dump_names_sort_by_time() {
        let early = dump_file_name(9, 4);
        let late = dump_file_name(1_700_000_000, 4);

        assert_eq!(early, "rizref-00000000009-4.dmp");
        assert!(early < late);
    }

    #[test]
    fn only_the_newest_dumps_survive() {
        let names: Vec<String> = [5_u64, 1, 3, 2, 4]
            .iter()
            .map(|&t| dump_file_name(t, 1))
            .chain(["panic-1.log".to_owned()])
            .collect();

        let stale = stale_dumps(&names, 3);

        assert_eq!(stale, vec![dump_file_name(2, 1), dump_file_name(1, 1)]);
    }
}
