// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: AgentDefaults,
    #[serde(default)]
    pub list: Vec<AgentConfigEntry>,
    /// H7 (U13 half): claude CLI delegation tool settings. Default
    /// disabled (opt-in) — absent section = tool never registered.
    #[serde(default)]
    pub claude_code_tool: ClaudeCodeToolConfig,
    /// I4 (U13 other half): Codex delegation tool settings (same shape).
    #[serde(default)]
    pub codex_tool: CodexToolConfig,
    /// L1 (U19): read-only LSP tool settings. Default disabled (opt-in) —
    /// and even when enabled, the tool registers only if at least one
    /// language server (rust-analyzer/gopls/…) is found on PATH.
    #[serde(default)]
    pub lsp_tool: LspToolConfig,
    /// Y1 (Phase4-a): semantic tool-documentation folding. Default off —
    /// absent section = tool defs render byte-identical to the pre-Y1 build.
    #[serde(default)]
    pub tool_doc_folding: ToolDocFoldingConfig,
    /// G2 (devtool-upgrade 阶段 3): sub-agent spawn settings.
    #[serde(default)]
    pub subagent: SubagentConfig,
    /// I1 (devtool-upgrade 阶段 3): workspace fs-watcher settings.
    #[serde(default)]
    pub fs_watcher: FsWatcherConfig,
    /// T2（L1 codemode）: `code_run` 脚本工具开关与资源限制（默认开，
    /// 2026-10-25 拍板；Dashboard Tools 页可手动关）。
    #[serde(default)]
    pub codemode: CodeModeConfig,
    /// A3（委派持续操控）: delegate 四工具开关与子代理生命上限（默认关）。
    #[serde(default)]
    pub delegate: DelegateToolConfig,
    /// L8（内部专项总计划重评估）：工具调用三态 WAL——工具
    /// admit/start/finish 每态一行 JSONL 落 `<workspace>/logs/turn_wal/`，
    /// 进程被强杀（taskkill /F）后重启扫描补 interrupted 并自动续轮。
    /// 默认开；关闭后完全不落盘。诚实边界：纯 flush 无 fsync，断电不保。
    #[serde(default = "default_true")]
    pub turn_wal: bool,
    /// L10（内部专项总计划重评估）：provider 流式输出观测面——主 turn
    /// LLM 调用经流式 lane（provider 支持则真实增量，否则聚合后合成）发
    /// `provider_stream_event` 易逝帧（SSE + WS push，不入补拉环）。默认
    /// 开；关闭后零事件零开销。运行时改键下一轮生效（每轮新鲜读）。
    #[serde(default = "default_true")]
    pub provider_stream_events: bool,
    /// F8 (devtool-upgrade 阶段 3): tool names hidden from the model's
    /// tool_defs AND rejected at dispatch (double gate). Entries may carry a
    /// trailing `*` wildcard (e.g. `mcp_*` hides every MCP tool). Runtime
    /// edits take effect from the next turn (fresh-read, no restart).
    #[serde(default)]
    pub hidden_tools: Vec<String>,
    /// W7（2026-10-25 加固）：
    /// MCP 工具惰性供给。开 = `mcp_*` 长尾工具
    /// schema 不全量随轮上行（tool_defs 只见 meta 工具 `mcp_call`），模型
    /// 两步走：`mcp_list`（可按名查完整 schema）→ `mcp_call {tool, args}`。
    /// 调用链路与直连等价（dispatch 解包真实工具名后重走全瀑布：安全 8 层
    /// / 审计 target=真实名 / tier 内层校验）。默认 **false**（行为变更
    /// 保守）；运行时改键下一轮生效（fresh-read）。
    #[serde(default)]
    pub mcp_lazy_supply: bool,
    /// 角色目录分档供给配套（2026-09-28 角色目录与分档供给）：`hidden`
    /// = 从 spawn schema 与 dispatch 双闸隐藏的角色 slug（镜像
    /// hidden_tools 的 F8 双闸模型；通配语义不开放——角色目录是静态
    /// 17 项，按名精确匹配）。运行时改键下一轮生效（fresh-read）。
    #[serde(default)]
    pub roles: RolesConfig,
    /// N2 (devtool-upgrade 阶段 4): 小模型专职杂务通道 — a model reference
    /// (alias from `model_list` or `vendor/model`) used for cheap chores.
    /// Sole consumer today: manual `/compact` summarization (E6) — auto
    /// compression stays on the main model (quality-sensitive). Resolved to
    /// a provider at AgentLoop construction; absent/empty/unresolvable =
    /// honest fallback to the main model (warn in the factory, never a
    /// startup blocker). Runtime edits need an Agent restart.
    #[serde(default)]
    pub small_model: Option<String>,
    /// B7（2026-10-13）：guardian LLM 审计专用模型通道（优先于
    /// `small_model`——审计点独立于 /compact 杂务，可用不同档位；两者都
    /// 未配置 = 回落主模型）。Absent/empty/unresolvable = 沿阶梯回落
    /// （small_model → main），never a startup blocker。消费点唯一：
    /// gateway guardian judge 装配（commands/gateway/runtime.rs）。
    #[serde(default)]
    pub guardian_model: Option<String>,
    /// J5 (devtool-upgrade 阶段 6): doom-loop 审批化开关。默认关——turn_guard
    /// 的 escalation 硬停是安全底座。开且 question asker 已装配时，escalation
    /// 触发先发提问卡问用户「继续吗？」：approve = 清签名计数继续；deny /
    /// 超时 / asker 缺失 = 现行为（停轮）。loop 侧 fresh-read（每次 escalation
    /// 现读 config.json），运行时改键下一轮生效，无需重启。
    #[serde(default)]
    pub doom_loop_approval: bool,
    /// J6 (devtool-upgrade 阶段 6): 超限图片自动降采样开关。默认**开**——
    /// 超过 25MB 或最长边超过 8000px 的图片在附加前先降采样（阶梯
    /// 4096→2048→1024，JPEG q85，产物不超过 8MB 落 uploads 内容寻址复用）；
    /// 关 = 原 25MB 硬拒绝。loop 侧 fresh-read（每条消息现读 config.json），
    /// 运行时改键下一轮生效。
    #[serde(default = "default_true")]
    pub image_downscale: bool,
    /// WS9/P22：workspace 写租约开关。默认**开**——主 loop / 项目 loop /
    /// executor 子进程的写类工具经 workspace 级文件锁互斥（宽限 30s 超时
    /// 诚实拒绝；持有进程死亡 OS 自动释放）。**装配期消费**（gateway/
    /// exec_worker 启动时按本键构造租约并包装写类工具）——改键需重启
    /// 生效（与 executor 段同语义，非 loop 侧 fresh-read）。
    #[serde(default = "default_true")]
    pub lease_enabled: bool,
    /// 件4（2026-09-24 HOOK 三合一收口 §6）：纪律闭环总开关。默认
    /// **false**（D5 灰度）——关 = 闸/证伪钩子不注册、`/discipline` 提示
    /// 未启用、任务 marker 不生效。开 = 任务描述含 `[discipline:bugfix]`
    /// 或 `/discipline on` 的会话进入参与态（声明闸 + 证伪预算，见
    /// nemesis-agent::discipline）。
    #[serde(default)]
    pub discipline: DisciplineConfig,
    /// 提示词包体系选择（2026-09-27 prompt-pack pro）。`pro` = 全中文分层
    /// 提示词体系（前置段 → 人格文件 → 行为段池 → 环境段，段落池确定性
    /// 组装，工具描述按模型能力档位两档供给）；`classic` = 既有装配路径
    /// （人格文件拼接 + 固定尾巴，字节级不变）。启动装配型：system prompt
    /// 在启动时构建后随会话冻结，运行时改键需重启（或重建实例）生效。
    /// 空串按未配置 = `pro`；未知非空值按 `classic` 处理并 warn
    /// （fail-safe 到已知旧行为）。解析在 nemesis-agent::prompt。
    #[serde(default = "default_prompt_system")]
    pub prompt_system: String,
    /// P32（cache-warmer）配置节（默认关；语义见 [`CacheWarmerConfig`]）。
    /// typed 字段化以保证 save_config 往返不丢键（vision extra 键回归的
    /// 同类预防——AgentsConfig 无 flatten extra，未声明键会被 typed 保存
    /// 静默抹掉）。
    #[serde(default)]
    pub cache_warmer: CacheWarmerConfig,
    /// NB-122 批（2026-10-16）④：单次工具调用墙钟看门狗（秒）。0 = 关。
    /// 默认 1800s（合法长活如 cargo build --release 10-20 分钟要放得过；
    /// 卡死调用的兜底闸而非提速闸）。loop 侧 fresh-read（每次 dispatch 现
    /// 读 config.json），运行时改键下一轮生效。诚实边界：超时放弃的是
    /// future，已 spawn 的子进程可能成为孤儿（B4 后台注册表/停摆清扫兜底）。
    #[serde(default = "default_executor_tool_timeout_secs")]
    pub executor_tool_timeout_secs: u64,
    /// L29 流断拼接开关（`agents.stream_concat_retry`）。默认开（拼接省
    /// token）；显式 false = stream 断流回退「重试从头」。消费方
    /// `config_watch::stream_concat_retry_from_value` 原始 JSON fresh-read
    /// （nemesis-agent）——typed 字段化保证 save_config 往返不丢键
    /// （P32 cache_warmer 同款预防，BUG-20261105-008：不建模则显式
    /// 「重试从头」被任何 typed 保存静默抹掉回翻拼接）。
    #[serde(default = "default_true")]
    pub stream_concat_retry: bool,
    /// B 端职能工具围栏总闸（`agents.profession_tool_fence`）。默认开；
    /// 显式 false = 任务装配不写 `AgentConfig.tools` 白名单（供给/派发
    /// 双闸的权威判据为空 = 不围）。消费方
    /// `config_watch::current_profession_tool_fence` 世代快照原始读
    /// （nemesisbot::cluster_agent）——typed 字段化同款预防
    /// （BUG-20261105-008）。
    #[serde(default = "default_true")]
    pub profession_tool_fence: bool,
    /// L10 工具参数面惰性供给（`agents.lazy_tool_schema`）。**缺省随
    /// tier**（mini 强制惰性 / big·auto 恒全量 / normal 缺省惰性）——必须
    /// `Option<bool>` 而非定值 bool：typed 字段化若写死默认会把「未表态」
    /// 钉成显式值，破坏 tier 切换语义。None（缺席/null）= 交给消费方
    /// `current_lazy_tool_schema` 裁决矩阵；Some(false)（normal 档全量）/
    /// Some(true) 显式表态照常保留（BUG-20261105-008）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lazy_tool_schema: Option<bool>,
}

