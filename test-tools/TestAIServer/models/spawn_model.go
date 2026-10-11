package models

import (
	"strings"
	"time"
)

// TestAISpawn — M5 跨节点审批桥 UAT 桩（cluster-uat T48）。
//
// 模拟 worker 执行看板任务时发起高危本地操作：解析任务描述里的
// <SPAWN_PATH>绝对路径</SPAWN_PATH>，emit **真实 exec 工具调用**
// `del "<path>"`——**cmd.exe 内置命令**（exec 工具 Windows 走 `cmd /C`
// 执行，loop_tools.rs:1690；PowerShell 的 Remove-Item 在 cmd 下是
// 「不是内部或外部命令」，报错无 SECURITY BLOCKED → 桩误报完成——T48
// 首轮实证）。默认 Windows 策略 process_rules.exec `del *` → ask（且不含
// `/s` 递归形态，命令守卫与 deny 规则 `del /s*` 均放行），确定性触发
// ABAC 审批 ask → worker ask 上行 master（M5 ask_register）→ master
// approval.remote_respond 裁决下发 → 工具放行/拦截两分支。
//
// 路径引号用双引号（cmd 不认单引号），正斜杠转反斜杠（防 cmd 把 `/段`
// 当开关解析）；SPAWN 标记回显保持 SPAWN_PATH 原形态（正斜杠），T48
// 以同形态字符串做包含断言。
//
// 对话机（与 board-edit 同款两轮形态 + 当前任务锚定纪律——B 端
// peer-chat 会话跨任务复用，禁止全历史扫描）：
//
//	轮1 最后一条是 user 消息且解析出 <SPAWN_PATH>
//	    → exec 工具调用 {command: `del "<backslash path>"`}
//	轮2 最后一条是 tool 结果：
//	    含 "SECURITY BLOCKED"（安全拦截链固定回灌前缀，含审批拒绝）
//	        → SPAWN_DENIED:<path>
//	    否则（批准后命令真实执行，无论命令本身成败）
//	        → SPAWN_COMPLETED:<path>
//
// 交付标记由 T48 消费：approve 臂断言文件已从磁盘消失，deny 臂断言
// 文件仍在——物理证据，不依赖模型自述。
type TestAISpawn struct{}

func NewTestAISpawn() *TestAISpawn { return &TestAISpawn{} }

func (m *TestAISpawn) Name() string { return "testai-spawn-1.0" }

func (m *TestAISpawn) Delay() time.Duration { return 0 }

// spawnPathFrom 提取 <SPAWN_PATH>...</SPAWN_PATH> 内的目标绝对路径；
// 缺标记/缺闭合返回空串（调用方走 SKIP 分支，测试断言暴露）。
func spawnPathFrom(task string) string {
	start := strings.Index(task, "<SPAWN_PATH>")
	if start < 0 {
		return ""
	}
	start += len("<SPAWN_PATH>")
	end := strings.Index(task[start:], "</SPAWN_PATH>")
	if end < 0 {
		return ""
	}
	return strings.TrimSpace(task[start : start+end])
}

func (m *TestAISpawn) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	// 路径只锚定当前任务（最后一条 user 消息）——会话复用下早前任务的
	// SPAWN_PATH 留在历史里，全历史扫描会拿错目标（board-edit 同款纪律）。
	path := spawnPathFrom(lastUserContent(messages))
	switch last.Role {
	case "user":
		if path == "" {
			return "SPAWN_SKIP_NO_PATH"
		}
		winPath := strings.ReplaceAll(path, "/", "\\")
		return buildSingleToolCall("exec", map[string]interface{}{
			"command": `del "` + winPath + `"`,
		})
	case "tool":
		if path == "" {
			return "SPAWN_DONE_NO_PATH"
		}
		if strings.Contains(last.Content, "SECURITY BLOCKED") {
			return "SPAWN_DENIED:" + path
		}
		return "SPAWN_COMPLETED:" + path
	}
	return boardAckText
}
