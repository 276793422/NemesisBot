//! `nemesisbot run` — headless 单任务执行（K1，devtool-upgrade 阶段 4）。
//!
//! 一条命令、一个任务、跑完即退——无端口、
//! 无 gateway、无 web/通道/集群装配。与 `nemesisbot agent --message` 的
//! 本质差异：`run` 走 **agent_factory 的完整装配**（SharedResources +
//! `build_agent_loop`），因此安全 8 层 + guardian + tier 过滤 + spill +
//! turn_guard 与 gateway **同源生效**——headless 是编码 agent 的第二入口，
//! 不是安全旁路。任务执行走 `AgentLoop::run_detached_events`（与 spawn
//! 子代理同一条 `run_with_trace` 链路，全治理），会话跑完即弃不落盘。
//!
//! - 任务来源：位置参数，或 `-` / 缺省 = 从 stdin 读（管道友好）。
//! - `--workspace DIR`：工作区覆盖（config.json 仍从 home 解析）。
//! - `--mode plan|build`：F1 双模式，plan 只读 + plans/ 写放行。
//! - `--model ALIAS`：本次运行覆盖模型（须在 model_list 内）。
//! - `--format json`：K2 NDJSON 事件流（schema 见
//!   `docs/INFO/2026-09-06_k2-ndjson-event-stream-schema.md`）——
//!   tool_start/tool_end 实时逐行输出，turn/final/error 任务结束后按序输出；
//!   stdout 只承载 NDJSON 行，日志/告警一律 stderr。
//! - 退出码：0 = agent 正常完成；1 = 装配失败或 agent 报错（stderr）。
//!   文本模式 stdout 只承载最终回复；json 模式只承载 NDJSON 行——管道可安全截取。

use std::path::PathBuf;
use std::sync::Arc;

use crate::common;

// ---------------------------------------------------------------------------
// 纯函数核心（测试对象）：参数校验 + 事件折叠
// ---------------------------------------------------------------------------

/// 任务来源：位置参数原文，或 stdin（`-` / 缺省）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSource {
    /// 命令行位置参数给出的任务原文。
    Arg(String),
    /// 从 stdin 读取任务原文（管道/重定向）。
    Stdin,
}

/// 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// 人类可读：stdout 只输出最终回复。
    Text,
    /// K2 NDJSON 事件流：tool_start/tool_end 实时、turn/final/error 收尾
    /// （schema 固定，供脚本消费；见 docs/INFO 契约文档）。
    Json,
}

/// 校验后的运行配置（`validate` 的产物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRun {
    pub task: TaskSource,
    pub workspace: Option<PathBuf>,
    pub mode: nemesis_agent::types::AgentMode,
    pub format: OutputFormat,
    /// 0 = 用配置默认（透传 `DetachedOpts.max_turns` 语义）。
    pub max_turns: u32,
    pub model: Option<String>,
}

/// 校验/归一 CLI 参数（纯函数）。`Err` = 用户可读错误（含 remedy）。
pub fn validate(
    task: Option<String>,
    workspace: Option<PathBuf>,
    mode: Option<&str>,
    format: Option<&str>,
    max_turns: Option<u32>,
    model: Option<String>,
) -> Result<ValidatedRun, String> {
    // `-` 与缺省同义 = stdin。
    let task = match task {
        Some(t) if t == "-" => TaskSource::Stdin,
        Some(t) => TaskSource::Arg(t),
        None => TaskSource::Stdin,
    };
    let mode = match mode {
        None => nemesis_agent::types::AgentMode::Build,
        Some(s) => nemesis_agent::types::AgentMode::parse(s)
            .ok_or_else(|| format!("Unknown mode '{s}'. Must be 'plan' or 'build'."))?,
    };
    let format = match format {
        None => OutputFormat::Text,
        Some("text") => OutputFormat::Text,
        Some("json") => OutputFormat::Json,
        Some(other) => {
            return Err(format!(
                "Unknown format '{other}'. Must be 'text' or 'json'."
            ));
        }
    };
    Ok(ValidatedRun {
        task,
        workspace,
        mode,
        format,
        max_turns: max_turns.unwrap_or(0),
        model,
    })
}

