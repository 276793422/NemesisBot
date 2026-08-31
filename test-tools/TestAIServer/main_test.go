package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testaiserver/handlers"
	"testaiserver/logger"
	"testaiserver/models"
	"strings"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
)

func setupTestRouter() *gin.Engine {
	gin.SetMode(gin.TestMode)

	registry := models.NewModelRegistry()
	registry.Register(models.NewTestAI11())
	registry.Register(models.NewTestAI12())
	registry.Register(models.NewTestAI13())
	registry.Register(models.NewTestAI20())

	// 创建临时日志记录器（测试时可能不需要实际写入）
	log, _ := logger.NewLogger()
	handler := handlers.NewHandler(registry, log)

	router := gin.New()
	v1 := router.Group("/v1")
	{
		v1.GET("/models", handler.ListModels)
		v1.POST("/chat/completions", handler.ChatCompletions)
	}

	return router
}

func TestListModels(t *testing.T) {
	registry := models.NewModelRegistry()
	registry.Register(models.NewTestAI11())
	registry.Register(models.NewTestAI12())
	registry.Register(models.NewTestAI13())
	registry.Register(models.NewTestAI20())

	modelList := registry.List()

	if len(modelList) != 4 {
		t.Errorf("Expected 4 models, got %d", len(modelList))
	}

	expectedModels := []string{"testai-1.1", "testai-1.2", "testai-1.3", "testai-2.0"}
	for _, expected := range expectedModels {
		found := false
		for _, model := range modelList {
			if model.Name() == expected {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("Model %s not found in registry", expected)
		}
	}
}

func TestTestAI11(t *testing.T) {
	model := models.NewTestAI11()

	if model.Name() != "testai-1.1" {
		t.Errorf("Expected model name 'testai-1.1', got '%s'", model.Name())
	}

	messages := []models.Message{
		{Role: "user", Content: "测试消息"},
	}

	response := model.Process(messages)
	expected := "好的，我知道了"

	if response != expected {
		t.Errorf("Expected response '%s', got '%s'", expected, response)
	}

	if model.Delay() != 0 {
		t.Errorf("Expected delay 0, got %v", model.Delay())
	}
}

func TestTestAI12(t *testing.T) {
	model := models.NewTestAI12()

	if model.Name() != "testai-1.2" {
		t.Errorf("Expected model name 'testai-1.2', got '%s'", model.Name())
	}

	if model.Delay() != 30*time.Second {
		t.Errorf("Expected delay 30s, got %v", model.Delay())
	}
}

func TestTestAI13(t *testing.T) {
	model := models.NewTestAI13()

	if model.Name() != "testai-1.3" {
		t.Errorf("Expected model name 'testai-1.3', got '%s'", model.Name())
	}

	if model.Delay() != 300*time.Second {
		t.Errorf("Expected delay 300s, got %v", model.Delay())
	}
}

func TestTestAI20(t *testing.T) {
	model := models.NewTestAI20()

	if model.Name() != "testai-2.0" {
		t.Errorf("Expected model name 'testai-2.0', got '%s'", model.Name())
	}

	testCases := []struct {
		input    string
		expected string
	}{
		{"Hello, World!", "Hello, World!"},
		{"测试消息", "测试消息"},
		{"", ""},
	}

	for _, tc := range testCases {
		messages := []models.Message{
			{Role: "user", Content: tc.input},
		}

		response := model.Process(messages)
		if response != tc.expected {
			t.Errorf("Expected '%s', got '%s'", tc.expected, response)
		}
	}
}

func TestChatCompletionRequest(t *testing.T) {
	req := models.ChatCompletionRequest{
		Model: "testai-1.1",
		Messages: []models.Message{
			{Role: "user", Content: "Hello"},
		},
		Stream: false,
	}

	data, err := json.Marshal(req)
	if err != nil {
		t.Errorf("Failed to marshal request: %v", err)
	}

	var unmarshaled models.ChatCompletionRequest
	err = json.Unmarshal(data, &unmarshaled)
	if err != nil {
		t.Errorf("Failed to unmarshal request: %v", err)
	}

	if unmarshaled.Model != req.Model {
		t.Errorf("Model mismatch: expected '%s', got '%s'", req.Model, unmarshaled.Model)
	}

	if len(unmarshaled.Messages) != len(req.Messages) {
		t.Errorf("Messages length mismatch")
	}
}

func TestModelRegistry(t *testing.T) {
	registry := models.NewModelRegistry()

	// 测试注册
	model := models.NewTestAI11()
	registry.Register(model)

	// 测试获取
	retrieved, exists := registry.Get("testai-1.1")
	if !exists {
		t.Error("Model should exist in registry")
	}

	if retrieved.Name() != "testai-1.1" {
		t.Errorf("Expected 'testai-1.1', got '%s'", retrieved.Name())
	}

	// 测试不存在的模型
	_, exists = registry.Get("nonexistent")
	if exists {
		t.Error("Nonexistent model should not exist")
	}

	// 测试列表
	registry.Register(models.NewTestAI12())
	modelList := registry.List()
	if len(modelList) != 2 {
		t.Errorf("Expected 2 models, got %d", len(modelList))
	}
}

// HTTP 集成测试
func TestHTTPIntegration(t *testing.T) {
	router := setupTestRouter()

	// 测试模型列表
	req := httptest.NewRequest("GET", "/v1/models", nil)
	w := httptest.NewRecorder()
	router.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("Expected status 200, got %d", w.Code)
	}

	// 测试聊天补全
	reqBody := models.ChatCompletionRequest{
		Model: "testai-1.1",
		Messages: []models.Message{
			{Role: "user", Content: "Test"},
		},
	}

	jsonData, _ := json.Marshal(reqBody)
	req = httptest.NewRequest("POST", "/v1/chat/completions", bytes.NewBuffer(jsonData))
	req.Header.Set("Content-Type", "application/json")
	w = httptest.NewRecorder()
	router.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("Expected status 200, got %d", w.Code)
	}

	var chatResp models.ChatCompletionResponse
	err := json.Unmarshal(w.Body.Bytes(), &chatResp)
	if err != nil {
		t.Errorf("Failed to unmarshal response: %v", err)
	}

	if chatResp.Model != "testai-1.1" {
		t.Errorf("Expected model 'testai-1.1', got '%s'", chatResp.Model)
	}

	if len(chatResp.Choices) == 0 {
		t.Error("Expected at least one choice")
	}

	if chatResp.Choices[0].Message.Content != "好的，我知道了" {
		t.Errorf("Unexpected response: %s", chatResp.Choices[0].Message.Content)
	}
}

