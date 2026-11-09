//! Single-instance guard: one RizRef process per session, with the arguments
//! of every further launch handed to the one already running.
//!
//! Each OS supplies a lock that decides who is primary and a channel that
//! carries a secondary's arguments to it:
//!
//! * **Windows** — a named mutex and a named pipe (`platform/windows/`);
//! * **Linux and macOS** — an advisory lock on a file and a Unix-domain socket
//!   next to it, in `$XDG_RUNTIME_DIR` (Linux) or the per-user temporary
//!   directory (macOS), both private to the user (`platform/unix/`).
//!
//! Both speak the wire format below, so the forwarding protocol, its limits
//! and its tests are shared. Paths are normalised by the *secondary* before
//! they are sent (`paths::normalize`), because the primary runs in another
//! working directory.
//!
//! If the lock says "primary already running" but the channel never answers —
//! the primary died between the two checks — the secondary logs it and
//! continues as the new primary rather than refusing to start. Failing open
//! is the right bias for a viewer: a second window beats no window.

#[cfg(windows)]
pub use super::windows::single_instance::{
    SingleInstance, forward_args_to_primary, serve_forwarded_args,
};

#[cfg(unix)]
pub use super::unix::single_instance::{
    SingleInstance, forward_args_to_primary, serve_forwarded_args,
};

/// The ack byte the primary sends after accepting a message.
pub(crate) const ACK: u8 = 0xA5;

/// The largest message body accepted, bytes. Messages are command lines — a
/// few hundred bytes; anything bigger is a protocol violation, not a big
/// message, and is refused instead of allocated.
pub(crate) const MAX_MESSAGE: usize = 8_192;

// ---------------------------------------------------------------------------
// Wire format. Pure functions, unit-tested.
//
// One message on the pipe or socket:
//     u32  body_len      little-endian, bytes of `body`
//     body:
//         u32  count     number of arguments
//         per argument:  u32 len + len bytes of UTF-8
//
// The outer length is what the server frames on; it is also why a partial
// write can never be misparsed as a shorter message.
// ---------------------------------------------------------------------------

/// Encodes arguments into one framed message (see the format note above).
pub(crate) fn encode_message(args: &[String]) -> Vec<u8> {
    let body = encode_body(args);
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Encodes the body: `u32 count`, then per argument `u32 len` + UTF-8 bytes.
fn encode_body(args: &[String]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + args.iter().map(|a| a.len() + 4).sum::<usize>());
    body.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for arg in args {
        body.extend_from_slice(&(arg.len() as u32).to_le_bytes());
        body.extend_from_slice(arg.as_bytes());
    }
    body
}

/// Decodes the body of a message — what the server reads after its own
/// length-prefix framing (the frame `encode_message` produced).
///
/// The decoder is total: every length is bounds-checked against the input, so
/// a truncated or hostile message decodes to `None`, never to a panic.
pub(crate) fn decode_body(body: &[u8]) -> Option<Vec<String>> {
    let read_u32 = |at: usize| -> Option<u32> {
        body.get(at..at.checked_add(4)?)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    };

    let count = read_u32(0)? as usize;
    let mut at = 4usize;
    let mut args = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        let len = read_u32(at)? as usize;
        at = at.checked_add(4)?;
        let raw = body.get(at..at.checked_add(len)?)?;
        at += len;
        args.push(String::from_utf8(raw.to_vec()).ok()?);
    }
    Some(args)
}

