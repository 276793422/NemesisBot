// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"encoding/json"
)

// ActionSchema 定义了单个 action 的完整 schema
type ActionSchema struct {
	Name        string                   `json:"name"`
	Description string                   `json:"description"`
	Parameters  map[string]interface{}   `json:"parameters,omitempty"`
	Returns     map[string]interface{}   `json:"returns,omitempty"`
	Examples    []map[string]interface{} `json:"examples,omitempty"`
}

// GetActionsSchema 返回所有可用 actions 的 schema 定义
func (c *Cluster) GetActionsSchema() []interface{} {
	schemas := []ActionSchema{
		{
			Name:        "ping",
			Description: "健康检查，测试节点是否在线",
			Parameters:  nil,
			Returns: map[string]interface{}{
				"properties": map[string]interface{}{
					"status": map[string]interface{}{
						"type":        "string",
						"description": "响应状态",
						"enum":        []string{"ok"},
					},
					"node_id": map[string]interface{}{
						"type":        "string",
						"description": "节点 ID",
					},
				},
			},
			Examples: []map[string]interface{}{
				{
					"request": map[string]interface{}{
						"action":  "ping",
						"payload": nil,
					},
					"response": map[string]interface{}{
						"status":  "ok",
						"node_id": "node-abc123",
					},
				},
			},
		},
		{
			Name:        "get_capabilities",
			Description: "获取节点的功能能力列表",
			Parameters:  nil,
			Returns: map[string]interface{}{
				"properties": map[string]interface{}{
					"capabilities": map[string]interface{}{
						"type":        "array",
						"description": "能力列表",
						"items": map[string]interface{}{
							"type": "string",
						},
					},
				},
			},
			Examples: []map[string]interface{}{
				{
					"request": map[string]interface{}{
						"action":  "get_capabilities",
						"payload": nil,
					},
					"response": map[string]interface{}{
						"capabilities": []string{"llm", "tools", "memory"},
					},
				},
			},
		},
		{
			Name:        "get_info",
			Description: "获取集群信息和在线节点列表",
			Parameters:  nil,
			Returns: map[string]interface{}{
				"properties": map[string]interface{}{
					"node_id": map[string]interface{}{
						"type":        "string",
						"description": "当前节点 ID",
					},
					"peers": map[string]interface{}{
						"type":        "array",
						"description": "在线节点列表",
						"items": map[string]interface{}{
							"type": "object",
							"properties": map[string]interface{}{
								"id": map[string]interface{}{
									"type":        "string",
									"description": "节点 ID",
								},
								"name": map[string]interface{}{
									"type":        "string",
									"description": "节点名称",
								},
								"capabilities": map[string]interface{}{
									"type":        "array",
									"description": "节点能力列表",
								},
								"status": map[string]interface{}{
									"type":        "string",
									"description": "节点状态",
								},
							},
						},
					},
				},
			},
			Examples: []map[string]interface{}{
				{
					"request": map[string]interface{}{
						"action":  "get_info",
						"payload": nil,
					},
					"response": map[string]interface{}{
						"node_id": "node-abc123",
						"peers": []interface{}{
							map[string]interface{}{
								"id":           "node-def456",
								"name":         "Bot Worker 1",
								"capabilities": []string{"llm", "tools"},
								"status":       "online",
							},
						},
					},
				},
			},
		},
		{
			Name:        "peer_chat",
			Description: "与对等节点进行智能对话和任务协作。节点间可以直接通信、互相请求帮助、协调任务，就像两个智能体在对话交流。",
			Parameters: map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"type": map[string]interface{}{
						"type":        "string",
						"description": "对话类型：chat(聊天), request(请求帮助), task(任务协作), query(查询信息)",
						"enum":        []string{"chat", "request", "task", "query"},
						"default":     "request",
					},
					"content": map[string]interface{}{
						"type":        "string",
						"description": "对话内容或任务描述",
					},
					"context": map[string]interface{}{
						"type":        "object",
						"description": "附加上下文信息，如数据、参数等",
					},
				},
				"required": []string{"content"},
			},
			Returns: map[string]interface{}{
				"properties": map[string]interface{}{
					"response": map[string]interface{}{
						"type":        "string",
						"description": "节点的响应内容",
					},
					"result": map[string]interface{}{
						"type":        "object",
						"description": "任务执行结果（如果适用）",
					},
					"status": map[string]interface{}{
						"type":        "string",
						"description": "响应状态",
						"enum":        []string{"success", "error", "busy"},
					},
				},
			},
			Examples: []map[string]interface{}{
				{
					"request": map[string]interface{}{
						"action": "peer_chat",
						"payload": map[string]interface{}{
							"type":    "task",
							"content": "帮我写一首关于春天的诗",
						},
					},
					"response": map[string]interface{}{
						"response": "春风拂过大地，万物苏醒生机...",
						"status":   "success",
					},
				},
				{
					"request": map[string]interface{}{
						"action": "peer_chat",
						"payload": map[string]interface{}{
							"type":    "chat",
							"content": "你好，我是节点A",
						},
					},
					"response": map[string]interface{}{
						"response": "你好节点A！我是节点B，很高兴认识你",
						"status":   "success",
					},
				},
			},
		},
		{
			Name:        "list_actions",
			Description: "获取当前节点所有可用的 actions 及其详细说明。此功能用于服务发现，让外部设备了解当前节点的功能。",
			Parameters:  nil,
			Returns: map[string]interface{}{
				"properties": map[string]interface{}{
					"actions": map[string]interface{}{
						"type":        "array",
						"description": "所有可用的 actions",
						"items": map[string]interface{}{
							"type": "object",
						},
					},
				},
			},
			Examples: []map[string]interface{}{
				{
					"request": map[string]interface{}{
						"action":  "list_actions",
						"payload": nil,
					},
					"response": map[string]interface{}{
						"actions": []interface{}{ /* ... */ },
					},
				},
			},
		},
	}

	// Convert to []interface{}
	result := make([]interface{}, len(schemas))
	for i, schema := range schemas {
		result[i] = schema
	}
	return result
}

// GetActionsSchemaJSON 返回 actions schema 的 JSON 格式
func (c *Cluster) GetActionsSchemaJSON() ([]byte, error) {
	schema := c.GetActionsSchema()
	return json.MarshalIndent(schema, "", "  ")
}
