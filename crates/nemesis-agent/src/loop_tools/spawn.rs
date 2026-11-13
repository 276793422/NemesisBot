// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Configuration for the spawn tool.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Default model for spawned agents.
    pub default_model: String,
    /// Maximum number of concurrent sub-agents.
    pub max_concurrent: usize,
    /// (devtool-upgrade)：子代理最大嵌套深度。父深度 N（0 = 顶层
    /// agent）发起 spawn 产生深度 N+1 的子代理，仅当 N+1 ≤ max_depth 放行；
    /// 超限回灌 `Sub-agent depth limit (N) reached` 供模型自纠。生产接线 =
    /// config `agents.subagent.max_depth`（默认 1 = 子代理不能再 spawn）。
    pub max_depth: usize,
}

/// readonly 子代理工具白名单（spawn `tools` 档位的受限集，与 explore
/// 档共用同一清单）。语义 = 与注册表求交（`effective_tool_defs` retain）——
/// 未注册/未启用的项（如 lsp_tool 关闭时）自然缺席，不报错。
pub const DETACHED_READONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "git",
    "web_fetch",
    "lsp",
    "cli_reference",
];

/// spawn `tools` 档位 → `DetachedOpts.allowed_tools`。
/// `"readonly"` → 白名单（`DETACHED_READONLY_TOOLS`）；`"full"` → `None`
/// （不设限，仍经父 tier 过滤）。未知档位 = `Err`（调用方诚实回灌模型自纠，
/// 不静默降级）。
pub fn detached_tools_for_profile(
    profile: &str,
) -> Result<Option<&'static [&'static str]>, String> {
    match profile {
        "readonly" => Ok(Some(DETACHED_READONLY_TOOLS)),
        "full" => Ok(None),
        other => Err(format!(
            "Unknown tools profile '{other}'. Valid values: \"readonly\" (default) or \"full\"."
        )),
    }
}

/// prompt-pack pro：spawn `tools` 档位 → 子代理角色。readonly 档
/// （只读白名单）的子任务天然是调查/汇报型，映射侦察员角色模板；full 档
/// 不指定（None = 继承主人格，行为不变）。与 [`detached_tools_for_profile`]
/// 同为闭包侧唯一映射点。
pub fn detached_role_for_profile(profile: &str) -> Option<crate::prompt::SubagentRole> {
    match profile {
        "readonly" => Some(crate::prompt::SubagentRole::Explorer),
        _ => None,
    }
}

/// Spawn 闭包类型：`(agent_id, task, model, channel, chat_id, tools, depth, background, role) -> Future<Result<String, String>>`。
/// (devtool-upgrade)：生产接线 = agent_factory 组装 AgentLoop 后向
/// spawn_slot 注入的闭包（持 `Weak<AgentLoop>` 调 `run_detached`）。
/// 第 6 参 `tools` = 工具档位（"readonly"|"full"，SpawnTool 侧已校验，
/// 闭包内经 `detached_tools_for_profile` 映射为白名单）。
/// 第 7 参 `depth` = 本 spawn 应产生的子代理深度（父深度 + 1，已过
/// max_depth 检查），闭包经 `DetachedOpts.depth` 写到子 instance。
/// 第 8 参 `background` = 后台化（true 时闭包侧 `tokio::spawn` 包住
/// run_detached，立即返回 `__BG_SPAWN__:{task_id}` marker；任务完成经
/// `subagent_continuation:{task_id}` bus 消息回灌续行）。深度限制沿用第
/// 7 参（后台任务不另计深度——派生前已过 max_depth 检查）。
/// （角色显式化）：第 9 参 `role` = 子代理角色 slug（`""` = 自动：沿用
/// 工具档位推导的既有行为；非空 = 已过 `SubagentRole::from_slug` 校验的
/// 显式角色，闭包侧覆盖 `DetachedOpts.role`）。
/// 子代理可见性：第 10 参 `parent_session_key` = 发起会话键（注册表
/// 台账归属 + 后台续行回灌路由的信号；客户端手动 spawn 传 `""`）。
/// （专家子代理）：第 11 参 `expert` = 专家 slug/显示名（`""` = 默认
/// 子代理，现行行为逐字节不变）。非空 = 已过 SpawnTool 侧 resolver 预检
/// （存在 + 启用 + 模式开关），闭包内再 resolve 一次取提示词/模型覆盖
/// （磁盘真相两次读，专家目录个位数成本可忽略）：`DetachedOpts.system_prompt`
/// 整段替换主人格、`role` 置 None、model 非空时 `model_swap`。
/// （无匹配自动降级，2026-11-13）：第 12 参 `prompt` = 自拟专家系统提示词
/// （`""` = 缺省）。非空且 `expert` 空 = ad-hoc 派发——默认子代理以该提示词
/// 整体替换主人格（`agent_id=expert:ad-hoc`），降级二段式的第二跳（第一跳
/// 点名吃降级指引后模型自拟提示词重调）。与 `expert` 互斥（SpawnTool 侧
/// 校验），模式开关在 SpawnTool 侧闸（闭包不再验策略——单一裁决点）。
pub type SpawnFn = Arc<
    dyn Fn(
            &str,
            &str,
            &str,
            &str,
            &str,
            &str,
            usize,
            bool,
            &str,
            &str,
            &str,
            &str,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

// ===========================================================================
// 专家子代理（experts）——契约类型
// ===========================================================================
// crate 边界：这里只定义契约（名册/解析产物/闭包类型）；IO 与解析全在
// bot 侧 experts_store，装配点把同一个 ExpertsHandle 注入 SpawnTool
// （list/预检）与 spawn 闭包（run 执行），两条路径同源不漂移。

/// 专家名册条目（`spawn action=list` 形态）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExpertRosterEntry {
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub enabled: bool,
}

