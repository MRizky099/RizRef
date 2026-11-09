# The `.rizref` file format — v1

Status: **implemented** in `crates/rizref-format` (phase 06). This document is the
normative description of the bytes; the crate is the reference implementation.
The optional note fields in [Note payloads](#note-payloads-v11--planned-phase-06a) are a
planned additive revision for phase 06a; the `source_url` provenance semantics are
planned for phase 06b. The container and `schema_version` stay 1 in both.

A `.rizref` file is one board: the scene (items, groups, notes, assets, view), the
original image bytes, a thumbnail and the provenance hints needed to re-link an image
that was not embedded. It is self-contained, crash-safe and incrementally saveable.

All integers are little-endian.

## Overview

```
┌──────────────────── header (64 bytes) ─────────────────┐
│ magic │ header_size │ revision │ writer │ scene id …   │
├────────────────── chunk stream (repeated) ─────────────┤
│ NAME │ THMB │ DOC  │ BLOB* │ PREV* │ INDX │ END!       │
└────────────────────────────────────────────────────────┘
```

Every chunk has an independent frame and can be skipped without decoding it. The
writer appends new chunks when the board changes; the **last valid index** (`INDX`)
describes which chunks are live. The footer (`END!`) is written last, so its presence
at the very end of the file marks a clean save.

## Header — 64 bytes

| Offset | Size | Field | Meaning |
|---:|---:|---|---|
| 0x00 | 8 | `magic` | `RIZREF\x01\x00` — the trailing bytes are the revision in the magic itself |
| 0x08 | 4 | `header_size` | offset of the first chunk; **64** for v1, ≥ 64 in future revisions |
| 0x0C | 2 | `format_version` | **1** |
| 0x0E | 2 | `writer_version` | `major << 8 \| minor`; informational |
| 0x10 | 16 | `scene_id` | UUID v4 (version nibble 4, variant bits 10) |
| 0x20 | 8 | `created_ms` | Unix milliseconds of the first full save |
| 0x28 | 8 | `modified_ms` | Unix milliseconds of the last **full** save |
| 0x30 | 8 | `flags` | bit 0 = `HAS_FOOTER`; other bits reserved |
| 0x38 | 8 | `reserved` | zero |

There is deliberately **no header checksum**. An incremental save must never rewrite a
byte a previous clean save depends on; the only header field that ages is
`modified_ms`, and a torn 8-byte timestamp write costs a timestamp, not the board.
Structural damage is caught by the magic, the fixed field sizes, the chunk framing and
the index digests. `INDX.modified_ms` is authoritative after an append; the header's
value is the last full save.

A reader that does not know the revision encoded in the magic reports "not a RizRef
board"; a file whose magic was damaged is reported as damaged.

## Chunk frame — 24 bytes, then the payload

| Offset | Size | Field |
|---:|---:|---|
| 0x00 | 4 | `tag` (ASCII, case-sensitive) |
| 0x04 | 1 | `compression`: 0 = raw, 1 = LZ4 block |
| 0x05 | 3 | reserved, zero |
| 0x08 | 8 | `stored_len` — payload bytes on disk |
| 0x10 | 8 | `decompressed_len` — payload bytes after decompression |

For `compression = 0`, the two lengths must be equal. Unknown tags and unknown
compression codes are skipped by `stored_len`; a chunk from a newer build is never
interpreted by an older one. A chunk may not exceed **1 GiB** decompressed; this is a
guard against a corrupted length, not a design limit on boards.

### Tags

| Tag | Name | Payload |
|---|---|---|
| `NAME` | board name | raw UTF-8; the last one wins |
| `DOC ` | scene document | UTF-8 JSON (schema below) |
| `THMB` | board thumbnail | `u32 width`, `u32 height`, then encoded image bytes |
| `BLOB` | original image | 32-byte BLAKE3 of the original bytes, then those bytes verbatim |
| `PREV` | asset preview | 32-byte asset hash, then an encoded preview (JPEG or PNG) |
| `INDX` | board index | 32-byte BLAKE3 of the JSON that follows, then the JSON |
| `END!` | footer | the fixed 80-byte footer below |

`THUMB` and `INDEX` are five letters; the on-disk tags are the four-byte `THMB` and
`INDX`. `DOC ` has a trailing space so every tag is four bytes.

`BLOB` keeps the original bytes byte-for-byte, never re-encoded: EXIF and C2PA
metadata inside the image survive a round trip. `PREV` exists so a board opens with
pictures on screen before any original is decoded; it is optional and cosmetic.

## Footer — 80 bytes

The footer is the payload of the last `END!` chunk, which is the last chunk of a clean
file. It is also written with both magic copies so a reader can recognise it by
reading the final 80 bytes.

| Offset | Size | Field |
|---:|---:|---|
| 0x00 | 8 | `magic` |
| 0x08 | 2 | footer revision = 1 |
| 0x0A | 2 | flags, zero |
| 0x0C | 8 | `generation` |
| 0x14 | 8 | `index_offset` — absolute offset of the `INDX` chunk frame |
| 0x1C | 8 | `index_len` — stored payload length of that index |
| 0x24 | 32 | BLAKE3 of the index's **stored payload** |
| 0x44 | 4 | reserved |
| 0x48 | 8 | `magic` again |

## Scene document — `DOC ` JSON

```jsonc
{
  "schema_version": 1,
  "scene_id": "32 hex digits",            // must equal the header
  "camera": { "eye": [0.0, 0.0], "zoom": 1.0, "rotation": 0.0 },
  "scene_rect": [x0, y0, x1, y1],        // optional, informational
  "items": [
    { "id": 1, "kind": "image", "asset": "64 hex digits",
      "position": [x, y], "size": [w, h], "rotation": 0.0,
      "opacity": 1.0, "flip": [false, false] },
    { "id": 2, "kind": "note", "text": "study the hand",
      "position": [x, y], "size": [w, h], "rotation": 0.0,
      "opacity": 1.0, "flip": [false, false] }
  ],
  "groups": [
    { "id": 1, "children": [ { "kind": "item", "id": 1 },
                             { "kind": "group", "id": 2 } ] }
  ],
  "assets": [
    { "hash": "64 hex digits", "format": "png", "pixel_size": [1920, 1080],
      "byte_len": 2480041, "origin": "file",
      "source_path": "refs/characters/hero.png", "source_url": null,
      "embedded": true,
      "preview": { "format": "jpg", "pixel_size": [512, 288], "byte_len": 24011 } }
  ],
  "board": { "name": "My refs" }
}
```

* `items` are in **z-order**; the last item is drawn on top. `size` is an extent, not
  a corner, and is non-negative.
* `groups` are a flat list of nodes with child references; membership is exclusive.
  A child names either an item or another group.
* `assets[].hash` is the BLAKE3 of the original encoded bytes and is the content
  address of the `BLOB` chunk. Identical images are one asset, however many items use
  them.
* `assets[].embedded = false` declares a **linked** asset: no `BLOB` is required, the
  reader reports it as missing instead of failing the board, and `source_path` /
  `source_url` are hints for a relink flow. The v1 interface always embeds.
* `origin` is `file`, `clipboard`, `generated` or `unknown`. `clipboard` also covers a
  browser drag or a virtual file that arrived without a durable local path. All
  provenance is advisory; a board always loads from its embedded bytes.
* `source_path` is a durable local path when the image was imported from one.
  `source_url` is populated for an image pasted or dragged from a browser when the
  source exposes one (**planned, phase 06b**). It is the preferred *Open location*
  target and only `http`/`https` URLs are ever launched. A browser drag that also
  produces a temporary cache file stores the URL rather than the cache path; a
  URL-only drag or clipboard link is never downloaded on paste.
* `preview` describes the corresponding `PREV` chunk when there is one.
* `scene_rect` is informational; the live rectangle is recomputed from the items.

**Forward compatibility.** Every document object keeps members it does not recognise
and writes them back on resave. A newer build may add fields freely; an older build
may not change the meaning of an existing field without bumping `schema_version`. A
reader refuses a document whose `schema_version` is above its own rather than guessing.
The migration scaffold lives in `SceneDocument::from_value`: v1 parses directly, and a
future revision inserts a rewrite arm.

### Note payloads (v1.1 — planned, phase 06a)

The markdown-notes phase adds three optional members to note items. Nothing else in the
document or the container changes, and `schema_version` remains **1**: the new members
are additive, and a note without `text_format` still means exactly what it meant in v1
(plain text). The full design is in the phase plan; this section is the on-disk contract.

```jsonc
{
  "id": 2,
  "kind": "note",
  "text": "This is **bold** and <span style=\"color: #ff5733\">orange</span>.",
  "text_format": "markdown",   // optional: "plain" | "markdown"; absent = "plain"
  "background": "#302c25",     // optional: opaque #rrggbb; absent = the default card
  "wrap_width": 332.0,         // optional: content wrap width in world units
  "position": [x, y],
  "size": [w, h],
  "rotation": 0.0,
  "opacity": 1.0,
  "flip": [false, false]
}
```

* `text` is the note's source. With `text_format = "markdown"` it is rendered as the
  markdown subset documented in the phase plan; with `"plain"` (or no member) it is
  rendered as plain UTF-8 with `\n` line breaks, exactly as in v1.
* `background` is an opaque sRGB colour, serialised as lowercase `#rrggbb`. A reader also
  accepts `#rgb` and normalises it. An invalid value is ignored with a warning and the
  default card colour is used; **it must not fail the load**. The default is `#302c25`,
  the phase-04/05 card colour. Text colour is derived from the background at render
  time (light ink on dark backgrounds and vice versa); it is not stored.
* `wrap_width` is the width, in world units, at which the note's content was last laid
  out. Resizing the note changes `size` but never reflows the content; an explicit
  “Reflow to box width” action updates `wrap_width`. Values are finite and clamped to
  16–10,000. When absent, the renderer uses `size[0] - 2 × 14` (the note's inner width),
  which is the v1 behaviour. It never affects hit-testing or `size`.
* `rotation` is preserved for legacy boards. From phase 06a on, the editor never creates
  a rotated note; notes are excluded from rotation gestures and commands.
* Unknown members are still preserved. A v1 reader sees the three new members in its
  per-item `extra` map, ignores them while rendering, and writes them back on resave.

**Why no `schema_version` bump?** Bumping it would make every v1 build refuse the entire
board, including its images, even though the additions are optional. The format's own
rule allows new optional members; the meaning of the existing `text` field is unchanged
for a note without `text_format`. If a later revision needs to change an existing field's
meaning, that is a `schema_version: 2` decision with a hard gate.

**Index and thumbnail.** The `INDX` JSON is unchanged. `THMB` and PNG export paint a
note card in its stored `background` colour when the field is present; note text remains
unrendered in thumbnails, as in phase 06.

## Board index — `INDX` JSON

```jsonc
{
  "schema_version": 1,
  "generation": 3,
  "created_ms": 1700000000000,
  "modified_ms": 1700000123456,
  "appended_bytes": 8192,               // content bytes appended since the last full save (informational)
  "writer": "rizref 0.1.0",
  "name":      { "offset": 64,  "tag": "NAME", "stored_len": 10,
                 "decompressed_len": 10, "compression": 0, "digest": "…" },
  "document":  { "offset": 200, … },
  "thumbnail": { "offset": 98,  … },
  "assets": [
    { "hash": "64 hex digits",
      "blob":    { "offset": 400, … },
      "preview": { "offset": 900, … } }
  ]
}
```

A chunk reference's `digest` is BLAKE3 over the **decompressed payload**. For `BLOB`
and `PREV` the digest therefore covers the 32-byte hash prefix plus the bytes, while
the asset hash itself is that prefix (for a blob, also BLAKE3 of the original bytes).
`offset` points at the chunk frame, `stored_len` is its payload length.

The index JSON is prefixed with its own 32-byte digest inside the `INDX` payload. That
is what lets a recovery scan trust an index even when the footer was never written.

## Saving

### Fresh save (first save, Save As, compaction)

1. Write `<file>.rizref.tmp` in the target directory.
2. Write the header (`HAS_FOOTER`), then `NAME`, `THMB`, `DOC `, one `BLOB` per
   embedded asset, one `PREV` per asset, `INDX`, and finally `END!`.
3. `sync_all`, close, then rename the temporary over the target.

On Windows the rename replaces the target atomically; a crash at any point leaves the
old file untouched. A failed write removes the temporary.

### Incremental save

Reopen a clean file of the same scene id and append only what changed:

* a new `NAME` if the display name changed;
* a new `THMB` if the thumbnail changed;
* a new `DOC ` if the document JSON changed;
* a `BLOB` only for new assets (unchanged originals are reused by offset);
* a `PREV` only for new or previously preview-less assets;
* always a new `INDX` and `END!`.

Old chunks become dead space. The writer compacts instead of appending when the file
would grow beyond **three times its live bytes** — the same threshold as "dead space
over twice live", counted from the actual file size rather than a running estimate.
`appended_bytes` records the content bytes appended since the last full save for
diagnostics. Save As always writes fresh. A recovered, foreign, or different-scene
file is never appended to: the next save compacts it.

## Reading and recovery

1. Read and validate the header.
2. If `HAS_FOOTER` is set, read the final 80 bytes. If they decode as a footer, read
   the index it names, check the footer and index digests, and validate that every
   referenced chunk lies before the index and inside the file.
3. Otherwise **scan** the chunk stream from `header_size` to EOF, keeping the last
   `INDX` that parses and whose references are in bounds. A partially written index
   fails its own digest, so the previous generation wins. This is the crash case.
4. If no index survived, rebuild one from the last complete `DOC `, `NAME`, `THMB`,
   and the last `BLOB`/`PREV` per hash. A file with no complete document is reported
   as missing its document.
5. `BoardReader::load` then reads the document, embedded blobs and previews and
   reconstructs the scene. Blobs are read on demand and checked against their content
   hash; missing **linked** assets are reported and their items kept for relinking.

A reader that only needs the name and thumbnail (recents list, Explorer provider)
reads the header, footer, index, `NAME` and `THMB` — a handful of seeks regardless of
board size.

## Verification

`rizref inspect --verify` walks every chunk, decodes it, checks the index and footer
digests, checks each blob's content hash and each preview's asset hash, and reports
every problem. Exit code 0 means every check passed; 1 means a problem or an unreadable
file; 2 means a bad command line.

## Design notes and deviations from the phase-06 draft

* The draft proposed `THUMB` and an `INDEX` chunk. A chunk tag is four bytes, so the
  on-disk tags are `THMB` and `INDX`; the names in prose stay `THUMB`/`INDEX`.
* `PREV` is an addition: phase 07 budgets a board load that shows pictures before
  originals are decoded, and a preview is the only way to do that without re-encoding
  originals.
* Previews and thumbnails are JPEG when opaque and PNG when they contain transparency.
  The draft preferred lossy WebP, but the pure-Rust `image-webp` backend only encodes
  losslessly, and a lossless 512 px preview is several times larger than a JPEG. The
  container stores encoded bytes and does not care which codec won.
* The header has no checksum, by design (see above); the phase-00 skeleton's
  checksummed 44-byte header is superseded.
* JSON is the deliberate trade for the document: human-inspectable, tolerant of
  unknown fields, and a few tens of kilobytes for hundreds of items. The chunk
  framing leaves room for a binary `DOC2` later without breaking v1 readers.

## Revision history

| Revision | Phase | Change |
|---|---|---|
| 1 | 06 | Initial container, chunk stream, index/footer, scene document, blobs and previews. |
| 1.1 (planned) | 06a | Additive note members `text_format`, `background`, `wrap_width`. No container change; `schema_version` stays 1. |
| 1.2 (planned) | 06b | No new members. `source_url` is populated for browser pastes/drags; `origin`/`source_path` semantics clarified. |
