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
use crate::host_impl::{
    InstanceConfig, LlmInvoker, PluginLogBuffer, SecretResolver, plugin_error_text,
};
use crate::limits::{PluginLimits, PolicyPoint};
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

/// 策略接管快照（注册进宿主的唯一事实源；来自安装期 get-policy-metadata
/// 对账）。runtime 路由按点查这里；`插件名单` → 点集的投影供 WSAPI 行。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PolicyMetaSnapshot {
    /// 接管 compact-check（触发 + 摘要生成）。
    pub compact: bool,
    /// 接管 judge-final（终答裁决）。
    pub final_judge: bool,
    /// 接管 validate-args（工具参数校验）。
    pub validation: bool,
}

impl PolicyMetaSnapshot {
    /// 接管点清单（≥1 才算合法策略组件）。
    #[must_use]
    pub fn points(&self) -> Vec<PolicyPoint> {
        let mut v = Vec::new();
        if self.compact {
            v.push(PolicyPoint::Compact);
        }
        if self.final_judge {
            v.push(PolicyPoint::FinalJudge);
        }
        if self.validation {
            v.push(PolicyPoint::Validation);
        }
        v
    }

    /// 本快照是否接管某点。
    #[must_use]
    pub fn takes(&self, point: PolicyPoint) -> bool {
        match point {
            PolicyPoint::Compact => self.compact,
            PolicyPoint::FinalJudge => self.final_judge,
            PolicyPoint::Validation => self.validation,
        }
    }
}

/// 观察者事件队列发送端（宿主持有于 observers map；drop 即关 worker）。
type ObserverSender = tokio::sync::mpsc::Sender<String>;

/// 已注册插件。
pub struct RegisteredPlugin {
    /// manifest。
    pub manifest: PluginManifest,
    /// 信任四态（安装/启动时验签结论）。
    pub trust: PluginTrustState,
    /// 编译产物（宿主壳 DLL 句柄；0 = 无效）。
    pub component: u64,
    /// 工具元数据（kind=tool 必有）。
    pub tool_meta: Option<ToolMetaSnapshot>,
    /// 策略接管快照（kind=policy 必有）。
    pub policy_meta: Option<PolicyMetaSnapshot>,
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
    /// 杂务 LLM 补全面（gateway 装配后注入；policy 帧消费）。
    llm: Mutex<Option<Arc<dyn LlmInvoker>>>,
    /// 子系统总开关（config `plugins.enabled`；false = 全部拒绝执行/停投）。
    enabled: AtomicBool,
    /// 审计器（gateway 装配后注入；None = 敏感宿主调用静默跳过审计）。
    audit: Mutex<Option<crate::host_impl::SharedAuditLogger>>,
    /// 实例配置文件读-改-写互斥（set_config_key/unset/enable 与 CLI 磁盘
    /// 启停共用；防同进程并发写互相踩踏。跨进程无闸——CLI vs gateway 是
    /// 已知边界，见交付报告）。
    config_io: Mutex<()>,
}

/// 解析 wasm 插件载荷目录（生产路径）。优先级：env `NEMESIS_WASM_PLUGINS_DIR`
/// 权威 → exe 旁 `plugins_wasm/` → 开发路径 `../../plugins_wasm`、
/// `../../../plugins_wasm`（首个存在者；全缺席 = 落 exe 旁并创建）。
/// CLI 与 gateway 同 exe → 双方解析天然一致；用户在 exe 旁一眼可见插件目录
/// （与 native DLL 的 `plugins/` 平级、命名区分）。测试不走此函数——构造
/// 参数注入（并行测试 env 互踩，env 仅作生产覆盖通道）。
#[must_use]
pub fn resolve_plugins_wasm_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var("NEMESIS_WASM_PLUGINS_DIR")
        && !env_dir.trim().is_empty()
    {
        let p = PathBuf::from(env_dir);
        let _ = std::fs::create_dir_all(&p);
        return p;
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe_dir) = &exe_dir {
        candidates.push(exe_dir.join("plugins_wasm"));
        // 开发形态：target/debug/nemesisbot.exe（两级）与
        // target/debug/deps/…（测试二进制，三级）→ 仓库根 plugins_wasm/。
        candidates.push(exe_dir.join("..").join("..").join("plugins_wasm"));
        candidates.push(
            exe_dir
                .join("..")
                .join("..")
                .join("..")
                .join("plugins_wasm"),
        );
    }
    for c in &candidates {
        if c.is_dir() {
            return c.clone();
        }
    }
    // 全缺席 = 落 exe 旁（无 exe 信息落 cwd）并创建。
    let fallback = exe_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join("plugins_wasm");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

