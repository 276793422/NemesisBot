# NemesisBot WASM 插件开发包（devkit）

> 与 nemesisbot @DEVKIT_VERSION@（commit @DEVKIT_COMMIT@）同版本发布。
> 本包是**最小可编译集合**：不依赖 NemesisBot 源码仓库，解压即编。

## 这是什么

NemesisBot 的 WASM 插件 = 用 Rust 写一个编译到 WebAssembly 组件的功能组件，
装进 NemesisBot 后成为 agent 可调用的工具（或监听事件流的观察者）。运行在
wasmtime 沙盒里：默认零权限、出站网络 deny-by-default、fuel/内存/墙钟三重
限额、凭据经 vault 注入（原文不进 guest）。

两个扩展点：

- **工具插件**（world `plugin-tool`）：实现 `get_metadata` + `execute` 两方法，
  装好后 agent 在对话中调用 `plugin.<slug>.<工具名>`；
- **观察者插件**（world `plugin-observer`）：实现 `observe` 单入口，接收事件
  投影（载荷脱敏，零内容体）。

## 目录结构

```
wasm-plugin-devkit/
├── README.md            ← 本文件
├── Cargo.toml           ← 根 workspace（只收 pack）
├── pack/                ← 一键打包工具（纯 Rust）
├── sdk/                 ← nemesis-plugin-sdk（三方 SDK，standalone 版）
├── examples/
│   ├── textstat/        ← 工具插件：字符/词数/行数统计（配置热生效 + 数据目录持久化）
│   ├── activity-log/    ← 观察者插件：事件投影落数据目录 events.log
│   └── translate/       ← 工具插件：宿主能力面自检（mode 切换十种行为，含各拒绝路径演示）
└── dist/                ← pack 产出（可安装 staging 目录），无需手工创建
```

## 前置（一次性）

- Rust 工具链（能跑 `cargo` 即可）；
- WASM 目标：`rustup target add wasm32-wasip2`。

## 三步上手

```bash
# ① 打包（在 devkit 根目录）：编译全部示例 + 算哈希 + 填清单 + 产出 dist/<slug>/
cargo run -p pack

# ② 安装（拿 pack 打印的命令，形如）：
nemesisbot plugin install "<devkit路径>/dist/textstat" --yes --allow-unsigned
#    或 Dashboard 插件页 → WASM 插件 → 安装表单填 staging 路径，勾「允许无签名」

# ③ 调用：对话里让 agent 用它，例如：
#    「用 plugin.textstat.textstat 统计一下 IDENTITY.md」
```

说明：

- `--allow-unsigned` / 「允许无签名」：示例未签名。真实发布插件应签名
  （`plugin sign` + `plugin trust`），详见nemesisbot 文档《WASM 插件三方开发指南》。
- **首次安装会弹审批卡**（安全漏斗），在聊天面板批准即可；约 5 分钟不批准
  自动拒绝。
- 安装成功后工具立即对 agent 可见（热装载，无需重启）。
- 只想试一个示例：`cargo run -p pack -- textstat`。

## 开发你自己的插件

最快路径：**复制一个示例目录改名，改 `src/lib.rs`**。最小工具插件如下
（完整可运行样例见 `sdk/src/lib.rs` 顶部文档）：

```rust
use nemesis::plugin::host;

struct Hello;

impl exports::nemesis::plugin::tool::Guest for Hello {
    fn get_metadata() -> Result<exports::nemesis::plugin::tool::ToolMetadata, host::HostError> {
        Ok(exports::nemesis::plugin::tool::ToolMetadata {
            name: "hello".into(),                       // 工具名（全名 = plugin.<slug>.<name>）
            title: "Hello".into(),
            description: "Greets someone.".into(),       // agent 靠它选工具，写清楚
            parameters_json: r#"{"type":"object","properties":{"who":{"type":"string"}},"required":["who"]}"#.into(),
            operation_type: "read".into(),               // read | write | exec | network
        })
    }
    fn execute(input: exports::nemesis::plugin::tool::ToolInput)
        -> Result<exports::nemesis::plugin::tool::ToolOutput, host::HostError> {
        Ok(exports::nemesis::plugin::tool::ToolOutput {
            content: format!("hi, {}", host::config_get("who").ok().flatten().unwrap_or_default()),
            is_error: false,
        })
    }
}

nemesis_plugin_sdk::export_tool!(Hello);
```

