//! Benchmark for the container primitives.
//!
//! `cargo bench` should answer "did the file format get slower?" from phase 00
//! on, so the costs that sit in front of every save and every open are measured
//! here: encoding/decoding the header and a chunk frame, BLAKE3 over a large
//! blob, and the LZ4 round trip a document pays.
//!
//! It deliberately does **not** measure any phase-07 budget: board load, save,
//! cold start and binary size need the full application and are verified in
//! phase 07. The budgets themselves are recorded in `README.md` under
//! "Performance budgets".

use std::hint::black_box;
use std::time::Instant;

use rizref_core::SceneId;
use rizref_format::{CHUNK_HEADER_LEN, ChunkHeader, ChunkKind, Header, checksum, compression};

/// Payload used for the compression lines: 1 MiB, incompressible-ish content so
/// the numbers reflect a worst case rather than a buffer of zeroes.
const PAYLOAD_LEN: usize = 1024 * 1024;

/// Iterations for the header lines; header work is nanoseconds, not milliseconds.
const HEADER_ITERATIONS: u32 = 200_000;

/// Iterations for the compression lines; each one moves a megabyte.
const COMPRESSION_ITERATIONS: u32 = 32;

/// Prints nanoseconds per call for a cheap operation.
fn time_per_call(label: &str, iterations: u32, mut body: impl FnMut() -> u64) {
    for _ in 0..iterations / 10 {
        black_box(body());
    }

    let start = Instant::now();
    let mut checksum = 0_u64;
    for _ in 0..iterations {
        checksum = checksum.wrapping_add(black_box(body()));
    }
    let elapsed = start.elapsed();

    let nanos_per_call = elapsed.as_secs_f64() * 1.0e9 / f64::from(iterations);
    println!("{label:<34}{nanos_per_call:9.1} ns/op  (checksum {checksum})");
}

/// Prints megabytes per second for an operation that moves `bytes` bytes.
fn time_throughput(label: &str, bytes: usize, iterations: u32, mut body: impl FnMut() -> u64) {
    black_box(body());

    let start = Instant::now();
    let mut checksum = 0_u64;
    for _ in 0..iterations {
        checksum = checksum.wrapping_add(black_box(body()));
    }
    let elapsed = start.elapsed();

    let mebibytes = bytes as f64 * f64::from(iterations) / (1024.0 * 1024.0);
    let per_second = mebibytes / elapsed.as_secs_f64();
    println!("{label:<34}{per_second:9.1} MiB/s  (checksum {checksum})");
}

fn main() {
    let scene_id = SceneId::from_hex("0123456789abcdeffedcba9876543210").expect("valid hex id");
    let header = Header::new(scene_id, 1_700_000_000_000);
    let encoded = header.encode();

    println!("rizref-format benchmark");

    time_per_call("Header::encode", HEADER_ITERATIONS, || {
        u64::from((black_box(header.encode()))[8])
    });

    time_per_call("Header::decode", HEADER_ITERATIONS, || {
        Header::decode(black_box(&encoded))
            .expect("benchmark header is valid")
            .flags
    });

    let frame = ChunkHeader::raw(ChunkKind::Blob, PAYLOAD_LEN as u64).encode();
    time_per_call("ChunkHeader::encode", HEADER_ITERATIONS, || {
        u64::from((black_box(ChunkHeader::raw(ChunkKind::Blob, 1))).encode()[4])
    });
    time_per_call("ChunkHeader::decode", HEADER_ITERATIONS, || {
        u64::from(
            ChunkHeader::decode(black_box(&frame))
                .expect("benchmark frame is valid")
                .stored_len
                .min(u64::from(u32::MAX)) as u32,
        )
    });
    assert_eq!(frame.len(), CHUNK_HEADER_LEN);

    // A deterministic xorshift stream, standing in for the original image bytes
    // a board carries: those have already been compressed once by their own
    // format, so this is the honest case for both LZ4 and BLAKE3. Anything that
    // compresses to nothing here would flatter the numbers.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let payload: Vec<u8> = (0..PAYLOAD_LEN)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();

    let packed = compression::compress_lz4(&payload).expect("1 MiB is within the chunk limit");

    time_throughput(
        "blake3 digest (32 B)",
        PAYLOAD_LEN,
        COMPRESSION_ITERATIONS,
        || u64::from(checksum::digest(black_box(&payload))[0]),
    );

    time_throughput("lz4 compress", PAYLOAD_LEN, COMPRESSION_ITERATIONS, || {
        u64::from(
            compression::compress_lz4(black_box(&payload))
                .expect("within the chunk limit")
                .len() as u16,
        )
    });

    time_throughput(
        "lz4 decompress",
        PAYLOAD_LEN,
        COMPRESSION_ITERATIONS,
        || {
            u64::from(
                compression::decompress_lz4(black_box(&packed), PAYLOAD_LEN as u64)
                    .expect("valid lz4")
                    .len() as u32,
            )
        },
    );

    println!(
        "payload: {PAYLOAD_LEN} bytes, compressed to {} bytes ({:.1}%)",
        packed.len(),
        packed.len() as f64 * 100.0 / PAYLOAD_LEN as f64
    );
}