/// 文本模式的事件折叠（纯函数）：`(final_output, error)`。
/// Done 优先于 Error（与 `run_detached` 同序）；两者皆无 = 无输出。
pub fn fold_text(events: &[nemesis_agent::types::AgentEvent]) -> (Option<String>, Option<String>) {
    let mut done: Option<String> = None;
    let mut error: Option<String> = None;
    for e in events {
        match e {
            nemesis_agent::types::AgentEvent::Done(m) => done = Some(m.clone()),
            nemesis_agent::types::AgentEvent::Error(e) => error = Some(e.clone()),
            _ => {}
        }
    }
    (done, error)
}

// ---------------------------------------------------------------------------
// K2：NDJSON 序列化（纯函数，schema 固定进 docs/INFO 契约文档）
// ---------------------------------------------------------------------------

/// 实时事件 → NDJSON 行。只输出 schema 内的 `tool_start` / `tool_end`；
/// `TodoUpdated` / `ModeChanged` 不在 v1 契约内，返回 `None` 跳过
/// （不向脚本发送未知 type——消费方按固定 5 type 解析）。
pub fn serialize_live_event(ev: &nemesis_types::agent::AgentEvent) -> Option<String> {
    let line = match ev {
        nemesis_types::agent::AgentEvent::ToolStarted {
            call_id,
            tool,
            args_preview,
            ..
        } => serde_json::json!({
            "type": "tool_start",
            "call_id": call_id,
            "tool": tool,
            "args_preview": args_preview,
        }),
        nemesis_types::agent::AgentEvent::ToolFinished {
            call_id,
            tool,
            duration_ms,
            ok,
            result_preview,
            ..
        } => serde_json::json!({
            "type": "tool_end",
            "call_id": call_id,
            "tool": tool,
            "duration_ms": duration_ms,
            "ok": ok,
            "result_preview": result_preview,
        }),
        // v1 契约外：不输出未知 type（诚实边界记录在 schema 文档）。
        _ => return None,
    };
    Some(line.to_string())
}

