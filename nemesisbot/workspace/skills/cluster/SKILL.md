---
name: cluster
description: 与集群中的其他节点进行 RPC 通信，实现分布式协作和能力发现
homepage: https://github.com/276793422/NemesisBot
metadata: {"nanobot":{"emoji":"🌐","requires":{"bins":["curl"]}}}
---

# 集群通信 (Cluster Communication)

## 概述

本技能允许你与集群中的其他 NemesisBot 节点进行 RPC 通信，实现：
- 服务发现（查询节点功能）
- 分布式协作（跨节点调用功能）
- 负载均衡（分发任务到不同节点）

## 前提条件

- 节点必须已经加入集群
- 目标节点必须在线
- 必须知道目标节点的 ID

## 核心概念

### RPC (Remote Procedure Call)

RPC 是一种远程过程调用协议，允许你：
- 调用其他节点上的功能
- 就像调用本地功能一样简单
- 获得远程节点的响应

### Actions

Actions 是节点提供的功能接口，每个 action：
- 有唯一的名称
- 接受特定的参数
- 返回结构化的响应

---

## 🔍 服务发现：获取节点功能

### 第 1 步：获取节点列表

使用 `get_info` action 查询集群中的所有节点：

```json
{
  "tool": "cluster_rpc",
  "parameters": {
    "peer_id": "any-online-node",
    "action": "get_info"
  }
}
```

**返回示例**：
```json
{
  "node_id": "node-abc123",
  "peers": [
    {
      "id": "node-def456",
      "name": "Worker Bot 1",
      "capabilities": ["llm", "tools", "web_fetch"],
      "status": "online"
    },
    {
      "id": "node-ghi789",
      "name": "Worker Bot 2",
      "capabilities": ["llm", "memory"],
      "status": "online"
    }
  ]
}
```

### 第 2 步：查询节点的功能

使用 `list_actions` action 查询节点支持的所有功能：

```json
{
  "tool": "cluster_rpc",
  "parameters": {
    "peer_id": "node-def456",
    "action": "list_actions"
  }
}
```

**返回示例**：
```json
{
  "actions": [
    {
      "name": "ping",
      "description": "健康检查，测试节点是否在线",
      "parameters": null,
      "examples": [
        {
          "request": {"action": "ping", "payload": null},
          "response": {"status": "ok", "node_id": "node-abc123"}
        }
      ]
    },
    {
      "name": "llm_forward",
      "description": "转发 LLM 请求到当前节点",
      "parameters": {
        "type": "object",
        "properties": {
          "model": {
            "type": "string",
            "description": "模型名称"
          },
          "messages": {
            "type": "array",
            "description": "对话消息列表"
          }
        },
        "required": ["model", "messages"]
      }
    }
  ]
}
```

---

## 🎯 常用操作

### 1. 健康检查

检查节点是否在线：

```json
{
  "tool": "cluster_rpc",
  "parameters": {
    "peer_id": "node-def456",
    "action": "ping"
  }
}
```

**使用场景**：
- 验证节点状态
- 测试网络连接
- 监控节点健康

### 2. 获取节点能力

了解节点支持的功能：

```json
{
  "tool": "cluster_rpc",
  "parameters": {
    "peer_id": "node-def456",
    "action": "get_capabilities"
  }
}
```

**返回示例**：
```json
{
  "capabilities": ["llm", "tools", "memory", "web_fetch"]
}
```

### 3. Peer Chat - 节点间对话与协作

与对等节点进行智能对话、任务协作或信息交流：

```json
{
  "tool": "cluster_rpc",
  "parameters": {
    "peer_id": "node-ghi789",
    "action": "peer_chat",
    "data": {
      "type": "task",
      "content": "帮我分析这段文本的情感倾向"
    }
  }
}
```

**使用场景**：
- **任务协作**: A节点请求B节点帮助完成某个任务
- **智能对话**: 节点间进行自然语言交流
- **服务请求**: 利用对方节点的特殊能力（LLM、计算、存储等）
- **信息查询**: 向对方节点查询信息

**对话类型**:
- `chat` - 纯聊天交流
- `request` - 请求帮助
- `task` - 任务协作
- `query` - 查询信息

**示例**:
```json
// 请求帮助写诗
{
  "peer_id": "node-ghi789",
  "action": "peer_chat",
  "data": {
    "type": "task",
    "content": "帮我写一首关于春天的诗"
  }
}

// 简单聊天
{
  "peer_id": "node-ghi789",
  "action": "peer_chat",
  "data": {
    "type": "chat",
    "content": "你好，最近忙什么呢？"
  }
}
```

---

## 📋 使用流程

### 标准流程

1. **发现节点** → `get_info` 获取在线节点列表
2. **查询功能** → `list_actions` 了解节点能力
3. **健康检查** → `ping` 验证节点状态
4. **执行操作** → 调用相应的 action 完成任务

### 示例：跨节点协作

