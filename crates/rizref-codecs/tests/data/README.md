# Test fixtures

Most files here are written by `examples/make_fixtures.rs` from this workspace's
own encoders:

```bash
cargo run -p rizref-codecs --example make_fixtures
```

Two are not, because nothing in this workspace can encode HEVC:

| File | Origin | Licence |
|---|---|---|
| `still.heic` | [`heic-rs`](https://github.com/tbraun96/heic-rs) test corpus, `tests/fixtures/flat-64.heic` | MIT OR Apache-2.0 |
| `grid.heic` | same corpus, `checker-1024.heic` (a 2 × 2 grid of 512 px tiles) | MIT OR Apache-2.0 |

They were produced by the repository's own fixtures script from synthetic PNGs
(no third-party photograph), and they are small on purpose: 488 bytes and
2.5 KB. `still.heic` covers the single-item path, `grid.heic` the tiled path that
every iPhone photograph above 512 px uses.

`malformed-ico-palette.ico` and `malformed-ico-eight-bit.ico` are fuzz
reproducers from the `decode_ico` target; see the tests that use them.
