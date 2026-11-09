//! `rizref thumbnail -s SIZE INPUT OUTPUT`: a board's thumbnail as a PNG.
//!
//! The command the freedesktop thumbnailer entry runs (`rizref register`
//! writes `Exec=… thumbnail -s %s %i %o`): GNOME Files and the other file
//! managers that read `thumbnailers/*.thumbnailer` call it with the size they
//! want, the board's path and the PNG to write. The picture is the one the
//! Windows Explorer provider shows — `rizref_thumb::image_or_artwork`, which
//! reads only the board's stored `THMB` chunk or its per-asset previews,
//! never the original images — so a 2 GB board costs a few small reads.
//!
//! Exit codes: `0` with the PNG written, `1` when the input cannot be read
//! or the output cannot be written, `2` for bad arguments.

use std::path::Path;
use std::process::ExitCode;

/// The largest thumbnail side served: freedesktop's `xx-large` bucket.
const MAX_SIDE: u32 = 1024;

/// Parsed `thumbnail` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request<'a> {
    size: u32,
    input: &'a str,
    output: &'a str,
}

/// `[-s SIZE] INPUT OUTPUT` (the size defaults to 256, freedesktop's
/// `large`).
fn parse(args: &[String]) -> Option<Request<'_>> {
    let (size, rest) = match args {
        [flag, size, rest @ ..] if flag == "-s" || flag == "--size" => {
            (size.parse::<u32>().ok()?, rest)
        }
        rest => (256, rest),
    };
    match rest {
        [input, output] if (1..=MAX_SIDE).contains(&size) => Some(Request {
            size,
            input,
            output,
        }),
        _ => None,
    }
}

/// Runs the subcommand.
pub fn run(args: &[String]) -> ExitCode {
    let Some(request) = parse(args) else {
        eprintln!("usage: rizref thumbnail [-s SIZE] INPUT.rizref OUTPUT.png  (SIZE 1-{MAX_SIDE})");
        return ExitCode::from(2);
    };
    match render(&request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rizref thumbnail: {error}");
            ExitCode::FAILURE
        }
    }
}

fn render(request: &Request<'_>) -> Result<(), String> {
    let input = Path::new(request.input);
    let file =
        std::fs::File::open(input).map_err(|e| format!("cannot open {}: {e}", input.display()))?;
    let image = rizref_thumb::image_or_artwork(std::io::BufReader::new(file), request.size);
    let png = rizref_codecs::encode_png(&image).map_err(|e| format!("cannot encode: {e}"))?;
    std::fs::write(request.output, png).map_err(|e| format!("cannot write {}: {e}", request.output))
}

#[cfg(test)]
mod tests {
    use super::{Request, parse, run};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|&a| a.to_owned()).collect()
    }

    #[test]
    fn the_thumbnailer_command_line_parses() {
        let a = args(&["-s", "128", "/b/x.rizref", "/tmp/out.png"]);
        assert_eq!(
            parse(&a),
            Some(Request {
                size: 128,
                input: "/b/x.rizref",
                output: "/tmp/out.png"
            })
        );
        let a = args(&["/b/x.rizref", "/tmp/out.png"]);
        assert_eq!(parse(&a).map(|r| r.size), Some(256));
    }

    #[test]
    fn bad_arguments_are_refused() {
        for bad in [
            &["-s", "0", "a", "b"][..],
            &["-s", "99999", "a", "b"],
            &["-s", "big", "a", "b"],
            &["only-input"],
            &["a", "b", "c"],
        ] {
            assert_eq!(parse(&args(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn an_unreadable_board_still_gets_the_document_artwork() {
        let dir = std::env::temp_dir().join(format!("rizref-thumb-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let input = dir.join("not-a-board.rizref");
        std::fs::write(&input, b"definitely not a board").expect("input");
        let output = dir.join("out.png");
        let code = run(&args(&[
            "-s",
            "64",
            input.to_str().expect("utf-8"),
            output.to_str().expect("utf-8"),
        ]));
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        let png = std::fs::read(&output).expect("png written");
        let image = rizref_codecs::decode(&png).expect("valid png");
        assert!(image.width() <= 64 && image.height() <= 64);
        std::fs::remove_dir_all(&dir).ok();
    }
}