/// 手写 Default（不 derive）：本结构 8 个直接字段带非零 serde 字段默认
/// （turn_wal / provider_stream_events / image_downscale / lease_enabled /
/// stream_concat_retry / profession_tool_fence = default_true，
/// prompt_system = "pro"，executor_tool_timeout_secs = 1800），derive 只给
/// 零值。分叉点 = 「agents 段整体缺席」的 config 经 Config 字段级
/// `#[serde(default)]` 回落走进 `Default::default()`，与「段在场但键缺席」
/// 的字段级 serde 路径各走各的（BUG-20261107-014：文档/fresh-read 层口径
/// 同说「默认开」，typed 层却拿到关；typed save 往返还会把 false 烤进
/// 文件，翻转 fresh-read 层「缺键=默认开」的语义）。手写与 FsWatcherConfig
/// 同款纪律：缺省值只认 serde default fn 单一真相源。
impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            defaults: AgentDefaults::default(),
            list: Vec::new(),
            claude_code_tool: ClaudeCodeToolConfig::default(),
            codex_tool: CodexToolConfig::default(),
            lsp_tool: LspToolConfig::default(),
            tool_doc_folding: ToolDocFoldingConfig::default(),
            subagent: SubagentConfig::default(),
            fs_watcher: FsWatcherConfig::default(),
            codemode: CodeModeConfig::default(),
            delegate: DelegateToolConfig::default(),
            turn_wal: default_true(),
            provider_stream_events: default_true(),
            hidden_tools: Vec::new(),
            mcp_lazy_supply: false,
            roles: RolesConfig::default(),
            small_model: None,
            guardian_model: None,
            doom_loop_approval: false,
            image_downscale: default_true(),
            lease_enabled: default_true(),
            discipline: DisciplineConfig::default(),
            prompt_system: default_prompt_system(),
            cache_warmer: CacheWarmerConfig::default(),
            executor_tool_timeout_secs: default_executor_tool_timeout_secs(),
            stream_concat_retry: default_true(),
            profession_tool_fence: default_true(),
            lazy_tool_schema: None,
        }
    }
}

