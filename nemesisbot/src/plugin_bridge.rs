// ---------------------------------------------------------------------------
// WASM 插件框架 ↔ gateway 桥（W5-3，计划 §5.4/§5.6）。
//
// 五件：
// 1. [`PluginToolBridge`]——agent `Tool` trait 适配器：execute →
//    PluginManager.execute_tool（full frame → runtime），is_error=业务失败
//    转 Err（触发 post-failure hooks，LLM 看到 "Tool error: ..." 同内置
//    风格）；输出经凭据复扫（W5⑤）——guest 回给 LLM 的文本命中凭据模式
//    即 redact + 审计，防 secret 原文经工具结果泄漏进会话历史。
// 2. [`GatewayHostInvoker`]——宿主 tool-invoke 面：Weak<AgentLoop> +
//    `plugin.` 前缀拒绝（插件不能调插件工具，depth 1 递归闸）+ 完整
//    dispatch 瀑布（estop/hidden/Plan/安全 8 层/hooks 全生效——guest 侧
//    tool-invoke 不是安全旁路）。
// 3. [`VaultPluginSecrets`]——manifest x-secret 别名 → vault 原文
//    （`vault:plugin/<slug>/<name>`），解析逻辑与 vault_runtime 同源。
// 4. [`LateInstallApprover`]——安装审批晚绑适配器（LateWebSkillsGate 同
//    模板）：init_agent 建 PluginInstaller 时注入，run_runtime 审批块
//    bind 真身 WebApprovalManager；未 bind = fail-closed 诚实拒绝。
// 5. [`register_plugin_tools`] / [`spawn_estop_watcher`]——装配入口：桥注册 +
//    操作类型声明（声明式 ABAC：read→FileRead / write→FileWrite /
//    exec→ProcessExec / network→NetworkRequest；空串/未知 = 不声明，
//    dispatch 未注册名 fail-closed CRITICAL——WIT 合同口径）+ estop watch
//    联动（急停冻结子系统，释放恢复；invoker 挂接在 build_agent_loop
//    每次重建时执行）。
//
// 模块随 `plugins-wasm` feature 门控（该 feature implies security——信任/
// 审计/扫描机制依赖 nemesis-security）。
// ---------------------------------------------------------------------------

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nemesis_agent::context::RequestContext;
use nemesis_plugins_wasm::host_impl::{HostToolInvoker, SecretResolver};
use nemesis_plugins_wasm::install::{InstallApprover, InstallReview};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::{PluginManager, ToolMetaSnapshot};
use tracing::info;

use crate::common;

/// 工具桥（agent Tool trait 适配；per-tool 一个实例）。
pub struct PluginToolBridge {
    meta: ToolMetaSnapshot,
    manager: Arc<PluginManager>,
    depth: AtomicUsize,
}

impl PluginToolBridge {
    /// 每工具一个桥实例（注册期构造；meta 是安装期对账快照）。
    pub fn new(meta: ToolMetaSnapshot, manager: Arc<PluginManager>) -> Self {
        Self {
            meta,
            manager,
            depth: AtomicUsize::new(0),
        }
    }

    /// 声明式 ABAC 映射（guest operation-type → 宿主 OperationType）。
    /// 四个合法值显式映射；空串/未知 → `None` = **不声明**——dispatch 时
    /// 走 `effective_tool_operation` 未注册名 fail-closed（ProcessExec /
    /// CRITICAL 过全管线），对齐 WIT 合同「未按惯例注册的操作类型按未注
    /// 册名默认 CRITICAL」承诺（2026-09-30 插件体系复查 #3 从紧：此前空
    /// 串/未知落 FileRead 基线，比 WIT 承诺宽；已入库示例全部显式声明，
    /// 零兼容影响）。
    /// `pub(crate)`：热装 hook（run_runtime 注入闭包）与启动装载共用同一映射。
    pub(crate) fn map_operation(op: &str) -> Option<nemesis_security::types::OperationType> {
        use nemesis_security::types::OperationType;
        match op {
            "read" => Some(OperationType::FileRead),
            "write" => Some(OperationType::FileWrite),
            "exec" => Some(OperationType::ProcessExec),
            "network" => Some(OperationType::NetworkRequest),
            // 未声明（空串）/未知值 = 不猜、不声明 → fail-closed。
            _ => None,
        }
    }