// 测试不存在的模型
func TestNonexistentModel(t *testing.T) {
	router := setupTestRouter()

	reqBody := models.ChatCompletionRequest{
		Model: "nonexistent-model",
		Messages: []models.Message{
			{Role: "user", Content: "Test"},
		},
	}

	jsonData, _ := json.Marshal(reqBody)
	req := httptest.NewRequest("POST", "/v1/chat/completions", bytes.NewBuffer(jsonData))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	router.ServeHTTP(w, req)

	if w.Code != http.StatusNotFound {
		t.Errorf("Expected status 404, got %d", w.Code)
	}
}

// 测试流式请求（应该返回错误）
func TestStreamingRequest(t *testing.T) {
	router := setupTestRouter()

	reqBody := models.ChatCompletionRequest{
		Model: "testai-1.1",
		Messages: []models.Message{
			{Role: "user", Content: "Test"},
		},
		Stream: true,
	}

	jsonData, _ := json.Marshal(reqBody)
	req := httptest.NewRequest("POST", "/v1/chat/completions", bytes.NewBuffer(jsonData))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	router.ServeHTTP(w, req)

	// 2026-08-31（W1.5 顺手根因修复）：handler 已实现 stream:true 的 SSE 流式
	// 路径（handlers.go handleStreamingResponse，200 + text/event-stream），
	// 本测试原先断言"流式不支持 → 400"是流式实现落地前的过期契约。
	// 现契约：stream:true → 200 + text/event-stream。
	if w.Code != http.StatusOK {
		t.Errorf("Expected status 200, got %d", w.Code)
	}
	if ct := w.Header().Get("Content-Type"); !strings.Contains(ct, "text/event-stream") {
		t.Errorf("Expected text/event-stream content type, got %q", ct)
	}
}

// 基准测试
func BenchmarkTestAI11(b *testing.B) {
	model := models.NewTestAI11()
	messages := []models.Message{
		{Role: "user", Content: "Test message"},
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		model.Process(messages)
	}
}

func BenchmarkTestAI20(b *testing.B) {
	model := models.NewTestAI20()
	messages := []models.Message{
		{Role: "user", Content: "Test message with some content"},
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		model.Process(messages)
	}
}

// 示例用法
func ExampleModelRegistry() {
	registry := models.NewModelRegistry()
	registry.Register(models.NewTestAI11())

	model, exists := registry.Get("testai-1.1")
	if exists {
		fmt.Println(model.Name())
	}
	// Output: testai-1.1
}

// W1.5 别名机制端到端：TESTAI_ALIASES 解析进 registry 后，
// /v1/chat/completions 用别名请求 → 200；未知模型仍 404 model_not_found。
func TestAliasModelResolvesOverHTTP(t *testing.T) {
	router := setupTestRouter()
	// registry 在 setupTestRouter 内部构建，这里另起一个带别名的等价 router。
	gin.SetMode(gin.TestMode)
	registry := models.NewModelRegistry()
	registry.Register(models.NewTestAI11())
	registry.Register(models.NewTestAI20())
	if errs := registry.ApplyAliases(map[string]string{"gpt-4o-mini": "testai-1.1"}); len(errs) != 0 {
		t.Fatalf("alias apply failed: %v", errs)
	}
	log, _ := logger.NewLogger()
	handler := handlers.NewHandler(registry, log)
	router = gin.New()
	v1 := router.Group("/v1")
	v1.POST("/chat/completions", handler.ChatCompletions)

	// 别名请求 → 200（行为等同其目标模型 testai-1.1）。
	body := `{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}`
	req := httptest.NewRequest("POST", "/v1/chat/completions", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	router.ServeHTTP(w, req)
	if w.Code != 200 {
		t.Fatalf("alias request expected 200, got %d: %s", w.Code, w.Body.String())
	}

	// 未知模型 → 404 model_not_found（别名未覆盖的路径零改动）。
	body404 := `{"model":"no-such-model","messages":[{"role":"user","content":"hi"}]}`
	req404 := httptest.NewRequest("POST", "/v1/chat/completions", bytes.NewBufferString(body404))
	req404.Header.Set("Content-Type", "application/json")
	w404 := httptest.NewRecorder()
	router.ServeHTTP(w404, req404)
	if w404.Code != 404 {
		t.Fatalf("unknown model expected 404, got %d", w404.Code)
	}
}
