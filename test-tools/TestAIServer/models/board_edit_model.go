package models

import (
	"strings"
	"time"
)

// TestAIBoardEdit — worker 侧档案管线编辑桩（看板项目档案 goal P4/T-mrg-1）。
//
// 模拟真实执行 agent 收到带 Working Directory 附加段的看板派发任务：解析
// 工作副本绝对路径 + <EDIT_ANCHOR> 编辑指令，emit **真实 edit_file 工具
// 调用**（B 端 agent 走真实文件工具改工作副本里的文件——不伪造文件系统
// 状态，变更集 diff 拿到的是真实工具链产物）。
//
// 对话机（与 testai-3.0/board-1.0 同款两轮模式）：
//
//	轮1 最后一条是 user 消息且同时解析出工作副本路径与编辑指令
//	    → edit_file 工具调用 {path, old_text, new_text}
//	轮2 最后一条是 tool 结果且含「File edited」（edit_file 成功锚点）
//	    → 固定交付文本 FILE_EDIT_DONE（review 桩默认 PASS 分支可收）
//	工具失败 → FILE_EDIT_FAILED + 原始错误（测试侧断言暴露）
//
// <EDIT_ANCHOR> 语法（UAT 任务描述里内嵌）：
//
//	<EDIT_ANCHOR>old_text|||new_text</EDIT_ANCHOR>
//
// 三竖线分隔符避免与文件内容里的常见标点冲突；old_text 必须在基线文件
// 里唯一（edit_file 多命中会报歧义错误——UAT 基线构造保证）。
//
// P5 扩展（T-mrg-4/5/6 联验）：
//   - `<EDIT_ANCHOR2>old|||new</EDIT_ANCHOR2>`：重派备选锚——派发 content
//     含冲突重派说明签名「重新实现本单改动」时改用本锚（新基线上 old_text
//     已是另一 worker 合入后的值，按新基线重新表达同一意图 → 补派合并干净）。
//   - `<EDIT_FILE>/rel/path</EDIT_FILE>`：目标文件（缺省 common.h）。
//   - `<BIN_EDIT>`：写二进制桩（write_file，内容含 NUL 字节——变更集按内容
//     级判二进制）；`<BIN_EDIT_V2>` 写另一份内容（双 worker 各写一份不同
//     字节 → add/add 二进制冲突，T-mrg-6 择边联验）。BIN 桩与锚点同用时
//     为两步形态：轮 1 写 logo.bin，轮 2（tool 结果后）再发锚点编辑。
type TestAIBoardEdit struct{}

// binStubV1/V2 二进制桩内容（NUL 字节驱动 looks_binary 内容级判定）。
var binStubV1 = []byte("BIN\x00STUB\x00V1\x00")
var binStubV2 = []byte("BIN\x00STUB\x00V2\x00\x00")

func NewTestAIBoardEdit() *TestAIBoardEdit { return &TestAIBoardEdit{} }

func (m *TestAIBoardEdit) Name() string { return "testai-board-edit-1.0" }

func (m *TestAIBoardEdit) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	switch last.Role {
	case "user":
		execDir := extractWorkdirPath(last.Content)
		if execDir == "" {
			return "FILE_EDIT_SKIP_NO_ANCHOR"
		}
		// 二进制桩：write_file 直接写含 NUL 的确定性内容（T-mrg-6）。
		// 若任务同时带锚点（BIN+anchor 两步形态），本轮只写 logo.bin，
		// 锚点编辑在下一轮 tool 结果后补发（见 tool 分支）。
		if strings.Contains(last.Content, "<BIN_EDIT>") {
			return buildSingleToolCall("write_file", map[string]interface{}{
				"path":    execDir + "/assets/logo.bin",
				"content": string(binStubV1),
			})
		}
		if strings.Contains(last.Content, "<BIN_EDIT_V2>") {
			return buildSingleToolCall("write_file", map[string]interface{}{
				"path":    execDir + "/assets/logo.bin",
				"content": string(binStubV2),
			})
		}
		return m.editCallFor(last.Content, execDir)
	case "tool":
		// 两步形态第二步：历史里同时有 BIN_EDIT(+V2) 与锚点、logo.bin 写入
		// 已成功（"wrote "）且锚点编辑尚未发生（无 "File edited"）→ 发锚点
		// 编辑（T-mrg-6：单 issue 同时改文本文件 + 写二进制）。
		all := concatMessageContent(messages)
		if strings.Contains(last.Content, "wrote ") &&
			!strings.Contains(all, "File edited") &&
			(strings.Contains(all, "<BIN_EDIT>") || strings.Contains(all, "<BIN_EDIT_V2>")) &&
			(strings.Contains(all, "<EDIT_ANCHOR>") || (strings.Contains(all, "<EDIT_ANCHOR2>"))) {
			execDir := extractWorkdirPath(all)
			if execDir != "" {
				return m.editCallFor(all, execDir)
			}
		}
		// edit_file 成功锚「File edited」/ write_file 成功锚「wrote N bytes」。
		if strings.Contains(last.Content, "File edited") || strings.Contains(last.Content, "wrote ") {
			return "FILE_EDIT_DONE 已按任务要求完成文件编辑，交付完成。"
		}
		return "FILE_EDIT_FAILED " + last.Content
	}
	return boardAckText
}

