package main

import (
	"fmt"
	"testaiserver/handlers"
	"testaiserver/logger"
	"testaiserver/models"

	"github.com/gin-gonic/gin"
)

func main() {
	// 初始化日志记录器
	log, err := logger.NewLogger()
	if err != nil {
		fmt.Printf("初始化日志记录器失败: %v\n", err)
		return
	}
	fmt.Println("日志目录已创建: log/")

	// 初始化模型注册表
	registry := models.NewModelRegistry()

	// 注册测试模型
	registry.Register(models.NewTestAI11())
	registry.Register(models.NewTestAI12())
	registry.Register(models.NewTestAI13())
	registry.Register(models.NewTestAI20())
	fmt.Println("测试模型已注册: testai-1.1, testai-1.2, testai-1.3, testai-2.0")

	// 创建 Gin 路由
	router := gin.Default()

	// 创建处理器
	handler := handlers.NewHandler(registry, log)

	// 注册路由
	v1 := router.Group("/v1")
	{
		v1.GET("/models", handler.ListModels)
		v1.POST("/chat/completions", handler.ChatCompletions)
	}

	// 启动信息
	fmt.Println("========================================")
	fmt.Println("TestAIServer 正在启动...")
	fmt.Println("========================================")
	fmt.Println("服务地址: http://0.0.0.0:8080")
	fmt.Println("日志目录: ./log/")
	fmt.Println("========================================")

	// 启动服务器（监听所有网络接口）
	if err := router.Run("0.0.0.0:8080"); err != nil {
		fmt.Printf("服务器启动失败: %v\n", err)
	}
}
