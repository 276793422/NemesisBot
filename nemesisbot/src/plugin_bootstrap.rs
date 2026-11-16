//! 全自动流转 / agent 自举插件（项6）：`plugin_search` + `plugin_propose`
//! 两工具、SelfBootstrapApprover（auto_approve 资格裁决）、源清单与配额。
//!
//! 产品语义「agent 撞到能力缺口 → 自己找插件 → 提案 → 人批 → 热装立即可用」：
//! - `plugin_search`：并发扫描 `config/plugin_bootstrap/sources.json` 声明的本地插件源
//!   （只读发现，manifest 子串匹配），行带 source/permissions 摘要；
//! - `plugin_propose {slug, reason}`：按 slug 从源定位插件目录 → 沿用既有
//!   installer 九步装配漏斗（校验/验签/hash/扫描/审批/编译对账/lockfile/落位/
//!   注册，一概不裁剪）→ 审批卡带 agent 论证（`InstallReview.proposer_reason`）
//!   → 人批（或 auto_approve 资格命中自动批）→ `enable_with_face_sync` 热装
//!   → **同一会话立即可用**（ToolSync 落当前 AgentLoop）。
//!
//! 策略面（`config/plugin_bootstrap/sources.json` 的 `self_bootstrap` 段）：
//! `enabled`（总闸，缺省 false）/`allowed_sources`（来源白名单，空 = 全部
//! 已启用源）/`auto_approve`（资格裁决，见 [`SelfBootstrapApprover`]）/
//! `max_installs_per_day`（每日保险丝，0 = 不限）。
//!
//! 红线：审批卡**永不静默跳过**——提案沿用 `plugins.install` 审批 op 标签
//!（`approval_rules.rs::is_install_op`），“总是允许”规则写入侧对安装面
//! 恒拒绝（装任意代码的记忆不安全），卡面必然出现或提案必然失败，无第三态。
//! auto_approve 也只在资格判据全命中时短路，其余照常转卡。
//!
//! estop：工具 execute 开头拒绝 + 漏斗返回后、enable 前重查（卡挂起期间
//! 急停 → 载荷已落盘但不装载，诚实报 aborted_estop）。
//!
//! 审计：proposed / auto_approved / installed / rejected / aborted_estop
//! 五种决策全走 `SecurityPlugin::append_direct_audit_event` 直记审计链
//!（非工具管线动作不经八层 execute，无直记 = 台账缺失）。
//!
//! 注册面：仅主 agent（`agent_factory.rs` 8f 块；`plugin_installer` /
//! `plugin_install_gate` 未装配 = 不注册，零影响）。供给面不进
//! `tier_allowed_tools`，`min_tier() = Some("big")` 双闸（供给 + 分发）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_plugins_wasm::install::{InstallApprover, InstallReview, PluginInstaller};

// ---------------------------------------------------------------------------
// 配置面（workspace/config/plugin_bootstrap/sources.json）
// ---------------------------------------------------------------------------

/// 源清单 + 自举策略配置文件名（`<workspace>/config/plugin_bootstrap/` 下）。
pub const SOURCES_FILE_NAME: &str = "sources.json";
/// 每日安装配额状态文件名（同目录；UTC 日界翻新）。
pub const QUOTA_FILE_NAME: &str = "bootstrap_state.json";

fn default_kind() -> String {
    "local".to_string()
}
fn default_true() -> bool {
    true
}
fn default_max_installs_per_day() -> u32 {
    3
}