/// 专家名册整体产物：条目 + 默认子代理显示名（名册尾部 default 行用）。
#[derive(Debug, Clone)]
pub struct ExpertsRoster {
    pub experts: Vec<ExpertRosterEntry>,
    pub default_name: String,
}

/// 专家派发产物（run 形态）：`prompt` 整段替换主人格（Swarm 裸提示词
/// 通道）；`model` 非空 = 模型别名覆盖（空 = 继承主 Agent 模型）。
#[derive(Debug, Clone)]
pub struct ExpertDispatch {
    pub slug: String,
    pub name: String,
    pub prompt: String,
    pub model: String,
}

pub type ExpertsRosterFn = Arc<dyn Fn() -> ExpertsRoster + Send + Sync>;

/// 专家解析器：key = slug 或显示名（不区分大小写回落匹配）。Err =
/// 诚实错误（文案自带在册名册/停用说明，供模型自纠；由实现方组装）。
pub type ExpertResolveFn = Arc<dyn Fn(&str) -> Result<ExpertDispatch, String> + Send + Sync>;

/// 专家模式策略（2026-11-13 总开关 + 自动降级）。config `agents.experts`
/// 段的契约镜像——实现方（bot 侧）每次调用现读 config.json 组装（
/// fresh-read，运行中改键即时生效）；默认值与
/// `nemesis_config::ExpertsModeConfig` 的 serde 缺省一致，由单测钉住。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpertsPolicy {
    /// 专家模式总开关（默认 false，opt-in）。false = 点名 `expert`/`prompt`
    /// 诚实拒绝（文案带开启指引）、`action=list` 照列 + 停用注记。
    pub enabled: bool,
    /// 无匹配专家自动降级（默认 true）。true = 点名不存在/停用回灌降级
    /// 指引（模型自拟提示词带 `prompt` 重调，勿停车问用户）。
    pub degrade: bool,
    /// 降级时通知用户（默认 false = 静默）。
    pub notify: bool,
}

impl Default for ExpertsPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            degrade: true,
            notify: false,
        }
    }
}

/// 策略读取器（实现方注入；每次调用现读——fresh-read 惯例）。
pub type ExpertsPolicyFn = Arc<dyn Fn() -> ExpertsPolicy + Send + Sync>;

