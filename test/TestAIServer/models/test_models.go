package models

import "time"

// TestAI11 - 立即返回固定响应
type TestAI11 struct{}

func NewTestAI11() *TestAI11 {
	return &TestAI11{}
}

func (m *TestAI11) Name() string {
	return "testai-1.1"
}

func (m *TestAI11) Process(messages []Message) string {
	return "好的，我知道了"
}

func (m *TestAI11) Delay() time.Duration {
	return 0
}

// TestAI12 - 延迟 30 秒返回固定响应
type TestAI12 struct{}

func NewTestAI12() *TestAI12 {
	return &TestAI12{}
}

func (m *TestAI12) Name() string {
	return "testai-1.2"
}

func (m *TestAI12) Process(messages []Message) string {
	return "好的，我知道了"
}

func (m *TestAI12) Delay() time.Duration {
	return 30 * time.Second
}

// TestAI13 - 延迟 300 秒返回固定响应
type TestAI13 struct{}

func NewTestAI13() *TestAI13 {
	return &TestAI13{}
}

func (m *TestAI13) Name() string {
	return "testai-1.3"
}

func (m *TestAI13) Process(messages []Message) string {
	return "好的，我知道了"
}

func (m *TestAI13) Delay() time.Duration {
	return 300 * time.Second
}

// TestAI20 - 原样返回用户消息
type TestAI20 struct{}

func NewTestAI20() *TestAI20 {
	return &TestAI20{}
}

func (m *TestAI20) Name() string {
	return "testai-2.0"
}

func (m *TestAI20) Process(messages []Message) string {
	// 返回最后一条用户消息
	if len(messages) > 0 {
		return messages[len(messages)-1].Content
	}
	return ""
}

func (m *TestAI20) Delay() time.Duration {
	return 0
}
