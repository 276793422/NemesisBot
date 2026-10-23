//! 紫微系统 WSAPI handler（总控 §1/§8/§9；T0.5 空壳 + 门关拒绝面）。
//!
//! 命令清单（l1 注册表全量登记）：
//! - 会话/资产面：`ziwei.list` / `show` / `attach` / `detach` / `invoke` /
//!   `status` / `remove` / `refresh`
//! - 配置期向导：`ziwei.setup.select_target` / `snapshot` / `analyze` /
//!   `diff_window` / `candidates` / `annotate` / `define_struct` / `commit`
//!
//! 双闸（对齐 terminal 双闸模式）：
//! 1. cargo feature `ziwei`（关 = 本 handler 整体不注册，命令 unknown）；
//! 2. config `ziwei.enabled`（默认 false = 全部命令诚实拒绝）。
//!
//! 里程碑实装面：list/show/status/remove = M0；attach/detach = M1；
//! invoke（poll 一次性快照）/refresh（动态工具重注册钩子）= M2；
//! setup.* = M3。refresh 经 `set_refresh_hook` 桥（依赖方向：nemesis-web
//! 不依赖 nemesisbot，钩子由 gateway 装配时安装）。

use std::sync::{Arc, OnceLock};

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_ziwei::profile::{Primitive, ProfileStore};

/// 门关拒绝文案（IT 契约断言串，改动须同步 integration-test）。
pub const GATE_CLOSED_MSG: &str = "紫微系统未启用（config ziwei.enabled=false）";

/// 动态工具重注册钩子（gateway 装配时安装；签名 = workspace → 注册数）。
type RefreshHook = Arc<dyn Fn(&std::path::Path) -> usize + Send + Sync>;
static REFRESH_HOOK: OnceLock<RefreshHook> = OnceLock::new();

/// gateway 装配点安装钩子（幂等；重复 set 忽略后到者）。
pub fn set_refresh_hook(hook: RefreshHook) -> bool {
    REFRESH_HOOK.set(hook).is_ok()
}

/// 触发动态工具重注册（WSAPI `ziwei.refresh` / setup.commit /
/// `/api/internal ziwei_setup` 共用；钩子未装配 = None）。
pub(crate) fn run_refresh_hook(workspace: &std::path::Path) -> Option<usize> {
    REFRESH_HOOK.get().map(|h| h(workspace))
}

/// 一次性 poll 快照读（WSAPI `ziwei.invoke` / CLI `ziwei_invoke` 同源；
/// call_fn/hook_capture 诚实报 M4）。
pub(crate) async fn invoke_poll_tool(
    manager: &Arc<nemesis_ziwei::session::SessionManager>,
    store: &ProfileStore,
    profile_id: &str,
    tool_name: &str,
) -> Result<serde_json::Value, String> {
    let profile = store.load(profile_id).map_err(|e| e.to_string())?;
    let tool = profile
        .tools
        .iter()
        .find(|t| t.tool_name == tool_name)
        .ok_or_else(|| format!("工具 {tool_name} 不存在（profile {profile_id}）"))?;
    if tool.action.primitive != Primitive::PollReport {
        return Err(format!(
            "工具 {tool_name} 非 poll_report（invoke 路由已按原语分派——本函数只收快照读）"
        ));
    }
    let session = manager
        .get(profile_id)
        .ok_or_else(|| format!("会话未附加（profile {profile_id}）：先 attach"))?;
    let ds_name = tool
        .action
        .addr
        .strip_prefix("struct:")
        .ok_or_else(|| format!("工具 {tool_name} 地址形态非法（{}）", tool.action.addr))?;
    let addr = session.struct_addr(ds_name).unwrap_or(0);
    if addr == 0 {
        return Err(format!(
            "结构 {ds_name} 地址未解析（attach 未提供 struct_addrs；M3 起由锚自动解析）"
        ));
    }
    let ds = profile
        .data_structures
        .iter()
        .find(|s| s.name == ds_name)
        .ok_or_else(|| format!("结构 {ds_name} 定义不存在"))?;
    let snapshot = manager.read_struct(profile_id, addr, ds).await?;
    Ok(serde_json::json!({
        "tool": tool_name,
        "profile": profile_id,
        "struct": ds_name,
        "data": snapshot,
    }))
}

