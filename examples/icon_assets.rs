//! Regenerates the icon files from `packaging/icons/lwspot.svg`:
//! `packaging/windows/lwspot.ico` (the exe and installer icon) and
//! `docs/assets/images/logo.svg`. Optionally writes PNG previews.
//!
//! cargo run --example icon_assets [-- --previews <dir>]

use std::path::PathBuf;

/// The sizes Windows asks an .ico for, from the taskbar's 16 to Explorer's 256.
const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let svg = std::fs::read_to_string(root.join("packaging/icons/lwspot.svg")).expect("icon svg");

    let mut args = std::env::args().skip(1);
    let mut previews = None;
    while let Some(arg) = args.next() {
        if arg == "--previews" {
            previews = args.next().map(PathBuf::from);
        }
    }

    let mut images = Vec::new();
    for size in ICO_SIZES {
        let rgba = lwspot::util::app_icon_rgba(size as usize);
        let png = encode_png(size, &rgba);
        if let Some(dir) = &previews {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("lwspot-{size}.png")), &png).unwrap();
        }
        images.push((size, png));
    }
    std::fs::write(root.join("packaging/windows/lwspot.ico"), ico(&images)).unwrap();
    std::fs::write(root.join("docs/assets/images/logo.svg"), &svg).unwrap();
    println!("wrote packaging/windows/lwspot.ico and docs/assets/images/logo.svg");
}

fn encode_png(size: u32, rgba: &[u8]) -> Vec<u8> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, size, size, image::ExtendedColorType::Rgba8)
        .unwrap();
    png
}

/// An .ico whose entries are PNG images, which every Windows since Vista reads.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in images {
        // 256 is written as 0.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}
