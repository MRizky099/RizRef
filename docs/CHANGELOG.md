# Changelog

Summary of what each delivery phase landed. One section per phase; newest at the top.

## Phase 07 — Polish & ship (done)

Branch `feat/phase-07-polish-and-ship`, closes issue #8. Version **1.0.0**, licensed
MIT OR Apache-2.0. No format change ([`format.md`](format.md) revision 1.2 gains a
documentation-only phase-07 row). Measurements: README → "Performance budgets"
(reference laptop: i7-4720HQ, GTX 950M, Windows 10 22H2).

Performance — every phase-00 budget measured by a committed harness, and met:

- Cold start: the first frame waited for the GPU (1,059 ms median, ~800 ms of it the
  NVIDIA driver creating the instance and device). `renderer::GpuDevice` (instance,
  adapter, device, pipelines) is now built on a thread started after the first frame,
  and until it arrives frames are the CPU-painted shell layer blitted with
  `SetDIBitsToDevice` (`platform::gdi`). First frame **130 ms** median, 226 ms worst;
  first GPU frame ~1.04 s. Starting the thread earlier was measured to stall window
  creation by up to ~500 ms in one launch out of five.
- `perf.rs`: a startup waterfall (logging, event loop, window, first frame, GPU, GPU
  frame) logged per launch; `--exit-after-first-frame` prints it as JSON for
  `scripts/bench-startup.ps1`. **F3** swaps the readout for a developer overlay (last
  frame, p95, worst, drawn/total items, textures and MB, decode queue).
  `--bench-frames=N <board>` drives a deterministic pan/zoom path and prints frame work
  (swapchain acquire and present excluded) and total p50/p95/max plus the open-to-first-
  GPU-frame time. The benches skip the single-instance handshake and the session marker.
- `rizref-core`: `Scene::visible_items`; the render loop instances and keeps warm only
  the visible set (5.8 µs per frame for 500 items). Frame work **0.59 ms** median,
  0.96 ms p95 on a 100-item board, 1.04 ms p95 on a 500-item 1.6 GB board.
- `rizref-format`: `load` hashed every original twice (chunk digest and content hash);
  blobs now verify by content hash only (which fixes every payload byte), and blob
  verification and preview decoding run on scoped threads. `benches/board_io.rs`
  (100 items, 210 MB): load 606 → **300 ms**, incremental save **96 ms** (item moved)
  and **121 ms** (2 MiB image added), summary 0.4 ms. A 327 MB 100-item board reaches
  its first preview frame in **905 ms**. `examples/make_soak_board.rs` writes the large
  real-JPEG boards.
- Binary size: thin LTO at opt 3 was 18.37 MB, over budget. Fat LTO at `opt-level = "s"`
  plus the static CRT: **14.28 MB** (14.58 MB with the artwork icons). `examples/decode_bench.rs` records the cost (12 MP
  JPEG 139 → 154 ms, PNG 116 → 154 ms); `opt-level = "z"` (12.52 MB) was rejected for a
  3.5× slower JPEG decode.
- The release exe imported `VCRUNTIME140.dll` and six `api-ms-win-crt-*` forwarders,
  absent on a clean Windows without the VC++ Redistributable. `.cargo/config.toml`
  links the CRT statically; `scripts/check-imports.ps1` reads the PE import table (no
  dumpbin) and fails on any C/C++ runtime. Release builds are GUI-subsystem (no console
  flash) and reattach to the parent console for `--help`/`--version`/`inspect`.

Explorer and shell integration:

- `rizref-thumb` (new crate, cdylib): an in-process COM thumbnail provider
  (`IInitializeWithStream` + `IThumbnailProvider`, class factory, `DllGetClassObject`/
  `DllCanUnloadNow`) reading through an `IStream` adapter with `rizref-format`.
  `BoardReader::thumbnail_image` decodes `THMB` or, without one, composites the
  document and `PREV` chunks (`preview_composite`); notes keep their 06a background
  colour either way. Never reads originals. Box-filtered, premultiplied BGRA DIB.
- `rizref register [--machine]` / `unregister`: `.rizref` → `RizRef.Board`, icon,
  "Open with RizRef", `Applications\rizref.exe`, and the thumbnail CLSID when
  `rizref_thumb.dll` sits beside the exe; `HKCU` by default, `HKLM` for an elevated
  installer; `SHChangeNotify(SHCNE_ASSOCCHANGED)`. The registry layout is a pure,
  tested plan.
- Jump list: opened and saved boards go to `SHAddToRecentDocs`, which feeds the taskbar
  jump list's Recent category once the type is registered (Windows keeps the list,
  pinning and removal; no `ICustomDestinationList` needed).
- The exe embeds two icons and version info, through a `build.rs` that writes the
  `.res` itself (`resources.rs`): no `rc.exe`, no C toolchain. `RT_GROUP_ICON` 1 is the
  application icon and 2 the `.rizref` document icon, generated from the user's
  artwork (`assets/app_icon.png`, `assets/app_document_icon.png`, committed) by
  `examples/make_icon.rs`: trimmed, padded to a centred square (the page is never
  stretched), Lanczos-3 in premultiplied colour, a light unsharp mask at 32 px and
  under, and a simplified mark where the full art turns to noise — the magnifier
  alone at 16–24 px, and the page with that magnifier large up to 32 px. Ten sizes,
  16–256, stored as PNG. `--sheet <png>` writes a review contact sheet. The window
  sets the app icon at runtime (title bar, Alt-Tab, running taskbar button), and
  `DefaultIcon` names `rizref.exe,1`, the document icon.
- `rizref register` / `unregister` were never reachable: `platform/registry.rs` was
  written and documented but neither declared as a module nor dispatched, so the
  subcommands fell through to opening a file named "register" (and its tests never
  ran). Both are wired now and listed in `--help`. The registry plan keeps its own
  copy of the thumbnail class id, so the exe does not link the DLL's COM exports; a
  test checks the copy against `rizref_thumb`.