    /// W5⑤：工具输出出站凭据复扫——guest 返回文本命中凭据模式即脱敏
    /// （`[REDACTED_CREDENTIAL]`）+ 审计。防 x-secret 原文经工具结果
    /// 泄漏进会话历史/审计面。
    ///
    /// Scanner 恒开（`true`，无视 security.credential 开关）是故意的：
    /// 这不是常规凭据检测层，而是 x-secret 注入面的收口——宿主把 vault
    /// 原文交给了 guest，无论操作员是否启用通用凭据扫描，插件输出都不
    /// 应把原文带出宿主（2026-09-30 复查 #3）。Scanner 构造廉价
    /// （pattern 集 OnceLock 缓存），无需缓存实例。
    fn rescan_output_credentials(&self, content: &str, session_key: &str) -> String {
        let scanner = nemesis_security::credential::Scanner::new(true, "block");
        let result = scanner.scan_content(content);
        if !result.has_matches {
            return content.to_string();
        }
        // 审计只记模式摘要不记命中原文（同 8 层凭据层口径）。
        self.manager.audit_event(
            "plugin_output_credential_redacted",
            "redacted",
            &self.meta.name,
            "MEDIUM",
            &result.summary,
            session_key,
        );
        scanner.redact_content(content)
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for PluginToolBridge {
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let exec_ctx = nemesis_plugins_wasm::runtime::ExecContext {
            session_key: context.session_key.clone(),
            call_id: uuid::Uuid::new_v4().to_string(),
        };
        let depth = self.depth.load(Ordering::SeqCst);
        tracing::debug!(
            tool = %self.meta.name,
            depth,
            session = %context.session_key,
            "[WasmPlugin] bridge execute"
        );
        match self
            .manager
            .execute_tool(&self.meta.name, args.to_string(), exec_ctx)
            .await
        {
            Ok(output) => {
                if output.is_error {
                    // guest 业务失败 → Err（dispatch 包 "Tool error: {err}"
                    // + post-failure hooks，与内置工具失败同语义）。错误文本
                    // 同样走出站凭据复扫——失败信息可能回显 guest 刚读到的
                    // 凭据（2026-09-30 复查 #1：此前仅成功路径扫描）。
                    return Err(
                        self.rescan_output_credentials(&output.content, &context.session_key)
                    );
                }
                Ok(self.rescan_output_credentials(&output.content, &context.session_key))
            }
            Err(e) => {
                // PluginError（trap/超时/预算/禁用）→ 错误文本回灌；不带
                // "Tool error: " 前缀（dispatch 统一包）。顺序必须是先复扫
                // 全文再截断——先截断可能把凭据切成两半各自漏检（2026-09-30
                // 复查 #1：错误路径此前完全不过复扫）。
                let scanned = self.rescan_output_credentials(&e.to_string(), &context.session_key);
                let mut cut = 300.min(scanned.len());
                while cut > 0 && !scanned.is_char_boundary(cut) {
                    cut -= 1;
                }
                Err(if scanned.len() > 300 {
                    format!("{}…", &scanned[..cut])
                } else {
                    scanned
                })
            }
        }
    }

    fn set_invocation_depth(&self, depth: usize) {
        self.depth.store(depth, Ordering::SeqCst);
    }

    fn description(&self) -> String {
        self.meta.description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::from_str(&self.meta.parameters_json)
            .unwrap_or(serde_json::json!({"type": "object", "properties": {}}))
    }

    fn preview(&self, _args: &str) -> Option<nemesis_agent::r#loop::FileChange> {
        // 插件工具无 checkpoint 预检面（guest 内部副作用宿主不可见——
        // 快照安全网不覆盖；诚实返回 None）。
        None
    }

