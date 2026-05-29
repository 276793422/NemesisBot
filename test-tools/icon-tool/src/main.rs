use std::fs;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");

    // Read SVG source
    let svg_path = root.join("nemesisbot/recourse/nemesisbot.svg");
    println!("Reading SVG: {}", svg_path.display());
    let svg_data = fs::read(&svg_path).expect("Failed to read SVG");

    // Parse SVG
    let tree = resvg::usvg::Tree::from_data(&svg_data, &resvg::usvg::Options::default())
        .expect("Failed to parse SVG");

    let svg_size = tree.size();
    println!(
        "SVG size: {:.0}x{:.0}",
        svg_size.width(),
        svg_size.height()
    );

    // Render at all required sizes
    let sizes = [16u32, 32, 48, 256];
    let mut png_bytes: Vec<(u32, Vec<u8>)> = Vec::new();

    for &size in &sizes {
        let pixmap = render_svg(&tree, svg_size, size);
        let png = pixmap_to_png(&pixmap);
        png_bytes.push((size, png));
        println!("  {}x{} rendered", size, size);
    }

    // Save individual PNG files
    let outputs = [
        (32u32, "web/public/favicon.png"),
        (256u32, "crates/nemesis-desktop/icons/icon.png"),
        (256u32, "plugins/plugin-ui/icons/icon.png"),
    ];

    for &(size, rel_path) in &outputs {
        let path = root.join(rel_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let data = png_bytes.iter().find(|(s, _)| *s == size).unwrap();
        fs::write(&path, &data.1).unwrap();
        println!("Saved: {}", rel_path);
    }

    // Build and save multi-size ICO
    let ico_data = build_ico(&png_bytes);
    let ico_path = root.join("nemesisbot/recourse/nemesisbot_multi.ico");
    fs::write(&ico_path, &ico_data).unwrap();
    println!(
        "Saved: nemesisbot/recourse/nemesisbot_multi.ico ({} entries)",
        sizes.len()
    );

    println!("\nAll icons generated!");
}

fn render_svg(
    tree: &resvg::usvg::Tree,
    svg_size: resvg::usvg::Size,
    target: u32,
) -> resvg::tiny_skia::Pixmap {
    let sx = target as f32 / svg_size.width();
    let sy = target as f32 / svg_size.height();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(target, target).unwrap();
    let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap
}

fn pixmap_to_png(pixmap: &resvg::tiny_skia::Pixmap) -> Vec<u8> {
    let mut raw = pixmap.data().to_vec();
    unpremultiply(&mut raw);

    let img = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), raw).unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// Convert premultiplied RGBA to straight alpha.
fn unpremultiply(data: &mut [u8]) {
    for chunk in data.chunks_exact_mut(4) {
        let a = chunk[3] as f32 / 255.0;
        if a > 0.0 {
            chunk[0] = (chunk[0] as f32 / a).min(255.0) as u8;
            chunk[1] = (chunk[1] as f32 / a).min(255.0) as u8;
            chunk[2] = (chunk[2] as f32 / a).min(255.0) as u8;
        }
    }
}

/// Build ICO binary from PNG-encoded entries (Windows Vista+ format).
fn build_ico(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let n = entries.len() as u16;
    let header_size = 6 + 16 * entries.len();
    let mut out = Vec::new();

    // ICONDIR
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    out.extend_from_slice(&n.to_le_bytes());

    // DIRENTRY for each image
    let mut data_offset = header_size as u32;
    for (size, data) in entries {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim); // width (0 = 256)
        out.push(dim); // height
        out.push(0); // palette count
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data_offset.to_le_bytes());
        data_offset += data.len() as u32;
    }

    // Image data (PNG encoded)
    for (_, data) in entries {
        out.extend_from_slice(data);
    }

    out
}
