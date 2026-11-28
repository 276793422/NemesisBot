// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Tool for discovering what tools, resources, and prompts an MCP server provides.
///
/// Connects to an MCP server via stdio or HTTP, performs the handshake, collects
/// metadata, formats it as markdown, and closes the connection.
pub struct McpDiscoverTool {
    /// BUG-20261123-037：stdio 子进程 cwd 锚点（注册点注入 workspace 根）。
    /// None = 继承进程 cwd（旧行为）。
    cwd: Option<std::path::PathBuf>,
}

impl Default for McpDiscoverTool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpDiscoverTool {
    pub fn new() -> Self {
        Self { cwd: None }
    }

    /// 设置 stdio 子进程 cwd 锚点（BUG-20261123-037：装配期注入 workspace
    /// 根，相对路径命令不随 gateway 部署形态漂移；None = 旧行为）。
    pub fn with_cwd(mut self, cwd: Option<std::path::PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }
}

/// mcp_discover 超时的 LLM 输入面硬上界（生效口径与
/// `tools.exec.max_timeout_secs` 缺省 600s 同族）。上游
/// `one_shot_budget` 纯透传（可放大是有意设计，服务 CLI
/// `mcp discover --timeout` 的用户显式意志）——但工具 args 是模型生成
/// 输入面，工具执行无外层墙钟（工具内部超时是唯一防线）：无上界时
/// prompt 注入可传 `timeout=10^12` 让一次 mcp_discover 挂死整个 agent
/// loop。CLI 路径不经过本函数，不受影响。
pub(super) const MCP_DISCOVER_MAX_TIMEOUT_SECS: u64 = 600;

/// 缺省/显式 0 → 15s（旧行为）；其余 clamp 到 [1, 600]。
pub(super) fn resolve_discover_timeout(raw: Option<u64>) -> u64 {
    match raw {
        None | Some(0) => 15,
        Some(v) => v.clamp(1, MCP_DISCOVER_MAX_TIMEOUT_SECS),
    }
}

#[async_trait]
impl Tool for McpDiscoverTool {
    fn description(&self) -> String {
        "Discover what tools, resources, and prompts an MCP server provides. \
         For stdio-based servers provide the 'command' (executable path); \
         for HTTP-based servers provide the 'url' (e.g. 'http://localhost:8080/mcp'). \
         This tool will connect, query capabilities, and return a formatted summary."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to start the MCP server for stdio-based servers (e.g. '/path/to/server.exe', 'npx', 'python')"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments to pass to the command (stdio only, optional)"
                },
                "url": {
                    "type": "string",
                    "description": "URL of an HTTP-based MCP server (e.g. 'http://localhost:8080/mcp')"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (default: 15, capped at 600)"
                }
            }
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed =
            serde_json::from_str::<serde_json::Value>(args).unwrap_or(serde_json::Value::Null);

        let url = parsed["url"].as_str();
        let command = parsed["command"].as_str();
        let timeout_secs = resolve_discover_timeout(parsed["timeout"].as_u64());

        let result = match (url, command) {
            (Some(url), _) => {
                nemesis_mcp::manager::discover_server_metadata_http(url, timeout_secs).await
            }
            (None, Some(command)) => {
                let tool_args: Vec<String> = parsed["args"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                nemesis_mcp::manager::discover_server_metadata(
                    command,
                    tool_args,
                    vec![],
                    timeout_secs,
                    self.cwd.clone(),
                )
                .await
            }
            (None, None) => {
                return Err("missing required 'command' or 'url' parameter".to_string());
            }
        };

        match result {
            Ok(info) => Ok(format_discovery_result(&info)),
            Err(e) => Err(e),
        }
    }

    /// stdio 臂（`command` 参数）是 LLM 可控的任意进程
    /// spawn（`discover_server_metadata` → `StdioTransport` → 直接
    /// `Command::spawn`）——效果面与 exec 同级，静态 NetworkRequest
    /// （MEDIUM）档让 exec 族全部闸失配：默认配置的 `process_rules.exec`
    /// 危险命令 deny 规则（rm -rf / format / del /s / remove-item
    /// -recurse…）、Layer 2 命令守卫、preexec 扫描、guardian 危级全部照
    /// 不到本工具。dispatch 嗅探本投影把 stdio 形态投成 process_exec
    /// （CRITICAL，与 exec 同管线同闸）；HTTP 臂（url）维持静态
    /// NetworkRequest（SSRF 由 nemesis-security `extract_url` 的
    /// mcp_discover 臂覆盖）。args 非对象 / command 缺席或空白 = None
    /// （旧行为，connector 投影同款 None 语义）。
    fn operation_override(&self, args: &str) -> Option<OperationOverride> {
        let v: serde_json::Value = serde_json::from_str(args).ok()?;
        let command = v.get("command")?.as_str()?.trim();
        if command.is_empty() {
            return None;
        }
        Some(OperationOverride {
            op_type: "process_exec".to_string(),
            force_approval: None,
            receipt_ctx: None,
        })
    }
}

// ---------------------------------------------------------------------------
// CliReferenceTool — CLI 命令按需查询
// ---------------------------------------------------------------------------

/// Tool for looking up NemesisBot CLI commands.
///
/// Without parameters returns a compact overview of all commands.
/// With a `command` parameter returns detailed help for that command area.
///
/// Sync is machine-enforced, not convention: the drift test
/// `nemesisbot/src/commands/cli_reference_drift_tests.rs` introspects the
/// real clap `Cli` command tree and fails red when this hand-written
/// catalog misses a command, misses a direct subcommand, or lacks a
/// `cli_detail` arm (BUG-20261110 drift incident: 16-arm catalog vs 40+
/// real commands produced a "CLI has no vault command" false negative).
/// Adding/renaming a command in `nemesisbot/src/main.rs` must come with
/// the matching arm here — the test is the reminder.
pub struct CliReferenceTool;

impl Default for CliReferenceTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CliReferenceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CliReferenceTool {
    fn description(&self) -> String {
        "Look up NemesisBot CLI commands. Without parameters returns an overview of all commands. \
         Pass a command name for detailed help (e.g. 'model', 'mcp', 'cluster', 'scanner')."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command name for detailed help. Omit for overview of all commands."
                }
            }
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed =
            serde_json::from_str::<serde_json::Value>(args).unwrap_or(serde_json::Value::Null);
        let command = parsed["command"].as_str().unwrap_or("").trim();

        if command.is_empty() {
            Ok(cli_overview())
        } else {
            cli_detail(command)
        }
    }
}

