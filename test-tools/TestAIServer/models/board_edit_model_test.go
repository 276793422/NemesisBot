package models

import (
	"strings"
	"testing"
)

// TestAIBoardEdit BIN+锚点两步形态（P5/T-mrg-6）：单 issue 同时写二进制
// 桩 + 编辑文本文件，轮 2 在 logo.bin 写入成功后补发锚点编辑。

func binTask(bin string, anchor string) string {
	return "## Working Directory\n位于：\n`C:\\exec\\t6`\n\n任务：<" + bin + "><EDIT_FILE>Cargo.lock</EDIT_FILE><EDIT_ANCHOR>version = 3|||version = 4</EDIT_ANCHOR>" + anchor
}

func TestBoardEditBinPlusAnchorTwoStep(t *testing.T) {
	m := NewTestAIBoardEdit()

	// 轮1：user（BIN_EDIT + 锚点）→ write_file logo.bin。
	r1 := m.Process([]Message{{Role: "user", Content: binTask("BIN_EDIT", "")}})
	name, args := mustToolCall(t, r1)
	if name != "write_file" || !strings.Contains(args, "logo.bin") {
		t.Fatalf("轮1 期望 write_file logo.bin，实际 name=%s args=%s", name, args)
	}

	// 轮2：tool（写入成功）→ 补发锚点编辑（Cargo.lock）。
	r2 := m.Process([]Message{
		{Role: "user", Content: binTask("BIN_EDIT", "")},
		{Role: "tool", Content: "wrote 15 bytes to logo.bin"},
	})
	name, args = mustToolCall(t, r2)
	if name != "edit_file" || !strings.Contains(args, "Cargo.lock") || !strings.Contains(args, "version = 4") {
		t.Fatalf("轮2 期望 edit_file Cargo.lock version=4，实际 name=%s args=%s", name, args)
	}

	// 轮3：tool（编辑成功）→ 交付收尾。
	r3 := m.Process([]Message{
		{Role: "user", Content: binTask("BIN_EDIT", "")},
		{Role: "tool", Content: "wrote 15 bytes to logo.bin"},
		{Role: "tool", Content: "File edited: Cargo.lock"},
	})
	if !strings.Contains(r3, "FILE_EDIT_DONE") {
		t.Fatalf("轮3 应收尾 FILE_EDIT_DONE，实际 %.120s", r3)
	}
}

func TestBoardEditBinAloneStillSingleStep(t *testing.T) {
	m := NewTestAIBoardEdit()
	// 纯 BIN_EDIT（无锚点）保持单步：写入成功即收尾（兼容 T-mrg-6 旧形态）。
	r2 := m.Process([]Message{
		{Role: "user", Content: "位于：\n`C:\\exec\\x`\n<BIN_EDIT>"},
		{Role: "tool", Content: "wrote 15 bytes to logo.bin"},
	})
	if !strings.Contains(r2, "FILE_EDIT_DONE") {
		t.Fatalf("纯 BIN_EDIT 写入成功应直接收尾，实际 %.120s", r2)
	}
}

func TestBoardEditTwoStepRespectsRedispatchAnchor2(t *testing.T) {
	m := NewTestAIBoardEdit()
	// 两步形态 + 冲突重派说明 → 轮 2 用 ANCHOR2（新基线上重新表达意图）。
	task := "位于：\n`C:\\exec\\x`\n⚠ 上次交付与仓库当前内容合并冲突，请基于最新基线重新实现本单改动。" +
		"<BIN_EDIT><EDIT_FILE>Cargo.lock</EDIT_FILE>" +
		"<EDIT_ANCHOR>version = 3|||version = 4</EDIT_ANCHOR>" +
		"<EDIT_ANCHOR2>version = 4|||version = 9</EDIT_ANCHOR2>"
	r1 := m.Process([]Message{{Role: "user", Content: task}})
	name, _ := mustToolCall(t, r1)
	if name != "write_file" {
		t.Fatalf("重派两步轮1 仍应先写 logo.bin，实际 %s", name)
	}
	r2 := m.Process([]Message{
		{Role: "user", Content: task},
		{Role: "tool", Content: "wrote 15 bytes to logo.bin"},
	})
	name, args := mustToolCall(t, r2)
	if name != "edit_file" || !strings.Contains(args, `"new_text":"version = 9"`) || !strings.Contains(args, `"old_text":"version = 4"`) {
		t.Fatalf("重派两步轮2 应用 ANCHOR2（version=4→9），实际 name=%s args=%s", name, args)
	}
}