- The thumbnail DLL shows the document artwork instead of failing or drawing a blank
  page for a board with nothing to picture (an empty board saves a 1 x 1 thumbnail)
  or one it cannot read. Board thumbnails get no corner badge: tried at 30% of the
  long side, it covered a note on a wide board at 256 px and dominated at 96 px.
  `examples/thumb_preview.rs` renders what Explorer would show.

Reliability:

- Crash minidumps: an unhandled-exception filter writes a dump through the OS's
  `dbghelp` `MiniDumpWriteDump` to `%LOCALAPPDATA%\RizRef\crashes` (ten kept);
  `--crash-test` proves it (exit `0xC0000005`, a 127 KB dump). No new crate — the plan
  named `minidump-writer` behind a vectored handler; a vectored handler also sees
  handled first-chance exceptions.
- Autosave: every 20 changes or 60 s to a recovery copy
  (`%LOCALAPPDATA%\RizRef\autosave\<scene>.rizref` + sidecar), never the user's file;
  incremental appends after the first write. Deleted on save, discard and clean exit.
  A session marker detects an unclean exit; the next launch offers the newest copy in
  an in-app "Recover unsaved work" dialog (Restore, the default, loads it as an unsaved
  document named after its board; "Discard the copy" deletes it) or, with nothing to
  recover, reopens the last board. A test spawns a child that autosaves in a loop and
  kills it six times mid-write: every copy left behind loads.
- Fixes from the first manual test run: the recovery offer was a native modal box
  opened before the window had presented — it stayed hidden and blocked the event
  loop, so a relaunch after a crash looked dead (7.7); it is now the in-app dialog
  above. The first GPU frame reused a UI layer the CPU frames had already consumed, so
  the start view showed a bare canvas until the next input; attaching the GPU now
  forces a repaint. Both checked on the release build by launching it (the offer,
  Restore, the start view and a board opened from the command line draw unprompted).