/// (sixth batch): cross-session full-text search over chat history
/// (session_logs). Lazy-indexes on first use (FTS5), falls back to a linear
/// scan if the index DB is unavailable. Read-only.
/// history-search 关：工具整体不编译（FTS 索引随 rusqlite 进出，供给闸）。
#[cfg(feature = "history-search")]
pub struct HistorySearchTool;

#[cfg(feature = "history-search")]
impl Default for HistorySearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "history-search")]
impl HistorySearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "history-search")]
#[async_trait]
impl Tool for HistorySearchTool {
    fn description(&self) -> String {
        "Search past conversation history across ALL sessions (FTS full-text). \
         Returns session key + snippet + timestamp for each hit — use it to find \
         which conversation discussed a topic or contains specific wording. \
         Supports Chinese and English."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search text (words or a phrase)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max hits (default 10, max 100)."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let query = v
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or("Missing 'query' argument")?;
        if query.trim().is_empty() {
            return Err("'query' must not be empty".to_string());
        }
        let limit = v.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize;
        // Lazy full index on first use (mtime-incremental after that).
        crate::history_search::reindex_session_logs();
        let hits = crate::history_search::search(query, limit);
        Ok(crate::history_search::render_hits(&hits))
    }
}

/// CLI 命令概览（`cli_reference` 无参形态）。`pub`：nemesisbot 侧漂移守卫
/// 测试（`cli_reference_drift_tests`）消费同一真相——概览文本必须含全部
/// 非 hidden 顶层命令名。
pub fn cli_overview() -> String {
    r#"## NemesisBot CLI 命令概览

| 命令 | 说明 | 关键子命令 |
|------|------|-----------|
| onboard | 初始化配置与工作空间 | default (--local) |
| gateway | 启动网关服务（Web UI + 全部子系统） | --local |
| run | headless 单任务（跑完即退，无端口） | --format json, --mode plan |
| acp | ACP 编辑器接入（stdio JSON-RPC） | — |
| mcp-serve | 把本 bot 暴露为 stdio MCP 服务器 | — |
| agent | Agent 配置管理 | set |
| status | 显示系统状态 | — |
| assets | 资产盘点快照 / AI-BOM（只读） | inventory, bom |
| history | 会话历史全文搜索 | search, reindex |
| session | 会话管理（轮次边界分叉） | list, show, fork |
| channel | 管理通信通道 | list, enable, disable, status, web, web-socket, external |
| cluster | 管理集群 | status, config, info, peers, pair, token, init, enable, disable, reset, identity, firewall, node, roaming |
| cors | 管理 CORS 配置 | list, add, remove, dev-mode, show, validate |
| model | 管理 LLM 模型 | add, list, remove, default, set-tier, probe, prices |
| cron | 管理定时任务 | list, add, remove, enable, disable |
| mcp | 管理 MCP 服务器 | list, add, remove, test, tools, resources, prompts, discover |
| security | 管理安全设置 | status, enable, disable, config, audit, rules, approvals, scanner, test, approve, deny, pending |
| scanner | 管理病毒扫描引擎 | list, add, remove, check, install, clamav |
| log | 管理日志配置 | llm (enable/disable/status/config/type), general (enable/disable/level/file/console) |
| auth | 管理认证 | login, logout, status |
| credentials | 模型 API key 迁移 credentials.yaml | import |
| vault | 凭据 vault（加密秘密存储，list 永不见值） | set, list, remove, migrate, bind, unbind, bindings |
| skills | 管理技能 | list, search, install, verify, remove, source, add-source, learn |
| forge | 管理自学习模块 | status, enable, disable, reflect, list, learn, test, replay-extract, replay-run, optimize |
| profile | 用户习惯画像 | list, enable, disable, veto, forget, pin, move |
| connector | 连接器注册表 / 回执 / 台账 | list, receipts, ledger, demo, sign, keygen, trust |
| issue | 看板 issue 管理 | create, list, get, assign, status, reopen, comment, stats |
| mail | 邮件基础设施 | status, list, send, addr |
| autopilot | 看板定时建单规则（cron） | list, create, update, enable, disable, remove, run, runs |
| dossier | 任务档案导出（自包含目录） | export |
| workflow | 管理工作流 | list, run, status, template, validate |
| sandbox | Sandboxie 沙盒管理（执行体隔离） | install, start, stop, status, pending, commit, clear, kill |
| quic | QUIC 漫游管理（与集群解耦） | relay, keygen, status |
| system | 系统集成（安装器契约面） | register, unregister, status, cert |
| vm | VM 环境自举（QEMU） | install, enable-accel, status, build-image |
| eval | 沙盒安全评估（风险三分类） | prompt, skill, rules |
| ziwei | 紫微系统（三方程序受控接管） | list, show, setup, attach, detach, invoke |
| voice | 本地语音管线 | status, setup, tts, stt, chat, devices |
| memory | 管理增强内存 | enable, disable, status |
| loop | 自配速 durable loop（别名 loops） | create, list, show, pause, resume, remove |
| persona | AI 人格管理 | list, search, install, activate, remove, current, restore |
| plugin | WASM 插件管理 | install, upgrade, enable, disable, list, sign, trust |
| estop | 急停开关（冻结全部 agent 活动） | --release, --status |
| selfupdate | 版本自切换通道 | check, apply, status |
| migrate | OpenClaw 数据迁移 | --dry-run |
| dashboard | 打开 Dashboard UI | — |
| shutdown | 优雅关机 | — |
| version | 显示版本信息 | — |

使用方式: `nemesisbot <命令> <子命令> [参数]`
示例: `nemesisbot model add --model zhipu/glm-4.7 --key YOUR_KEY --default`

查询具体命令的详细用法请传入 command 参数。
本目录由漂移守卫测试与真实 CLI 枚举对齐；如仍疑有滞后，请以 `nemesisbot --help` 实测为准。"#.to_string()
}

