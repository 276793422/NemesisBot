//! 目标系统评估闸驱动器（gateway 侧；feature=`goals`）。
//!
//! 计划真相源：`docs/PLAN/2026-11-16_目标系统计划.md`（D-Q0~Q12）。
//!
//! 职责单一：**watcher**——订阅 bus outbound（带会话键的终局回复 = turn
//! 结束）触发评估闸；订阅 AgentEvent（ToolStarted/ToolFinished）按
//! call_id 关联累积本轮工具调用记录（含实际返回）；评估（LLM 独立评审
//! goal.md + 本轮材料）→ DONE 落状态 / CONTINUE 注入 goal 轮。全部裁决
//! 逻辑在 nemesis-goals `gate`（纯函数可单测），本模块只管 IO、LLM 调用
//! 与注入（nemesis-loop 驱动器同款分层）。
//!
//! 并发防护：进程级评估互斥锁 + chat_log total 行数单调 cursor（同一
//! turn 的多条终局帧只评第一次；人在场的新消息推进 cursor → 下一轮
//! 评估）。estop 冻结期跳过评估（goal 状态不动，release 后下一 turn
//! 照常）；注入前二次检查（评估途中急停则丢弃本轮 CONTINUE）。
//!
//! 会话 → AgentLoop 解析复刻 `resolve_session_loop` 单一裁决序（agents
//! bridge → projects bridge → 主槽 Weak-style 引用），评估用 LLM 走
//! `chore_completion_ex`（与 gateway 杂务同源：墙钟护栏 + 空输出拒绝 +
//! aux 账本；默认主模型，goal `eval_model="small"` 才走杂务槽，D-Q5）。
//!
//! 本模块同时提供 [`workspace_goals_guard`]——文件保护闭包（D-Q3 创建者
//! 二分）装进 WorkspaceBoundary（主 agent / 集群 worker / executor 子
//! 进程）；非 goals feature 构建 = None（不设防）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 目标系统文件保护闭包（WorkspaceBoundary.goals_guard 供给面）。
///
/// 非 goals feature 构建 = None（feature 裁剪形态不设防）；goals 构建 =
/// 读注册表取创建者 + `gate::fence_denial` 裁决。注册表读取失败
/// （损坏）= fail-open 放行 + WARN（注册表损坏是异常态，阻断全部文件写
/// 的代价高于放行——保护面由评估闸状态与 Dashboard 兜底）。
pub fn workspace_goals_guard(
    ws_root: &Path,
) -> Option<Arc<dyn Fn(&str) -> Option<String> + Send + Sync>> {
    #[cfg(feature = "goals")]
    {
        let root = ws_root.to_path_buf();
        Some(Arc::new(move |rel: &str| {
            let registry = match nemesis_goals::load_registry(&root) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("[Goals] registry read failed in guard, fail-open: {e}");
                    return None;
                }
            };
            nemesis_goals::fence_denial(rel, &|id: &str| {
                registry.iter().find(|e| e.id == id).map(|e| e.created_by)
            })
        }))
    }
    #[cfg(not(feature = "goals"))]
    {
        let _ = ws_root;
        None
    }
}

/// 出站订阅端 Lagged 处理（BUG-20261117-076）：丢帧先入 bus 出站丢失
/// 账本再 WARN。bus 记账协议（`nemesis-bus` `record_outbound_lagged`：
/// 全部订阅端 recv() 返回 Lagged(n) 时入账）此前在本 watcher 只 warn
/// 不入账——`lagged_outbound()` 指标漏计（其余生产订阅端 adapters/
/// web_init/local_dispatch/projects/channel_approval/post_agent/web
/// pump 均已入账，此处是家族横扫的唯一漏网点）。AgentEvent 臂不经 bus
/// （gateway 独立 broadcast，无对应账本），warn-only 维持。
#[cfg(feature = "goals")]
fn note_outbound_lagged(bus: &nemesis_bus::MessageBus, n: u64) {
    bus.record_outbound_lagged(n);
    tracing::warn!("[Goals] outbound stream lagged, dropped {n} frames");
}

