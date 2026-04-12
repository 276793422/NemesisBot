//go:build !cross_compile

package process

import (
	"os"
	"testing"
)

func TestHasChildModeFlag(t *testing.T) {
	origArgs := os.Args
	defer func() { os.Args = origArgs }()

	tests := []struct {
		name     string
		args     []string
		expected bool
	}{
		{
			name:     "with --multiple flag",
			args:     []string{"test.exe", "--multiple", "--child-id", "child-1"},
			expected: true,
		},
		{
			name:     "without --multiple flag",
			args:     []string{"test.exe", "--child-id", "child-1"},
			expected: false,
		},
		{
			name:     "empty args",
			args:     []string{"test.exe"},
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.Args = tt.args
			result := HasChildModeFlag()
			if result != tt.expected {
				t.Errorf("HasChildModeFlag() = %v, want %v", result, tt.expected)
			}
		})
	}
}

func TestGetChildID(t *testing.T) {
	origArgs := os.Args
	defer func() { os.Args = origArgs }()

	tests := []struct {
		name     string
		args     []string
		expected string
	}{
		{
			name:     "valid child-id",
			args:     []string{"test.exe", "--multiple", "--child-id", "child-42"},
			expected: "child-42",
		},
		{
			name:     "missing child-id value",
			args:     []string{"test.exe", "--multiple", "--child-id"},
			expected: "",
		},
		{
			name:     "no child-id flag",
			args:     []string{"test.exe", "--multiple"},
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.Args = tt.args
			result := GetChildID()
			if result != tt.expected {
				t.Errorf("GetChildID() = %q, want %q", result, tt.expected)
			}
		})
	}
}

func TestGetWindowType(t *testing.T) {
	origArgs := os.Args
	defer func() { os.Args = origArgs }()

	tests := []struct {
		name     string
		args     []string
		expected string
	}{
		{
			name:     "approval window type",
			args:     []string{"test.exe", "--multiple", "--window-type", "approval"},
			expected: "approval",
		},
		{
			name:     "headless window type",
			args:     []string{"test.exe", "--multiple", "--window-type", "headless"},
			expected: "headless",
		},
		{
			name:     "missing window-type value",
			args:     []string{"test.exe", "--multiple", "--window-type"},
			expected: "",
		},
		{
			name:     "no window-type flag",
			args:     []string{"test.exe", "--multiple"},
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.Args = tt.args
			result := GetWindowType()
			if result != tt.expected {
				t.Errorf("GetWindowType() = %q, want %q", result, tt.expected)
			}
		})
	}
}
