//! fresh-store-per-call 执行器（经宿主壳 DLL 的句柄 ABI）。
//!
//! 每次 execute / observe 都从全新 Store 开始（fuel/内存/表计数/宿主调用
//! 预算全部按帧重置；组件级状态跨帧留存只能写自己的 data 目录）。墙钟
//! 双闸：壳内 epoch/fuel 预算（归并 TIMEOUT 状态码）+ 外层 tokio timeout
//!（超长宿主调用兜底；超时后阻塞线程继续跑完，信号量持有到完成——防
//! zombie 实例挤爆 max_instances）。
//!
//! env 契约：每次调用 `Box::into_raw(Box<CallCaps>)` 作 env 传入壳；
//! 壳回调首参原样透传；`call_export`（一次性）+ `store_drop` 完成后
//! `Box::from_raw` 回收——回调只在 call_export 期间运行，时序封闭。

use std::sync::Arc;
use std::time::Duration;

use crate::bindings::{self, ToolMetadata};
use crate::error::PluginError;
use crate::host_impl::{
    CallCaps, FrameBudget, HostToolInvoker, InstanceConfig, PluginLogBuffer, SecretResolver,
};
use crate::limits::{InstanceGate, PluginLimits};
use crate::loader::HostDll;
use crate::manifest::PluginKind;

/// 一次调用的能力授予帧（async 侧构建，move 进阻塞线程）。
pub(crate) struct CallFrame {
    pub(crate) observer_frame: bool,
    pub(crate) slug: String,
    pub(crate) kind: PluginKind,
    /// 本插件生效限制表（全局上限 ∧ manifest 收紧；壳侧 store 的
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

/// 一次壳调用会话（env 生命周期管理；Drop 兜底回收 CallCaps）。
struct CallSession {
    store: u64,
    env: *mut std::os::raw::c_void,
    host: Arc<HostDll>,
}

impl CallSession {
    fn new(host: &Arc<HostDll>, component: u64, frame: &CallFrame) -> Result<Self, PluginError> {
        let caps = Box::new(CallCaps {
            slug: frame.slug.clone(),
            kind: frame.kind,
            observer_frame: frame.observer_frame,
            frame: FrameBudget::new(frame.limits.host_call_budget),
            logs: frame.logs.clone(),
            limits: frame.limits.clone(),
            config: frame.config.clone(),
            secrets: frame.secrets.clone(),
            workspace_root: frame.workspace_root.clone(),
            egress: frame.egress.clone(),
            invoker: frame.invoker.clone(),
            tokio_handle: tokio::runtime::Handle::try_current().ok(),
            audit: frame.audit.clone(),
            session_key: frame.session_key.clone(),
        });
        let env = Box::into_raw(caps) as *mut std::os::raw::c_void;
        match host.store_new(
            component,
            &frame.data_dir,
            frame.limits.memory_bytes,
            frame.limits.table_elements,
            frame.limits.fuel,
            frame.limits.timeout_ms,
            env,
        ) {
            Ok(store) => Ok(Self {
                store,
                env,
                host: host.clone(),
            }),
            Err(e) => {
                // store_new 失败：壳未接管 env，就地回收。
                drop(unsafe { Box::from_raw(env as *mut CallCaps) });
                Err(e)
            }
        }
    }

    fn call(&self, name: &str, args_json: &str, timeout_ms: u64) -> Result<String, PluginError> {
        self.host
            .call_export(self.store, name, args_json, timeout_ms)
    }
}

impl Drop for CallSession {
    fn drop(&mut self) {
        // 一次性语义：call_export 已消耗 store 时为容忍的 no-op；env 回收
        // 必须在 store_drop 之后（store 仍存活时壳可能触发回调）。
        self.host.store_drop(self.store);
        drop(unsafe { Box::from_raw(self.env as *mut CallCaps) });
    }
}

/// 执行器（PluginManager 内聚持有）。
pub(crate) struct PluginRuntime {
    pub(crate) host: Arc<HostDll>,
    pub(crate) gate: InstanceGate,
}

impl PluginRuntime {
    pub(crate) fn new(
        limits: PluginLimits,
        cache_root: Option<&std::path::Path>,
    ) -> Result<Self, PluginError> {
        let host = HostDll::open()?;
        host.engine_init(cache_root)?;
        Ok(Self {
            host,
            gate: InstanceGate::new(limits.max_instances),
        })
    }