- Fixes from the second manual test run:
  - File dialogs (Open images, Open board, Save board, Export PNG) are owned by the
    main window: with always-on-top on they opened behind it, focused but unseen. The
    fatal alert is `MB_TOPMOST`. (The unsaved-changes guard is in-app and was fine.)
  - The revealed title bar was never drawn over a board: the GPU frame composited
    the UI layer only for the start view, panels, toasts and window modes, so
    hovering the top strip changed the bar's state invisibly. A revealed bar and a
    note-toolbar tooltip now count, and a layer that becomes visible again is
    uploaded even without a repaint.
  - Scaling anchors the opposite corner or edge by default; `alt` scales about the
    centre. Image corner drags keep the aspect ratio exactly (`shift` frees it);
    notes resize freely (`shift` locks). This reverses the phase 04 default.
  - The note hover toolbar and the ✗/✓ edit pill sit outside the card — above its
    top corners, or below when there is no room above or the toolbar would cover
    the rotate handle — so they never cover the note's text. A note larger than the
    window gets them pinned to the window's top edge.
  - The zoom floor is 0.5% (was 5%) for Fit and the wheel alike, so Fit frames any
    board; the grid hides below 5%.
  - Grid arrange packs row by row (selection order, rows as tall as their tallest
    item, even gaps, near-square wrap) instead of uniform cells sized to the largest
    item; still one undo step.
  - Fourth run: an embedded TGA stayed on its 512 px preview after the board was
    reopened. The full-resolution decode got the bytes alone and TGA has no
    signature; it now gets the asset's recorded `format` (already in every asset
    entry, so no format change), then the file extension, then a TGA header check.
    Dropped virtual files (Outlook attachments) pass their name's extension too. A
    test round-trips TGA, PPM, QOI and ICO through import, save, load and a
    full-resolution decode.
  - Recent-board cards draw the stored board thumbnail (the monogram is the
    fallback, also for an empty board's 1 x 1 picture). `recents.json` stores the
    preview as base64 instead of a JSON array of numbers, about a third of the
    size; the array form is still read.
  - Recent boards: the same board opened as `F:/a/b.rizref` and saved as
    `F:\a\b.rizref` was two cards, and a relative command-line path was stored as
    given. Paths are now made absolute against the working directory as they arrive
    (command line, forwarded from a second instance, jump list) and normalised
    (`\`, `.`/`..`, no `\\?\` prefix), and compared without regard to case.
    Loading `recents.json` merges duplicates, keeping the most recent edit, and
    drops a relative entry that does not resolve to a board.
  - A failed import was logged twice, by the worker and by the application; the
    worker's line is debug now. A failed full-resolution decode is a warning.
  - Sixth run: the two note budgets phase 06a deferred are met.
    - Rendering: the text, decorations and equations of a markdown note are painted
      once into a cached layer (keyed by source hash, wrap width, density and
      palette; 64 MiB) and blitted onto a fresh card; the card is drawn analytically
      (tiny-skia's anti-aliased path fill and stroke took 33 ms of a 626 x 2048
      card) and the straight-alpha conversion is table-driven (17 ms before).
    - Off the UI thread: every note render except the live editor's runs on a note
      worker with its own font system; requests coalesce per note, a panic is
      contained, and the note shows its last texture or a card-only placeholder
      (0.11 ms) meanwhile. Notes more than half a viewport away are not rendered.
    - Budget: full-resolution note textures stay within 64 MiB; off-screen notes,
      least recently seen first, drop to a 256 px copy kept from their last render
      and come back sharp, without a blank frame, when they return.
    - Measured with `note_budgets` (release, reference laptop), before (`32892e0`)
      and after: warm re-render of a 4,000-character note 61.1 ms to **7.1 ms**
      (budget 8 ms); a resize that keeps the wrap width 11.7 ms (it re-used
      the layout before but repainted everything); 20 equations cold 35.7 ms to
      14.3 ms and cached 26.3 ms to 4.0 ms; cold layout ~100 ms, unchanged but
      on the worker. A board of four 3.8k-character notes with 20 equations each
      reaches its first interactive frame in 252 to 340 ms instead of 557 to 764
      ms (`--bench-frames`, three runs each); steady pan/zoom frame work is
      unchanged (p95 0.7 ms).
  - Menu items publish their shortcut as AccessKit `keyboard_shortcut`, from the
    live table, so a rebinding shows up. UIA `AcceleratorKey` still reads empty:
    no `accesskit_windows` release (0.32.1 here, 0.35.2 newest) maps any node
    property to it, so the shortcut also stays in the description, which Narrator
    reads as the full description. Tests: Save gives `Ctrl+S`, and a rebound Undo
    gives the new chord.
  - Installer: the folder page always shows, also on upgrades (pre-filled with the
    previous folder). The default follows the install mode (`{autopf}`: still
    `%LOCALAPPDATA%\Programs\RizRef` per user, Program Files for all users). Setup
    checks the chosen folder before copying: a folder a per-user install cannot
    write to (Program Files without elevation) gets a message pointing at the
    all-users mode, and a network folder asks first because Explorer will not load
    the thumbnail DLL from it. `register` / `unregister` (with `--machine` for all
    users) run from the chosen folder; uninstall removes it. Silent installs take
    `/DIR="..."` and `/ALLUSERS` or `/CURRENTUSER`. Not compiled here (no Inno Setup
    on this machine).
  - Title-bar buttons show a tooltip after 500 ms on the button, worded for the
    state and with the live shortcut: "Main menu", "Pin on top (Ctrl+Shift+T)" /
    "Unpin", "Minimize", "Maximize" / "Restore", "Close (Ctrl+Q)". It reuses the
    note-toolbar tooltip (clamped inside the window); moving between buttons
    restarts the delay, and the note hover never overwrites it.
  - View → **Scene boundary** (Ctrl+B, checkable, persisted as `scene_border`) next
    to Checkerboard and HUD readout; it was only in View → Grid as "Scene border".
    Named the same in the F1 overlay and settings, and exposed in the UIA tree like
    every menu item. On screen only: exports and thumbnails are composited by
    `rizref-format`, which never draws it.
  - Transparency is the canvas background only: the steps (100/80/60/40/20 %) and
    Ctrl+Shift+O now fade the fill, checkerboard, grid and boundary, while images,
    notes, selection chrome, the HUD, toasts and the title bar stay opaque, and the
    transparent canvas still takes clicks (it is not a layered window). How: the
    window is created transparent (DWM alpha composition), DX12 is tried before
    Vulkan on Windows and presents through a DirectComposition visual with a
    premultiplied-alpha swapchain, the clear and the background pass write the
    canvas alpha (the background pass keeps the clear's alpha), and the later passes
    already blend "over". Checked live on the reference laptop: DX12 composites per
    pixel; Vulkan's Win32 surface offers opaque alpha only, so with
    `RIZREF_GPU_BACKEND=vulkan` the setting falls back to whole-window opacity, with
    a toast saying so. DX12 startup matches Vulkan (first frame 167 ms, GPU frame
    ~1.1 s); presentation is pinned to vsync on both. Ghost mode is no longer in the
    Transparency submenu; it keeps its own Canvas entry, Ctrl+Shift+G and its
    whole-window 15 % (it is for tracing over another application, where the images
    must fade too). The setting is stored as `canvas_opacity_percent`; an older
    `opacity_percent` is read into it, the same value now meaning the canvas.
  - The colour picker: no path was found by which it drops the next key or the
    dirty mark (it changes neither selection nor history, and the dirty mark comes
    from the board's effects); not reproduced, left unchanged.
- Fix: the panic hook exited the process for every panic, so the `catch_unwind` guards
  around decoders and math never ran — one hostile image closed RizRef.
  `contain::contain` marks the boundary and the hook now lets it handle the panic.
- Files: a read-only target is refused before anything is written
  (`FormatError::Denied`), the final rename retries six times over ~1.2 s while another
  program briefly holds the board, and a refused save offers Save As.
- **Help → Open logs folder**; `-v`/`--verbose` and daily rotation already existed.

Packaging and docs:

- `packaging/package.ps1`: release build, import checks, optional signing (only when
  `RIZREF_SIGN_PFX` is set), portable zip `RizRef-<v>-x64-portable.zip` (exe, DLL,
  README.txt, licences), and the installer when Inno Setup is present.
  `packaging/rizref.iss`: per-user install, runs `rizref register`/`unregister`.
- `.github/workflows/release.yml`: on a `v*` tag matching the workspace version, test,
  package, sign if the secrets exist, and create a **draft** release with notes from
  conventional commits (`scripts/release-notes.sh`). It never publishes.
- `docs/demo.rizref` (`examples/make_demo_board.rs`): four images drawn in code (CC0)
  and three markdown notes. README: status, layout, measured budgets, install and
  integration, releasing, licence. `LICENSE-MIT`, `LICENSE-APACHE`.
- Tests: 712 workspace tests pass (3 ignored), up from 630; clippy `-D warnings` and fmt
  clean; no new dependency crate and no new `-sys` crate (audit: `windows-sys`,
  `renderdoc-sys`).
- Not verified here, by design or by environment: `register`/`unregister` and Explorer
  thumbnails on Win10 and Win11 (they change the user's file associations — part of
  the manual test plan; the registry plan, the COM factory and the DLL's rendering are
  unit-tested); the installer (no Inno Setup on this machine; the script is untested);
  a clean Win10 22H2 VM run (substituted by the import-table check); the release
  workflow (runs on the first tag).
- Deferred: code signing (no certificate; the hook is in place); README screenshots
  (best captured during the manual test run); undo history across a crash (the format
  stores no history, so recovery restores the board, not its undo tail); a CPU canvas
  for images when no GPU exists at all (the CPU frames draw the shell only); the stretch
  items (headless CLI scripting, link-vs-embed UI, slideshow/drawings/comments).

## Phase 06b — Image provenance (done)

Merged 2026-11-09 in PR #22 (branch `feat/phase-06b-image-provenance`, merge commit
`ede7409`). Closes issue #20, which completes phase 06 (06, 06a and 06b). On-disk
contract: [`format.md` → scene document](format.md) (`origin`, `source_path`,
`source_url`; revision 1.2, no new members, `schema_version` stays 1).

- `rizref`: `provenance.rs` — pure, tested capture helpers. `normalize_url` accepts only
  `http`/`https` with a host, lowercases the scheme, keeps percent-encoding, and refuses
  control characters, whitespace, backslashes and anything over 4,096 bytes.
  `resolve_url` joins absolute, scheme-relative, root-relative, query/fragment-only and
  dot-segment references. `parse_cf_html` reads the `CF_HTML` header with checked
  offsets and a 4 MiB cap (a malformed header yields no URL); a tolerant `<img src>` /
  `<base href>` scan decodes `&amp;`-style and numeric entities. `text/uri-list`,
  `text/x-moz-url` and `UniformResourceLocatorW/A` payloads are decoded with size caps.
- `rizref`: paste reads the raw `HTML Format` block (`platform::clipboard_html`, since
  arboard returns only the fragment) and attaches the `<img src>` (else the page
  `SourceURL`) to a pasted bitmap. HTML without pixels toasts "The clipboard holds a
  link, not an image"; nothing is downloaded. Files still win over a bitmap.
- `rizref`: the OLE drop target reads `UniformResourceLocatorW/A`, `text/uri-list`,
  `text/x-moz-url` and `HTML Format`/`text/html`, preferring the image's own `src`,
  and adds `DropEvent::source_url` only for a single-image drop. A registered `PNG`
  payload without a file or virtual file imports as a virtual file named from the URL
  (or `dropped.png`). A URL-only drag still shows "no drop".
- `rizref`: `ImportSource::with_source_url` / `ImportedImage::source_url` thread the URL
  through the workers. `apply_import` records it through `provenance::record_for` (a
  URL suppresses a browser cache path; `origin` is `file` only for a path without a
  URL, `clipboard` for URL-backed, virtual and bitmap imports) and `provenance::merge`
  (keep an existing path/URL, fill missing fields, recompute `origin`).
- `rizref`: `location.rs` plans *Open location* without launching — a launchable URL
  wins, else the path (relative paths resolve against the board's folder), a moved file
  opens its parent folder with a toast, a vanished one is reported.
  `platform::launch` is the security boundary: the URL is re-validated and opened with
  `ShellExecuteW("open")` (≤ 32 is a failure), and Explorer gets one raw argument,
  `/select,"<path>"`, with quotes and control characters refused; no shell is involved.
- `rizref`: the Image menu (title-bar ☰ menu and canvas context menu) leads with
  **Image info…** and a dynamic **Open file location** / **Open image URL**, enabled only
  for exactly one image (and a launchable location); they also lead the right-click menu
  when one image is selected. No new default chord. Opening never touches the board,
  dirty flag or undo history.
- `rizref`: `ui/image_info.rs` — a read-only modal (~640 × 430, clamped to the
  viewport) with the stored preview letterboxed via the new `Painter::image`, and rows
  for location, source, format, pixels, board size, scale, file size (human and exact),
  BLAKE3 asset id, animation frames and loop time, preview size, rotation/mirroring,
  opacity, z-order and group. Long values are middle-elided; Open, Copy path/URL and
  Copy asset ID buttons toast on success; `Esc`, Close or a click outside close it, and
  pointer, wheel and keys never reach the board behind it. AccessKit publishes a modal
  `Dialog` with full-value rows and named buttons; Tab cycles without a trap.
- `rizref-format`: no code change. New tests cover provenance through fresh and
  appended saves (local path, web URL, a hostile `javascript:` URL kept verbatim, and an
  asset without a record) and a v1 asset entry without provenance members loading and
  re-saving without them.
- Fixes found by hand testing: a left-button release always resolved to `EndPan` and
  never reached `Board::pointer_up`, so a clicked item stayed stuck to the cursor and
  could not be deselected (a phase 04 bug; the release now ends the pan and the board
  gesture). Explorer `/select` with an 8.3 short path (`USERNA~1`) opened the folder
  without selecting the file; paths are expanded with `GetLongPathNameW` first. The
  info dialog's keyboard hint overlapped the buttons in a small window; it now shortens.
  **Show grid** and **Scene border** were toggled in the menu but never reached the
  renderer, so the grid, its origin axis and the border always drew; `FrameFlags` now
  carries both.
- `rizref`: the grid and origin axis are hidden on first launch (`grid_visible`
  defaults to `false`); View → Grid → Show grid turns them on, and a saved choice still
  wins.
- Tests: 630 workspace tests pass (3 ignored), up from 571; clippy `-D warnings` and fmt
  clean; no dependency changes and no new `-sys` crate.
- Hand-tested with Edge on Windows 10: copy image → `Ctrl+V` records the `<img src>`
  URL (origin `clipboard`); dragging the image delivers a virtual `images.jpg` and
  records the same URL; Image info shows every field, Copy URL copies the full URL,
  Open URL opens it in the browser; Open file location selects a file whose folder and
  name contain spaces, commas and `&`; click, drag-move and click-to-deselect work.
- Deferred: the rest of the browser matrix (Chrome, Firefox, a PNG-only payload, a
  multi-file drop, the HTML-only clipboard toast) was not run by hand; those paths are
  covered by the parser and planner unit tests only. In the default 600 × 400 window
  the info dialog clips its last row (Group) rather than growing. `CF_DIBV5`-only drags
  stay unsupported, as planned. An OS drop while the info dialog is open still imports
  behind it, as it does behind the close guard.

## Phase 06a — Markdown notes (done)

Merged 2026-11-09 in PR #21 (branch `feat/phase-06a-markdown-notes`, merge commit
`0901881`). Closes issue #19. On-disk contract:
[`format.md` → Note payloads](format.md#note-payloads-v11--phase-06a).

- `rizref-core`: `NoteFormat` (`plain` | `markdown`) and `NoteContent` (text, format,
  optional opaque background, optional wrap width). `ItemBody::Note` carries all four;
  absent fields deserialize as plain/default so old payloads keep their meaning.
  `SetNoteContent` replaces `SetNoteText`, so text, format, background and wrap width
  undo together; it is one step per commit (coalescing is opt-in). `Item::note_with`
  builds a note from parsed content; `Item::set_note` reports changes; wrap widths are
  clamped to 16–10,000 world units.
- `rizref-format`: `ItemEntry` gains `text_format`, `background` and `wrap_width`
  (skipped when absent; plain notes omit `text_format`). An invalid background, unknown
  format or bad wrap width is ignored with a load warning (`LoadedBoard::warnings`),
  never an error. `schema_version` stays 1; unknown members still round-trip.
  Thumbnails and PNG export paint note cards in their stored background with a
  contrast-derived border. `rizref inspect` prints `notes: N (markdown M, styled S)`.
- `rizref`: `markdown/` — a math pre-pass (`$…$`, `\(…\)`, `$$…$$`, `\[…\]`, skipping
  code, `\$` escapes, 4 KiB / 128-equation limits), a pulldown-cmark fold into a small
  block/inline model (headings, emphasis, strike, inline/fenced/indented code, ordered/
  unordered/task lists, quotes, rules, links, GFM tables, image alt text), and
  `<span style>` with only `color` (csscolorparser) and `font-size` in px (6–200); all
  other HTML stays literal. The layout is in world units at the wrap width only, so a
  resize never moves a line break; glyphs are rasterised at the texture density from
  the same shaping. Inline math is an atom whose advance and line height reserve the
  equation; display math is centred in its own block. RaTeX 0.1.15 (embedded KaTeX
  fonts, transparent background) produces alpha masks tinted by the span colour; parse
  errors are cached and shown as underlined source.
- `rizref`: the note renderer (`note.rs`, texture id `rizref-note-v2`) paints the card in
  its background with WCAG auto-contrast ink (`#f2eee8` / `#1b1b1b`), links in the
  accent only when it clears 3:1, span colours never adjusted; plain notes keep the
  phase-04 path. Layouts and math masks are cached; superseded note textures are freed.
- `rizref`: edit mode edits the source with a screen-constant ✗/✓ pill (top-right);
  `Esc` cancels, `Ctrl+Enter`/✓/click-outside commit, `Enter` is a newline. New notes
  are provisional: create + first commit is one undo step, an empty commit or a cancel
  adds nothing, and cancelling on a saved board restores the clean title. The box stays
  resizable while editing without rewrapping.
- `rizref`: rendered-mode hover toolbar (background swatch · edit · delete) drawn in the
  overlay, hit-tested before board input, with tooltips; a background popover (6 × 2
  palette, validated `#rrggbb` field, "Pick from screen" through the GPU eyedropper;
  each applied colour is one undo step). A title-bar ☰ button opens the full menu with
  a left click; a new **Note** menu holds New note (moved from Image) plus Edit note,
  Background colour…, Reflow to box width, Fit box to content and Delete note, which
  also lead the right-click menu when a note is selected.
- `rizref`: notes are never rotated — the rotate handle is hidden for note-only
  selections and a mixed selection rotates only its images; legacy rotations load
  unchanged. AccessKit exposes the ☰ button, the note toolbar/pill buttons and the
  edited source (read-only).
- Tests: format round trip with and without the new members, v1-reader simulation,
  invalid-background tolerance, wrap-width clamping, parser/span/math unit tests with a
  generated malformed-input corpus, layout structure (wrapping, 24 px line height, math
  baseline and centring, resize invariance), sampled pixel colours for the two span
  examples and auto-contrast, board undo semantics, and a `markdown` fuzz target.
  571 workspace tests pass (3 ignored); clippy and fmt clean; no new `-sys` crate.
  Measured (release, dev machine): a 4,000-character note lays out in ~116 ms cold and
  re-renders in ~35 ms with its layout cached; a resize that keeps the wrap width costs
  ~9 µs of layout; 20 equations render in ~21 ms cold.
- Deferred: math rasterises on the UI thread rather than a worker (20 equations stay
  within the 40 ms budget); the warm 4,000-character re-render misses the 8 ms budget
  (~35 ms) and the 64 MiB note-texture LRU with off-screen eviction is not implemented
  (one texture per note is kept instead) — both go to phase 07's performance pass; an
  AccessKit set-value request on the note source is not applied yet.

## Phase 06 — The `.rizref` file format (done)

Merged 2026-10-29 in PR #18 (branch `feat/phase-06-rizref-format`, merge commit
`9b44501`). Closes issue #7. Format specification: [`format.md`](format.md).

- `rizref-format`: the complete v1 container. A fixed 64-byte header carries a UUID v4
  scene id, revision, writer version and creation/modification times. Chunks use a
  24-byte frame (4-byte tag, compression code, stored and decompressed lengths):
  `NAME`, `THMB`, `DOC `, `BLOB`, `PREV`, `INDX` and `END!`. A fixed 80-byte footer at
  the end of a clean file points at the last index and carries its digest; the index
  lists every live chunk by offset with a BLAKE3 digest and stores its own digest in
  front of its JSON.
- `rizref-format`: content-addressed originals. Every `BLOB` is the 32-byte BLAKE3 of
  the original bytes followed by those bytes verbatim; identical images are one asset
  and one chunk. `PREV` stores a small JPEG/PNG preview per asset so a board shows
  pictures before any original is decoded.
- `rizref-format`: the scene document is format-owned JSON mirroring `rizref-core`
  (camera, scene rect, items in z-order, groups, assets, board name). Unknown members
  are preserved at every level on resave; `schema_version` refuses newer revisions;
  `assets[].embedded = false` declares a link-only asset and the reader reports it as
  missing instead of failing. Provenance paths/URLs are hints only.
- `rizref-format`: fresh saves write `<file>.rizref.tmp` and rename atomically.
  Incremental saves append only changed chunks plus a new index and footer; unchanged
  blobs are never copied. Compaction (full rewrite) triggers when the file would grow beyond
  three times its live bytes (dead space over twice live), on Save As, and whenever the existing file cannot be appended
  to (recovered, foreign or another scene).
- `rizref-format`: recovery scans the chunk stream when no valid footer exists and
  keeps the last valid index; a partially written index fails its own digest, so the
  previous generation loads. All chunk payloads are read lazily by offset.
- `rizref-format`: board thumbnails are composited from item previews with tiny-skia
  (bounding box, ≤ 512 px, JPEG when opaque and PNG with alpha); export reuses the
  same compositor for a PNG.
- `rizref`: `rizref inspect <file> [--verify]` — header, document summary, chunk table
  and digest/blob verification without opening a window. Exit codes 0/1/2.
- `rizref`: Save, Save As, Export (PNG, composited from the stored previews), Open board, Open recent and
  dropped `.rizref` files are wired; New/Open/Quit share the unsaved-changes guard. A successful save
  clears the dirty marker, updates the title and records the board (path, item count,
  thumbnail) in recents. Loading restores the camera, re-uploads previews immediately
  and streams full-resolution textures from the worker pool as items become visible.
- `rizref`: removed the phase-00 `dev_state` camera checkpoint and its temporary
  checksummed header; `SceneId::random` produces real UUID v4 ids. Asset provenance
  (origin, source path) is recorded on import and restored on open.
- Tests: 200-item mixed-codec round trip, incremental-vs-compaction growth, a
  1000-point crash matrix, fresh-save truncation matrix, digest verification and
  deliberate corruption. 499 workspace tests pass; clippy and fmt clean. A 500 MB
  board's name/thumbnail read measured 334 µs (best of 20, warm).

## Phase 05 — UI Shell (done)

Merged 2026-10-29 in PR #17 (branch `feat/phase-05-ui-shell`, merge commit
`0335105`). Closes issue #6.

