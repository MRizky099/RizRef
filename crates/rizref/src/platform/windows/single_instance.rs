//! Single-instance guard, Windows half: a named mutex and a named pipe. The
//! wire format both ends speak lives in `platform::single_instance`.
//!
//! Two OS objects cooperate, both named so they are session-global:
//!
//! * a **named mutex** (`Local\RizRef.SingleInstance.v1`) — the first process
//!   to create it is the primary and holds it until it exits; the kernel
//!   destroys the mutex when the last handle closes, so a crashed primary
//!   cannot leave a stale lock behind;
//! * a **named pipe** (`\\.\pipe\RizRef.Args.v1`) — owned by the primary. A
//!   secondary connects, sends its command-line arguments as one message,
//!   waits for the one-byte acknowledgement, and exits.
//!
//! The `Local\` prefix scopes the mutex to the login session; two users on the
//! same machine each get their own RizRef. The pipe lives in the per-session
//! object namespace for the same reason.
//!
//! If the mutex says "primary already running" but the pipe never answers —
//! the primary died between the two checks — the secondary logs it and
//! continues as the new primary rather than refusing to start. Failing open
//! is the right bias for a viewer: a second window beats no window.

use std::io;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, FlushFileBuffers, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

use crate::platform::single_instance::{ACK, MAX_MESSAGE, decode_body, encode_message};

/// How long a secondary waits for the primary's busy pipe, milliseconds.
/// Generous on purpose: the primary may be mid-startup.
const PIPE_WAIT_MS: u32 = 2_000;
/// Pipe buffer size, bytes: the largest message accepted.
const PIPE_BUFFER: u32 = MAX_MESSAGE as u32;

// ---------------------------------------------------------------------------
// Handle discipline
// ---------------------------------------------------------------------------

/// Closes a kernel handle, ignoring errors: called from cleanup paths where
/// there is nothing left to do with a failure except not propagate it.
// SAFETY: the handle came from a successful create call and is closed exactly
// once — the caller passes ownership by giving up its copy right after.
#[allow(unsafe_code)]
fn close(handle: HANDLE) {
    let _ = unsafe { CloseHandle(handle) };
}

/// Maps a `windows` error onto the `io` error kind the caller already speaks.
fn to_io(error: windows::core::Error) -> io::Error {
    if WIN32_ERROR::from_error(&error) == Some(ERROR_PIPE_BUSY) {
        return io::Error::from(io::ErrorKind::ResourceBusy);
    }
    io::Error::other(error.to_string())
}

/// Reads exactly `buffer.len()` bytes from a synchronous file or pipe handle.
///
/// A peer that closes mid-message surfaces as `UnexpectedEof`.
// SAFETY: `ReadFile` receives a slice pointing only into `buffer`, which
/// outlives the call, and a valid out-pointer for the count read. The handle
// is synchronous (no OVERLAPPED anywhere), which is the contract `ReadFile`
// with a null OVERLAPPED requires.
#[allow(unsafe_code)]
fn read_exact(handle: HANDLE, buffer: &mut [u8]) -> io::Result<()> {
    let mut done = 0usize;
    while done < buffer.len() {
        let mut got = 0u32;
        let chunk = &mut buffer[done..];
        // SAFETY: see function note.
        let ok = unsafe { ReadFile(handle, Some(chunk), Some(&mut got), None) };
        ok.map_err(to_io)?;
        if got == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed"));
        }
        done += got as usize;
    }
    Ok(())
}

