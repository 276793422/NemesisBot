package handlers

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"testaiserver/logger"
	"testaiserver/models"
	"time"

	"github.com/gin-gonic/gin"
)

// Handler 处理 HTTP 请求
type Handler struct {
	registry *models.ModelRegistry
	logger   *logger.Logger
}

// NewHandler 创建新的处理器
func NewHandler(registry *models.ModelRegistry, log *logger.Logger) *Handler {
	return &Handler{
		registry: registry,
		logger:   log,
	}
}

// ListModels 列出所有可用模型
func (h *Handler) ListModels(c *gin.Context) {
	modelList := h.registry.List()
	modelInfos := make([]models.ModelInfo, 0, len(modelList))

	for _, model := range modelList {
		modelInfos = append(modelInfos, models.ModelInfo{
			ID:      model.Name(),
			Object:  "model",
			Created: time.Now().Unix(),
			OwnedBy: "test-ai-server",
		})
	}

	response := models.ModelsListResponse{
		Object: "list",
		Data:   modelInfos,
	}

	c.JSON(http.StatusOK, response)
}

// ChatCompletions 处理聊天补全请求
func (h *Handler) ChatCompletions(c *gin.Context) {
	// 读取原始请求体（用于日志记录）
	rawBody, err := io.ReadAll(c.Request.Body)
	if err != nil {
		c.JSON(http.StatusBadRequest, gin.H{
			"error": gin.H{
				"message": "Failed to read request body",
				"type":    "invalid_request_error",
				"code":    "read_body_failed",
			},
		})
		return
	}

	// 恢复请求体以供后续使用
	c.Request.Body = io.NopCloser(bytes.NewBuffer(rawBody))

	var req models.ChatCompletionRequest
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{
			"error": gin.H{
				"message": "Invalid request format",
				"type":    "invalid_request_error",
				"code":    "invalid_json",
			},
		})
		return
	}

	// 获取模型
	model, exists := h.registry.Get(req.Model)
	if !exists {
		c.JSON(http.StatusNotFound, gin.H{
			"error": gin.H{
				"message": fmt.Sprintf("Model '%s' not found", req.Model),
				"type":    "invalid_request_error",
				"code":    "model_not_found",
			},
		})
		return
	}

	// 记录请求日志（在处理之前）
	if h.logger != nil {
		if err := h.logger.LogRequestDetails(c, req.Model, rawBody); err != nil {
			// 日志记录失败不应该影响请求处理，只记录错误
			fmt.Printf("记录请求日志失败: %v\n", err)
		}
	}

	// 处理延迟
	if delay := model.Delay(); delay > 0 {
		time.Sleep(delay)
	}

	// 处理消息
	responseContent := model.Process(req.Messages)

	// 尝试解析为 ProcessedResponse（可能包含工具调用）
	var processedResp models.ProcessedResponse
	if err := json.Unmarshal([]byte(responseContent), &processedResp); err == nil {
		// 成功解析为 ProcessedResponse
		// 如果包含 ToolCalls，则使用它们
		if len(processedResp.ToolCalls) > 0 || processedResp.Content != "" {
			responseContent = processedResp.Content

			// 如果有工具调用，添加到响应消息中
			if len(processedResp.ToolCalls) > 0 {
				// 非流式响应
				if !req.Stream {
					response := models.ChatCompletionResponse{
						ID:      fmt.Sprintf("chatcmpl-%d", time.Now().UnixNano()),
						Object:  "chat.completion",
						Created: time.Now().Unix(),
						Model:   model.Name(),
						Choices: []models.Choice{
							{
								Index: 0,
								Message: models.Message{
									Role:      "assistant",
									Content:   responseContent,
									ToolCalls: processedResp.ToolCalls,
								},
								FinishReason: "tool_calls",
							},
						},
						Usage: models.Usage{
							PromptTokens:     h.countTokens(req.Messages),
							CompletionTokens: len(responseContent) + len(processedResp.ToolCalls)*50,
							TotalTokens:      h.countTokens(req.Messages) + len(responseContent) + len(processedResp.ToolCalls)*50,
						},
					}
					c.JSON(http.StatusOK, response)
					return
				}

				// 流式响应
				h.handleStreamingResponseWithTools(c, model.Name(), responseContent, processedResp.ToolCalls)
				return
			}
		}
	}

	// 普通文本响应（不是 ProcessedResponse 或没有工具调用）

	// 如果请求流式响应，使用 SSE 格式
	if req.Stream {
		h.handleStreamingResponse(c, model.Name(), responseContent)
		return
	}

	// 非流式响应（原有逻辑）
	response := models.ChatCompletionResponse{
		ID:      fmt.Sprintf("chatcmpl-%d", time.Now().UnixNano()),
		Object:  "chat.completion",
		Created: time.Now().Unix(),
		Model:   model.Name(),
		Choices: []models.Choice{
			{
				Index: 0,
				Message: models.Message{
					Role:    "assistant",
					Content: responseContent,
				},
				FinishReason: "stop",
			},
		},
		Usage: models.Usage{
			PromptTokens:     h.countTokens(req.Messages),
			CompletionTokens: len(responseContent),
			TotalTokens:      h.countTokens(req.Messages) + len(responseContent),
		},
	}

	c.JSON(http.StatusOK, response)
}

