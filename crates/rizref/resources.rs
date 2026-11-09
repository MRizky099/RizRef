//! The compiled resource (`.res`) writer, shared by `build.rs` (which links
//! the result into `rizref.exe`) and the unit tests (which check what it
//! embeds). A `.res` file is a sequence of entries, each a 32-byte header
//! naming an ordinal type and id, followed by the data.

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
/// en-US.
const LANGUAGE: u16 = 0x0409;

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// One resource entry: a 32-byte header (ordinal type and name) and data.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    push_u32(out, data.len() as u32);
    push_u32(out, 32);
    push_u16(out, 0xFFFF);
    push_u16(out, kind);
    push_u16(out, 0xFFFF);
    push_u16(out, id);
    push_u32(out, 0); // DataVersion
    push_u16(out, 0x1030); // MOVEABLE | PURE | DISCARDABLE
    push_u16(out, if kind == 0 { 0 } else { LANGUAGE });
    push_u32(out, 0); // Version
    push_u32(out, 0); // Characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// The resources: one `RT_GROUP_ICON` per ICO, ids 1, 2, … in order (so
/// `rizref.exe,0` is the first icon and `rizref.exe,1` the second), their
/// images as `RT_ICON` with ids unique across groups, and `RT_VERSION` 1.
/// An ICO too short to hold a header is skipped.
pub fn build_res(icos: &[&[u8]], version: &str) -> Vec<u8> {
    let mut out = Vec::new();
    // Every .res file starts with an empty entry.
    resource(&mut out, 0, 0, &[]);

    let mut next_image = 1_u16;
    let mut next_group = 1_u16;
    for ico in icos {
        // ICONDIR: reserved, type (1 = icon), count; then 16-byte entries.
        if ico.len() < 6 {
            continue;
        }
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        let mut group = Vec::new();
        push_u16(&mut group, 0);
        push_u16(&mut group, 1);
        push_u16(&mut group, count as u16);
        for index in 0..count {
            let entry = &ico[6 + index * 16..6 + (index + 1) * 16];
            let bytes = u32::from_le_bytes(entry[8..12].try_into().expect("4 bytes"));
            let offset = u32::from_le_bytes(entry[12..16].try_into().expect("4 bytes")) as usize;
            let id = next_image;
            next_image += 1;
            resource(&mut out, RT_ICON, id, &ico[offset..offset + bytes as usize]);
            // GRPICONDIRENTRY: the ICONDIRENTRY with the offset replaced by an id.
            group.extend_from_slice(&entry[..12]);
            push_u16(&mut group, id);
        }
        resource(&mut out, RT_GROUP_ICON, next_group, &group);
        next_group += 1;
    }
    resource(&mut out, RT_VERSION, 1, &version_info(version));
    out
}

fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A `VS_VERSIONINFO`-style node: length, value length, type, key, value,
/// children, each part 32-bit aligned.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    push_u16(&mut out, value_len);
    push_u16(&mut out, u16::from(text));
    out.extend_from_slice(&wide(key));
    pad4(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad4(&mut out);
        out.extend_from_slice(child);
    }
    let len = out.len() as u16;
    out[0..2].copy_from_slice(&len.to_le_bytes());
    out
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let data = wide(value);
    node(key, &data, (data.len() / 2) as u16, true, &[])
}

fn version_info(version: &str) -> Vec<u8> {
    let parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let ms = (u32::from(parts[0]) << 16) | u32::from(parts[1]);
    let ls = (u32::from(parts[2]) << 16) | u32::from(parts[3]);

    let mut fixed = Vec::new();
    push_u32(&mut fixed, 0xFEEF_04BD); // signature
    push_u32(&mut fixed, 0x0001_0000); // structure version
    push_u32(&mut fixed, ms); // file version
    push_u32(&mut fixed, ls);
    push_u32(&mut fixed, ms); // product version
    push_u32(&mut fixed, ls);
    push_u32(&mut fixed, 0x3F); // flags mask
    push_u32(&mut fixed, 0); // flags
    push_u32(&mut fixed, 0x0004_0004); // VOS_NT_WINDOWS32
    push_u32(&mut fixed, 1); // VFT_APP
    push_u32(&mut fixed, 0); // subtype
    push_u32(&mut fixed, 0); // date
    push_u32(&mut fixed, 0);

    let strings = node(
        "040904B0",
        &[],
        0,
        true,
        &[
            string("CompanyName", "Muhammad Rizky Abdillah (MRizky099)"),
            string("FileDescription", "RizRef reference board"),
            string("FileVersion", version),
            string("InternalName", "rizref"),
            string(
                "LegalCopyright",
                "Copyright (c) 2026 Muhammad Rizky Abdillah (MRizky099). MIT OR Apache-2.0.",
            ),
            string("OriginalFilename", "rizref.exe"),
            string("ProductName", "RizRef"),
            string("ProductVersion", version),
        ],
    );
    let string_info = node("StringFileInfo", &[], 0, true, &[strings]);
    let mut translation = Vec::new();
    push_u16(&mut translation, LANGUAGE);
    push_u16(&mut translation, 0x04B0); // Unicode code page
    let var = node("Translation", &translation, 4, false, &[]);
    let var_info = node("VarFileInfo", &[], 0, true, &[var]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[string_info, var_info],
    )
}
