//! per-call 能力授予上下文（[`CallCaps`]）+ 宿主策略核心的共享构件。
//!
//! 语义要点（策略执行体在 `callbacks.rs` 的 extern "C" shim 里）：
//! - **帧预算**：每次 execute / observe 是一帧，宿主调用计数超限返回
//!   `budget-exceeded`（防 guest 循环刷宿主）。
//! - **observe 帧收窄**：tool-invoke / secret-get 恒 `unavailable`（防递归
//!   放大、防事件路径取密）；egress / workspace-read 照 manifest 授予。
//! - **日志背压**：环形缓冲满丢最旧并计数，永不反压 guest。
//! - **secret 永不出宿主进程**：经 [`SecretResolver`] 按别名取值，组件只在
//!   返回值里见到原文。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::PluginError;
use crate::limits::PluginLimits;
use crate::manifest::PluginKind;

/// 宿主日志单条（Dashboard 插件日志页数据源）。
#[derive(Debug, Clone)]
pub struct PluginLogLine {
    /// Unix 毫秒。
    pub ts_ms: u64,
    /// 级别字符串（trace/debug/info/warn/error）。
    pub level: String,
    /// 正文（超长已截断）。
    pub message: String,
}

/// 宿主日志环形缓冲（per-plugin，注册表持有；满丢最旧+计数）。
#[derive(Debug)]
pub struct PluginLogBuffer {
    inner: Mutex<LogRing>,
}

#[derive(Debug)]
struct LogRing {
    lines: VecDeque<PluginLogLine>,
    capacity: usize,
    dropped: u64,
}

impl PluginLogBuffer {
    #[must_use]
    pub fn new(capacity: usize, _line_max: usize) -> Self {
        // line_max 的截断在 CallCaps::log_line（写入口）执行；这里只管环形
        // 容量与丢弃计数。参数保留占位以对齐 limits 语义。
        Self {
            inner: Mutex::new(LogRing {
                lines: VecDeque::new(),
                capacity,
                dropped: 0,
            }),
        }
    }

    fn push(&self, line: PluginLogLine) {
        let mut ring = match self.inner.lock() {
            Ok(r) => r,
            Err(p) => p.into_inner(),
        };
        if ring.lines.len() >= ring.capacity {
            ring.lines.pop_front();
            ring.dropped += 1;
        }
        ring.lines.push_back(line);
    }

    /// 快照（Dashboard 拉取）。
    #[must_use]
    pub fn snapshot(&self) -> (Vec<PluginLogLine>, u64) {
        let ring = match self.inner.lock() {
            Ok(r) => r,
            Err(p) => p.into_inner(),
        };
        (ring.lines.iter().cloned().collect(), ring.dropped)
    }
}

/// 共享审计器（gateway 注入；`None` = 审计链未装配，静默跳过——headless
/// 兼容。审计接在宿主能力面的敏感通道上：secret-get / http-send /
/// workspace-read 三个入口的 allowed/denied 都落安全审计文件）。
pub type SharedAuditLogger = Arc<Mutex<nemesis_security::audit_log::AuditLogger>>;

/// 凭据解析面（gateway 注入；实现 = VaultStore 按 `vault:plugin/<slug>/<name>`
/// 别名取值）。`None` = 别名不存在。
pub trait SecretResolver: Send + Sync {
    /// 按别名解析凭据原文。
    fn resolve(&self, alias: &str) -> Option<String>;
}

