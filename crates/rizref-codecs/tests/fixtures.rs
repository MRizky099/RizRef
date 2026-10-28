//! Fixture-driven decode tests.
//!
//! Most files under `tests/data/` are produced by
//! `examples/make_fixtures.rs` and committed, so these tests see byte layouts
//! written by real encoders instead of only round-tripping our own calls. The
//! two `.heic` files are the exception: they come from the `heic-rs` test corpus
//! (see `tests/data/README.md`), because no encoder in this workspace writes
//! HEVC. They are the fixtures that prove the format is actually decoded, rather
//! than only sniffed.

use std::path::PathBuf;

use rizref_codecs::{
    CodecError, DecodeLimits, ImageFormat, decode, decode_as, decode_with_limits, sniff,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "cannot read {}: {error}\nrun `cargo run -p rizref-codecs --example make_fixtures`",
            path.display()
        )
    })
}

#[test]
fn every_committed_fixture_sniffs_as_the_format_its_name_says() {
    let cases = [
        ("rgba8.png", ImageFormat::Png),
        ("palette.png", ImageFormat::Png),
        ("gray16.png", ImageFormat::Png),
        ("baseline.jpg", ImageFormat::Jpeg),
        ("progressive.jpg", ImageFormat::Jpeg),
        ("gray.jpg", ImageFormat::Jpeg),
        ("exif-rotated.jpg", ImageFormat::Jpeg),
        ("lossless.webp", ImageFormat::WebP),
        ("lossy.webp", ImageFormat::WebP),
        ("animated.gif", ImageFormat::Gif),
        ("plain.bmp", ImageFormat::Bmp),
        ("plain.ppm", ImageFormat::Pnm),
        ("plain.qoi", ImageFormat::Qoi),
        ("multi.ico", ImageFormat::Ico),
        ("still.heic", ImageFormat::Heif),
        ("grid.heic", ImageFormat::Heif),
    ];

    for (name, expected) in cases {
        assert_eq!(sniff(&fixture(name)), Some(expected), "{name}");
    }
}

#[test]
fn png_variants_normalise_to_rgba() {
    let rgba = decode(&fixture("rgba8.png")).expect("rgba png");
    assert_eq!((rgba.width(), rgba.height()), (8, 6));
    assert_eq!(rgba.pixel(0, 0).expect("first pixel")[3], 96);

    let palette = decode(&fixture("palette.png")).expect("palette png");
    assert_eq!((palette.width(), palette.height()), (2, 2));
    assert_eq!(palette.pixel(0, 0), Some([255, 0, 0, 255]));
    assert_eq!(
        palette.pixel(1, 0),
        Some([0, 255, 0, 128]),
        "the palette's transparency chunk survives"
    );

    let gray16 = decode(&fixture("gray16.png")).expect("16-bit png");
    assert_eq!((gray16.width(), gray16.height()), (2, 1));
    assert_eq!(gray16.pixel(0, 0), Some([0, 0, 0, 255]));
    assert_eq!(gray16.pixel(1, 0), Some([255, 255, 255, 255]));
}

#[test]
fn jpeg_variants_decode_including_progressive_and_grayscale() {
    for name in ["baseline.jpg", "progressive.jpg"] {
        let image = decode(&fixture(name)).expect(name);
        assert_eq!((image.width(), image.height()), (8, 6), "{name}");
    }

    let gray = decode(&fixture("gray.jpg")).expect("grayscale jpeg");
    assert_eq!((gray.width(), gray.height()), (4, 1));
    for (x, expected) in [0_u8, 64, 128, 255].into_iter().enumerate() {
        let value = gray.pixel(x as u32, 0).expect("inside")[0];
        assert!(
            value.abs_diff(expected) <= 8,
            "gray sample {x} was {value}, expected about {expected}"
        );
    }
}

#[test]
fn exif_orientation_is_applied_before_the_caller_sees_the_image() {
    let upright = decode(&fixture("exif-rotated.jpg")).expect("rotated jpeg");

    // The encoded source is 8 x 6; orientation 6 turns it a quarter clockwise.
    assert_eq!((upright.width(), upright.height()), (6, 8));
}

#[test]
fn webp_lossy_and_lossless_both_decode() {
    let lossless = decode(&fixture("lossless.webp")).expect("lossless webp");
    assert_eq!((lossless.width(), lossless.height()), (8, 6));

    let lossy = decode(&fixture("lossy.webp")).expect("lossy webp");
    assert!(lossy.width() > 1 && lossy.height() > 1);
}

