//! Crash minidumps: Crashpad parity without Crashpad.
//!
//! A Rust panic is reported by the panic hook in `app`. What the hook cannot
//! see is a *structured exception*: an access violation inside a GPU driver, a
//! stack overflow in a decoder, a fault in a COM callee. For those Windows
//! calls the process's unhandled-exception filter, and this module installs
//! one that writes a minidump with `MiniDumpWriteDump` (dbghelp, part of every
//! Windows install) to `%LOCALAPPDATA%\RizRef\crashes`, then lets the process
//! die. The session marker left behind is what makes the next launch say
//! "RizRef recovered" and offer the autosave.
//!
//! The plan named the `minidump-writer` crate behind a vectored handler. An
//! unhandled-exception filter is the better hook — a vectored handler also
//! sees first-chance exceptions that a driver or the OS handles itself — and
//! dbghelp is an OS library reached through the `windows` declarations we
//! already depend on, so no dependency is added.
//!
//! The filter runs on a damaged process, so it allocates nothing: the dump
//! path is built at install time.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithIndirectlyReferencedMemory,
    MiniDumpWithThreadInfo, MiniDumpWriteDump, SetUnhandledExceptionFilter,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::core::PCWSTR;

/// How many dumps to keep; older ones are pruned at install time.
pub const DUMPS_KEPT: usize = 10;

/// `EXCEPTION_EXECUTE_HANDLER`: the process ends after the filter returns.
const EXECUTE_HANDLER: i32 = 1;

/// The NUL-terminated dump path, fixed at install time.
static DUMP_PATH: OnceLock<Vec<u16>> = OnceLock::new();

/// `%LOCALAPPDATA%\RizRef\crashes`.
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

/// Installs the unhandled-exception filter, writing dumps into `dir`.
/// Returns the path a dump would be written to. Calling it twice keeps the
/// first path.
// SAFETY: `SetUnhandledExceptionFilter` only stores the function pointer; the
// filter itself is `unsafe extern "system"` with the documented signature.
#[allow(unsafe_code)]
pub fn install(dir: &Path) -> PathBuf {
    let _ = std::fs::create_dir_all(dir);
    prune(dir);
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(dump_file_name(unix, std::process::id()));
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let _ = DUMP_PATH.set(wide);
    unsafe {
        SetUnhandledExceptionFilter(Some(write_dump));
    }
    path
}

fn prune(dir: &Path) {
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

/// The filter: write the dump, then let Windows end the process.
// SAFETY: called by the OS with a valid `EXCEPTION_POINTERS` for the faulting
// thread. The path buffer is a NUL-terminated `'static` allocation made at
// install time; the file handle is closed before returning.
#[allow(unsafe_code)]
unsafe extern "system" fn write_dump(info: *const EXCEPTION_POINTERS) -> i32 {
    let Some(path) = DUMP_PATH.get() else {
        return EXECUTE_HANDLER;
    };
    unsafe {
        let Ok(file) = CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) else {
            return EXECUTE_HANDLER;
        };
        let exception = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: GetCurrentThreadId(),
            ExceptionPointers: info.cast_mut(),
            ClientPointers: false.into(),
        };
        let _ = MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file,
            MiniDumpWithThreadInfo | MiniDumpWithIndirectlyReferencedMemory,
            Some(&exception),
            None,
            None,
        );
        let _ = CloseHandle(file);
    }
    EXECUTE_HANDLER
}

/// Raises an access violation, for `rizref --crash-test` (the by-hand and
/// integration check that a dump is written).
// SAFETY: deliberately undefined at the language level and well defined at
// the OS level: a volatile write to address 0 faults with
// EXCEPTION_ACCESS_VIOLATION, which is the point.
#[allow(unsafe_code)]
pub fn raise_access_violation() -> ! {
    unsafe {
        std::ptr::null_mut::<u8>().write_volatile(1);
    }
    std::process::abort()
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