/// 宿主工具调用面（gateway 注入；实现 = Weak<AgentLoop> dispatch 入口 +
/// 深度 1 闸：`plugin.` 前缀目标拒绝）。异步——dispatch 走安全 8 层。
pub trait HostToolInvoker: Send + Sync {
    /// 调用一个宿主工具，返回结果文本（Err = 工具失败文本，转
    /// `HostError::Host` 语义由调用方归约）。
    fn invoke(
        &self,
        tool: &str,
        args_json: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>;
}

/// 杂务 LLM 补全面（gateway 注入；2026-10-22 样板工程批 4）。实现 =
/// Weak<AgentLoop> 委托 `chore_completion`（bypass_llm 护栏同源 + estop
/// fail-closed + small_model 杂务槽）。policy 帧纪律：只有策略帧可用
///（observer 帧恒 unavailable——同 tool-invoke 收窄理由：观察路径不授
/// LLM 花销能力）。
pub trait LlmInvoker: Send + Sync {
    /// 杂务补全一次（max_tokens 已由调用方钳上限；Err = 失败文本）。
    fn complete(
        &self,
        prompt: &str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>;
}

/// 实例配置（manifest config-schema 声明 + 操作员 entries；x-secret 键名集）。
#[derive(Debug, Clone, Default)]
pub struct InstanceConfig {
    /// 已声明的普通键（schema properties 键集）。
    pub declared_keys: Vec<String>,
    /// x-secret 声明（凭据名 → vault 别名后缀）。
    pub x_secret_keys: Vec<String>,
    /// 操作员设置的普通键值。
    pub entries: std::collections::BTreeMap<String, String>,
}

impl InstanceConfig {
    /// 从 schema JSON 抽取属性键（顶层 properties）。
    pub fn with_schema_json(mut self, schema_json: Option<&str>) -> Self {
        if let Some(json) = schema_json
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(json)
            && let Some(props) = v.get("properties").and_then(|p| p.as_object())
        {
            self.declared_keys = props.keys().cloned().collect();
        }
        self
    }
}

/// 单帧预算（宿主调用计数；原子——Store data 需 Send）。
#[derive(Debug, Default)]
pub struct FrameBudget {
    remaining: AtomicU32,
}

impl FrameBudget {
    pub(crate) fn new(limit: u32) -> Self {
        Self {
            remaining: AtomicU32::new(limit),
        }
    }

    /// 消耗一次调用额度；耗尽返回 Err（budget-exceeded 文本）。
    pub(crate) fn consume(&self) -> Result<(), String> {
        // fetch_sub 饱和语义：0 时不再下探。
        let prev = self
            .remaining
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                if n == 0 { None } else { Some(n - 1) }
            });
        match prev {
            Ok(_) => Ok(()),
            Err(_) => Err("host call budget exhausted".to_string()),
        }
    }
}

/// Unix 毫秒（回调时钟）。
pub(crate) fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 一次调用帧的能力授予（构建于 runtime 的 fresh-store 组装期；经
/// `Box<CallCaps>` 裸指针作为 env 透传进壳，回调首参原样归还）。
pub struct CallCaps {
    /// 插件 slug。
    pub slug: String,
    /// 插件 kind。
    pub kind: PluginKind,
    /// 观察者帧（tool-invoke / secret-get 收窄）。
    pub observer_frame: bool,
    /// 帧预算。
    pub frame: FrameBudget,
    /// 日志缓冲（注册表持有，共享）。
    pub logs: Arc<PluginLogBuffer>,
    /// 限制表。
    pub limits: PluginLimits,
    /// 实例配置。
    pub config: Arc<InstanceConfig>,
    /// 凭据解析面。
    pub secrets: Arc<dyn SecretResolver>,
    /// 工作区根（绝对）。
    pub workspace_root: std::path::PathBuf,
    /// 出站代理策略。
    pub egress: Arc<crate::egress::EgressPolicy>,
    /// 宿主工具调用面（None = 未装配，恒 unavailable）。
    pub invoker: Option<Arc<dyn HostToolInvoker>>,
    /// 杂务 LLM 补全面（None = 未装配，恒 unavailable；2026-10-22 批 4）。
    pub llm: Option<Arc<dyn LlmInvoker>>,
    /// tokio runtime 句柄（同步 host 函数里 block_on 异步下游用）。
    pub tokio_handle: Option<tokio::runtime::Handle>,
    /// 审计器（None = 未装配，静默跳过）。
    pub audit: Option<SharedAuditLogger>,
    /// 发起会话键（审计 user 字段；观察者帧为空）。
    pub session_key: String,
}

impl CallCaps {
    /// 落一条插件侧审计事件（审计器未装配 = 静默跳过；锁 poisoned 取内值
    /// 继续写——审计链不因单次锁故障丢事件）。
    pub(crate) fn audit_event(
        &self,
        event: &str,
        decision: &str,
        target: &str,
        danger: &str,
        reason: &str,
    ) {
        let Some(audit) = self.audit.as_ref() else {
            return;
        };
        let mut guard = match audit.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.log_event(
            event,
            decision,
            event,
            &self.session_key,
            &format!("wasm-plugin:{}", self.slug),
            target,
            danger,
            reason,
            "wasm-plugin",
        );
    }

