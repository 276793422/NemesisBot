//! fresh-store-per-call 执行器。
//!
//! 每次 execute / observe 都从全新 Store 开始（fuel/内存/表计数/宿主调用
//! 预算全部按帧重置；组件级状态跨帧留存只能写自己的 data 目录）。
//! 墙钟双闸：epoch deadline（trap，线程干净结束）+ 外层 tokio timeout
//! （超长宿主调用兜底；超时后阻塞线程继续跑完，信号量持有到完成——防
//! zombie 实例挤爆 max_instances）。

use std::sync::Arc;
use std::time::Duration;

use crate::bindings::{observer as obs_bind, tool as tool_bind};
use crate::engine::epoch_deadline_ticks;
use crate::error::PluginError;
use crate::host_impl::{
    CallCaps, FrameBudget, HostToolInvoker, InstanceConfig, PluginCtx, PluginLogBuffer,
    SecretResolver, is_epoch_timeout, truncate_err,
};
use crate::limits::{InstanceGate, PluginLimits, PluginResourceLimiter};
use crate::manifest::PluginKind;

/// 一次调用的能力授予帧（async 侧构建，move 进阻塞线程）。
pub(crate) struct CallFrame {
    pub(crate) observer_frame: bool,
    pub(crate) slug: String,
    pub(crate) kind: PluginKind,
    /// 本插件生效限制表（全局上限 ∧ manifest 收紧；fresh_store 的
    /// fuel/内存/帧预算/超时全部取自这里——执行路径的单一真相源）。
    pub(crate) limits: PluginLimits,
    pub(crate) logs: Arc<PluginLogBuffer>,
    pub(crate) config: Arc<InstanceConfig>,
    pub(crate) secrets: Arc<dyn SecretResolver>,
    pub(crate) workspace_root: std::path::PathBuf,
    pub(crate) egress: Arc<crate::egress::EgressPolicy>,
    pub(crate) invoker: Option<Arc<dyn HostToolInvoker>>,
    pub(crate) data_dir: std::path::PathBuf,
    pub(crate) audit: Option<crate::host_impl::SharedAuditLogger>,
    pub(crate) session_key: String,
}

/// 工具执行入参上下文（桥层注入）。
#[derive(Debug, Clone, Default)]
pub struct ExecContext {
    /// 发起会话键（空 = 无会话上下文）。
    pub session_key: String,
    /// 本次调用 ID。
    pub call_id: String,
}

/// 工具执行结果。
#[derive(Debug, Clone)]
pub struct ExecOutput {
    /// 回给 LLM 的文本。
    pub content: String,
    /// 业务失败标记（guest 语义；非 trap）。
    pub is_error: bool,
}

/// fresh store 组装参数（三个入口共用）。
struct StoreParts {
    engine: wasmtime::Engine,
    limits: PluginLimits,
    frame: CallFrame,
}

fn fresh_store(parts: &StoreParts) -> Result<wasmtime::Store<PluginCtx>, PluginError> {
    let f = &parts.frame;
    let caps = Arc::new(CallCaps {
        slug: f.slug.clone(),
        kind: f.kind,
        observer_frame: f.observer_frame,
        frame: FrameBudget::new(parts.limits.host_call_budget),
        logs: f.logs.clone(),
        limits: parts.limits.clone(),
        config: f.config.clone(),
        secrets: f.secrets.clone(),
        workspace_root: f.workspace_root.clone(),
        egress: f.egress.clone(),
        invoker: f.invoker.clone(),
        tokio_handle: tokio::runtime::Handle::try_current().ok(),
        audit: f.audit.clone(),
        session_key: f.session_key.clone(),
    });
    // per-plugin data 目录 preopen（guest 挂载点恒 /data）；WasiCtx 空构造
    // ——无 stdio 管道、无环境变量、无网络，其余 fs/socket 全 deny。
    let mut builder = wasmtime_wasi::WasiCtx::builder();
    builder
        .preopened_dir(&f.data_dir, "/data", wasmtime_wasi::FsPerms::ReadWrite)
        .map_err(|e| PluginError::Io(format!("preopen data dir: {e}")))?;
    let wasi = builder.build();
    let limiter = PluginResourceLimiter::new(&parts.limits);
    let mut store = wasmtime::Store::new(
        &parts.engine,
        PluginCtx {
            wasi,
            table: wasmtime::component::ResourceTable::new(),
            caps,
            limiter,
        },
    );
    store.set_fuel(parts.limits.fuel).expect("fuel available");
    store.set_epoch_deadline(epoch_deadline_ticks(parts.limits.timeout_ms));
    store.limiter(|s| &mut s.limiter);
    Ok(store)
}