/// Writes all of `buffer` to a synchronous file or pipe handle.
// SAFETY: `WriteFile` receives a slice pointing only into `buffer`, which
// outlives the call, and a valid out-pointer. Synchronous handle as above.
#[allow(unsafe_code)]
fn write_all(handle: HANDLE, buffer: &[u8]) -> io::Result<()> {
    let mut done = 0usize;
    while done < buffer.len() {
        let chunk = &buffer[done..];
        let mut put = 0u32;
        // SAFETY: see function note.
        let ok = unsafe { WriteFile(handle, Some(chunk), Some(&mut put), None) };
        ok.map_err(to_io)?;
        done += put as usize;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------

/// Ownership of the single-instance mutex.
///
/// Held for the process's lifetime; `Drop` releases it. Not `Send` on
/// purpose: exactly one thread owns the "am I primary" decision.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// Tries to become the primary instance.
    ///
    /// `Ok` means this process owns the mutex and must run the pipe server.
    /// `Err` means another process holds it: forward your arguments and exit —
    /// or, if the primary turns out to be unreachable, continue anyway.
    ///
    /// A kernel failure to create the mutex also returns `Err` after logging:
    /// the caller treats it as "run the server-less fallback" rather than
    /// refusing to start, because a reference viewer that will not open is the
    /// worse failure.
    pub fn acquire() -> Result<Self, ()> {
        // SAFETY: the name is a static NUL-terminated wide literal; null
        // attributes mean default security and no inheritance. The call
        // returns a valid handle or an error, never a dangling one.
        #[allow(unsafe_code)]
        let created = unsafe { CreateMutexW(None, false, w!("Local\\RizRef.SingleInstance.v1")) };

        let handle = match created {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%error, "single-instance mutex unavailable; starting unguarded");
                return Err(());
            }
        };

        // `CreateMutexW` reports "already existed" through the last error
        // while still returning a valid handle to the same mutex. Read it
        // immediately after the call it describes.
        // SAFETY: `GetLastError` reads thread-local state; no intervening call
        // runs between the two statements.
        #[allow(unsafe_code)]
        let last = unsafe { GetLastError() };

        if last == ERROR_ALREADY_EXISTS {
            // The primary owns the mutex; this handle is one more reference to
            // the same object. Drop the reference — the secondary exits soon.
            close(handle);
            tracing::debug!("another instance owns the single-instance mutex");
            Err(())
        } else {
            tracing::debug!("single-instance mutex acquired");
            Ok(Self { handle })
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        close(self.handle);
    }
}

// ---------------------------------------------------------------------------
// The primary side: the pipe server
// ---------------------------------------------------------------------------

/// Runs the pipe server on a background thread until the process exits.
///
/// Each accepted message is decoded and handed to `on_args` on that thread;
/// the primary passes a closure that forwards into the event loop, which is
/// where "a second launch asked to open X" turns into state (phase 05+).
///
/// The thread never panics by construction: every fallible step is logged and
/// the loop continues. It is a daemon in practice — the process exits with
/// the window.
pub fn serve_forwarded_args<F>(on_args: F) -> JoinHandle<()>
where
    F: Fn(Vec<String>) + Send + 'static,
{
    std::thread::Builder::new()
        .name("args-pipe-server".into())
        .spawn(move || server_loop(on_args))
        .expect("spawning the args pipe server thread")
}

/// Accepts one client at a time, forever.
fn server_loop<F>(on_args: F)
where
    F: Fn(Vec<String>) + Send + 'static,
{
    // SAFETY: static name; byte-mode duplex pipe with one instance; null
    // security; blocking waits — matching every read/write helper above.
    #[allow(unsafe_code)]
    let created = unsafe {
        CreateNamedPipeW(
            w!(r"\\.\pipe\RizRef.Args.v1"),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            None,
        )
    };

    let pipe = match created {
        handle if !handle.is_invalid() => handle,
        _ => {
            tracing::error!("cannot create the args pipe; forwarding is disabled this run");
            return;
        }
    };

    loop {
        // Blocking wait for the next secondary. A client that vanishes
        // between connect and read surfaces as ERROR_NO_DATA or
        // ERROR_BROKEN_PIPE — a normal race, not a failure of the server.
        // A client that connected before this call completes surfaces as
        // ERROR_PIPE_CONNECTED, which is a connected client too.
        // SAFETY: valid pipe handle owned by this thread.
        #[allow(unsafe_code)]
        let connected = unsafe { ConnectNamedPipe(pipe, None) };
        let mut have_client = connected.is_ok();
        if !have_client {
            // SAFETY: reads the error of the call directly above.
            #[allow(unsafe_code)]
            let err = unsafe { GetLastError() };
            have_client = err == ERROR_NO_DATA
                || err == ERROR_BROKEN_PIPE
                || err == windows::Win32::Foundation::ERROR_PIPE_CONNECTED;
            if !have_client {
                tracing::debug!(code = err.0, "args pipe: connect failed");
            }
        }

        if have_client && let Some(args) = read_message(pipe) {
            tracing::info!(count = args.len(), args = ?args, "forwarded args received");
            on_args(args);
            // SAFETY: synchronous handle; one-byte write as above.
            #[allow(unsafe_code)]
            let _ = unsafe { WriteFile(pipe, Some(&[ACK]), None, None) };
        }

        // Flush the ack, then ready the instance for the next client.
        // SAFETY: valid pipe handle in the connected state.
        #[allow(unsafe_code)]
        let flushed = unsafe { FlushFileBuffers(pipe) };
        let _ = flushed;
        // SAFETY: valid pipe handle; no client is connected after the flush.
        #[allow(unsafe_code)]
        let disconnected = unsafe { DisconnectNamedPipe(pipe) };
        let _ = disconnected;
    }
}