/// 单个插件源（当前仅 `kind = "local"` 本地目录形态：每个子目录一个插件，
/// 含 plugin.toml + 载荷）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginSourceConfig {
    /// 源名（`allowed_sources` 白名单与搜索结果的 `source` 字段引用）。
    pub name: String,
    /// 源类型；当前仅 `"local"`。
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 本地目录绝对路径。
    pub path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 自举策略段。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SelfBootstrapConfig {
    /// 总闸。false（缺省）= plugin_propose 诚实拒绝，plugin_search 仍可发现。
    #[serde(default)]
    pub enabled: bool,
    /// 来源白名单（source.name 集合）。空 = 全部已启用源可用。
    #[serde(default)]
    pub allowed_sources: Vec<String>,
    /// 自动批准（资格判据见 [`SelfBootstrapApprover::eligible`]）；
    /// false（缺省）= 提案一律转人工审批卡。
    #[serde(default)]
    pub auto_approve: bool,
    /// 每日自举安装保险丝（UTC 日界；0 = 不限）。缺省 3。
    #[serde(default = "default_max_installs_per_day")]
    pub max_installs_per_day: u32,
}

// 手写 Default（不用 derive）：整段 `self_bootstrap` 缺席时 serde 用本
// impl 兜底——derive 版会把保险丝置 0（= 不限），与字段级 serde 缺省
//（3）分裂成两种语义。保险丝缺省单一真相源 = default_max_installs_per_day。
impl Default for SelfBootstrapConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_sources: Vec::new(),
            auto_approve: false,
            max_installs_per_day: default_max_installs_per_day(),
        }
    }
}

/// `sources.json` 全文形态。
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct BootstrapSourcesFile {
    #[serde(default)]
    pub sources: Vec<PluginSourceConfig>,
    #[serde(default)]
    pub self_bootstrap: SelfBootstrapConfig,
}

/// `<workspace>/config/plugin_bootstrap/` 目录。
///
/// **刻意不用 `config/plugins/`**：那是 WASM 插件旧实例文件的迁移目录
///（`nemesis_config::plugin_wasm::LEGACY_INSTANCE_DIR`）——惰性迁移会把
/// 该目录下任何 `*.json` 当旧实例并入 `config.plugin.wasm.json` 后**删除
/// 原文件**，bootstrap 的 sources.json / bootstrap_state.json 落进去会被
/// 吞（e2e 实录：sources.json 被迁移删除 + 幻影 "sources" 插件条目）。
pub fn plugins_config_dir(workspace: &Path) -> PathBuf {
    workspace.join("config").join("plugin_bootstrap")
}
/// `sources.json` 全路径。
pub fn sources_file_path(workspace: &Path) -> PathBuf {
    plugins_config_dir(workspace).join(SOURCES_FILE_NAME)
}
/// `bootstrap_state.json` 全路径。
pub fn quota_file_path(workspace: &Path) -> PathBuf {
    plugins_config_dir(workspace).join(QUOTA_FILE_NAME)
}

/// 加载源清单。文件缺席 = 诚实 setup 错误（内联 JSON 形态说明），
/// 不生成模板——自举是 opt-in 能力，静默建文件会制造「以为配好了」的假象。
pub fn load_sources_file(workspace: &Path) -> Result<BootstrapSourcesFile, String> {
    let p = sources_file_path(workspace);
    let raw = std::fs::read_to_string(&p).map_err(|_| {
        format!(
            "自举源清单不存在：{}。请先创建，JSON 形态示例：\
             {{\"sources\":[{{\"name\":\"local\",\"kind\":\"local\",\
             \"path\":\"<插件库目录>\",\"enabled\":true}}],\
             \"self_bootstrap\":{{\"enabled\":true,\"allowed_sources\":[],\
             \"auto_approve\":false,\"max_installs_per_day\":3}}}}",
            p.display()
        )
    })?;
    serde_json::from_str(&raw).map_err(|e| format!("源清单解析失败（{}）：{e}", p.display()))
}

/// 今日 UTC 日期（配额日界）。
fn utc_today() -> String {
    chrono::Utc::now().date_naive().to_string()
}

/// 配额状态（跨重启持久化；损坏 = 视作新的一天，保险丝从紧方向宁可少记）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BootstrapQuotaState {
    /// UTC 日期（yyyy-mm-dd）。
    pub date: String,
    /// 当日已自举安装次数。
    pub installs_today: u32,
}

