package models

import (
	"strings"
	"testing"
)

// testai-credexec-1.0 脚本契约（凭据 env 槽 e2e 驱动）。
func TestCredExecModel(t *testing.T) {
	m := NewCredExecModel()
	if m.Name() != "testai-credexec-1.0" {
		t.Fatalf("name = %q", m.Name())
	}

	// 用户轮：标记 → exec 调用，command+env 逐字透传（arguments 是内嵌
	// JSON 字符串，引号在响应里是 \" 转义形态——断言不带引号）。
	resp := m.Process([]Message{{Role: "user", Content: `drive <CRED_EXEC>{"command":"printenv K","env":{"K":"vault:alias"}}</CRED_EXEC> now`}})
	if !strings.Contains(resp, "exec") || !strings.Contains(resp, "env") || !strings.Contains(resp, "vault:alias") {
		t.Fatalf("user round should emit exec call with env verbatim, got: %s", resp)
	}

	// 无 env 的标记也合法（纯 command）。
	resp = m.Process([]Message{{Role: "user", Content: `<CRED_EXEC>{"command":"echo hi"}</CRED_EXEC>`}})
	if !strings.Contains(resp, "exec") || strings.Contains(resp, "env") {
		t.Fatalf("command-only marker should omit env, got: %s", resp)
	}

	// 畸形标记 → 明确报错不静默。
	if resp := m.Process([]Message{{Role: "user", Content: `<CRED_EXEC>{bad}</CRED_EXEC>`}}); resp != "BAD_CRED_EXEC_TAG" {
		t.Fatalf("malformed marker: %s", resp)
	}

	// 工具轮：回首行。
	resp = m.Process([]Message{{Role: "tool", Content: "LEN 5 SHA12 abc\nsecond line"}})
	if resp != "CRED_DONE: LEN 5 SHA12 abc" {
		t.Fatalf("tool round should echo first line, got: %s", resp)
	}

	// 无标记 → 使用提示（不静默挂死）。
	if resp := m.Process([]Message{{Role: "user", Content: "hi"}}); !strings.Contains(resp, "CRED_EXEC") {
		t.Fatalf("no marker should return usage hint, got: %s", resp)
	}
}
