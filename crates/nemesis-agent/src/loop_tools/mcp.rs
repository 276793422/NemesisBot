// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Tool for discovering what tools, resources, and prompts an MCP server provides.
///
/// Connects to an MCP server via stdio or HTTP, performs the handshake, collects
/// metadata, formats it as markdown, and closes the connection.
pub struct McpDiscoverTool;

impl Default for McpDiscoverTool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpDiscoverTool {
    pub fn new() -> Self {
        Self
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
/// Keep data in sync with `nemesisbot/src/commands/*.rs`.
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

pub(super) fn cli_overview() -> String {
    r#"## NemesisBot CLI 命令概览

| 命令 | 说明 | 关键子命令 |
|------|------|-----------|
| model | 管理 LLM 模型 | add, list, remove, default |
| mcp | 管理 MCP 服务器 | list, add, remove, test, tools, resources, prompts, discover |
| channel | 管理通信通道 | list, enable, disable, status, web, websocket, external |
| cluster | 管理集群 | status, config, info, peers, token, init, enable, disable, reset |
| skills | 管理技能 | list, search, install, remove, source, add-source, install-builtin |
| forge | 管理自学习模块 | status, enable, disable, reflect, list, evaluate, export, learning |
| cron | 管理定时任务 | list, add, remove, enable, disable |
| security | 管理安全设置 | status, enable, disable, config, audit, rules, test, approve, deny, pending |
| scanner | 管理病毒扫描引擎 | list, add, remove, check, install, clamav |
| log | 管理日志配置 | llm (enable/disable/status/config/type), general (enable/disable/level/file/console) |
| auth | 管理认证 | login, logout, status |
| memory | 管理增强内存 | enable, disable, status |
| workflow | 管理工作流 | list, run, status, template, validate |
| cors | 管理 CORS 配置 | list, add, remove, dev-mode, show, validate |
| status | 显示系统状态 | — |
| version | 显示版本信息 | — |

使用方式: `nemesisbot <命令> <子命令> [参数]`
示例: `nemesisbot model add --model zhipu/glm-4.7 --key YOUR_KEY --default`

查询具体命令的详细用法请传入 command 参数。"#.to_string()
}

pub(super) fn cli_detail(command: &str) -> Result<String, String> {
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

  websocket <操作>              WebSocket 通道管理:
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

        _ => Err(format!(
            "Unknown command '{}'. Call cli_reference without parameters to see all commands.",
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

/// Tool for listing all currently registered MCP tools.
///
/// Reads from a shared snapshot updated by AgentLoop when MCP tools change.
pub struct McpListTool {
    mcp_tools: Arc<parking_lot::RwLock<Vec<McpToolSnapshotEntry>>>,
}

impl McpListTool {
    pub fn new(mcp_tools: Arc<parking_lot::RwLock<Vec<McpToolSnapshotEntry>>>) -> Self {
        Self { mcp_tools }
    }
}

#[async_trait]
impl Tool for McpListTool {
    fn description(&self) -> String {
        "List all currently registered MCP tools and their descriptions, or with \"name\"          given, show one tool's full parameter schema. Use this to see what MCP tools are          available in the current session (and how to call them via mcp_call)."
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

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let tools = self.mcp_tools.read();
        if tools.is_empty() {
            return Ok("No MCP tools are currently registered.".to_string());
        }
        // 按名查询单工具完整 schema（惰性供给的 schema 查询面）。无名 =
        // 全量列表（输出格式与历史版本逐字节一致，保 prompt cache 兼容）。
        let want: Option<String> = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| {
                v.get("name")
                    .and_then(|n| n.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            });
        if let Some(name) = want {
            return match tools.iter().find(|e| e.name == name) {
                Some(entry) => Ok(format!(
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
                )),
                None => Ok(format!(
                    "No MCP tool named `{name}` is registered. Call without arguments to list all registered MCP tools."
                )),
            };
        }
        let mut lines = vec![format!(
            "## Registered MCP Tools ({})
",
            tools.len()
        )];
        for entry in tools.iter() {
            lines.push(format!("- **{}**: {}", entry.name, entry.description));
        }
        Ok(lines.join(
            "
",
        ))
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