/// CLI 命令详情（`cli_reference {"command": name}` 形态）。`pub`：同
/// [`cli_overview`]——漂移守卫测试对每个非 hidden 顶层命令断言本函数
/// Ok（match 臂在场）且详情文本含全部直接子命令名。
pub fn cli_detail(command: &str) -> Result<String, String> {
    match command.to_lowercase().as_str() {
        "model" => Ok(r#"## model — 管理 LLM 模型

用法: `nemesisbot model <子命令>`

子命令:
  add       添加模型配置
            --model <vendor/model>（必填）如 zhipu/glm-4.7
            --key <api-key>        API 密钥
            --base <url>           自定义 API 地址
            --proxy <url>          代理地址
            --auth <method>        认证方式
            --default              设为默认模型
  list      列出已配置的模型
            --verbose              显示详细信息
  remove    删除模型配置
            <name>                 模型名称
            --force                跳过确认
  default   显示当前默认模型
  set-tier <name> <tier>    设能力档位（auto/mini/normal/big，覆盖自动检测）
  set-size <name> <量>      设参数量（如 30B，辅助 auto 检测）
  set-real-name <name> <名> 设真实模型名（opaque 别名辅助检测）
  set-stream <name> <auto|yes|no>  per-model 流式开关
  set-compact-context <name> <N>   per-model 绝对压缩点（token）
  set-effort <name> <档位>  设推理努力档位（off 清除）
  probe <name>              能力探针（8 题实测写回 tier + vision_probe）
  catalog-update            拉取 models.dev 目录缓存（窗口/输出上限）
  prices <操作>             价目表管理:
    list / update / import / add / remove

示例:
  nemesisbot model add --model openai/gpt-4o --key sk-xxx --default
  nemesisbot model list --verbose
  nemesisbot model remove gpt-4o --force"#
            .to_string()),

        "mcp" => Ok(r#"## mcp — 管理 MCP 服务器

用法: `nemesisbot mcp <子命令>`

子命令:
  list                          列出已配置的 MCP 服务器
  add -n <名称> -c <命令>       添加 MCP 服务器
            --args <参数>        启动参数
            --env <变量>         环境变量（KEY=VALUE）
            --timeout <秒>       超时时间（默认 30）
  remove <名称>                 删除 MCP 服务器
  test <名称>                   测试服务器连接
  inspect <名称>                查看服务器配置详情
  tools <名称>                  列出服务器提供的工具
  resources <名称>              列出服务器提供的资源
  prompts <名称>                列出服务器提供的提示词
  discover --command <路径>     发现 MCP 服务器能力（stdio 模式）
            --url <URL>          发现 MCP 服务器能力（HTTP 模式）
            --args <参数>        启动参数（stdio）
            --timeout <秒>       超时时间（默认 15）

示例:
  nemesisbot mcp add -n desktop -c C:\AI\MCP\desktop-mcp.exe
  nemesisbot mcp tools desktop
  nemesisbot mcp discover --command C:\AI\MCP\server.exe"#
            .to_string()),

        "channel" => Ok(r#"## channel — 管理通信通道

用法: `nemesisbot channel <子命令>`

子命令:
  list                          列出所有通道及状态
  enable <名称>                 启用通道
  disable <名称>                禁用通道
  status <名称>                 查看通道详情

  web <操作>                    Web 通道管理:
    auth                        交互式设置认证令牌
    auth-set <token>            直接设置令牌
    auth-get                    查看当前令牌（掩码）
    host <地址>                 设置服务器地址
    port <端口>                 设置端口
    status / config / clear     状态/配置/清除令牌

  web-socket <操作>            WebSocket 通道管理:
    setup / config              设置/查看配置
    set <key> <value>           设置配置项
    get <key>                   获取配置项

  external <操作>               External 通道管理:
    setup / config / test       设置/配置/测试
    set <key> <value>           设置配置项
    get <key>                   获取配置项

示例:
  nemesisbot channel list
  nemesisbot channel enable discord
  nemesisbot channel web port 49000"#
            .to_string()),

        "cluster" => Ok(r#"## cluster — 管理集群

用法: `nemesisbot cluster <子命令>`

子命令:
  status                        显示集群状态
  config                        显示/修改集群配置
    --udp-port / --rpc-port / --broadcast-interval
  info                          显示/修改本节点信息
    --name / --role / --category / --tags / --address / --capabilities
  init                          初始化集群
    --name / --role / --category / --tags / --address / --capabilities
  enable / disable / start / stop   启用/禁用集群
  reset --hard                  重置集群配置

  pair <地址>                   配对新对端（自动探测 UDP/RPC 端口，经 RPC 取真实 ID 写入 peers.toml）

  identity <操作>               节点身份管理:
    show                        显示运行时身份（peers.toml [node] 段 + 覆盖态）

  firewall <操作>               集群端口防火墙规则管理

  node                          启动轻量集群节点（发现+RPC，无 LLM）
    --id / --port / --name / --role / --category

  roaming <操作>                iroh 跨网漫游管理（身份密钥归 `quic keygen`）

  peers <操作>                  管理对等节点:
    list / add / remove / enable / disable
    add --id <ID> --name <名称> --address <地址>

  token <操作>                  管理 RPC 认证令牌:
    generate --length 32 --save   生成令牌
    show --full                   显示令牌
    set <token> / verify <token>  设置/验证令牌
    revoke                        撤销令牌

示例:
  nemesisbot cluster init --name bot1 --role worker
  nemesisbot cluster peers list
  nemesisbot cluster token generate --save"#
            .to_string()),

        "skills" => Ok(r#"## skills — 管理技能

用法: `nemesisbot skills <子命令>`

子命令:
  list                          列出已安装的技能
  search [关键词] --limit <N>   搜索远程技能
  install <技能>                安装技能
  remove <名称>                 删除技能
  show <名称>                   查看技能详情
  validate <路径>               验证技能文件
  add-source <url>              添加技能源（GitHub）
  install-builtin [名称]        安装内置技能
  list-builtin                  列出可用的内置技能
  verify [名称]                 lockfile 漂移检测（缺省全量）
  install-clawhub <技能>        从 openclaw/skills 走统一漏斗安装（旧命令名保留）
  learn <来源>                  从目录/URL/对话/笔记学习并生成技能

  source <操作>                 管理技能源:
    list / add <url> / remove <名称>

  cache <操作>                  管理搜索缓存:
    stats / clear

示例:
  nemesisbot skills search weather
  nemesisbot skills install clawhub/author/weather
  nemesisbot skills list"#
            .to_string()),

        "forge" => Ok(r#"## forge — 管理自学习模块

用法: `nemesisbot forge <子命令>`

子命令:
  status                        显示 forge 状态
  enable / disable              启用/禁用
  reflect                       手动触发反思
  list --type <类型>            列出制品（默认 all）
  evaluate <id>                 评估制品
  export [id] --output <路径> --all  导出制品

  learning <操作>               学习管理:
    status / enable / disable
    history --limit <N>

  learn [话题]                  学习对话（扫描→组包→四阶段对话→沉淀）
  test <技能> [--test 名]       技能行为回归测试（A 线回归网；无 tests 段诚实跳过）
  replay-extract                turn WAL 失败记录提取为重放用例（C 线）
  replay-list                   列出重放用例（pending/confirmed）
  replay-confirm [名]           确认 pending 用例入回归集
  replay-run                    重放已确认失败用例（断言零错误）
  optimize <技能> --op <算子>   微优化候选生成（D 线；reorder_constraints/add_example/clarify_constraints/inject_failure_case）
  insights <操作>               洞察台账管理

示例:
  nemesisbot forge status
  nemesisbot forge reflect
  nemesisbot forge list"#
            .to_string()),

        "cron" => Ok(r#"## cron — 管理定时任务

用法: `nemesisbot cron <子命令>`

子命令:
  list                          列出所有任务
  add -n <名称> -m <消息>       添加任务
            --every <秒>         间隔执行
            --cron <表达式>      Cron 表达式执行
            --deliver            存储兼容旗标（不抑制投递；响应恒投递，去向由 --channel/--to 决定）
            --to <接收者>        指定接收者
            --channel <通道>     指定通道
  remove <id>                   删除任务
  enable <id>                   启用任务
  disable <id>                  禁用任务

示例:
  nemesisbot cron add -n "每日问候" -m "早上好" --cron "0 9 * * *"
  nemesisbot cron list
  nemesisbot cron remove abc123"#
            .to_string()),

        "security" => Ok(r#"## security — 管理安全设置

用法: `nemesisbot security <子命令>`

子命令:
  status                        显示安全状态
  enable / disable              启用/禁用安全模块
  edit                          编辑安全配置
  config-reset                  重置为默认配置

  config <操作>                 配置管理:
    show / edit / reset

  audit <操作>                  审计日志:
    show --limit <N>             查看日志
    export <文件>                导出日志
    denied                       查看被拒绝的操作

  rules <操作>                  安全规则:
    list [类型]                  列出规则
    add <类型> <操作> --pattern <模式> --action <deny/allow>
    remove <类型> <操作> <索引>
    test <类型> <操作> <目标>
  类型: file, directory, process, network, hardware, registry

  test --tool <工具> --args <JSON>  测试安全检查
  approve <id>                  批准待审批操作
  deny <id> [原因]              拒绝待审批操作
  pending                       列出待审批操作

  scanner <操作>                病毒扫描引擎管理（独立 `scanner` 命令更全）
  approvals <操作>              审批记忆规则表:
    list / clear                「总是允许」落盘规则（approval_rules.json）

  calibrate                     安全校准：样本批量重放过规则管线（永不执行）

示例:
  nemesisbot security status
  nemesisbot security rules list
  nemesisbot security approve 123"#
            .to_string()),

        "scanner" => Ok(r#"## scanner — 管理病毒扫描引擎

用法: `nemesisbot scanner <子命令>`

子命令:
  list                          列出所有引擎
  add <名称> --url <URL> --path <路径> --address <地址>  添加引擎
  remove <名称>                 删除引擎
  check                         检查所有引擎的安装状态
  install [--dir <目录>]        安装所有待安装引擎

  <引擎名> <操作>               引擎级操作（如 clamav）:
    install [--force] [--url <URL>] [--dir <目录>]  安装
    enable / disable                                启用/禁用
    update                                          更新病毒库
    test <文件路径>                                  测试扫描
    info                                            引擎详情

示例:
  nemesisbot scanner list
  nemesisbot scanner check
  nemesisbot scanner clamav install
  nemesisbot scanner clamav test /path/to/file"#
            .to_string()),

        "log" => Ok(r#"## log — 管理日志配置

用法: `nemesisbot log <子命令>`

LLM 日志:
  llm enable / disable          启用/禁用 LLM 日志
  llm status                    查看状态
  llm config --detail-level <级别> --log-dir <目录>  配置
  llm type <raw|default>        设置日志类型（原始 JSON / Markdown 摘要）

通用日志:
  general enable / disable       启用/禁用
  general status                 查看状态
  general level <级别>           设置日志级别（debug/info/warn/error）
  general file <路径>            设置日志文件路径
  general console                切换控制台输出

兼容性别名:
  log enable / disable / status / config / set-level
  log enable-file / disable-file / enable-console / disable-console

示例:
  nemesisbot log llm enable
  nemesisbot log llm type raw
  nemesisbot log general level debug"#
            .to_string()),

        "auth" => Ok(r#"## auth — 管理认证

用法: `nemesisbot auth <子命令>`

子命令:
  login --provider <名称>       登录（OAuth 或粘贴令牌）
            --device-code       使用设备码流程
  logout --provider <名称>      登出（省略名称则登出全部）
  status                        查看认证状态

示例:
  nemesisbot auth login --provider openai
  nemesisbot auth status"#
            .to_string()),

        "memory" => Ok(r#"## memory — 管理增强内存

用法: `nemesisbot memory <子命令>`

子命令:
  enable    启用增强内存（需要 plugin_onnx.dll 在 plugins/ 目录）
  disable   禁用增强内存
  status    查看内存系统状态

示例:
  nemesisbot memory status
  nemesisbot memory enable"#
            .to_string()),

        "workflow" => Ok(r#"## workflow — 管理工作流

用法: `nemesisbot workflow <子命令>`

子命令:
  list                          列出工作流
  run <名称> [key=value ...]    运行工作流
  status [执行ID]               查看执行状态
  validate <文件路径>           验证工作流定义

  template <操作>               模板管理:
    list                        列出可用模板
    show <名称>                 查看模板详情
    create <模板> --output <路径>  从模板创建

示例:
  nemesisbot workflow list
  nemesisbot workflow run my-flow input=hello
  nemesisbot workflow template list"#
            .to_string()),

        "cors" => Ok(r#"## cors — 管理 CORS 配置

用法: `nemesisbot cors <子命令>`

子命令:
  list                          列出所有允许的来源
  add <来源> --cdn              添加允许的来源（--cdn 添加为 CDN 域名）
  remove <来源> --cdn           删除来源
  show                          显示完整 CORS 配置
  validate <来源>               验证来源是否被允许

  dev-mode <操作>               开发模式管理:
    enable / disable / status   允许所有 localhost 来源

示例:
  nemesisbot cors add https://example.com
  nemesisbot cors dev-mode enable"#
            .to_string()),

        "status" => Ok(r#"## status — 显示系统状态

用法: `nemesisbot status`

显示当前系统配置和运行状态。"#
            .to_string()),

        "version" => Ok(r#"## version — 显示版本信息

用法: `nemesisbot version`

显示 NemesisBot 版本号和构建信息。"#
            .to_string()),

        "onboard" => Ok(r#"## onboard — 初始化配置与工作空间

用法: `nemesisbot onboard default [--local]`

释放默认配置模板与工作空间模板（不覆盖已有文件）。`--local` 使用当前目录 `./.nemesisbot` 而不是 `~/.nemesisbot`。"#
            .to_string()),

        "gateway" => Ok(r#"## gateway — 启动网关服务

用法: `nemesisbot gateway [--local]`

装配并运行 Web UI + 消息通道 + Agent 引擎 + 安全管线等全部子系统。`--local` 配置落当前目录 `./.nemesisbot`（多实例部署）。"#
            .to_string()),

        "run" => Ok(r#"## run — headless 单任务（跑完即退）

用法: `nemesisbot run [任务文本|-] [--format text|json] [--mode plan|build] [--workspace DIR] [--max-turns N] [--model ALIAS]`

无端口、无 gateway。`-` 或缺省读 stdin。`--format json` 输出 NDJSON 事件流（tool_start/tool_end/turn/final/error）；stdout 只承载最终回复，日志一律 stderr。`--mode plan` 只读模式（写类拦截，仅 plans/ 放行）。退出码 0=final / 1=装配失败或仅 error。安全 8 层与 gateway 同源。"#
            .to_string()),

        "acp" => Ok(r#"## acp — ACP 编辑器接入

用法: `nemesisbot acp`

以 stdio ndjson JSON-RPC 提供 ACP server（Zed 等编辑器客户端接入）。审批桥接到编辑器 permission 请求（超时/断连=拒绝）。"#
            .to_string()),

        "mcp-serve" => Ok(r#"## mcp-serve — stdio MCP 服务器形态

用法: `nemesisbot mcp-serve`

把 NemesisBot 能力暴露为 stdio MCP 服务器（Claude Code / Cursor 等零改动接入；安全管线同源生效）。"#
            .to_string()),

        "agent" => Ok(r#"## agent — Agent 配置管理

用法: `nemesisbot agent <子命令>`

子命令:
  set    设置 agent 配置项"#
            .to_string()),

        "assets" => Ok(r#"## assets — 资产盘点快照 / AI-BOM（只读）

用法: `nemesisbot assets <子命令>`

子命令:
  inventory    打印资产盘点快照原文（gateway 落盘的 inventory.json）
  bom          打印 AI-BOM 视图（framework/model/skill/tool_plugin/mcp 五类投影）"#
            .to_string()),

        "history" => Ok(r#"## history — 会话历史全文搜索

用法: `nemesisbot history <子命令>`

子命令:
  search <查询>    全文搜索全部会话聊天日志（FTS）
  reindex          强制全量重建索引（常规为按 mtime 增量）"#
            .to_string()),

        "session" => Ok(r#"## session — 会话管理（轮次边界分叉）

用法: `nemesisbot session <子命令>`

子命令:
  list               列出本 home 的会话（key / turns / messages / updated）
  show <key>         显示某会话的轮次边界表（选 fork --at 用）
  fork <key> --at <N>  在轮次边界分叉出新会话 key"#
            .to_string()),

        "credentials" => Ok(r#"## credentials — 模型 API key 迁移 credentials.yaml

用法: `nemesisbot credentials <子命令>`

子命令:
  import    把 config.json 明文模型 key 迁移到 workspace/config/credentials.yaml（0600）并改写为 `yaml:<别名>` 引用（幂等）

注意: 此处的 credentials 是模型 API key；与 `auth` 的 OAuth 凭据存储、`vault` 的通用秘密存储是三回事。"#
            .to_string()),

        "vault" => Ok(r#"## vault — 凭据 vault（通用加密秘密存储）

用法: `nemesisbot vault <子命令>`

子命令:
  set <别名>                     写入/轮换别名（secret 隐藏输入不回显）
  list                           列出全部别名与元数据（永不显示值）
  remove <别名>                  删除别名（需确认）
  migrate                        模型 API key 迁入 vault（配置回写 `vault:<别名>`，幂等）
  bind <别名> --tool <T> --slot <S>  绑定域白名单（该工具槽位免审批卡使用；出域审计拒绝）
  unbind <别名>                  解除绑定（--tool/--slot 可选，缺省批量删）
  bindings                       列出全部域绑定"#
            .to_string()),

        "profile" => Ok(r#"## profile — 用户习惯画像（自进化 B 线）

用法: `nemesisbot profile <子命令>`

子命令:
  list                 显示习惯条目与开关状态
  enable               启用画像（记录 enabled_at 仅向前锚点）
  disable              停用画像（既有条目保留）
  veto <子串>          否决含子串的习惯（负样本）
  forget <子串>        遗忘含子串的习惯（可重生）
  pin <子串> [--unpin] 钉住/解除钉住（不衰减）
  move <子串>          在对话/项目画像间移动习惯条目"#
            .to_string()),

        "connector" => Ok(r#"## connector — 连接器注册表 / 回执 / 台账

用法: `nemesisbot connector <子命令>`

子命令:
  list                       列出已注册连接器
  receipts                   最近回执（新→旧）
  ledger                     纸经纪台账（cash/positions/orders）
  demo                       30 秒零账号零网络信任承诺演示
  sign <目录>                签名 connector.toml（Ed25519 分离签名）
  keygen                     生成 Ed25519 密钥对
  trust / trust-list / trust-revoke    信任库管理（与 skills 共享）
  add-source / remove-source / sources  远程源管理（git clone 本地缓存）
  search <查询>              跨远程源搜索连接器
  install <源/名>            从远程源安装进工作区注册表
  add                        在工作区注册表创建连接器（先验证后落盘）
  enable / disable <名>      启用/禁用（sidecar 位，manifest 不动）
  remove <名>                删除连接器目录（回执/台账保留）"#
            .to_string()),

        "issue" => Ok(r#"## issue — 看板 issue 管理（托管 agent 看板）

用法: `nemesisbot issue <子命令>`

子命令:
  create <标题> [--priority N]    建单（NB-N 自动编号）
  list / get <编号>               列出 / 查看单据
  assign <编号> --assignee <a>    指派（typed `type:id` 形态；--clear 解除）
  status <编号> <状态>            状态机转移（backlog/todo/in_progress/in_review/done/cancelled）
  reopen <编号>                   取消单重开回 backlog
  comment <编号> <内容>           评论
  projects                        列出项目
  stats                           按状态统计"#
            .to_string()),

        "mail" => Ok(r#"## mail — 邮件基础设施

用法: `nemesisbot mail <子命令>`

子命令:
  status            DLL 在场/总闸/名册/句柄（只读）
  list              列出邮件（可过滤）
  send              发送邮件（正文来自 --body 或 --stdin）
  addr <操作>       地址簿管理"#
            .to_string()),

        "autopilot" => Ok(r#"## autopilot — 看板定时建单规则（cron 触发）

用法: `nemesisbot autopilot <子命令>`

子命令:
  list                            列出全部规则
  create <名> --cron <表达式> --title <模板>   建规则（{date} 模板变量；--target 定向；--auto-plan 建单即拆解）
  update <id>                     更新规则字段（只改提供的字段）
  enable / disable <id>           启用/禁用规则
  remove <id>                     删除规则
  run <id>                        手动触发一次（定向规则需运行中集群，CLI 诚实拒绝）
  runs <id>                       运行历史（产出的 issue，新→旧）"#
            .to_string()),

        "dossier" => Ok(r#"## dossier — 任务档案导出（自包含目录）

用法: `nemesisbot dossier export <NB-N 或 --project-id N> [--out DIR]`

子命令:
  export    导出单据树（全部后代记录）或整项目记录到 logs/dossiers/（README/timeline/audit jsonl/records/inbox-unplaced/usage.csv）"#
            .to_string()),

        "plugin" => Ok(r#"## plugin — WASM 插件管理

用法: `nemesisbot plugin <子命令>`

子命令:
  install <目录> [--yes]              安装（九步装配漏斗；--allow-unsigned 显式豁免）
  upgrade <目录> [--yes]              升级（重跑漏斗，数据目录保留）
  uninstall <slug>                    卸载（删载荷留数据目录）
  enable / disable <slug>             启用/禁用（改实例配置，gateway 重启后生效）
  list / info [slug]                  已装列表 / 详情（磁盘只读）
  sign <manifest> --key <hex64>       签名 plugin.toml（Ed25519）
  keygen                              生成 Ed25519 密钥对
  trust / trust-list / trust-revoke / trust-remove    信任库管理
  panel-llm [on|off]                  面板 LLM 总闸（无参=查看）"#
            .to_string()),

        "sandbox" => Ok(r#"## sandbox — Sandboxie 沙盒管理（执行体隔离 Layer 2）

用法: `nemesisbot sandbox <子命令>`

子命令:
  install                        下载+校验+解压 Sandboxie 运行时（无 UAC）
  start                          激活引擎（装驱动+服务+ini+启动 SbieSvc；触发 UAC）
  stop [--purge]                 停止+卸载引擎（--purge 连文件删）
  status                         安装/服务状态
  pending [--box-name N]         列出盒里待提交工作区文件
  commit [--all|<子串>...]       待提交文件回真盘（镜像写是「写盘即生效」承诺本体）
  clear [--force]                清空盒内容（丢弃待提交）
  kill [--box-name N]            强杀盒内全部进程并弃内容（eval/executor 失控用）"#
            .to_string()),

        "quic" => Ok(r#"## quic — QUIC 漫游管理（与集群解耦）

用法: `nemesisbot quic <子命令>`

子命令:
  relay             启动自建 QUIC 中继服务器（内网穿透；运行面在 plugin_roaming 插件）
  keygen            生成本机 iroh 漫游身份密钥
  status            查看配置解析产物与证书状态（只读）"#
            .to_string()),

        "system" => Ok(r#"## system — 系统集成（安装器契约面）

用法: `nemesisbot system <子命令>`

子命令:
  register           注册系统集成（ARP 卸载面板/开机自启/快捷方式；逐项 best-effort）
  unregister         注销系统集成（幂等）
  status             查看集成状态（ARP/自启/快捷方式各条目）
  cert               信任根证书库管理（独立操作，默认永不安装）"#
            .to_string()),

        "vm" => Ok(r#"## vm — VM 环境自举（QEMU）

用法: `nemesisbot vm <子命令>`

子命令:
  install            安装 QEMU 环境（平台矩阵配置分派；幂等）
  enable-accel       硬件加速引导（Windows=WHPX 一键；Linux=打印引导命令）
  status             环境自举状态全量（只读探针）
  build-image        构建基础镜像（前台等待）
  sync-status        项目工作副本同步状态（只读）
  commit             工作副本一键提交（VM 修改回拉宿主；冲突指向 Dashboard）
  resync             强制重同步（VM 未回传改动将丢失；交互确认）"#
            .to_string()),

        "eval" => Ok(r#"## eval — 沙盒安全评估（风险三分类）

用法: `nemesisbot eval <子命令>`

子命令:
  prompt <文本> [--fail-on-risk]    沙盒内跑 agent 评估提示词（有风险/安全/未知）
  skill <名称>                      评估已安装技能（workspace→global→builtin 解析）
  rules <操作>                      评估规则管理（new/list/show/add/edit/remove/enable/disable/reset）"#
            .to_string()),

        "ziwei" => Ok(r#"## ziwei — 紫微系统（三方程序受控接管）

用法: `nemesisbot ziwei <子命令>`

config `ziwei.enabled` 运行闸默认关。

子命令:
  list / show <名>           Profile 列表 / 详情
  setup                      进入配置期（步骤化子命令）
  attach                     使用期附加（经 gateway 转发）
  detach                     卸载会话（降险动作）
  status                     活动会话列表
  invoke                     手动触发子工具
  verify                     手动触发附加验证链
  refresh / refresh-confirm  stale Profile 找回 / 找回确认
  revoke                     吊销 profile（终态不可复活）
  remove                     删除 profile（含确认）"#
            .to_string()),

        "voice" => Ok(r#"## voice — 本地语音管线

用法: `nemesisbot voice <子命令>`

子命令:
  status            语音环境状态
  setup             环境部署（下载库+模型）
  download          下载全部语音模型
  tts <文本>        TTS 测试
  stt               STT 听写测试（麦克风→VAD→STT→文本）
  chat              完整语音对话环（麦克风→VAD→STT→TTS→播放）
  devices           列出音频设备"#
            .to_string()),

        "loop" | "loops" => Ok(r#"## loop — 自配速 durable loop（别名 loops）

用法: `nemesisbot loop <子命令>`

子命令:
  create            创建 loop（绑定已有会话；不静默建会话）
  list              列出全部 loop
  show <id>         单个 loop 详情（含 progress_log）
  pause <id>        暂停（人操作；驱动器停止投递）
  resume <id>       恢复（paused/parked → active）
  remove <id>       删除 loop 状态文件"#
            .to_string()),

        "persona" => Ok(r#"## persona — AI 人格管理

用法: `nemesisbot persona <子命令>`

子命令:
  list              列出已装人格
  search <关键词>   搜索人格市场
  install <名>      从市场安装人格
  activate <名>     激活人格（Agent 用新 system_prompt 重启，不重启进程）
  remove <名>       移除已装人格
  current           显示当前激活人格
  restore           恢复默认人格"#
            .to_string()),

        "estop" => Ok(r#"## estop — 急停开关（Kill Switch）

用法: `nemesisbot estop [--release] [--status]`

一键冻结全部 agent 活动（CLI/托盘/Dashboard/WSAPI 四入口同一状态）。裸 `estop` 触发；`--release` 释放恢复；`--status` 查询。"#
            .to_string()),

        "selfupdate" => Ok(r#"## selfupdate — 版本自切换通道

用法: `nemesisbot selfupdate <子命令>`

子命令:
  check             对照当前构建 commit 检查最新 Release 资产（离线可用）
  apply             经运行中 gateway 应用更新（无待发包则下载；v4 签名验证在 gateway 侧）
  status            当前身份 + 待处理 journal / 本地待发包（离线可用）"#
            .to_string()),

        "migrate" => Ok(r#"## migrate — OpenClaw 数据迁移

用法: `nemesisbot migrate [--dry-run] [--config-only] [--workspace-only]`

从 OpenClaw 迁移配置与工作空间数据。`--dry-run` 仅预览；`--config-only`/`--workspace-only` 只迁移对应部分。"#
            .to_string()),

        "dashboard" => Ok(r#"## dashboard — 打开 Dashboard UI

用法: `nemesisbot dashboard`

自动启动网关（未运行时）并打开 Dashboard 界面。"#
            .to_string()),

        "shutdown" => Ok(r#"## shutdown — 优雅关机

用法: `nemesisbot shutdown`

触发运行中服务的优雅停机协议（不硬杀）。"#
            .to_string()),

        _ => Err(format!(
            "Unknown command '{}'. Call cli_reference without parameters to see all commands. 注意：此目录可能滞后于实际 CLI，请以 `nemesisbot --help` 实测为准。",
            command
        )),
    }
}