fn fresh_linker(engine: &wasmtime::Engine) -> wasmtime::component::Linker<PluginCtx> {
    let mut linker = wasmtime::component::Linker::new(engine);
    // 宿主能力面（两个 world 的 host 接口同名同型；组件只绑定自己导入的）。
    // 同名实例二次注册会 Err——当前两 world 实现同形、语义等价（同一 WIT
    // interface），但 WIT 演化失配时不能静默吞掉：warn 留痕，否则只会在
    // instantiate 期炸出难懂错误（2026-09-29 交付审查 L4）。
    use wasmtime::component::HasSelf;
    if let Err(e) =
        tool_bind::PluginTool::add_to_linker::<PluginCtx, HasSelf<PluginCtx>>(&mut linker, |s| s)
    {
        tracing::warn!(error = %e, "[WasmPlugin] tool world 宿主面注册 linker 失败（同名实例已被占用？）");
    }
    if let Err(e) =
        obs_bind::PluginObserver::add_to_linker::<PluginCtx, HasSelf<PluginCtx>>(&mut linker, |s| s)
    {
        tracing::warn!(error = %e, "[WasmPlugin] observer world 宿主面注册 linker 失败（同名实例已被占用？）");
    }
    // wasi 空壳（组件经 std 引入的 wasi:cli/random/clocks 导入兜底；无
    // preopen、无网络、无参数——宿主能力面之外 guest 什么也够不着）。
    if let Err(e) = wasmtime_wasi::p2::add_to_linker_sync(&mut linker) {
        tracing::warn!(error = %e, "[WasmPlugin] wasi p2 宿主面注册 linker 失败（同名实例已被占用？）");
    }
    linker
}

/// trap 错误归约（fuel/epoch → Timeout；其余 → Trap）。
fn classify_trap(e: wasmtime::Error, timeout_ms: u64) -> PluginError {
    if is_epoch_timeout(&e) {
        PluginError::Timeout { ms: timeout_ms }
    } else {
        PluginError::Trap(truncate_err(&e.to_string(), 500))
    }
}

/// 执行器（PluginManager 内聚持有）。
pub(crate) struct PluginRuntime {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) gate: InstanceGate,
}

impl PluginRuntime {
    pub(crate) fn new(
        limits: PluginLimits,
        cache_root: Option<&std::path::Path>,
    ) -> Result<Self, PluginError> {
        let engine = crate::engine::engine(cache_root)?;
        Ok(Self {
            engine,
            gate: InstanceGate::new(limits.max_instances),
        })
    }

    fn parts(&self, frame: CallFrame) -> StoreParts {
        // per-plugin 收紧表（manifest [limits] 生效点）：fuel/内存/帧预算/
        // 超时全部跟随帧，执行器全局表只剩引擎与信号量职责。
        let limits = frame.limits.clone();
        StoreParts {
            engine: self.engine.clone(),
            limits,
            frame,
        }
    }