#[test]
fn an_animated_gif_yields_its_first_frame() {
    let animated = decode(&fixture("animated.gif")).expect("animated gif");

    assert_eq!((animated.width(), animated.height()), (4, 4));
    // The first frame is pure red; the second is blue.
    let pixel = animated.pixel(2, 2).expect("inside");
    assert!(
        pixel[0] > 200 && pixel[2] < 40,
        "unexpected pixel {pixel:?}"
    );
}

#[test]
fn the_plain_backends_decode_their_fixtures() {
    for name in ["plain.bmp", "plain.ppm", "plain.qoi"] {
        let image = decode(&fixture(name)).expect(name);
        assert!(image.width() > 0 && image.height() > 0, "{name}");
    }

    let tga = decode_as(&fixture("plain.tga"), ImageFormat::Tga).expect("tga");
    assert_eq!((tga.width(), tga.height()), (8, 6));

    let icon = decode(&fixture("multi.ico")).expect("ico");
    assert_eq!(
        (icon.width(), icon.height()),
        (4, 4),
        "the largest embedded icon wins"
    );
}

#[test]
fn truncating_a_file_never_panics_and_reports_damage() {
    for name in ["rgba8.png", "baseline.jpg", "progressive.jpg", "lossy.webp"] {
        if !PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name)
            .exists()
        {
            continue;
        }
        let bytes = fixture(name);
        for divisor in [2, 4] {
            let cut = bytes.len() / divisor;
            let outcome = std::panic::catch_unwind(|| decode(&bytes[..cut]));
            assert!(outcome.is_ok(), "{name} panicked at {cut} bytes");
            assert!(
                outcome.expect("checked").is_err(),
                "{name} truncated to {cut} bytes should not decode"
            );
        }
    }
}

#[test]
fn heic_single_item_and_grid_images_decode() {
    // A single coded picture.
    let still = decode(&fixture("still.heic")).expect("single-item heic");
    assert_eq!((still.width(), still.height()), (64, 64));
    let pixel = still.pixel(32, 32).expect("inside");
    assert_eq!(pixel[3], 255, "HEIC without alpha is opaque");

    // A 2 x 2 grid of 512 px tiles: the shape every iPhone photograph takes
    // above 512 px on a side. Decoding it proves tile compositing, not just the
    // single-item path.
    let grid = decode(&fixture("grid.heic")).expect("grid heic");
    assert_eq!((grid.width(), grid.height()), (1024, 1024));
}

#[test]
fn the_heic_pixel_cap_is_enforced_before_decoding() {
    let bytes = fixture("grid.heic");

    let error = decode_with_limits(
        &bytes,
        DecodeLimits {
            max_pixels: 1024 * 1024 - 1,
        },
    )
    .expect_err("one pixel under the grid");

    assert!(
        matches!(error, CodecError::Decode { .. }),
        "unexpected error: {error:?}"
    );
    assert!(error.to_string().contains("pixel limit"), "{error}");
}

#[test]
fn an_over_limit_file_is_refused_without_allocating_pixels() {
    let bytes = fixture("rgba8.png");

    let error = decode_with_limits(
        &bytes,
        DecodeLimits {
            max_pixels: 8 * 6 - 1,
        },
    )
    .expect_err("one pixel under the image");

    assert!(matches!(error, CodecError::TooLarge { .. }));
}

#[test]
fn the_fuzzer_found_ico_palette_overflow_is_a_decode_error() {
    // 121 bytes from the `decode_ico` fuzz target: a 4-bit icon whose palette
    // holds two colours while its pixels use colour 2. The `ico` crate indexed
    // its colour table unchecked and panicked before `decode_ico` validated the
    // entry itself.
    let bytes = fixture("malformed-ico-palette.ico");

    let error = decode(&bytes).expect_err("the palette is too short");

    assert!(
        matches!(
            error,
            CodecError::Decode {
                format: ImageFormat::Ico,
                ..
            }
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn an_eight_bit_ico_reproducer_does_not_panic() {
    // 70 bytes from the same fuzz target. The first version of the ICO
    // validator panicked on it while computing its bit mask (`1u8 << 8`).
    let bytes = fixture("malformed-ico-eight-bit.ico");

    let outcome = std::panic::catch_unwind(|| decode(&bytes));

    assert!(outcome.is_ok(), "decode panicked on the 8-bit reproducer");
}