    pub(crate) fn log_line(&self, level: &str, message: &str) {
        // 凭据脱敏先于截断（2026-09-30 复查 #3）：先截断可能把凭据切成两半
        // 各自漏检——顺序必须是先脱敏后截断。secret-get 有「observe 帧内
        // 拒答」的帧纪律，但 tool 帧里 guest 完全可以把拿到的原文 log 出来
        // ——环形缓冲经 WSAPI wasm.logs 直达 Dashboard，同属出站面。脱敏
        // 不写审计链：log 频度 guest 可控，刷审计链本身就是 DoS 面；工具
        // 输出的出站凭据复扫在 bridge 侧承担审计（plugin_bridge
        // ::rescan_output_credentials）。
        let redacted = credential_scanner().redact_content(message);
        let truncated: String = {
            let max = self.limits.log_line_max_bytes;
            if redacted.len() > max {
                // 预算按字节、切点必须落在字符边界——字节判据命中而字符数
                // 不足时，裸字节切点会切进多字节 UTF-8 序列中间 panic
                //（2026-09-29 交付审查 M2，guest 可控输入直达）。
                let keep = max / 4 * 3;
                let mut cut = keep.min(redacted.len());
                while cut > 0 && !redacted.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!("{}…[截断]", &redacted[..cut])
            } else {
                redacted
            }
        };
        self.logs.push(PluginLogLine {
            ts_ms: unix_millis(),
            level: level.to_string(),
            message: truncated,
        });
    }
}

/// 凭据脱敏 Scanner 单例（log_line 专用；pattern 集内部已有 OnceLock 缓存，
/// 这里省的是结构体构造——log 每行都走，别做重复功）。恒开无视
/// credential 开关：与 bridge 出站复扫同一口径，这是宿主自保不是常规检测层。
fn credential_scanner() -> &'static nemesis_security::credential::Scanner {
    static SCANNER: std::sync::OnceLock<nemesis_security::credential::Scanner> =
        std::sync::OnceLock::new();
    SCANNER.get_or_init(|| nemesis_security::credential::Scanner::new(true, "block"))
}

// ---------------------------------------------------------------------------
// 共享帮助
// ---------------------------------------------------------------------------

/// 工作区相对路径只读（越界/绝对/8.3 短名拒绝；超限拒绝）。
pub(crate) fn read_workspace_rel(
    root: &std::path::Path,
    rel: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if rel.is_empty() {
        return Err("empty path".into());
    }
    let cand = std::path::Path::new(rel);
    if cand.is_absolute() {
        return Err(format!("absolute path rejected: {rel}"));
    }
    if rel
        .split(['/', '\\'])
        .any(|seg| seg == ".." || seg.contains('~') || crate::install::is_eight_short_name(seg))
    {
        return Err(format!("path escapes workspace or malformed: {rel}"));
    }
    let full = root.join(cand);
    let canonical = full
        .canonicalize()
        .map_err(|e| format!("cannot resolve {rel}: {e}"))?;
    if !canonical.starts_with(root) {
        return Err(format!("path escapes workspace: {rel}"));
    }
    let meta = std::fs::metadata(&canonical).map_err(|e| format!("stat {rel}: {e}"))?;
    if meta.is_dir() {
        return Err(format!("path is a directory: {rel}"));
    }
    if meta.len() > max_bytes as u64 {
        return Err(format!(
            "file exceeds workspace-read limit ({max_bytes} bytes): {rel}"
        ));
    }
    std::fs::read(&canonical).map_err(|e| format!("read {rel}: {e}"))
}

/// 错误文本截断（回 guest 的错误串恒定上限，防大错误撑爆 guest 内存）。
/// 切点向字符边界收（同 [`CallCaps::log_line`]，2026-09-29 交付审查 M2）。
pub(crate) fn truncate_err(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

/// [`PluginError`] → 工具失败文本（回灌 LLM 的形态；与内置工具错误风格一致）。
#[must_use]
pub fn plugin_error_text(e: &PluginError) -> String {
    match e {
        PluginError::Timeout { ms } => format!("Tool error: plugin execution timeout after {ms}ms"),
        PluginError::Busy(n) => format!("Tool error: plugin instance limit reached ({n})"),
        PluginError::Trap(s) => format!("Tool error: plugin trapped: {}", truncate_err(s, 300)),
        other => format!("Tool error: {}", truncate_err(&other.to_string(), 300)),
    }
}