```
用户: 使用 node-ghi789 节点帮我写一首诗

LLM: 好的，让我先查询节点信息。

[调用 get_info]
→ 发现 node-ghi789 在线

[调用 list_actions on node-ghi789]
→ 确认支持 peer_chat

[调用 peer_chat on node-ghi789]
→ 成功生成诗歌

LLM: 诗歌已生成完成！
```

---

## ⚠️ 注意事项

### 1. 节点在线状态

**问题**: 调用离线节点会失败

**解决方案**:
- 先使用 `ping` 检查节点状态
- 准备备用节点
- 处理连接失败错误

### 2. 参数验证

**问题**: 参数格式错误会导致调用失败

**解决方案**:
- 使用 `list_actions` 查看参数格式
- 严格按照 schema 传递参数
- 参考示例代码

### 3. 超时处理

**问题**: 远程调用可能超时

**解决方案**:
- 设置合理的超时时间
- 检查网络连接
- 实现重试机制（如果需要）

### 4. 错误处理

**常见错误**:
- `peer not found`: 节点 ID 错误或节点离线
- `no handler for action`: 节点不支持该 action
- `timeout`: 请求超时

**处理方式**:
```json
{
  "error": "peer not found: node-xyz",
  "suggestion": "使用 get_info 查看可用节点"
}
```

---

## 🔧 高级用法

### 动态服务发现

```javascript
// 1. 获取所有在线节点
let nodes = cluster_rpc(peer_id: "any", action: "get_info")

// 2. 为每个节点获取功能列表
for (let node of nodes.peers) {
  let actions = cluster_rpc(
    peer_id: node.id,
    action: "list_actions"
  )
  // 存储节点功能信息
  cache[node.id] = actions.actions
}

// 3. 根据能力选择合适的节点
let targetNode = findNodeWithCapability("llm")
```

### 能力匹配

根据节点能力选择合适的服务：

| 能力 | 用途 | 推荐节点 |
|------|------|---------|
| `llm` | 文本生成 | 高性能节点 |
| `tools` | 工具调用 | 全功能节点 |
| `memory` | 记忆存储 | 大存储节点 |
| `web_fetch` | 网络抓取 | 有网络节点 |

---

## 📊 Action 完整列表

### 系统默认 Actions

#### 1. ping
- **功能**: 健康检查
- **参数**: 无
- **返回**: `{status: "ok", node_id: "..."}`

#### 2. get_capabilities
- **功能**: 获取节点能力列表
- **参数**: 无
- **返回**: `{capabilities: ["..."]}`

#### 3. get_info
- **功能**: 获取集群信息
- **参数**: 无
- **返回**: `{node_id: "...", peers: [...]}`

#### 4. list_actions
- **功能**: 获取所有可用 actions
- **参数**: 无
- **返回**: `{actions: [{name, description, parameters, returns, examples}]}`

#### 5. peer_chat
- **功能**: 与对等节点进行智能对话和任务协作
- **描述**: 节点间可以直接通信、互相请求帮助、协调任务，就像两个智能体在对话交流
- **参数**: `{type: chat|request|task|query, content: string, context?: object}`
- **返回**: `{response: string, result?: object, status: success|error|busy}`
- **使用场景**: 任务协作、智能对话、服务请求、信息查询

#### 6. hello
- **功能**: 示例问候处理器
- **参数**: `{from?: string, timestamp?: string}`
- **返回**: `{greeting, timestamp, node_id, status}`

---

## 🎯 最佳实践

### 1. 逐步验证

```
✓ 先检查节点在线
✓ 再查询节点功能
✓ 然后执行具体操作
```

### 2. 缓存节点信息

- 节点列表可以缓存一段时间
- Actions 列表变化较少，可以长期缓存
- 节点状态需要实时更新

### 3. 错误恢复

- 准备备用节点
- 实现自动重试
- 记录失败原因

---

## 🔮 示例场景

### 场景 1: 负载均衡与任务协作

```
用户: 帮我写一篇关于 AI 的文章

LLM: 我发现有 3 个节点可以协作：
- node-1, node-2, node-3

[检查节点负载]
→ node-2 最空闲

[调用 peer_chat on node-2]
{
  "type": "task",
  "content": "帮我写一篇关于 AI 的文章"
}
→ 成功生成文章
```

### 场景 2: 节点间对话协作

```
用户: 让 node-xyz 帮我分析这段数据

LLM: 好的，让我先和 node-xyz 对话。

[调用 peer_chat on node-xyz]
{
  "type": "request",
  "content": "请帮我分析以下数据...",
  "context": {"data": [...]}
}

node-xyz: 收到！我这就为你分析...
→ 分析完成
```

---

## 📚 相关资源

- **Cluster 文档**: `docs/cluster/`
- **RPC 协议**: `module/cluster/rpc/`
- **Handlers**: `module/cluster/handlers/`

---

**最后更新**: 2026-03-05
**版本**: 1.1 (更新: llm_forward → peer_chat)