/// 目标评估闸驱动器（watcher）。gateway 装配点 spawn 一次常驻。
#[cfg(feature = "goals")]
pub fn spawn_goals_engine(
    bus: Arc<nemesis_bus::MessageBus>,
    main_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    estop: Arc<nemesis_agent::estop::EstopState>,
    workspace: PathBuf,
    agent_event_rx: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    conv_router: nemesis_web::SharedConvRouter,
) -> tokio::task::JoinHandle<()> {
    let engine = Arc::new(GoalsEngine {
        bus,
        main_loop_ref,
        estop,
        workspace,
        conv_router,
        eval_lock: tokio::sync::Mutex::new(()),
        cursors: tokio::sync::Mutex::new(HashMap::new()),
        tool_calls: std::sync::Mutex::new(HashMap::new()),
    });
    tokio::spawn(async move {
        let mut outbound_rx = engine.bus.subscribe_outbound();
        let mut event_rx = agent_event_rx;
        loop {
            tokio::select! {
                res = outbound_rx.recv() => match res {
                    Ok(outbound) => {
                        let sk = outbound.meta.session_key.as_deref();
                        if nemesis_goals::should_eval_on_outbound(
                            sk,
                            outbound.meta.receipt,
                            outbound.meta.occupies,
                        ) {
                            let key = outbound.meta.session_key.clone().unwrap_or_default();
                            engine.evaluate(&key).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        note_outbound_lagged(&engine.bus, n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                res = event_rx.recv() => match res {
                    Ok(nemesis_types::agent::AgentEvent::ToolStarted {
                        session_key,
                        call_id,
                        tool,
                        args_preview,
                        ..
                    }) => engine.record_tool_started(&session_key, &call_id, &tool, &args_preview),
                    Ok(nemesis_types::agent::AgentEvent::ToolFinished {
                        session_key,
                        call_id,
                        tool,
                        ok,
                        result_preview,
                        ..
                    }) => engine.record_tool_finished(&session_key, &call_id, &tool, ok, &result_preview),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[Goals] agent event stream lagged, dropped {n}");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        tracing::info!("[Goals] engine watcher exited (bus closed)");
    })
}

/// 注入入站 chat_id 解析（回归锚 BUG-20261117-goals-echo）：委托
/// `post_agent::resolve_inject_chat_id` 单一真相源（events/loops 注入器
/// 同款）——conv_router 活页绑定优先（回复推回打开的 Dashboard tab），
/// 无绑定回落 `web:<会话id>` 通道原生形态（死会话软失败，回复仍按
/// session_key 落历史）。
#[cfg(feature = "goals")]
fn inject_chat_id(router: &nemesis_web::SharedConvRouter, session_key: &str) -> String {
    crate::commands::gateway::post_agent::resolve_inject_chat_id(session_key, router)
}

#[cfg(feature = "goals")]
struct GoalsEngine {
    bus: Arc<nemesis_bus::MessageBus>,
    /// 主 loop 引用（与 AppState 同源的 `Arc<RwLock<Option<Arc<AgentLoop>>>>`，
    /// agent 重启自动跟进；评估 LLM 调用与注入都经它解析）。
    main_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    estop: Arc<nemesis_agent::estop::EstopState>,
    workspace: PathBuf,
    /// 会话→活跃 web 连接路由（conv_router，cron 同源）：CONTINUE 注入的
    /// 入站 chat_id 用它解析出 `web:<conn>` 实时推回打开的 Dashboard
    /// tab；无绑定（tab 已关/服务端形态）回落裸 session_key——回复仍按
    /// session_key 落历史，只少实时推送（软失败语义与 cron 投递一致）。
    conv_router: nemesis_web::SharedConvRouter,
    /// 评估互斥（进程级单评估——并发终局帧串行进闸）。
    eval_lock: tokio::sync::Mutex<()>,
    /// session → 已评估时的 chat_log total 行数（同 turn 双评防线）。
    cursors: tokio::sync::Mutex<HashMap<String, usize>>,
    /// session → 本轮工具调用累积（ToolStarted 登记、ToolFinished 补结果；
    /// 评估时取走清空）。std Mutex——临界区只有 insert/get（无 await，微秒
    /// 级），select 臂同步上下文可安全持有。
    tool_calls: std::sync::Mutex<HashMap<String, SessionToolCalls>>,
}

/// 单会话本轮工具调用累积：`order` 保时序（ToolStarted 顺序），`by_call`
/// 按 call_id 配对结果（ToolFinished 补 ok + result）。
#[cfg(feature = "goals")]
#[derive(Default)]
struct SessionToolCalls {
    order: Vec<String>,
    by_call: HashMap<String, nemesis_goals::ToolCallMaterial>,
}

/// 本轮工具调用累积上限（防超长会话撑爆评估提示词；取最近 N 个调用）。
#[cfg(feature = "goals")]
const TOOL_CALLS_CAP: usize = 30;

/// 单次评估的 LLM 调用上限（解析失败重试；全败 = blocked(eval-failure)）。
#[cfg(feature = "goals")]
const EVAL_ATTEMPTS: usize = 3;

#[cfg(feature = "goals")]
impl GoalsEngine {
    /// ToolStarted 登记（时序入列 + 按 call_id 建记录；同 call_id 双发防重）。
    fn record_tool_started(
        &self,
        session_key: &str,
        call_id: &str,
        tool: &str,
        args_preview: &str,
    ) {
        let mut map = self
            .tool_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let s = map.entry(session_key.to_string()).or_default();
        if s.by_call.contains_key(call_id) {
            return;
        }
        s.order.push(call_id.to_string());
        s.by_call.insert(
            call_id.to_string(),
            nemesis_goals::ToolCallMaterial {
                tool: tool.to_string(),
                args_preview: args_preview.to_string(),
                ok: None,
                result_preview: String::new(),
            },
        );
        let excess = s.order.len().saturating_sub(TOOL_CALLS_CAP);
        if excess > 0 {
            for dropped in s.order.drain(..excess) {
                s.by_call.remove(&dropped);
            }
        }
    }

    /// ToolFinished 补结果（按 call_id 配对；无配对 Started（引擎 mid-turn
    /// 启动错过）也登记——孤儿记录保时序尾插，参数面留空诚实呈现）。
    fn record_tool_finished(
        &self,
        session_key: &str,
        call_id: &str,
        tool: &str,
        ok: bool,
        result_preview: &str,
    ) {
        let mut map = self
            .tool_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let s = map.entry(session_key.to_string()).or_default();
        if let Some(rec) = s.by_call.get_mut(call_id) {
            rec.ok = Some(ok);
            rec.result_preview = result_preview.to_string();
            return;
        }
        s.order.push(call_id.to_string());
        s.by_call.insert(
            call_id.to_string(),
            nemesis_goals::ToolCallMaterial {
                tool: tool.to_string(),
                args_preview: String::new(),
                ok: Some(ok),
                result_preview: result_preview.to_string(),
            },
        );
        let excess = s.order.len().saturating_sub(TOOL_CALLS_CAP);
        if excess > 0 {
            for dropped in s.order.drain(..excess) {
                s.by_call.remove(&dropped);
            }
        }
    }

    /// 取走并清空该会话的工具调用记录（评估材料，时序保序）。
    async fn take_tool_calls(&self, session_key: &str) -> Vec<nemesis_goals::ToolCallMaterial> {
        let mut map = self
            .tool_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.remove(session_key)
            .map(|s| {
                s.order
                    .iter()
                    .filter_map(|id| s.by_call.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 会话 → AgentLoop 解析（复刻 resolve_session_loop 单一裁决序：
    /// agents bridge → projects bridge → 主槽）。None = 停机窗口/会话
    /// 归属不可解析——本轮评估诚实跳过。
    fn loop_for_session(&self, session_key: &str) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
        if let Some(agent_id) = nemesis_web::handlers::agents::agent_owner_of(session_key)
            && let Some(bridge) = nemesis_web::handlers::agents::agents_bridge()
            && let Ok(l) = bridge.loop_for_agent(&agent_id)
        {
            return Some(l);
        }
        if let Some(bridge) = nemesis_web::handlers::projects::projects_bridge()
            && let Some(pid) = bridge.owner_of(session_key)
            && let Some(l) = bridge.loop_for_session(&pid)
        {
            return Some(l);
        }
        self.main_loop_ref.read().clone()
    }

    /// 评估闸主体（turn 结束触发）。
    async fn evaluate(&self, session_key: &str) {
        let _guard = self.eval_lock.lock().await;
        // estop 冻结：评估闸全冻结，goal 状态不动（release 后下一 turn 照常）。
        if self.estop.is_engaged() {
            return;
        }
        // 本轮工具调用记录在评估周期入口消费（BUG-20261118-154）：登记面
        // （record_tool_started/finished）对全部会话累积（事件不过滤），
        // 消费点曾深在材料收集步（goal.md 读取之后）——无 goal 绑定会话
        // （常态）/ 预算停车 / goal.md 缺失 / LLM 硬失败等早退路径全部不
        // 消费，记录（每会话 ≤30 条 × KB 级预览）只进不出 = 长驻进程
        // 无界增长家族（BUG-20261116-001 同族；本 crate 建成晚于该轮
        // 横扫 = 漏网站点）。提前到入口：任何后续路径都不得让记录跨
        // turn 滞留（顺带根除评估 skip 后陈旧材料混入下一轮评估的污染
        // 面）。estop 冻结期不消费（与「goal 状态不动」对称；release 后
        // 首次评估一并消费，cap 30 有界）。
        let tool_calls = self.take_tool_calls(session_key).await;
        let entries = match nemesis_goals::load_registry(&self.workspace) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("[Goals] registry read failed, skip eval: {e}");
                return;
            }
        };
        let Some(entry) = entries
            .iter()
            .find(|e| e.session_key == session_key && e.status == nemesis_goals::GoalStatus::Active)
            .cloned()
        else {
            return;
        };
        // 预算判定（评估前；耗尽 → blocked 停车，不猜 CONTINUE，D-Q10）。
        if let Some((code, message)) = nemesis_goals::budget_block_reason(&entry) {
            self.block_goal(&entry.id, &code, &message);
            return;
        }
        // 同 turn 双评防线：chat_log total 行数未变 = 该 turn 已评过。
        let (rows, total, _has_more, _oldest) =
            nemesis_agent::chat_log::read_chat_log(session_key, 60, None);
        {
            let mut cursors = self.cursors.lock().await;
            if cursors.get(session_key) == Some(&total) {
                return;
            }
            cursors.insert(session_key.to_string(), total);
        }
        // 材料：goal.md 全文（每次评估重读——单一真相源，人改下轮生效）
        // + 本轮 user 消息 + 最终回复 + 本轮工具调用记录（含实际返回）。
        let goal_md = match nemesis_goals::read_goal_md(&self.workspace, &entry.id) {
            Ok(Some(md)) => md,
            Ok(None) => {
                tracing::warn!(
                    "[Goals] goal {} md file missing (deleted by hand?), skip eval",
                    entry.id
                );
                return;
            }
            Err(e) => {
                tracing::warn!("[Goals] goal {} md read failed, skip eval: {e}", entry.id);
                return;
            }
        };
        let (user_msg, final_reply) = nemesis_goals::extract_turn_material(&rows);
        let user_msg = user_msg.unwrap_or_default();
        let final_reply = final_reply.unwrap_or_default();
        let prefer_small = entry.eval_model.as_deref() == Some("small");
        let round = entry.rounds_used;
        let max_rounds = entry.max_rounds;
        let started = std::time::Instant::now();
        let model_label = if prefer_small { "small" } else { "main" };
        let mut last_raw = String::new();
        for attempt in 0..EVAL_ATTEMPTS {
            let Some(loop_h) = self.loop_for_session(session_key) else {
                tracing::warn!(
                    "[Goals] goal {}: no agent loop for session (shutdown window?), skip eval",
                    entry.id
                );
                return;
            };
            let prompt = nemesis_goals::build_eval_prompt(
                &goal_md,
                &user_msg,
                &final_reply,
                &tool_calls,
                round,
                max_rounds,
            );
            match loop_h
                .chore_completion_ex("goal_eval", prompt, 1024, prefer_small)
                .await
            {
                Ok(raw) => match nemesis_goals::parse_verdict(&raw) {
                    Ok((verdict, gap)) => {
                        self.apply_verdict(&entry, verdict, gap, model_label, started, &raw);
                        return;
                    }
                    Err(e) => {
                        last_raw = raw;
                        tracing::warn!(
                            "[Goals] goal {} verdict parse failed (attempt {}/{}): {e}",
                            entry.id,
                            attempt + 1,
                            EVAL_ATTEMPTS
                        );
                    }
                },
                Err(e) => {
                    // LLM 调用硬失败（网络/配置）：诚实跳过本轮（下一 turn
                    // 再评），不进 blocked——与解析失败（确定性坏输出）分性。
                    tracing::warn!("[Goals] goal {} eval llm call failed: {e}", entry.id);
                    return;
                }
            }
        }
        // 解析 EVAL_ATTEMPTS 次全败 → blocked(eval-failure) 诚实停车，不猜。
        let message = format!(
            "评估输出连续 {EVAL_ATTEMPTS} 次无法解析为 JSON 判定，评估闸停车；检查评估模型后 enable 解锁"
        );
        self.record_eval(&entry, "error", None, model_label, started, &last_raw);
        self.block_goal(&entry.id, "eval-failure", &message);
    }

    /// 判定落地：DONE → done（唯一 done 出口）；CONTINUE → rounds_used+1
    /// + goal 轮注入。
    fn apply_verdict(
        &self,
        entry: &nemesis_goals::GoalEntry,
        verdict: nemesis_goals::EvalVerdict,
        gap: Option<String>,
        model_label: &str,
        started: std::time::Instant,
        raw: &str,
    ) {
        match verdict {
            nemesis_goals::EvalVerdict::Done => {
                self.record_eval(entry, "done", None, model_label, started, raw);
                // 状态机闸（BUG-20261118-154）：评估 LLM 窗口（秒~分钟级，
                // 含 3 次重试）内人可能已 disable——paused→done 是转移表
                // 禁止的非法态。闭包在注册表锁内重读 fresh 态，仅 Active
                // 才落 done；状态已变 = 引擎让位人操作（跳过 + warn）。
                let mut wrote = false;
                if let Err(e) = nemesis_goals::update_registry(&self.workspace, |entries| {
                    match entries.iter_mut().find(|e| e.id == entry.id) {
                        Some(e) if e.status == nemesis_goals::GoalStatus::Active => {
                            e.status = nemesis_goals::GoalStatus::Done;
                            e.blocked_code = None;
                            e.blocked_message = None;
                            e.updated_at_ms = now_ms();
                            wrote = true;
                        }
                        Some(e) => tracing::warn!(
                            "[Goals] goal {} state moved to {} during eval window, done write skipped (transition table)",
                            entry.id,
                            e.status.as_str()
                        ),
                        None => {}
                    }
                    Ok(())
                }) {
                    tracing::warn!("[Goals] goal {} done transition failed: {e}", entry.id);
                    return;
                }
                if wrote {
                    tracing::info!(
                        "[Goals] goal {} DONE — evaluation gate closed the loop (rounds {})",
                        entry.id,
                        entry.rounds_used
                    );
                }
            }
            nemesis_goals::EvalVerdict::Continue => {
                self.record_eval(entry, "continue", gap.as_deref(), model_label, started, raw);
                let next_round = entry.rounds_used + 1;
                // 状态机闸（同 Done 臂，BUG-20261118-154）：评估窗口内人
                // pause 的 goal 不占轮数预算也不注入——paused goal 本不
                // 该触发评估，无声 rounds+1 等于偷扣预算。
                let mut bumped = false;
                if let Err(e) = nemesis_goals::update_registry(&self.workspace, |entries| {
                    match entries.iter_mut().find(|e| e.id == entry.id) {
                        Some(e) if e.status == nemesis_goals::GoalStatus::Active => {
                            e.rounds_used = next_round;
                            e.updated_at_ms = now_ms();
                            bumped = true;
                        }
                        Some(e) => tracing::warn!(
                            "[Goals] goal {} state moved to {} during eval window, round bump skipped (transition table)",
                            entry.id,
                            e.status.as_str()
                        ),
                        None => {}
                    }
                    Ok(())
                }) {
                    tracing::warn!("[Goals] goal {} round bump failed: {e}", entry.id);
                    return;
                }
                if !bumped {
                    return;
                }
                // estop 途中急停：丢弃本轮 CONTINUE（状态已 +1 无害——下一
                // turn 评估闸照常，轮号语义连续）。
                if self.estop.is_engaged() {
                    tracing::info!("[Goals] estop engaged before injection, drop round");
                    return;
                }
                let content = nemesis_goals::build_round_message(
                    &entry.id,
                    next_round,
                    entry.max_rounds,
                    gap.as_deref(),
                );
                if !nemesis_agent::chat_log::session_occupied(&entry.session_key) {
                    tracing::warn!(
                        "[Goals] goal {} target session {} missing, skip injection",
                        entry.id,
                        entry.session_key
                    );
                    return;
                }
                // 注入 chat_id 经 conv_router 解析活跃 web 连接（cron 同
                // 源）：有绑定 = 回复实时推回打开的 Dashboard tab；无绑定
                // 回落裸 session_key（回复仍落历史，只少实时推送——
                // WebChannel 对裸 session_key 的拒投是既有软失败语义）。
                let inject_chat_id = inject_chat_id(&self.conv_router, &entry.session_key);
                self.bus
                    .publish_inbound(nemesis_types::channel::InboundMessage {
                        channel: "web".to_string(),
                        sender_id: "system".to_string(),
                        chat_id: inject_chat_id,
                        content,
                        media: Vec::new(),
                        session_key: entry.session_key.clone(),
                        correlation_id: String::new(),
                        metadata: std::collections::HashMap::new(),
                        voice_playback: None,
                    });
                tracing::info!(
                    "[Goals] goal {} CONTINUE → round {} injected (gap: {})",
                    entry.id,
                    next_round,
                    gap.as_deref().unwrap_or("-")
                );
            }
        }
    }

    /// blocked 停车（预算耗尽 / 评估失败合并失败面，D-Q11）。
    fn block_goal(&self, id: &str, code: &str, message: &str) {
        // 状态机闸（BUG-20261118-154）：仅 Active 可入 blocked——budget
        // 判定基于评估开始时的条目快照，eval-failure 路径距快照已隔多次
        // LLM 重试，窗口内人 pause/done 的 goal 不被引擎覆盖成 blocked。
        let mut blocked = false;
        if let Err(e) = nemesis_goals::update_registry(&self.workspace, |entries| {
            match entries.iter_mut().find(|e| e.id == id) {
                Some(e) if e.status == nemesis_goals::GoalStatus::Active => {
                    e.status = nemesis_goals::GoalStatus::Blocked;
                    e.blocked_code = Some(code.to_string());
                    e.blocked_message = Some(message.to_string());
                    e.updated_at_ms = now_ms();
                    blocked = true;
                }
                Some(e) => tracing::warn!(
                    "[Goals] goal {id} state is {} during eval window, block write skipped (transition table)",
                    e.status.as_str()
                ),
                None => {}
            }
            Ok(())
        }) {
            tracing::warn!("[Goals] goal {id} block transition failed: {e}");
            return;
        }
        if blocked {
            tracing::warn!("[Goals] goal {id} blocked ({code}): {message}");
        }
    }

    /// 评估台账留痕（逐轮判定 + 模型 + 缺口 + 原始输出摘录）。
    fn record_eval(
        &self,
        entry: &nemesis_goals::GoalEntry,
        verdict: &str,
        gap: Option<&str>,
        model_label: &str,
        started: std::time::Instant,
        raw: &str,
    ) {
        let rec = nemesis_goals::EvalRecord {
            at_ms: now_ms(),
            round: entry.rounds_used,
            verdict: verdict.to_string(),
            gap: gap.map(|s| s.to_string()),
            model: model_label.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
            raw_excerpt: raw.chars().take(512).collect(),
        };
        if let Err(e) = nemesis_goals::append_eval(&self.workspace, &entry.id, &rec) {
            tracing::warn!("[Goals] goal {} eval record failed: {e}", entry.id);
        }
    }
}

#[cfg(feature = "goals")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(all(test, feature = "goals"))]
mod goals_engine_tests;