/// `agents.executor_tool_timeout_secs` 缺键默认（30 分钟）。
pub(super) fn default_executor_tool_timeout_secs() -> u64 {
    1800
}

/// `agents.discipline` 配置节。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DisciplineConfig {
    /// 总开关，默认 false（D5）。
    #[serde(default)]
    pub enabled: bool,
}

/// P32（cache-warmer）`agents.cache_warmer` 配置节。消费方在
/// `nemesis-agent` loop/cache_warmer.rs（fresh-read，运行中改键热关生效；
/// 启动后才开需重启——诚实不对称见该模块注释）。类型在此单一真相源，
/// agent 侧经 serde 反序列化消费同一份默认值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheWarmerConfig {
    /// 总开关，默认 **false**（D-4 成本敏感 opt-in）——关 = gateway 启动
    /// 不 spawn 任何定时任务（零副作用）。开 = idle 会话在 prompt cache
    /// TTL×90% 时用 1-token 重放刷新缓存（TTL 需模型条目显式声明
    /// `cache_ttl_secs`，未知不 warm）。
    #[serde(default)]
    pub enabled: bool,
    /// 单次 warm 重放的成本上限（USD）。按价目表保守估算（全价 input +
    /// 1 output token，不假设 cache read 折扣）；估算超限或价目表未命中
    /// → 跳过。默认 0.05。
    #[serde(default = "default_cache_warmer_cost_limit_usd")]
    pub cost_limit_usd: f64,
}

