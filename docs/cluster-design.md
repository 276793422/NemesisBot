# NemesisBot Cluster 设计文档

## 版本信息
- **文档版本**: 1.0
- **创建日期**: 2026-03-02
- **最后更新**: 2026-03-02
- **状态**: 设计完成，待实施

---

## 目录
- [1. 概述](#1-概述)
- [2. 架构设计](#2-架构设计)
- [3. 技术方案](#3-技术方案)
- [4. 接口定义](#4-接口定义)
- [5. 配置说明](#5-配置说明)
- [6. 实现计划](#6-实现计划)
- [7. 测试验证](#7-测试验证)

---

## 1. 概述

### 1.1 目标

创建一个独立的 Bot-to-Bot 集群通信系统，使不同的 NemesisBot 实例能够：

- ✅ **自动发现**: 同一局域网内的 bot 自动发现彼此
- ✅ **能力协作**: bot 可以调用其他 bot 的专业能力
- ✅ **状态持久化**: 集群状态保存到本地文件，重启后恢复
- ✅ **LLM 可读**: 集群信息以 TOML 格式存储，LLM 可直接解析

### 1.2 核心设计原则

1. **简洁性**: 去除不必要的复杂度（如应答、握手等）
2. **独立性**: cluster 独立于 channels 模块，属于基础设施层
3. **自动化**: 完全自动发现，无需种子节点
4. **持久化**: 状态保存在本地文件，提供"记忆"能力
5. **局域网**: 当前仅支持同局域网，暂不考虑跨网段

### 1.3 架构定位

```
┌─────────────────────────────────────────────────────┐
│                  Application Layer                   │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────┐ │
│  │ Agent (LLM)  │  │  Web UI      │  │  Channels  │ │
│  └──────┬───────┘  └──────────────┘  └────────────┘ │
└─────────┼───────────────────────────────────────────┘
          │
          ↓
┌─────────────────────────────────────────────────────┐
│                  Cluster Module (NEW)               │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────┐ │
│  │ Discovery    │  │    RPC       │  │  Registry  │ │
│  │ (UDP 49100)  │  │ (WS 49200)   │  │ (Memory)   │ │
│  └──────────────┘  └──────────────┘  └────────────┘ │
└─────────────────────────────────────────────────────┘
          │
          ↓
┌─────────────────────────────────────────────────────┐
│                  Persistence Layer                   │
│         workspace/cluster/peers.toml                 │
└─────────────────────────────────────────────────────┘
```

---

## 2. 架构设计

### 2.1 模块结构

```
module/cluster/
├── cluster.go              # 核心结构和接口
├── node.go                 # 节点抽象定义
├── registry.go             # 节点注册表（内存）
│
├── discovery/              # UDP 广播发现
│   ├── discovery.go        # 广播/监听逻辑
│   ├── listener.go         # UDP 监听器 (port 49100)
│   └── message.go          # 广播消息定义
│
├── transport/              # WebSocket 传输层
│   ├── websocket.go        # WebSocket 连接 (port 49200)
│   ├── pool.go             # 连接池管理
│   └── rpc.go              # RPC 消息协议
│
├── config/                 # 配置管理
│   ├── loader.go           # TOML 加载
│   └── saver.go            # TOML 保存
│
├── rpc/                    # RPC 调用
│   ├── client.go           # RPC 客户端
│   └── server.go           # RPC 服务端
│
└── logger.go               # 日志管理
```

### 2.2 目录结构

```
workspace/
├── cluster/
│   └── peers.toml                  # 自动生成和维护
│
└── .nemesisbot/
    └── workspace/
        └── logs/
            └── cluster/             # 集群日志目录
                ├── discovery.log    # 发现/广播日志
                └── rpc.log          # RPC 调用日志
```

### 2.3 端口分配

| 用途 | 协议 | 端口 | 说明 |
|------|------|------|------|
| **UDP 广播** | UDP | 49100 | 自动发现和心跳 |
| **WebSocket RPC** | WebSocket | 49200 | Bot 间 RPC 通信 |

---

## 3. 技术方案

### 3.1 自动发现机制

#### 工作原理

```
时间线：

T0: Bot A 启动
    ↓
    监听 UDP :49100
    ↓
T1: 发送 announce 广播（包含自己的信息）
    ↓
T2: Bot B 收到 announce
    ↓
    记录 Bot A 到 peers.toml
    （不发送响应）
    ↓
T3: Bot B 定时广播自己的信息
    ↓
T4: Bot A 收到 Bot B 的 announce
    ↓
    记录 Bot B 到 peers.toml
    ↓
循环：每 30 秒广播一次
超时：90 秒未收到某 bot 广播 → 标记为 offline
```

#### 广播策略

- **启动时**: 立即发送一次 announce
- **正常运行**: 每 30 秒广播一次
- **下线时**: 发送 bye 消息（可选）
- **超时检测**: 90 秒未收到广播 → 标记离线
- **随机抖动**: ±5 秒，避免广播风暴

### 3.2 通信协议

#### UDP 广播消息（JSON）

```go
type DiscoveryMessage struct {
    Version      string   `json:"version"`      // 协议版本 "1.0"
    Type         string   `json:"type"`         // "announce" | "bye"
    NodeID       string   `json:"node_id"`      // 节点唯一 ID
    Name         string   `json:"name"`         // 节点名称
    Address      string   `json:"address"`      // "IP:49200"
    Capabilities []string `json:"capabilities"` // 能力列表
    Timestamp    int64    `json:"timestamp"`     // Unix 时间戳
}
```

**示例 - Announce**:
```json
{
  "version": "1.0",
  "type": "announce",
  "node_id": "bot-code-expert",
  "name": "代码专家",
  "address": "192.168.1.101:49200",
  "capabilities": ["code_analysis", "code_generation"],
  "timestamp": 1740913800
}
```

**示例 - Bye**:
```json
{
  "version": "1.0",
  "type": "bye",
  "node_id": "bot-code-expert",
  "timestamp": 1740913900
}
```

#### WebSocket RPC 消息（JSON）

```go
type RPCMessage struct {
    Version   string                 `json:"version"`   // "1.0"
    ID        string                 `json:"id"`        // 消息唯一 ID
    Type      string                 `json:"type"`      // "request" | "response" | "error"
    From      string                 `json:"from"`      // 发送者 node_id
    To        string                 `json:"to"`        // 接收者 node_id
    Action    string                 `json:"action"`    // 动作类型
    Payload   map[string]interface{} `json:"payload"`   // 业务数据
    Timestamp int64                  `json:"timestamp"` // Unix 时间戳
}
```

**示例 - Request**:
```json
{
  "version": "1.0",
  "id": "req-001",
  "type": "request",
  "from": "bot-main",
  "to": "bot-code-expert",
  "action": "code_analysis",
  "payload": {
    "code": "func main() {}",
    "language": "go"
  },
  "timestamp": 1740913800
}
```

**示例 - Response**:
```json
{
  "version": "1.0",
  "id": "req-001",
  "type": "response",
  "from": "bot-code-expert",
  "to": "bot-main",
  "action": "code_analysis",
  "payload": {
    "result": "分析完成",
    "complexity": "O(1)"
  },
  "timestamp": 1740913801
}
```

### 3.3 状态持久化

#### peers.toml 格式

```toml
# Cluster Configuration
# Auto-generated by cluster module

[cluster]
id = "auto-discovered"
auto_discovery = true
last_updated = "2026-03-02T15:30:00Z"

[node]
id = "bot-main"
name = "主 Bot"
address = "192.168.1.100:49200"
role = "worker"
capabilities = []

[[peers]]
id = "bot-code-expert"
name = "代码专家"
address = "192.168.1.101:49200"
role = "worker"
capabilities = ["code_analysis", "code_generation", "code_review"]

[peers.status]
state = "online"
last_seen = "2026-03-02T15:29:55Z"
uptime = "2h 15m"
tasks_completed = 47
success_rate = 0.98

[[peers]]
id = "bot-translator"
name = "翻译专家"
address = "192.168.1.102:49200"
role = "worker"
capabilities = ["translation", "language_detection"]

[peers.status]
state = "offline"
last_seen = "2026-03-02T15:20:00Z"
uptime = "0h 0m"
tasks_completed = 12
success_rate = 0.95
last_error = "connection timeout"
```

#### 同步策略

- **内存优先**: 快速操作，全部在内存中进行
- **定时同步**: 每 30 秒将内存状态同步到 peers.toml
- **事件驱动**: 发现新 peer 或 peer 离线时立即同步

### 3.4 核心逻辑

#### Cluster 主结构

```go
type Cluster struct {
    // 节点信息
    nodeID     string
    nodeName   string

    // 路径
    workspace  string
    configPath string
    logDir     string

    // 组件
    registry   *Registry          // 内存注册表
    discovery  *discovery.Discovery // UDP 发现
    transport  *transport.Transport // WebSocket 传输
    logger     *ClusterLogger

    // 配置
    config     *ClusterConfig

    // 状态
    mu         sync.RWMutex
    running    bool
    stopCh     chan struct{}
}
```

#### 工作流程

```go
func (c *Cluster) Start() error {
    c.mu.Lock()
    c.running = true
    c.mu.Unlock()

    // 1. 启动 UDP 广播发现
    c.discovery.Start()

    // 2. 启动 WebSocket 服务
    c.transport.Start()

    // 3. 启动定时同步
    go c.syncLoop()

    c.logger.Info("Cluster started: node_id=%s", c.nodeID)
    return nil
}

func (c *Cluster) syncLoop() {
    ticker := time.NewTicker(30 * time.Second)
    defer ticker.Stop()

    for {
        select {
        case <-ticker.C:
            // 检查超时
            c.checkTimeouts()

            // 同步到文件
            c.SyncToDisk()

        case <-c.stopCh:
            return
        }
    }
}
```

---

## 4. 接口定义

### 4.1 Cluster 对外接口

```go
// 启动集群
func (c *Cluster) Start() error

// 停止集群
func (c *Cluster) Stop() error

// RPC 调用
func (c *Cluster) Call(peerID string, action string, payload map[string]interface{}) ([]byte, error)

// 获取所有可用能力
func (c *Cluster) GetCapabilities() []string

// 查找有特定能力的 peer
func (c *Cluster) FindPeersByCapability(capability string) []*Peer

// 获取在线 peers
func (c *Cluster) GetOnlinePeers() []*Peer

// 同步状态到磁盘
func (c *Cluster) SyncToDisk() error
```

### 4.2 Agent 工具接口

```go
// module/tools/cluster_rpc.go

type ClusterRPCTool struct {
    cluster *cluster.Cluster
}

func (t *ClusterRPCTool) Execute(ctx context.Context, params map[string]interface{}) (string, error) {
    // 参数解析
    peerID := params["peer_id"].(string)
    action := params["action"].(string)
    payload := params["data"].(map[string]interface{})

    // RPC 调用
    response, err := t.cluster.Call(peerID, action, payload)
    if err != nil {
        return "", err
    }

    return string(response), nil
}
```

### 4.3 LLM 调用示例

```
User: 帮我分析这段 Go 代码的复杂度

LLM:
1. 读取 peers.toml，发现有 bot-code-expert 有 code_analysis 能力
2. 调用 cluster_rpc 工具：
   - peer_id: "bot-code-expert"
   - action: "analyze_complexity"
   - data: {code: "...", language: "go"}
3. 接收结果并回复用户
```

---

## 5. 配置说明

### 5.1 配置文件

**注意**: cluster 模块不使用 `config.json` 配置，而是通过 `peers.toml` 自管理。

### 5.2 环境变量（可选）

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `CLUSTER_UDP_PORT` | UDP 广播端口 | 49100 |
| `CLUSTER_RPC_PORT` | WebSocket RPC 端口 | 49200 |
| `CLUSTER_BROADCAST_INTERVAL` | 广播间隔（秒） | 30 |
| `CLUSTER_TIMEOUT` | 超时时间（秒） | 90 |

---

## 6. 实现计划

### Phase 1: 核心框架（基础设施）

**文件**:
- `module/cluster/cluster.go` - 核心结构
- `module/cluster/node.go` - 节点定义
- `module/cluster/registry.go` - 注册表
- `module/cluster/logger.go` - 日志管理

**验收标准**:
- ✅ Cluster 结构可以创建
- ✅ Registry 可以添加/删除/查询节点
- ✅ 日志可以正确写入 `.nemesisbot/workspace/logs/cluster/`

---

### Phase 2: UDP 广播发现

**文件**:
- `module/cluster/discovery/discovery.go` - 广播逻辑
- `module/cluster/discovery/listener.go` - UDP 监听
- `module/cluster/discovery/message.go` - 消息定义

**验收标准**:
- ✅ 可以在 49100 端口发送广播
- ✅ 可以接收其他 bot 的广播
- ✅ 收到广播后更新 peers.toml
- ✅ 测试：启动两个 bot，互相可以发现

---

### Phase 3: 配置持久化

**文件**:
- `module/cluster/config/loader.go` - TOML 加载
- `module/cluster/config/saver.go` - TOML 保存

**验收标准**:
- ✅ 首次启动创建 peers.toml
- ✅ 可以加载现有的 peers.toml
- ✅ 可以正确解析 TOML 格式
- ✅ 可以保存状态到 peers.toml

---

### Phase 4: WebSocket RPC

**文件**:
- `module/cluster/transport/websocket.go` - WebSocket 连接
- `module/cluster/transport/pool.go` - 连接池
- `module/cluster/transport/rpc.go` - RPC 消息
- `module/cluster/rpc/client.go` - RPC 客户端
- `module/cluster/rpc/server.go` - RPC 服务端

**验收标准**:
- ✅ WebSocket 服务监听 49200 端口
- ✅ 可以接收 RPC 请求
- ✅ 可以发送 RPC 请求到其他 bot
- ✅ 测试：Bot A 可以调用 Bot B 的能力

---

### Phase 5: Agent 集成

**文件**:
- `module/tools/cluster_rpc.go` - Agent 工具
- `module/cluster/cluster.go` - 完善 Cluster 接口

**验收标准**:
- ✅ Agent 可以通过工具调用其他 bot
- ✅ LLM 可以理解 peers.toml 内容
- ✅ 完整的端到端测试

---

### Phase 6: 测试和文档

**文件**:
- `test/unit/cluster/*_test.go` - 单元测试
- `test/integration/cluster/*_test.go` - 集成测试
- `docs/cluster-usage.md` - 使用文档
- `docs/cluster-api.md` - API 文档

**验收标准**:
- ✅ 所有单元测试通过
- ✅ 集成测试通过
- ✅ 文档完整

---

## 7. 测试验证

### 7.1 单元测试

**测试覆盖**:
- [ ] Registry 添加/删除/查询节点
- [ ] Discovery 消息序列化/反序列化
- [ ] TOML 加载/保存
- [ ] 超时检测逻辑
- [ ] 并发安全

### 7.2 集成测试

**测试场景**:

#### 场景 1: 自动发现
```
1. 启动 Bot A
2. 启动 Bot B
3. 验证: Bot A 的 peers.toml 包含 Bot B
4. 验证: Bot B 的 peers.toml 包含 Bot A
```

#### 场景 2: RPC 调用
```
1. Bot A 和 Bot B 都在运行
2. Bot A 通过 RPC 调用 Bot B 的能力
3. 验证: 调用成功，返回正确结果
```

#### 场景 3: 离线检测
```
1. Bot A 和 Bot B 都在运行
2. 停止 Bot B
3. 等待 90 秒
4. 验证: Bot A 将 Bot B 标记为 offline
```

#### 场景 4: 恢复上线
```
1. Bot B 被标记为 offline
2. 重新启动 Bot B
3. 验证: Bot A 在 30 秒内将 Bot B 标记为 online
```

### 7.3 性能测试

**测试指标**:
- [ ] 广播消息延迟 < 50ms
- [ ] RPC 调用延迟 < 200ms
- [ ] 内存占用 < 50MB
- [ ] 网络带宽 < 1KB/s（仅心跳）

---

## 8. 关键设计决策记录

### 决策 1: 使用 UDP 广播而非服务中心

**理由**:
- ✅ 简单：无需额外服务
- ✅ 自动：完全自动化发现
- ✅ 局域网：当前仅支持局域网
- ✅ 去中心化：无单点故障

**权衡**:
- ❌ 跨网段需要额外方案（未来扩展）

### 决策 2: 不需要应答机制

**理由**:
- ✅ 简化：广播是公告模式，不是对话
- ✅ 高效：减少一半的网络流量
- ✅ 无状态：不需要维护会话

### 决策 3: 广播即心跳

**理由**:
- ✅ 简化：去掉单独的 heartbeat 模块
- ✅ 统一：一个广播循环解决所有
- ✅ 可靠：30秒间隔足够

### 决策 4: TOML 配置 + 内存缓存

**理由**:
- ✅ 持久化：重启后恢复状态
- ✅ 性能：内存操作快速
- ✅ LLM 可读：TOML 人类友好
- ✅ 简单：不需要 Markdown 转换

### 决策 5: 独立于 channels 模块

**理由**:
- ✅ 职责清晰：cluster 是基础设施，不是用户通道
- ✅ 易扩展：不受通道框架限制
- ✅ 专业化：机器间通信不同于人机交互

---

## 9. 未来扩展方向（可选）

### 9.1 跨局域网支持
- 添加公网地址配置
- 支持 STUN/TURN 打洞
- 添加中继服务器

### 9.2 安全增强
- 添加共享密钥认证
- 实现 TLS 加密
- 添加白名单机制

### 9.3 服务发现
- 支持 Consul 集成
- 支持 etcd 集成
- 支持 Kubernetes service discovery

### 9.4 消息队列
- 添加离线消息队列
- 支持消息持久化
- 支持消息重传

### 9.5 负载均衡
- 基于能力的智能路由
- 基于负载的动态选择
- 支持多副本

---

## 10. 附录

### 10.1 端口分配总结

| 端口 | 协议 | 用途 |
|------|------|------|
| 49100 | UDP | 广播发现 |
| 49200 | WebSocket | RPC 通信 |

### 10.2 文件路径总结

| 路径 | 说明 |
|------|------|
| `workspace/cluster/peers.toml` | 集群配置和状态 |
| `.nemesisbot/workspace/logs/cluster/discovery.log` | 发现日志 |
| `.nemesisbot/workspace/logs/cluster/rpc.log` | RPC 日志 |

### 10.3 消息类型总结

| 类型 | 用途 | 协议 |
|------|------|------|
| announce | 广播上线和能力 | UDP |
| heartbeat | 定时心跳 = 定时广播 | UDP |
| bye | 广播下线 | UDP |
| request | RPC 请求 | WebSocket |
| response | RPC 响应 | WebSocket |
| error | RPC 错误 | WebSocket |

---

## 文档变更历史

| 版本 | 日期 | 变更内容 | 作者 |
|------|------|---------|------|
| 1.0 | 2026-03-02 | 初始版本 | Claude |

---

**文档状态**: ✅ 设计完成，准备实施