    fn is_read_only(&self) -> bool {
        self.meta.operation_type == "read"
    }

    fn min_tier(&self) -> Option<&str> {
        // manifest 校验保证非空且 ∈ {mini,normal,big}（registry 对账快照
        // 随桥走，无边车表无漂移）；合成测试外的空串经 rank 兜底 = big。
        Some(&self.meta.min_tier)
    }
}

/// 宿主工具调用面（guest tool-invoke → dispatch 瀑布）。
pub struct GatewayHostInvoker {
    loop_ref: std::sync::Weak<nemesis_agent::r#loop::AgentLoop>,
}

impl GatewayHostInvoker {
    /// Arc<AgentLoop> 降级持 Weak（loop 重建不死锁旧引用）。
    pub fn new(loop_arc: &Arc<nemesis_agent::r#loop::AgentLoop>) -> Self {
        Self {
            loop_ref: Arc::downgrade(loop_arc),
        }
    }
}

impl HostToolInvoker for GatewayHostInvoker {
    fn invoke(
        &self,
        tool: &str,
        args_json: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        let tool = tool.to_string();
        let args_json = args_json.to_string();
        Box::pin(async move {
            // depth 1 递归闸：插件不能调插件工具（registry 侧已拦，此处
            // 双保险——防御纵深成本为零）。
            if tool.starts_with(nemesis_plugins_wasm::PLUGIN_TOOL_PREFIX) {
                return Err("plugin tools cannot invoke plugin tools".to_string());
            }
            let Some(loop_arc) = self.loop_ref.upgrade() else {
                return Err("agent loop unavailable (shutting down?)".to_string());
            };
            let call = nemesis_agent::types::ToolCallInfo {
                id: uuid::Uuid::new_v4().to_string(),
                name: tool,
                arguments: args_json,
            };
            // 完整 dispatch 瀑布：estop/hidden/Plan/安全 8 层/hooks 全生效
            // （guest tool-invoke 不是安全旁路）。user 字段标 wasm-plugin
            // 供审计溯源；session_key 空——宿主侧审计已带 slug 溯源。
            let ctx = RequestContext::new("plugin", "", "wasm-plugin", "");
            let result = loop_arc.handle_tool_call_at_depth(&call, &ctx, 0).await;
            if nemesis_agent::turn_guard::tool_result_indicates_error(&result) {
                Err(result)
            } else {
                Ok(result)
            }
        })
    }
}

/// manifest x-secret 别名解析（vault 桥）。别名到达时已是完整
/// `vault:plugin/<slug>/<name>` 串——剥 `vault:` 前缀后走 vault_runtime
/// 同源解析（每次现开现解锁，无缓存）。
pub struct VaultPluginSecrets {
    vault_path: std::path::PathBuf,
}

impl VaultPluginSecrets {
    /// vault 路径取 workspace 唯一拼接点。
    pub fn for_home(home: &std::path::Path) -> Self {
        let ws = common::workspace_path(home);
        Self {
            vault_path: nemesis_path::resolve_vault_path_in_workspace(&ws),
        }
    }
}

impl SecretResolver for VaultPluginSecrets {
    fn resolve(&self, alias: &str) -> Option<String> {
        let bare = alias.strip_prefix("vault:").unwrap_or(alias);
        match crate::vault_runtime::resolve_alias_at(&self.vault_path, bare) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(alias = %bare, error = %e, "[WasmPlugin] 凭据解析失败");
                None
            }
        }
    }
}