pub struct ZiweiHandler;

impl ZiweiHandler {
    fn workspace(ctx: &RequestContext) -> Result<std::path::PathBuf, String> {
        ctx.workspace
            .clone()
            .or_else(|| ctx.state.workspace.clone())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "workspace 未知（gateway 未装配）".to_string())
    }

    /// data 里可选 `struct_addrs: {"Name": <u64>}`（M2 显式注入；M3 起锚解析）。
    fn struct_addrs(data: Option<&serde_json::Value>) -> std::collections::HashMap<String, u64> {
        let mut out = std::collections::HashMap::new();
        if let Some(map) = data
            .and_then(|d| d.get("struct_addrs"))
            .and_then(|v| v.as_object())
        {
            for (k, v) in map {
                if let Some(addr) = v.as_u64() {
                    out.insert(k.clone(), addr);
                }
            }
        }
        out
    }

    /// M5 verify/recover 的目标 exe 路径解析：`data.exe` 显式优先，缺省 =
    /// profile.target.exe_path_hint；两者皆缺 = 诚实报错。
    fn resolve_exe(
        data: &serde_json::Value,
        profile: &nemesis_ziwei::profile::Profile,
    ) -> Result<std::path::PathBuf, String> {
        if let Some(exe) = data.get("exe").and_then(|v| v.as_str()) {
            return Ok(std::path::PathBuf::from(exe));
        }
        profile
            .target
            .exe_path_hint
            .as_deref()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                "目标 exe 路径未知（data.exe 与 profile.target.exe_path_hint 均缺）".to_string()
            })
    }
}

/// 急停活性闸（同族先例：board 的 `refuse_dispatch_when_estopped`，EST-01/02
/// 横扫加固）：estop 生效中拒绝紫微活性面（attach 注入 / invoke 远程调用 /
/// resolve 活体扫描）。estop 卸载（nemesis-ziwei estop_hook）只在触发上升沿
/// 执行一次——闸住 estop 期间新建会话 = 堵「estop 中 attach 的会话永不被
/// 卸载」的洞。
fn refuse_active_when_estopped(ctx: &RequestContext) -> Result<(), String> {
    if ctx
        .state
        .estop
        .as_ref()
        .map(|e| e.is_engaged())
        .unwrap_or(false)
    {
        return Err("⛔ 急停（E-STOP）生效中：紫微注入/调用面被拒绝（先释放急停）".to_string());
    }
    Ok(())
}

