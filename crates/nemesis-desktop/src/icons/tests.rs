use super::*;

#[test]
fn test_load_from_bytes_valid_and_invalid() {
    // Valid: 2x2 icon = 16 bytes of RGBA
    let data = vec![0u8; 16];
    let icon = Icon::load_from_bytes(data, 2, 2).unwrap();
    assert_eq!(icon.width, 2);
    assert_eq!(icon.height, 2);
    assert_eq!(icon.data.len(), 16);

    // Invalid: wrong length
    let bad = Icon::load_from_bytes(vec![0u8; 10], 2, 2);
    assert!(bad.is_none());
}

#[test]
fn test_solid_color_and_pixel_access() {
    let icon = Icon::solid_color(4, 4, 0xFF, 0x80, 0x00, 0xFF);
    assert_eq!(icon.width, 4);
    assert_eq!(icon.height, 4);
    assert_eq!(icon.data.len(), 64); // 4*4*4

    // Every pixel should be the same
    for y in 0..4 {
        for x in 0..4 {
            let (r, g, b, a) = icon.pixel(x, y).unwrap();
            assert_eq!(r, 0xFF);
            assert_eq!(g, 0x80);
            assert_eq!(b, 0x00);
            assert_eq!(a, 0xFF);
        }
    }

    // Out of bounds
    assert!(icon.pixel(4, 0).is_none());
    assert!(icon.pixel(0, 4).is_none());
}

#[test]
fn test_get_scaled_dimensions_and_pixel_preservation() {
    // 4x4 solid red icon, scale down to 2x2
    let icon = Icon::solid_color(4, 4, 0xFF, 0x00, 0x00, 0xFF);
    let scaled = icon.get_scaled(2, 2);
    assert_eq!(scaled.width, 2);
    assert_eq!(scaled.height, 2);
    assert_eq!(scaled.data.len(), 16); // 2*2*4

    // Solid color should be preserved after scaling
    let (r, g, b, a) = scaled.pixel(0, 0).unwrap();
    assert_eq!(r, 0xFF);
    assert_eq!(g, 0x00);
    assert_eq!(b, 0x00);
    assert_eq!(a, 0xFF);

    // Scale up: 2x2 -> 4x4
    let small = Icon::solid_color(2, 2, 0x00, 0xFF, 0x00, 0xCC);
    let large = small.get_scaled(4, 4);
    assert_eq!(large.width, 4);
    assert_eq!(large.height, 4);
    let (r, g, b, a) = large.pixel(1, 1).unwrap();
    assert_eq!(r, 0x00);
    assert_eq!(g, 0xFF);
    assert_eq!(b, 0x00);
    assert_eq!(a, 0xCC);
}

// ---- New tests ----

#[test]
fn test_icon_size_dimensions() {
    assert_eq!(IconSize::Small.dimensions(), (16, 16));
    assert_eq!(IconSize::Medium.dimensions(), (32, 32));
    assert_eq!(IconSize::Large.dimensions(), (64, 64));
}

#[test]
fn test_icon_size_equality() {
    assert_eq!(IconSize::Small, IconSize::Small);
    assert_ne!(IconSize::Small, IconSize::Medium);
}

#[test]
fn test_icon_size_serialization() {
    let size = IconSize::Medium;
    let json = serde_json::to_string(&size).unwrap();
    let parsed: IconSize = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, IconSize::Medium);
}

#[test]
fn test_icon_size_debug() {
    assert!(format!("{:?}", IconSize::Small).contains("Small"));
    assert!(format!("{:?}", IconSize::Medium).contains("Medium"));
    assert!(format!("{:?}", IconSize::Large).contains("Large"));
}

#[test]
fn test_solid_color_transparent() {
    let icon = Icon::solid_color(2, 2, 0, 0, 0, 0);
    let (r, _g, _b, a) = icon.pixel(0, 0).unwrap();
    assert_eq!(r, 0);
    assert_eq!(a, 0);
}

