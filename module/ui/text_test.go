package ui

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"strings"
	"testing"
)

func TestGetDisplayWidth(t *testing.T) {
	tests := []struct {
		input    string
		expected int
	}{
		{"", 0},
		{"hello", 5},
		{"Cluster Status", 14},
		{"ABC", 3},
		{"123", 3},
		{"!@#$%", 5},
		{"集群状态", 8},       // 4 Chinese chars, each 2 width
		{"你好世界", 8},      // 4 Chinese chars
		{"TestAIServer 帮助系统", 21}, // 12 ASCII + 1 space + 8 CJK = 21
		{"a中b文c", 7},              // a(1)+中(2)+b(1)+文(2)+c(1) = 7
		{"日本語テスト", 12},         // 6 Japanese chars, each 2 width
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			got := GetDisplayWidth(tt.input)
			if got != tt.expected {
				t.Errorf("GetDisplayWidth(%q) = %d, want %d", tt.input, got, tt.expected)
			}
		})
	}
}

func TestGetDisplayWidth_ASCII(t *testing.T) {
	if w := GetDisplayWidth("a"); w != 1 {
		t.Errorf("Single ASCII char should be width 1, got %d", w)
	}
}

func TestGetDisplayWidth_CJK(t *testing.T) {
	if w := GetDisplayWidth("中"); w != 2 {
		t.Errorf("Single CJK char should be width 2, got %d", w)
	}
}

func TestGetDisplayWidth_Mixed(t *testing.T) {
	// "abc中文" = 3*1 + 2*2 = 7
	if w := GetDisplayWidth("abc中文"); w != 7 {
		t.Errorf("'abc中文' should be width 7, got %d", w)
	}
}

// captureOutput captures fmt.Printf output during the execution of f
func captureOutput(f func()) string {
	old := os.Stdout
	r, w, _ := os.Pipe()
	os.Stdout = w
	f()
	w.Close()
	os.Stdout = old

	var buf bytes.Buffer
	io.Copy(&buf, r)
	return buf.String()
}

func TestPrintBoxTitle(t *testing.T) {
	tests := []struct {
		name     string
		title    string
		boxWidth int
	}{
		{"English title", "Hello World", 30},
		{"Chinese title", "测试标题", 30},
		{"Mixed title", "Test测试", 30},
		{"Narrow box", "A", 10},
		{"Wide box", "Wide Title Here", 80},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			output := captureOutput(func() {
				PrintBoxTitle(tt.title, tt.boxWidth)
			})

			if !strings.Contains(output, "╔") {
				t.Error("Output should contain top-left corner")
			}
			if !strings.Contains(output, "╗") {
				t.Error("Output should contain top-right corner")
			}
			if !strings.Contains(output, "╚") {
				t.Error("Output should contain bottom-left corner")
			}
			if !strings.Contains(output, "╝") {
				t.Error("Output should contain bottom-right corner")
			}
			if !strings.Contains(output, tt.title) {
				t.Errorf("Output should contain title %q", tt.title)
			}
			if !strings.Contains(output, "═") {
				t.Error("Output should contain horizontal bar")
			}
			if !strings.Contains(output, "║") {
				t.Error("Output should contain vertical bar")
			}
			// Should have exactly 3 lines
			lines := strings.Count(output, "\n")
			if lines != 3 {
				t.Errorf("Expected 3 lines, got %d", lines)
			}
		})
	}
}

func TestPrintSectionTitle(t *testing.T) {
	tests := []struct {
		name      string
		title     string
		lineWidth int
	}{
		{"English", "Section Title", 40},
		{"Chinese", "区域标题", 40},
		{"Mixed", "Title标题", 40},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			output := captureOutput(func() {
				PrintSectionTitle(tt.title, tt.lineWidth)
			})

			if !strings.Contains(output, tt.title) {
				t.Errorf("Output should contain title %q", tt.title)
			}
			if !strings.Contains(output, "═") {
				t.Error("Output should contain separator")
			}
			// Should have 3 lines
			lines := strings.Count(output, "\n")
			if lines != 3 {
				t.Errorf("Expected 3 lines, got %d", lines)
			}
		})
	}
}