同时：复制示例的 `wit/` 目录（**工程根须有 wit/ 副本**，与 `sdk/wit/` 同版本）；
写一份 `plugin.toml.sample`（参考示例，`wasm-sha256` 行保持占位即可，pack 会替换）。

### 宿主能力面（host 函数，共 8 个）

| 函数 | 能力 | 备注 |
|---|---|---|
| `log(level, msg)` | 宿主日志 | 插件页「日志」可见 |
| `now_millis()` | 当前时间戳 | |
| `config_get(key)` | 读实例配置 | x-secret 键恒 PolicyDenied（凭据与配置分离） |
| `secret_get(name)` | 读凭据 | 须 manifest 声明 + vault 命中，返回原文 |
| `workspace_read(path)` | 读工作区文件 | 相对路径，越界/绝对/8.3 短名被围栏拒绝 |
| `data_dir_path()` | 插件数据目录 | per-plugin preopen，可持久化文件 |
| `http_send(req)` | 出站 HTTP | 须 manifest egress 声明，deny-by-default |
| `tool_invoke(name, args)` | 调其他工具 | 深度 1：不能调插件工具；observe 帧内恒 unavailable |

`HostError` 五值闭合：`NoPermission` / `PolicyDenied` / `NotFound` /
`BudgetExceeded` / `Unavailable`。每帧宿主调用有预算（默认 1000 次），耗尽
返回 `BudgetExceeded`。

### plugin.toml 字段速查

| 字段 | 必填 | 说明 |
|---|---|---|
| `wasm-sha256` | ✓ | 载荷 SHA-256（pack 自动填；手工流程须自算） |
| `api-version` | ✓ | 当前 1 |
| `slug` | ✓ | 插件标识（目录名/数据目录/工具前缀） |
| `kind` | ✓ | `tool` 或 `observer`（一 manifest 一 kind） |
| `name` / `version` / `description` | ✓ | 展示信息 |
| `[permissions]` | | `egress`（域名 allowlist）/ `x-secret`（凭据名）；空 = 零权限 |
| `[config-schema]` | | 配置键说明（插件页配置表单据此渲染） |
| `[limits]` | | 可选，在宿主全局上限内收紧（fuel/内存/表/实例/墙钟） |

### 安全与限额（写插件前值得知道）

- **出站网络 deny-by-default**：manifest `[permissions] egress` 显式列域名才放行；
- **fuel / 内存 / 墙钟**：死循环、内存炸弹会被硬闸（`BudgetExceeded` / trap），
  manifest `[limits]` 只能收紧不能放宽；
- **插件工具默认只对 big 档模型供给**（`min-tier`，manifest 可收紧不可放宽）：
  装了工具但 agent 看不到？先确认当前模型档位（`model set-tier`）；
- observe 帧内 `tool_invoke` / `secret_get` 恒 `Unavailable`（观察者是脱敏投影，
  不给反查通道）。

### 测试你的插件

`translate` 示例就是一份活的能力面自检清单（mode 切换：echo / host_calls /
egress_probe / fuel_bomb / trap / secret_probe / ws_read / mem_bomb /
host_flood）——开发中可对照它观察宿主在各边界的行为。插件日志在 Dashboard
插件页「日志」按钮可见。

## 版本对应

本包与 nemesisbot 同版本发布：WIT 合同（`sdk/wit/plugin.wit`，v0.1）或 SDK
行为变更时，旧包编出的组件可能无法装入新宿主——宿主升级后请重下同版本
devkit。`Cargo.lock` 已随包附带，构建完全复现发布时验证过的依赖组合。