/// spawn 工具与 spawn 闭包共用的专家解析面（装配点单一注入面）。None
/// （headless / 项目 loop / 测试未装配）= `action=list` 诚实空名册、
/// `expert` 点名诚实报错——不假装有专家，也不假装没有 spawn。
#[derive(Clone)]
pub struct ExpertsHandle {
    pub roster: ExpertsRosterFn,
    pub resolve: ExpertResolveFn,
    /// 模式策略读取器（2026-11-13 总开关 + 降级；每次调用现读）。
    pub policy: ExpertsPolicyFn,
}

/// Spawn tool for creating sub-agents.
///
/// Spawns a new sub-agent to handle a specific task independently.
/// When a spawn function is present in `spawn_slot`, it performs a real
/// spawn; otherwise it returns an error indicating sub-agent support is not
/// configured.
///
/// (devtool-upgrade)：spawn 闭包经 `Arc<OnceLock>` **槽**注入而
/// 非直接 setter——`Tool` trait 对象无法 downcast，注册进 tools map 后拿
/// 不到自身；槽让 agent_factory 在 AgentLoop 组装完成后（闭包需要 loop 的
/// Arc）延迟注入，工具即刻生效。`max_concurrent` 由真信号量兜住（超出
/// 排队等待，不再是无装饰的配置数字）。
pub struct SpawnTool {
    config: SpawnConfig,
    /// Allowlist checker: returns true if the parent can spawn the target.
    allowlist_checker: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
    /// Stored channel from set_context.
    stored_channel: Arc<std::sync::Mutex<String>>,
    /// Stored chat_id from set_context.
    stored_chat_id: Arc<std::sync::Mutex<String>>,
    /// 并发上限信号量（permits = `config.max_concurrent`；execute 时
    /// acquire，超并发排队等待而非放行）。
    semaphore: Arc<tokio::sync::Semaphore>,
    /// 本次调用的父深度（`set_invocation_depth` 注入，0 = 顶层 agent）。
    /// execute 读它执行 `parent + 1 > config.max_depth` 拒绝，并把子深度
    /// （parent + 1）经第 7 参传给 spawn 闭包。共享实例上 set-then-read 的
    /// 竞态只会在并发派发间边际误归属深度（与既有 set_context 竞态同类，
    /// 已接受）。
    invocation_depth: std::sync::atomic::AtomicUsize,
    /// Spawn 闭包槽（agent_factory 组装后 `set`；`None` 时 execute 诚实
    /// 报 not available——旧装配路径行为不回归）。
    spawn_slot: Arc<std::sync::OnceLock<SpawnFn>>,
    /// 专家解析面（装配点注入；None = list 诚实空名册 / expert 点名诚实
    /// 报错——见 [`ExpertsHandle`] 文档）。
    experts: Option<ExpertsHandle>,
    /// 降级通知事件槽（2026-11-13）：`agents.experts.notify=true` 时降级
    /// 发生即发布 [`nemesis_types::agent::AgentEvent::ExpertDegraded`]（web
    /// pump 转 SSE 全局广播 → 前端 toast）。None（headless/测试未注入）=
    /// 无通知面。
    event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
}

impl SpawnTool {
    /// Create a new spawn tool with the given configuration.
    pub fn new(config: SpawnConfig) -> Self {
        let permits = config.max_concurrent.max(1);
        Self::with_spawn_slot(
            config,
            Arc::new(std::sync::OnceLock::new()),
            Arc::new(tokio::sync::Semaphore::new(permits)),
        )
    }