/// 插件敏感宿主调用审计（独立 AuditLogger 实例同审计目录追加；None =
/// 静默跳过——审计文件开关关闭时诚实不记）。SecurityPlugin 的审计器
/// 不可共享（audit_logger() 恒 None），gateway 侧独立实例同目录 append。
pub fn build_plugin_audit_logger(
    home: &std::path::Path,
) -> Option<nemesis_plugins_wasm::host_impl::SharedAuditLogger> {
    // 审计文件开关（CFG-06，缺省 true——与 security_setup 同键同语义）。
    let sec_path = common::security_config_path(home);
    let enabled = std::fs::read_to_string(&sec_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("audit_log_file_enabled").and_then(|f| f.as_bool()))
        .unwrap_or(true);
    let audit_dir = nemesis_path::resolve_audit_log_dir_in_workspace(&common::workspace_path(home));
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: audit_dir,
            enabled,
        },
    )
    .ok()?;
    Some(Arc::new(std::sync::Mutex::new(logger)))
}

/// 安装审批晚绑适配器（LateWebSkillsGate 同模板）。
pub struct LateInstallApprover {
    inner: std::sync::OnceLock<Arc<crate::web_approval::WebApprovalManager>>,
    timeout_secs: u64,
}

impl LateInstallApprover {
    /// `timeout_secs`：卡片等待用户裁决的时长（超时自动拒绝）。
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            inner: std::sync::OnceLock::new(),
            timeout_secs,
        }
    }

    /// run_runtime 审批块调用（幂等失败 = 重复 bind，首个真身胜出）。
    pub fn bind(&self, manager: Arc<crate::web_approval::WebApprovalManager>) {
        let _ = self.inner.set(manager);
    }
}

impl InstallApprover for LateInstallApprover {
    fn approve<'a>(
        &'a self,
        review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let Some(manager) = self.inner.get() else {
                // 3.1（2026-09-29 交付审查）：未 bind 是本进程内的恒定状态
                // （security.enabled=false 或非 gateway 运行态），不是瞬时
                // 故障——「请稍后重试」是误导，诚实指出替代路径。
                return Err(
                    "插件审批门未装配（security.enabled=false 或非 gateway 运行态）：运行期安装不可用；可改用 CLI `nemesisbot plugin install --yes`（磁盘形态，重启后生效）"
                        .to_string(),
                );
            };
            // 安装类操作不走「总是允许」规则（2026-09-30 复查 #12）：规则
            // 记的是 source_dir，同一目录明天可以放进不同 sha 的载荷——目录
            // 记忆对「装任意代码」天然不安全，每次安装保持人工裁决。规则
            // 写入侧（respond always）由 rule_permitted_for 的 is_install_op
            // 子句拒绝，历史遗留的 install 规则在此成为死信。
            // 审批卡阻塞等待（spawn_blocking 防占用当前 worker——block_in_place
            // 非多线程 runtime 会 panic，同 skills gate 理由）。
            let manager = manager.clone();
            let timeout_secs = self.timeout_secs;
            let summary = format!(
                "WASM 插件安装：{} v{}（kind={}，信任={}，sha256={}…，出站 {} 项，凭据 {} 项，{} 字节）\n来源：{}",
                review.slug,
                review.version,
                review.kind,
                if review.trust_state.is_empty() {
                    "无签名"
                } else {
                    &review.trust_state
                },
                // sha 前缀 16 hex 给人比对锚点（完整 sha 在 install 漏斗第③步
                // 已与 lockfile/manifest 对账；卡片上给全文反而不可读）。
                &review.wasm_sha256[..16.min(review.wasm_sha256.len())],
                review.egress.len(),
                review.x_secret.len(),
                review.wasm_bytes,
                review.source_dir,
            );
            let target = review.source_dir.clone();
            let wait = tokio::task::spawn_blocking(move || {
                use nemesis_security::auditor::ApprovalManager as _;
                let request_id = format!("plugins-install-{}", uuid::Uuid::new_v4());
                manager.request_approval_sync(
                    &request_id,
                    "plugins.install",
                    &target,
                    "HIGH",
                    &summary,
                    timeout_secs,
                )
            })
            .await;
            match wait {
                Ok(Ok(verdict)) if verdict.approved => Ok(true),
                Ok(Ok(verdict)) => Err(verdict
                    .note
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "用户拒绝了插件安装".to_string())),
                Ok(Err(e)) => Err(format!("审批请求失败: {e}")),
                Err(e) => Err(format!("审批任务失败: {e}")),
            }
        })
    }
}