- `rizref`: custom-painted UI shell instead of native menus. `ui/menu.rs` is a
  data-driven `Action`/`MenuEntry` tree (File, Edit, Image, View, Canvas, Tools, Help)
  with submenus, check marks, disabled state, accelerators read from the live shortcut
  table, mouse and full keyboard navigation, and dismissal on Escape or focus loss.
  Right-click opens it at the cursor after selecting the item underneath.
- `rizref`: `ui/paint.rs` (`Painter` over tiny-skia, scale-aware in logical pixels) and
  `ui/mod.rs` (`Shell`, toasts, mode indicators). The shell rasterises into one cached
  viewport-sized RGBA layer; a new `vs_ui`/`fs_ui` pass in `canvas.wgsl` uploads and
  draws it as a single quad only when the revision changes.
- `rizref`: start view (`ui/start.rs`) with wordmark, drop zone, New board / Open
  images… / Open board… buttons and recent-board cards; a responsive compact layout
  for windows under 560 px tall. `recents.rs` stores path, edited-at, item count and
  optional preview in `%APPDATA%\RizRef\recents.json`, dedupes case-insensitively,
  prunes missing files and caps at 12 entries. Opening a board and recording recents
  wait for the `.rizref` reader in phase 06.
- `rizref`: shortcut system (`shortcuts.rs`): 65 actions, 64 bound by default, exact
  modifier matching, aliases for redo, `ctrl+shift+U` for ungroup, conflicts rejected
  at bind time, and persisted overrides keyed kebab-case. `panels.rs` renders the F1
  overlay with search, selection, scrolling, Enter-to-rebind, Delete-to-reset and an
  inline conflict message, plus the pending chord prompt ("Press a chord…").
