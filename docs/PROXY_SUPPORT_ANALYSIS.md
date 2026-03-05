# NemesisBot 模型级代理支持深度分析报告

**日期**: 2026-03-05
**主题**: 模型级 HTTP/SOCKS5 代理的技术实现分析

---

## 一、当前代理支持架构

### 1.1 配置层级

```
config.json
    ↓
ModelConfig.Proxy (string)
    ↓
config.ResolveModelConfig()
    ↓
ProviderResolution.Proxy
    ↓
providers.factory.resolveProviderSelection()
    ↓
providerSelection.proxy
    ↓
providers.NewHTTPProvider(apiKey, apiBase, proxy)
```

### 1.2 配置示例

```json
{
  "model_list": [
    {
      "model_name": "zhipu-flash",
      "model": "zhipu/glm-4.7-flash",
      "api_base": "https://open.bigmodel.cn/api/paas/v4",
      "api_key": "your-api-key",
      "proxy": "http://127.0.0.1:7890"
    }
  ]
}
```

### 1.3 当前实现代码

**openai_compat/provider.go:35-56**:
```go
func NewProvider(apiKey, apiBase, proxy string) *Provider {
    client := &http.Client{
        Timeout: 120 * time.Second,
    }

    if proxy != "" {
        parsed, err := url.Parse(proxy)
        if err == nil {
            client.Transport = &http.Transport{
                Proxy: http.ProxyURL(parsed),
            }
        }
    }

    return &Provider{
        apiKey:     apiKey,
        apiBase:    strings.TrimRight(apiBase, "/"),
        httpClient: client,
    }
}
```

### 1.4 架构特点

| 特点 | 说明 |
|------|------|
| **模型级隔离** | 每个模型配置有独立的代理设置 |
| **Provider 绑定** | HTTP Client 在 Provider 创建时绑定 |
| **静态配置** | 运行时无法动态更改代理 |
| **仅支持 HTTP(S)** | 不支持原生 SOCKS5 代理 |

---

## 二、底层处理方式

### 2.1 HTTP 请求流程（有代理）

```
┌─────────────────────────────────────────────────────────────────┐
│                         应用层                                   │
├─────────────────────────────────────────────────────────────────┤
│  AgentLoop → Provider.Chat() → HTTP Client                      │
│                                  ↓                               │
│                          Transport.Proxy                         │
│                                  ↓                               │
│                    判断是否需要代理                               │
│                    /              \                              │
│              有代理               无代理                          │
│                ↓                   ↓                             │
│         连接代理服务器        直接连接目标服务器                    │
└─────────────────────────────────────────────────────────────────┘
                                  ↓
┌─────────────────────────────────────────────────────────────────┐
│                         网络层                                   │
├─────────────────────────────────────────────────────────────────┤
│  HTTP 代理:                                                      │
│    Client → Proxy Server (CONNECT) → Target Server              │
│                                                                  │
│  HTTPS 代理:                                                     │
│    Client → Proxy Server (CONNECT) → TLS Tunnel → Target        │
│                                                                  │
│  SOCKS5 代理:                                                    │
│    Client → SOCKS5 Server (握手) → Target Server                 │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 HTTP 代理协议详解

**CONNECT 方法（用于 HTTPS 目标）**:
```
1. 客户端 → 代理服务器:
   CONNECT open.bigmodel.cn:443 HTTP/1.1
   Host: open.bigmodel.cn:443

2. 代理服务器 → 客户端:
   HTTP/1.1 200 Connection Established

3. 建立隧道后，所有数据透传（加密）
```

**普通 HTTP 请求（用于 HTTP 目标）**:
```
1. 客户端 → 代理服务器:
   POST http://api.example.com/v1/chat HTTP/1.1
   Host: api.example.com
   [请求体...]

2. 代理服务器 → 目标服务器:
   POST /v1/chat HTTP/1.1
   Host: api.example.com
   [请求体...]
```

### 2.3 SOCKS5 代理协议

```
1. 客户端 → SOCKS5 服务器:
   版本: 0x05
   认证方法: 无认证(0x00) / 用户名密码(0x02)

2. SOCKS5 服务器 → 客户端:
   选择的认证方法

3. 客户端 → SOCKS5 服务器（如需认证）:
   用户名长度 + 用户名 + 密码长度 + 密码

4. 客户端 → SOCKS5 服务器:
   版本: 0x05
   命令: CONNECT (0x01)
   目标地址类型: IPv4(0x01) / 域名(0x03) / IPv6(0x04)
   目标地址 + 端口

