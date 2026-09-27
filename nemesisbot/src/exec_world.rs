//! U10 统一执行世界 —— gateway / CLI 共用的 executor 通道装配 + workflow
//! 引擎的 [`ExecutionWorld`] 桥。
//!
//! 背景（为什么存在）：2026-07-17 的「script 走 exec 路径」改造把 gateway 的
//! workflow script 节点接到了 agent 工具注册表（经 AgentToolAdapter →
//! RemoteExecutorTool → executor 子进程 / Sandboxie 盒），但仍有三类执行不经过
//! 任何执行环境抽象：
//!
//! 1. **CLI `nemesisbot workflow run`** 用裸 `WorkflowEngine::new()`，script
//!    节点落到 `ScriptNodeExecutor::new()` 的裸 `Command::new` —— 生产可达的
//!    沙盒逃逸路径（executor.sandbox=true 也拦不住）。
//! 2. **引擎自身 IO**（persist_workflow / persist_execution / delete）直接写
//!    磁盘，workflow_name 不清洗时存在路径穿越面。
//! 3. 各装配点（agent / workflow / 未来的 tool-plugin）各写一套 spawn 语义。
//!
//! 本模块提供两个出口：
//! - [`build_executor_channel`]：从 agent_factory 抽出的**单一真相源**装配函数
//!   （live ConfigStore probe + Sandboxie 就绪检查 + 优雅降级），gateway 的
//!   agent 工具层与 workflow 引擎共用同一套就绪/probe 逻辑（各自持有独立的
//!   `ExecutorChannel` 实例——它只是无状态配置持有器，per-call spawn，共享无益）。
//! - [`build_workflow_world`]（`sandbox` feature 门控）：把通道包成
//!   [`ExecutorWorld`]（实现 `nemesis_sandbox::exec_world::ExecutionWorld`），
//!   交给 `WorkflowEngine::set_execution_world`：
//!   - **Tool 车道**（`supports_tool_calls() = true`）：无工具注册表的装配
//!     （CLI）经 `spawn_and_call("run_script", …)` 走 executor 子进程/盒——
//!     与 agent `exec` 同一开关链；
//!   - **Spawn 车道**：per-node `sandbox: false` 显式 opt-out 的受守卫直跑
//!     （cwd 根 + env 清洗 + CREATE_NO_WINDOW + 超时，见
//!     `nemesis_sandbox::exec_world::guarded_direct_spawn`）；
//!   - **writable_roots**：引擎控制面写盘守卫（persist/delete 拒绝根外写）。
//!
//! 诚实边界：
//! - `sandbox` feature 编译期裁掉时本模块的 world 部分（含 CLI 的沙盒路由）
//!   一并裁掉——gateway 的 script 节点仍经注册表车道走 ExecutorChannel
//!   （Layer 1 不依赖本模块），但 CLI 裸引擎回退裸 spawn（与裁剪前行为一致，
//!   IoT/minimal 构建本就不装 Sandboxie）。U11（Linux 用户态沙盒）落地时
//!   把 SandboxBackend 挂进 nemesis-sandbox 后可解除此门控。
//! - Tool 车道的 `RequestContext` 是合成的静态标识（channel=workflow），
//!   不承载会话语义——executor 子进程只把它用于日志/安全层上下文。
//! - spawn 语义（`SpawnSemantics`）是**描述轴**：描述当前装配，不驱动分支；
//!   分支由每次调用时 live probe 决定（见 ExecutorChannel::spawn_and_call）。

use std::path::Path;
use std::sync::Arc;

use nemesis_config::ConfigHandle;

