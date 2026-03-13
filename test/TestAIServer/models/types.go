package models

import "time"

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
}

// NewModelRegistry 创建新的模型注册表
func NewModelRegistry() *ModelRegistry {
	return &ModelRegistry{
		models: make(map[string]Model),
	}
}

// Register 注册模型
func (r *ModelRegistry) Register(model Model) {
	r.models[model.Name()] = model
}

// Get 获取模型
func (r *ModelRegistry) Get(name string) (Model, bool) {
	model, exists := r.models[name]
	return model, exists
}

// List 列出所有模型
func (r *ModelRegistry) List() []Model {
	models := make([]Model, 0, len(r.models))
	for _, model := range r.models {
		models = append(models, model)
	}
	return models
}
