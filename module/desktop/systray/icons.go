//go:build !cross_compile

package systray

import (
	"bytes"
	"image"
	_ "image/png"
	"runtime"

	_ "embed"
)

//go:embed icon.png
var iconPNG []byte

// getIcon returns icon data based on platform
// Windows: .ico format (converted from PNG)
// Linux/macOS: .png format
func getIcon() []byte {
	if runtime.GOOS == "windows" {
		return pngToIco(iconPNG)
	}
	return iconPNG
}

// pngToIco converts PNG data to ICO format
// ICO format with PNG embedded (supported by Windows Vista+)
func pngToIco(pngData []byte) []byte {
	// ICO Header (6 bytes)
	header := []byte{
		0x00, 0x00,       // Reserved
		0x01, 0x00,       // Type: 1 = ICO
		0x01, 0x00,       // Count: 1 icon
	}

	// Decode PNG to get dimensions
	img, _, err := image.Decode(bytes.NewReader(pngData))
	if err != nil {
		// Fallback: return original PNG data
		return pngData
	}

	bounds := img.Bounds()
	width := uint8(bounds.Dx())
	height := uint8(bounds.Dy())

	// Only support sizes up to 256
	if width > 255 {
		width = 0 // 0 means 256
	}
	if height > 255 {
		height = 0
	}

	// Icon Directory Entry (16 bytes)
	entry := []byte{
		width,                  // Width
		height,                 // Height
		0,                      // Color count: 0 for PNG
		0,                      // Reserved
		1, 0,                   // Color planes: 1
		32, 0,                  // Bits per pixel: 32
	}

	// Size of image data (4 bytes, little-endian)
	size := len(pngData)
	entry = append(entry,
		byte(size),
		byte(size>>8),
		byte(size>>16),
		byte(size>>24),
	)

	// Offset to image data (4 bytes, little-endian)
	// Header (6) + Entry (16) = 22
	offset := uint32(22)
	entry = append(entry,
		byte(offset),
		byte(offset>>8),
		byte(offset>>16),
		byte(offset>>24),
	)

	// Combine all parts
	var ico bytes.Buffer
	ico.Write(header)
	ico.Write(entry)
	ico.Write(pngData)

	return ico.Bytes()
}