#[test]
fn test_solid_color_white_opaque() {
    let icon = Icon::solid_color(1, 1, 255, 255, 255, 255);
    let (r, g, b, a) = icon.pixel(0, 0).unwrap();
    assert_eq!(r, 255);
    assert_eq!(g, 255);
    assert_eq!(b, 255);
    assert_eq!(a, 255);
}

#[test]
fn test_load_from_bytes_zero_size() {
    let icon = Icon::load_from_bytes(vec![], 0, 0);
    assert!(icon.is_some());
    assert_eq!(icon.unwrap().data.len(), 0);
}

#[test]
fn test_load_from_bytes_1x1() {
    let data = vec![255, 0, 128, 200];
    let icon = Icon::load_from_bytes(data, 1, 1).unwrap();
    let (r, g, b, a) = icon.pixel(0, 0).unwrap();
    assert_eq!(r, 255);
    assert_eq!(g, 0);
    assert_eq!(b, 128);
    assert_eq!(a, 200);
}

#[test]
fn test_to_png_bytes() {
    let icon = Icon::solid_color(3, 3, 10, 20, 30, 40);
    let bytes = icon.to_png_bytes();
    assert_eq!(bytes.len(), 3 * 3 * 4);
}

#[test]
fn test_pixel_out_of_bounds() {
    let icon = Icon::solid_color(2, 2, 0, 0, 0, 255);
    assert!(icon.pixel(0, 0).is_some());
    assert!(icon.pixel(1, 1).is_some());
    assert!(icon.pixel(2, 0).is_none());
    assert!(icon.pixel(0, 2).is_none());
    assert!(icon.pixel(2, 2).is_none());
}

#[test]
fn test_icon_clone() {
    let icon = Icon::solid_color(2, 2, 100, 150, 200, 255);
    let cloned = icon.clone();
    assert_eq!(cloned.width, icon.width);
    assert_eq!(cloned.data.len(), icon.data.len());
}

#[test]
fn test_icon_debug() {
    let icon = Icon::solid_color(1, 1, 0, 0, 0, 0);
    let debug = format!("{:?}", icon);
    assert!(debug.contains("Icon"));
}

#[test]
fn test_icon_serialization_roundtrip() {
    let icon = Icon::solid_color(4, 4, 128, 64, 32, 200);
    let json = serde_json::to_string(&icon).unwrap();
    let parsed: Icon = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.width, 4);
    assert_eq!(parsed.height, 4);
    assert_eq!(parsed.data.len(), 64);
    let (r, g, b, a) = parsed.pixel(0, 0).unwrap();
    assert_eq!(r, 128);
    assert_eq!(g, 64);
    assert_eq!(b, 32);
    assert_eq!(a, 200);
}

#[test]
fn test_scale_same_size() {
    let icon = Icon::solid_color(4, 4, 50, 100, 150, 255);
    let same = icon.get_scaled(4, 4);
    assert_eq!(same.data.len(), icon.data.len());
    let (r, _, _, _) = same.pixel(0, 0).unwrap();
    assert_eq!(r, 50);
}

#[test]
fn test_scale_to_1x1() {
    let icon = Icon::solid_color(8, 8, 200, 100, 50, 255);
    let tiny = icon.get_scaled(1, 1);
    assert_eq!(tiny.width, 1);
    assert_eq!(tiny.height, 1);
    let (r, _, _, _) = tiny.pixel(0, 0).unwrap();
    assert_eq!(r, 200);
}

#[test]
fn test_scale_up_large() {
    let icon = Icon::solid_color(1, 1, 42, 84, 126, 168);
    let big = icon.get_scaled(16, 16);
    assert_eq!(big.width, 16);
    assert_eq!(big.height, 16);
    assert_eq!(big.data.len(), 16 * 16 * 4);
    let (r, g, b, a) = big.pixel(8, 8).unwrap();
    assert_eq!(r, 42);
    assert_eq!(g, 84);
    assert_eq!(b, 126);
    assert_eq!(a, 168);
}