// handleStreamingResponse 处理流式响应（SSE）
func (h *Handler) handleStreamingResponse(c *gin.Context, modelName, content string) {
	// 设置 SSE headers
	c.Header("Content-Type", "text/event-stream")
	c.Header("Cache-Control", "no-cache")
	c.Header("Connection", "keep-alive")
	c.Header("Access-Control-Allow-Origin", "*")

	// 生成唯一的响应 ID
	chatID := fmt.Sprintf("chatcmpl-%d", time.Now().UnixNano())
	created := time.Now().Unix()

	// 1. 发送角色信息（role: assistant）
	roleChunk := models.StreamChunk{
		ID:      chatID,
		Object:  "chat.completion.chunk",
		Created: created,
		Model:   modelName,
		Choices: []models.StreamChoice{
			{
				Index: 0,
				Delta: models.Delta{
					Role:    "assistant",
					Content: "",
				},
				FinishReason: nil,
			},
		},
	}
	h.sendSSEChunk(c.Writer, roleChunk)

	// 2. 发送内容（分字符发送以模拟流式效果）
	for _, char := range content {
		contentChunk := models.StreamChunk{
			ID:      chatID,
			Object:  "chat.completion.chunk",
			Created: created,
			Model:   modelName,
			Choices: []models.StreamChoice{
				{
					Index: 0,
					Delta: models.Delta{
						Content: string(char),
					},
					FinishReason: nil,
				},
			},
		}
		h.sendSSEChunk(c.Writer, contentChunk)
		// 小延迟，模拟打字效果
		time.Sleep(10 * time.Millisecond)
	}

	// 3. 发送完成标记
	finishChunk := models.StreamChunk{
		ID:      chatID,
		Object:  "chat.completion.chunk",
		Created: created,
		Model:   modelName,
		Choices: []models.StreamChoice{
			{
				Index: 0,
				Delta: models.Delta{},
				FinishReason: func() *string {
					s := "stop"
					return &s
				}(),
			},
		},
	}
	h.sendSSEChunk(c.Writer, finishChunk)

	// 4. 发送 [DONE] 标记
	c.Writer.Write([]byte("data: [DONE]\n\n"))
	c.Writer.Flush()
}

// sendSSEChunk 发送单个 SSE 数据块
func (h *Handler) sendSSEChunk(w gin.ResponseWriter, chunk models.StreamChunk) {
	data, err := json.Marshal(chunk)
	if err != nil {
		fmt.Printf("[ERROR] Failed to marshal SSE chunk: %v\n", err)
		return
	}
	w.Write([]byte("data: "))
	w.Write(data)
	w.Write([]byte("\n\n"))
	w.Flush()
}