/// 把已注册插件工具桥接进 agent loop（init_agent / build_agent_loop 装配点）。
///
/// 逐工具：register_tool（`plugin.<slug>.<base>` 全名）+ declare 操作类型
/// （声明式 ABAC——声明后 8 层管线照跑而非「未知名放行」整跳）。返回注册数。
pub fn register_plugin_tools(
    agent_loop: &mut nemesis_agent::r#loop::AgentLoop,
    manager: &Arc<PluginManager>,
) -> usize {
    let mut registered = 0usize;
    for reg in manager.list() {
        // 禁用插件的工具不进供给面（与 WSAPI enable/disable 热同步同语义：
        // agent 重启/loop 重建后不复活，2026-09-29 交付审查 1.1）。
        if !reg.enabled.load(Ordering::SeqCst) {
            continue;
        }
        let Some(meta) = reg.tool_meta.clone() else {
            continue;
        };
        #[cfg(feature = "security")]
        if let Some(op) = PluginToolBridge::map_operation(&meta.operation_type) {
            nemesis_security::types::declare_tool_operation(&meta.name, op);
        }
        agent_loop.register_tool(
            meta.name.clone(),
            Box::new(PluginToolBridge::new(meta, manager.clone())),
        );
        registered += 1;
    }
    registered
}

/// 运行时接线·estop 联动（init_agent 一次性调用）。
///
/// watcher 是 manager 级任务：Manager 挂 SharedResources 跨 agent 重启存活，
/// 本函数只在 gateway 生命周期内 spawn 一次（放 build_agent_loop 会随每次
/// agent 重启泄任务）。急停 engaged → 冻结子系统（execute/observe 全拒），
/// release → 恢复；初始态对齐（启动时已急停的场景）。
pub fn spawn_estop_watcher(manager: &Arc<PluginManager>, estop: &Arc<nemesis_agent::EstopState>) {
    // 先订阅后读初值：订阅先行后，任何 trigger/release 的 send 都有 receiver
    // 记账，不存在错过窗口（读后订阅会永久错过订阅前一刻的急停——2026-09-29
    // 交付审查 1.4；spawn_watcher 同款时序先例）。
    //
    // 但初值必须读 AtomicBool（is_engaged）而非 watch 值：tokio watch::send
    // 零订阅者时直接 Err 且**值不落**（1.52 send 首行 receiver_count()==0 即
    // 返回；EstopState::new 的首个 receiver 构造即弃），「订阅前已 trigger」
    // 的历史态只有 AtomicBool 承载——从 watch 读初值会把已急停读成未急停
    // （estop_watcher_initial_alignment 测试钉的就是这个回归）。订阅已先行，
    // 此刻到 watcher 接手之间的 trigger/release 必落 watch，loop 首个
    // changed() 即收敛，两侧无事件丢失。
    let mut rx = estop.subscribe();
    manager.set_enabled(!estop.is_engaged());
    let mgr = manager.clone();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let engaged = *rx.borrow();
            mgr.set_enabled(!engaged);
            info!(
                engaged,
                "[WasmPlugin] estop 联动：子系统{}",
                if engaged { "冻结" } else { "恢复" }
            );
        }
    });
}

/// 插件运行时配置（config.json `plugins.wasm` 段；raw 读——typed AppConfig
/// 无此段，#[serde(default)] 语义手工对齐）。
#[derive(Debug, Clone)]
pub struct PluginRuntimeConfig {
    /// 子系统总开关（缺省 true）。
    pub enabled: bool,
    /// 全局限制表（只允许收紧 = 与默认取 min）。
    pub limits: PluginLimits,
}