/// Reads one length-prefixed message; `None` on any I/O error or malformed
/// payload. All failures are logged; none abort the server.
fn read_message(pipe: HANDLE) -> Option<Vec<String>> {
    let mut header = [0u8; 4];
    read_exact(pipe, &mut header)
        .inspect_err(|e| tracing::debug!(error = %e, "args pipe: header read failed"))
        .ok()?;
    let len = u32::from_le_bytes(header) as usize;

    // A length beyond the negotiated buffer is a protocol violation, not a
    // big message: refuse instead of allocating.
    if len as u32 > PIPE_BUFFER {
        tracing::warn!(len, "args pipe: oversized message refused");
        return None;
    }

    let mut body = vec![0u8; len];
    read_exact(pipe, &mut body)
        .inspect_err(|e| tracing::debug!(error = %e, "args pipe: body read failed"))
        .ok()?;
    let args = decode_body(&body);
    if args.is_none() {
        // A decode failure means the two sides disagree about the wire
        // format — worth a warning, not just a debug line, because it is a
        // version-skew smell, not a flaky I/O race.
        tracing::warn!(len, "args pipe: malformed message refused");
    }
    args
}

// ---------------------------------------------------------------------------
// The secondary side
// ---------------------------------------------------------------------------

/// Opens the primary's pipe for a read/write session.
// SAFETY: static name; access matching the server's duplex mode; no sharing;
// synchronous handle. On failure the win32 error is faithfully mapped into
// the returned `windows` error.
#[allow(unsafe_code)]
fn open_pipe() -> windows::core::Result<HANDLE> {
    // SAFETY: see function note.
    unsafe {
        CreateFileW(
            w!(r"\\.\pipe\RizRef.Args.v1"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
}

/// Sends `args` to the primary instance; `true` when the primary acked.
///
/// Returns `false` on any failure — unreachable pipe, a busy pipe that never
/// frees, a write error, a missing ack — leaving the caller to decide between
/// exiting (the guard worked) and continuing (the guard was stale).
pub fn forward_args_to_primary(args: &[String]) -> bool {
    let message = encode_message(args);

    // SAFETY: see `open_pipe`.
    #[allow(unsafe_code)]
    let opened = open_pipe();
    let pipe = match opened {
        Ok(handle) => handle,
        Err(error) => {
            if WIN32_ERROR::from_error(&error) == Some(ERROR_PIPE_BUSY) {
                tracing::debug!("args pipe busy; waiting for the primary");
                // SAFETY: static name; bounded wait, the standard busy-pipe
                // retry handshake.
                #[allow(unsafe_code)]
                let freed = unsafe { WaitNamedPipeW(w!(r"\\.\pipe\RizRef.Args.v1"), PIPE_WAIT_MS) };
                if !freed.as_bool() {
                    tracing::debug!("args pipe stayed busy; giving up on forwarding");
                    return false;
                }
                // SAFETY: see `open_pipe`.
                #[allow(unsafe_code)]
                match open_pipe() {
                    Ok(handle) => handle,
                    Err(error) => {
                        tracing::debug!(%error, "args pipe still unreachable");
                        return false;
                    }
                }
            } else {
                tracing::debug!(%error, "args pipe unreachable");
                return false;
            }
        }
    };

    let delivery = (|| -> Result<(), &'static str> {
        write_all(pipe, &message).map_err(|_| "write failed")?;
        let mut ack = [0u8; 1];
        read_exact(pipe, &mut ack).map_err(|_| "ack read failed")?;
        if ack[0] == ACK {
            Ok(())
        } else {
            Err("bad ack byte")
        }
    })();

    close(pipe);
    match delivery {
        Ok(()) => {
            tracing::info!("arguments handed to the running instance");
            true
        }
        Err(stage) => {
            tracing::debug!(stage, "forwarding failed after connecting");
            false
        }
    }
}
