package handlers

import (
	"bytes"
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

	// 如果请求流式响应，返回错误（暂不支持）
	if req.Stream {
		c.JSON(http.StatusBadRequest, gin.H{
			"error": gin.H{
				"message": "Streaming is not supported by test models",
				"type":    "invalid_request_error",
				"code":    "streaming_not_supported",
			},
		})
		return
	}

	// 处理延迟
	if delay := model.Delay(); delay > 0 {
		time.Sleep(delay)
	}

	// 处理消息
	responseContent := model.Process(req.Messages)

	// 构建响应
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

// countTokens 简单的 token 计数（按字符数估算）
func (h *Handler) countTokens(messages []models.Message) int {
	count := 0
	for _, msg := range messages {
		count += len(msg.Content)
	}
	return count
}