/// Reads one framed message from a byte stream: the length prefix, then a
/// body of at most [`MAX_MESSAGE`] bytes, decoded. `None` on any I/O error,
/// an oversized frame or a malformed body; every failure is logged.
///
/// Used by the Unix socket server; the Windows pipe server frames through
/// its own handle helpers but applies the same limits.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn read_message(stream: &mut impl std::io::Read) -> Option<Vec<String>> {
    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .inspect_err(|e| tracing::debug!(error = %e, "args channel: header read failed"))
        .ok()?;
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_MESSAGE {
        tracing::warn!(len, "args channel: oversized message refused");
        return None;
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .inspect_err(|e| tracing::debug!(error = %e, "args channel: body read failed"))
        .ok()?;
    let args = decode_body(&body);
    if args.is_none() {
        // A decode failure means the two sides disagree about the wire
        // format: a version-skew smell, not a flaky I/O race.
        tracing::warn!(len, "args channel: malformed message refused");
    }
    args
}

#[cfg(test)]
mod tests {
    use super::{MAX_MESSAGE, decode_body, encode_message, read_message};

    /// The seam under test: the client puts a framed message on the pipe, the
    /// server strips its 4-byte length prefix and hands the rest to
    /// [`decode_body`]. Every round-trip below goes through that exact path.
    fn from_client(args: &[String]) -> Option<Vec<String>> {
        let framed = encode_message(args);
        decode_body(&framed[4..])
    }

    #[test]
    fn a_message_round_trips_through_the_pipe_seam() {
        let args = vec!["board.rizref".to_owned(), "second board.rizref".to_owned()];
        assert_eq!(from_client(&args), Some(args));
    }

    #[test]
    fn an_empty_message_round_trips() {
        assert_eq!(from_client(&[]), Some(Vec::new()));
    }

    #[test]
    fn arguments_may_contain_any_utf8() {
        let args = vec!["path/with\nnewline and ünïcode".to_owned()];
        assert_eq!(from_client(&args), Some(args));
    }

    #[test]
    fn the_frame_declares_its_own_length() {
        // The seam that bit once: the server frames on the outer length. The
        // first four bytes of a message must therefore be the body size, and
        // the message must end exactly there.
        let bytes = encode_message(&["probe-board.rizref".to_owned()]);
        let declared = u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
        assert_eq!(
            declared + 4,
            bytes.len(),
            "frame length must cover the body exactly"
        );
    }

    #[test]
    fn garbage_decodes_to_none_not_a_panic() {
        assert_eq!(decode_body(&[]), None);
        assert_eq!(decode_body(&[0xFF, 0xFF, 0xFF, 0xFF]), None);
        // Count 1 but no argument follows.
        assert_eq!(decode_body(&[1, 0, 0, 0]), None);
        // Argument length that overruns the body.
        assert_eq!(
            decode_body(&[1, 0, 0, 0, u8::MAX, u8::MAX, u8::MAX, u8::MAX, 1]),
            None
        );
        // A well-formed message stays well-formed after its last byte flips.
        let mut bytes = encode_message(&["abc".to_owned()]);
        let last = bytes.len() - 1;
        bytes[last] = b'x';
        assert!(decode_body(&bytes[4..]).is_some());
        // One byte short of the frame: the server never sees this shape, but
        // the decoder is still total on it.
        bytes.truncate(last);
        assert_eq!(decode_body(&bytes[4..]), None);
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut bytes = encode_message(&["ok".to_owned()]);
        *bytes.last_mut().expect("non-empty") = 0xC3; // broken continuation byte
        assert_eq!(decode_body(&bytes[4..]), None);
    }

    #[test]
    fn a_stream_yields_exactly_one_framed_message() {
        let args = vec!["/home/u/a b.rizref".to_owned()];
        let mut wire = encode_message(&args);
        wire.extend_from_slice(b"trailing bytes belong to nobody");
        assert_eq!(read_message(&mut wire.as_slice()), Some(args));
    }

    #[test]
    fn a_stream_refuses_oversized_and_truncated_frames() {
        let oversized = ((MAX_MESSAGE + 1) as u32).to_le_bytes();
        assert_eq!(read_message(&mut oversized.as_slice()), None);
        let wire = encode_message(&["abc".to_owned()]);
        assert_eq!(read_message(&mut &wire[..wire.len() - 1]), None);
    }
}