#[async_trait::async_trait]
impl ModuleHandler for ZiweiHandler {
    fn module_name(&self) -> &str {
        "ziwei"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "show",
            "attach",
            "detach",
            "invoke",
            "resolve",
            "status",
            "remove",
            "refresh",
            "verify",
            "recover",
            "recover_confirm",
            "setup.select_target",
            "setup.snapshot",
            "setup.analyze",
            "setup.diff_window",
            "setup.candidates",
            "setup.annotate",
            "setup.define_struct",
            "setup.commit",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        if !self.commands().contains(&cmd) {
            return Err(format!("unknown command: ziwei.{cmd}"));
        }
        // 运行闸：config ziwei.enabled（默认 false）。**直读磁盘文件**（不走
        // live 缓存）——热开闸语义：gateway 运行中改 config.json 即刻生效
        // （update_config 落盘写穿，文件即真相源；IT ziwei 套件依赖本语义）。
        let home = ctx.home.clone().ok_or("home 未知")?;
        let cfg = nemesis_config::load_config(&super::config::config_path(&home))
            .map_err(|e| format!("failed to load config: {e}"))?;
        let zcfg = cfg.ziwei.clone().unwrap_or_default();
        if !zcfg.enabled {
            return Err(GATE_CLOSED_MSG.to_string());
        }
        // 急停活性闸：estop 生效中拒绝触及目标进程的命令（attach 注入 /
        // invoke 远程调用 / resolve 活体扫描）。紫微的 estop 卸载只在触发
        // 上升沿跑一次（nemesis-ziwei estop_hook）——不闸 attach 的话，
        // estop 期间新建的会话在下降沿前永不被卸载；estop 期间向目标进程
        // 注入 payload 也直接违背急停冻结语义。同族先例：board 的
        // refuse_dispatch_when_estopped（EST-01/02 横扫加固）。
        // 放行面：detach（降险动作，与 estop 卸载同向）、list/show/status/
        // remove/verify/recover*/setup.*（离线本地件）、refresh（agent 侧
        // 工具注册元数据——estop 已冻结 dispatch，注册的工具调不动）。
        if matches!(cmd, "attach" | "invoke" | "resolve") {
            refuse_active_when_estopped(ctx)?;
        }

        match cmd {
            // ── 只读面（数据层 T0.4 即 backing）──
            "list" => {
                let store = ProfileStore::new(&Self::workspace(ctx)?);
                let profiles = store.list().map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({ "profiles": profiles })))
            }
            "show" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("profile_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?
                    .to_string();
                let store = ProfileStore::new(&Self::workspace(ctx)?);
                let profile = store.load(&id).map_err(|e| e.to_string())?;
                let anchors = store.load_anchors(&id).ok();
                let notes = store.load_notes(&id).unwrap_or_default();
                Ok(Some(serde_json::json!({
                    "profile": profile,
                    "anchors": anchors,
                    "notes": notes,
                })))
            }
            "status" => {
                // 会话面（M1 实装）：SessionManager 全局槽快照；未装配 = 空表诚实。
                let sessions = nemesis_ziwei::session::global()
                    .map(|m| m.list_sessions())
                    .unwrap_or_default();
                let info = nemesis_ziwei::setup::engine_info();
                let setup_draft = nemesis_ziwei::setup::global().status(&Self::workspace(ctx)?);
                Ok(Some(serde_json::json!({
                    "enabled": true,
                    "manager_present": nemesis_ziwei::session::global().is_some(),
                    "sessions": sessions,
                    "setup_engine": info,
                    "setup_draft": setup_draft,
                })))
            }
            // ── 资产删除（用户显式动作；数据层在位）──
            "remove" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("profile_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?
                    .to_string();
                let store = ProfileStore::new(&Self::workspace(ctx)?);
                store.delete(&id).map_err(|e| e.to_string())?;
                tracing::info!("[ziwei] profile {} 已删除（WSAPI）", id);
                Ok(Some(serde_json::json!({ "removed": id })))
            }
            // ── 运行期面（M1：attach/detach；M2：invoke/refresh）──
            "attach" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("profile_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?
                    .to_string();
                let pid = data
                    .as_ref()
                    .and_then(|d| d.get("pid"))
                    .and_then(|v| v.as_u64())
                    .ok_or("缺 pid")? as u32;
                let manager = nemesis_ziwei::session::global()
                    .ok_or("紫微会话管理器未装配（gateway 未启用或非 Windows 平台）")?;
                let store = ProfileStore::new(&Self::workspace(ctx)?);
                let profile = store.load(&id).map_err(|e| e.to_string())?;
                // attach + 按需 poll 调度一站式（sink/capture 在 manager 上，
                // gateway 装配时已注入；无 sink = 只附加不调度）。
                let session = nemesis_ziwei::scheduler::attach_and_schedule(
                    &manager,
                    &profile,
                    pid,
                    Self::struct_addrs(data.as_ref()),
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok(Some(serde_json::json!({ "session": session.snapshot() })))
            }
            "detach" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("profile_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?
                    .to_string();
                let manager = nemesis_ziwei::session::global()
                    .ok_or("紫微会话管理器未装配（gateway 未启用或非 Windows 平台）")?;
                let msg = manager.detach(&id).await?;
                Ok(Some(serde_json::json!({ "detached": id, "message": msg })))
            }
            "invoke" => {
                // M2：poll_report 一次性快照；M4 起 call_fn / hook_capture
                // 同口 invoke（统一实现在 nemesis_ziwei::runtime——与 agent
                // 子工具桥同源，两入口语义恒一致）。
                let obj = data.as_ref().ok_or("缺 data")?;
                let id = obj
                    .get("profile_id")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?
                    .to_string();
                let tool_name = obj
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 tool")?
                    .to_string();
                let manager = nemesis_ziwei::session::global()
                    .ok_or("紫微会话管理器未装配（gateway 未启用或非 Windows 平台）")?;
                let store = ProfileStore::new(&Self::workspace(ctx)?);
                let profile = store.load(&id).map_err(|e| e.to_string())?;
                let tool = profile
                    .tools
                    .iter()
                    .find(|t| t.tool_name == tool_name)
                    .ok_or_else(|| format!("工具 {tool_name} 不存在（profile {id}）"))?;
                match tool.action.primitive {
                    Primitive::PollReport => Ok(Some(
                        invoke_poll_tool(&manager, &store, &id, &tool_name).await?,
                    )),
                    Primitive::CallFn | Primitive::HookCapture => {
                        #[cfg(windows)]
                        {
                            let semantic =
                                tool.action.addr.strip_prefix("fn:").ok_or_else(|| {
                                    format!(
                                        "工具 {tool_name} 地址形态非法（{}）：运行原语须 fn:语义名",
                                        tool.action.addr
                                    )
                                })?;
                            let v = match tool.action.primitive {
                                Primitive::CallFn => {
                                    let fn_args = match obj.get("args") {
                                        Some(a) => nemesis_ziwei::runtime::callfn_args_from_slice(
                                            a.as_array().ok_or(
                                                "args 须为整数数组（call_fn v1 只接受整数位置参数）",
                                            )?,
                                        )?,
                                        None => [0; 4],
                                    };
                                    let timeout_ms =
                                        tool.action.timeout_ms.unwrap_or(5_000).min(60_000) as u32;
                                    let mut out = nemesis_ziwei::runtime::invoke_call_fn(
                                        &manager,
                                        &Self::workspace(ctx)?,
                                        &id,
                                        semantic,
                                        fn_args,
                                        timeout_ms,
                                    )
                                    .await?;
                                    if let Some(o) = out.as_object_mut() {
                                        o.insert("tool".into(), serde_json::json!(tool_name));
                                    }
                                    out
                                }
                                _ => {
                                    let mut out = nemesis_ziwei::runtime::invoke_hook_capture(
                                        &manager,
                                        &Self::workspace(ctx)?,
                                        &id,
                                        semantic,
                                        tool.output.as_ref().map(|o| o.dest.as_str()),
                                        tool.action.max_records.unwrap_or(200),
                                    )
                                    .await?;
                                    if let Some(o) = out.as_object_mut() {
                                        o.insert("tool".into(), serde_json::json!(tool_name));
                                    }
                                    out
                                }
                            };
                            Ok(Some(v))
                        }
                        #[cfg(not(windows))]
                        {
                            Err("紫微系统运行时原语仅支持 Windows 平台".to_string())
                        }
                    }
                }
            }
            "resolve" => {
                // M4：函数锚批量解析（byte_pattern 活体扫描 / rva 直算）→
                // 会话 fn_addrs；报告含 notes 诚实清单。
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("profile_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?
                    .to_string();
                #[cfg(windows)]
                {
                    let manager = nemesis_ziwei::session::global()
                        .ok_or("紫微会话管理器未装配（gateway 未启用）")?;
                    Ok(Some(manager.resolve_functions(&id).await?))
                }
                #[cfg(not(windows))]
                {
                    let _ = id;
                    Err("紫微系统运行时仅支持 Windows 平台".to_string())
                }
            }
            "refresh" => {
                // M2：动态工具重注册（profile 新增/编辑后；钩子由 gateway 安装）。
                let hook = REFRESH_HOOK
                    .get()
                    .ok_or("动态工具刷新钩子未装配（gateway 未启用 ziwei）")?;
                let n = hook(&Self::workspace(ctx)?);
                Ok(Some(serde_json::json!({ "registered": n })))
            }
            // ── 验证链 / 找回（M5 实装：T5.1/T5.2/T5.3）──
            "verify" => {
                let obj = data.as_ref().ok_or("缺 data")?;
                let id = obj
                    .get("profile_id")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?;
                let workspace = Self::workspace(ctx)?;
                let store = ProfileStore::new(&workspace);
                let profile = store.load(id).map_err(|e| e.to_string())?;
                let exe = Self::resolve_exe(obj, &profile)?;
                let out =
                    nemesis_ziwei::verify::verify_profile(&store, &workspace, &profile, &exe)?;
                if let Some(updated) = &out.profile_updated {
                    store.save(updated, None).map_err(|e| e.to_string())?;
                }
                Ok(Some(serde_json::json!({
                    "report": out.report,
                    "report_path": out.report_path.map(|p| p.display().to_string()),
                })))
            }
            "recover" => {
                let obj = data.as_ref().ok_or("缺 data")?;
                let id = obj
                    .get("profile_id")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?;
                let workspace = Self::workspace(ctx)?;
                let store = ProfileStore::new(&workspace);
                let profile = store.load(id).map_err(|e| e.to_string())?;
                let exe = Self::resolve_exe(obj, &profile)?;
                let out = nemesis_ziwei::verify::recover_start(&store, &workspace, id, &exe)?;
                Ok(Some(out))
            }
            "recover_confirm" => {
                let obj = data.as_ref().ok_or("缺 data")?;
                let id = obj
                    .get("profile_id")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 profile_id")?;
                let accept: Vec<String> = obj
                    .get("accept")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .ok_or("缺 accept（接受的函数语义名数组；全部接受须显式列出）")?;
                let workspace = Self::workspace(ctx)?;
                let store = ProfileStore::new(&workspace);
                let profile = store.load(id).map_err(|e| e.to_string())?;
                let exe = Self::resolve_exe(obj, &profile)?;
                // 授权找回 = 用户显式 confirm（Dashboard 按钮 / CLI 子命令即
                // 凭据；agent 通道走 ziwei_bridge 的 force_approval）。
                let out =
                    nemesis_ziwei::verify::recover_confirm(&store, &workspace, id, &exe, &accept)?;
                Ok(Some(out))
            }
            // ── 配置期向导（M3 实装；SetupManager 全局槽惰性自建）──
            "setup.select_target" => {
                let pid = data
                    .as_ref()
                    .and_then(|d| d.get("pid"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                let exe = data
                    .as_ref()
                    .and_then(|d| d.get("exe"))
                    .and_then(|v| v.as_str());
                let out = nemesis_ziwei::setup::global()
                    .select_target(&Self::workspace(ctx)?, pid, exe)
                    .await?;
                Ok(Some(out))
            }
            "setup.snapshot" => Ok(Some(
                nemesis_ziwei::setup::global()
                    .snapshot(&Self::workspace(ctx)?)
                    .await?,
            )),
            "setup.analyze" => Ok(Some(
                nemesis_ziwei::setup::global().analyze(&Self::workspace(ctx)?)?,
            )),
            "setup.diff_window" => {
                let wait = data
                    .as_ref()
                    .and_then(|d| d.get("wait_secs"))
                    .and_then(|v| v.as_u64());
                let hook = data
                    .as_ref()
                    .and_then(|d| d.get("hook"))
                    .and_then(|v| v.as_bool());
                Ok(Some(
                    nemesis_ziwei::setup::global()
                        .diff_window(&Self::workspace(ctx)?, wait, hook)
                        .await?,
                ))
            }
            "setup.candidates" => Ok(Some(
                nemesis_ziwei::setup::global().candidates(&Self::workspace(ctx)?)?,
            )),
            "setup.annotate" => {
                let obj = data.as_ref().ok_or("缺 data")?;
                Ok(Some(
                    nemesis_ziwei::setup::global().annotate(&Self::workspace(ctx)?, obj)?,
                ))
            }
            "setup.define_struct" => {
                let obj = data.as_ref().ok_or("缺 data")?;
                Ok(Some(
                    nemesis_ziwei::setup::global().define_struct(&Self::workspace(ctx)?, obj)?,
                ))
            }
            "setup.commit" => {
                let ws = Self::workspace(ctx)?;
                let mut out = nemesis_ziwei::setup::global().commit(&ws)?;
                // 提交后动态工具重注册（钩子在位才生效；CLI 同语义）
                if let Some(n) = run_refresh_hook(&ws)
                    && let Some(obj) = out.as_object_mut()
                {
                    obj.insert("tools_registered".into(), serde_json::json!(n));
                }
                Ok(Some(out))
            }
            _ => Err(format!("unknown command: ziwei.{cmd}")),
        }
    }
}

#[cfg(all(test, feature = "ziwei"))]
mod tests;