pub(super) fn format_discovery_result(result: &nemesis_mcp::manager::DiscoveryResult) -> String {
    let mut lines = Vec::new();

    // Server info
    if let Some(ref info) = result.server_info {
        lines.push(format!("## MCP Server: {} v{}\n", info.name, info.version));
    } else {
        lines.push("## MCP Server (unknown)\n".to_string());
    }

    // Tools
    if result.tools.is_empty() {
        lines.push("### Tools\nNone.\n".to_string());
    } else {
        lines.push(format!("### Tools ({})\n", result.tools.len()));
        for tool in &result.tools {
            let desc = tool.description.as_deref().unwrap_or("no description");
            lines.push(format!("- **{}**: {}", tool.name, desc));

            // Parameter summary
            if let Some(props) = tool
                .input_schema
                .get("properties")
                .and_then(|p| p.as_object())
            {
                let required: Vec<&str> = tool
                    .input_schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let param_summary: Vec<String> = props
                    .iter()
                    .map(|(name, schema)| {
                        let type_str = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                        if required.contains(&name.as_str()) {
                            format!("{}* ({})", name, type_str)
                        } else {
                            format!("{} ({})", name, type_str)
                        }
                    })
                    .collect();

                if !param_summary.is_empty() {
                    lines.push(format!("  - Parameters: {}", param_summary.join(", ")));
                }
            }
        }
        lines.push(String::new());
    }

    // Resources
    if result.resources.is_empty() {
        lines.push("### Resources\nNone.\n".to_string());
    } else {
        lines.push(format!("### Resources ({})\n", result.resources.len()));
        for res in &result.resources {
            let desc = res.description.as_deref().unwrap_or("");
            if desc.is_empty() {
                lines.push(format!("- **{}** ({})", res.name, res.uri));
            } else {
                lines.push(format!("- **{}** ({}): {}", res.name, res.uri, desc));
            }
        }
        lines.push(String::new());
    }

    // Prompts
    if result.prompts.is_empty() {
        lines.push("### Prompts\nNone.".to_string());
    } else {
        lines.push(format!("### Prompts ({})\n", result.prompts.len()));
        for prompt in &result.prompts {
            let desc = prompt.description.as_deref().unwrap_or("no description");
            lines.push(format!("- **{}**: {}", prompt.name, desc));
            if !prompt.arguments.is_empty() {
                let args: Vec<String> = prompt
                    .arguments
                    .iter()
                    .map(|a| {
                        let req = if a.required.unwrap_or(false) { "*" } else { "" };
                        let desc = a.description.as_deref().unwrap_or("");
                        if desc.is_empty() {
                            format!("{}{}", a.name, req)
                        } else {
                            format!("{}{} ({})", a.name, req, desc)
                        }
                    })
                    .collect();
                lines.push(format!("  - Arguments: {} (* = required)", args.join(", ")));
            }
        }
    }

    lines.join("\n")
}

