//go:build windows

package desktop

import (
	"testing"
)

func TestCalculateOptimalWindowSize(t *testing.T) {
	width, height := calculateOptimalWindowSize()

	t.Logf("Calculated window size: %dx%d", width, height)

	if width < 400 || height < 300 {
		t.Errorf("Window size too small: %dx%d", width, height)
	}

	if width > 3840 || height > 2160 {
		t.Logf("Warning: Window size seems large: %dx%d", width, height)
	}

	if width <= 0 || height <= 0 {
		t.Errorf("Window dimensions must be positive, got: %dx%d", width, height)
	}
}

func TestGetScreenSize(t *testing.T) {
	width, height := getScreenSizeWindows()

	t.Logf("Detected screen size: %dx%d", width, height)

	if width <= 0 || height <= 0 {
		t.Errorf("Screen size must be positive, got: %dx%d", width, height)
	}

	if width < 800 || height < 600 {
		t.Logf("Warning: Screen size seems small: %dx%d", width, height)
	}

	if width == 1920 && height == 1080 {
		t.Log("Using default Full HD resolution (fallback)")
	}
}