impl Default for CacheWarmerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cost_limit_usd: default_cache_warmer_cost_limit_usd(),
        }
    }
}

fn default_cache_warmer_cost_limit_usd() -> f64 {
    0.05
}

/// `agents.roles` 配置节（2026-09-28 角色目录与分档供给）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RolesConfig {
    /// 隐藏的角色 slug 列表（spawn schema 枚举照旧全量、dispatch 拒绝——
    /// F8 双闸；`roles_visible_to` 过滤后的子集再减本表）。
    #[serde(default)]
    pub hidden: Vec<String>,
}

/// I1 (devtool-upgrade 阶段 3): `agents.fs_watcher` config section —
/// workspace file watching (notify-based, recursive). External edits
/// surface to the agent as a one-shot `<external_changes>` annotation in
/// the next context snapshot; instruction-chain files (AGENTS.md/CLAUDE.md)
/// additionally invalidate context digests (structural no-op since round-5
/// stateless digests — kept as the anchor, same as the dispatch path's
/// touch-driven call). Watcher startup failure degrades to warn-once and
/// never blocks the loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWatcherConfig {
    /// Master switch (default true). Failure to start is warn-once +
    /// disabled either way — this only controls whether we try.
    #[serde(default = "default_fs_watcher_enabled")]
    pub enabled: bool,
    /// Extra ignore entries merged over the built-in table (built-in:
    /// `.git`/`node_modules`/`target`/`dist`/`logs`/`uploads`/`memory`/
    /// `cluster`/`board`/`data`/`config`/`tools` dir names + `*.jsonl`/
    /// `*.db*`/`*.sqlite*` extensions). A bare name matches any path
    /// component (workspace-relative); `*.ext` matches by extension.
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// goal P2/D0：派发准入默认上限（单 worker 在途 1 单；0=不限）。
pub(super) fn default_worker_max_inflight() -> i64 {
    1
}

fn default_fs_watcher_enabled() -> bool {
    true
}

// 手写 Default（不 derive）：`enabled` 的本意默认是 true，derive 给零值
// false（同 DiagnosticsLoopConfig 的坑）。
impl Default for FsWatcherConfig {
    fn default() -> Self {
        Self {
            enabled: default_fs_watcher_enabled(),
            ignore: Vec::new(),
        }
    }
}

/// G2 (devtool-upgrade 阶段 3): `agents.subagent` config section — spawn
/// nesting governance. The enforcement lives in `SpawnTool` (nemesis-agent):
/// a spawn request is rejected when `parent_depth + 1 > max_depth`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    /// Maximum sub-agent nesting depth. Depth 0 = the top-level agent;
    /// a spawn from depth N creates a child at depth N+1, allowed only when
    /// N+1 <= max_depth. Default 1 = sub-agents cannot spawn sub-agents
    /// (grandchild requests bounce back to the model with a limit message).
    #[serde(default = "default_subagent_max_depth")]
    pub max_depth: usize,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_depth: default_subagent_max_depth(),
        }
    }
}

