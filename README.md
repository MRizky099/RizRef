# RizRef

A fast, portable reference-image canvas for artists — collect, arrange and study
reference images on an infinite board, always available over your painting app.

RizRef is a from-scratch implementation written in **Rust** (pure Rust wherever a
reasonable crate exists), with its own scene file format, `.rizref`.

## Status

Early development. Progress is tracked on the issue tracker, one issue per delivery
phase:

```bash
gh issue list --repo MRizky099/RizRef --state open
```

## Design principles

- **Pure Rust first.** A dependency that vendors or links C/C++ is rejected unless no
  viable Rust alternative exists; OS FFI bindings (Win32/COM declarations) are fine.
- **One portable file per board.** A `.rizref` file is self-contained: the scene,
  original image bytes (verbatim, so metadata like EXIF/C2PA survives), and an embedded
  preview thumbnail for fast browsing.
- **Crash-safe by construction.** Saves are atomic; a board never loads corrupt.
- **Always-available window.** Always-on-top, translucent, and click-through modes are
  first-class, not hacks.

## Repository layout

```
crates/
  rizref/          the application binary (window, renderer, input, platform glue)
  rizref-core/     scene model, geometry, commands/undo — no IO, no UI
  rizref-format/   .rizref container reader/writer — single source of truth
  rizref-codecs/   image decode/encode dispatch (pure-Rust codec backends)
docs/
  ...              local working documents (not published to this remote)
```

## Building

```bash
cargo run -p rizref      # run the app (once the workspace lands)
cargo test --workspace   # run all tests
```

## License

Not yet decided; a license will be added before the first public release.
