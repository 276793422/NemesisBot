//! Icon management.
//!
//! Provides in-memory icon representation and basic manipulation (scaling,
//! PNG serialization). Actual file I/O and platform icon conversion (e.g.
//! PNG to ICO on Windows) are handled by platform-specific backends that
//! consume the types defined here.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Icon size
// ---------------------------------------------------------------------------

/// Standard icon dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconSize {
    /// 16x16 pixels.
    Small,
    /// 32x32 pixels.
    Medium,
    /// 64x64 pixels.
    Large,
}

impl IconSize {
    /// Returns the width and height in pixels for this size variant.
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Small => (16, 16),
            Self::Medium => (32, 32),
            Self::Large => (64, 64),
        }
    }
}

// ---------------------------------------------------------------------------
// Icon
// ---------------------------------------------------------------------------

/// An in-memory icon image backed by raw RGBA pixel data.
///
/// The data vector contains `width * height * 4` bytes (one byte per channel,
/// RGBA order).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    /// Raw RGBA pixel data.
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Icon {
    /// Creates a new icon from raw RGBA data.
    ///
    /// Returns `None` if the data length does not match `width * height * 4`.
    pub fn load_from_bytes(data: Vec<u8>, width: u32, height: u32) -> Option<Self> {
        let expected = (width * height) as usize * 4;
        if data.len() != expected {
            return None;
        }
        Some(Self {
            data,
            width,
            height,
        })
    }

    /// Creates a solid-color icon of the given size.
    ///
    /// Useful for testing and as placeholder icons.
    pub fn solid_color(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Self {
        let pixel = [r, g, b, a];
        let data: Vec<u8> = pixel
            .iter()
            .cycle()
            .take((width * height * 4) as usize)
            .copied()
            .collect();
        Self {
            data,
            width,
            height,
        }
    }

    /// Returns a scaled copy of the icon using nearest-neighbor interpolation.
    ///
    /// For downscaling, pixels are sampled at regular intervals. For
    /// upscaling, pixels are duplicated. This is intentionally simple --
    /// production code would use a proper image resampling library.
    pub fn get_scaled(&self, target_width: u32, target_height: u32) -> Icon {
        let mut out = Vec::with_capacity((target_width * target_height * 4) as usize);

        for y in 0..target_height {
            // Map target pixel y back to source pixel y
            let src_y = (y as u64 * self.height as u64 / target_height.max(1) as u64) as u32;
            for x in 0..target_width {
                let src_x = (x as u64 * self.width as u64 / target_width.max(1) as u64) as u32;
                let src_idx = ((src_y * self.width + src_x) * 4) as usize;
                out.extend_from_slice(&self.data[src_idx..src_idx + 4]);
            }
        }

        Icon {
            data: out,
            width: target_width,
            height: target_height,
        }
    }

    /// Returns the raw RGBA data as-is (already in the internal format).
    ///
    /// Named `to_png_bytes` for API compatibility with the Go counterpart,
    /// but note this returns raw RGBA, not actual PNG-compressed data.
    /// A real implementation would encode to PNG format here.
    pub fn to_png_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }

    /// Returns the pixel at `(x, y)` as `(r, g, b, a)`.
    ///
    /// Returns `None` if the coordinates are out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        if idx + 3 >= self.data.len() {
            return None;
        }
        Some((
            self.data[idx],
            self.data[idx + 1],
            self.data[idx + 2],
            self.data[idx + 3],
        ))
    }
}

// ---------------------------------------------------------------------------
// Tray icon loading
// ---------------------------------------------------------------------------

/// Load the embedded icon as a `tray_icon::Icon` for use in the system tray.
///
/// Decodes the embedded PNG to RGBA data and creates a platform-native icon.
/// On Windows, `tray-icon` handles the RGBA → HICON conversion internally.
pub fn load_tray_icon() -> tray_icon::Icon {
    load_tray_icon_checked().expect("failed to load tray icon")
}

/// Same as [`load_tray_icon`] but returns a `Result` instead of panicking.
pub fn load_tray_icon_checked() -> Result<tray_icon::Icon, String> {
    let png_data = include_bytes!("../icons/icon.png");
    let img = image::load_from_memory(png_data)
        .map_err(|e| format!("decode icon.png: {}", e))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    tray_icon::Icon::from_rgba(rgba.into_raw(), w, h)
        .map_err(|e| format!("create Icon from RGBA: {:?}", e))
}

/// Returns the raw PNG bytes of the embedded icon.
pub fn embedded_icon_png() -> &'static [u8] {
    include_bytes!("../icons/icon.png")
}

/// Convert PNG data to ICO format (Windows-compatible).
///
/// This is the same algorithm as Go's `pngToIco` in icons.go:
/// ICO Header (6 bytes) + Icon Directory Entry (16 bytes) + PNG data.
/// The PNG is embedded directly in the ICO container (supported by Windows Vista+).
pub fn png_to_ico(png_data: &[u8]) -> Vec<u8> {
    // Decode PNG to get dimensions
    let img = match image::load_from_memory(png_data) {
        Ok(img) => img,
        Err(_) => return png_data.to_vec(), // Fallback: return original PNG
    };
    let (width, height) = (img.width(), img.height());

    // Width/height in ICO header: 0 means 256
    let w_byte = if width >= 256 { 0u8 } else { width as u8 };
    let h_byte = if height >= 256 { 0u8 } else { height as u8 };

    // ICO Header (6 bytes)
    let mut ico = Vec::with_capacity(6 + 16 + png_data.len());
    ico.extend_from_slice(&[0x00, 0x00]); // Reserved
    ico.extend_from_slice(&[0x01, 0x00]); // Type: 1 = ICO
    ico.extend_from_slice(&[0x01, 0x00]); // Count: 1 icon

    // Icon Directory Entry (16 bytes)
    ico.push(w_byte);                // Width
    ico.push(h_byte);                // Height
    ico.push(0);                     // Color count: 0 for PNG
    ico.push(0);                     // Reserved
    ico.extend_from_slice(&[1, 0]);  // Color planes: 1
    ico.extend_from_slice(&[32, 0]); // Bits per pixel: 32

    // Size of image data (4 bytes, little-endian)
    let size = png_data.len() as u32;
    ico.extend_from_slice(&size.to_le_bytes());

    // Offset to image data (4 bytes, little-endian): Header (6) + Entry (16) = 22
    let offset: u32 = 22;
    ico.extend_from_slice(&offset.to_le_bytes());

    // PNG data
    ico.extend_from_slice(png_data);

    ico
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
}