    /// 共享槽构造器——生产接线用。调用方持有同一 slot Arc，在
    /// AgentLoop 组装完成后向其注入闭包。
    pub fn with_spawn_slot(
        config: SpawnConfig,
        spawn_slot: Arc<std::sync::OnceLock<SpawnFn>>,
        semaphore: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            config,
            allowlist_checker: None,
            stored_channel: Arc::new(std::sync::Mutex::new(String::new())),
            stored_chat_id: Arc::new(std::sync::Mutex::new(String::new())),
            semaphore,
            invocation_depth: std::sync::atomic::AtomicUsize::new(0),
            spawn_slot,
            experts: None,
            event_tx: None,
        }
    }

    /// 注入专家解析面（装配点 builder；None 缺省 = 无专家子系统）。
    pub fn with_experts(mut self, experts: ExpertsHandle) -> Self {
        self.experts = Some(experts);
        self
    }

    /// 注入降级通知事件槽（装配点 builder；None 缺省 = 无通知面）。
    pub fn with_event_tx(
        mut self,
        tx: tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>,
    ) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Set the allowlist checker function.
    pub fn set_allowlist_checker(&mut self, checker: Box<dyn Fn(&str) -> bool + Send + Sync>) {
        self.allowlist_checker = Some(checker);
    }

    /// Set the spawn function for performing actual sub-agent creation.
    ///
    /// The function signature is: `(agent_id, task, model, channel, chat_id) -> Future<Output = Result<String, String>>`
    ///
    /// 内部写入共享槽（`OnceLock::set` 首次生效，重复 set 静默忽略——
    /// 与旧「直接替换」语义的差异仅在同一工具实例重复 set 的场景，生产/
    /// 测试均不存在该用法）。
    pub fn set_spawn_fn(&mut self, f: SpawnFn) {
        let _ = self.spawn_slot.set(f);
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn description(&self) -> String {
        // 专家锚句是**静态**文本（专家名册永不进 schema/system prompt——
        // 动态数据只在 action=list 工具结果里出现，专家增删改对 prompt
        // cache 零冲击）。
        "Spawn a sub-agent to handle a task independently. The sub-agent runs with the same tools and governance as the main agent and returns its final answer. You may first call with {\"action\": \"list\"} to enumerate hired experts (slug/name/summary/tags), then delegate by {\"expert\": \"<slug>\"} — the expert's own system prompt replaces the default persona for that task.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // 角色 enum 从目录生成（统一定义：新增角色只改 catalog，schema
        // 自动跟上）。"" 不进 enum——缺省 = 键缺省，模型显式传 "" 反而是
        // 混淆面。枚举保持全量目录（稳定契约）；tier/hidden 的供给收窄
        // 由 dispatch 闸权威裁决（同款双闸模型，陈旧 cache 窗口兜底）。
        // 枚举描述同步由目录生成（slug=职责），消灭第二份手写文案。
        let role_slugs: Vec<&str> = crate::prompt::SubagentRole::catalog()
            .iter()
            .map(|(slug, _, _)| *slug)
            .collect();
        let role_docs: String = crate::prompt::SubagentRole::catalog()
            .iter()
            .map(|(slug, desc, _)| format!("{slug}={desc}"))
            .collect::<Vec<_>>()
            .join(", ");
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Task description for the sub-agent"},
                "action": {
                    "type": "string",
                    "enum": ["run", "list"],
                    "description": "Action to perform (default \"run\"). \"run\" spawns the sub-agent for `task`; \"list\" returns the hired-expert roster (slug/name/summary/tags + default line) and ignores `task` — call it first when unsure which expert fits."
                },
                "expert": {
                    "type": "string",
                    "description": "Delegate this task to a hired expert (slug or display name; see action=list). The expert's own system prompt replaces the default persona and takes precedence over `role`; an optional per-expert model override applies. Omit for the default sub-agent."
                },
                "prompt": {
                    "type": "string",
                    "description": "Ad-hoc system prompt you write yourself for the sub-agent (mutually exclusive with `expert`). Primary use: auto-degrade fallback — when a requested expert is missing or disabled and the degrade policy is active, re-call with the SAME task plus this prompt carrying the specialist instructions that expert would have had; do not stop to ask the user. The sub-agent runs with this prompt replacing the default persona."
                },
                "context": {"type": "string", "description": "Additional context"},
                "agent_id": {"type": "string", "description": "Optional sub-agent preset name (v1 informational only; defaults to \"default\")"},
                "tools": {
                    "type": "string",
                    "enum": ["readonly", "full"],
                    "description": "Tool profile for the sub-agent. \"readonly\" (default) limits it to read-only tools (read_file/list_dir/grep/git/web_fetch/lsp/cli_reference); \"full\" inherits the parent's tier-filtered tool set. Use readonly for research/exploration tasks, full only when the sub-agent must write."
                },
                "role": {
                    "type": "string",
                    "enum": role_slugs,
                    "description": format!("Specialist role template for the sub-agent (optional). Each role overlays a discipline prompt on the sub-agent's persona: {role_docs}. Omit to auto-derive from the tools profile (readonly→explorer). Roles visible to you depend on the active model tier and agents.roles.hidden config; out-of-scope roles are rejected at dispatch with the visible list.")
                },
                "background": {
                    "type": "boolean",
                    "description": "Run the sub-agent in the background (default false). When true, this tool returns immediately with a task marker and the main conversation continues; the sub-agent's result is automatically delivered back to this session when it completes. Use for long-running tasks (builds, large refactors, research) where waiting would stall the conversation."
                },
                "inherit_context": {
                    "type": "boolean",
                    "description": "Prepend a recent-transcript digest of THIS session to the task (default false). Use with role=fork for one-directive delegation that inherits conversational context — the digest is injected as reference data, not as instructions. Leave false for self-contained tasks."
                }
            },
            "required": ["task"]
        })
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        if let Ok(mut guard) = self.stored_channel.lock() {
            *guard = channel.to_string();
        }
        if let Ok(mut guard) = self.stored_chat_id.lock() {
            *guard = chat_id.to_string();
        }
    }

    /// 分发层注入的父深度（`handle_tool_call_at_depth` → trait 默认
    /// no-op，这里覆写为存储）。execute 据此执行 max_depth 限制。
    fn set_invocation_depth(&self, depth: usize) {
        self.invocation_depth
            .store(depth, std::sync::atomic::Ordering::Relaxed);
    }

    /// spawn 加入 并行池——并发安全由构造保证：池 4 许可 +
    /// 信号量双层限流、深度经 at_depth 注入（上面覆写消费）、每笔分发
    /// 仍过完整瀑布、兄弟子代理实例/会话全隔离。spawn 有副作用（真的
    /// 起子代理）故 `is_read_only` 保持默认 false。
    fn is_parallel_safe(&self) -> bool {
        true
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        // agent_id 改 optional（schema/实现一致化——schema 一直只
        // required task）；语义 = 子代理 preset 名，v1 信息性，缺省 default。
        let agent_id = val
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let task = val.get("task").and_then(|v| v.as_str()).unwrap_or("");

        // 工具档位（缺省 readonly=最小权限）。未知档位在 spawn 前诚实
        // 拒绝（不占信号量、不起子代理），错误文案列合法值供模型自纠。
        let tools_profile = val
            .get("tools")
            .and_then(|v| v.as_str())
            .unwrap_or("readonly");
        detached_tools_for_profile(tools_profile)?;

        // 后台化开关（缺省 false=同步等待子代理完成）。true 时闭包
        // 侧转后台，本调用立即返回 `__BG_SPAWN__` marker（loop 侧存续行
        // 快照 + 中间消息收尾回合；完成结果经 subagent_continuation 回灌）。
        let background = val
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 显式角色（缺省 "" = 自动，沿用工具档位推导）。未知 slug 在
        // spawn 前诚实拒绝，错误文案带合法值清单供模型自纠。（tier/hidden
        // 的可见性由 dispatch 闸裁决——SpawnTool 无 tier 视角，管 slug
        // 合法性这一层。）
        let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if !role.is_empty() && crate::prompt::SubagentRole::from_slug(role).is_none() {
            let valid: Vec<&str> = crate::prompt::SubagentRole::catalog()
                .iter()
                .map(|(s, _, _)| *s)
                .collect();
            return Err(format!(
                "Unknown sub-agent role '{}'. Valid roles: {} (or omit to auto-derive from tools profile).",
                role,
                valid.join(", ")
            ));
        }

        // 专家子代理（experts）：action 分叉 + expert 预检。list = 纯读
        // 名册（不占信号量、不产生子代理台账、忽略 task/depth）；run 带
        // expert = 先在本侧预检解析（存在 + 启用），错误不占信号量——
        // 真正的提示词/模型消费在 spawn 闭包内再 resolve 一次（磁盘真相
        // 两次读，专家目录个位数成本可忽略）。
        let action = val.get("action").and_then(|v| v.as_str()).unwrap_or("run");
        if action != "run" && action != "list" {
            return Err(format!(
                "Unknown action '{}'. Valid values: \"run\" (default) or \"list\" (enumerate hired experts).",
                action
            ));
        }
        let expert = val
            .get("expert")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        // 自拟提示词（2026-11-13 无匹配自动降级的第二跳；与 expert 互斥）。
        let prompt = val
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !expert.is_empty() && !prompt.is_empty() {
            return Err(
                "Pass either `expert` or `prompt`, not both — `expert` dispatches a hired expert; `prompt` installs your own ad-hoc system prompt (the auto-degrade second hop).".to_string(),
            );
        }
        // 模式策略（2026-11-13 总开关 + 降级）：每次调用现读（fresh-read，
        // 运行中改键即时生效）。handle 缺席 = Default（enabled=false——
        // 下方 None 分支的诚实错误先于策略语义，不依赖此值）。
        let policy = self
            .experts
            .as_ref()
            .map(|h| (h.policy)())
            .unwrap_or_default();
        if action == "list" {
            return Ok(match &self.experts {
                Some(h) => {
                    let roster_text = render_experts_roster(&(h.roster)());
                    if policy.enabled {
                        roster_text
                    } else {
                        format!(
                            "NOTE: Expert mode is currently DISABLED (agents.experts.enabled=false). The roster below is for reference only — dispatching with `expert` or `prompt` is rejected until the mode is enabled (config.json or the Dashboard experts page).\n\n{roster_text}"
                        )
                    }
                }
                None => "No expert roster is configured in this deployment (experts are a main-loop feature). Spawning without `expert` still works with the default sub-agent.".to_string(),
            });
        }
        if !expert.is_empty() {
            let Some(h) = &self.experts else {
                return Err(format!(
                    "Experts are not configured in this deployment; omit `expert` to use the default sub-agent (requested: '{}').",
                    expert
                ));
            };
            if !policy.enabled {
                return Err(format!(
                    "Expert mode is disabled (agents.experts.enabled=false). Enable it in config.json (or the Dashboard experts page) and retry; or omit `expert` to use the default sub-agent (requested: '{}').",
                    expert
                ));
            }
            match (h.resolve)(expert) {
                // 预检只消费校验结果（存在 + 启用），产物丢弃——真正的
                // 提示词/模型解析在 spawn 闭包内做（单一消费点）。
                Ok(_) => {}
                Err(e) => {
                    if policy.degrade {
                        // 无匹配自动降级（2026-11-13）：回灌降级指引（模型
                        // 自拟提示词二段重调，勿停车问用户）；notify=true 时
                        // 发布降级事件（Dashboard toast）。false = 现行诚实
                        // 拒绝行为字节不变（原始错误原样上抛）。
                        if policy.notify {
                            self.emit_degrade_notification(
                                &context.session_key,
                                &context.chat_id,
                                expert,
                                &e,
                            );
                        }
                        return Err(degrade_guidance(&e));
                    }
                    return Err(e);
                }
            }
        }
        if !prompt.is_empty() {
            if self.experts.is_none() {
                return Err(
                    "Experts are not configured in this deployment; omit `prompt` to use the default sub-agent.".to_string(),
                );
            }
            if !policy.enabled {
                return Err(
                    "Expert mode is disabled (agents.experts.enabled=false). Enable it in config.json (or the Dashboard experts page) first — `prompt` dispatch is part of the expert surface.".to_string(),
                );
            }
        }

        // Check allowlist.
        if let Some(ref checker) = self.allowlist_checker
            && !checker(agent_id)
        {
            return Err(format!(
                "Not allowed to spawn agent '{}'. Check sub-agent permissions.",
                agent_id
            ));
        }

        // 深度限制——父深度 + 1（子代理深度）不得超过 `max_depth`。
        // 超限在占信号量/起子代理**之前**诚实拒绝，文案供模型自纠
        // （告诉它当前嵌套已到顶，应自己完成任务而不是再派生）。
        let parent_depth = self
            .invocation_depth
            .load(std::sync::atomic::Ordering::Relaxed);
        let child_depth = parent_depth + 1;
        if child_depth > self.config.max_depth {
            return Err(format!(
                "Sub-agent depth limit ({}) reached. You are already {} level(s) deep in the sub-agent hierarchy — complete the task yourself instead of spawning another sub-agent.",
                self.config.max_depth, parent_depth
            ));
        }

        // Use context from RequestContext, falling back to stored context.
        let channel = if context.channel.is_empty() {
            self.stored_channel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.channel.clone()
        };

        let chat_id = if context.chat_id.is_empty() {
            self.stored_chat_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        } else {
            context.chat_id.clone()
        };

        let spawn_fn = match self.spawn_slot.get() {
            Some(f) => f,
            None => {
                return Err(format!(
                    "Sub-agent spawning is not available (no spawn function configured). Cannot spawn agent '{}' for task.",
                    agent_id
                ));
            }
        };

        // 并发上限真生效——acquire 许可，超出 max_concurrent 的调用在
        // 此排队等待（信号量关闭时 acquire 返回 Err，诚实上报）。
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "spawn semaphore closed".to_string())?;

        spawn_fn(
            agent_id,
            task,
            &self.config.default_model,
            &channel,
            &chat_id,
            tools_profile,
            child_depth,
            background,
            role,
            &context.session_key,
            expert,
            prompt,
        )
        .await
    }
}

