//! 插件注册表（PluginManager）：slug → 已注册插件的运行时管理。
//!
//! 职责：注册/注销、工具定义公开面（桥层消费）、execute 分派（构建
//! CallFrame → runtime）、观察者事件队列与 worker、实例配置存取
//! （`config/plugins/<slug>.json`）、enable/disable 状态。
//!
//! 目录约定（workspace 相对）：
//! - `plugins/<slug>/`——插件载荷（plugin.toml + plugin.wasm）
//! - `plugins/lockfile.json`——安装快照
//! - `plugin-data/<slug>/`——per-plugin 私有数据目录（guest 挂 /data）
//! - `cache/wasm/`——wasmtime 编译缓存根

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};

use crate::error::PluginError;
use crate::host_impl::{InstanceConfig, PluginLogBuffer, SecretResolver, plugin_error_text};
use crate::limits::PluginLimits;
use crate::manifest::{PluginKind, PluginManifest};
use crate::runtime::{CallFrame, ExecContext, ExecOutput, PluginRuntime};
use crate::trust::PluginTrustState;

/// 注册进宿主的插件工具全名前缀（LLM 可见；`plugin.<slug>.<base>`）。
pub const PLUGIN_TOOL_PREFIX: &str = crate::PLUGIN_TOOL_PREFIX;

/// 工具元数据快照（注册进宿主的唯一事实源；来自安装期 get-metadata 对账）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolMetaSnapshot {
    /// 全名（`plugin.<slug>.<base>`）。
    pub name: String,
    /// 工具基础名（guest 自报）。
    pub base: String,
    /// 人读标题。
    pub title: String,
    /// LLM 可见描述。
    pub description: String,
    /// 参数 JSON Schema（顶层 object）。
    pub parameters_json: String,
    /// 操作类型声明（read/write/exec/network/空）。
    pub operation_type: String,
    /// manifest 最低调用档（mini/normal/big）。
    pub min_tier: String,
}

/// 观察者事件队列发送端（宿主持有于 observers map；drop 即关 worker）。
type ObserverSender = tokio::sync::mpsc::Sender<String>;

/// 已注册插件。
pub struct RegisteredPlugin {
    /// manifest。
    pub manifest: PluginManifest,
    /// 信任四态（安装/启动时验签结论）。
    pub trust: PluginTrustState,
    /// 编译产物。
    pub component: Arc<wasmtime::component::Component>,
    /// 工具元数据（kind=tool 必有）。
    pub tool_meta: Option<ToolMetaSnapshot>,
    /// 宿主日志环形缓冲。
    pub logs: Arc<PluginLogBuffer>,
    /// 出站策略。
    pub egress: Arc<crate::egress::EgressPolicy>,
    /// 收紧后的限制表（全局上限 ∧ manifest 覆盖）。
    pub limits: PluginLimits,
    /// 载荷目录（绝对）。
    pub dir: PathBuf,
    /// 数据目录（绝对；canonicalize 过）。
    pub data_dir: PathBuf,
    /// 启用位（disable 后 execute 拒绝、观察者停投）。
    pub enabled: AtomicBool,
    /// 队列丢弃计数（含队列满与实例满员丢帧）。
    pub dropped_events: Arc<AtomicU64>,
}

impl RegisteredPlugin {
    /// 工具全名（kind=tool）。
    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        self.tool_meta.as_ref().map(|m| m.name.as_str())
    }
}