    /// 安装期对账：探针 get-metadata（fresh-store + 全限制照常）。
    pub(crate) async fn probe_metadata(
        &self,
        comp: Arc<wasmtime::component::Component>,
        mut frame: CallFrame,
    ) -> Result<tool_bind::exports::nemesis::plugin::tool::ToolMetadata, PluginError> {
        frame.observer_frame = false;
        frame.kind = PluginKind::Tool;
        frame.invoker = None;
        let permit = self.gate.try_acquire()?;
        let parts = self.parts(frame);
        let timeout_ms = parts.frame.limits.timeout_ms;
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut store = fresh_store(&parts)?;
            let linker = fresh_linker(&parts.engine);
            let instance =
                tool_bind::PluginTool::instantiate(&mut store, &comp, &linker).map_err(|e| {
                    PluginError::Compile(format!(
                        "instantiate: {}",
                        truncate_err(&e.to_string(), 500)
                    ))
                })?;
            let tool = instance
                .nemesis_plugin_tool()
                .call_get_metadata(&mut store)
                .map_err(|e| classify_trap(e, timeout_ms))?
                .map_err(|he| {
                    PluginError::MetadataMismatch(format!("get-metadata host-error: {he:?}"))
                })?;
            Ok(tool)
        });

        tokio::time::timeout(Duration::from_millis(timeout_ms), handle)
            .await
            .map_err(|_| PluginError::Timeout { ms: timeout_ms })?
            .map_err(|e| PluginError::Trap(format!("join: {e}")))?
    }

    /// 执行一次工具调用。
    pub(crate) async fn execute_tool(
        &self,
        comp: Arc<wasmtime::component::Component>,
        mut frame: CallFrame,
        args_json: String,
        exec_ctx: ExecContext,
    ) -> Result<ExecOutput, PluginError> {
        frame.session_key = exec_ctx.session_key.clone();
        let permit = self.gate.try_acquire()?;
        let parts = self.parts(frame);
        let timeout_ms = parts.frame.limits.timeout_ms;
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut store = fresh_store(&parts)?;
            let linker = fresh_linker(&parts.engine);
            let instance = tool_bind::PluginTool::instantiate(&mut store, &comp, &linker)
                .map_err(|e| classify_trap(e, timeout_ms))?;
            let tool = instance.nemesis_plugin_tool();
            let input = tool_bind::exports::nemesis::plugin::tool::ToolInput {
                args_json,
                context: tool_bind::exports::nemesis::plugin::tool::ToolContext {
                    workspace_root: parts.frame.workspace_root.to_string_lossy().to_string(),
                    session_key: exec_ctx.session_key,
                    call_id: exec_ctx.call_id,
                },
            };
            let out = tool
                .call_execute(&mut store, &input)
                .map_err(|e| classify_trap(e, timeout_ms))?;
            // guest 显式返回 host-error（设计上只发生在非预期形态）；
            // 归约为业务错误文本回灌（is_error=true，非 trap）。
            let out = match out {
                Ok(o) => ExecOutput {
                    content: o.content,
                    is_error: o.is_error,
                },
                Err(he) => ExecOutput {
                    content: format!("Tool error: plugin returned host-error: {he:?}"),
                    is_error: true,
                },
            };
            Ok::<ExecOutput, PluginError>(out)
        });

        tokio::time::timeout(Duration::from_millis(timeout_ms), handle)
            .await
            .map_err(|_| PluginError::Timeout { ms: timeout_ms })?
            .map_err(|e| PluginError::Trap(format!("join: {e}")))?
        // 已知边界（2026-09-29 交付审查 M3）：外层超时返回后 spawn_blocking
        // 闭包继续跑到 guest 自行结束（permit 随闭包释放）——epoch 闸只约束
        // wasm 执行，宿主调用（http-egress/tool-invoke）不受 epoch/fuel 约束，
        // 恶意插件可用「帧预算 × 无界宿主调用时长」把并发实例槽钉住。触发
        // 前提是插件已过签名/审批闸，属可用性面非安全面；给宿主调用加独立
        // 时限会截断正常长任务（tool-invoke 下游是完整 agent dispatch），
        // v1 不做，见交付报告已知边界。
    }

    /// 投递一条观察者事件（fresh-store-per-event）。
    pub(crate) async fn call_observe(
        &self,
        comp: Arc<wasmtime::component::Component>,
        mut frame: CallFrame,
        event_json: String,
    ) -> Result<(), PluginError> {
        frame.observer_frame = true;
        frame.kind = PluginKind::Observer;
        frame.invoker = None;
        let permit = self.gate.try_acquire()?;
        let parts = self.parts(frame);
        let timeout_ms = parts.frame.limits.timeout_ms;
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut store = fresh_store(&parts)?;
            let linker = fresh_linker(&parts.engine);
            let instance = obs_bind::PluginObserver::instantiate(&mut store, &comp, &linker)
                .map_err(|e| classify_trap(e, timeout_ms))?;
            let observer = instance.nemesis_plugin_observer();
            observer
                .call_observe(&mut store, &event_json)
                .map_err(|e| classify_trap(e, timeout_ms))?
                .map_err(|he| PluginError::Trap(format!("observe host-error: {he:?}")))?;
            Ok(())
        });

        tokio::time::timeout(Duration::from_millis(timeout_ms), handle)
            .await
            .map_err(|_| PluginError::Timeout { ms: timeout_ms })?
            .map_err(|e| PluginError::Trap(format!("join: {e}")))?
    }
}