/// 旧载荷一次性迁移（2026-10-22 前落位点 `<workspace>/plugins/`）：整树
/// 并入解析出的 `plugins_wasm/`。触发条件 = 旧树 `lockfile.json` 在场；
/// 同名冲突（目标 slug 目录/lockfile 已在）= 该条目留守 + WARN。迁毕清理
/// 残留旧目录（韧性删除，Windows 句柄占用重试）。幂等：旧 lockfile 不在
/// 场即空操作。
fn migrate_legacy_payloads(workspace_root: &Path, plugins_dir: &Path) {
    let legacy = workspace_root.join("plugins");
    if !legacy.join("lockfile.json").exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return;
    };
    // read_dir 顺序是平台实现细节——显式排序保证迁移顺序确定性。
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    let mut moved = 0usize;
    for name in names {
        let from = legacy.join(&name);
        let to = plugins_dir.join(&name);
        if to.exists() {
            tracing::warn!(
                slug = %name,
                target = %to.display(),
                "[WasmPlugin] 旧载荷迁移冲突：目标已在，该条目留守旧位（手工合并）"
            );
            continue;
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => moved += 1,
            Err(_) => {
                // 跨卷 rename 不支持 → copy + 韧性删除兜底。
                match copy_dir_recursive(&from, &to) {
                    Ok(()) => match crate::install::resilient_remove_dir_all(&from) {
                        Ok(_) => moved += 1,
                        Err(e) => tracing::warn!(
                            slug = %name,
                            error = %e,
                            "[WasmPlugin] 旧载荷复制成功但旧目录清理失败（副本留守）"
                        ),
                    },
                    Err(e) => tracing::warn!(
                        slug = %name,
                        error = %e,
                        "[WasmPlugin] 旧载荷迁移失败，该条目留守旧位"
                    ),
                }
            }
        }
    }
    // 残留目录清理：全迁走才删（有留守 = 保现场）。
    let leftover = std::fs::read_dir(&legacy).map(|d| d.count()).unwrap_or(1);
    if leftover == 0
        && let Err(e) = crate::install::resilient_remove_dir_all(&legacy)
    {
        tracing::warn!(error = %e, "[WasmPlugin] 旧插件目录清理失败（空目录残留，无害）");
    }
    tracing::info!(
        from = %legacy.display(),
        to = %plugins_dir.display(),
        moved,
        "[WasmPlugin] 旧插件载荷已迁移至 plugins_wasm/（安装历史 lockfile 一并迁入）"
    );
}

/// 递归拷目录（迁移跨卷兜底；stderr 静默，错误上抛）。
fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

impl PluginManager {
    /// 构建管理器（生产路径：载荷目录走 [`resolve_plugins_wasm_dir`]——
    /// exe 旁 `plugins_wasm/`，用户一眼可见）。宿主壳自动选路 + 进程缓存
    /// 装载；生产 gateway 装配走 [`Self::new_with_host_path`]（闸获准路径
    /// 定点装载——验签对象即装载对象）。壳缺席 = `NotAvailable`（诚实降级）。
    /// 测试一律走 [`Self::new_with_dirs`]（显式注入，杜绝共享真实路径串扰）。
    pub fn new(
        workspace_root: &Path,
        limits: PluginLimits,
        secrets: Arc<dyn SecretResolver>,
    ) -> Result<Self, PluginError> {
        let plugins_dir = resolve_plugins_wasm_dir();
        Self::new_with_dirs(workspace_root, &plugins_dir, limits, secrets)
    }

