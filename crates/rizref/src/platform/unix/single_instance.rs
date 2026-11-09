//! Single-instance guard, Linux and macOS half: an advisory file lock and a
//! Unix-domain socket.
//!
//! * `rizref.lock` — the first process to take an exclusive `flock` on it is
//!   the primary and holds it until it exits. The kernel drops the lock with
//!   the process, so a crashed primary cannot leave a stale lock behind (the
//!   file itself stays; it is never the signal, the lock is).
//! * `rizref.sock` — bound by the primary. A secondary connects, sends its
//!   arguments as one framed message (`platform::single_instance`), waits for
//!   the one-byte acknowledgement, and exits.
//!
//! Both live in a directory only this user can enter: `$XDG_RUNTIME_DIR` on
//! Linux (falling back to a `0700` directory in `/tmp` owned by the user), the
//! per-user `$TMPDIR` on macOS. Socket paths are short there, well inside the
//! 104-byte `sun_path` limit.
//!
//! Everything is `std`: no `unsafe`, no C.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::platform::single_instance::{ACK, encode_message, read_message};

/// How long either side waits on a stalled peer.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a secondary retries a primary that holds the lock but has not
/// bound its socket yet (it may be mid-startup).
const CONNECT_PATIENCE: Duration = Duration::from_secs(1);

/// The lock file and the socket of one runtime directory.
#[derive(Debug, Clone)]
pub(crate) struct Channel {
    lock: PathBuf,
    socket: PathBuf,
}

impl Channel {
    /// The channel files inside `dir`.
    pub(crate) fn in_dir(dir: &Path) -> Self {
        Self {
            lock: dir.join("rizref.lock"),
            socket: dir.join("rizref.sock"),
        }
    }

    /// This user's channel, creating its private directory if needed.
    fn for_user() -> io::Result<Self> {
        let dir = runtime_dir()?;
        Ok(Self::in_dir(&dir))
    }
}

/// The per-user directory the lock and socket live in.
fn runtime_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir())
    {
        return Ok(dir);
    }
    // macOS's `$TMPDIR` is already per-user and private; `/tmp` on Linux is
    // not, so the fallback is a directory of our own that nobody else may
    // enter, and one that someone else created first is refused.
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_owned());
    let dir = std::env::temp_dir().join(format!("rizref-{user}"));
    private_dir(&dir)?;
    Ok(dir)
}

/// Creates `dir` with mode `0700`, or checks an existing one is ours and
/// private.
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    let mine = own_uid().is_none_or(|uid| uid == meta.uid());
    if !meta.is_dir() || !mine || meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory of this user", dir.display()),
        ));
    }
    Ok(())
}

/// This process's user id, where the OS exposes it without a libc call.
fn own_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    // `/proc/self` belongs to the process's real user on Linux. macOS has no
    // procfs, but its fallback directory is under the per-user `$TMPDIR`.
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Ownership of the single-instance lock.
///
/// Held for the process's lifetime; dropping it releases the lock. Not
/// `Send` on purpose: exactly one thread owns the "am I primary" decision.
pub struct SingleInstance {
    _lock: File,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl SingleInstance {
    /// Tries to become the primary instance.
    ///
    /// `Ok` means this process holds the lock and must run the socket server.
    /// `Err` means another process holds it — or the lock could not be taken
    /// at all, which is logged and treated the same way: forward the
    /// arguments, and continue as the instance when nobody answers.
    pub fn acquire() -> Result<Self, ()> {
        let channel = Channel::for_user().map_err(|error| {
            tracing::warn!(%error, "no private runtime directory; starting unguarded");
        })?;
        Self::acquire_in(&channel)
    }

    /// [`acquire`](Self::acquire) on an explicit channel (the tests' seam).
    pub(crate) fn acquire_in(channel: &Channel) -> Result<Self, ()> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&channel.lock)
            .map_err(|error| {
                tracing::warn!(%error, "single-instance lock unavailable; starting unguarded");
            })?;
        match file.try_lock() {
            Ok(()) => {
                tracing::debug!(lock = %channel.lock.display(), "single-instance lock acquired");
                Ok(Self {
                    _lock: file,
                    _not_send: std::marker::PhantomData,
                })
            }
            Err(TryLockError::WouldBlock) => {
                tracing::debug!("another instance holds the single-instance lock");
                Err(())
            }
            Err(TryLockError::Error(error)) => {
                tracing::warn!(%error, "single-instance lock failed; starting unguarded");
                Err(())
            }
        }
    }
}

