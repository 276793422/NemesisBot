//! 宿主能力面实现（WIT `nemesis:plugin/host` 的 Host trait 两 world 各一份）
//! + per-call 上下文（WasiView + 能力授予）。
//!
//! 语义要点：
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

use crate::bindings::{observer as obs_bind, tool as tool_bind};
use crate::error::PluginError;
use crate::limits::{PluginLimits, PluginResourceLimiter};
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

    /// 消耗一次调用额度；耗尽返回 Err。
    fn consume(&self) -> Result<(), ToolHostError> {
        // fetch_sub 饱和语义：0 时不再下探。
        let prev = self
            .remaining
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                if n == 0 { None } else { Some(n - 1) }
            });
        match prev {
            Ok(_) => Ok(()),
            Err(_) => Err(ToolHostError::BudgetExceeded(
                "host call budget exhausted".to_string(),
            )),
        }
    }
}

/// tool world 的 host-error 类型别名。
pub type ToolHostError = crate::bindings::tool::nemesis::plugin::host::HostError;
/// observer world 的 host-error 类型别名。
pub type ObsHostError = crate::bindings::observer::nemesis::plugin::host::HostError;

/// 构造 policy-denied（tool world）。
pub(crate) fn host_error_denied(msg: &str) -> ToolHostError {
    ToolHostError::PolicyDenied(msg.to_string())
}

/// 构造 not-found（tool world）。
pub(crate) fn host_error_not_found(msg: &str) -> ToolHostError {
    ToolHostError::NotFound(msg.to_string())
}

/// 构造 unavailable（tool world）。
pub(crate) fn host_error_unavailable(msg: &str) -> ToolHostError {
    ToolHostError::Unavailable(msg.to_string())
}

/// 构造 no-permission（tool world）。
pub(crate) fn host_error_no_permission() -> ToolHostError {
    ToolHostError::NoPermission
}

