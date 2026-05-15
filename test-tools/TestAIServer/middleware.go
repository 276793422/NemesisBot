package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"

	"github.com/gin-gonic/gin"
)

// ModelNameMiddleware 是一个中间件，用于提取请求中的模型名称
// 并将其存储在 gin.Context 中，以便日志中间件使用
func ModelNameMiddleware() gin.HandlerFunc {
	return func(c *gin.Context) {
		// 只处理 /v1/chat/completions 路径
		if c.Request.URL.Path == "/v1/chat/completions" && c.Request.Method == "POST" {
			// 读取请求体
			rawBody, err := io.ReadAll(c.Request.Body)
			if err != nil {
				c.Next()
				return
			}

			// 恢复请求体以供后续使用
			c.Request.Body = io.NopCloser(bytes.NewBuffer(rawBody))

			// 尝试解析模型名称
			var req struct {
				Model string `json:"model"`
			}
			if err := json.Unmarshal(rawBody, &req); err == nil && req.Model != "" {
				// 将模型名称存储在 Context 中
				c.Set("model_name", req.Model)
			}
		}

		c.Next()
	}
}

// CustomLogger 是自定义的日志中间件，在日志中包含模型名称
func CustomLogger() gin.HandlerFunc {
	return gin.LoggerWithFormatter(func(param gin.LogFormatterParams) string {
		// 获取模型名称
		modelName, exists := param.Keys["model_name"]
		if !exists {
			modelName = "-"
		}

		// 自定义日志格式
		return fmt.Sprintf("[GIN] %v | %3d | %13v | %15s | %-7s %s | %s\n",
			param.TimeStamp.Format("2006/01/02 - 15:04:05"),
			param.StatusCode,
			param.Latency,
			param.ClientIP,
			param.Method,
			param.Path,
			modelName,
		)
	})
}

// 注意：需要导入 fmt 包
// import "fmt"