5. SOCKS5 服务器 → 客户端:
   连接成功响应

6. 建立连接后，所有数据透传
```

---

## 三、客户端处理方式

### 3.1 当前实现（静态绑定）

```go
type Provider struct {
    apiKey     string
    apiBase    string
    httpClient *http.Client  // 创建时绑定代理配置
}

// 问题：创建后无法修改代理
func NewProvider(apiKey, apiBase, proxy string) *Provider {
    // HTTP Client 创建时确定代理配置
    // 之后无法更改
}
```

### 3.2 当前限制

| 操作 | 是否支持 | 说明 |
|------|----------|------|
| 启动时设置代理 | ✅ | 通过配置文件 |
| 运行时切换代理 | ❌ | 需要重启应用 |
| 临时禁用代理 | ❌ | 需要修改配置并重启 |
| 代理故障自动切换 | ❌ | 请求会直接失败 |
| 多代理负载均衡 | ❌ | 只能配置一个代理 |

---

## 四、数据处理流程

### 4.1 数据流向

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Provider   │────▶│  HTTP Client │────▶│ Proxy Server │
│  (应用层)    │     │  (Transport) │     │  (代理服务器) │
└──────────────┘     └──────────────┘     └──────────────┘
                                                 │
                                                 ▼
                                         ┌──────────────┐
                                         │ Target API   │
                                         │ (LLM 服务)   │
                                         └──────────────┘
```

### 4.2 数据包变化

| 阶段 | 数据内容 |
|------|----------|
| 应用层 | JSON 请求体（明文） |
| HTTP Client | 添加 HTTP 头 |
| 到代理服务器 | HTTP CONNECT 或完整 URL 请求 |
| 代理到目标 | 标准 HTTP/HTTPS 请求 |
| 响应 | 原路返回 |

### 4.3 安全对比

| 方面 | HTTP 代理 | HTTPS 代理 | SOCKS5 |
|------|-----------|------------|--------|
| 数据可见性 | 代理可见全部 | 代理仅见地址 | 代理仅见地址 |
| 中间人风险 | 高 | 低 | 低 |
| 日志记录 | 完整请求 | 仅地址 | 仅地址 |
| 适用场景 | 调试 | 生产 | 高安全 |

---

## 五、设置/取消代理

### 5.1 当前方式

**设置代理**:
```bash
# 命令行
nemesisbot model add -n my-model -m openai/gpt-4o \
  --api-base https://api.openai.com/v1 \
  --key sk-xxx \
  --proxy http://127.0.0.1:7890

# 配置文件
{
  "model_list": [{
    "proxy": "http://127.0.0.1:7890"
  }]
}
```

**取消代理**:
```bash
# 修改配置
nemesisbot model update my-model --proxy ""

# 或编辑配置文件删除 proxy 字段

# 重启应用
```

### 5.2 代理 URL 格式

```
# 无认证
http://proxy.example.com:8080
https://proxy.example.com:443
socks5://proxy.example.com:1080

# 有认证
http://username:password@proxy.example.com:8080
socks5://username:password@proxy.example.com:1080
```

---

## 六、影响分析

### 6.1 性能影响

| 影响项 | 说明 | 量级 |
|--------|------|------|
| 连接建立 | 增加代理连接 | +10-100ms |
| 数据传输 | 中转延迟 | +5-50ms |
| TLS 握手 | 额外握手 | +20-200ms |
| 带宽消耗 | 代理限制 | 取决于代理 |

### 6.2 可靠性影响

| 风险 | 说明 | 缓解措施 |
|------|------|----------|
| 代理故障 | 请求失败 | 多代理配置 |
| 代理超时 | 响应变慢 | 合理超时 |
| 代理被封锁 | 无法连接 | 备用代理 |
| DNS 问题 | 代理端解析 | IP 直连 |

### 6.3 功能影响

| 场景 | 有代理 | 无代理 |
|------|--------|--------|
| 国内访问国外 API | ✅ 可用 | ❌ 可能超时 |
| 调试请求内容 | ✅ 方便 | ⚠️ 需要抓包 |
| 访问内网 API | ❌ 可能不行 | ✅ 正常 |

---

## 七、增强方案

### 7.1 支持 SOCKS5 代理

**添加依赖**:
```go
import "golang.org/x/net/proxy"
```