/// 构建 executor 分离通道（单一真相源，gateway agent 层与 workflow 引擎共用）。
///
/// 返回 `Ok(None)` = `executor.enabled = false`（Layer 0：无通道）。
/// 就绪语义与优雅降级和 agent 工具层完全一致：
/// - `sandbox` feature + Sandboxie 就绪（Start.exe 存在 + SbieSvc Running +
///   engine_owned）→ 通道带 `start_exe`（Layer 2 盒）；未就绪 → stdio（Layer 1）
///   + warn，不崩（gateway 不被 SbieSvc 状态绑架）。
/// - `executor.sandbox` 的实时翻转由注入的 probe 闭包承担（每次工具调用读
///   ConfigStore，不重启进程生效）。
///
/// P5-2 严格模式（`executor.strict`，默认 false=现状字节不变）：通道在所有
/// 分支都挂 [`nemesis_agent::StrictGate`]——闸门 live 读 strict，false 秒过；
/// true 时对"要求沙盒"的调用做就绪性复检，不过则**拒绝执行**（fail-closed，
/// 见 `spawn_and_call`），不再静默降级。按平台：
/// - Windows：Sandboxie 就绪 + 构造时确实挂上了 Start.exe（引擎在 Agent
///   启动后才就绪的窗口里通道没有 start_exe——闸门必须连构造结果一起验，
///   否则会放行一个根本进不了盒的通道；此时提示 `sandbox start` + 重启）；
/// - 非 Windows：用户态后端可用性（`detect_backend`；Partial 算可用，缺口
///   在日志/状态如实标注——严格模式保证「有盒」，不保证「盒无能力缺口」）；
/// - trim 构建（`sandbox` feature 被裁）：本构建不可能有盒 → 严格时恒拒。
///
/// `home` 只在 `sandbox` feature 开启时用于 Sandboxie 就绪检查（trim 构建下
/// 允许未使用）。
#[cfg_attr(not(feature = "sandbox"), allow(unused_variables))]
pub fn build_executor_channel(
    home: &Path,
    workspace_dir: &Path,
    config_handle: ConfigHandle,
) -> anyhow::Result<Option<Arc<nemesis_agent::ExecutorChannel>>> {
    let enabled = config_handle
        .read()
        .executor
        .as_ref()
        .is_some_and(|e| e.enabled);
    if !enabled {
        return Ok(None);
    }

    let exe_path = std::env::current_exe()
        .map_err(|err| anyhow::anyhow!("resolve current_exe for executor: {err}"))?;
    let workspace = workspace_dir.to_string_lossy().to_string();

    // Live sandbox probe: read the ConfigStore on EVERY tool call so toggling
    // executor.sandbox (dashboard stop/start, config edit) takes effect WITHOUT
    // a gateway restart.
    // ⚠ 闭包必须用 clone 出来的句柄（strict/lease/net 四个探针同款）——直接
    // move `config_handle` 本体会让后文的 `.clone()` 全部 E0382。
    let sandbox_handle = config_handle.clone();
    let strict_handle = config_handle.clone();
    let sandbox_probe: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        sandbox_handle
            .read()
            .executor
            .as_ref()
            .is_some_and(|ec| ec.sandbox)
    });
    // Live strict probe (P5-2): the gate re-reads this per call, so flipping
    // executor.strict takes effect on the next tool call — no restart.
    let strict_now: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        strict_handle
            .read()
            .executor
            .as_ref()
            .is_some_and(|ec| ec.strict)
    });
    // WS9/P22：租约透传探针——子进程不读用户 config，`agents.lease_enabled`
    // 的装配语义由父进程在 spawn 时经 `NEMESISBOT_LEASE=1` 钉死。live 读与
    // 严格闸门同款（租约实际是装配期字段，重启才变；这里 live 读保持家族
    // 一致，无额外代价）。
    let lease_handle = config_handle.clone();
    let lease_child_now: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || lease_handle.read().agents.lease_enabled);

    // P24（2026-09-26）：Windows 盒缺位时的用户态 ACL 降级选型——**显式
    // `executor.backend = "acl"`** 且 AclBackend 本机可用才可顶（子进程
    // engage 同判据 opt-in 自装）。auto/未知值/不可用 → false 维持现状裸
    // spawn（auto 不回落实验档——默认行为字节不变）。仅 fail-open 增强：
    // strict 闸门（上面）仍只认 Sandboxie 引擎——ACL 是 Partial 实验档，
    // 不算 strict 合格沙盒。
    #[cfg(all(feature = "sandbox", windows))]
    let userland_fallback: nemesis_agent::UserlandFallback = {
        let fallback_handle = config_handle.clone();
        Arc::new(move || {
            use nemesis_sandbox::backend::{Availability, SandboxBackend};
            let choice = fallback_handle
                .read()
                .executor
                .as_ref()
                .map(|ec| ec.backend.clone());
            if !matches!(
                nemesis_sandbox::backend::parse_executor_backend(choice.as_deref()),
                nemesis_sandbox::backend::ExecutorBackendChoice::Acl
            ) {
                return false;
            }
            !matches!(
                nemesis_sandbox::backend::AclBackend::new().availability(),
                Availability::Unavailable(_)
            )
        })
    };

    // ── D3/D4（2026-09-27）：DACL 定向档 spawn hook ─────────────────────────
    // live 读 `executor.acl.dacl`（热生效，与 sandbox_probe 同链路）：
    // - 关 → `Ok(None)`：走现状路径（字节不变）；
    // - 开 → 懒铸造 write-restricted 受限令牌（一次；结果含失败也缓存——
    //   铸造失败是机器态，逐调用重试只会刷屏）：
    //   * 成功 → `Ok(Some(spawn_fn))`：`stdio_txn_raw` 事务（CreateProcessAsUserW
    //     + DETACHED_PROCESS——受限令牌下新 console 分配必死，见 token.rs 实证；
    //     stdio 全管道，executor 协议无需 console）；
    //   * 失败 + `acl.strict` → `Err`：fail-closed 拒绝（不降级）；
    //   * 失败 + 宽松 → warn + `Ok(None)`：降级现状路径（完整性标签档/裸 spawn）。
    // feature 门控：`acl` 裁剪 / 非 Windows 构建不注入 hook（agent 侧 `None`
    // = 现状），`sandbox` feature 裁剪时 `config_handle` 仍在但本机无沙盒面，
    // 同样不注入。
    #[cfg(all(feature = "sandbox", windows))]
    let dacl_hook: Option<nemesis_agent::DaclSpawnHook> = {
        let hook_handle = config_handle.clone();
        let ws_root = workspace_dir.to_path_buf();
        // 令牌缓存：OnceLock 语义 = 只铸一次（成功与失败都定格）；错误串
        // 克隆返回，后续调用不重复铸造。
        let token_cell: Arc<
            std::sync::OnceLock<Result<Arc<nemesis_sandbox::backend::WriteRestrictedToken>, String>>,
        > = Arc::new(std::sync::OnceLock::new());
        Some(Arc::new(move || {
            let dacl_on = hook_handle
                .read()
                .executor
                .as_ref()
                .is_some_and(|ec| ec.acl.dacl);
            if !dacl_on {
                return Ok(None);
            }
            let strict_on = hook_handle
                .read()
                .executor
                .as_ref()
                .is_some_and(|ec| ec.acl.strict);
            // 就绪性探针（D4 状态面同源）：机器不支持 → 按降级链处理。
            if let nemesis_sandbox::backend::Availability::Unavailable(reason) =
                nemesis_sandbox::backend::dacl_availability()
            {
                let msg = format!("workspace-dacl backend unavailable: {reason}");
                return if strict_on { Err(msg) } else { Ok(None) };
            }
            // 受限输出捕获残留清扫（D4，2026-09-28 复查改位）：exec 工具在
            // 受限模式下输出落 workspace/.exec_out/（std 匿名管道受限下创建
            // 必死——loop_tools ShellOutputCapture；基线 DELETE 恢复后受限侧
            // 读回自删可成功，本清扫降级为崩溃残留兜底）。gateway（普通令牌）
            // **每次 spawn 前**清 1 小时以上旧文件（与 loop_tools 注释「
            // DaclSpawnFn spawn 前清超龄文件」对齐——首版放在令牌铸造一次性
            // 块里，长会话残留要等进程重启才清）；目录不存在 = 首跑无残留。
            // 尽力而为不阻塞装配。
            let exec_out = ws_root.join(".exec_out");
            if let Ok(rd) = std::fs::read_dir(&exec_out) {
                let cutoff =
                    std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
                for e in rd.flatten() {
                    let stale = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .map(|t| t < cutoff)
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let token = token_cell
                .get_or_init(|| {
                    let sid = match nemesis_sandbox::backend::derive_workspace_sid(&ws_root) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!("[Executor] workspace-dacl SID 派生失败: {e}");
                            return Err(e);
                        }
                    };
                    let token = match nemesis_sandbox::backend::create_write_restricted_token(&sid)
                    {
                        Ok(t) => Arc::new(t),
                        Err(e) => {
                            tracing::warn!("[Executor] workspace-dacl 令牌铸造失败: {e}");
                            return Err(e);
                        }
                    };
                    // standing GRANT ACE 树（D2）——受限令牌的 restricting 侧在
                    // 工作区内的**放行凭据**：不铺树则双合取检查在工作区内也拒
                    // （写操作全死，不只树外）。幂等（已达标对象只读跳过）；
                    // 目录 ACE 带 OI|CI 继承，新建文件自动获得——一次铺树够用。
                    // 失败与令牌铸造失败同链降级（strict → fail-closed Err；
                    // 宽松 → warn + Ok(None) 走现状路径）。
                    let max_files = hook_handle
                        .read()
                        .executor
                        .as_ref()
                        .map(|ec| ec.acl.max_files as usize)
                        .unwrap_or(20_000);
                    if let Err(e) =
                        nemesis_sandbox::backend::ensure_grant_ace_tree(&ws_root, &sid, max_files)
                    {
                        tracing::warn!("[Executor] workspace-dacl standing ACE 树铺设失败: {e}");
                        return Err(e);
                    }
                    tracing::info!("[Executor] workspace-dacl 令牌+ACE 树就绪（ws_sid={sid}）");
                    Ok(token)
                })
                .clone();
            let token = match token {
                Ok(t) => t,
                Err(e) => {
                    let msg = format!("write-restricted token mint failed: {e}");
                    return if strict_on { Err(msg) } else { Ok(None) };
                }
            };
            let spawn_fn: nemesis_agent::DaclSpawnFn = Arc::new(move |req| {
                use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
                let token = token.clone();
                let out = nemesis_sandbox::backend::stdio_txn_raw(
                    &token,
                    &req.exe,
                    &req.args,
                    &req.env_extra,
                    &req.request_line,
                    DETACHED_PROCESS,
                    req.timeout,
                )?;
                Ok(nemesis_agent::DaclSpawnOutcome {
                    response: out.response,
                    exit_code: out.exit_code,
                    stderr_tail: out.stderr_tail,
                })
            });
            Ok(Some(spawn_fn))
        }))
    };

    // Sandboxie Layer-2 attach decision (feature-gated; computed on every
    // platform that compiles the feature — on non-Windows Start.exe never
    // exists under home, so it falls through to the stdio path).
    // F2（2026-09-26 复查，契约注记）：**显式 `executor.backend="acl"` 不抢
    // 盒**——盒就绪时盒照常上岗，本判定不看 backend；ACL 档只走下面的
    // userland_fallback 钩子（盒缺位的 stdio 通道才咨询，见 P24 契约与
    // nemesis-sandbox backend 模块决策表）。读代码者勿把「显式 acl 被盒
    // 压制」当 bug——降级选型语义本就如此。
    #[cfg(feature = "sandbox")]
    let (start_exe, sbiesvc_running, engine_owned_now): (std::path::PathBuf, bool, bool) = {
        let paths = nemesis_sandbox::SandboxPaths::new(home);
        (
            paths.start_exe(),
            matches!(
                nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE),
                nemesis_sandbox::status::ServiceState::Running
            ),
            nemesis_sandbox::status::engine_owned(&paths),
        )
    };

    // ── P5-2 strict gate, per platform (see fn docs) ─────────────────────────
    // The attach decision must be made ONCE here — the gate captures what
    // construction actually attaches, not just the path on disk (an engine that
    // came up AFTER this agent started leaves the channel box-less even though
    // Start.exe now exists; strict must refuse that, not wave it through).
    #[cfg(feature = "sandbox")]
    let will_attach: bool = start_exe.exists() && sbiesvc_running && engine_owned_now;

    #[cfg(all(feature = "sandbox", windows))]
    let strict_gate: nemesis_agent::StrictGate = {
        let home = home.to_path_buf();
        let attached_start_exe: Option<std::path::PathBuf> = if will_attach {
            Some(start_exe.clone())
        } else {
            None
        };
        Arc::new(move || {
            if !strict_now() {
                return Ok(());
            }
            // 构造时没挂上 Start.exe：这个通道物理上进不了盒（哪怕引擎现在
            // 已就绪）——严格模式拒绝并要求重启（重建通道）。
            let Some(attached_path) = &attached_start_exe else {
                return Err(
                    "this agent was started while Sandboxie was not ready — its executor \
                     channel has no box attached; run `nemesisbot sandbox start`, then \
                     restart the agent"
                        .to_string(),
                );
            };
            let sbiesvc_running = matches!(
                nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE),
                nemesis_sandbox::status::ServiceState::Running
            );
            let owned =
                nemesis_sandbox::status::engine_owned(&nemesis_sandbox::SandboxPaths::new(&home));
            if attached_path.exists() && sbiesvc_running && owned {
                Ok(())
            } else {
                Err(format!(
                    "Sandboxie engine not ready (Start.exe present: {}, SbieSvc \
                     running: {sbiesvc_running}, engine owned: {owned}) — run `nemesisbot \
                     sandbox start`, then restart the agent",
                    attached_path.exists(),
                ))
            }
        })
    };
    #[cfg(all(feature = "sandbox", not(windows)))]
    let strict_gate: nemesis_agent::StrictGate = {
        // P1（2026-09-25）：选型上下文与 exec_worker::engage 同源——live 读
        // executor.allow_network 再选后端（禁网 + bwrap 可用 → bwrap；这里
        // 只关心存在性，detect 不返回 Unavailable 后端）。
        let net_handle = config_handle.clone();
        let allow_network_now = move || {
            net_handle
                .read()
                .executor
                .as_ref()
                .is_some_and(|e| e.allow_network)
        };
        Arc::new(move || {
            if !strict_now() {
                return Ok(());
            }
            match nemesis_sandbox::backend::detect_backend(allow_network_now()) {
                // 只关心存在性（detect 不返回 Unavailable 后端）；名字留给日志层。
                Some(_) => Ok(()),
                None => Err(
                    "no userland sandbox backend available (landlock + bwrap both unavailable) \
                     — install bubblewrap (or run on a landlock kernel) to use strict mode"
                        .to_string(),
                ),
            }
        })
    };
    #[cfg(not(feature = "sandbox"))]
    let strict_gate: nemesis_agent::StrictGate = Arc::new(move || {
        if !strict_now() {
            return Ok(());
        }
        Err(
            "the 'sandbox' feature is not compiled into this build — executor.sandbox \
             cannot be honoured (use a full build for strict mode)"
                .to_string(),
        )
    });

    // start_exe is fixed at startup (the path never changes). Attach it only if
    // Sandboxie is actually ready now; otherwise leave None and the probe-driven
    // path picks stdio.
    #[cfg(feature = "sandbox")]
    {
        if will_attach {
            tracing::info!(
                "[Executor] executor separation enabled (sandbox = live probe via \
                 ConfigStore, Start.exe box available): child {}",
                exe_path.display()
            );
            let channel = nemesis_agent::ExecutorChannel::new(exe_path, workspace, sandbox_probe)
                .with_start_exe(start_exe)
                .with_home(home.to_path_buf())
                .with_strict_gate(strict_gate)
                .with_lease_child((lease_child_now)());
            // D3：DACL hook 挂载（盒在场时 agent 侧 start_exe 闸保证不咨询）。
            #[cfg(all(feature = "sandbox", windows))]
            let channel = match dacl_hook {
                Some(hook) => channel.with_dacl_spawn(hook),
                None => channel,
            };
            return Ok(Some(Arc::new(channel)));
        }
        tracing::warn!(
            "[Executor] Sandboxie not ready (Start.exe exists={}, SbieSvc running={}). \
             executor.sandbox is still honoured live via the ConfigStore probe, but \
             without Start.exe the box is not applied{}.",
            start_exe.exists(),
            sbiesvc_running,
            if nemesis_config::load_live()
                .map(|c| c.executor.is_some_and(|e| e.strict))
                .unwrap_or(false)
            {
                " — executor.strict is ON: sandboxed tool calls will be REFUSED until \
                 the engine is started and the agent restarted"
            } else {
                ""
            }
        );
    }
    #[cfg(not(feature = "sandbox"))]
    {
        tracing::warn!(
            "[Executor] executor.sandbox is honoured live (ConfigStore probe), but \
             the 'sandbox' feature is not compiled into this build — tools run via \
             stdio, no box"
        );
    }

    tracing::info!(
        "[Executor] executor separation enabled (sandbox = live probe via ConfigStore, \
         stdio transport): {}",
        exe_path.display()
    );
    let channel = nemesis_agent::ExecutorChannel::new(exe_path, workspace, sandbox_probe)
        .with_home(home.to_path_buf())
        .with_strict_gate(strict_gate)
        .with_lease_child((lease_child_now)())
        // P24：Windows 盒缺位时的 ACL 降级选型钩子（其余平台不注入，
        // 判定面不存在 → 现状不变）。
        .with_userland_fallback({
            #[cfg(all(feature = "sandbox", windows))]
            {
                userland_fallback
            }
            #[cfg(not(all(feature = "sandbox", windows)))]
            {
                Arc::new(|| false)
            }
        });
    // D3：DACL 定向档 spawn hook（cfg 门控同 userland_fallback；未挂 = 现状
    // 字节不变）。
    #[cfg(all(feature = "sandbox", windows))]
    let channel = match dacl_hook {
        Some(hook) => channel.with_dacl_spawn(hook),
        None => channel,
    };
    Ok(Some(Arc::new(channel)))
}

