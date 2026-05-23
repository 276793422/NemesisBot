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
mod tests;