**实现代码**:
```go
func NewProvider(apiKey, apiBase, proxy string) *Provider {
    client := &http.Client{
        Timeout: 120 * time.Second,
    }

    if proxy != "" {
        parsed, err := url.Parse(proxy)
        if err == nil {
            switch parsed.Scheme {
            case "socks5":
                // SOCKS5 代理
                auth := &proxy.Auth{}
                if parsed.User != nil {
                    auth.User = parsed.User.Username()
                    auth.Password, _ = parsed.User.Password()
                }
                dialer, err := proxy.SOCKS5("tcp", parsed.Host, auth, proxy.Direct)
                if err == nil {
                    client.Transport = &http.Transport{
                        Dial: dialer.Dial,
                    }
                }
            default:
                // HTTP/HTTPS 代理
                client.Transport = &http.Transport{
                    Proxy: http.ProxyURL(parsed),
                }
            }
        }
    }

    return &Provider{
        apiKey:     apiKey,
        apiBase:    strings.TrimRight(apiBase, "/"),
        httpClient: client,
    }
}
```

### 7.2 支持环境变量代理

```go
func getProxyFromEnv() string {
    // 优先级: HTTPS_PROXY > https_proxy > HTTP_PROXY > http_proxy
    if proxy := os.Getenv("HTTPS_PROXY"); proxy != "" {
        return proxy
    }
    if proxy := os.Getenv("https_proxy"); proxy != "" {
        return proxy
    }
    if proxy := os.Getenv("HTTP_PROXY"); proxy != "" {
        return proxy
    }
    if proxy := os.Getenv("http_proxy"); proxy != "" {
        return proxy
    }
    return ""
}

func NewProvider(apiKey, apiBase, proxy string) *Provider {
    // 显式配置优先，否则使用环境变量
    if proxy == "" {
        proxy = getProxyFromEnv()
    }
    // ...
}
```

### 7.3 支持动态代理切换

```go
type Provider struct {
    apiKey     string
    apiBase    string
    httpClient *http.Client
    mu         sync.RWMutex
    currentProxy string
}

func (p *Provider) SetProxy(proxyURL string) error {
    p.mu.Lock()
    defer p.mu.Unlock()

    if proxyURL == "" {
        p.httpClient.Transport = nil
        p.currentProxy = ""
        return nil
    }

    parsed, err := url.Parse(proxyURL)
    if err != nil {
        return fmt.Errorf("invalid proxy URL: %w", err)
    }

    p.httpClient.Transport = &http.Transport{
        Proxy: http.ProxyURL(parsed),
    }
    p.currentProxy = proxyURL
    return nil
}

func (p *Provider) GetProxy() string {
    p.mu.RLock()
    defer p.mu.RUnlock()
    return p.currentProxy
}
```

### 7.4 支持代理故障转移

```go
type ProxyPool struct {
    proxies []string
    current int
    mu      sync.RWMutex
}

func (p *ProxyPool) GetNext() string {
    p.mu.Lock()
    defer p.mu.Unlock()

    if len(p.proxies) == 0 {
        return ""
    }

    proxy := p.proxies[p.current]
    p.current = (p.current + 1) % len(p.proxies)
    return proxy
}

func (p *ProxyPool) MarkFailed(proxy string) {
    // 标记代理失败，下次切换到下一个
    p.mu.Lock()
    defer p.mu.Unlock()
    p.current = (p.current + 1) % len(p.proxies)
}
```

---

## 八、总结

### 8.1 当前状态

| 功能 | 状态 | 实现难度 |
|------|------|----------|
| HTTP/HTTPS 代理 | ✅ 已支持 | - |
| SOCKS5 代理 | ❌ 未支持 | 低 |
| 环境变量代理 | ❌ 未支持 | 低 |
| 动态切换代理 | ❌ 未支持 | 中 |
| 代理认证 | ✅ 已支持 | - |
| 代理故障转移 | ❌ 未支持 | 中 |
| 多代理负载均衡 | ❌ 未支持 | 中 |

### 8.2 推荐实现优先级

1. **SOCKS5 代理** - 最常用，实现简单
2. **环境变量代理** - 方便部署，实现简单
3. **动态切换** - 需求较少，实现复杂

### 8.3 使用建议

| 场景 | 推荐代理类型 |
|------|--------------|
| 开发调试 | HTTP 代理 |
| 生产环境 | HTTPS 代理 |
| 高安全需求 | SOCKS5 代理 |
| 网络受限环境 | 任意可用代理 |