impl SpawnTool {
    /// 降级事件发布（notify=true 时）。事件经 event_tx 广播到 web pump
    /// → SSE `expert-degraded`（approval 家族先例的全局展平通道）。发布
    /// 失败静默忽略——通知是尽力而为面，不能反过来影响降级指引回灌。
    fn emit_degrade_notification(
        &self,
        session_key: &str,
        chat_id: &str,
        requested: &str,
        reason: &str,
    ) {
        let Some(tx) = &self.event_tx else {
            return;
        };
        let _ = tx.send(nemesis_types::agent::AgentEvent::ExpertDegraded {
            session_key: session_key.to_string(),
            chat_id: chat_id.to_string(),
            requested: requested.to_string(),
            reason: reason.chars().take(300).collect(),
        });
    }
}

/// 无匹配自动降级指引（2026-11-13）：嵌入原始原因（unknown/disabled
/// 文案由 ExpertsStore 侧区分），指示模型勿停车问用户——带同 task +
/// 自拟 `prompt` 二段重调。这条文本是降级策略的指令载体（策略本身
/// 不进提示词/cache——见 PLAN §二）。
fn degrade_guidance(original_error: &str) -> String {
    format!(
        "AUTO-DEGRADE: {original_error}\n\nAuto-degrade policy is active (agents.experts.degrade=true): do NOT stop to ask the user. Re-call spawn with the SAME task plus a `prompt` parameter containing your own specialist system prompt for this task — write the instructions that expert would have had (role, method, output contract). The sub-agent will run with that prompt instead of the missing expert."
    )
}