/// Runs the socket server on a background thread until the process exits.
///
/// Each accepted message is decoded and handed to `on_args` on that thread.
/// The thread never panics by construction: every fallible step is logged
/// and the loop continues.
pub fn serve_forwarded_args<F>(on_args: F) -> JoinHandle<()>
where
    F: Fn(Vec<String>) + Send + 'static,
{
    let channel = Channel::for_user();
    std::thread::Builder::new()
        .name("args-socket-server".into())
        .spawn(move || match channel {
            Ok(channel) => match bind(&channel) {
                Ok(listener) => server_loop(&listener, on_args),
                Err(error) => {
                    tracing::error!(%error, "cannot bind the args socket; forwarding is disabled this run");
                }
            },
            Err(error) => {
                tracing::error!(%error, "no runtime directory; forwarding is disabled this run");
            }
        })
        .expect("spawning the args socket server thread")
}

/// Binds the channel's socket, replacing a stale one. Only the lock holder
/// calls this, so no live primary can own the file being replaced.
pub(crate) fn bind(channel: &Channel) -> io::Result<UnixListener> {
    match std::fs::remove_file(&channel.socket) {
        Ok(()) => tracing::debug!("removed a stale args socket"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    UnixListener::bind(&channel.socket)
}

/// Accepts one client at a time, forever.
pub(crate) fn server_loop<F>(listener: &UnixListener, on_args: F)
where
    F: Fn(Vec<String>),
{
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                tracing::debug!(%error, "args socket: accept failed");
                continue;
            }
        };
        // A client that connects and then stalls must not wedge the server.
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        if let Some(args) = read_message(&mut stream) {
            tracing::info!(count = args.len(), args = ?args, "forwarded args received");
            on_args(args);
            let _ = stream.write_all(&[ACK]);
        }
    }
}

/// Sends `args` to the primary instance; `true` when the primary acked.
///
/// Returns `false` on any failure — no socket, a refused connection, a write
/// error, a missing ack — leaving the caller to decide between exiting (the
/// guard worked) and continuing (the guard was stale).
pub fn forward_args_to_primary(args: &[String]) -> bool {
    match Channel::for_user() {
        Ok(channel) => forward_in(&channel, args),
        Err(error) => {
            tracing::debug!(%error, "no runtime directory; cannot forward");
            false
        }
    }
}

/// [`forward_args_to_primary`] on an explicit channel (the tests' seam).
pub(crate) fn forward_in(channel: &Channel, args: &[String]) -> bool {
    let deadline = Instant::now() + CONNECT_PATIENCE;
    let mut stream = loop {
        match UnixStream::connect(&channel.socket) {
            Ok(stream) => break stream,
            Err(error)
                if Instant::now() < deadline
                    && matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                tracing::debug!(%error, "args socket unreachable");
                return false;
            }
        }
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let delivery = (|| -> Result<(), &'static str> {
        stream
            .write_all(&encode_message(args))
            .map_err(|_| "write failed")?;
        let mut ack = [0u8; 1];
        io::Read::read_exact(&mut stream, &mut ack).map_err(|_| "ack read failed")?;
        if ack[0] == ACK {
            Ok(())
        } else {
            Err("bad ack byte")
        }
    })();
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

#[cfg(test)]
mod tests {
    use super::{Channel, SingleInstance, bind, forward_in, private_dir, server_loop};
    use std::sync::mpsc;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rizref-si-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        private_dir(&dir).expect("private scratch dir");
        dir
    }

    #[test]
    fn one_primary_per_channel_until_it_lets_go() {
        let dir = scratch("lock");
        let channel = Channel::in_dir(&dir);
        let primary = SingleInstance::acquire_in(&channel).expect("first is primary");
        assert!(
            SingleInstance::acquire_in(&channel).is_err(),
            "second waits"
        );
        drop(primary);
        assert!(
            SingleInstance::acquire_in(&channel).is_ok(),
            "lock released"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn arguments_reach_the_primary_and_are_acknowledged() {
        let dir = scratch("forward");
        let channel = Channel::in_dir(&dir);
        let _primary = SingleInstance::acquire_in(&channel).expect("primary");
        // A stale socket file from a crashed primary is replaced.
        std::fs::write(dir.join("rizref.sock"), b"stale").expect("stale file");
        let listener = bind(&channel).expect("bind");
        let (sent, received) = mpsc::channel();
        std::thread::spawn(move || server_loop(&listener, move |args| sent.send(args).unwrap()));

        let args = vec!["/home/u/refs/a b.rizref".to_owned(), "-".to_owned()];
        assert!(forward_in(&channel, &args));
        assert_eq!(received.recv().expect("delivered"), args);
        assert!(
            forward_in(&channel, &["second".to_owned()]),
            "server keeps going"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nobody_listening_fails_open() {
        let dir = scratch("absent");
        assert!(!forward_in(&Channel::in_dir(&dir), &["x".to_owned()]));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_shared_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("shared");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        assert!(private_dir(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