// ============================================================
// Additional tests for ~92% coverage
// ============================================================

#[test]
fn test_icon_pixel_edge_cases() {
    // Create a 3x2 icon (6 pixels, 24 bytes)
    let icon = Icon::solid_color(3, 2, 10, 20, 30, 40);
    // Valid pixels
    assert!(icon.pixel(0, 0).is_some());
    assert!(icon.pixel(2, 1).is_some());
    // Out of bounds
    assert!(icon.pixel(3, 0).is_none());
    assert!(icon.pixel(0, 2).is_none());
}

#[test]
fn test_icon_get_scaled_uneven_dimensions() {
    // Scale a 3x3 icon to 7x5
    let icon = Icon::solid_color(3, 3, 100, 150, 200, 255);
    let scaled = icon.get_scaled(7, 5);
    assert_eq!(scaled.width, 7);
    assert_eq!(scaled.height, 5);
    assert_eq!(scaled.data.len(), 7 * 5 * 4);
}

#[test]
fn test_load_from_bytes_too_few() {
    let data = vec![0u8; 8]; // Need 16 for 2x2
    let icon = Icon::load_from_bytes(data, 2, 2);
    assert!(icon.is_none());
}

#[test]
fn test_load_from_bytes_too_many() {
    let data = vec![0u8; 32]; // Need 16 for 2x2
    let icon = Icon::load_from_bytes(data, 2, 2);
    assert!(icon.is_none());
}

#[test]
fn test_load_from_bytes_exact() {
    let data = vec![255u8; 16]; // Exactly right for 2x2
    let icon = Icon::load_from_bytes(data, 2, 2);
    assert!(icon.is_some());
}

#[test]
fn test_solid_color_pixel_values() {
    let icon = Icon::solid_color(2, 2, 1, 2, 3, 4);
    for y in 0..2 {
        for x in 0..2 {
            let (r, g, b, a) = icon.pixel(x, y).unwrap();
            assert_eq!(r, 1);
            assert_eq!(g, 2);
            assert_eq!(b, 3);
            assert_eq!(a, 4);
        }
    }
}

#[test]
fn test_icon_scale_preserves_color() {
    let icon = Icon::solid_color(8, 8, 200, 100, 50, 255);
    let scaled = icon.get_scaled(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            let (r, g, b, a) = scaled.pixel(x, y).unwrap();
            assert_eq!(r, 200);
            assert_eq!(g, 100);
            assert_eq!(b, 50);
            assert_eq!(a, 255);
        }
    }
}

#[test]
fn test_to_png_bytes_matches_data() {
    let icon = Icon::solid_color(2, 2, 10, 20, 30, 40);
    let bytes = icon.to_png_bytes();
    assert_eq!(bytes, icon.data);
}

#[test]
fn test_icon_size_all_variants() {
    let sizes = [IconSize::Small, IconSize::Medium, IconSize::Large];
    for size in &sizes {
        let (w, h) = size.dimensions();
        assert_eq!(w, h); // All icons are square
        assert!(w > 0);
    }
}

#[test]
fn test_icon_size_serde_roundtrip_all() {
    for size in [IconSize::Small, IconSize::Medium, IconSize::Large] {
        let json = serde_json::to_string(&size).unwrap();
        let parsed: IconSize = serde_json::from_str(&json).unwrap();
        assert_eq!(size, parsed);
    }
}

// ============================================================
// Tray icon tests
// ============================================================

#[test]
fn test_embedded_icon_png_is_valid() {
    let png_data = embedded_icon_png();
    // Should be non-empty
    assert!(!png_data.is_empty());
    // PNG magic bytes: 89 50 4E 47 0D 0A 1A 0A
    assert_eq!(png_data[0], 0x89);
    assert_eq!(&png_data[1..4], b"PNG");
}