    /// 用宿主壳编译一段 wasm 组件字节（句柄；0 = 无效）。
    pub(crate) fn compile_component(&self, wasm_bytes: &[u8]) -> Result<u64, PluginError> {
        self.host.component_new(wasm_bytes)
    }

    /// 一次壳调用的公共骨架（gate 许可 + env 会话 + 外层墙钟兜底）。
    async fn run_call<T, F>(&self, comp: u64, frame: CallFrame, f: F) -> Result<T, PluginError>
    where
        F: FnOnce(&CallSession) -> Result<T, PluginError> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.gate.try_acquire()?;
        let timeout_ms = frame.limits.timeout_ms;
        let host = self.host.clone();
        let handle = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let session = CallSession::new(&host, comp, &frame)?;
            f(&session)
        });
        tokio::time::timeout(Duration::from_millis(timeout_ms), handle)
            .await
            .map_err(|_| PluginError::Timeout { ms: timeout_ms })?
            .map_err(|e| PluginError::Trap(format!("join: {e}")))?
        // 已知边界（2026-09-29 交付审查 M3，形态不变）：外层超时返回后
        // spawn_blocking 闭包继续跑到 guest 自行结束（permit 随闭包释放）
        // ——epoch 闸只约束 wasm 执行，宿主调用（http-egress/tool-invoke）
        // 不受 epoch/fuel 约束，恶意插件可用「帧预算 × 无界宿主调用时长」
        // 把并发实例槽钉住。触发前提是插件已过签名/审批闸，属可用性面非
        // 安全面；v1 不做，见交付报告已知边界。
    }

    /// 安装期对账：探针 get-metadata（fresh-store + 全限制照常）。
    pub(crate) async fn probe_metadata(
        &self,
        comp: u64,
        mut frame: CallFrame,
    ) -> Result<ToolMetadata, PluginError> {
        frame.observer_frame = false;
        frame.kind = PluginKind::Tool;
        frame.invoker = None;
        let timeout_ms = frame.limits.timeout_ms;
        self.run_call(comp, frame, move |session| {
            let out = session.call("get-metadata", "{}", timeout_ms)?;
            bindings::parse_get_metadata(&out).map_err(|he| {
                PluginError::MetadataMismatch(format!("get-metadata host-error: {he:?}"))
            })
        })
        .await
    }

    /// 执行一次工具调用。
    pub(crate) async fn execute_tool(
        &self,
        comp: u64,
        mut frame: CallFrame,
        args_json: String,
        exec_ctx: ExecContext,
    ) -> Result<ExecOutput, PluginError> {
        frame.session_key = exec_ctx.session_key.clone();
        let timeout_ms = frame.limits.timeout_ms;
        let workspace_root = frame.workspace_root.to_string_lossy().to_string();
        self.run_call(comp, frame, move |session| {
            let input = serde_json::json!({
                "args-json": args_json,
                "context": {
                    "workspace-root": workspace_root,
                    "session-key": exec_ctx.session_key,
                    "call-id": exec_ctx.call_id,
                }
            })
            .to_string();
            let out = session.call("execute", &input, timeout_ms)?;
            match bindings::parse_execute(&out)
                .map_err(|he| PluginError::Trap(format!("execute decode: {he}")))?
            {
                Ok((content, is_error)) => Ok(ExecOutput { content, is_error }),
                // guest 显式返回 host-error（设计上只发生在非预期形态）；
                // 归约为业务错误文本回灌（is_error=true，非 trap）。
                Err(he) => Ok(ExecOutput {
                    content: format!("Tool error: plugin returned host-error: {he:?}"),
                    is_error: true,
                }),
            }
        })
        .await
    }

    /// 投递一条观察者事件（fresh-store-per-event）。
    pub(crate) async fn call_observe(
        &self,
        comp: u64,
        mut frame: CallFrame,
        event_json: String,
    ) -> Result<(), PluginError> {
        frame.observer_frame = true;
        frame.kind = PluginKind::Observer;
        frame.invoker = None;
        let timeout_ms = frame.limits.timeout_ms;
        self.run_call(comp, frame, move |session| {
            let out = session.call("observe", &event_json, timeout_ms)?;
            bindings::parse_observe(&out)
                .map_err(|he| PluginError::Trap(format!("observe decode: {he}")))?
                .map_err(|he| PluginError::Trap(format!("observe host-error: {he:?}")))
        })
        .await
    }
}