- `rizref`: `settings.rs` + settings dialog: decode cap, grid/snap/HUD/border/
  checkerboard/grayscale, canvas theme (Charcoal/Slate/Black/Paper), always-on-top,
  opacity preset and ghost opacity. `sanitize()` guards hand-edited values; saves are
  atomic; a malformed shortcut override is skipped without discarding the rest of the
  file. `paths.rs` centralises `%APPDATA%\RizRef`.
- `rizref`: `platform/window_modes.rs` composes always-on-top (`SetWindowPos`),
  uniform opacity (`WS_EX_LAYERED` + `SetLayeredWindowAttributes`), click-through
  (`WS_EX_TRANSPARENT`), borderless fullscreen and ghost mode. The pure
  `styled_ex_style` plus a state matrix test pin the `WS_EX_*` juggling, including
  ghost mode restoring the exact pre-ghost opacity/click-through state.
- `rizref`: **ghost mode** (`ctrl+shift+G`, requested after the phase review): 15 %
  window alpha (configurable 10–40 %) with whole-window click-through. Because the
  window can no longer receive input, the chord is polled through
  `GetAsyncKeyState` every 50 ms while ghost/click-through is active; the loop returns
  to `ControlFlow::Wait` otherwise. An accent border and a "GHOST MODE · Ctrl+Shift+G
  to restore" pill make the state legible. Ghost opacity is session-only, never
  persisted.
