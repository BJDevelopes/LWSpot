//! Writes the macOS app icon's iconset from `packaging/icons/lwspot.svg`,
//! for `iconutil -c icns` (packaging/macos/build.sh runs both).
//!
//! cargo run --example macos_icon -- <folder>.iconset
//!
//! The SVG's violet square fills almost all of its canvas, as Windows icons
//! do. macOS icons leave a margin around the square (824 of 1024 points on
//! Apple's grid), so each size is drawn smaller and centred to sit beside
//! the other icons in the Dock.

use std::path::PathBuf;

/// The square's share of the canvas on Apple's icon grid.
const MAC_SQUARE: f32 = 824.0 / 1024.0;
/// The square's share of the canvas in the SVG (120 of 128).
const SVG_SQUARE: f32 = 120.0 / 128.0;

fn main() {
    let folder = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: macos_icon <folder>.iconset");
    std::fs::create_dir_all(&folder).unwrap();
    for points in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let size = points * scale;
            let name = if scale == 1 {
                format!("icon_{points}x{points}.png")
            } else {
                format!("icon_{points}x{points}@2x.png")
            };
            std::fs::write(folder.join(name), encode_png(size, &padded(size))).unwrap();
        }
    }
    println!("wrote {}", folder.display());
}

/// The icon drawn at `size`, shrunk onto Apple's grid and centred.
fn padded(size: u32) -> Vec<u8> {
    let inner = ((size as f32) * MAC_SQUARE / SVG_SQUARE).round() as u32;
    let drawn = lwspot::util::app_icon_rgba(inner as usize);
    let offset = (size - inner) / 2;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for row in 0..inner {
        let from = (row * inner * 4) as usize;
        let to = (((row + offset) * size + offset) * 4) as usize;
        let length = (inner * 4) as usize;
        out[to..to + length].copy_from_slice(&drawn[from..from + length]);
    }
    out
}

fn encode_png(size: u32, rgba: &[u8]) -> Vec<u8> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, size, size, image::ExtendedColorType::Rgba8)
        .unwrap();
    png
}