// editCallFor 按任务文本选锚并产出 edit_file 工具调用（轮 1 与 BIN 两步
// 形态的第二步共用同一锚选择规则）。
func (m *TestAIBoardEdit) editCallFor(taskContent string, execDir string) string {
	target := extractEditFile(taskContent)
	// 冲突重派说明在场 → 用备选锚（新基线上重新表达意图）。
	if strings.Contains(taskContent, "重新实现本单改动") {
		if old2, new2, ok := extractEditAnchor2(taskContent); ok {
			return buildSingleToolCall("edit_file", map[string]interface{}{
				"path":     execDir + "/" + target,
				"old_text": old2,
				"new_text": new2,
			})
		}
		return "FILE_EDIT_SKIP_NO_ANCHOR2"
	}
	oldText, newText, ok := extractEditAnchor(taskContent)
	if ok {
		// 工作副本内指定目标文件（缺省 common.h，T-mrg-1 基线契约）。
		return buildSingleToolCall("edit_file", map[string]interface{}{
			"path":     execDir + "/" + target,
			"old_text": oldText,
			"new_text": newText,
		})
	}
	return "FILE_EDIT_SKIP_NO_ANCHOR"
}

// concatMessageContent 拼接全部消息文本（两步形态的历史扫描用）。
func concatMessageContent(messages []Message) string {
	var buf strings.Builder
	for _, msg := range messages {
		buf.WriteString(msg.Content)
		buf.WriteString("\n")
	}
	return buf.String()
}

func (m *TestAIBoardEdit) Delay() time.Duration { return 0 }

// extractWorkdirPath 从任务 content 抠档案管线 Working Directory 附加段的
// 工作副本绝对路径（render_workdir_section 契约：「位于：」下一行反引号
// 包裹）。无附加段（非档案管线/降级形态）返回空串。
func extractWorkdirPath(content string) string {
	const key = "位于：\n`"
	i := strings.Index(content, key)
	if i < 0 {
		return ""
	}
	rest := content[i+len(key):]
	j := strings.Index(rest, "`")
	if j < 0 {
		return ""
	}
	return strings.TrimSpace(rest[:j])
}

// extractEditAnchor 抠 <EDIT_ANCHOR>old|||new</EDIT_ANCHOR> 编辑指令；
// 缺闭合标签/分隔符 = ok=false（宁可不编辑也不瞎改——测试侧断言暴露）。
func extractEditAnchor(content string) (string, string, bool) {
	const open = "<EDIT_ANCHOR>"
	i := strings.Index(content, open)
	if i < 0 {
		return "", "", false
	}
	rest := content[i+len(open):]
	end := strings.Index(rest, "</EDIT_ANCHOR>")
	if end < 0 {
		return "", "", false
	}
	body := rest[:end]
	k := strings.Index(body, "|||")
	if k < 0 {
		return "", "", false
	}
	return body[:k], body[k+3:], true
}

// extractEditAnchor2 抠 <EDIT_ANCHOR2>old|||new</EDIT_ANCHOR2> 重派备选锚
// （P5：冲突重派说明在场时优先使用）。
func extractEditAnchor2(content string) (string, string, bool) {
	const open = "<EDIT_ANCHOR2>"
	i := strings.Index(content, open)
	if i < 0 {
		return "", "", false
	}
	rest := content[i+len(open):]
	end := strings.Index(rest, "</EDIT_ANCHOR2>")
	if end < 0 {
		return "", "", false
	}
	body := rest[:end]
	k := strings.Index(body, "|||")
	if k < 0 {
		return "", "", false
	}
	return body[:k], body[k+3:], true
}

// extractEditFile 抠 <EDIT_FILE>/rel/path</EDIT_FILE> 目标文件（P5：缺省
// common.h 兼容 T-mrg-1 契约）。
func extractEditFile(content string) string {
	const open = "<EDIT_FILE>"
	i := strings.Index(content, open)
	if i < 0 {
		return "common.h"
	}
	rest := content[i+len(open):]
	end := strings.Index(rest, "</EDIT_FILE>")
	if end < 0 {
		return "common.h"
	}
	t := strings.TrimSpace(rest[:end])
	if t == "" {
		return "common.h"
	}
	return strings.TrimPrefix(t, "/")
}