- `rizref`: **custom auto-hiding title bar** (requested after the phase was closed):
  the window is created frameless (`.with_decorations(false)`) and `ui/titlebar.rs`
  paints its own 34 px bar into the cached shell layer. The bar stays hidden until the
  pointer enters the title-bar strip; it hides again on leave, focus loss, picker arm,
  ghost/click-through activation and borderless fullscreen. Controls are the always-on-top
  pin (accent state, persisted through `ToggleAlwaysOnTop`), minimize, maximize/
  restore and close, with Windows-style hover feedback (red close). Dragging the empty
  bar calls `Window::drag_window`, double-click toggles maximize, and the left, right,
  bottom and bottom-corner edges get a 5 px `drag_resize_window` band with matching
  resize cursors. `Painter::line` draws the crisp vector glyphs.
- `rizref`: **document title and unsaved close guard** (requested after the phase was
  closed): `document.rs` carries the open document's file name and dirty flag. New
  boards are `DocumentN.rizref`; the title bar's left side shows only that name (no
  application icon) and inserts `*` before the extension while dirty —
  `Document1*.rizref`. The start view stays blank, and the display title is mirrored
  to the native window title/taskbar and the UIA root. Every board effect marks the
  document dirty; imports from the start view create the document when queued. Close
  requests from the title-bar X, `ctrl+Q`, the menu's Quit and the native
  `WM_CLOSE`/Alt+F4 all call one guarded path: a dirty document opens a modal
  "Unsaved changes" panel with Cancel (default) and Close without saving. Escape
  cancels, arrows/Tab switch, Enter confirms; UIA publishes the dialog and both
  buttons and moves focus to it. The Save choice arrives with the phase-06 writer.
