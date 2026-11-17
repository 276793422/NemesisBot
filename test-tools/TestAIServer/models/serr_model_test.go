package models

import (
	"strings"
	"testing"
)

// LLM-ERR-A 流内错误事件模型对的单测：契约面（奇偶交替 / 载荷形态 /
// partial 不泄漏）。活体链路（SSE error 事件 → 客户端分类）由 Rust 侧
// 驱动测试覆盖，这里只钉模型自身语义。
func TestStreamErrorerAlternation(t *testing.T) {
	code := NewTestAIStreamErrCode()
	nocode := NewTestAIStreamErrNoCode()

	for _, m := range []interface {
		Model
		ShouldError() bool
	}{code, nocode} {
		name := m.Name()
		// 奇数次（1/3）应 error，偶数次（2/4）应正常。
		if !m.ShouldError() {
			t.Fatalf("%s: 第 1 次请求应 ShouldError=true", name)
		}
		if m.ShouldError() {
			t.Fatalf("%s: 第 2 次请求应 ShouldError=false", name)
		}
		if !m.ShouldError() {
			t.Fatalf("%s: 第 3 次请求应 ShouldError=true", name)
		}
	}
}

func TestStreamErrorPayloads(t *testing.T) {
	// 带码变体：code=429 数字在场，message 无词表词（证据链隔离——修复后
	// 进限流环唯一解释是码采集，不是文本兜底）。
	codePayload := NewTestAIStreamErrCode().ErrorPayload()
	if want := `{"error":{"code":429,"message":"SERR_CODE_429_MARK upstream busy, try later"}}`; codePayload != want {
		t.Fatalf("code 变体载荷形态漂移:\n got: %s\nwant: %s", codePayload, want)
	}

	// 无码变体：不带 code 键，message 复刻 cc-switch 实测文案。
	nocodePayload := NewTestAIStreamErrNoCode().ErrorPayload()
	if want := `{"error":{"message":"Concurrency limit exceeded for account"}}`; nocodePayload != want {
		t.Fatalf("nocode 变体载荷形态漂移:\n got: %s\nwant: %s", nocodePayload, want)
	}
}

func TestStreamErrorPartialAndReplyDisjoint(t *testing.T) {
	// 偶数次（重试成功轮）回复不得携带 partial marker（partial 不泄漏断言）。
	code := NewTestAIStreamErrCode()
	nocode := NewTestAIStreamErrNoCode()
	for _, m := range []Model{code, nocode} {
		if reply := m.Process(nil); strings.Contains(reply, "SERR_PARTIAL") {
			t.Fatalf("%s: 成功回复泄漏 partial marker: %s", m.Name(), reply)
		}
	}
}