#[test]
fn test_embedded_icon_decodable() {
    let png_data = embedded_icon_png();
    let img = image::load_from_memory(png_data);
    assert!(img.is_ok(), "embedded icon.png should be decodable");
    let img = img.unwrap();
    // Icon should be square and at least 16x16
    let (w, h) = (img.width(), img.height());
    assert_eq!(w, h, "icon should be square");
    assert!(w >= 16, "icon should be at least 16x16, got {}x{}", w, h);
}

#[test]
fn test_png_to_ico_header_format() {
    let png_data = embedded_icon_png();
    let ico_data = png_to_ico(png_data);
    // ICO header: reserved(2) + type(2) + count(2) = 6 bytes
    assert!(ico_data.len() > 22, "ICO should be at least header + entry + PNG data");
    // Reserved = 0x0000
    assert_eq!(&ico_data[0..2], &[0x00, 0x00]);
    // Type = 0x0001 (ICO)
    assert_eq!(&ico_data[2..4], &[0x01, 0x00]);
    // Count = 0x0001 (1 image)
    assert_eq!(&ico_data[4..6], &[0x01, 0x00]);
}

#[test]
fn test_png_to_ico_entry_dimensions() {
    let png_data = embedded_icon_png();
    let ico_data = png_to_ico(png_data);
    // Decode original image to get expected dimensions
    let img = image::load_from_memory(png_data).unwrap();
    let (w, h) = (img.width(), img.height());
    let expected_w = if w >= 256 { 0u8 } else { w as u8 };
    let expected_h = if h >= 256 { 0u8 } else { h as u8 };
    // Entry starts at offset 6
    assert_eq!(ico_data[6], expected_w, "width byte mismatch");
    assert_eq!(ico_data[7], expected_h, "height byte mismatch");
    // Color count: 0 (PNG)
    assert_eq!(ico_data[8], 0);
    // Color planes: 1
    assert_eq!(&ico_data[10..12], &[1, 0]);
    // Bits per pixel: 32
    assert_eq!(&ico_data[12..14], &[32, 0]);
}

#[test]
fn test_png_to_ico_embeds_original_png() {
    let png_data = embedded_icon_png();
    let ico_data = png_to_ico(png_data);
    // PNG data starts at offset 22 (6 header + 16 entry)
    let embedded = &ico_data[22..];
    assert_eq!(embedded, png_data, "embedded PNG data should match original");
}

#[test]
fn test_png_to_ico_size_field() {
    let png_data = embedded_icon_png();
    let ico_data = png_to_ico(png_data);
    // Size field at offset 14, 4 bytes LE
    let size = u32::from_le_bytes(
        ico_data[14..18].try_into().unwrap()
    );
    assert_eq!(size, png_data.len() as u32, "size field should match PNG data length");
}

#[test]
fn test_png_to_ico_offset_field() {
    let png_data = embedded_icon_png();
    let ico_data = png_to_ico(png_data);
    // Offset field at offset 18, 4 bytes LE = 22
    let offset = u32::from_le_bytes(
        ico_data[18..22].try_into().unwrap()
    );
    assert_eq!(offset, 22, "offset should be 22 (6 header + 16 entry)");
}

#[test]
fn test_png_to_ico_with_synthetic_small_png() {
    // Create a tiny 2x2 red PNG
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
    let mut png_buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_buf), image::ImageFormat::Png)
        .unwrap();
    let ico_data = png_to_ico(&png_buf);
    assert_eq!(ico_data.len(), 6 + 16 + png_buf.len());
    assert_eq!(ico_data[6], 2); // width
    assert_eq!(ico_data[7], 2); // height
}

#[test]
fn test_png_to_ico_invalid_data_fallback() {
    // Invalid PNG data should fall back to returning original bytes
    let bad_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let result = png_to_ico(&bad_data);
    assert_eq!(result, bad_data, "invalid data should return original bytes as fallback");
}
