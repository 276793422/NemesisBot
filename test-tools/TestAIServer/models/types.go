package models

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// Model 定义测试模型接口
type Model interface {
	// Name 返回模型名称
	Name() string

	// Process 处理消息并返回响应
	Process(messages []Message) string

	// Delay 返回延迟时间（秒）
	Delay() time.Duration
}

// ProcessedResponse 定义处理后的响应（可能包含工具调用）
type ProcessedResponse struct {
	Content   string     `json:"content,omitempty"`
	ToolCalls []ToolCall `json:"tool_calls,omitempty"`
}

// Message 定义聊天消息
type Message struct {
	Role      string     `json:"role"`
	Content   string     `json:"content"`
	ToolCalls []ToolCall `json:"tool_calls,omitempty"`
	// Parts 保存多模态 content 数组的完整内容（UnmarshalJSON 填充）；
	// json:"-" 不回传——响应侧 assistant 消息恒为纯文本。
	Parts []ContentPart `json:"-"`
}

// ContentPart 定义 OpenAI 兼容的多模态内容部分
type ContentPart struct {
	Type     string    `json:"type"` // "text" | "image_url"
	Text     string    `json:"text,omitempty"`
	ImageURL *ImageURL `json:"image_url,omitempty"`
}

// ImageURL 定义图像引用（base64 data URI 或 http URL）
type ImageURL struct {
	URL    string `json:"url"`
	Detail string `json:"detail,omitempty"`
}

// UnmarshalJSON 兼容两种 content 形态：纯字符串 / 多模态数组。
// 数组形态下文本部分拼接进 Content（现有模型零改动继续工作），
// 完整 parts 保留在 Parts 供视觉检测模型（testai-vision-1.0）断言。
func (m *Message) UnmarshalJSON(data []byte) error {
	type alias struct {
		Role      string          `json:"role"`
		Content   json.RawMessage `json:"content"`
		ToolCalls []ToolCall      `json:"tool_calls,omitempty"`
	}
	var a alias
	if err := json.Unmarshal(data, &a); err != nil {
		return err
	}
	m.Role = a.Role
	m.ToolCalls = a.ToolCalls
	m.Parts = nil

	if len(a.Content) == 0 || string(a.Content) == "null" {
		m.Content = ""
		return nil
	}

	// 形态 1：纯字符串
	var s string
	if err := json.Unmarshal(a.Content, &s); err == nil {
		m.Content = s
		return nil
	}

	// 形态 2：多模态数组
	var parts []ContentPart
	if err := json.Unmarshal(a.Content, &parts); err == nil {
		m.Parts = parts
		var texts []string
		for _, p := range parts {
			if p.Type == "text" {
				texts = append(texts, p.Text)
			}
		}
		m.Content = strings.Join(texts, "\n")
		return nil
	}

	return fmt.Errorf("content 字段既不是字符串也不是数组")
}

// ChatCompletionRequest 定义 OpenAI 兼容的请求格式
type ChatCompletionRequest struct {
	Model    string    `json:"model"`
	Messages []Message `json:"messages"`
	Stream   bool      `json:"stream,omitempty"`
}

// ChatCompletionResponse 定义 OpenAI 兼容的响应格式
type ChatCompletionResponse struct {
	ID      string   `json:"id"`
	Object  string   `json:"object"`
	Created int64    `json:"created"`
	Model   string   `json:"model"`
	Choices []Choice `json:"choices"`
	Usage   Usage    `json:"usage"`
}

// Choice 定义响应选择项
type Choice struct {
	Index        int     `json:"index"`
	Message      Message `json:"message"`
	FinishReason string  `json:"finish_reason"`
}

// Usage 定义 token 使用统计
type Usage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
	TotalTokens      int `json:"total_tokens"`
}

// StreamChunk 定义流式响应的数据块
type StreamChunk struct {
	ID      string         `json:"id"`
	Object  string         `json:"object"`
	Created int64          `json:"created"`
	Model   string         `json:"model"`
	Choices []StreamChoice `json:"choices"`
}

// StreamChoice 定义流式响应的选择项
type StreamChoice struct {
	Index        int     `json:"index"`
	Delta        Delta   `json:"delta"`
	FinishReason *string `json:"finish_reason"`
}

// Delta 定义流式响应的增量内容
type Delta struct {
	Role      string     `json:"role,omitempty"`
	Content   string     `json:"content,omitempty"`
	ToolCalls []ToolCall `json:"tool_calls,omitempty"`
}

// ToolCall 定义工具调用
type ToolCall struct {
	ID       string        `json:"id"`
	Type     string        `json:"type"`
	Function *FunctionCall `json:"function,omitempty"`
}

// FunctionCall 定义函数调用
type FunctionCall struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// ModelInfo 定义模型信息
type ModelInfo struct {
	ID      string `json:"id"`
	Object  string `json:"object"`
	Created int64  `json:"created"`
	OwnedBy string `json:"owned_by"`
}

// ModelsListResponse 定义模型列表响应
type ModelsListResponse struct {
	Object string      `json:"object"`
	Data   []ModelInfo `json:"data"`
}

// ModelRegistry 模型注册表
type ModelRegistry struct {
	models map[string]Model
	// aliases 别名表（W1.5）：TESTAI_ALIASES 配置的 别名→testai 真名 映射，
	// 只在 Get 未命中 models 时参与解析（一层，别名指向别名不支持）。
	aliases map[string]string
}

// NewModelRegistry 创建新的模型注册表
func NewModelRegistry() *ModelRegistry {
	return &ModelRegistry{
		models:  make(map[string]Model),
		aliases: make(map[string]string),
	}
}

// Register 注册模型
func (r *ModelRegistry) Register(model Model) {
	r.models[model.Name()] = model
}

// Get 获取模型。注册名未命中时回退查别名表（TESTAI_ALIASES，见 aliases.go）。
func (r *ModelRegistry) Get(name string) (Model, bool) {
	model, exists := r.models[name]
	if exists {
		return model, true
	}
	if target, ok := r.aliases[name]; ok {
		model, exists := r.models[target]
		return model, exists
	}
	return nil, false
}

// List 列出所有模型
func (r *ModelRegistry) List() []Model {
	models := make([]Model, 0, len(r.models))
	for _, model := range r.models {
		models = append(models, model)
	}
	return models
}