/// 渲染：mcp_list 全量清单（历史字节格式不变，保输出兼容；空 = 诚实
/// 空表文案）。消费方：tool_dispatch mcp_list 拦截臂（BUG-20261118-095
/// ——模型面发现通路经 dispatch 各闸过滤后渲染，不再走快照）。
pub(crate) fn render_mcp_list(entries: &[McpToolSnapshotEntry]) -> String {
    if entries.is_empty() {
        return "No MCP tools are currently registered.".to_string();
    }
    let mut lines = vec![format!(
        "## Registered MCP Tools ({})
",
        entries.len()
    )];
    for entry in entries {
        lines.push(format!("- **{}**: {}", entry.name, entry.description));
    }
    lines.join(
        "
",
    )
}

/// 渲染：mcp_list 按名查单个工具完整 schema（历史字节格式不变）。
pub(crate) fn render_mcp_tool_detail(entry: &McpToolSnapshotEntry) -> String {
    format!(
        "## MCP Tool `{}`

- **description**: {}
- **read_only_hint**: {}
- **destructive_hint**: {}
- **parameters**: {}

Call it with `mcp_call` {{\"tool\": \"{}\", \"args\": {{...}}}}.",
        entry.name,
        entry.description,
        entry
            .read_only_hint
            .map(|b| b.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        entry
            .destructive_hint
            .map(|b| b.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        serde_json::to_string_pretty(&entry.parameters).unwrap_or_default(),
        entry.name
    )
}

/// Tool for listing all currently registered MCP tools.
///
/// **执行走 dispatch 拦截，不走本 execute**：tool_dispatch 在闸门瀑布入口
/// 按 dispatch 各闸（hidden/独占/policy/tier + mcp_call 自身 lazy 闸）
/// 同源现读过滤后，从 registry 真值渲染清单/单工具 schema
/// （BUG-20261118-095——此前 execute 直读快照：生产装配从未把 loop 快照
/// 接进 SharedToolConfig 槽 → 恒空 Arc 死工具；接通形态下快照无可见性
/// 过滤 → 隐藏工具面泄漏）。本 execute 只在绕过 loop 分发的直调场景
/// 触达（诚实报错的安全网，正常流程死代码）——与 McpCallTool 同形态。
pub struct McpListTool;

#[async_trait]
impl Tool for McpListTool {
    fn description(&self) -> String {
        "List all currently registered MCP tools and their descriptions, or with \"name\"          given, show one tool's full parameter schema. Use this to see what MCP tools are          available in the current session (and how to call them via mcp_call). Hidden or          filtered tools are not listed."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Optional tool name: when given, print that tool's full parameter schema instead of the whole list."
                }
            }
        })
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Err("mcp_list is dispatched by the agent loop (renders the visibility-filtered MCP tool list from the registry); direct invocation is not supported. Use the normal tool-call path.".to_string())
    }
}