/// tool→observer 的 host-error 同构转换。
pub(crate) fn to_obs_error(e: ToolHostError) -> ObsHostError {
    match e {
        ToolHostError::NoPermission => ObsHostError::NoPermission,
        ToolHostError::PolicyDenied(s) => ObsHostError::PolicyDenied(s),
        ToolHostError::NotFound(s) => ObsHostError::NotFound(s),
        ToolHostError::BudgetExceeded(s) => ObsHostError::BudgetExceeded(s),
        ToolHostError::Unavailable(s) => ObsHostError::Unavailable(s),
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 单次调用的宿主上下文（fresh-store-per-call 的 store state）。
pub struct PluginCtx {
    pub(crate) wasi: wasmtime_wasi::WasiCtx,
    pub(crate) table: wasmtime::component::ResourceTable,
    /// 能力授予面（本帧有效）。
    pub caps: Arc<CallCaps>,
    /// 内存/表预算（fresh-store 组装期由 runtime 赋值）。
    pub(crate) limiter: PluginResourceLimiter,
}

impl wasmtime_wasi::WasiView for PluginCtx {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// 一次调用帧的能力授予（构建于 runtime 的 fresh-store 组装期）。
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
    fn audit_event(&self, event: &str, decision: &str, target: &str, danger: &str, reason: &str) {
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

    fn log_line(&self, level: &str, message: &str) {
        let truncated: String = {
            let max = self.limits.log_line_max_bytes;
            if message.len() > max {
                // 预算按字节、切点必须落在字符边界——字节判据命中而字符数
                // 不足时，裸字节切点会切进多字节 UTF-8 序列中间 panic
                //（2026-09-29 交付审查 M2，guest 可控输入直达）。
                let keep = max / 4 * 3;
                let mut cut = keep.min(message.len());
                while cut > 0 && !message.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!("{}…[截断]", &message[..cut])
            } else {
                message.to_string()
            }
        };
        self.logs.push(PluginLogLine {
            ts_ms: unix_millis(),
            level: level.to_string(),
            message: truncated,
        });
    }
}

// ---------------------------------------------------------------------------
// tool world 的 Host 实现
// ---------------------------------------------------------------------------

/// `types` 接口纯类型定义（json-string 别名），Host 为空 trait——bindgen 的
/// add_to_linker 要求 store data 覆盖全部被导入接口，这里补空实现。
impl tool_bind::nemesis::plugin::types::Host for PluginCtx {}
impl obs_bind::nemesis::plugin::types::Host for PluginCtx {}

impl tool_bind::nemesis::plugin::host::Host for PluginCtx {
    fn log(&mut self, level: tool_bind::nemesis::plugin::host::LogLevel, message: String) {
        let level_s = match level {
            tool_bind::nemesis::plugin::host::LogLevel::Trace => "trace",
            tool_bind::nemesis::plugin::host::LogLevel::Debug => "debug",
            tool_bind::nemesis::plugin::host::LogLevel::Info => "info",
            tool_bind::nemesis::plugin::host::LogLevel::Warn => "warn",
            tool_bind::nemesis::plugin::host::LogLevel::Error => "error",
        };
        self.caps.log_line(level_s, &message);
    }

    fn now_millis(&mut self) -> u64 {
        if self.caps.frame.consume().is_err() {
            // 预算耗尽：退化为 0（guest 拿到语义可辨的时间原点）——
            // 无 result 返回型的函数不能在此回 host-error。
            return 0;
        }
        unix_millis()
    }

    fn config_get(&mut self, key: String) -> Result<Option<String>, ToolHostError> {
        self.caps.frame.consume()?;
        if self.caps.config.x_secret_keys.contains(&key) {
            return Err(host_error_denied(
                "x-secret key is not readable via config-get; use secret-get",
            ));
        }
        if !self.caps.config.declared_keys.contains(&key) {
            return Err(host_error_not_found(&format!("key not declared: {key}")));
        }
        Ok(self.caps.config.entries.get(&key).cloned())
    }

    fn secret_get(&mut self, name: String) -> Result<String, ToolHostError> {
        self.caps.frame.consume()?;
        if self.caps.observer_frame {
            return Err(host_error_unavailable(
                "secret-get is unavailable in observer frames",
            ));
        }
        if !self.caps.config.x_secret_keys.contains(&name) {
            self.caps.audit_event(
                "plugin_secret_get",
                "denied",
                &format!("plugin/{}/{}", self.caps.slug, name),
                "MEDIUM",
                "secret name not declared in manifest x-secret",
            );
            return Err(host_error_denied(&format!(
                "secret name not declared in manifest x-secret: {name}"
            )));
        }
        let alias = format!("vault:plugin/{}/{}", self.caps.slug, name);
        match self.caps.secrets.resolve(&alias) {
            Some(v) => {
                // 审计只记别名不记原文。
                self.caps.audit_event(
                    "plugin_secret_get",
                    "allowed",
                    &alias,
                    "MEDIUM",
                    "secret resolved for wasm plugin",
                );
                Ok(v)
            }
            None => {
                self.caps.audit_event(
                    "plugin_secret_get",
                    "denied",
                    &alias,
                    "MEDIUM",
                    "no secret stored for alias",
                );
                Err(host_error_not_found(&format!(
                    "no secret stored for alias {alias}"
                )))
            }
        }
    }

    fn workspace_read(&mut self, rel_path: String) -> Result<Vec<u8>, ToolHostError> {
        self.caps.frame.consume()?;
        match read_workspace_rel(
            &self.caps.workspace_root,
            &rel_path,
            self.caps.limits.workspace_read_max_bytes,
        ) {
            Ok(bytes) => {
                self.caps.audit_event(
                    "plugin_workspace_read",
                    "allowed",
                    &rel_path,
                    "LOW",
                    "workspace file read by wasm plugin",
                );
                Ok(bytes)
            }
            Err(e) => {
                self.caps.audit_event(
                    "plugin_workspace_read",
                    "denied",
                    &rel_path,
                    "LOW",
                    &truncate_err(&e, 100),
                );
                Err(host_error_denied(&e))
            }
        }
    }

    fn data_dir_path(&mut self) -> Result<String, ToolHostError> {
        self.caps.frame.consume()?;
        Ok("/data".to_string())
    }

    fn http_send(
        &mut self,
        req: tool_bind::nemesis::plugin::host::HttpRequest,
    ) -> Result<tool_bind::nemesis::plugin::host::HttpResponse, ToolHostError> {
        self.caps.frame.consume()?;
        let policy = self.caps.egress.clone();
        let limits = self.caps.limits.clone();
        let handle = self.caps.tokio_handle.clone();
        if !policy.enabled {
            self.caps.audit_event(
                "plugin_http_send",
                "denied",
                &req.url,
                "MEDIUM",
                "egress not granted (deny-by-default)",
            );
            return Err(host_error_no_permission());
        }
        let Some(h) = handle else {
            // 无 tokio 上下文（罕见：headless 装配遗漏）；fail-closed。
            return Err(host_error_unavailable("egress runtime unavailable"));
        };
        let fut = crate::egress::proxy_request(
            &policy,
            &limits,
            crate::egress::EgressRequest {
                method: req.method,
                url: req.url.clone(),
                headers: req.headers,
                body: req.body,
            },
        );
        match h.block_on(fut) {
            Ok(resp) => {
                self.caps.audit_event(
                    "plugin_http_send",
                    "allowed",
                    &req.url,
                    "MEDIUM",
                    &format!("egress via wasm plugin, status {}", resp.status),
                );
                Ok(tool_bind::nemesis::plugin::host::HttpResponse {
                    status: resp.status,
                    headers: resp.headers,
                    body: resp.body,
                })
            }
            Err(e) => {
                self.caps.audit_event(
                    "plugin_http_send",
                    "denied",
                    &req.url,
                    "MEDIUM",
                    &truncate_err(&e, 100),
                );
                Err(host_error_denied(&e))
            }
        }
    }

    fn tool_invoke(&mut self, tool: String, args_json: String) -> Result<String, ToolHostError> {
        self.caps.frame.consume()?;
        if self.caps.observer_frame {
            return Err(host_error_unavailable(
                "tool-invoke is unavailable in observer frames",
            ));
        }
        if tool.starts_with(crate::PLUGIN_TOOL_PREFIX) {
            return Err(host_error_denied(
                "plugin tools cannot invoke plugin tools (depth 1)",
            ));
        }
        let Some(invoker) = self.caps.invoker.clone() else {
            return Err(host_error_unavailable(
                "host tool invoker not wired (plugin host standalone)",
            ));
        };
        let handle = self
            .caps
            .tokio_handle
            .clone()
            .ok_or_else(|| host_error_unavailable("no tokio runtime for tool dispatch"))?;
        let result = handle.block_on(invoker.invoke(&tool, &args_json));
        match result {
            Ok(text) => Ok(text),
            Err(e) => Err(host_error_unavailable(&truncate_err(&e, 300))),
        }
    }
}

// ---------------------------------------------------------------------------
// observer world 的 Host 实现（同一语义，bindgen 类型不同——委托共享逻辑）
// ---------------------------------------------------------------------------

impl obs_bind::nemesis::plugin::host::Host for PluginCtx {
    fn log(&mut self, level: obs_bind::nemesis::plugin::host::LogLevel, message: String) {
        let level_s = match level {
            obs_bind::nemesis::plugin::host::LogLevel::Trace => "trace",
            obs_bind::nemesis::plugin::host::LogLevel::Debug => "debug",
            obs_bind::nemesis::plugin::host::LogLevel::Info => "info",
            obs_bind::nemesis::plugin::host::LogLevel::Warn => "warn",
            obs_bind::nemesis::plugin::host::LogLevel::Error => "error",
        };
        self.caps.log_line(level_s, &message);
    }

    fn now_millis(&mut self) -> u64 {
        if self.caps.frame.consume().is_err() {
            return 0;
        }
        unix_millis()
    }

    fn config_get(&mut self, key: String) -> Result<Option<String>, ObsHostError> {
        self.caps.frame.consume().map_err(to_obs_error)?;
        if self.caps.config.x_secret_keys.contains(&key) {
            return Err(to_obs_error(host_error_denied(
                "x-secret key is not readable via config-get; use secret-get",
            )));
        }
        if !self.caps.config.declared_keys.contains(&key) {
            return Err(to_obs_error(host_error_not_found(&format!(
                "key not declared: {key}"
            ))));
        }
        Ok(self.caps.config.entries.get(&key).cloned())
    }

    fn secret_get(&mut self, _name: String) -> Result<String, ObsHostError> {
        self.caps.frame.consume().map_err(to_obs_error)?;
        Err(to_obs_error(host_error_unavailable(
            "secret-get is unavailable in observer frames",
        )))
    }

    fn workspace_read(&mut self, rel_path: String) -> Result<Vec<u8>, ObsHostError> {
        self.caps.frame.consume().map_err(to_obs_error)?;
        match read_workspace_rel(
            &self.caps.workspace_root,
            &rel_path,
            self.caps.limits.workspace_read_max_bytes,
        ) {
            Ok(bytes) => {
                self.caps.audit_event(
                    "plugin_workspace_read",
                    "allowed",
                    &rel_path,
                    "LOW",
                    "workspace file read by wasm plugin (observer)",
                );
                Ok(bytes)
            }
            Err(e) => {
                self.caps.audit_event(
                    "plugin_workspace_read",
                    "denied",
                    &rel_path,
                    "LOW",
                    &truncate_err(&e, 100),
                );
                Err(to_obs_error(host_error_denied(&e)))
            }
        }
    }

    fn data_dir_path(&mut self) -> Result<String, ObsHostError> {
        self.caps.frame.consume().map_err(to_obs_error)?;
        Ok("/data".to_string())
    }

    fn http_send(
        &mut self,
        req: obs_bind::nemesis::plugin::host::HttpRequest,
    ) -> Result<obs_bind::nemesis::plugin::host::HttpResponse, ObsHostError> {
        self.caps.frame.consume().map_err(to_obs_error)?;
        let policy = self.caps.egress.clone();
        let limits = self.caps.limits.clone();
        let handle = self.caps.tokio_handle.clone();
        if !policy.enabled {
            self.caps.audit_event(
                "plugin_http_send",
                "denied",
                &req.url,
                "MEDIUM",
                "egress not granted (deny-by-default)",
            );
            return Err(to_obs_error(host_error_no_permission()));
        }
        let Some(h) = handle else {
            return Err(to_obs_error(host_error_unavailable(
                "egress runtime unavailable",
            )));
        };
        let fut = crate::egress::proxy_request(
            &policy,
            &limits,
            crate::egress::EgressRequest {
                method: req.method,
                url: req.url.clone(),
                headers: req.headers,
                body: req.body,
            },
        );
        match h.block_on(fut) {
            Ok(resp) => {
                self.caps.audit_event(
                    "plugin_http_send",
                    "allowed",
                    &req.url,
                    "MEDIUM",
                    &format!("egress via wasm plugin (observer), status {}", resp.status),
                );
                Ok(obs_bind::nemesis::plugin::host::HttpResponse {
                    status: resp.status,
                    headers: resp.headers,
                    body: resp.body,
                })
            }
            Err(e) => {
                self.caps.audit_event(
                    "plugin_http_send",
                    "denied",
                    &req.url,
                    "MEDIUM",
                    &truncate_err(&e, 100),
                );
                Err(to_obs_error(host_error_denied(&e)))
            }
        }
    }

    fn tool_invoke(&mut self, _tool: String, _args_json: String) -> Result<String, ObsHostError> {
        self.caps.frame.consume().map_err(to_obs_error)?;
        Err(to_obs_error(host_error_unavailable(
            "tool-invoke is unavailable in observer frames",
        )))
    }
}

// ---------------------------------------------------------------------------
// 共享帮助
// ---------------------------------------------------------------------------

/// 工作区相对路径只读（越界/绝对/8.3 短名拒绝；超限拒绝）。
fn read_workspace_rel(
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

/// 从 trap 错误里识别超时类失败（runtime 外层归约用）。
///
/// 原因在 anyhow source 链深处（Display 只给 backtrace 上下文），所以要
/// 沿链下钻：epoch 超时 = `Trap::Interrupt`，fuel 耗尽 = `Trap::OutOfFuel`
/// （wasmtime 48 措辞）；文本匹配留作跨版本兜底。
pub(crate) fn is_epoch_timeout(err: &wasmtime::Error) -> bool {
    let mut cur = Some(err.as_ref() as &dyn std::error::Error);
    while let Some(e) = cur {
        if let Some(t) = e.downcast_ref::<wasmtime::Trap>()
            && matches!(t, wasmtime::Trap::Interrupt | wasmtime::Trap::OutOfFuel)
        {
            return true;
        }
        let s = e.to_string();
        if s.contains("epoch") || s.contains("fuel") || s.contains("all store resources consumed") {
            return true;
        }
        cur = e.source();
    }
    false
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