/// 终结事件集 → NDJSON 行序列（纯函数）。`Message` = 中间轮次文本
/// （`turn`），`Done` = `final`，`Error` = `error`；工具流量跳过
/// （已在执行期实时输出）。这些行在任务**结束后**统一输出——实时性
/// 边界诚实记录在 schema 文档（live 的只有工具生命周期）。
pub fn serialize_terminal_events(events: &[nemesis_agent::types::AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            let line = match e {
                nemesis_agent::types::AgentEvent::Message(m) => {
                    serde_json::json!({ "type": "turn", "text": m })
                }
                nemesis_agent::types::AgentEvent::Done(m) => {
                    serde_json::json!({ "type": "final", "text": m })
                }
                nemesis_agent::types::AgentEvent::Error(e) => {
                    serde_json::json!({ "type": "error", "message": e })
                }
                _ => return None,
            };
            Some(line.to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// Run the headless task. Setup failures and agent errors both bubble as
/// `Err`（dispatch 臂统一 eprintln + exit 1）；正常完成 = 打印最终回复 +
/// `Ok(())`（exit 0）。
pub async fn run(
    home: &std::path::Path,
    task: Option<String>,
    workspace: Option<PathBuf>,
    mode: Option<String>,
    format: Option<String>,
    max_turns: Option<u32>,
    model: Option<String>,
) -> anyhow::Result<()> {
    let v = validate(
        task,
        workspace,
        mode.as_deref(),
        format.as_deref(),
        max_turns,
        model,
    )
    .map_err(anyhow::Error::msg)?;

    // 任务原文：参数或 stdin（空 stdin = 诚实报错，不空跑）。
    let task_text = match &v.task {
        TaskSource::Arg(t) => t.clone(),
        TaskSource::Stdin => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| anyhow::anyhow!("failed to read task from stdin: {}", e))?;
            let t = buf.trim().to_string();
            if t.is_empty() {
                anyhow::bail!("empty task: pass a prompt argument or pipe a task via stdin");
            }
            t
        }
    };

    // 1. 配置必须存在（headless 不隐式 onboard）。
    let config_path = common::config_path(home);
    if !config_path.exists() {
        anyhow::bail!(
            "Configuration not found: {}.\nRun 'nemesisbot onboard default' first.",
            config_path.display()
        );
    }
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to load config: {}", e))?;
    // U15：模型 API key 走 credentials.yaml（与 gateway 同一解析路径）。
    nemesis_config::credentials::set_global_credentials_path(
        nemesis_config::credentials::credentials_path_for_home(home),
    );

    // 2. 工作区：显式 --workspace 优先；缺省 canonical 布局。不存在则创建
    //    （一次性 mkdir，安全且符合「指向新目录跑任务」的直觉）。
    let workspace_dir = match &v.workspace {
        Some(w) => {
            std::fs::create_dir_all(w)
                .map_err(|e| anyhow::anyhow!("cannot create workspace {}: {}", w.display(), e))?;
            w.clone()
        }
        None => common::workspace_path(home),
    };

    // 3. SecurityPlugin——与 gateway 完全同一构造（K1 提取的单一真相源）。
    let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    let security_plugin =
        crate::security_setup::build_security_plugin(home, security_enabled).await;

    // 3.5 K2：json 模式才挂工具事件通道（M1a broadcast——Some 时工厂给
    //     AgentLoop 装 ToolEventHook，tool_start/tool_end 实时可读；text
    //     模式保持 K1 形态 None = 零 hook 开销）。
    let (agent_event_tx, event_rx) = if v.format == OutputFormat::Json {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // 4. SharedResources + 完整工厂装配（安全/tier/spill/turn_guard 同源）。
    let config_store = Arc::new(nemesis_config::ConfigStore::from_config(
        cfg.clone(),
        config_path,
    ));
    let shared = Arc::new(crate::agent_factory::SharedResources {
        home: home.to_path_buf(),
        workspace: workspace_dir.clone(),
        config_store,
        security_plugin,
        mcp_enabled: cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false),
        mcp_config_path: common::mcp_config_path(home),
        agent_event_tx,
        ..Default::default()
    });
    let agent_loop = crate::agent_factory::build_agent_loop(&shared)
        .map_err(|e| anyhow::anyhow!("failed to build agent loop: {}", e))?;

    // 5. --model 覆盖：build_agent_loop 从 config 解析了默认模型；这里换上
    //    指定模型（与 agent_factory 同一个 resolve→factory→adapter 链），
    //    set_provider_and_model 内部顺带刷新 tier。
    if let Some(model_ref) = &v.model {
        let resolution = nemesis_config::resolve_model_config(&cfg, model_ref).map_err(|e| {
            anyhow::anyhow!(
                "model '{}': {} (add it first: nemesisbot model add --model <provider/model> --key KEY)",
                model_ref,
                e
            )
        })?;
        let factory_cfg = nemesis_providers::factory::FactoryConfig {
            llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
            api_key: resolution.api_key.clone(),
            api_base: resolution.api_base.clone(),
            workspace: workspace_dir.to_string_lossy().to_string(),
            connect_mode: resolution.connect_mode,
            protocol: resolution.protocol.clone(),
            timeout_secs: resolution.timeout_secs,
            account_id: String::new(),
            headers: std::collections::HashMap::new(),
        };
        let provider: Arc<dyn nemesis_providers::router::LLMProvider> =
            nemesis_providers::factory::create_provider(&factory_cfg)
                .map_err(|e| anyhow::anyhow!("failed to create provider: {}", e))?;
        let adapter = nemesis_web::ProviderAdapter::new(provider, resolution.model_name.clone());
        agent_loop.set_provider_and_model(Arc::new(adapter), resolution.model_name.clone());
    }

    // 6. --mode：plan/build 双模式（事件发空 session —— headless 无 web
    //    订阅者，广播静默空转）。
    agent_loop.set_mode_with_event(v.mode, "", "");

    // 7. 跑任务（depth 0 = 顶层；全治理链路同 spawn 子代理）。
    //     json 模式：run_detached_events 放进独立任务，主任务边执行边从
    //     M1a broadcast 消费工具事件逐行输出（K2 实时流）；执行完成后
    //     try_recv 排干残余、再按序输出 turn/final/error 终结行——
    //     broadcast send 同步入缓冲 + 任务完成先行发生于 try_recv，顺序
    //     有保证（工具行不会落在终结行之后）。
    match v.format {
        OutputFormat::Text => {
            let events = agent_loop
                .run_detached_events(
                    &task_text,
                    nemesis_agent::r#loop::DetachedOpts {
                        depth: 0,
                        max_turns: v.max_turns,
                        ..Default::default()
                    },
                )
                .await;
            fold_and_finish(&events)
        }
        OutputFormat::Json => {
            let mut rx = event_rx.expect("json 模式必有事件接收端");
            let task_loop = Arc::clone(&agent_loop);
            let mut handle = tokio::spawn(async move {
                task_loop
                    .run_detached_events(
                        &task_text,
                        nemesis_agent::r#loop::DetachedOpts {
                            depth: 0,
                            max_turns: v.max_turns,
                            ..Default::default()
                        },
                    )
                    .await
            });
            // 实时消费工具事件，直到执行任务完成。注意：select 的 handle
            // 分支会**消费** JoinHandle 的完成态输出——之后再 await 就是
            // 「JoinHandle polled after completion」panic（实测抓到过），
            // 所以完成态结果必须在分支里接住，不存在第二次 poll。
            let mut finished: Option<
                Result<Vec<nemesis_agent::types::AgentEvent>, tokio::task::JoinError>,
            > = None;
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Ok(ev) => {
                            if let Some(line) = serialize_live_event(&ev) {
                                println!("{}", line);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // 消费太慢丢帧：诚实注记走 stderr（stdout 只承载 NDJSON 行）。
                            eprintln!("ndjson: {} live events dropped (consumer too slow)", n);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    res = &mut handle => {
                        finished = Some(res);
                        break;
                    }
                }
            }
            let events = match finished {
                Some(res) => res.map_err(|e| anyhow::anyhow!("agent task join error: {}", e))?,
                // Closed 先退出（理论上不可能：senders 活到本函数结束）——
                // handle 未被 poll 至完成，这里首 poll 合法。
                None => handle
                    .await
                    .map_err(|e| anyhow::anyhow!("agent task join error: {}", e))?,
            };
            // 排干执行完成前已入缓冲的残余事件（send 先于任务完成，必已入缓冲）。
            while let Ok(ev) = rx.try_recv() {
                if let Some(line) = serialize_live_event(&ev) {
                    println!("{}", line);
                }
            }
            for line in serialize_terminal_events(&events) {
                println!("{}", line);
            }
            // 退出码语义与文本模式同源折叠除——但不再裸打印 final 文本
            // （它已作为 {"type":"final"} 行输出，stdout 只承载 NDJSON）。
            match fold_text(&events) {
                (Some(_), _) => Ok(()),
                (None, Some(e)) => Err(anyhow::anyhow!("agent error: {}", e)),
                (None, None) => Err(anyhow::anyhow!("agent produced no output")),
            }
        }
    }
}

/// 折叠终结事件并落退出码语义（仅文本模式）：Done→stdout+Ok；仅
/// Error→Err；两者皆无→诚实报错。
fn fold_and_finish(events: &[nemesis_agent::types::AgentEvent]) -> anyhow::Result<()> {
    match fold_text(events) {
        (Some(m), _) => {
            println!("{}", m);
            Ok(())
        }
        (None, Some(e)) => Err(anyhow::anyhow!("agent error: {}", e)),
        (None, None) => Err(anyhow::anyhow!("agent produced no output")),
    }
}

#[cfg(test)]
mod tests;
