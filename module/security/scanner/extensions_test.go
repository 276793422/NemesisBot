// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package scanner

import (
	"testing"
)

func TestShouldScanFile_ScanExtensions_Yes(t *testing.T) {
	rules := ExtensionRules{ScanExtensions: []string{".exe", ".dll"}}
	if !ShouldScanFile("C:\\test\\program.exe", rules) {
		t.Error(".exe should be scanned in whitelist mode")
	}
}

func TestShouldScanFile_ScanExtensions_No(t *testing.T) {
	rules := ExtensionRules{ScanExtensions: []string{".exe", ".dll"}}
	if ShouldScanFile("C:\\test\\readme.txt", rules) {
		t.Error(".txt should not be scanned in whitelist mode")
	}
}

func TestShouldScanFile_SkipExtensions_Yes(t *testing.T) {
	rules := ExtensionRules{SkipExtensions: []string{".txt", ".md"}}
	if !ShouldScanFile("C:\\test\\program.exe", rules) {
		t.Error(".exe should be scanned in blacklist mode")
	}
}

func TestShouldScanFile_SkipExtensions_No(t *testing.T) {
	rules := ExtensionRules{SkipExtensions: []string{".txt", ".md"}}
	if ShouldScanFile("C:\\test\\readme.txt", rules) {
		t.Error(".txt should be skipped in blacklist mode")
	}
}

func TestShouldScanFile_NoRules(t *testing.T) {
	rules := ExtensionRules{}
	if !ShouldScanFile("C:\\test\\anything.xyz", rules) {
		t.Error("No rules should scan everything")
	}
}

func TestShouldScanFile_EmptyExtension(t *testing.T) {
	rules := ExtensionRules{}
	if !ShouldScanFile("C:\\test\\Makefile", rules) {
		t.Error("Files without extension should be scanned")
	}
}

func TestShouldScanFile_CaseInsensitive(t *testing.T) {
	rules := ExtensionRules{SkipExtensions: []string{".txt"}}
	if ShouldScanFile("C:\\test\\README.TXT", rules) {
		t.Error("Extension matching should be case insensitive")
	}
}

func TestShouldScanFile_ScanExtensionsCaseInsensitive(t *testing.T) {
	rules := ExtensionRules{ScanExtensions: []string{".EXE"}}
	if !ShouldScanFile("C:\\test\\program.exe", rules) {
		t.Error("Extension matching should be case insensitive")
	}
}

func TestShouldScanFile_BothListsEmpty(t *testing.T) {
	rules := ExtensionRules{
		ScanExtensions: []string{},
		SkipExtensions: []string{},
	}
	if !ShouldScanFile("C:\\test\\file.dat", rules) {
		t.Error("Empty lists should scan everything")
	}
}

func TestShouldScanFile_ScanExtensionsPriority(t *testing.T) {
	// When ScanExtensions is set, SkipExtensions is ignored
	rules := ExtensionRules{
		ScanExtensions: []string{".exe"},
		SkipExtensions: []string{".exe"}, // contradictory, but ScanExtensions wins
	}
	if !ShouldScanFile("C:\\test\\file.exe", rules) {
		t.Error("ScanExtensions should take priority and match .exe")
	}
	if ShouldScanFile("C:\\test\\file.txt", rules) {
		t.Error("ScanExtensions should exclude .txt")
	}
}