fn default_subagent_max_depth() -> usize {
    1
}

/// T2（内部专项总计划 L1 codemode，2026-10-13）: `agents.codemode` config
/// section — `code_run` 脚本工具（QuickJS 沙箱）的运行开关与资源限制。
/// 编译闸是 `codemode` cargo feature；这里管运行态：`enabled` 默认**开**
/// （2026-10-25 拍板转默认开 + Dashboard Tools 页手动关）。开关 enabled
/// 经 check_config_reload 热生效（下一轮 LLM 前比对注册状态增删，无需
/// 重启）；timeout/memory 是构造期值，改动需 loop 重建生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeModeConfig {
    /// Enable the `code_run` tool registration (default true since
    /// 2026-10-25; explicit `false` in config keeps it off).
    #[serde(default = "default_codemode_enabled")]
    pub enabled: bool,
    /// Per-call wall-clock budget in seconds; the QuickJS interrupt handler
    /// checks it (default 300, aligned with the approval-card lifetime —
    /// script tool() calls that trigger an approval wait consume budget,
    /// so the budget must not expire before the card does). Infinite JS
    /// loops are cut at the deadline and the error is returned to the model
    /// as tool result text.
    #[serde(default = "default_codemode_timeout_secs")]
    pub timeout_secs: u64,
    /// QuickJS memory cap in MB via `set_memory_limit` (default 64). A
    /// larger allocation fails as a JS OOM error inside the sandbox.
    #[serde(default = "default_codemode_memory_limit_mb")]
    pub memory_limit_mb: u64,
}

fn default_codemode_enabled() -> bool {
    true
}

fn default_codemode_timeout_secs() -> u64 {
    300
}

fn default_codemode_memory_limit_mb() -> u64 {
    64
}

impl Default for CodeModeConfig {
    fn default() -> Self {
        Self {
            enabled: default_codemode_enabled(),
            timeout_secs: default_codemode_timeout_secs(),
            memory_limit_mb: default_codemode_memory_limit_mb(),
        }
    }
}

/// A3（委派持续操控，2026-10-18）: `agents.delegate` config section —
/// delegate 四工具（delegate_start/send/output/stop）的运行开关与子代理
/// 生命上限。注册发生在 AgentLoop 装配期且要求 CLI 在场（claude/codex 至
/// 少一个在 PATH），改键需 loop 重建生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegateToolConfig {
    /// Enable the delegate_* tool registration (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Delegate sub-agent lifetime cap in seconds (default 3600). 0 = no
    /// limit; the watchdog tree-kills at the deadline and leaves an honest
    /// marker in the delegate's output stream.
    #[serde(default = "default_delegate_max_lifetime_secs")]
    pub max_lifetime_secs: u64,
    /// claude permission-mode passthrough (default "" = CLI default). Never
    /// implies bypassing the child CLI's own permission surface.
    #[serde(default)]
    pub permission_mode: String,
}

fn default_delegate_max_lifetime_secs() -> u64 {
    3600
}

impl Default for DelegateToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_lifetime_secs: default_delegate_max_lifetime_secs(),
            permission_mode: String::new(),
        }
    }
}

/// Y1 (Phase4-a): `agents.tool_doc_folding` config section. When enabled,
/// the `expand_top_n` tools most similar to the current conversation (cosine
/// between the latest user message and each tool description, via the P3.1
/// embed backend) keep their full description; the rest collapse to a
/// one-line summary. Orthogonal to tier supply — folding rewrites
/// description TEXT only, every tool stays callable with its full parameter
/// schema. The Mini tier never participates (13-tool core set, nothing to
/// save). Requires a wired memory manager (embed backend); without one the
/// loop degrades to unfolded defs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDocFoldingConfig {
    /// Master switch (default false).
    #[serde(default)]
    pub enabled: bool,
    /// How many tools stay fully expanded, ranked by similarity
    /// (default 8; ties break alphabetically for determinism).
    #[serde(default = "default_tool_doc_folding_top_n")]
    pub expand_top_n: usize,
}

fn default_tool_doc_folding_top_n() -> usize {
    8
}

impl Default for ToolDocFoldingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            expand_top_n: default_tool_doc_folding_top_n(),
        }
    }
}

