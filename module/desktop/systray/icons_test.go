package systray

import (
	"testing"
)

func TestPngToIco(t *testing.T) {
	t.Run("embedded icon data", func(t *testing.T) {
		if len(iconPNG) == 0 {
			t.Skip("No embedded icon data")
		}
		result := pngToIco(iconPNG)
		if len(result) == 0 {
			t.Error("pngToIco should return non-empty data")
		}
	})

	t.Run("invalid data falls back to raw", func(t *testing.T) {
		invalidData := []byte("not a png")
		result := pngToIco(invalidData)
		if string(result) != "not a png" {
			t.Error("Invalid data should be returned as-is")
		}
	})

	t.Run("empty data", func(t *testing.T) {
		result := pngToIco([]byte{})
		if len(result) != 0 {
			t.Error("Empty data should return empty result")
		}
	})
}

func TestGetIcon(t *testing.T) {
	icon := getIcon()
	if len(icon) == 0 {
		t.Error("getIcon should return non-empty data")
	}
}
