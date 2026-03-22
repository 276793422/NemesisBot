package main

import (
	"flag"
	"fmt"
	"testaiserver/handlers"
	"testaiserver/logger"
	"testaiserver/models"

	"github.com/gin-gonic/gin"
)

func main() {
	// 解析命令行参数
	showHelp := flag.Bool("help", false, "显示帮助信息")
	flag.Parse()

	// 如果请求帮助，显示分层帮助系统
	if *showHelp {
		// 获取额外的参数（如：categories, models, testai-5.0 等）
		args := flag.Args()
		ShowHelp(args)
		return
	}

	// 检查是否有其他命令（如：models, api 等）
	if len(flag.Args()) > 0 {
		command := flag.Args()[0]
		switch command {
		case "models":
			ShowHelp([]string{"models"})
			return
		case "help":
			// 支持 "help" 子命令，参数从 flag.Args()[1:] 获取
			args := flag.Args()[1:]
			ShowHelp(args)
			return
		default:
			// 尝试作为模型 ID 显示帮助
			ShowHelp([]string{command})
			return
		}
	}

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
	registry.Register(models.NewTestAI30())
	registry.Register(models.NewTestAI42())
	registry.Register(models.NewTestAI43())
	registry.Register(models.NewTestAI50())
	fmt.Println("测试模型已注册: testai-1.1, testai-1.2, testai-1.3, testai-2.0, testai-3.0, testai-4.2, testai-4.3, testai-5.0")

	// 创建 Gin 路由
	router := gin.New()

	// 使用自定义中间件（替换 gin.Default()）
	router.Use(ModelNameMiddleware()) // 提取模型名称
	router.Use(CustomLogger())        // 自定义日志（包含模型名称）
	router.Use(gin.Recovery())        // 崩溃恢复

	// 创建处理器
	handler := handlers.NewHandler(registry, log)

	// 注册路由
	v1 := router.Group("/v1")
	{
		v1.GET("/models", handler.ListModels)
		v1.GET("/help", handler.Help)
		v1.POST("/chat/completions", handler.ChatCompletions)
	}

	// 启动信息
	fmt.Println("========================================")
	fmt.Println(" TestAIServer 正在启动...")
	fmt.Println("========================================")
	fmt.Println("服务地址: http://0.0.0.0:8080")
	fmt.Println("日志目录: ./log/")
	fmt.Println()
	fmt.Println("💡 帮助命令:")
	fmt.Println("   ./testaiserver.exe --help           # 显示帮助概览")
	fmt.Println("   ./testaiserver.exe --help categories # 显示分类详情")
	fmt.Println("   ./testaiserver.exe --help models    # 显示所有模型")
	fmt.Println("   ./testaiserver.exe --help testai-5.0 # 显示特定模型帮助")
	fmt.Println("   ./testaiserver.exe models           # 快捷查看模型列表")
	fmt.Println("========================================")

	// 启动服务器（监听所有网络接口）
	if err := router.Run("0.0.0.0:8080"); err != nil {
		fmt.Printf("服务器启动失败: %v\n", err)
	}
}