/// L1 (U19): `agents.lsp_tool` config section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LspToolConfig {
    /// Enable the `lsp` semantic-code tool (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Per-request LSP timeout in seconds (default 120 — first queries wait
    /// behind server indexing on big repos).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Idle session reap threshold in seconds (default 600). Sessions idle
    /// longer than this get shut down lazily on the next query.
    #[serde(default)]
    pub idle_secs: Option<u64>,
    /// C6 (devtool-upgrade 阶段 6): 静默自举缺失的语言服务器——true 时网关
    /// 启动期后台串行安装全部缺失的非交互语言服务器（官方安装通道白名单
    /// 目录），装完需重启 Agent 重新探测注册（default false——默认绝不自动装东西）。
    #[serde(default)]
    pub auto_install: bool,
}

/// C4 (2026-09-04 devtool-upgrade 阶段 1): `agents.diagnostics_loop` config
/// section. 编辑→诊断回灌闭环（阶段 2 C1-C3 消费）的独立开关。**与
/// `agents.lsp_tool.enabled` 解耦**：诊断闭环开而 lsp 工具关是合法组合
/// （闭环自身按需起服务器）。
///
/// P2（2026-09-25 能力扩展 WS3）：`enabled` 默认翻转为 **true**（对齐
/// 业界通行的「内置无条件开启」）——缺键/缺段按开处理；用户显式写
/// `false` 的配置不受影响（serde 显式值优先于 default fn）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsLoopConfig {
    /// Enable the edit→diagnostics feedback loop (default true since P2).
    #[serde(default = "default_diagnostics_enabled")]
    pub enabled: bool,
    /// Max diagnostics fed back per edit round (default 20; P3 起为跨文件
    /// 聚合后的总量上限).
    #[serde(default = "default_diagnostics_max_errors")]
    pub max_errors: usize,
    /// Max wall-clock wait for diagnostics after an edit, in ms (default 2000).
    #[serde(default = "default_diagnostics_wait_max_ms")]
    pub wait_max_ms: u64,
}

// 手写 Default（不 derive）：字段级 serde default fn 在 **整个
// `diagnostics_loop` 键缺席**时用的是类型的 Default impl（derive 会给零值
// 20/2000 的本意就落空了——首测抓出）。手写保证「全键缺席」与「部分键
// 缺席」（内部字段的 serde default fn 管）两条路都落在同一组默认值上。
impl Default for DiagnosticsLoopConfig {
    fn default() -> Self {
        Self {
            enabled: default_diagnostics_enabled(),
            max_errors: default_diagnostics_max_errors(),
            wait_max_ms: default_diagnostics_wait_max_ms(),
        }
    }
}

/// P2（能力扩展 WS3）：诊断回灌默认开（同源供 serde 缺键路径与手写
/// Default impl 消费——两路必须落同一值）。
fn default_diagnostics_enabled() -> bool {
    true
}

fn default_diagnostics_max_errors() -> usize {
    20
}

fn default_diagnostics_wait_max_ms() -> u64 {
    2000
}

/// I4: `agents.codex_tool` config section (mirrors claude_code_tool).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexToolConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// T5 (U13): fixed sandbox tier — `read_only` | `workspace_write` |
    /// `danger_full_access` (default `read_only`). Config-side snake_case,
    /// mapped to codex's kebab-case CLI value (`--sandbox read-only` etc.) at
    /// spawn. Empty/unknown → default (fail-safe). NOT model-selectable.
    #[serde(default)]
    pub sandbox: String,
}

/// H7 (U13 half): `agents.claude_code_tool` config section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClaudeCodeToolConfig {
    /// Enable the claude_code delegation tool (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Per-delegation wall-clock timeout in seconds (default 300).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// T5 (U13): fixed permission tier — `default` | `accept_edits` | `plan`
    /// | `bypass_permissions` (default `accept_edits`, the
    /// non-interactive-safe tier) → `--permission-mode <tier>` at spawn.
    /// Empty/unknown → default (fail-safe). NOT model-selectable.
    #[serde(default)]
    pub permission_mode: String,
}

