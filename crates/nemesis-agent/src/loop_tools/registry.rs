// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

// 跨子文件引用（机械拆分）：ForgeBridgeTool 定义在 misc.rs
#[cfg(feature = "forge")]
use super::misc::ForgeBridgeTool;

/// Register all default tools and return them as a HashMap.
///
/// The default tools are:
/// - `message` - Send a simple text message
/// - `read_file` - Read a file from disk
/// - `write_file` - Write content to a file on disk
/// - `list_dir` - List the contents of a directory
/// - `edit_file` - Edit a file (action=edit 精确替换 / action=append 追加)
/// - `delete_file` - Delete a file from disk
/// - `create_dir` - Create a directory (and parents)
/// - `delete_dir` - Remove a directory
/// - `sleep` - Sleep for a specified duration
pub fn register_default_tools() -> HashMap<String, Box<dyn Tool>> {
    let mut tools: HashMap<String, Box<dyn Tool>> = HashMap::new();
    tools.insert("message".to_string(), Box::new(MessageTool::new()));
    tools.insert("read_file".to_string(), Box::new(ReadFileTool::default()));
    tools.insert("write_file".to_string(), Box::new(WriteFileTool::default()));
    tools.insert(
        "list_dir".to_string(),
        Box::new(ListDirectoryTool::default()),
    );
    tools.insert("edit_file".to_string(), Box::new(EditFileTool::default()));
    tools.insert("multiedit".to_string(), Box::new(MultiEditTool::default()));
    tools.insert(
        "delete_file".to_string(),
        Box::new(DeleteFileTool::default()),
    );
    tools.insert("create_dir".to_string(), Box::new(CreateDirTool::default()));
    tools.insert("delete_dir".to_string(), Box::new(DeleteDirTool::default()));
    tools.insert("sleep".to_string(), Box::new(SleepTool));
    tools
}