- `rizref`: color picker (`I`): the surface gains `COPY_SRC`; renderer copies the 1×1
  pixel under the cursor with a 256-byte row pitch and returns RGBA after a blocking
  poll. The pick frame hides HUD and shell so the crosshair cannot be sampled; the
  toast shows `#RRGGBB` plus RGB/HSV and a click copies the hex.
- `rizref`: canvas lock (`L`) blocks item mutations while navigation still works, with
  a `LOCKED` HUD badge; HUD status also reports `GHOST` and `CLICK-THROUGH`.
- `rizref`: framebuffer effects — checkerboard backdrop and grayscale item shading in
  `canvas.wgsl`; clear colors are now converted sRGB→linear (fixes the washed-out
  phase-02 background), and grid minor/major lines fade in over zoom instead of
  saturating every pixel.
- `rizref`: accessibility (`accessibility.rs`) via AccessKit: Window → a "Window
  controls" group (always-on-top switch, minimize, maximize/restore, close) → MenuBar
  → menu/submenu leaves with names, accelerators, checkable toggle state, disabled
  state and click actions routed back into `Action`. While the close guard is open the
  tree also carries the "Unsaved changes" dialog with its Cancel and Close without
  saving buttons, and focus moves to the dialog. Verified with a UIAutomation client
  (same tree Inspect.exe shows), unlike PureRef's empty tree.
- Deferred: glyph-atlas text pipeline (shell uses one cached layer), tray icon,
  dragging the window by empty canvas background, opacity slider (presets are used),
  board save/open and recents recording (phase 06). Replacing a dirty board through
  New/Open will reuse the close guard once phase 06 owns the document lifecycle.
- Verification: 462 workspace tests, clippy and rustfmt clean. Live checks covered
  the start view, context menu and submenu navigation, F1 rebinding persistence,
  settings dialog, ghost mode style/alpha flip and global-chord restore, color picker
  values against screenshots, and the UIA tree. The title bar was checked live:
  hidden/revealed screenshots, pin flipping `WS_EX_TOPMOST` and persisting, drag and
  edge resize changing `GetWindowRect`, double-click maximize/restore, ghost mode
  keeping it hidden, close exiting, and UIA exposing all four controls. The document
  feature was checked live: blank start-view title, `Document1*.rizref` in the custom
  bar and native title after New board, the unsaved dialog from the title-bar X and
  from `WM_CLOSE`, Cancel and Escape keeping the app open, Close without saving
  exiting, and UIA listing the dialog and both buttons.

## Phase 04 — Items & Manipulation (done)

Merged 2026-10-29 in PR #16 (branch `feat/phase-04-items-and-manipulation`,
merge commit `f1c5724`). Closes issue #5.

- `rizref-core`: `Transform` (centre/size/rotation, oriented hit-test, exact
  separating-axis rectangle test), a unified `Item` with `ItemBody::Image`/`Note` and
  UV-mirror flips, `Groups` (exclusive parentage, recursive with a depth guard),
  `Snapping` with guides and screen-pixel tolerance, and pure `arrange`/`align`/
  `distribute`/`match_size` functions.
- `rizref-core`: one command stack for every mutation — `SetTransforms`, `AddItems`,
  `RemoveItems`, `ReorderItems`, `SetNoteText`, `AddGroup`, `DissolveGroup` — each
  returning `Effects` (geometry/content/added/removed/order/groups). History depth
  200; key-repeat nudges coalesce inside 100 ms, gestures stay discrete. Undo-all
  reproduces the serialised scene byte-for-byte (session high-water marks are not
  serialised).
- `rizref`: new `Board` controller — click/shift-click selection with group expansion,
  marquee (exact against rotated items), move, eight-handle scale (centre pivot,
  `alt` = opposite corner, `shift` = aspect), rotate (15° steps), flips, nudges,
  duplicate/delete, z-order, grouping, arrange/align/distribute, note creation and
  text commits. The scripted 20-item acceptance test drives it through synthetic
  pointer events and checks exact geometry through undo and redo.
- `rizref`: screen-space overlay pipeline (a fourth pass in `canvas.wgsl`) for item
  outlines, hover, the selection frame, handles, rotate handle, marquee and smart
  guides. Handles stay 9 px and guides 1 px at 5% and 3200% zoom.