/// A6 (2026-09-06 devtool-upgrade 阶段 5): `agents.defaults.format_on_save`
/// config section — write/edit 成功后自动跑外部格式化工具（format-on-save）。
/// 执行点在 PostToolUse hooks 之前（用户自定义 hook 看到格式化后文件）、
/// C3 诊断带之前（诊断看到格式化后文件）；`executor.enabled=true` 时诚实
/// 停用（写落子进程/盒内，gateway 侧后格式化会读到陈旧内容甚至绕盒写）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatOnSaveConfig {
    /// Enable format-on-save (default false — opt-in).
    #[serde(default)]
    pub enabled: bool,
    /// Per-extension formatter override: ext → argv template（首元素 = 工具
    /// 名，其余 = 参数；`{file}` 占位符替换为目标路径）。按条目覆盖——命中的
    /// ext 用配置 argv，未命中的 ext 回落内置表。空表 = 全内置。
    #[serde(default)]
    pub formatters: std::collections::BTreeMap<String, Vec<String>>,
    /// 单次格式化墙钟预算（秒；超时 kill 子进程，静默放行）。默认 3——冷环境
    /// （CI / 慢盘 / rustup shim 链冷启）可能超 3s，可调大。0 视作缺省。
    #[serde(default = "default_format_timeout_secs")]
    pub timeout_secs: u64,
}

/// 格式化超时缺省秒数（单一真相源：serde 缺省 / Default / 0 防呆共用）。
pub fn default_format_timeout_secs() -> u64 {
    3
}

impl Default for FormatOnSaveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            formatters: std::collections::BTreeMap::new(),
            timeout_secs: default_format_timeout_secs(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    #[serde(default)]
    pub workspace: String,
    #[serde(default = "default_true")]
    pub restrict_to_workspace: bool,
    #[serde(default)]
    pub llm: String,
    #[serde(default)]
    pub image_model: String,
    #[serde(default)]
    pub image_model_fallbacks: Vec<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i64,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: i64,
    #[serde(default = "default_concurrent_request_mode")]
    pub concurrent_request_mode: String,
    /// Full-review M4: context-snapshot message role. "user" (default)
    /// places the merged snapshot as a user message; some
    /// strict chat templates reject adjacent user/user pairs — set
    /// "system" to restore the pre-I2 single system-role injection.
    #[serde(default = "default_snapshot_role")]
    pub snapshot_role: String,
    #[serde(default = "default_queue_size")]
    pub queue_size: i64,
    #[serde(default)]
    pub max_continuation_permits: i64,
    /// D-4（2026-09-23 多会话并行清账）：单 loop 并发 turn 上限（安全阀）。
    /// 泵统一 gate+spawn 后跨会话天然并发，此值防止极端场景无界并发。
    /// 0 = 不设限。默认 8（每个 AgentLoop——主 loop / 项目 loop 各自独立
    /// 计数；同会话仍串行，不受此值影响）。
    #[serde(default = "default_max_concurrent_turns")]
    pub max_concurrent_turns: i64,
    /// U4: tool-result spill retention (days). Spill files under
    /// `<home>/logs/spill/` older than this are deleted (startup scan +
    /// daily midnight task in agent_factory). 0 = never clean up. Default 7.
    #[serde(default = "default_spill_retention_days")]
    pub spill_retention_days: i64,
    /// C4: edit→diagnostics feedback loop switch (see [`DiagnosticsLoopConfig`]).
    #[serde(default)]
    pub diagnostics_loop: DiagnosticsLoopConfig,
    /// A6: format-on-save after write/edit (see [`FormatOnSaveConfig`]).
    #[serde(default)]
    pub format_on_save: FormatOnSaveConfig,
    /// 429 重试环（2026-09-17 BUG 文档裁决④）：上游限流（HTTP 429）重试次数，
    /// 默认 10。间隔走 10 档阶梯（5s→…→210s），上游带 Retry-After 时取
    /// max(上游要求, 阶梯值)。0 = 关闭重试（一次失败即终局，旧行为）。
    #[serde(default = "default_rate_limit_retries")]
    pub rate_limit_retries: i64,
    /// P5（能力扩展 WS2 compaction）：自动压缩的逐字尾巴 token 预算
    /// （默认 20000，对齐业界通行 keepRecentTokens）。压缩边界从「保留近
    /// K_TARGET=6 条」改为「保留近 N token」（MODEL-FACING 投影估算），
    /// 预算边界再经 tool_safe_boundary 回退，保证永不切在 tool_call/result
    /// 对中间。**0 = 回退旧按条数行为**。loop 侧 fresh-read（每轮现读
    /// config.json），运行中改键下一轮生效。
    #[serde(default = "default_compact_keep_recent_tokens")]
    pub compact_keep_recent_tokens: i64,
    /// P8（2026-10-10 集群无人值守硬化 A2）：会话 token 预算——单会话累计
    /// 用量（input+cache_read+output，与 aggregate_session_usage 账本同
    /// 口径）上限。**0 = 关（默认）**，注意与 `max_tool_iterations` 的
    /// 0=不限惯例相反。消费点 = loop 侧轮顶检（硬停 ≥100% / 软提示 ≥75%）、
    /// cluster_agent 重派入口拒接、board_review 转人工；读法统一
    /// `agents_defaults_u64("session_token_budget", 0)` fresh-read
    /// （config.json 单一真相源，运行中改键下一轮生效）。typed 字段的
    /// 意义 = 防 typed save_config round-trip 静默抹键（vision 键
    /// 2026-09-03 同类回归；AgentDefaults 无 flatten extra）。
    #[serde(default)]
    pub session_token_budget: u64,
    /// W6（2026-10-25 加固）：优雅
    /// 停机 drain 预算（秒）。正常停机序列：关入口（新消息诚实拒收）→
    /// 有界等在途 turn 归零 → 再 teardown；超时明示在途数后强制收尾。
    /// **0 = 跳过 drain**（立即 teardown）。默认 30。estop 语义不变
    /// （drain 只走正常停机路径）。
    #[serde(default = "default_shutdown_drain_secs")]
    pub shutdown_drain_secs: u64,
    /// 输出撞顶翻倍阶梯的封顶值（§6.2 无人值守硬化；默认 262144 = 256K）。
    /// LLM 响应 `finish_reason=length` 且正文为空（思考烧穿形态）时翻倍重发
    ///（`max(当前×2, 32768)` 起步地板），直到有正文或触及本封顶。触及封顶
    /// 仍空 = 按现行失败收口（文案注明已试到封顶）。只改当次请求，不回写
    /// config（会话级 escalation，防单次事故永久抬高全局基线）。
    #[serde(default = "default_max_output_ceiling")]
    pub max_output_ceiling: u64,
}