// ===========================================================================
// setup_cluster_rpc_channel -- RPC channel setup (mirrors Go's setupClusterRPCChannel)
// ===========================================================================
/// Extended tool registration configuration.
///
/// Mirrors Go's `registerSharedTools` parameters, bundling all the
/// configuration needed for shared tool registration.
#[derive(Clone, Default)]
pub struct SharedToolConfig {
    /// Web search configuration.
    pub web_search: Option<WebSearchConfig>,
    /// Cluster RPC configuration.
    pub cluster_rpc: Option<ClusterRpcConfig>,
    /// Spawn/subagent configuration.
    pub spawn: Option<SpawnConfig>,
    /// (devtool-upgrade)：spawn 闭包共享槽——agent_factory 构造
    /// slot 并在 AgentLoop 组装完成后注入闭包（持 `Weak<AgentLoop>`），
    /// SpawnTool 与其共享同一 Arc。`None` = SpawnTool 自建空槽（行为与
    /// 旧「无 spawn_fn」完全一致：execute 诚实报 not available）。
    pub spawn_slot: Option<Arc<std::sync::OnceLock<SpawnFn>>>,
    /// Skills registry manager for find/install tools.
    pub skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    /// Skills loader for listing local skills.
    pub skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    /// Workspace path for skill installation.
    pub workspace: Option<String>,
    /// Cron service for scheduling jobs.
    pub cron_service: Option<Arc<std::sync::Mutex<nemesis_cron::service::CronService>>>,
    /// (half): enable the claude_code delegation tool. Default false
    /// (opt-in) — a user with the CLI installed is never surprised by it.
    pub claude_code_tool_enabled: bool,
    /// wall-clock timeout per delegation (None = 300s default).
    pub claude_code_tool_timeout_secs: Option<u64>,
    /// fixed claude permission tier (empty = default accept_edits;
    /// enum default/accept_edits/plan/bypass_permissions). NOT in the tool
    /// schema — deployment config governs, the model cannot choose it.
    pub claude_code_tool_permission_mode: String,
    /// (half): enable the codex_delegate tool. Default false.
    pub codex_tool_enabled: bool,
    /// wall-clock timeout per codex delegation (None = 300s default).
    pub codex_tool_timeout_secs: Option<u64>,
    /// fixed codex sandbox tier (empty = default read_only; enum
    /// read_only/workspace_write/danger_full_access, snake_case config →
    /// kebab-case CLI value). NOT in the tool schema.
    pub codex_tool_sandbox: String,
    /// enable the read-only `lsp` semantic-code tool. Default
    /// false — and even when true, registration additionally requires at
    /// least one language server on PATH (probe at registration).
    pub lsp_tool_enabled: bool,
    /// per-request LSP timeout (None = 120s default).
    pub lsp_tool_timeout_secs: Option<u64>,
    /// idle session reap threshold (None = 600s default).
    pub lsp_tool_idle_secs: Option<u64>,
    /// 外部 LSP manager 单例（agent_factory 在
    /// SharedResources 建一次；gateway shutdown_all 可达）。None = 注册路径
    /// 自建 manager（`LspTool::new` 兜底，旧行为——测试/独立 runner 用）。
    #[cfg(feature = "lsp")]
    pub lsp_manager: Option<Arc<nemesis_lsp::LspManager>>,
    /// Forge tool executor for the merged `forge` self-learning tool
    /// (8 旧 `forge_*` 名收编为单工具多 action，2026-11-06)。
    #[cfg(feature = "forge")]
    pub forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
    #[cfg(not(feature = "forge"))]
    pub forge_executor: Option<()>,
    /// Forge instance for experience collection in AgentLoop.
    #[cfg(feature = "forge")]
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    pub forge: Option<()>,
    /// Memory tool executor for memory_search, memory_store, etc.
    #[cfg(feature = "memory")]
    pub memory_executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)]
    pub memory_executor: Option<()>,
    /// Snapshot of registered MCP tools for McpListTool.
    pub mcp_tool_snapshot: Option<Arc<parking_lot::RwLock<Vec<McpToolSnapshotEntry>>>>,
    /// Workflow engine reference for the `workflow` agent tool（run/create/
    /// capabilities 三合一单工具，2026-11-06）。
    /// `None` means workflows aren't wired in (tool stays unregistered).
    #[cfg(feature = "workflow")]
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub workflow_engine: Option<()>,
    /// Optional approval manager slot for skill_manage write approval.
    pub approval_manager: Option<ApprovalManagerSlot>,
    /// Whether skill_manage writes require interactive approval.
    pub skills_manage_approval: bool,
    /// `install_skill` 工具的装前审批门槽（gateway 晚填
    /// `LateWebSkillsGate`；None = 无审批面直装——headless/测试装配）。
    pub skills_install_gate_slot: Option<SkillsInstallGateSlot>,
    /// SSRF 闸宿主（SecurityPlugin），web_fetch 重定向
    /// 循环逐跳复查用。None = 闸不生效（跳数上限仍生效）。执行体子进程按
    /// 设计不注入（哑执行——判断在 gateway，且 web_fetch 不在 MOVE_TOOLS，
    /// 子进程里的 web_fetch 本就不会被调用）。
    #[cfg(feature = "security")]
    pub security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub security: Option<()>,
    /// A5（2026-09-04）：文件工具工作区边界（write_file / edit_file 纵深
    /// 防御，不单靠安全 8 层管线）。None = 不设界（基线
    /// 注册 / 测试形态）。executor 子进程按同根注入（NEMESISBOT_EXECUTOR_
    /// WORKSPACE），保证 Layer-1 分离后写路径仍受同一边界约束。
    pub workspace_boundary: Option<Arc<WorkspaceBoundary>>,
    /// todowrite 工具配置（workspace 存储 + TodoUpdated
    /// 广播通道）。None = 不注册该工具（register_default_tools 基线形态 /
    /// 无 workspace 的测试）。事件通道与 ToolEventHook 共用同一
    /// broadcast（gateway 建、web pump 消费）；None tx = 仍落盘、不广播。
    pub todo: Option<TodoToolConfig>,
    /// 后台进程注册表（gateway 级单例，跨 agent 重启
    /// 存活；Drop 时给残余任务发 kill 旗标）。Some 时注册
    /// background_start / background_output / background_kill /
    /// background_steer 四动词（steer 为 扩展）。
    /// None = 不注册（exec_worker 子进程——per-call 生命周期里起后台任务
    /// 必然孤儿，见 remote_executor_tool.rs 的 exec_async 注记；以及
    /// register_default_tools 基线/测试形态）。
    pub background_registry: Option<Arc<BackgroundProcessRegistry>>,
    /// exec 执行中输出流事件通道（2026-11-08 子进程实时输出与完成唤醒）：
    /// Some 时 exec 工具走 `new_with_output_hook` 装配——执行中 stdout/
    /// stderr 以 `AgentEvent::ToolOutput` 经此 broadcast 流出（与
    /// ToolEventHook / TodoUpdated 同一通道家族，web pump 默认路径消费）。
    /// None = exec 旧行为（`ExecTool::new`，执行中输出不可见——基线注册/
    /// 测试/executor 分离路径）。broadcast::Sender 是 Clone，hook 闭包
    /// 直接持值。
    pub exec_output_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    /// exec 同步阻塞超时上限（`tools.exec.max_timeout_secs`，2026-11-08
    /// config 化）。None = 600 默认（旧行为字节不变）；Some(n) = exec 的
    /// timeout clamp 上限改为 n（部署主权参数——上限高度 = estop 残留
    /// 窗口）。LLM 侧 `timeout` 参数语义不变（缺省 30，超上限 clamp）。
    pub exec_max_timeout_secs: Option<u64>,
    /// question 工具的 broker 槽（gateway 装配后填
    /// `WebQuestionBroker`）。None = 不注册该工具（headless / exec_worker /
    /// register_default_tools 基线形态——模型看不到一个只会失败的调用）。
    pub question_broker: Option<QuestionBrokerSlot>,
    /// 2026-11-06：硬件总线工具（i2c/spi）注册总闸（config
    /// `tools.hardware.enabled`，默认 false——桌面/服务器部署不注册，
    /// IoT/嵌入式场景显式开）。
    pub hardware_tools_enabled: bool,
    /// P8（2026-09-25 三批合并）：GPIO/I2C 地址访问策略（`tools.hardware`
    /// 段，agent_factory 从 config 构造）。None = 内置默认白名单
    /// （可自由寻址空间 0x08-0x77）。loop_tools 桩与 nemesis-tools 真实
    /// 实现共用同一 `HardwarePolicy` 统一定义。
    pub hardware_policy: Option<Arc<nemesis_tools::hardware::HardwarePolicy>>,
    /// ：workspace 级写租约（`agents.lease_enabled` 默认开，
    /// agent_factory 构造）。Some 时 register_shared_tools 尾部把
    /// LEASE_WRITE_TOOLS 换成 LeaseGuardTool 委派包装（acquire → 执行 →
    /// release；宽限 30s 超时诚实拒绝）。None = 基线/测试形态不包装。
    pub workspace_lease: Option<Arc<crate::workspace_lease::WorkspaceLease>>,
    /// present_plan 工具的共享接线（broker/mode/tx/
    /// verdict 四件）。Some 时注册 `present_plan`（与 question 同条件——
    /// gateway 交互形态）；None = 不注册（headless / 基线注册）。
    pub plan_present: Option<Arc<PlanPresentHandle>>,
    /// present_files 工具的交付物流水桶（与 AgentLoop
    /// `turn_deliverables` 共享同一 Arc——工具登记、loop drain）。Some 时
    /// 注册 `present_files`；None = 不注册（cluster worker / 基线注册——
    /// worker 文件路径对 A 端用户无意义）。
    pub turn_deliverables: Option<TurnDeliverablesBucket>,
}
impl std::fmt::Debug for SharedToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedToolConfig")
            .field("web_search", &self.web_search)
            .field("cluster_rpc", &self.cluster_rpc)
            .field("spawn", &self.spawn)
            .field(
                "skills_registry",
                &self.skills_registry.as_ref().map(|_| "RegistryManager"),
            )
            .field(
                "skills_loader",
                &self.skills_loader.as_ref().map(|_| "SkillsLoader"),
            )
            .field("workspace", &self.workspace)
            .field(
                "memory_executor",
                &self.memory_executor.as_ref().map(|_| "MemoryToolExecutor"),
            )
            .field(
                "mcp_tool_snapshot",
                &self.mcp_tool_snapshot.as_ref().map(|_| "McpToolSnapshot"),
            )
            .field(
                "workflow_engine",
                &self.workflow_engine.as_ref().map(|_| "WorkflowEngine"),
            )
            .field(
                "approval_manager",
                &self
                    .approval_manager
                    .as_ref()
                    .map(|_| "ApprovalManagerSlot"),
            )
            .field("skills_manage_approval", &self.skills_manage_approval)
            .field(
                "skills_install_gate_slot",
                &self
                    .skills_install_gate_slot
                    .as_ref()
                    .map(|_| "SkillsInstallGateSlot"),
            )
            .field("todo", &self.todo)
            .field(
                "question_broker",
                &self.question_broker.as_ref().map(|_| "QuestionBrokerSlot"),
            )
            .field("hardware_tools_enabled", &self.hardware_tools_enabled)
            .field(
                "hardware_policy",
                &self.hardware_policy.as_ref().map(|_| "HardwarePolicy"),
            )
            .field(
                "workspace_lease",
                &self.workspace_lease.as_ref().map(|_| "WorkspaceLease"),
            )
            .field(
                "plan_present",
                &self.plan_present.as_ref().map(|_| "PlanPresentHandle"),
            )
            .field(
                "turn_deliverables",
                &self
                    .turn_deliverables
                    .as_ref()
                    .map(|_| "TurnDeliverablesBucket"),
            )
            .field(
                "security",
                &self.security.as_ref().map(|_| "SecurityPlugin"),
            )
            .finish()
    }
}

