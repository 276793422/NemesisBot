package logger

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"testaiserver/models"
	"time"

	"github.com/gin-gonic/gin"
)

// Logger 日志记录器
type Logger struct {
	baseDir string
}

// NewLogger 创建新的日志记录器
func NewLogger() (*Logger, error) {
	// 获取当前工作目录
	currentDir, err := os.Getwd()
	if err != nil {
		return nil, fmt.Errorf("获取当前目录失败: %w", err)
	}

	// 创建 log 目录
	logDir := filepath.Join(currentDir, "log")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		return nil, fmt.Errorf("创建 log 目录失败: %w", err)
	}

	return &Logger{
		baseDir: logDir,
	}, nil
}

// LogRequest 记录请求日志
func (l *Logger) LogRequest(c *gin.Context, modelName string, reqBody *models.ChatCompletionRequest) error {
	// 创建模型目录
	modelDir := filepath.Join(l.baseDir, modelName)
	if err := os.MkdirAll(modelDir, 0755); err != nil {
		return fmt.Errorf("创建模型目录失败: %w", err)
	}

	// 生成日志文件名（使用当前时间）
	timestamp := time.Now().Format("20060102_150405.000")
	logFileName := fmt.Sprintf("%s.log", timestamp)
	logFilePath := filepath.Join(modelDir, logFileName)

	// 创建日志文件
	file, err := os.Create(logFilePath)
	if err != nil {
		return fmt.Errorf("创建日志文件失败: %w", err)
	}
	defer file.Close()

	// 构建日志内容
	logContent := l.buildLogContent(c, reqBody)

	// 写入日志
	if _, err := file.WriteString(logContent); err != nil {
		return fmt.Errorf("写入日志失败: %w", err)
	}

	return nil
}

// buildLogContent 构建日志内容
func (l *Logger) buildLogContent(c *gin.Context, reqBody *models.ChatCompletionRequest) string {
	var content string

	// 写入分隔线
	content += "========================================\n"
	content += "TestAIServer Request Log\n"
	content += "========================================\n\n"

	// 时间戳
	content += fmt.Sprintf("Timestamp: %s\n", time.Now().Format("2006-01-02 15:04:05.000"))
	content += "\n"

	// 请求信息
	content += "--- Request Info ---\n"
	content += fmt.Sprintf("Method: %s\n", c.Request.Method)
	content += fmt.Sprintf("URL: %s\n", c.Request.URL.String())
	content += fmt.Sprintf("Protocol: %s\n", c.Request.Proto)
	content += fmt.Sprintf("Remote Addr: %s\n", c.Request.RemoteAddr)
	content += "\n"

	// 请求头
	content += "--- Request Headers ---\n"
	for key, values := range c.Request.Header {
		for _, value := range values {
			content += fmt.Sprintf("%s: %s\n", key, value)
		}
	}
	content += "\n"

	// 查询参数
	if len(c.Request.URL.Query()) > 0 {
		content += "--- Query Parameters ---\n"
		for key, values := range c.Request.URL.Query() {
			for _, value := range values {
				content += fmt.Sprintf("%s: %s\n", key, value)
			}
		}
		content += "\n"
	}

	// 请求体（JSON 格式）
	if reqBody != nil {
		content += "--- Request Body ---\n"
		jsonData, err := json.MarshalIndent(reqBody, "", "  ")
		if err != nil {
			content += fmt.Sprintf("Error marshaling request body: %v\n", err)
		} else {
			content += string(jsonData) + "\n"
		}
		content += "\n"
	}

	// 模型信息
	if reqBody != nil {
		content += "--- Model Info ---\n"
		content += fmt.Sprintf("Model: %s\n", reqBody.Model)
		content += fmt.Sprintf("Stream: %v\n", reqBody.Stream)
		content += fmt.Sprintf("Messages Count: %d\n", len(reqBody.Messages))
		if len(reqBody.Messages) > 0 {
			content += "\nMessages:\n"
			for i, msg := range reqBody.Messages {
				content += fmt.Sprintf("  [%d] Role: %s\n", i, msg.Role)
				content += fmt.Sprintf("      Content: %s\n", msg.Content)
			}
		}
		content += "\n"
	}

	// Gin 上下文信息
	content += "--- Gin Context ---\n"
	content += fmt.Sprintf("Client IP: %s\n", c.ClientIP())
	content += fmt.Sprintf("Content Length: %d\n", c.Request.ContentLength)
	content += fmt.Sprintf("Content Type: %s\n", c.ContentType())
	content += "\n"

	// 结束分隔线
	content += "========================================\n"
	content += "End of Log\n"
	content += "========================================\n"

	return content
}

