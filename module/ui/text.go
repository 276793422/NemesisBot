// Package ui provides text formatting and alignment utilities for CLI output
package ui

import (
	"fmt"
	"strings"
)

// GetDisplayWidth calculates the actual display width of a string
// Chinese characters (and other wide Unicode chars) = 2 width
// ASCII characters = 1 width
func GetDisplayWidth(s string) int {
	width := 0
	for _, r := range s {
		if r < 128 {
			// ASCII character (English, numbers, symbols)
			width += 1
		} else {
			// Non-ASCII character (Chinese, Japanese, Korean, etc.)
			width += 2
		}
	}
	return width
}

// PrintBoxTitle prints a centered title inside a box
// boxWidth is the total width including the border characters (║)
// Example:
//
//	╔═════════════════════════════════════════╗
//	║           Centered Title               ║
//	╚═════════════════════════════════════════╝
func PrintBoxTitle(title string, boxWidth int) {
	// Top border
	fmt.Printf("╔%s╗\n", strings.Repeat("═", boxWidth-2))

	// Title line - calculate centering
	titleWidth := GetDisplayWidth(title)
	leftPadding := (boxWidth - 2 - titleWidth) / 2
	rightPadding := boxWidth - 2 - titleWidth - leftPadding

	fmt.Printf("║%s%s%s║\n",
		strings.Repeat(" ", leftPadding),
		title,
		strings.Repeat(" ", rightPadding))

	// Bottom border
	fmt.Printf("╚%s╝\n", strings.Repeat("═", boxWidth-2))
}

// PrintSectionTitle prints a centered title with underline
// Example:
//
//	═════════════════════════════════════════
//	       Section Title
//	═════════════════════════════════════════
func PrintSectionTitle(title string, lineWidth int) {
	titleWidth := GetDisplayWidth(title)
	leftPadding := (lineWidth - titleWidth) / 2
	rightPadding := lineWidth - titleWidth - leftPadding

	fmt.Printf("%s\n", strings.Repeat("═", lineWidth))
	fmt.Printf("%s%s%s\n",
		strings.Repeat(" ", leftPadding),
		title,
		strings.Repeat(" ", rightPadding))
	fmt.Printf("%s\n", strings.Repeat("═", lineWidth))
}

// PrintSeparator prints a separator line with configurable character and width
func PrintSeparator(char string, width int) {
	fmt.Printf("%s\n", strings.Repeat(char, width))
}

// PrintBox prints a text box with configurable width
// Example:
//
//	┌─ Title ─────────────────────────────────┐
//	│ Content line 1                          │
//	│ Content line 2                          │
//	└─────────────────────────────────────────┘
func PrintBox(title, content string, boxWidth int) {
	// Top border with title
	if title != "" {
		titleWidth := GetDisplayWidth(title)
		padding := boxWidth - 3 - titleWidth - 1
		fmt.Printf("┌─ %s%s┐\n", title, strings.Repeat("─", padding))
	} else {
		fmt.Printf("┌%s┐\n", strings.Repeat("─", boxWidth-2))
	}

	// Content
	if content != "" {
		contentWidth := GetDisplayWidth(content)
		padding := boxWidth - 2 - contentWidth - 1
		fmt.Printf("│ %s%s│\n", content, strings.Repeat(" ", padding))
	}

	// Bottom border
	fmt.Printf("└%s┘\n", strings.Repeat("─", boxWidth-2))
}