/// （加固）：MCP 惰性供给 meta 工具。`mcp_*` 长尾工具
/// schema 不随轮上行时，模型经本工具两步调用：`mcp_list`（可按名查
/// schema）→ `mcp_call {tool, args}`。
///
/// **执行走 dispatch 拦截，不走本 execute**：tool_dispatch 在闸门瀑布入口
/// 解包 {tool, args} 后以真实工具名重走全流程（estop/hidden/tier/安全 8 层
/// /审计 target=真实名）——等价于直连调用。本 execute 只在绕过 loop 分发
/// 的直调场景触达（诚实报错的安全网，正常流程死代码）。
pub struct McpCallTool;

#[async_trait]
impl Tool for McpCallTool {
    fn description(&self) -> String {
        "Call a registered MCP tool by name (lazy-supply meta tool). Two-step usage:          first `mcp_list` (optionally with {\"name\": ...} for a tool's full schema),          then mcp_call with {\"tool\": \"<mcp tool name>\", \"args\": {<tool arguments>}}."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tool": {
                    "type": "string",
                    "description": "Registered MCP tool name (see mcp_list). Must be an mcp_* tool."
                },
                "args": {
                    "type": "object",
                    "description": "Arguments object forwarded verbatim to the target MCP tool (validated by the MCP server's own schema)."
                }
            },
            "required": ["tool"]
        })
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Err("mcp_call is dispatched by the agent loop (unwrapped to the real tool name before execution); direct invocation is not supported. Use the normal tool-call path.".to_string())
    }
}