// ---------------------------------------------------------------------------
// ExecutionWorld 桥（sandbox feature 门控 —— 见模块文档「诚实边界」）
// ---------------------------------------------------------------------------
#[cfg(feature = "sandbox")]
mod world {
    use super::*;
    use std::path::PathBuf;

    /// `ExecutorChannel` → `ExecutionWorld` 桥。
    ///
    /// 与 `AgentRunner` 桥（nemesis-workflow ↔ gateway）同构：trait 落在
    /// 被依赖 crate（nemesis-sandbox，跨平台中立），实现落在能看见
    /// `ExecutorChannel` 的装配 crate（nemesisbot）。
    pub struct ExecutorWorld {
        channel: Arc<nemesis_agent::ExecutorChannel>,
        /// 控制面写盘守卫根（引擎 persist/delete 只允许写这些子树）。
        writable_roots: Vec<PathBuf>,
        /// 受守卫直跑（Spawn 车道）的 cwd 根。
        spawn_roots: Vec<PathBuf>,
    }

    impl ExecutorWorld {
        /// 当前是否走 Sandboxie 盒（live probe + 通道是否带 Start.exe）。
        fn boxed_now(&self) -> bool {
            (self.channel.sandbox_probe)() && self.channel.start_exe.is_some()
        }
    }