/// 专家名册 → 模型可读文本（list 动作产物）。default 行永远在册
/// （spawn 兜底不依赖专家子系统）；停用专家照列并显式标注（模型得知
/// 名单全貌，不再误点名吃「已停用」错）。
fn render_experts_roster(roster: &ExpertsRoster) -> String {
    if roster.experts.is_empty() {
        return format!(
            "No experts hired yet (default sub-agent: \"{}\"). Hire experts from the persona shop or define them on the dashboard, then dispatch with {{\"expert\": \"<slug>\"}}.",
            roster.default_name
        );
    }
    let mut lines = vec![format!(
        "Hired experts ({}); delegate with {{\"expert\": \"<slug>\"}}:",
        roster.experts.len()
    )];
    for e in &roster.experts {
        let state = if e.enabled {
            "enabled"
        } else {
            "DISABLED (cannot be dispatched)"
        };
        let summary = if e.summary.is_empty() {
            String::new()
        } else {
            format!(" — {}", e.summary)
        };
        let tags = if e.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", e.tags.join(", "))
        };
        lines.push(format!(
            "- {} — {} ({}){}{}",
            e.slug, e.name, state, summary, tags
        ));
    }
    lines.push(format!(
        "- (omit `expert`) — {} (default sub-agent, unchanged behavior)",
        roster.default_name
    ));
    lines.join("\n")
}

// ===========================================================================
// Memory tools
// ===========================================================================