/// 建设：惰性 schema 供给的取回工具——
/// 两级化供给下工具 defs 只带 name+description（参数折叠为占位），模型
/// 按需以 `{"name": "<tool>"}` 取回完整 schema（取回过的名字进 per-session
/// 解锁账，后续轮次以「已解锁 schema 注记」持续携带）。
///
/// **执行走 dispatch 拦截，不走本 execute**：tool_dispatch 在闸门瀑布入口
/// 查 registry 渲染真实 schema 并记账（未知名/hidden/tier 不可见=诚实
/// 报错）。本 execute 只在绕过 loop 分发的直调场景触达（诚实报错的
/// 安全网，正常流程死代码）——与 McpCallTool 同形态。
pub struct ToolsSchemaGetTool;

#[async_trait]
impl Tool for ToolsSchemaGetTool {
    fn description(&self) -> String {
        "Get the full parameter schema of a registered tool by name (lazy-supply meta tool). In lazy-supply mode tool definitions carry only name+description; call this with {\"name\": \"<tool name>\"} to retrieve a tool's complete schema before calling it. Hidden or tier-filtered tools cannot be fetched."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Registered tool name whose full schema should be retrieved."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Err("tools_schema_get is dispatched by the agent loop (renders the real schema from the registry); direct invocation is not supported. Use the normal tool-call path.".to_string())
    }
}