    #[async_trait::async_trait]
    impl nemesis_sandbox::exec_world::ExecutionWorld for ExecutorWorld {
        fn name(&self) -> &str {
            "executor-channel"
        }

        fn writable_roots(&self) -> Vec<PathBuf> {
            self.writable_roots.clone()
        }

        fn spawn_semantics(&self) -> nemesis_sandbox::exec_world::SpawnSemantics {
            use nemesis_sandbox::exec_world::SpawnSemantics;
            if self.boxed_now() {
                SpawnSemantics::SandboxBoxed
            } else {
                SpawnSemantics::ExecutorChild
            }
        }

        fn supports_tool_calls(&self) -> bool {
            true
        }

        async fn run(
            &self,
            op: nemesis_sandbox::exec_world::ExecOp,
        ) -> Result<nemesis_sandbox::exec_world::ExecOutcome, String> {
            match op {
                // Tool 车道：经 executor 子进程跑工具。`spawn_and_call` 内部
                // 每次 live probe 决定 stdio（Layer 1）还是命名管道 + Start.exe
                // 盒（Layer 2）。run_script 成功时恒返回结构化 JSON
                // {stdout,stderr,exit_code}（失败也编码在 exit_code 里），
                // 这里归一化成 ExecOutcome；只有传输/超时层失败才是 Err。
                nemesis_sandbox::exec_world::ExecOp::Tool(t) => {
                    let ctx = nemesis_agent::context::RequestContext::new(
                        "workflow",
                        "workflow-engine",
                        "workflow",
                        "workflow",
                    );
                    let result = self.channel.spawn_and_call(&t.tool, &t.args, &ctx).await?;
                    let parsed: serde_json::Value =
                        serde_json::from_str(&result).unwrap_or(serde_json::Value::Null);
                    if parsed.is_null() {
                        // 非 JSON 工具结果（防御分支——run_script 恒返回 JSON）
                        return Ok(nemesis_sandbox::exec_world::ExecOutcome {
                            exit_code: None,
                            stdout: result,
                            stderr: String::new(),
                            timed_out: false,
                        });
                    }
                    Ok(nemesis_sandbox::exec_world::ExecOutcome {
                        exit_code: parsed
                            .get("exit_code")
                            .and_then(|v| v.as_i64())
                            .map(|c| c as i32),
                        stdout: parsed
                            .get("stdout")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        stderr: parsed
                            .get("stderr")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        timed_out: false,
                    })
                }
                // Spawn 车道：受守卫的进程内直跑（per-node opt-out 专用）。
                nemesis_sandbox::exec_world::ExecOp::Spawn(s) => {
                    nemesis_sandbox::exec_world::guarded_direct_spawn(&s, &self.spawn_roots).await
                }
            }
        }
    }

    /// 装配 workflow 引擎的执行世界：executor 分离关（Layer 0）→ `Ok(None)`
    /// （引擎无 world = 旧行为，script 节点裸 spawn / 注册表车道、引擎 IO
    /// 无守卫）；开 → 通道 + world。
    ///
    /// `writable_roots`：引擎控制面允许写的子树（workflow definitions /
    /// checkpoints / executions 等）；`spawn_roots`：Spawn 车道允许的 cwd 根
    /// （通常是 `{home}/workspace`）。
    #[allow(clippy::too_many_arguments)]
    pub fn build_workflow_world(
        home: &Path,
        workspace_dir: &Path,
        writable_roots: Vec<PathBuf>,
        spawn_roots: Vec<PathBuf>,
        config_handle: ConfigHandle,
    ) -> anyhow::Result<Option<Arc<ExecutorWorld>>> {
        let channel = build_executor_channel(home, workspace_dir, config_handle)?;
        Ok(channel.map(|channel| {
            Arc::new(ExecutorWorld {
                channel,
                writable_roots,
                spawn_roots,
            })
        }))
    }
}

#[cfg(feature = "sandbox")]
pub use world::build_workflow_world;

#[cfg(test)]
mod tests;