// LogRequestDetails 记录更详细的请求信息（包括原始 body）
func (l *Logger) LogRequestDetails(c *gin.Context, modelName string, rawBody []byte) error {
	// 创建模型目录
	modelDir := filepath.Join(l.baseDir, modelName)
	if err := os.MkdirAll(modelDir, 0755); err != nil {
		return fmt.Errorf("创建模型目录失败: %w", err)
	}

	// 生成日志文件名
	timestamp := time.Now().Format("20060102_150405.000")
	logFileName := fmt.Sprintf("%s.log", timestamp)
	logFilePath := filepath.Join(modelDir, logFileName)

	// 创建日志文件
	file, err := os.Create(logFilePath)
	if err != nil {
		return fmt.Errorf("创建日志文件失败: %w", err)
	}
	defer file.Close()

	// 构建日志内容
	content := l.buildDetailedLogContent(c, rawBody)

	// 写入日志
	if _, err := file.WriteString(content); err != nil {
		return fmt.Errorf("写入日志失败: %w", err)
	}

	return nil
}

// buildDetailedLogContent 构建详细日志内容
func (l *Logger) buildDetailedLogContent(c *gin.Context, rawBody []byte) string {
	var content string

	// 写入分隔线
	content += "========================================\n"
	content += "TestAIServer Request Log (Detailed)\n"
	content += "========================================\n\n"

	// 时间戳
	content += fmt.Sprintf("Timestamp: %s\n", time.Now().Format("2006-01-02 15:04:05.000"))
	content += "\n"

	// 请求信息
	content += "--- Request Info ---\n"
	content += fmt.Sprintf("Method: %s\n", c.Request.Method)
	content += fmt.Sprintf("URL: %s\n", c.Request.URL.String())
	content += fmt.Sprintf("Protocol: %s\n", c.Request.Proto)
	content += fmt.Sprintf("Remote Addr: %s\n", c.Request.RemoteAddr)
	content += fmt.Sprintf("Host: %s\n", c.Request.Host)
	content += "\n"

	// 请求头
	content += "--- Request Headers ---\n"
	for key, values := range c.Request.Header {
		for _, value := range values {
			content += fmt.Sprintf("%s: %s\n", key, value)
		}
	}
	content += "\n"

	// 查询参数
	if len(c.Request.URL.Query()) > 0 {
		content += "--- Query Parameters ---\n"
		for key, values := range c.Request.URL.Query() {
			for _, value := range values {
				content += fmt.Sprintf("%s: %s\n", key, value)
			}
		}
		content += "\n"
	}

	// 原始请求体
	if len(rawBody) > 0 {
		content += "--- Raw Request Body ---\n"
		content += fmt.Sprintf("Length: %d bytes\n", len(rawBody))
		content += "\n"

		// 尝试格式化为 JSON
		var jsonBody interface{}
		if err := json.Unmarshal(rawBody, &jsonBody); err == nil {
			formatted, _ := json.MarshalIndent(jsonBody, "", "  ")
			content += string(formatted) + "\n"
		} else {
			// 如果不是 JSON，直接输出
			content += string(rawBody) + "\n"
		}
		content += "\n"
	}

	// Gin 上下文信息
	content += "--- Gin Context ---\n"
	content += fmt.Sprintf("Client IP: %s\n", c.ClientIP())
	content += fmt.Sprintf("Content Length: %d\n", c.Request.ContentLength)
	content += fmt.Sprintf("Content Type: %s\n", c.ContentType())
	content += fmt.Sprintf("User Agent: %s\n", c.Request.UserAgent())
	content += fmt.Sprintf("Is AJAX: %v\n", c.IsWebsocket())
	content += "\n"

	// 读取请求体的副本（用于调试）
	if c.Request.Body != nil {
		bodyCopy, _ := io.ReadAll(c.Request.Body)
		if len(bodyCopy) > 0 {
			content += "--- Body Copy ---\n"
			content += string(bodyCopy) + "\n"
			content += "\n"
		}
	}

	// 结束分隔线
	content += "========================================\n"
	content += "End of Log\n"
	content += "========================================\n"

	return content
}
