package models

// testai-credexec-1.0 — 凭据 env 槽 e2e 驱动模型（vault bind 域绑定闸冒烟）。
//
// 脚本行为（确定性，无采样随机性）：
//   用户轮：消息含 <CRED_EXEC>{"command":"...","env":{"KEY":"vault:alias"}}</CRED_EXEC>
//     → 原样下发 exec 工具调用（command + env 逐字透传——闸面看到的正是
//     驱动器构造的声明槽形态）。
//   工具轮：回 "CRED_DONE: <tool 结果首行>"（驱动器断言 LEN/SHA 摘要行；
//     掩码探针在全文上断言）。
//   其余：返回使用提示（不会静默挂死）。

import (
	"encoding/json"
	"strings"
	"time"
)

type CredExecModel struct{}

func NewCredExecModel() *CredExecModel { return &CredExecModel{} }

func (m *CredExecModel) Name() string { return "testai-credexec-1.0" }

func (m *CredExecModel) Delay() time.Duration { return 0 }

func (m *CredExecModel) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// 工具结果轮：回首行（凭据冒烟的脚本只打一行摘要）。
	if last.Role == "tool" {
		line := strings.TrimSpace(last.Content)
		if i := strings.IndexByte(line, '\n'); i >= 0 {
			line = strings.TrimSpace(line[:i])
		}
		return "CRED_DONE: " + line
	}

	// 用户轮：<CRED_EXEC> 标记 → exec 调用（command+env 逐字透传）。
	if i := strings.Index(last.Content, "<CRED_EXEC>"); i >= 0 {
		rest := last.Content[i+len("<CRED_EXEC>"):]
		if j := strings.Index(rest, "</CRED_EXEC>"); j >= 0 {
			var req struct {
				Command string            `json:"command"`
				Env     map[string]string `json:"env"`
			}
			if err := json.Unmarshal([]byte(strings.TrimSpace(rest[:j])), &req); err == nil && req.Command != "" {
				args := map[string]interface{}{"command": req.Command}
				if len(req.Env) > 0 {
					args["env"] = req.Env
				}
				return buildSingleToolCall("exec", args)
			}
			return "BAD_CRED_EXEC_TAG"
		}
	}
	return `send <CRED_EXEC>{"command":"...","env":{"KEY":"vault:alias"}}</CRED_EXEC> to drive a cred exec`
}