impl Default for BootstrapQuotaState {
    fn default() -> Self {
        Self {
            date: utc_today(),
            installs_today: 0,
        }
    }
}

/// 读配额状态。缺席/损坏/日期过期（非今日）= 全量 0（新的一天）。
pub fn load_quota_state(workspace: &Path) -> BootstrapQuotaState {
    let Ok(raw) = std::fs::read_to_string(quota_file_path(workspace)) else {
        return BootstrapQuotaState::default();
    };
    match serde_json::from_str::<BootstrapQuotaState>(&raw) {
        Ok(st) if st.date == utc_today() => st,
        // 日期过期或解析失败：归零（过期是正常日界，损坏按从紧方向归零）。
        _ => BootstrapQuotaState::default(),
    }
}

/// 配额状态落盘（原子写：tmp + rename；目录缺席自动建）。
pub fn save_quota_state(workspace: &Path, state: &BootstrapQuotaState) -> Result<(), String> {
    let dir = plugins_config_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let path = quota_file_path(workspace);
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, raw).map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {} 失败：{e}", path.display()))?;
    Ok(())
}

/// 计数 +1 并落盘（成功安装后调用）。
pub fn bump_quota(workspace: &Path) {
    let mut st = load_quota_state(workspace);
    st.installs_today += 1;
    if let Err(e) = save_quota_state(workspace, &st) {
        // 记账失败不回滚安装（安装已成功）；tracing warn 兜底可见性。
        tracing::warn!("[PluginBootstrap] 配额记账失败（保险丝可能少计）：{e}");
    }
}

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

/// 搜索结果行。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginSearchRow {
    pub slug: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub description: String,
    /// 来源源名。
    pub source: String,
    /// 插件目录（plugin_propose 的定位依据）。
    pub path: String,
    /// 出站域名数（0 = 无出站能力）。
    pub egress_domains: usize,
    /// 秘密声明数。
    pub secrets: usize,
    /// 接管声明（内置工具名清单；空 = 无接管）。
    pub takeover: Vec<String>,
}

/// 搜索汇总（行 + 每源错误注记——破损源不静默消失）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchOutcome {
    pub query: String,
    pub results: Vec<PluginSearchRow>,
    pub source_errors: Vec<String>,
}

/// 子串匹配（大小写不敏感；query 空白 = 列出全部）。
fn query_matches(query: &str, row_slug: &str, row_name: &str, row_desc: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    row_slug.to_lowercase().contains(&q)
        || row_name.to_lowercase().contains(&q)
        || row_desc.to_lowercase().contains(&q)
}

/// 扫描单源：子目录按字典序遍历（read_dir 顺序是平台实现细节，必须显式
/// 排序——NTFS/ext4 命名序不一致），含 plugin.toml 的子目录加载 manifest
/// 参与匹配；破损 manifest 跳过（搜索是发现面，不是裁决面）。
fn scan_source(
    source: &PluginSourceConfig,
    query: &str,
    limit: usize,
) -> (Vec<PluginSearchRow>, Option<String>) {
    // kind 消费点（诚实闸）：声明了不支持的源类型 = 明确报错跳过，
    // 不静默当 local 处理（当前仅 local 目录形态）。
    if source.kind != "local" {
        return (
            Vec::new(),
            Some(format!(
                "源「{}」类型不支持（kind=\"{}\"，当前仅 \"local\"）——已跳过",
                source.name, source.kind
            )),
        );
    }
    let dir = PathBuf::from(&source.path);
    let mut rows = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            return (
                rows,
                Some(format!(
                    "源「{}」目录不可读（{}）：{e}",
                    source.name,
                    dir.display()
                )),
            );
        }
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    for name in names {
        if rows.len() >= limit {
            break;
        }
        let pdir = dir.join(&name);
        if !pdir.join("plugin.toml").is_file() {
            continue;
        }
        let Ok((manifest, _raw)) =
            nemesis_plugins_wasm::manifest::PluginManifest::load_from_dir(&pdir)
        else {
            continue;
        };
        if !query_matches(query, &manifest.slug, &manifest.name, &manifest.description) {
            continue;
        }
        rows.push(PluginSearchRow {
            slug: manifest.slug.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            kind: manifest.kind.as_str().to_string(),
            description: manifest.description.clone(),
            source: source.name.clone(),
            path: pdir.display().to_string(),
            egress_domains: manifest.permissions.egress.len(),
            secrets: manifest.permissions.x_secret.len(),
            takeover: manifest.takeover.clone(),
        });
    }
    (rows, None)
}