/// 读 `config.json` 的 `plugins.wasm` 段（缺文件/缺段 = 全默认）。
pub fn read_runtime_config(home: &std::path::Path) -> PluginRuntimeConfig {
    let defaults = PluginLimits::default();
    let mut out = PluginRuntimeConfig {
        enabled: true,
        limits: defaults.clone(),
    };
    let Ok(raw) = std::fs::read_to_string(common::config_path(home)) else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    let Some(wasm) = v.get("plugins").and_then(|p| p.get("wasm")) else {
        return out;
    };
    if let Some(b) = wasm.get("enabled").and_then(|x| x.as_bool()) {
        out.enabled = b;
    }
    if let Some(l) = wasm.get("limits").and_then(|x| x.as_object()) {
        let take_u64 = |key: &str| -> Option<u64> { l.get(key).and_then(|x| x.as_u64()) };
        if let Some(n) = take_u64("fuel").filter(|n| *n > 0 && *n < defaults.fuel) {
            out.limits.fuel = n;
        }
        if let Some(n) =
            take_u64("max_memory_bytes").filter(|n| *n > 0 && (*n as usize) < defaults.memory_bytes)
        {
            out.limits.memory_bytes = n as usize;
        }
        if let Some(n) = take_u64("max_table_elements")
            .filter(|n| *n > 0 && (*n as usize) < defaults.table_elements)
        {
            out.limits.table_elements = n as usize;
        }
        if let Some(n) =
            take_u64("max_instances").filter(|n| *n > 0 && (*n as usize) < defaults.max_instances)
        {
            out.limits.max_instances = n as usize;
        }
        if let Some(n) =
            take_u64("call_timeout_secs").filter(|n| *n > 0 && (*n * 1000) < defaults.timeout_ms)
        {
            out.limits.timeout_ms = n * 1000;
        }
        if let Some(n) = take_u64("max_host_calls_per_frame")
            .filter(|n| *n > 0 && (*n as u32) < defaults.host_call_budget)
        {
            out.limits.host_call_budget = n as u32;
        }
        if let Some(n) = take_u64("observer_queue_depth")
            .filter(|n| *n > 0 && (*n as usize) < defaults.observer_queue_depth)
        {
            out.limits.observer_queue_depth = n as usize;
        }
    }
    out
}

/// 装配 PluginManager + Installer（gateway / headless 共用构造单一真相源）。
///
/// 返回 `(manager, installer)`；子系统关（config `plugins.wasm.enabled=false`）
/// 返回 `None`（整面子系统不存在——SharedResources 槽空、工具不注册、
/// WSAPI/CLI 诚实报「未启用」）。
pub fn build_plugin_stack(
    home: &std::path::Path,
    security_plugin: Option<&Arc<nemesis_security::pipeline::SecurityPlugin>>,
    gate: Arc<LateInstallApprover>,
) -> Option<(
    Arc<PluginManager>,
    nemesis_plugins_wasm::install::PluginInstaller,
)> {
    let rc = read_runtime_config(home);
    if !rc.enabled {
        info!("[WasmPlugin] 子系统已关闭（plugins.wasm.enabled=false）");
        return None;
    }
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(home),
            rc.limits,
            Arc::new(VaultPluginSecrets::for_home(home)),
        )
        .map_err(|e| {
            tracing::warn!(error = %e, "[WasmPlugin] PluginManager 构建失败，子系统停用");
            e
        })
        .ok()?,
    );
    if let Some(audit) = build_plugin_audit_logger(home) {
        manager.set_audit(audit);
    }
    // 病毒扫描链复用 SecurityPlugin 的同一实例（第 7 层同源——装了什么
    // 引擎就扫什么）；无 security 真身 = None（第④步诚实跳过并注记）。
    let scanner = security_plugin.map(|p| p.scan_chain());
    let installer =
        nemesis_plugins_wasm::install::PluginInstaller::new(manager.clone(), gate, scanner);
    Some((manager, installer))
}

#[cfg(test)]
mod tests;