- `rizref`: notes — cosmic-text `Editor` sessions (caret, arrows, delete/backspace,
  Enter, Ctrl+A, text paste), rounded-card rasterisation through tiny-skia, and
  content-addressed textures so identical notes share one GPU texture. `N` creates a
  note; double-click edits; Escape or clicking outside commits one undo step.
- `rizref`: HUD readout now includes the selection count and bounding size; the cursor
  switches between move/grab/resize/rotate/text/crosshair; the input table gained the
  item shortcuts (`ctrl+A/D/G/Z/Y`, `del`, arrows, `pgup/pgdn/home/end`, `shift+H/V`,
  `ctrl+shift+W/H/R`, `3/4/5/6`, `ctrl+shift+arrows`, `ctrl+shift+,/.`).
- Verification: 393 workspace tests, clippy and rustfmt clean; the offscreen GPU test
  renders the overlay pipeline; an 8-second smoke launch creates the window and
  Vulkan surface without a validation error.

## Phase 03 — Image Pipeline (done)

Merged 2026-10-29 in PR #14 (decode, drop, paste, cache) and PR #15 (HEIC in Rust,
AVIF through the OS codec, animated GIFs). Closes issues #4 and #13.

- `rizref-codecs`: `DecodeLimits` (100 MP default) enforced from headers before
  allocation; EXIF orientation baked in for JPEG and WebP; committed fixture suite
  (palette/16-bit PNG, baseline/progressive/grayscale JPEG, lossy+lossless WebP,
  animated GIF, BMP/TGA/PNM/QOI, multi-size ICO); malformed-input sweep; `fuzz/`
  crate with one target per codec family, a per-target CI matrix on nightly and a
  scheduled ten-minute soak per target. The `decode_ico` soak found a real panic in
  `ico` 0.6 (indexed pixels outside the declared palette); ICO entries are now
  validated — real dimensions for the megapixel cap plus a palette-index walk —
  before the crate runs, with the 121-byte reproducer committed as a fixture.
- `rizref-core`: content-addressed `Asset`/`AssetStore` (BLAKE3 of the original
  bytes), `ImageItem` with oriented hit-testing, `Scene` now holds items and assets,
  box-filter previews that never upscale, and a 512 px preview per asset.
- `rizref`: `crossbeam-channel` worker pool (`import.rs`) with trace spans and panic
  isolation; 2 GB LRU decoded-bitmap cache; drop-point ring packing; clipboard paste
  (files or bitmap) and Ctrl+O image dialog.
- `rizref`: OLE `IDropTarget` (`platform/drop_target.rs`) replaces winit's handler so
  drag & drop also accepts **virtual files** (`FileGroupDescriptorW` + `FileContents`:
  Outlook attachments, entries of a compressed folder), not just `CF_HDROP` paths.
  Registration degrades to winit's handler when OLE cannot initialise; descriptor
  parsing is unit-tested against the packed Win32 layout.
- `rizref-codecs`: `decode_gif_animation` returns every GIF frame already composed onto
  a canvas — offsets, transparency and `Background` disposal applied — under frame and
  pixel budgets, with a 20 ms floor on zero-delay frames.
- `rizref-core`: `Animation`/`AnimationFrame` answer "which frame is due at time t?"
  (looping, no clock of its own), and `Asset::with_animation` keeps the first frame as the
  still image so previews, export and the file format are unchanged.
- `rizref`: animated GIFs play on a one-shot `ControlFlow::WaitUntil` timer, re-uploading
  only the current frame; **a selected item freezes** and the HUD reports the frame. A
  plain left click now selects the topmost item (the seed phase 04 builds on).
- `rizref`: **HEIC/HEIF** is bridged to Windows Imaging Component
  (`platform/imaging.rs`), which reuses the HEIF Image Extension instead of bundling an
  HEVC decoder; a missing extension is reported as a package to install. The bridge is
  covered by a test that drives it with a PNG, so CI does not need the extension.
- `rizref-codecs`: **HEIC/HEIF is now decoded in Rust** by `heic-rs` (container, HEVC
  still picture, grid tiles, 10-bit, `irot`/`imir`/`clap`), under the same pixel cap as
  every other format. Two fixtures from its corpus (`still.heic`, `grid.heic`) prove the
  single-item and tiled paths, and a dedicated `decode_heif` fuzz target guards the young
  decoder. The OS bridge stays as the fallback for coding tools it declines.
- `rizref-codecs`: `ImageFormat::Avif` is now separate from `Heif` (the `ftypavif`/`avis`
  brands), so an AVIF is named as one rather than as a generic HEIF file.
- `rizref`: AVIF is decoded through the operating system codec, because the pure-Rust
  option (`avif-decode` → `rav1d` with `asm`) cannot build without NASM; a missing AV1
  Video Extension is reported by name.
- Renderer: placeholder quads deleted; items draw from real per-item
  `RGBA8UnormSrgb` textures with a 512 MB LRU budget, 4096 px display cap,
  preview-first fallback and re-upload when visible again.

## Phase 02 — Canvas & Viewport (done)

- `rizref-core`: f64 `Camera` with rotation, cursor-anchored zoom, fit/frame and
  `visible_world_rect`; incremental `SceneRectTracker`.
- `rizref`: declarative trigger→command input table; `ViewState` gestures and 80 ms
  smoothstep zoom animation; three canvas pipelines (background grid/origin/scene border,
  instanced items over an 8-layer texture array, screen-space HUD); cosmic-text +
  tiny-skia HUD readout; dev-only view persistence.

## Phase 01 — Foundations (done)

- winit window, wgpu surface (Vulkan → DX12), on-demand frame pacing (0 ms idle).
- Single-instance guard with argument forwarding; rolling file logs; panic hook.

## Phase 00 — Research & Decisions (done)

- Cargo workspace with the pure-Rust dependency policy, lints, CI and frame budgets.
- REA study of PureRef 2.1.3 (binary, `.pur` format, runtime UI) as the clean-room basis.
