//! Writes what Explorer would show for a board, for reviewing the thumbnail
//! provider without registering it: the board at 256, 96 and 48 px, side by
//! side on a light background, then the document artwork a board with
//! nothing to picture gets.
//!
//! ```bash
//! cargo run -p rizref-thumb --example thumb_preview -- docs/demo.rizref out.png
//! ```

use rizref_codecs::ImageBuffer;

fn main() {
    let mut args = std::env::args().skip(1);
    let board = args.next().expect("usage: thumb_preview <board> <out.png>");
    let out = args.next().expect("usage: thumb_preview <board> <out.png>");

    let sizes = [256_u32, 96, 48];
    let mut tiles: Vec<rizref_thumb::Thumbnail> = sizes
        .iter()
        .map(|&size| {
            let file = std::fs::File::open(&board).expect("open the board");
            rizref_thumb::render_or_artwork(file, size)
        })
        .collect();
    let garbage = std::io::Cursor::new(Vec::new());
    tiles.push(rizref_thumb::render_or_artwork(garbage, 256));

    let pad = 16;
    let width: u32 = tiles.iter().map(|t| t.width + pad).sum::<u32>() + pad;
    let height = 256 + 2 * pad;
    let mut canvas = vec![240_u8; (width * height * 4) as usize];
    let mut x0 = pad;
    for tile in &tiles {
        let y0 = pad + (256 - tile.height) / 2;
        for y in 0..tile.height {
            for x in 0..tile.width {
                let s = ((y * tile.width + x) * 4) as usize;
                let d = (((y0 + y) * width + x0 + x) * 4) as usize;
                // Premultiplied BGRA over the light background.
                let a = u32::from(tile.bgra[s + 3]);
                for (c, from) in [(0, 2), (1, 1), (2, 0)] {
                    let under = u32::from(canvas[d + c]);
                    canvas[d + c] =
                        (u32::from(tile.bgra[s + from]) + under * (255 - a) / 255).min(255) as u8;
                }
                canvas[d + 3] = 255;
            }
        }
        x0 += tile.width + pad;
    }
    let sheet = ImageBuffer::from_rgba(width, height, canvas).expect("sized");
    std::fs::write(&out, rizref_codecs::encode_png(&sheet).expect("png")).expect("write");
    println!("wrote {out}");
}