/// Register all shared tools across agents.
///
/// Mirrors Go's `registerSharedTools` function. Creates the complete set of
/// tools (basic file ops + web + cluster + spawn + memory + skills + hardware)
/// and returns them as a HashMap ready for registration with an AgentLoopExecutor.
///
/// # Arguments
/// * `config` - Configuration for optional tools (web, cluster, spawn, MCP)
///
/// # Returns
/// A HashMap of tool name -> tool implementation.
pub fn register_shared_tools(config: &SharedToolConfig) -> HashMap<String, Box<dyn Tool>> {
    let mut tools = register_default_tools();

    // A5（2026-09-04）：配置了工作区边界时，write/edit 换成带界的
    // 生产形态（纵深防御，不单靠安全 8 层管线）。无界（测试 / 基线注册）
    // 保持 register_default_tools 的默认形态。
    // FT：读族 5 工具（read_file/list_dir/delete_file/
    // create_dir/delete_dir）同批换带界形态——相对路径锚定工作区根 +
    // restrict 时第二道闸（delete 族是破坏性操作，此前完全无闸）。
    // 2026-11-06：append_file 并入 edit_file（action=append），原 append
    // 的带界形态由 edit_file::with_boundary 一并承载（append 分支同走
    // FenceOpFamily::WriteDelete）。
    if let Some(ref boundary) = config.workspace_boundary {
        tools.insert(
            "write_file".to_string(),
            Box::new(WriteFileTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "edit_file".to_string(),
            Box::new(EditFileTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "multiedit".to_string(),
            Box::new(MultiEditTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "read_file".to_string(),
            Box::new(ReadFileTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "list_dir".to_string(),
            Box::new(ListDirectoryTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "delete_file".to_string(),
            Box::new(DeleteFileTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "create_dir".to_string(),
            Box::new(CreateDirTool::with_boundary(boundary.clone())),
        );
        tools.insert(
            "delete_dir".to_string(),
            Box::new(DeleteDirTool::with_boundary(boundary.clone())),
        );
    }

    // todowrite —— 全量提交式 todo 清单，写
    // sessions/todo_{safe_key}.json + 广播 TodoUpdated（web pump → WS push
    // + SSE）。None = 不注册（register_default_tools 基线形态零开销）。
    if let Some(ref todo) = config.todo {
        tools.insert(
            "todowrite".to_string(),
            Box::new(TodoWriteTool::new(
                todo.workspace.clone(),
                todo.event_tx.clone(),
            )),
        );
    }

    // Web search tool.
    if let Some(ref web_config) = config.web_search {
        tools.insert(
            "web_search".to_string(),
            Box::new(WebSearchTool::new(web_config.clone())),
        );
    }

    // Web fetch tool (always available).：注入 SSRF 闸
    // 宿主——重定向手动循环逐跳复查（无闸时仍限跳数 ≤5）。mut 只在
    // security 形态用到（ssrf 注入），minimal 形态 cfg_attr 压 unused_mut
    // （远端 minimal clippy；不能删 mut——security 形态需要）。
    // 注入 workspace 根（超限全文存档 spill 用）。web_fetch
    // 无条件注册（与 workspace 有无无关）——workspace 缺席时 with_workspace
    // 收空串，超限退回旧「截断+注记」行为（诚实不静默）。
    #[cfg_attr(not(feature = "security"), allow(unused_mut))]
    let mut web_fetch =
        WebFetchTool::new(50000).with_workspace(config.workspace.as_deref().unwrap_or(""));
    #[cfg(feature = "security")]
    {
        web_fetch.ssrf = config.security.clone();
    }
    tools.insert("web_fetch".to_string(), Box::new(web_fetch));

    // Cluster RPC tool (bot-to-bot communication).
    if let Some(ref cluster_config) = config.cluster_rpc {
        tools.insert(
            "cluster_rpc".to_string(),
            Box::new(ClusterRpcTool::new(cluster_config.clone())),
        );
    }

    // Spawn/subagent tool.
    // spawn_slot Some 时共享槽（factory 延迟注入闭包）；None 自建空槽
    // （not available 行为与旧装配路径完全一致）。
    if let Some(ref spawn_config) = config.spawn {
        let tool = match config.spawn_slot.clone() {
            Some(slot) => SpawnTool::with_spawn_slot(
                spawn_config.clone(),
                slot,
                Arc::new(tokio::sync::Semaphore::new(
                    spawn_config.max_concurrent.max(1),
                )),
            ),
            None => SpawnTool::new(spawn_config.clone()),
        };
        tools.insert("spawn".to_string(), Box::new(tool));
    }

    // Memory tools.
    #[cfg(feature = "memory")]
    {
        tools.insert(
            "memory_search".to_string(),
            Box::new(MemorySearchTool::new(config.memory_executor.clone())),
        );
        tools.insert(
            "memory_store".to_string(),
            Box::new(MemoryStoreTool::new(config.memory_executor.clone())),
        );
        tools.insert(
            "memory_forget".to_string(),
            Box::new(MemoryForgetTool::new(config.memory_executor.clone())),
        );
        tools.insert(
            "memory_list".to_string(),
            Box::new(MemoryListTool::new(config.memory_executor.clone())),
        );
    }

    // Skills tools: use real loader when available, otherwise use stub.
    tools.insert(
        "skills_list".to_string(),
        Box::new(SkillsListTool::new(config.skills_loader.clone())),
    );
    tools.insert(
        "skills_info".to_string(),
        Box::new(SkillsInfoTool::new(config.skills_loader.clone())),
    );

    // Find and install skills from remote registries.
    if let Some(ref registry) = config.skills_registry {
        tools.insert(
            "find_skills".to_string(),
            Box::new(FindSkillsTool::new(registry.clone())),
        );
        if let Some(ref workspace) = config.workspace {
            tools.insert(
                "install_skill".to_string(),
                Box::new(InstallSkillTool::new(
                    registry.clone(),
                    workspace.clone(),
                    config.skills_install_gate_slot.clone(),
                )),
            );
        }
    }

    // Skill manage tool — agent-authored skills (procedural memory).
    if let Some(ref workspace) = config.workspace {
        // approval_manager 按 cfg 取值：security 形态是 Arc 槽需 clone，
        // minimal 形态别名退化为 （Copy）——clone 会被判 clone_on_copy
        // （远端 minimal clippy）。
        #[cfg(feature = "security")]
        let approval_manager = config.approval_manager.clone();
        #[cfg(not(feature = "security"))]
        let approval_manager = config.approval_manager;
        tools.insert(
            "skill_manage".to_string(),
            Box::new(SkillManageTool::new(
                workspace.clone(),
                approval_manager,
                config.skills_manage_approval,
            )),
        );
    }

    // question tool — structured ask-the-user card on the
    // Dashboard, blocking on the answer. Only registered when a broker is
    // wired (gateway); headless / baseline forms never see it.
    if let Some(ref slot) = config.question_broker {
        tools.insert(
            "question".to_string(),
            Box::new(QuestionTool::new(slot.clone())),
        );
    }

    // present_plan — plan 模式审批卡（批准翻 build /
    // 驳回结构化止步）。注册条件与 question 同源（broker 已接线 = 交互
    // 形态）；handle 由 agent_factory 构造并双侧注入（工具 + loop）。
    if let Some(ref handle) = config.plan_present {
        tools.insert(
            "present_plan".to_string(),
            Box::new(PlanPresentTool::new(handle.clone())),
        );
    }

    // present_files — turn 交付物登记（「该看什么」产物
    // 指引，随 assistant 回复落盘为 deliverables 字段）。注册条件 = 桶与
    // workspace 双接线（agent_factory 注入 loop 同一 Arc）；cluster worker /
    // 基线形态不注册（worker 的文件路径对 A 端用户无意义）。
    if let (Some(workspace), Some(bucket)) = (&config.workspace, &config.turn_deliverables) {
        tools.insert(
            "present_files".to_string(),
            Box::new(PresentFilesTool::new(workspace.clone(), bucket.clone())),
        );
    }

    // Coding tools (grep / git) — read-only code search & git queries.
    if let Some(ref workspace) = config.workspace {
        tools.insert(
            "grep".to_string(),
            Box::new(GrepTool::new(workspace.clone())),
        );
        tools.insert("git".to_string(), Box::new(GitTool::new(workspace.clone())));
    }

    // Hardware tools (I2C / SPI - Linux only, no-op on other platforms).
    // I2C 带地址策略（config `tools.hardware` 段；None = 内置默认
    // 白名单 0x08-0x77——Default 形态同样拒绝保留段，不因未配置而裸奔）。
    //
    // ⚠️ 刻意默认关，不是遗漏（2026-11-06 用户拍板，勿当 bug 修回）：
    // i2c/spi 只在 Linux + 真实硬件总线存在的 IoT/嵌入式场景有意义；
    // 桌面/服务器部署注册它们 = 工具表白白多两行 schema + 小模型选型
    // 噪声，且绝大多数宿主根本没有 /dev/i2c-*，注册了也只会调用失败。
    // 不要因为「这里 if 看起来漏了什么」而把它改回默认注册。
    //
    // 开启方式：workspace/config/config.json 显式加
    //   "tools": { "hardware": { "enabled": true } }
    // 注册后仍受 hardware_policy 地址白名单约束（默认可自由寻址空间
    // 0x08-0x77，保留段恒拒）。
    if config.hardware_tools_enabled {
        let hw_policy = config
            .hardware_policy
            .clone()
            .map(|p| (*p).clone())
            .unwrap_or_else(nemesis_tools::hardware::HardwarePolicy::builtin);
        tools.insert("i2c".to_string(), Box::new(I2CTool::with_policy(hw_policy)));
        tools.insert("spi".to_string(), Box::new(SPITool));
    }

    // Exec tool + Async exec tool (mirrors Go's ExecTool + AsyncExecTool).
    if let Some(ref workspace) = config.workspace {
        let restrict = true; // restrict to workspace by default
        // [SEALED 2026-11-08] 旧模式（执行中输出不可见）：裸 `ExecTool::new`
        // 注册——exec 运行期间 Chat 页无中间输出。保留备查；恢复 = 把下面
        // new_with_output_hook 分支换回这三行。
        // tools.insert(
        //     "exec".to_string(),
        //     Box::new(ExecTool::new(workspace, restrict)),
        // );
        // ── 新模式（执行中输出流）：exec_output_tx 在场 → hook 闭包把
        // 排水 tee 的尾窗以 ToolOutput 事件发进同一 broadcast（web pump
        // 默认 tool_event 路径消费）；tx 缺席（基线/测试/exec_worker）→
        // 旧行为构造，逐字节一致。
        let exec_tool = if let Some(tx) = config.exec_output_tx.clone() {
            ExecTool::new_with_output_hook(workspace, restrict, {
                let tx = tx.clone();
                Arc::new(move |ctx: &RequestContext, tail: &str, total_bytes: u64| {
                    let _ = tx.send(nemesis_types::agent::AgentEvent::ToolOutput {
                        session_key: ctx.session_key.clone(),
                        chat_id: ctx.chat_id.clone(),
                        // call_id 置空：dispatch call_id 工具层不可见
                        // （ToolEventHook 层私有）——前端按「会话内最后
                        // 一个 running exec 行」归属。
                        call_id: String::new(),
                        tool: "exec".to_string(),
                        tail: tail.to_string(),
                        total_bytes,
                    });
                })
            })
        } else {
            ExecTool::new(workspace, restrict)
        }
        // timeout clamp 上限（2026-11-08 config 化）：None = 600 旧行为。
        .with_max_timeout_secs(config.exec_max_timeout_secs.unwrap_or(600));
        tools.insert("exec".to_string(), Box::new(exec_tool));
        // [SEALED 2026-11-08] exec_async 封存（用户拍板，实现保留不删除）。
        // 根因：fire-and-forget（管道只接不排，64KB 写满即死锁）+ 无
        // job_id/kill/完成唤醒；描述文本「启动一个应用并立即返回」主动诱导
        // 模型在 GUI 场景绕开 background_start 的完成唤醒路径（2026-11-08
        // 用户亲测实录）。用例由 background_start/output/kill/steer +
        // 完成唤醒全覆盖。恢复 = 取消下面注释（同步恢复 capability.rs
        // mini/normal 白名单条目 + nemesis-prompts tools.rs 表条目——三面
        // 一致才完整供给）。
        // tools.insert(
        //     "exec_async".to_string(),
        //     Box::new(AsyncExecTool::new(workspace, restrict)),
        // );

        // run_script: interpreter-driven script execution for the workflow
        // `script` node. Listed in MOVE_TOOLS so executor separation / sandbox
        // contains workflow scripts identically to `exec`. Returns structured
        // {stdout,stderr,exit_code} (not flattened), preserving the script
        // node's output contract.
        tools.insert(
            "run_script".to_string(),
            Box::new(RunScriptTool::new(workspace, restrict)),
        );

        // Bootstrap completion tool — deletes BOOTSTRAP.md after initialization.
        tools.insert(
            "complete_bootstrap".to_string(),
            Box::new(BootstrapTool::new(workspace)),
        );

        // build/test runner with focused output. In
        // MOVE_TOOLS (builds write target/; executor separation & sandbox
        // should contain it; plan mode blocks it via the same derivation).
        tools.insert(
            "run_checks".to_string(),
            Box::new(RunChecksTool::new(workspace)),
        );
    }

    // background process trio — needs BOTH a workspace (cwd
    // resolution / boundary) and a gateway-level registry. NOT in MOVE_TOOLS:
    // job handles live in this registry (gateway process lifetime), so a
    // per-call executor child could not own them — see background_registry.rs
    // top docs. exec_worker passes background_registry: None ⇒ not registered
    // there.
    if let (Some(workspace), Some(registry)) = (
        config.workspace.as_deref(),
        config.background_registry.as_ref(),
    ) {
        let restrict = true; // cwd confined to workspace, same as exec
        tools.insert(
            "background_start".to_string(),
            Box::new(BackgroundStartTool::new(
                workspace,
                restrict,
                Arc::clone(registry),
            )),
        );
        tools.insert(
            "background_output".to_string(),
            Box::new(BackgroundOutputTool::new(Arc::clone(registry))),
        );
        tools.insert(
            "background_kill".to_string(),
            Box::new(BackgroundKillTool::new(Arc::clone(registry))),
        );
        tools.insert(
            "background_steer".to_string(),
            Box::new(BackgroundSteerTool::new(Arc::clone(registry))),
        );
    }

    // Cron tool (mirrors Go's CronTool).
    if let Some(ref cron_svc) = config.cron_service {
        tools.insert(
            "cron".to_string(),
            Box::new(CronTool::new(Arc::clone(cron_svc))),
        );
    }

    // (half): claude CLI delegation — opt-in (config flag, default
    // false) AND the CLI must be locatable. Absent CLI ⇒ not registered
    // (graceful degradation, one info line). The probe is cached for the
    // process lifetime (static OnceLock).
    if config.claude_code_tool_enabled {
        static CLI_PROBE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let cli = CLI_PROBE.get_or_init(claude_code_tool::find_claude_cli);
        match cli {
            Some(path) => {
                info!(
                    "[Tools] claude_code delegation tool registered (cli: {})",
                    path
                );
                tools.insert(
                    "claude_code".to_string(),
                    Box::new(claude_code_tool::ClaudeCodeTool::new(
                        path.clone(),
                        config.claude_code_tool_timeout_secs,
                        Some(&config.claude_code_tool_permission_mode),
                    )),
                );
            }
            None => {
                info!(
                    "[Tools] claude_code_tool enabled in config but claude CLI not found on PATH; tool not registered"
                );
            }
        }
    }

    // (other half): Codex CLI delegation — same opt-in + probe shape.
    if config.codex_tool_enabled {
        static CODEX_PROBE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let cli = CODEX_PROBE.get_or_init(codex_tool::find_codex_cli);
        match cli {
            Some(path) => {
                info!("[Tools] codex_delegate tool registered (cli: {})", path);
                tools.insert(
                    "codex_delegate".to_string(),
                    Box::new(codex_tool::CodexTool::new(
                        path.clone(),
                        config.codex_tool_timeout_secs,
                        Some(&config.codex_tool_sandbox),
                    )),
                );
            }
            None => {
                info!(
                    "[Tools] codex_tool enabled in config but codex CLI not found on PATH; tool not registered"
                );
            }
        }
    }

    // read-only LSP semantic-code tool — same opt-in + probe
    // shape: config opts in AND at least one language server on PATH.
    // Absent/failed probe ⇒ tool not registered at all (no dead schema).
    #[cfg(feature = "lsp")]
    if config.lsp_tool_enabled {
        static LSP_PROBE: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
        let available = LSP_PROBE.get_or_init(|| {
            nemesis_lsp::registry::probe_available()
                .iter()
                .map(|l| l.label())
                .collect()
        });
        if available.is_empty() {
            info!(
                "[Tools] lsp_tool enabled in config but no language server \
                 (rust-analyzer/gopls/typescript-language-server/pyright/clangd) \
                 found on PATH; tool not registered"
            );
        } else {
            info!(
                "[Tools] lsp tool registered (language servers on PATH: {})",
                available.join(", ")
            );
            // external singleton manager when provided (gateway-owned,
            // shutdown_all-reachable); self-built fallback otherwise.
            let mut lsp_impl = match config.lsp_manager.clone() {
                Some(mgr) => lsp_tool::LspTool::with_manager(mgr),
                None => {
                    lsp_tool::LspTool::new(config.lsp_tool_timeout_secs, config.lsp_tool_idle_secs)
                }
            };
            // rename 落盘闸宿主——与 web_fetch 的 ssrf 注入同构
            // （SharedToolConfig.security 是 起的统一注入点）。
            // cfg 双形态：security feature 下字段是 Option<Arc<SecurityPlugin>>
            // （clone 增引用计数）；no-security 编译降级 Option<>（Copy 直接
            // 移动，clone 会吃 clone_on_copy）。此前注释「workspace 门禁不触发」
            // 的单一.clone() 写法在 workspace clippy 实证会红
            // （agent lib 以 security 关形态进图），双形态写法两头都合法。
            #[cfg(feature = "security")]
            {
                lsp_impl.security = config.security.clone();
            }
            #[cfg(not(feature = "security"))]
            {
                lsp_impl.security = config.security;
            }
            // rename 落盘挂工作区边界（与 write_file
            // 等文件工具同一实例——纵深防御第二道闸覆盖 rename 这条写路径）。
            if let Some(ref boundary) = config.workspace_boundary {
                lsp_impl = lsp_impl.with_boundary(boundary.clone());
            }
            tools.insert("lsp".to_string(), Box::new(lsp_impl));
        }
    }

    // Forge tools (mirrors Go's forgeTools registration in bot_service.go).
    // Registered when forge executor is provided (i.e. forge.enabled = true).
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge_executor) = config.forge_executor {
            let forge_defs = nemesis_forge::forge_tools::forge_tool_definitions();
            let forge_count = forge_defs.len();
            for def in &forge_defs {
                let bridge = ForgeBridgeTool::new(
                    def.name.clone(),
                    def.description.clone(),
                    def.parameters.clone(),
                    Arc::clone(forge_executor),
                );
                tools.insert(def.name.clone(), Box::new(bridge));
            }
            info!("[AgentTools] Registered {} forge tools", forge_count);
        }
    }

    // MCP discovery and listing tools.
    tools.insert("mcp_discover".to_string(), Box::new(McpDiscoverTool::new()));
    tools.insert(
        "cli_reference".to_string(),
        Box::new(CliReferenceTool::new()),
    );
    // (sixth batch): cross-session history full-text search.
    #[cfg(feature = "history-search")]
    tools.insert(
        "history_search".to_string(),
        Box::new(HistorySearchTool::new()),
    );
    {
        let snapshot = config
            .mcp_tool_snapshot
            .clone()
            .unwrap_or_else(|| Arc::new(parking_lot::RwLock::new(Vec::new())));
        tools.insert("mcp_list".to_string(), Box::new(McpListTool::new(snapshot)));
    }
    // 惰性供给 meta 工具（供给闸在 build_tool_defs，dispatch 拦截解包；
    // lazy off 时供给面不出现——注册恒在，行为零变化由双闸保证）。
    tools.insert("mcp_call".to_string(), Box::new(McpCallTool));
    // 建设：惰性 schema 供给取回工具（同上形态——注册恒在，供给闸钉死
    // 在 schema_lazy 旗标，dispatch 拦截渲染 registry 真实 schema 并记账）。
    tools.insert("tools_schema_get".to_string(), Box::new(ToolsSchemaGetTool));

    // Workflow tool — 单工具多 action（2026-11-06 三合一收编，对齐 forge
    // 形态；2026-11-07 增 apply 成四动作）：action=run 触发已注册工作流 /
    // action=create（对话生成）把 AI 生成的定义存草稿 / action=apply 把草稿
    // 登记进引擎（与工作流页应用按钮同一 DraftStore::apply 路径）/ action=
    // capabilities 查节点类型与 config 字段清单。
    #[cfg(feature = "workflow")]
    {
        if let Some(ref engine) = config.workflow_engine {
            tools.insert(
                "workflow".to_string(),
                Box::new(WorkflowTool::new(engine.clone())),
            );
            info!("[AgentTools] Registered workflow tool (action=run/create/apply/capabilities)");
        }
    }

    // /：workspace 写租约——`workspace_lease` 配置时把写类工具换成
    // LeaseGuardTool 委派包装（协议面/预览面全量透传 inner，模型 schema
    // 与 checkpoint 安全网不变；执行面 acquire → inner.execute → drop 释
    // 放，宽限 30s 超时诚实拒绝）。**包装层接线**：不动 dispatch 流；插
    // 在边界块之后——inner 是带界的生产形态（租约是协调机制不是安全闸，
    // 边界/安全 8 层照常在 inner 生效）。exec 不包（v1 诚实边界：命令面
    // 包装拦不到 exec 的效果面写）。
    if let Some(ref lease) = config.workspace_lease {
        let mut wrapped = 0usize;
        for name in crate::workspace_lease::LEASE_WRITE_TOOLS {
            if let Some(inner) = tools.remove(name) {
                tools.insert(
                    (*name).to_string(),
                    Box::new(crate::workspace_lease::LeaseGuardTool::new(
                        Arc::from(inner),
                        lease.clone(),
                    )),
                );
                wrapped += 1;
            }
        }
        info!("[AgentTools] lease guard wrapped {wrapped} write-class tools");
    }

    info!(
        "[AgentTools] Registered {} shared tools (web={}, cluster={}, spawn={}, workflow={})",
        tools.len(),
        config.web_search.is_some(),
        config.cluster_rpc.is_some(),
        config.spawn.is_some(),
        config.workflow_engine.is_some(),
    );

    tools
}

// ===========================================================================
// WorkflowTool — agent 侧工作流单工具（run/create/apply/capabilities 四动作）。
// ===========================================================================
/// Register extended tools including web search, memory, and skills.
///
/// Returns all tools: default + extended.
pub fn register_extended_tools(
    web_config: Option<WebSearchConfig>,
    cluster_config: Option<ClusterRpcConfig>,
    spawn_config: Option<SpawnConfig>,
) -> HashMap<String, Box<dyn Tool>> {
    let shared_config = SharedToolConfig {
        web_search: web_config,
        cluster_rpc: cluster_config,
        spawn: spawn_config,
        spawn_slot: None,
        skills_registry: None,
        skills_loader: None,
        workspace: None,
        cron_service: None,
        claude_code_tool_enabled: false,
        claude_code_tool_timeout_secs: None,
        claude_code_tool_permission_mode: String::new(),
        codex_tool_enabled: false,
        codex_tool_timeout_secs: None,
        codex_tool_sandbox: String::new(),
        lsp_tool_enabled: false,
        lsp_tool_timeout_secs: None,
        lsp_tool_idle_secs: None,
        #[cfg(feature = "lsp")]
        lsp_manager: None,
        forge_executor: None,
        forge: None,
        memory_executor: None,
        mcp_tool_snapshot: None,
        workflow_engine: None,
        approval_manager: None,
        skills_manage_approval: false,
        skills_install_gate_slot: None,
        security: None,
        workspace_boundary: None,
        todo: None,
        background_registry: None,
        exec_output_tx: None,
        exec_max_timeout_secs: None,
        question_broker: None,
        hardware_tools_enabled: false,
        hardware_policy: None,
        workspace_lease: None,
        plan_present: None,
        turn_deliverables: None,
    };
    register_shared_tools(&shared_config)
}