// handleStreamingResponseWithTools 处理带工具调用的流式响应
func (h *Handler) handleStreamingResponseWithTools(c *gin.Context, modelName, content string, toolCalls []models.ToolCall) {
	// 设置 SSE headers
	c.Header("Content-Type", "text/event-stream")
	c.Header("Cache-Control", "no-cache")
	c.Header("Connection", "keep-alive")
	c.Header("Access-Control-Allow-Origin", "*")

	// 生成唯一的响应 ID
	chatID := fmt.Sprintf("chatcmpl-%d", time.Now().UnixNano())
	created := time.Now().Unix()

	// 1. 发送角色信息
	roleChunk := models.StreamChunk{
		ID:      chatID,
		Object:  "chat.completion.chunk",
		Created: created,
		Model:   modelName,
		Choices: []models.StreamChoice{
			{
				Index: 0,
				Delta: models.Delta{
					Role:    "assistant",
					Content: "",
				},
				FinishReason: nil,
			},
		},
	}
	h.sendSSEChunk(c.Writer, roleChunk)

	// 2. 发送内容（如果有）
	if content != "" {
		for _, char := range content {
			contentChunk := models.StreamChunk{
				ID:      chatID,
				Object:  "chat.completion.chunk",
				Created: created,
				Model:   modelName,
				Choices: []models.StreamChoice{
					{
						Index: 0,
						Delta: models.Delta{
							Content: string(char),
						},
						FinishReason: nil,
					},
				},
			}
			h.sendSSEChunk(c.Writer, contentChunk)
			time.Sleep(10 * time.Millisecond)
		}
	}

	// 3. 发送工具调用
	if len(toolCalls) > 0 {
		// 发送工具调用开始
		toolCallStartChunk := models.StreamChunk{
			ID:      chatID,
			Object:  "chat.completion.chunk",
			Created: created,
			Model:   modelName,
			Choices: []models.StreamChoice{
				{
					Index: 0,
					Delta: models.Delta{
						ToolCalls: toolCalls,
					},
					FinishReason: nil,
				},
			},
		}
		h.sendSSEChunk(c.Writer, toolCallStartChunk)
	}

	// 4. 发送完成标记
	finishReason := "stop"
	if len(toolCalls) > 0 {
		finishReason = "tool_calls"
	}
	finishChunk := models.StreamChunk{
		ID:      chatID,
		Object:  "chat.completion.chunk",
		Created: created,
		Model:   modelName,
		Choices: []models.StreamChoice{
			{
				Index:        0,
				Delta:        models.Delta{},
				FinishReason: &finishReason,
			},
		},
	}
	h.sendSSEChunk(c.Writer, finishChunk)

	// 5. 发送 [DONE] 标记
	c.Writer.Write([]byte("data: [DONE]\n\n"))
	c.Writer.Flush()
}