/// 并发扫描全部已启用源并合并结果（join_all；单源失败 = 错误注记不中断）。
pub async fn search_sources(
    sources: &[PluginSourceConfig],
    query: &str,
    limit: usize,
) -> SearchOutcome {
    let enabled: Vec<&PluginSourceConfig> = sources.iter().filter(|s| s.enabled).collect();
    let scans = futures::future::join_all(
        enabled
            .iter()
            .map(|s| {
                let query = query.to_string();
                async move { scan_source(s, &query, limit) }
            })
            .collect::<Vec<_>>(),
    )
    .await;
    let mut results = Vec::new();
    let mut source_errors = Vec::new();
    for (rows, err) in scans {
        results.extend(rows);
        if let Some(e) = err {
            source_errors.push(e);
        }
    }
    results.sort_by(|a, b| a.slug.cmp(&b.slug));
    SearchOutcome {
        query: query.to_string(),
        results,
        source_errors,
    }
}

// ---------------------------------------------------------------------------
// 审计
// ---------------------------------------------------------------------------

/// 直记审计链的统一入口（None 安全面 = 链未启用 → tracing warn 兜底）。
#[cfg(feature = "security")]
type AuditSink = Option<Arc<nemesis_security::pipeline::SecurityPlugin>>;
#[cfg(not(feature = "security"))]
/// 非 security 组合（plugins-wasm 可独立开启）：审计槽恒空，tracing 兜底。
type AuditSink = Option<()>;

fn audit_bootstrap(sink: &AuditSink, decision: &str, target: &str, reason: &str, source: &str) {
    let operation = "plugin_bootstrap";
    #[cfg(feature = "security")]
    if let Some(sp) = sink {
        if sp
            .append_direct_audit_event(
                operation,
                "plugin_propose",
                "agent",
                source,
                target,
                decision,
                reason,
            )
            .is_some()
        {
            return;
        }
    }
    tracing::warn!(
        "[PluginBootstrap] 审计链未启用——自举决策仅落日志：decision={decision} target={target} reason={reason}"
    );
}

// ---------------------------------------------------------------------------
// SelfBootstrapApprover（auto_approve 资格裁决审批面）
// ---------------------------------------------------------------------------

/// auto_approve 资格判据（对「纯只读」的保守近似——operation_type 是审批后
/// step ⑥ guest 探针才产出的事实，审批面看不到，只能用 review 已有字段收紧）：
/// 信任四态 = trusted 且信任级 = verified、kind = tool、无出站、无秘密声明、
/// 无接管、limits 无放宽。任一不命中 = 回落人工审批卡（永不静默跳过）。
/// 已知边界（诚实记录）：「零声明插件」的 host tool-invoke 调用面仍逐次过
/// 安全 8 层管线——安装面自动批准不构成调用面旁路。
pub fn auto_approve_eligible(review: &InstallReview) -> bool {
    review.trust_state == nemesis_plugins_wasm::PluginTrustState::Trusted.as_str()
        && review.trust_level == "verified"
        && review.kind == "tool"
        && review.egress.is_empty()
        && review.x_secret.is_empty()
        && review.takeover.is_empty()
        && review.limits_ignored.is_empty()
}