func TestPrintSeparator(t *testing.T) {
	tests := []struct {
		char  string
		width int
	}{
		{"─", 60},
		{"=", 40},
		{"*", 20},
		{"─", 1},
	}

	for _, tt := range tests {
		t.Run(fmt.Sprintf("%s_%d", tt.char, tt.width), func(t *testing.T) {
			output := captureOutput(func() {
				PrintSeparator(tt.char, tt.width)
			})

			expected := strings.Repeat(tt.char, tt.width)
			if !strings.Contains(output, expected) {
				t.Errorf("Output should contain %q repeated %d times", tt.char, tt.width)
			}
		})
	}
}

func TestPrintBox(t *testing.T) {
	t.Run("with title and content", func(t *testing.T) {
		output := captureOutput(func() {
			PrintBox("Title", "Content", 40)
		})

		if !strings.Contains(output, "Title") {
			t.Error("Output should contain title")
		}
		if !strings.Contains(output, "Content") {
			t.Error("Output should contain content")
		}
		if !strings.Contains(output, "┌") {
			t.Error("Output should contain top-left corner")
		}
		if !strings.Contains(output, "┐") {
			t.Error("Output should contain top-right corner")
		}
		if !strings.Contains(output, "│") {
			t.Error("Output should contain side bar")
		}
		if !strings.Contains(output, "└") {
			t.Error("Output should contain bottom-left corner")
		}
		if !strings.Contains(output, "┘") {
			t.Error("Output should contain bottom-right corner")
		}
	})

	t.Run("without title", func(t *testing.T) {
		output := captureOutput(func() {
			PrintBox("", "Content only", 40)
		})

		if !strings.Contains(output, "Content only") {
			t.Error("Output should contain content")
		}
		// Should still have box borders
		if !strings.Contains(output, "┌") {
			t.Error("Output should contain top-left corner")
		}
	})

	t.Run("without content", func(t *testing.T) {
		output := captureOutput(func() {
			PrintBox("Title only", "", 40)
		})

		if !strings.Contains(output, "Title only") {
			t.Error("Output should contain title")
		}
		// Should have top border and bottom border but no content line
		if !strings.Contains(output, "┌") {
			t.Error("Output should contain top-left corner")
		}
		if !strings.Contains(output, "└") {
			t.Error("Output should contain bottom-left corner")
		}
	})

	t.Run("empty title and content", func(t *testing.T) {
		output := captureOutput(func() {
			PrintBox("", "", 20)
		})

		// Should produce box with just borders
		if !strings.Contains(output, "┌") {
			t.Error("Output should contain top-left corner")
		}
		if !strings.Contains(output, "└") {
			t.Error("Output should contain bottom-left corner")
		}
	})

	t.Run("with Chinese content", func(t *testing.T) {
		output := captureOutput(func() {
			PrintBox("基础响应模型", "快速测试和消息验证", 62)
		})

		if !strings.Contains(output, "基础响应模型") {
			t.Error("Output should contain Chinese title")
		}
		if !strings.Contains(output, "快速测试和消息验证") {
			t.Error("Output should contain Chinese content")
		}
	})
}

func TestPrintBoxTitle_EdgeCases(t *testing.T) {
	t.Run("box width equals title width", func(t *testing.T) {
		// Should not panic even if box is narrow
		output := captureOutput(func() {
			PrintBoxTitle("AB", 6)
		})
		_ = output
	})

	t.Run("very wide box", func(t *testing.T) {
		output := captureOutput(func() {
			PrintBoxTitle("Title", 200)
		})
		if !strings.Contains(output, "Title") {
			t.Error("Output should contain title")
		}
	})
}

func TestPrintSeparator_EdgeCases(t *testing.T) {
	t.Run("zero width", func(t *testing.T) {
		output := captureOutput(func() {
			PrintSeparator("─", 0)
		})
		// Should just have a newline
		if output != "\n" {
			t.Errorf("Expected just newline, got %q", output)
		}
	})
}