// Help 显示帮助信息
func (h *Handler) Help(c *gin.Context) {
	helpText := `# TestAIServer - 测试模型服务器

## 可用模型列表

### 1. testai-1.1 - 快速响应模型
- **延迟**: 0 秒
- **功能**: 立即返回固定响应 "好的，我知道了"
- **用途**: 测试基本的消息处理流程

### 2. testai-1.2 - 中等延迟模型
- **延迟**: 30 秒
- **功能**: 延迟 30 秒后返回固定响应 "好的，我知道了"
- **用途**: 测试中等延迟场景

### 3. testai-1.3 - 长延迟模型
- **延迟**: 300 秒 (5 分钟)
- **功能**: 延迟 300 秒后返回固定响应 "好的，我知道了"
- **用途**: 测试长延迟场景、超时配置
- **场景**: 模拟长时间 LLM 处理，测试 28/29/30 分钟超时配置

### 4. testai-2.0 - 回显模型
- **延迟**: 0 秒
- **功能**: 原样返回用户发送的最后一条消息
- **用途**: 测试消息传递是否正确

### 5. testai-3.0 - 工具调用模型 ⭐

**延迟**: 0 秒

**功能**: 根据消息内容决定行为

#### 场景 A: 检测到 PEER_CHAT 标记
- **触发条件**: 消息包含 <PEER_CHAT>...</PEER_CHAT>
- **返回**: cluster_rpc 工具调用
- **Agent 会执行工具，发送 peer_chat RPC

#### 场景 B: 普通消息
- **触发条件**: 消息不包含 PEER_CHAT 标记
- **返回**: 原样返回用户消息
- **无工具调用**

**用途**: 测试 peer_chat 工具调用、集群通信、长超时配置

---

## testai-3.0 详细使用说明

### 消息格式

要触发 peer_chat 工具调用，需要在消息中包含特殊标记:

` + "```" + `
<PEER_CHAT>{JSON内容}</PEER_CHAT>
` + "```" + `

### JSON 结构

` + "```" + `{
  "peer_id": "目标Agent的ID",
  "content": "要发送给目标Agent的消息内容"
}
` + "```" + `

### 示例

**示例 1 - 单行格式:**
` + "```" + `
<PEER_CHAT>{"peer_id":"agent-b","content":"你好，我是 Agent A"}</PEER_CHAT>
` + "```" + `

**示例 2 - 多行格式:**
` + "```" + `
<PEER_CHAT>
{
  "peer_id": "agent-b",
  "content": "这是一条测试消息"
}
</PEER_CHAT>
` + "```" + `

---

## 测试场景示例

### 场景 1: 测试基本 peer_chat

**配置:**
- Agent A: test/testai-3.0
- Agent B: test/testai-1.1 (快速响应)

**测试消息:**
` + "```" + `
请给 agent-b 发送消息: <PEER_CHAT>{"peer_id":"agent-b","content":"测试消息"}</PEER_CHAT>
` + "```" + `

**预期结果:**
- Agent A 立即返回工具调用
- Agent B 立即处理并返回
- 总耗时: < 5 秒

### 场景 2: 测试长延迟 peer_chat (关键测试)

**配置:**
- Agent A: test/testai-3.0
- Agent B: test/testai-1.3 (300秒延迟)
- 超时: RPC Client=30分钟, PeerChatHandler=29分钟, RPCChannel=28分钟

**测试消息:**
` + "```" + `
请给 agent-b 发送消息: <PEER_CHAT>{"peer_id":"agent-b","content":"测试长延迟"}</PEER_CHAT>
` + "```" + `

**预期结果:**
- Agent A 立即发送 RPC
- Agent B 在 300 秒后返回
- Agent A 收到响应 (在超时范围内)
- 总耗时: ~300 秒 (5 分钟)

**验证点:**
- ✅ RPCChannel cleanup 不会提前关闭 channel
- ✅ PeerChatHandler 的 29分钟超时不会触发
- ✅ RPC Client 的 30分钟超时不会触发
- ✅ 响应能正常返回

---

## API 端点

- GET  /v1/models           - 列出所有可用模型
- POST /v1/chat/completions - 聊天补全接口
- GET  /v1/help             - 显示此帮助信息

---

## 启动服务器

` + "```" + `
# 显示详细帮助
./testaiserver.exe --help

# 启动服务器
./testaiserver.exe
` + "```" + `

---

## NemesisBot 配置示例

**配置 Agent A (使用 testai-3.0):**
` + "```" + `
# 添加模型
nemesisbot model add \
  --model test/testai-3.0 \
  --base http://127.0.0.1:8080/v1 \
  --key test-key \
  --default

# 配置集群
nemesisbot cluster init --name "Agent-A" --role worker
nemesisbot cluster enable
` + "```" + `

**配置 Agent B (使用 testai-1.3):**
` + "```" + `
# 添加模型
nemesisbot model add \
  --model test/testai-1.3 \
  --base http://127.0.0.1:8080/v1 \
  --key test-key \
  --default

# 配置集群
nemesisbot cluster init --name "Agent-B" --role worker
nemesisbot cluster enable
` + "```" + `

---

## 日志位置

- TestAIServer 日志: ./log/
- NemesisBot 日志: ~/.nemesisbot/logs/
`

	c.Header("Content-Type", "text/markdown; charset=utf-8")
	c.String(http.StatusOK, helpText)
}

// countTokens 简单的 token 计数（按字符数估算）
func (h *Handler) countTokens(messages []models.Message) int {
	count := 0
	for _, msg := range messages {
		count += len(msg.Content)
	}
	return count
}