/// 自举审批面：包住既有审批门（gateway 形态 = LateInstallApprover），
/// auto_approve 开启且资格命中时短路放行 + 计审计，否则原样委托人工卡。
/// 工具经 `install_with` 传入本包装器 → **Dashboard 手动安装走 installer
/// 默认审批面，永不受 agent 政策影响**。
pub struct SelfBootstrapApprover {
    inner: Arc<dyn InstallApprover>,
    workspace: PathBuf,
    policy: SelfBootstrapConfig,
    audit: AuditSink,
    source_name: String,
    /// 裁决回传槽（Some(true)=自动批准 / Some(false)=转人工且人工裁决在
    /// inner；None=尚未裁决）。工具在 install_with 返回后读取，把真实
    /// auto_approved 事实写进结果与审计——不猜。
    decision_slot: Arc<parking_lot::Mutex<Option<bool>>>,
}

impl SelfBootstrapApprover {
    pub fn new(
        inner: Arc<dyn InstallApprover>,
        workspace: PathBuf,
        policy: SelfBootstrapConfig,
        audit: AuditSink,
        source_name: String,
        decision_slot: Arc<parking_lot::Mutex<Option<bool>>>,
    ) -> Self {
        Self {
            inner,
            workspace,
            policy,
            audit,
            source_name,
            decision_slot,
        }
    }
}

impl InstallApprover for SelfBootstrapApprover {
    fn approve<'a>(
        &'a self,
        review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            // 保险丝：人工卡路径也吃当日配额（提案面统一预算，人工批准
            // 不豁免——配额是「自举面预算」不是「自动批准预算」）。
            let quota = load_quota_state(&self.workspace);
            if self.policy.max_installs_per_day > 0
                && quota.installs_today >= self.policy.max_installs_per_day
            {
                // 配额耗尽：不转卡（卡也会被拒），直接结构化拒绝。
                *self.decision_slot.lock() = Some(false);
                return Ok(false);
            }
            if self.policy.auto_approve && auto_approve_eligible(review) {
                *self.decision_slot.lock() = Some(true);
                audit_bootstrap(
                    &self.audit,
                    "auto_approved",
                    &review.slug,
                    &format!(
                        "资格命中：trusted/verified + tool + 零出站 + 零秘密 + 零接管；\
                         论证：{}",
                        review.proposer_reason.as_deref().unwrap_or("")
                    ),
                    &self.source_name,
                );
                tracing::info!(
                    "[PluginBootstrap] auto_approve 命中（{}）——自动批准",
                    review.slug
                );
                return Ok(true);
            }
            *self.decision_slot.lock() = Some(false);
            self.inner.approve(review).await
        })
    }

    /// 委托内层面（gateway 审批卡具备人批豁免；CLI/headless 无）。
    fn can_approve_unsigned(&self) -> bool {
        self.inner.can_approve_unsigned()
    }
}

// ---------------------------------------------------------------------------
// 工具 1：plugin_search
// ---------------------------------------------------------------------------

/// 工具注册名。
pub const SEARCH_TOOL_NAME: &str = "plugin_search";
/// 工具注册名。
pub const PROPOSE_TOOL_NAME: &str = "plugin_propose";

/// 只读插件源检索工具。
pub struct PluginSearchTool {
    workspace: PathBuf,
}

impl PluginSearchTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for PluginSearchTool {
    fn description(&self) -> String {
        "Search locally configured plugin sources for installable WASM plugins. \
         Use this when you hit a capability gap that a plugin could fill \
         (e.g. \"if only I had a tool to ...\"), then propose the best match \
         with `plugin_propose`. Substring-matches slug/name/description; \
         empty query lists everything. Returns rows with slug, version, kind, \
         description, source, permission summary (egress/secrets/takeover)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Substring to match against slug/name/description (case-insensitive). Empty string lists all plugins."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max rows per source (default 10)."
                }
            }
        })
    }

    /// big 档双闸（供给 + 分发；自举是高权限编排面，小模型不供给）。
    fn min_tier(&self) -> Option<&str> {
        Some("big")
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 不是合法 JSON：{e}"))?;
        let query = v
            .get("query")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let limit = v.get("limit").and_then(|x| x.as_u64()).unwrap_or(10) as usize;
        let file = load_sources_file(&self.workspace)?;
        if file.sources.is_empty() {
            return Err(format!(
                "源清单为空（{}）：未声明任何插件源，无从检索。",
                sources_file_path(&self.workspace).display()
            ));
        }
        let outcome = search_sources(&file.sources, &query, limit.max(1)).await;
        // search 是只读发现面：self_bootstrap 未启用时照常返回，附注记
        // （propose 才被总闸拦）。
        let bootstrap_note = if !file.self_bootstrap.enabled {
            "（注意：self_bootstrap.enabled=false——当前只能检索，提案安装不可用）"
        } else {
            ""
        };
        let payload = serde_json::json!({
            "query": outcome.query,
            "sources_scanned": file.sources.iter().filter(|s| s.enabled).count(),
            "source_errors": outcome.source_errors,
            "bootstrap_note": bootstrap_note,
            "results": outcome.results,
        });
        Ok(serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?)
    }
}

