//! Checks the icons `build.rs` embeds: both ICO assets and the `.res` the
//! build writes from them (through the same `resources.rs`).

use crate::resources::build_res;

/// The sizes `examples/make_icon.rs` writes.
const SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

const APP_ICO: &[u8] = include_bytes!("../assets/rizref.ico");
const DOCUMENT_ICO: &[u8] = include_bytes!("../assets/rizref_document.ico");

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

/// Each entry's declared side and the PNG's own width and height.
fn ico_entries(ico: &[u8]) -> Vec<(u32, u32, u32)> {
    assert_eq!(u16_at(ico, 0), 0, "reserved");
    assert_eq!(u16_at(ico, 2), 1, "type 1 = icon");
    (0..usize::from(u16_at(ico, 4)))
        .map(|index| {
            let entry = &ico[6 + index * 16..6 + (index + 1) * 16];
            let side = if entry[0] == 0 {
                256
            } else {
                u32::from(entry[0])
            };
            let (len, offset) = (u32_at(entry, 8) as usize, u32_at(entry, 12) as usize);
            let png = &ico[offset..offset + len];
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{side} px is a PNG");
            let width = u32::from_be_bytes(png[16..20].try_into().expect("IHDR"));
            let height = u32::from_be_bytes(png[20..24].try_into().expect("IHDR"));
            (side, width, height)
        })
        .collect()
}

#[test]
fn both_icons_hold_every_size_as_square_pngs() {
    for ico in [APP_ICO, DOCUMENT_ICO] {
        let entries = ico_entries(ico);
        let sides: Vec<u32> = entries.iter().map(|(side, _, _)| *side).collect();
        assert_eq!(sides, SIZES);
        for (side, width, height) in entries {
            // The document page is taller than wide: padded, not stretched.
            assert_eq!((width, height), (side, side));
        }
    }
}

/// `(type, id, data)` for every entry of a `.res` file.
fn res_entries(res: &[u8]) -> Vec<(u16, u16, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut at = 0;
    while at < res.len() {
        let size = u32_at(res, at) as usize;
        let header = u32_at(res, at + 4) as usize;
        assert_eq!(u16_at(res, at + 8), 0xFFFF, "ordinal type");
        let kind = u16_at(res, at + 10);
        assert_eq!(u16_at(res, at + 12), 0xFFFF, "ordinal id");
        let id = u16_at(res, at + 14);
        let data = res[at + header..at + header + size].to_vec();
        entries.push((kind, id, data));
        at += header + size;
        at = at.next_multiple_of(4);
    }
    entries
}

#[test]
fn the_exe_resources_hold_the_app_icon_then_the_document_icon() {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    const RT_VERSION: u16 = 16;

    let res = build_res(&[APP_ICO, DOCUMENT_ICO], "1.0.0");
    let entries = res_entries(&res);

    // Group 1 is index 0 (the app), group 2 is index 1 (`DefaultIcon`).
    let groups: Vec<&(u16, u16, Vec<u8>)> = entries
        .iter()
        .filter(|(kind, _, _)| *kind == RT_GROUP_ICON)
        .collect();
    assert_eq!(
        groups.iter().map(|(_, id, _)| *id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        crate::platform::register::DOCUMENT_ICON_INDEX,
        1,
        "the file icon names the second group"
    );

    // Every group entry points at an embedded RT_ICON of the right size.
    let images: Vec<&(u16, u16, Vec<u8>)> = entries
        .iter()
        .filter(|(kind, _, _)| *kind == RT_ICON)
        .collect();
    assert_eq!(
        images.len(),
        2 * SIZES.len(),
        "image ids unique across groups"
    );
    for ((_, _, group), ico) in groups.iter().zip([APP_ICO, DOCUMENT_ICO]) {
        assert_eq!(usize::from(u16_at(group, 4)), SIZES.len());
        for (index, side) in SIZES.iter().enumerate() {
            let entry = &group[6 + index * 14..6 + (index + 1) * 14];
            let declared = if entry[0] == 0 {
                256
            } else {
                u32::from(entry[0])
            };
            assert_eq!(declared, *side);
            let id = u16_at(entry, 12);
            let (_, _, data) = images
                .iter()
                .find(|(_, image, _)| *image == id)
                .expect("the image is embedded");
            assert_eq!(data.len() as u32, u32_at(entry, 8));
            assert!(
                ico.windows(data.len())
                    .any(|window| window == data.as_slice())
            );
        }
    }
    assert!(
        entries
            .iter()
            .any(|(kind, id, _)| (*kind, *id) == (RT_VERSION, 1))
    );
}

/// The macOS bundle icons `examples/make_icon.rs` writes.
const APP_ICNS: &[u8] = include_bytes!("../assets/rizref.icns");
const DOCUMENT_ICNS: &[u8] = include_bytes!("../assets/rizref_document.icns");

/// `(type, side)` of each `.icns` chunk, checking the framing on the way.
fn icns_chunks(icns: &[u8]) -> Vec<([u8; 4], u32)> {
    let be = |at: usize| u32::from_be_bytes(icns[at..at + 4].try_into().expect("4 bytes"));
    assert_eq!(&icns[..4], b"icns");
    assert_eq!(
        be(4) as usize,
        icns.len(),
        "the header length is the file length"
    );
    let mut chunks = Vec::new();
    let mut at = 8;
    while at < icns.len() {
        let kind: [u8; 4] = icns[at..at + 4].try_into().expect("4 bytes");
        let len = be(at + 4) as usize;
        let png = &icns[at + 8..at + len];
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "chunk {kind:?} is a PNG");
        chunks.push((kind, u32_at_be(png, 16)));
        at += len;
    }
    assert_eq!(at, icns.len(), "chunks end exactly at the file end");
    chunks
}

/// A PNG's IHDR width (big-endian, at byte 16).
fn u32_at_be(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

#[test]
fn both_icns_files_hold_every_size_finder_asks_for() {
    let expected: Vec<([u8; 4], u32)> = [
        (*b"icp4", 16),
        (*b"icp5", 32),
        (*b"icp6", 64),
        (*b"ic07", 128),
        (*b"ic08", 256),
        (*b"ic09", 512),
        (*b"ic10", 1024),
    ]
    .into();
    assert_eq!(icns_chunks(APP_ICNS), expected);
    assert_eq!(icns_chunks(DOCUMENT_ICNS), expected);
}

#[test]
fn the_bundle_plist_declares_the_board_type_it_opens() {
    let plist = include_str!("../../../packaging/macos/Info.plist");
    for needle in [
        "<string>com.rizref.board</string>",
        "<string>rizref</string>",
        "<string>application/x-rizref</string>",
        "<key>LSMinimumSystemVersion</key>\n\t<string>10.13</string>",
        "<key>CFBundleIconFile</key>\n\t<string>rizref</string>",
        "<string>rizref_document</string>",
        "<string>@VERSION@</string>",
    ] {
        assert!(plist.contains(needle), "Info.plist lacks {needle:?}");
    }
    assert_eq!(
        crate::platform::register::MIME_TYPE,
        "application/x-rizref",
        "the plist repeats the MIME type"
    );
}