    /// 注入形态（测试 / 多实例专用）：载荷目录显式给定，不碰进程级解析。
    pub fn new_with_dirs(
        workspace_root: &Path,
        plugins_dir: &Path,
        limits: PluginLimits,
        secrets: Arc<dyn SecretResolver>,
    ) -> Result<Self, PluginError> {
        let (workspace_root, plugins_dir, data_root, cache_root) =
            Self::build_dirs(workspace_root, plugins_dir)?;
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
            llm: Mutex::new(None),
            enabled: AtomicBool::new(true),
            audit: Mutex::new(None),
            config_io: Mutex::new(()),
        })
    }

    /// 定点构建（R1 整改）：宿主壳从**闸获准路径**装载（native_plugins
    /// enforce 验签通过的那个文件），绕过自动选路与进程装载缓存。载荷目录
    /// 走生产解析。装载/符号绑定/ABI 握手失败 = `NotAvailable`（调用方诚实
    /// 降级）。
    pub fn new_with_host_path(
        workspace_root: &Path,
        limits: PluginLimits,
        secrets: Arc<dyn SecretResolver>,
        host_path: &Path,
    ) -> Result<Self, PluginError> {
        let plugins_dir = resolve_plugins_wasm_dir();
        Self::new_with_host_path_and_dirs(workspace_root, &plugins_dir, limits, secrets, host_path)
    }

    /// 定点构建 + 注入载荷目录（测试形态）。
    pub fn new_with_host_path_and_dirs(
        workspace_root: &Path,
        plugins_dir: &Path,
        limits: PluginLimits,
        secrets: Arc<dyn SecretResolver>,
        host_path: &Path,
    ) -> Result<Self, PluginError> {
        let (workspace_root, plugins_dir, data_root, cache_root) =
            Self::build_dirs(workspace_root, plugins_dir)?;
        let host = crate::loader::open_at(host_path)?;
        let runtime = PluginRuntime::with_host(limits.clone(), Some(&cache_root), host)?;
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
            llm: Mutex::new(None),
            enabled: AtomicBool::new(true),
            audit: Mutex::new(None),
            config_io: Mutex::new(()),
        })
    }

    /// 目录装配（构造器共享）：workspace canonicalize + 载荷目录确保在场 +
    /// 旧载荷一次性迁移，返回各推导根。
    fn build_dirs(
        workspace_root: &Path,
        plugins_dir: &Path,
    ) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), PluginError> {
        let workspace_root = workspace_root
            .canonicalize()
            .map_err(|e| PluginError::Io(format!("workspace root: {e}")))?;
        std::fs::create_dir_all(plugins_dir)
            .map_err(|e| PluginError::Io(format!("create plugins dir: {e}")))?;
        let plugins_dir = plugins_dir
            .canonicalize()
            .map_err(|e| PluginError::Io(format!("plugins dir: {e}")))?;
        migrate_legacy_payloads(&workspace_root, &plugins_dir);
        let data_root = workspace_root.join("plugin-data");
        let cache_root = workspace_root.join("cache").join("wasm");
        std::fs::create_dir_all(&data_root)
            .map_err(|e| PluginError::Io(format!("create plugin-data dir: {e}")))?;
        Ok((workspace_root, plugins_dir, data_root, cache_root))
    }

    /// 装配宿主工具调用面（gateway 在 agent loop 就绪后调用）。
    pub fn set_invoker(&self, invoker: Arc<dyn crate::host_impl::HostToolInvoker>) {
        *self.invoker.lock() = Some(invoker);
    }

    /// 装配杂务 LLM 补全面（gateway 在 agent loop 就绪后调用；policy 帧消费。
    /// None 保持 = llm-complete 恒 unavailable）。
    pub fn set_llm(&self, llm: Arc<dyn LlmInvoker>) {
        *self.llm.lock() = Some(llm);
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

    /// 用宿主壳编译一段 wasm 组件字节（含 workspace 编译缓存收益）。
    pub fn compile_component(&self, wasm_bytes: &[u8]) -> Result<u64, PluginError> {
        self.runtime.compile_component(wasm_bytes)
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
    /// kind=observer 会同时拉起事件 worker（有界队列，满丢弃+计数）；
    /// kind=policy 做**唯一接管者对账**——同策略点已有其他注册组件 = 诚实
    /// 拒绝（运行期每点仅一个激活组件；displaced 的旧组件由调用方先
    /// uninstall/upgrade 走漏斗）。
    pub fn register(
        self: &Arc<Self>,
        manifest: PluginManifest,
        trust: PluginTrustState,
        component: u64,
        tool_meta: Option<ToolMetaSnapshot>,
        policy_meta: Option<PolicyMetaSnapshot>,
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
        if let Some(pm) = policy_meta.as_ref() {
            if *manifest.kind != PluginKind::Policy {
                return Err(PluginError::Manifest(
                    "policy metadata provided for non-policy plugin".into(),
                ));
            }
            let points = pm.points();
            if points.is_empty() {
                return Err(PluginError::MetadataMismatch(
                    "policy component must take over at least one policy point".into(),
                ));
            }
            // 唯一接管者对账：现注册表中任一点已被别的组件接管 = 拒绝。
            for reg in self.list() {
                let Some(existing) = reg.policy_meta.as_ref() else {
                    continue;
                };
                let clash: Vec<&str> = points
                    .iter()
                    .filter(|p| existing.takes(**p))
                    .map(|p| p.export_name())
                    .collect();
                if !clash.is_empty() {
                    return Err(PluginError::MetadataMismatch(format!(
                        "policy point(s) {} already taken by plugin '{}'",
                        clash.join("/"),
                        reg.manifest.slug
                    )));
                }
            }
        } else if *manifest.kind == PluginKind::Policy {
            return Err(PluginError::MetadataMismatch(
                "policy plugin missing metadata snapshot".into(),
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
            component,
            tool_meta,
            policy_meta,
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
        let removed = self.plugins.write().remove(slug);
        if let Some(reg) = removed.as_ref() {
            // 释放编译产物（壳对 0/重复 drop 容忍）。
            self.runtime.host.component_drop(reg.component);
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
            llm: self.llm.lock().clone(),
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
            .execute_tool(reg.component, frame, args_json, exec_ctx)
            .await
    }

    /// 找出接管某策略点的启用组件（桥层路由入口；None = 无接管 → 调用方
    /// 走内置实现）。
    #[must_use]
    pub fn policy_owner(&self, point: PolicyPoint) -> Option<Arc<RegisteredPlugin>> {
        self.list().into_iter().find(|reg| {
            *reg.manifest.kind == PluginKind::Policy
                && reg.enabled.load(Ordering::SeqCst)
                && reg.policy_meta.is_some_and(|pm| pm.takes(point))
        })
    }

    /// 找出接管某内置工具的启用插件（桥层路由入口；None = 无接管 → 调用方
    /// 走内置实现）。多个插件声明同名接管 = slug 字典序最小者胜 + WARN
    /// 留痕——注册表是无序 HashMap（「注册序」无从谈起），装配漏斗也不做
    /// 接管排他校验，双声明在此确定性仲裁。
    #[must_use]
    pub fn takeover_owner(&self, builtin: &str) -> Option<Arc<RegisteredPlugin>> {
        let cands: Vec<Arc<RegisteredPlugin>> = self
            .list()
            .into_iter()
            .filter(|reg| {
                *reg.manifest.kind == PluginKind::Tool
                    && reg.enabled.load(Ordering::SeqCst)
                    && reg.manifest.takeover.iter().any(|t| t == builtin)
            })
            .collect();
        match cands.len() {
            0 => None,
            1 => cands.into_iter().next(),
            _ => {
                let mut sorted = cands;
                sorted.sort_by(|a, b| a.manifest.slug.cmp(&b.manifest.slug));
                tracing::warn!(
                    builtin = %builtin,
                    slugs = %sorted
                        .iter()
                        .map(|r| r.manifest.slug.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                    "[WasmPlugin] 多个插件声明接管同一内置工具（仲裁=slug 字典序最小者生效；漏斗不排他，建议安装期人工裁决）"
                );
                sorted.into_iter().next()
            }
        }
    }

    /// 调用一次循环策略点（桥层入口；接管者缺席/禁用/子系统总开关关 =
    /// `NotAvailable`——调用方按「无接管」语义回退内置）。
    pub async fn call_policy_point(
        &self,
        point: PolicyPoint,
        input_json: String,
    ) -> Result<String, PluginError> {
        let Some(reg) = self.policy_owner(point) else {
            return Err(PluginError::NotAvailable(format!(
                "no active policy plugin takes {}",
                point.export_name()
            )));
        };
        if !self.is_enabled() {
            return Err(PluginError::NotAvailable(
                "wasm plugin subsystem disabled".into(),
            ));
        }
        let frame = self.build_frame(&reg, false);
        self.runtime
            .call_policy(reg.component, frame, point, input_json)
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
                // 队列缺位只发生在 register 的摘旧插新窗口（L301 remove →
                // insert 之间 enqueue 见 reg 在 map、队列还没有）。静默丢弃
                // 是设计（fire-and-forget），但 Observer 类要补计数——否则
                // 「事件去哪了」不可观测（2026-09-30 复查 #8）。tool 类本无
                // 队列，计数会误导，不补。
                if *reg.manifest.kind == PluginKind::Observer {
                    reg.dropped_events.fetch_add(1, Ordering::Relaxed);
                }
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
                let comp = reg.component;
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
    // 开关与实例配置（`config.plugin.wasm.json`，单一真相源在
    // `nemesis-config::plugin_wasm`——本 crate 只经它读写，不直接落盘）
    // -----------------------------------------------------------------------

    /// 开关文件所在目录（`<workspace>/config`）。
    fn config_dir(&self) -> PathBuf {
        self.workspace_root.join("config")
    }

    /// 读 slug 的开关 + 实例配置。缺省 = 未启用 + 空 config（wasm 纪律
    /// 「没明确开就不加载」；与 native 插件的缺省启用相反）。旧 per-slug
    /// 实例文件的并入由 plugin_wasm 读路径惰性完成（enabled 强制 false，
    /// config entries 保留）。
    #[must_use]
    pub fn get_switch(&self, slug: &str) -> nemesis_config::plugin_wasm::WasmSwitch {
        nemesis_config::plugin_wasm::switch(&self.config_dir(), slug)
    }

    fn instance_config(&self, reg: &RegisteredPlugin) -> InstanceConfig {
        let sw = self.get_switch(&reg.manifest.slug);
        InstanceConfig {
            declared_keys: reg.manifest.config_schema_keys(),
            x_secret_keys: reg.manifest.permissions.x_secret.clone(),
            entries: sw
                .config
                .into_iter()
                .map(|(k, v)| (k, value_to_config_string(v)))
                .collect(),
        }
    }

    /// 盘上载荷目录的 manifest（未加载也可读——config-schema 校验与
    /// 列表投影都靠它，不要求注册表在场）。
    pub fn disk_manifest(&self, slug: &str) -> Result<PluginManifest, PluginError> {
        let (m, _) = PluginManifest::load_from_dir(&self.plugin_dir(slug))?;
        Ok(m)
    }

    /// 设置实例配置键（key 必须在 manifest config-schema 声明内；值恒字符串）。
    /// 未加载的插件也可设置——开关开（装载）时生效（读路径同源）。
    pub fn set_config_key(&self, slug: &str, key: &str, value: &str) -> Result<(), PluginError> {
        if !crate::manifest::valid_slug(slug) {
            return Err(PluginError::Manifest(format!("invalid slug: {slug}")));
        }
        let manifest = self.disk_manifest(slug)?;
        if manifest.permissions.x_secret.iter().any(|k| k == key) {
            return Err(PluginError::Manifest(format!(
                "'{key}' 是 x-secret 键，请走 vault（vault:plugin/{slug}/{key}）"
            )));
        }
        let declared = manifest.config_schema_keys();
        if !declared.iter().any(|k| k == key) {
            return Err(PluginError::Manifest(format!(
                "key not declared in manifest config-schema: {key}（可声明键：{declared:?}）"
            )));
        }
        let _guard = self.config_io.lock();
        let mut sw = self.get_switch(slug);
        sw.config.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
        nemesis_config::plugin_wasm::set_config(&self.config_dir(), slug, sw.config)
            .map_err(PluginError::Io)
    }

    /// 清空实例配置键（未加载也可清；键不在场 = 幂等成功）。
    pub fn unset_config_key(&self, slug: &str, key: &str) -> Result<(), PluginError> {
        if !crate::manifest::valid_slug(slug) {
            return Err(PluginError::Manifest(format!("invalid slug: {slug}")));
        }
        let _guard = self.config_io.lock();
        let mut sw = self.get_switch(slug);
        if sw.config.remove(key).is_none() {
            return Ok(());
        }
        nemesis_config::plugin_wasm::set_config(&self.config_dir(), slug, sw.config)
            .map_err(PluginError::Io)
    }

    /// 读开关全量（WSAPI config get；调用方负责按 manifest x-secret 脱敏）。
    #[must_use]
    pub fn get_config(&self, slug: &str) -> nemesis_config::plugin_wasm::WasmSwitch {
        self.get_switch(slug)
    }
}

/// config 值 → guest 可读字符串（实例配置恒以字符串形态入帧；非字符串
/// JSON 值序列化为其字面文本）。
fn value_to_config_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
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