/// 插件管理器（gateway / headless 装配点各持一份 Arc）。
pub struct PluginManager {
    /// 工作区根（canonical）。
    workspace_root: PathBuf,
    /// 插件载荷根（canonical）。
    plugins_dir: PathBuf,
    /// 数据目录根（canonical）。
    data_root: PathBuf,
    /// 全局限制表（config 收紧后）。
    limits: PluginLimits,
    /// 执行器。
    runtime: PluginRuntime,
    /// 注册表。
    plugins: RwLock<HashMap<String, Arc<RegisteredPlugin>>>,
    /// 观察者 worker 发送端（slug → 队列；drop 发送端 = worker 退出）。
    observers: Mutex<HashMap<String, ObserverSender>>,
    /// 凭据解析面（vault 桥）。
    secrets: Arc<dyn SecretResolver>,
    /// 宿主工具调用面（gateway 装配后注入；None = 恒 unavailable）。
    invoker: Mutex<Option<Arc<dyn crate::host_impl::HostToolInvoker>>>,
    /// 子系统总开关（config `plugins.enabled`；false = 全部拒绝执行/停投）。
    enabled: AtomicBool,
    /// 审计器（gateway 装配后注入；None = 敏感宿主调用静默跳过审计）。
    audit: Mutex<Option<crate::host_impl::SharedAuditLogger>>,
    /// 实例配置文件读-改-写互斥（set_config_key/unset/enable 与 CLI 磁盘
    /// 启停共用；防同进程并发写互相踩踏。跨进程无闸——CLI vs gateway 是
    /// 已知边界，见交付报告）。
    config_io: Mutex<()>,
}

impl PluginManager {
    /// 构建管理器（目录不存在即创建）。
    pub fn new(
        workspace_root: &Path,
        limits: PluginLimits,
        secrets: Arc<dyn SecretResolver>,
    ) -> Result<Self, PluginError> {
        let workspace_root = workspace_root
            .canonicalize()
            .map_err(|e| PluginError::Io(format!("workspace root: {e}")))?;
        let plugins_dir = workspace_root.join("plugins");
        let data_root = workspace_root.join("plugin-data");
        let cache_root = workspace_root.join("cache").join("wasm");
        std::fs::create_dir_all(&plugins_dir)
            .map_err(|e| PluginError::Io(format!("create plugins dir: {e}")))?;
        std::fs::create_dir_all(&data_root)
            .map_err(|e| PluginError::Io(format!("create plugin-data dir: {e}")))?;
        let runtime = PluginRuntime::new(limits.clone(), Some(&cache_root))?;
        Ok(Self {
            workspace_root,
            plugins_dir,
            data_root,
            limits,
            runtime,
            plugins: RwLock::new(HashMap::new()),
            observers: Mutex::new(HashMap::new()),
            secrets,
            invoker: Mutex::new(None),
            enabled: AtomicBool::new(true),
            audit: Mutex::new(None),
            config_io: Mutex::new(()),
        })
    }

    /// 装配宿主工具调用面（gateway 在 agent loop 就绪后调用）。
    pub fn set_invoker(&self, invoker: Arc<dyn crate::host_impl::HostToolInvoker>) {
        *self.invoker.lock() = Some(invoker);
    }

    /// 装配审计器（gateway 在安全插件装配后调用；敏感宿主调用落审计链）。
    pub fn set_audit(&self, audit: crate::host_impl::SharedAuditLogger) {
        *self.audit.lock() = Some(audit);
    }

    /// 桥侧审计事件（宿主能力面之外、manager 关联的敏感路径——如工具
    /// 输出凭据复扫）。未装配审计器 = 静默跳过；锁 poisoned 取内值继续
    /// 写（审计链不因单次锁故障丢事件）。
    pub fn audit_event(
        &self,
        event: &str,
        decision: &str,
        target: &str,
        danger: &str,
        reason: &str,
        session_key: &str,
    ) {
        let guard = self.audit.lock();
        let Some(audit) = guard.as_ref() else {
            return;
        };
        let mut logger = match audit.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        logger.log_event(
            event,
            decision,
            event,
            session_key,
            "wasm-plugin",
            target,
            danger,
            reason,
            "wasm-plugin",
        );
    }

