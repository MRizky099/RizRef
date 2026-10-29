//! `rizref inspect`: look inside a board without opening a window.
//!
//! This is the file format's own debugging tool and the first consumer of the
//! header-only read path. It prints the header, the chunk table and a document
//! summary, and with `--verify` recomputes every digest in the file.
//!
//! Exit codes: `0` when everything checked out, `1` when the file could not be
//! read or verification found a problem, `2` for a malformed command line.

use std::process::ExitCode;

use rizref_format::{
    BoardReader, ChunkKind, FormatError, SceneDocument, VerifyReport, read_header,
};

/// Runs the subcommand with the arguments that follow `inspect`.
pub fn run(args: &[String]) -> ExitCode {
    let mut verify = false;
    let mut path = None;
    for arg in args {
        match arg.as_str() {
            "--verify" => verify = true,
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            flag if flag.starts_with('-') => {
                eprintln!("rizref inspect: unrecognised argument {flag:?}");
                eprintln!("rizref inspect --help");
                return ExitCode::from(2);
            }
            file => {
                if path.replace(file.to_owned()).is_some() {
                    eprintln!("rizref inspect: exactly one board file is expected");
                    return ExitCode::from(2);
                }
            }
        }
    }
    let Some(path) = path else {
        eprintln!("rizref inspect: a board file is required");
        eprintln!("rizref inspect --help");
        return ExitCode::from(2);
    };

    match inspect(&path, verify) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("rizref inspect: {error}");
            ExitCode::from(1)
        }
    }
}

/// Inspects `path` and prints the report.
fn inspect(path: &str, verify: bool) -> Result<ExitCode, FormatError> {
    let header = read_header(path)?;
    let mut reader = BoardReader::open_path(path)?;
    let index = reader.index().clone();
    let scan = reader.scan_chunks()?;
    let document = reader.document()?;
    let name = reader.name().ok().flatten();
    let thumbnail = reader.thumbnail().ok().flatten();
    let report = if verify { Some(reader.verify()?) } else { None };

    println!("{}", path);
    println!("  file size       {}", bytes(reader.file_len()));
    println!("  scene id        {}", header.scene_id);
    println!(
        "  container       revision {}, writer {}.{}",
        header.format_version,
        header.writer_version >> 8,
        header.writer_version & 0xff
    );
    println!("  created         {}", timestamp(header.created_ms));
    println!("  modified        {}", timestamp(index.modified_ms));
    println!(
        "  clean save      {}",
        if reader.recovered() {
            "no — recovered after an interrupted save"
        } else {
            "yes"
        }
    );
    println!(
        "  generation      {}, {} appended since the last full save",
        index.generation,
        bytes(index.appended_bytes)
    );
    if let Some(name) = &name {
        println!("  name            {name:?}");
    }
    match &thumbnail {
        Some(thumbnail) => println!(
            "  thumbnail       {} × {} px, {}",
            thumbnail.width,
            thumbnail.height,
            bytes(thumbnail.bytes.len() as u64)
        ),
        None => println!("  thumbnail       none"),
    }

    print_document(&document);
    print_chunks(&scan);

    match report {
        Some(report) => {
            print_verify(&report);
            Ok(if report.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

fn print_help() {
    println!("rizref inspect — summarise a .rizref board");
    println!();
    println!("USAGE:");
    println!("    rizref inspect <FILE> [--verify]");
    println!();
    println!("OPTIONS:");
    println!("    --verify    recompute every digest and check every blob");
    println!("    -h, --help  print this help and exit");
}

fn print_document(document: &SceneDocument) {
    println!(
        "  document        schema {}, {} items ({} images, {} notes), {} groups, {} assets",
        document.schema_version,
        document.items.len(),
        document.image_count(),
        document.note_count(),
        document.groups.len(),
        document.assets.len()
    );
    let camera = document.camera();
    println!(
        "  view            eye ({:.2}, {:.2}), zoom {:.0}%, rotation {:.1}°",
        camera.eye.x,
        camera.eye.y,
        camera.zoom * 100.0,
        camera.rotation.to_degrees()
    );
    if let Some(rect) = document.scene_rect {
        println!(
            "  scene rect      [{:.1}, {:.1}] .. [{:.1}, {:.1}]",
            rect[0], rect[1], rect[2], rect[3]
        );
    }
    let embedded = document
        .assets
        .iter()
        .filter(|asset| asset.embedded)
        .count();
    let linked = document.assets.len() - embedded;
    println!(
        "  assets          {embedded} embedded, {linked} linked; {} previews",
        document
            .assets
            .iter()
            .filter(|asset| asset.preview.is_some())
            .count()
    );
}

fn print_chunks(scan: &rizref_format::ChunkScan) {
    println!(
        "  chunks          {}{}",
        scan.chunks.len(),
        if scan.truncated {
            format!(" (stream truncated at offset {})", scan.end)
        } else {
            String::new()
        }
    );
    println!(
        "    {:>10}  {:<4}  {:>10}  {:>10}  codec",
        "offset", "tag", "stored", "raw"
    );
    for chunk in &scan.chunks {
        println!(
            "    {:>10}  {:<4}  {:>10}  {:>10}  {}",
            chunk.offset,
            kind_label(chunk.kind),
            chunk.stored_len,
            chunk.decompressed_len,
            if chunk.compression == 0 { "raw" } else { "lz4" }
        );
    }
}

fn print_verify(report: &VerifyReport) {
    if report.is_ok() {
        println!(
            "  verify          OK — {} chunks, {} assets, {} blobs hashed",
            report.chunks, report.assets, report.blobs_verified
        );
        if report.recovered {
            println!("                  (the file needed recovery, and passed)");
        }
    } else {
        println!(
            "  verify          FAILED — {} problem(s)",
            report.problems.len()
        );
        for problem in &report.problems {
            println!("                  · {problem}");
        }
    }
}

/// Formats a byte count for a human reading a terminal.
fn bytes(count: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if count >= GIB {
        format!("{:.1} GiB", count as f64 / GIB as f64)
    } else if count >= MIB {
        format!("{:.1} MiB", count as f64 / MIB as f64)
    } else if count >= KIB {
        format!("{:.1} KiB", count as f64 / KIB as f64)
    } else {
        format!("{count} B")
    }
}

/// Formats Unix milliseconds as `YYYY-MM-DD HH:MM:SS UTC`.
///
/// The civil-date conversion is Howard Hinnant's `civil_from_days`: no date
/// library is pulled in for one log-style line.
fn timestamp(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Days since 1970-01-01 to a civil `(year, month, day)`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(m <= 2);
    (year, m as u32, d as u32)
}

/// A short kind label for the chunk table.
fn kind_label(kind: ChunkKind) -> String {
    let tag = kind.tag();
    String::from_utf8_lossy(&tag).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{bytes, civil_from_days, timestamp};

    #[test]
    fn byte_counts_read_like_a_person_wrote_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KiB");
        assert_eq!(bytes(3 * 1024 * 1024), "3.0 MiB");
        assert_eq!(bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn timestamps_are_utc_and_zero_padded() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(timestamp(1_700_000_000_000), "2023-11-14 22:13:20 UTC");
    }
}