/// 翻倍阶梯封顶缺省（serde 缺省 / Default 单一真相源）。
pub fn default_max_output_ceiling() -> u64 {
    262144
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: String::new(),
            restrict_to_workspace: true,
            llm: String::new(),
            image_model: String::new(),
            image_model_fallbacks: vec![],
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            max_tool_iterations: default_max_tool_iterations(),
            concurrent_request_mode: default_concurrent_request_mode(),
            snapshot_role: default_snapshot_role(),
            queue_size: default_queue_size(),
            max_continuation_permits: 0,
            max_concurrent_turns: default_max_concurrent_turns(),
            spill_retention_days: default_spill_retention_days(),
            diagnostics_loop: DiagnosticsLoopConfig::default(),
            format_on_save: FormatOnSaveConfig::default(),
            rate_limit_retries: default_rate_limit_retries(),
            compact_keep_recent_tokens: default_compact_keep_recent_tokens(),
            session_token_budget: 0,
            shutdown_drain_secs: default_shutdown_drain_secs(),
            max_output_ceiling: default_max_output_ceiling(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfigEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub model: Option<AgentModelConfig>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub subagents: Option<SubagentsConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentModelConfig {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// Support both string and structured model config during deserialization.
impl<'de> serde::Deserialize<'de> for AgentModelConfig {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::{self, Visitor};
        use std::fmt;

        pub struct AgentModelConfigVisitor;

        impl<'de> Visitor<'de> for AgentModelConfigVisitor {
            type Value = AgentModelConfig;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a string or an object with primary and fallbacks")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(AgentModelConfig {
                    primary: v.to_string(),
                    fallbacks: vec![],
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                #[derive(Deserialize)]
                struct Raw {
                    #[serde(default)]
                    primary: String,
                    #[serde(default)]
                    fallbacks: Vec<String>,
                }
                let raw = Raw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(AgentModelConfig {
                    primary: raw.primary,
                    fallbacks: raw.fallbacks,
                })
            }
        }

        deserializer.deserialize_any(AgentModelConfigVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentsConfig {
    #[serde(default)]
    pub allow_agents: Vec<String>,
    #[serde(default)]
    pub model: Option<AgentModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBinding {
    #[serde(default)]
    pub agent_id: String,
    pub r#match: BindingMatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingMatch {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub peer: Option<PeerMatch>,
    #[serde(default)]
    pub guild_id: String,
    #[serde(default)]
    pub team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMatch {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
}