    /// 设置子系统总开关。
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
    }

    /// 子系统总开关。
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 工作区根（canonical）。
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// 插件载荷根。
    #[must_use]
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// 全局限制表。
    #[must_use]
    pub fn limits(&self) -> &PluginLimits {
        &self.limits
    }

    /// 执行器引用（install 漏斗复用 probe/编译路径）。
    pub(crate) fn runtime(&self) -> &PluginRuntime {
        &self.runtime
    }

    /// 用共享引擎编译一段 wasm 组件字节（含 workspace 编译缓存收益）。
    pub fn compile_component(
        &self,
        wasm_bytes: &[u8],
    ) -> Result<wasmtime::component::Component, PluginError> {
        wasmtime::component::Component::new(&self.runtime.engine, wasm_bytes)
            .map_err(|e| PluginError::Compile(format!("compile: {e}")))
    }

    /// slug 的载荷目录。
    #[must_use]
    pub fn plugin_dir(&self, slug: &str) -> PathBuf {
        self.plugins_dir.join(slug)
    }

    /// slug 的数据目录。
    #[must_use]
    pub fn plugin_data_dir(&self, slug: &str) -> PathBuf {
        self.data_root.join(slug)
    }

    // -----------------------------------------------------------------------
    // 注册 / 注销
    // -----------------------------------------------------------------------

    /// 注册一个已通过装配漏斗的插件（install 漏斗第⑧步 / 启动装载共用）。
    ///
    /// kind=observer 会同时拉起事件 worker（有界队列，满丢弃+计数）。
    pub fn register(
        self: &Arc<Self>,
        manifest: PluginManifest,
        trust: PluginTrustState,
        component: wasmtime::component::Component,
        tool_meta: Option<ToolMetaSnapshot>,
    ) -> Result<Arc<RegisteredPlugin>, PluginError> {
        let slug = manifest.slug.clone();
        if let Some(m) = tool_meta.as_ref() {
            if *manifest.kind != PluginKind::Tool {
                return Err(PluginError::Manifest(
                    "tool metadata provided for non-tool plugin".into(),
                ));
            }
            let expect = format!("{PLUGIN_TOOL_PREFIX}{slug}.{}", m.base);
            if m.name != expect {
                return Err(PluginError::MetadataMismatch(format!(
                    "registered name '{}' != expected '{expect}'",
                    m.name
                )));
            }
        } else if *manifest.kind == PluginKind::Tool {
            return Err(PluginError::MetadataMismatch(
                "tool plugin missing metadata snapshot".into(),
            ));
        }
        let (limits, ignored) = self.limits.tighten_with(&manifest.limits);
        for k in &ignored {
            tracing::warn!(slug = %slug, key = %k, "[WasmPlugin] manifest limits 放宽请求被忽略（只允许收紧）");
        }
        let data_dir = {
            let d = self.plugin_data_dir(&slug);
            std::fs::create_dir_all(&d)
                .map_err(|e| PluginError::Io(format!("create data dir: {e}")))?;
            d.canonicalize().unwrap_or(d)
        };
        let egress = Arc::new(crate::egress::EgressPolicy::from_permissions(
            manifest.permissions.egress.clone(),
            manifest.permissions.allow_private,
        ));
        let logs = Arc::new(PluginLogBuffer::new(
            limits.log_ring_capacity,
            limits.log_line_max_bytes,
        ));
        let dropped = Arc::new(AtomicU64::new(0));
        // 升级替换路径：先停旧 worker（drop 旧发送端），observer 再拉新的。
        drop(self.observers.lock().remove(&slug));
        if *manifest.kind == PluginKind::Observer {
            let q = self.spawn_observer_worker(&slug, dropped.clone());
            self.observers.lock().insert(slug.clone(), q);
        }
        let reg = Arc::new(RegisteredPlugin {
            manifest,
            trust,
            component: Arc::new(component),
            tool_meta,
            logs,
            egress,
            limits,
            dir: self.plugin_dir(&slug),
            data_dir,
            enabled: AtomicBool::new(true),
            dropped_events: dropped,
        });
        self.plugins.write().insert(slug.clone(), reg.clone());
        tracing::info!(slug = %slug, kind = reg.manifest.kind.as_str(), "[WasmPlugin] 已注册");
        Ok(reg)
    }

    /// 注销（升级/卸载路径）。返回是否原本在场。
    pub fn unregister(&self, slug: &str) -> bool {
        // drop 发送端 → worker 收 None 退出（即使注册表无条目也清队列）。
        drop(self.observers.lock().remove(slug));
        if self.plugins.write().remove(slug).is_some() {
            tracing::info!(slug, "[WasmPlugin] 已注销");
            true
        } else {
            false
        }
    }

    /// 查注册表。
    #[must_use]
    pub fn get(&self, slug: &str) -> Option<Arc<RegisteredPlugin>> {
        self.plugins.read().get(slug).cloned()
    }

    /// 全部注册项快照。
    #[must_use]
    pub fn list(&self) -> Vec<Arc<RegisteredPlugin>> {
        self.plugins.read().values().cloned().collect()
    }

    /// 按工具全名查找（`plugin.<slug>.<base>`）。
    #[must_use]
    pub fn find_by_tool_name(&self, tool_name: &str) -> Option<Arc<RegisteredPlugin>> {
        let rest = tool_name.strip_prefix(PLUGIN_TOOL_PREFIX)?;
        let slug = rest.split_once('.')?.0;
        let reg = self.get(slug)?;
        reg.tool_name().filter(|n| *n == tool_name)?;
        Some(reg)
    }

    // -----------------------------------------------------------------------
    // 执行 / 观察
    // -----------------------------------------------------------------------

    fn build_frame(&self, reg: &RegisteredPlugin, observer_frame: bool) -> CallFrame {
        let config = self.instance_config(reg);
        CallFrame {
            observer_frame,
            slug: reg.manifest.slug.clone(),
            kind: *reg.manifest.kind,
            // 注册时已收紧的 per-plugin 限制表（manifest [limits] 生效面）。
            limits: reg.limits.clone(),
            logs: reg.logs.clone(),
            config: Arc::new(config),
            secrets: self.secrets.clone(),
            workspace_root: self.workspace_root.clone(),
            egress: reg.egress.clone(),
            invoker: self.invoker.lock().clone(),
            data_dir: reg.data_dir.clone(),
            audit: self.audit.lock().clone(),
            session_key: String::new(),
        }
    }

    /// 执行一次插件工具调用（桥层入口）。
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        args_json: String,
        exec_ctx: ExecContext,
    ) -> Result<ExecOutput, PluginError> {
        let Some(reg) = self.find_by_tool_name(tool_name) else {
            return Err(PluginError::NotAvailable(format!(
                "plugin tool not found: {tool_name}"
            )));
        };
        if !self.is_enabled() || !reg.enabled.load(Ordering::SeqCst) {
            return Err(PluginError::NotAvailable(format!(
                "plugin '{}' disabled",
                reg.manifest.slug
            )));
        }
        let frame = self.build_frame(&reg, false);
        self.runtime
            .execute_tool(reg.component.clone(), frame, args_json, exec_ctx)
            .await
    }

    /// 投递一条观察者事件给全部启用的观察者插件（fire-and-forget；满丢弃+计数）。
    pub fn enqueue_observer_event(&self, event_json: String) {
        if !self.is_enabled() {
            return;
        }
        for reg in self.list() {
            if !reg.enabled.load(Ordering::SeqCst) {
                continue;
            }
            let q = self.observers.lock().get(&reg.manifest.slug).cloned();
            let Some(q) = q else {
                continue;
            };
            if q.try_send(event_json.clone()).is_err() {
                reg.dropped_events.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// 拉起单个观察者插件的投递 worker（fresh-store-per-event；Busy 计丢弃）。
    fn spawn_observer_worker(
        self: &Arc<Self>,
        slug: &str,
        dropped: Arc<AtomicU64>,
    ) -> ObserverSender {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(self.limits.observer_queue_depth);
        let mgr = Arc::downgrade(self);
        let slug = slug.to_string();
        tokio::spawn(async move {
            while let Some(event_json) = rx.recv().await {
                let Some(mgr) = mgr.upgrade() else { break };
                let Some(reg) = mgr.get(&slug) else { break };
                if !reg.enabled.load(Ordering::SeqCst) {
                    continue;
                }
                let frame = mgr.build_frame(&reg, true);
                let comp = reg.component.clone();
                if let Err(e) = mgr.runtime.call_observe(comp, frame, event_json).await {
                    match e {
                        PluginError::Timeout { ms } => {
                            tracing::warn!(slug = %slug, ms, "[WasmPlugin] observe 超时（事件丢弃）")
                        }
                        PluginError::Busy(_) => {
                            dropped.fetch_add(1, Ordering::Relaxed);
                        }
                        other => tracing::warn!(
                            slug = %slug,
                            error = %plugin_error_text(&other),
                            "[WasmPlugin] observe 失败（事件丢弃）"
                        ),
                    }
                }
            }
        });
        tx
    }

    // -----------------------------------------------------------------------
    // 实例配置
    // -----------------------------------------------------------------------

    fn config_path(&self, slug: &str) -> PathBuf {
        self.workspace_root
            .join("config")
            .join("plugins")
            .join(format!("{slug}.json"))
    }

    /// 读实例配置文件。不存在 = 全新缺省（启用）；**存在但损坏** = warn +
    /// fail-closed 禁用态——损坏静默回缺省会把已禁用插件悄悄复活
    ///（2026-09-29 交付审查 L6；修复或删除该文件后恢复）。
    pub(crate) fn read_instance_config(&self, slug: &str) -> InstanceConfigFile {
        match std::fs::read_to_string(self.config_path(slug)) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        slug,
                        error = %e,
                        "[WasmPlugin] 实例配置文件损坏，按禁用处理（修复或删除该文件后重启/重装恢复）"
                    );
                    InstanceConfigFile {
                        enabled: false,
                        entries: Default::default(),
                    }
                }
            },
            Err(_) => InstanceConfigFile::default(),
        }
    }

    /// 写实例配置文件（原子写）。
    pub(crate) fn write_instance_config(
        &self,
        slug: &str,
        cfg: &InstanceConfigFile,
    ) -> Result<(), PluginError> {
        let path = self.config_path(slug);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| PluginError::Io(format!("create config dir: {e}")))?;
        }
        let body = serde_json::to_string_pretty(cfg)
            .map_err(|e| PluginError::Io(format!("serialize: {e}")))?;
        atomic_write(&path, body.as_bytes())
            .map_err(|e| PluginError::Io(format!("write {}: {e}", path.display())))
    }

    fn instance_config(&self, reg: &RegisteredPlugin) -> InstanceConfig {
        let file = self.read_instance_config(&reg.manifest.slug);
        InstanceConfig {
            declared_keys: reg.manifest.config_schema_keys(),
            x_secret_keys: reg.manifest.permissions.x_secret.clone(),
            entries: file.entries,
        }
    }

    /// 设置实例配置键（key 必须在 manifest config-schema 声明内；值恒字符串）。
    pub fn set_config_key(&self, slug: &str, key: &str, value: &str) -> Result<(), PluginError> {
        let Some(reg) = self.get(slug) else {
            return Err(PluginError::NotAvailable(format!(
                "plugin not registered: {slug}"
            )));
        };
        if reg.manifest.permissions.x_secret.iter().any(|k| k == key) {
            return Err(PluginError::Manifest(format!(
                "'{key}' 是 x-secret 键，请走 vault（vault:plugin/{slug}/{key}）"
            )));
        }
        let declared = reg.manifest.config_schema_keys();
        if !declared.iter().any(|k| k == key) {
            return Err(PluginError::Manifest(format!(
                "key not declared in manifest config-schema: {key}（可声明键：{declared:?}）"
            )));
        }
        let _guard = self.config_io.lock();
        let mut cfg = self.read_instance_config(slug);
        cfg.entries.insert(key.to_string(), value.to_string());
        self.write_instance_config(slug, &cfg)
    }

    /// 清空实例配置键。
    pub fn unset_config_key(&self, slug: &str, key: &str) -> Result<(), PluginError> {
        if self.get(slug).is_none() {
            return Err(PluginError::NotAvailable(format!(
                "plugin not registered: {slug}"
            )));
        }
        let _guard = self.config_io.lock();
        let mut cfg = self.read_instance_config(slug);
        cfg.entries.remove(key);
        self.write_instance_config(slug, &cfg)
    }

    /// 读实例配置（WSAPI config get；调用方负责按 manifest x-secret 脱敏）。
    #[must_use]
    pub fn get_config(&self, slug: &str) -> InstanceConfigFile {
        self.read_instance_config(slug)
    }

    /// enable/disable（内存 + 落盘）。
    pub fn set_enabled_plugin(&self, slug: &str, on: bool) -> Result<(), PluginError> {
        let Some(reg) = self.get(slug) else {
            return Err(PluginError::NotAvailable(format!(
                "plugin not registered: {slug}"
            )));
        };
        reg.enabled.store(on, Ordering::SeqCst);
        let _guard = self.config_io.lock();
        let mut cfg = self.read_instance_config(slug);
        cfg.enabled = on;
        self.write_instance_config(slug, &cfg)
    }

    /// 磁盘形态 enable/disable（CLI 专用：CLI 进程不装载注册表，只操作盘上
    /// 状态，gateway 重启后 load_all/apply_enabled_from_file 生效）。slug
    /// 必须已在 lockfile——防幽灵实例配置文件凭空创建、装同 slug 时被陈旧
    /// 状态接管（2026-09-29 交付审查 3.2）。
    pub fn set_enabled_file_only(&self, slug: &str, on: bool) -> Result<(), PluginError> {
        if !crate::manifest::valid_slug(slug) {
            return Err(PluginError::Manifest(format!("invalid slug: {slug}")));
        }
        let lock_path = self.plugins_dir().join("lockfile.json");
        let known = std::fs::read_to_string(&lock_path)
            .ok()
            .and_then(|s| serde_json::from_str::<crate::install::PluginLockfile>(&s).ok())
            .is_some_and(|lf| lf.plugins.contains_key(slug));
        if !known {
            return Err(PluginError::NotAvailable(format!(
                "插件未安装: {slug}（plugin list 查看已装列表）"
            )));
        }
        let _guard = self.config_io.lock();
        let mut cfg = self.read_instance_config(slug);
        cfg.enabled = on;
        self.write_instance_config(slug, &cfg)
    }

    /// 从实例配置文件恢复启用位（启动装载用）。
    pub(crate) fn apply_enabled_from_file(&self, slug: &str, reg: &Arc<RegisteredPlugin>) {
        let cfg = self.read_instance_config(slug);
        reg.enabled.store(cfg.enabled, Ordering::SeqCst);
    }
}

/// 实例配置文件（`config/plugins/<slug>.json`）。
///
/// Default 手写而非 derive：缺省语义是「启用」（derive 的 bool 默认 false
/// 会把全新安装的插件全部静默禁用——e2e 抓出的真实回归）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstanceConfigFile {
    /// 启用位（缺省 true）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 普通配置键值（仅 manifest config-schema 声明键）。
    #[serde(default)]
    pub entries: std::collections::BTreeMap<String, String>,
}

impl Default for InstanceConfigFile {
    fn default() -> Self {
        Self {
            enabled: true,
            entries: std::collections::BTreeMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// 原子写（temp + rename；tmp 名带进程+毫秒后缀——并发写者不同名互不踩踏，
/// 固定名会在并发写下互相覆盖；失败清理 temp）。
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let stem = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp = path.with_file_name(format!(
        "{stem}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