// ---------------------------------------------------------------------------
// 工具 2：plugin_propose
// ---------------------------------------------------------------------------

/// 插件自举提案工具：定位 → 校验 → 九步漏斗（审批卡带论证）→ 热装。
pub struct PluginProposeTool {
    workspace: PathBuf,
    installer: Arc<PluginInstaller>,
    /// 既有审批门（gateway = LateInstallApprover 审批卡）；auto_approve
    /// 关闭或资格不命中时直接作为审批面传入漏斗。
    gate: Arc<crate::plugin_bridge::LateInstallApprover>,
    estop: Arc<nemesis_agent::estop::EstopState>,
    audit: AuditSink,
}

impl PluginProposeTool {
    pub fn new(
        workspace: PathBuf,
        installer: Arc<PluginInstaller>,
        gate: Arc<crate::plugin_bridge::LateInstallApprover>,
        estop: Arc<nemesis_agent::estop::EstopState>,
        audit: AuditSink,
    ) -> Self {
        Self {
            workspace,
            installer,
            gate,
            estop,
            audit,
        }
    }

    /// 按 slug 定位插件目录：先全部已启用源里找（拿到真实来源名），再做
    /// 白名单裁决——这样「白名单外命中」与「哪儿都没有」两种失败可区分，
    /// 拒绝原因结构化可自纠。
    fn locate(
        sources: &[PluginSourceConfig],
        allowed: &[String],
        slug: &str,
    ) -> Result<(PathBuf, String), String> {
        let mut found: Option<(PathBuf, String)> = None;
        let mut scanned = Vec::new();
        let mut skipped_kinds = 0usize;
        for s in sources.iter().filter(|s| s.enabled) {
            // 非 local 源不参与提案定位（与搜索面同口径诚实跳过）。
            if s.kind != "local" {
                skipped_kinds += 1;
                continue;
            }
            scanned.push(s.name.clone());
            let pdir = PathBuf::from(&s.path).join(slug);
            if pdir.join("plugin.toml").is_file() {
                found = Some((pdir, s.name.clone()));
                break;
            }
        }
        let (pdir, source_name) = found.ok_or_else(|| {
            let skipped_note = if skipped_kinds > 0 {
                format!("；另跳过 {skipped_kinds} 个不支持类型的源")
            } else {
                String::new()
            };
            format!(
                "插件 {slug} 在已启用源中未找到（扫描：{scanned:?}{skipped_note}）。\
                 可用 plugin_search 先检索确认 slug。"
            )
        })?;
        // 白名单裁决：非空 = 必须命中（bootstrap.enabled 才是总闸）。
        if !allowed.is_empty() && !allowed.iter().any(|a| a == &source_name) {
            return Err(format!(
                "来源「{source_name}」不在 self_bootstrap.allowed_sources 白名单内\
                 （允许：{allowed:?}）——提案拒绝。请人工经 Dashboard 插件页安装，\
                 或请管理员把该源加入白名单。"
            ));
        }
        Ok((pdir, source_name))
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for PluginProposeTool {
    fn description(&self) -> String {
        "Propose installing a WASM plugin from a configured source. State your \
         reason: what capability gap you hit and why this plugin fills it — \
         the reason is shown to the user on the approval card. The plugin goes \
         through the full install funnel (verification, signature check, virus \
         scan, approval); after approval it is hot-loaded and usable in this \
         conversation immediately. Requires plugins/sources.json with \
         self_bootstrap.enabled=true."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "slug": {
                    "type": "string",
                    "description": "Plugin slug (directory name in the source). Get it from plugin_search."
                },
                "reason": {
                    "type": "string",
                    "description": "Why you need this plugin: the capability gap you hit and how this plugin fills it. Shown on the approval card."
                }
            },
            "required": ["slug", "reason"]
        })
    }

    /// big 档双闸（同 plugin_search）。
    fn min_tier(&self) -> Option<&str> {
        Some("big")
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 不是合法 JSON：{e}"))?;
        let slug = v
            .get("slug")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let reason = v
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if slug.is_empty() {
            return Err("slug 必填（来源内插件目录名，用 plugin_search 检索）".to_string());
        }
        if reason.is_empty() {
            return Err("reason 必填——审批卡要展示你的论证（撞到什么能力缺口、\
                        该插件如何补上），空提案一律拒绝"
                .to_string());
        }

        // estop 冻结期提案起不来（dispatch 层已闸工具调用，这里是工具级兜底）。
        if self.estop.is_engaged() {
            return Err("急停（estop）已触发——插件提案冻结中".to_string());
        }

        let file = load_sources_file(&self.workspace)?;
        if !file.self_bootstrap.enabled {
            return Err(format!(
                "自举未启用（{} 的 self_bootstrap.enabled=false）——提案安装不可用。\
                 请提示用户在 Dashboard 配置或直接人工安装插件。",
                sources_file_path(&self.workspace).display()
            ));
        }

        // 定位 + 白名单（结构化拒绝，可自纠）。
        let (pdir, source_name) =
            Self::locate(&file.sources, &file.self_bootstrap.allowed_sources, &slug)?;

        // 已装拒绝（注册表在册或 lockfile 有记录——停用也算已装）；
        // 升级走 Dashboard 插件页 / plugin upgrade，提案面不做升级。
        if self.installer.is_installed(&slug) {
            return Err(format!(
                "插件 {slug} 已安装——提案面不做升级。如需升级请提示用户走 \
                 Dashboard 插件页或 CLI plugin upgrade。"
            ));
        }

        // 配额预检（fail fast；审批面内还有权威复查）。
        let quota = load_quota_state(&self.workspace);
        if file.self_bootstrap.max_installs_per_day > 0
            && quota.installs_today >= file.self_bootstrap.max_installs_per_day
        {
            return Err(format!(
                "今日自举安装已达保险丝上限 {}（max_installs_per_day）——\
                 明日重试，或提示用户调大配额 / 直接人工安装（人工安装不走此额度）。",
                file.self_bootstrap.max_installs_per_day
            ));
        }

        // 记账：提案动作本身（含论证）进审计链。
        audit_bootstrap(&self.audit, "proposed", &slug, &reason, &source_name);

        // 审批面：auto_approve 开 = 包 SelfBootstrapApprover（资格裁决 +
        // 兜底委托人工卡）；关 = 直接人工卡。红线：卡面永不静默跳过
        //（插件沿用 plugins.install op 标签，“总是允许”写入侧恒拒绝）。
        // decision_slot = 审批面真实裁决回传（结果与审计不猜）。
        let decision_slot = Arc::new(parking_lot::Mutex::new(None));
        let approver: Arc<dyn InstallApprover> = if file.self_bootstrap.auto_approve {
            Arc::new(SelfBootstrapApprover::new(
                self.gate.clone(),
                self.workspace.clone(),
                file.self_bootstrap.clone(),
                self.audit.clone(),
                source_name.clone(),
                decision_slot.clone(),
            ))
        } else {
            *decision_slot.lock() = Some(false);
            self.gate.clone()
        };

        // 九步漏斗（校验/验签/hash/扫描/审批/编译对账/lockfile/落位/注册）
        // 一概不裁剪；proposer_reason 进审批卡「提案论证」段。
        //
        // allow_unsigned 容差与审批面能力绑定（单一真相源 = 卡面能力，与
        // Dashboard 插件页「审批卡裁决」语义同源）：gateway 形态
        //（LateInstallApprover）具备人批卡 → 无签名载荷可走到卡面，人批 =
        // 显式接受（install --allow-unsigned 同一留档语义）；headless/CLI
        // 内核形态无卡能力 → strict 拒绝。auto_approve 永不越此线——资格
        // 判据要求 trusted/verified，无签名（ReviewRequired）必然转人工卡。
        let allow_unsigned = self.gate.can_approve_unsigned();
        let outcome = self
            .installer
            .install_with(&pdir, allow_unsigned, approver, Some(reason.clone()))
            .await;

        let outcome = match outcome {
            Ok(o) => o,
            Err(nemesis_plugins_wasm::PluginError::Rejected(r)) => {
                audit_bootstrap(&self.audit, "rejected", &slug, &r, &source_name);
                return Err(format!(
                    "提案被否决（审批拒绝：{r}）。不要重试同一提案；\
                     可向用户说明缺口，由用户决定是否手动安装。"
                ));
            }
            Err(e) => {
                audit_bootstrap(
                    &self.audit,
                    "rejected",
                    &slug,
                    &format!("漏斗失败：{e}"),
                    &source_name,
                );
                return Err(format!("插件装配漏斗失败：{e}"));
            }
        };

        // 漏斗返回后再查 estop（卡挂起期间急停）：载荷已落盘（lockfile
        // 已记录、开关 = false）但不装载启用 = 惰性；release 后用户可在
        // 插件页手动启用。诚实报 aborted_estop，不假成功。
        if self.estop.is_engaged() {
            audit_bootstrap(
                &self.audit,
                "aborted_estop",
                &slug,
                "审批通过后、装载前急停触发——载荷已落盘未装载（惰性）",
                &source_name,
            );
            return Err(format!(
                "急停（estop）在审批后触发——插件 {slug} 载荷已落盘但未装载（惰性）。\
                 release 后可在 Dashboard 插件页手动启用。"
            ));
        }

        // 热装 + 双面同步（ToolSync 落当前 AgentLoop = 同一会话立即可用；
        // PolicyFace 同步策略组件）。enable 臂单一真相源 = WSAPI 同款。
        let loaded =
            nemesis_web::handlers::plugins_wasm::enable_with_face_sync(&self.installer, &slug)
                .await?;

        // 配额记账（成功安装才计数；auto_approve 路径在审批面只复查不计数）。
        bump_quota(&self.workspace);

        // 真实 auto 与否：审批面裁决槽回传（None = auto_approve 关闭的
        // 直通人工卡路径——恒 false）。
        let auto_approved = (*decision_slot.lock()).unwrap_or(false);
        audit_bootstrap(
            &self.audit,
            "installed",
            &slug,
            &format!(
                "version={} loaded={auto_loaded} auto_approved={auto_approved} 论证：{}",
                outcome.version,
                reason,
                auto_loaded = loaded.is_some()
            ),
            &source_name,
        );

        let payload = serde_json::json!({
            "status": if loaded.is_some() { "installed_and_loaded" } else { "installed_not_loaded" },
            "slug": outcome.slug,
            "version": outcome.version,
            "trust_state": outcome.trust.as_str(),
            "auto_approved": auto_approved,
            "source": source_name,
            "loaded": loaded.is_some(),
            "note": if loaded.is_some() {
                "插件已热装载，工具立即可用（工具名前缀 plugin.<slug>.*）。"
            } else {
                "插件已安装但未装载（load 失败已降级）——详情见 gateway 日志。"
            }
        });
        Ok(serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?)
    }
}
