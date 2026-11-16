//! 自配速 durable loop（项5）：`loop_wakeup` / `loop_status` / `loop_finish`
//! 三工具——绑定活跃 loop 的会话里 agent 自主排程、查状态、收尾。
//!
//! 绑定模型：loop 状态文件带 `session_key`（`agent:main:session:{id}` 形态），
//! 工具执行时按 `context.session_key` 反查绑定（`find_bound_loop`）。未绑定
//! 会话调用 = 诚实报错（工具恒注册但不假装可用——同 board_issue 未装配
//! 不注册的诚实边界不同：这里注册面无装配依赖，恒在；不可用面在语义层
//! 诚实报）。
//!
//! 防自锁（计划钦定）：agent 工具面**不含**暂停/恢复——`paused`/`parked`
//! 只能由人经 CLI/WSAPI 置/解；`loop_finish` 只能收尾（done），不能复活。
//! 预算耗尽 → 驱动器自动 parked（诚实停车留痕）。
//!
//! estop：`loop_wakeup` 冻结期拒绝（排程也拒——全冻结语义）；驱动器侧
//! sweep 冻结期零动作（双保险）。
//!
//! 注册面：仅主 agent（`agent_factory` 8e；项目 loop 工具子集不带——
//! loop 绑定项目会话的形态 P1 不支持，见交付报告诚实边界）。
//!
//! tier 策略：不进 `tier_allowed_tools`（mini/normal 白名单天然不含，仅
//! big/Unresolved 全量档可见）——自配速是重决策面，小模型不供给。

use nemesis_agent::context::RequestContext;
use nemesis_loop::{LoopState, LoopStatus, list_loops, save_loop};
use std::path::PathBuf;
use std::sync::Arc;

pub const WAKEUP_TOOL_NAME: &str = "loop_wakeup";
pub const STATUS_TOOL_NAME: &str = "loop_status";
pub const FINISH_TOOL_NAME: &str = "loop_finish";

/// 按会话键反查绑定 loop（同键多条时取 updated_at 最新——理论不可达，
/// 防御性收敛）。
pub fn find_bound_loop(
    dir: &std::path::Path,
    session_key: &str,
) -> Result<Option<LoopState>, String> {
    let mut best: Option<LoopState> = None;
    for st in list_loops(dir) {
        if st.session_key == session_key {
            let newer = best
                .as_ref()
                .map(|b| st.updated_at_ms > b.updated_at_ms)
                .unwrap_or(true);
            if newer {
                best = Some(st);
            }
        }
    }
    Ok(best)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── loop_wakeup ────────────────────────────────────────────────────

/// 自配速核心：agent 自己决定下次何时醒来。
pub struct LoopWakeupTool {
    loops_dir: PathBuf,
    estop: Arc<nemesis_agent::estop::EstopState>,
}

impl LoopWakeupTool {
    pub fn new(loops_dir: PathBuf, estop: Arc<nemesis_agent::estop::EstopState>) -> Self {
        Self { loops_dir, estop }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for LoopWakeupTool {
    fn description(&self) -> String {
        "Schedule the next wake-up for the durable loop bound to this session \
         (self-pacing). Pass exactly one of `delay_secs` (one-shot) or `cron` \
         (recurring). Optionally attach a progress `note` and update the loop \
         `plan`. The driver injects a wake message into this session when due; \
         without a schedule the loop stays idle. Refused while e-stop is \
         engaged or the loop is paused/parked/done."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "delay_secs": { "type": "integer", "minimum": 1, "description": "One-shot: wake in N seconds." },
                "cron": { "type": "string", "description": "Recurring: cron expression (local time), e.g. \"0 9 * * *\"." },
                "note": { "type": "string", "description": "Optional progress note appended to the loop progress log." },
                "plan": { "type": "string", "description": "Optional replacement plan text (agent-maintained)." }
            },
            "required": []
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 不是合法 JSON: {e}"))?;
        let delay = parsed.get("delay_secs").and_then(|v| v.as_u64());
        let cron = parsed
            .get("cron")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let note = parsed
            .get("note")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let plan = parsed
            .get("plan")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        // estop 全冻结：排程也拒（计划钦定「注入闸 + loop 工具拒绝」双保险
        // 之二；驱动器 sweep 冻结期零动作是之一）。
        if self.estop.is_engaged() {
            return Err("急停（estop）已触发，loop 排程冻结——释放急停后再试".to_string());
        }

        let mut st = find_bound_loop(&self.loops_dir, &context.session_key)?
            .ok_or_else(|| "本会话未绑定任何 loop（由人经 CLI/WSAPI 创建并绑定）".to_string())?;

        match st.status {
            LoopStatus::Active => {}
            LoopStatus::Paused | LoopStatus::Parked => {
                return Err(format!(
                    "loop {} 当前状态 {}（仅人可暂停/恢复）——请告知用户经 CLI `loop resume` 或 WSAPI loops.resume 解除",
                    st.id,
                    st.status.as_str()
                ));
            }
            t @ (LoopStatus::Done | LoopStatus::Failed) => {
                return Err(format!(
                    "loop {} 已终态（{}），无法再排程",
                    st.id,
                    t.as_str()
                ));
            }
        }

        let ts = now_ms();
        let (next, cron_set) = nemesis_loop::compute_next_wake(ts, delay, cron)?;
        st.next_wake_ms = next;
        st.cron = cron_set;
        if let Some(p) = plan {
            st.plan = p.to_string();
        }
        if let Some(n) = note {
            st.push_progress(ts, n.to_string());
        }
        let when = st
            .next_wake_ms
            .map(nemesis_loop::format_wake_time)
            .unwrap_or_else(|| "unknown".to_string());
        save_loop(&self.loops_dir, &st)?;
        Ok(format!(
            "已排程：loop {} 将在 {} 唤醒本会话（{}）。届时你会收到唤醒消息并自主决定继续/调整/收尾。",
            st.id,
            when,
            if st.cron.is_some() {
                "周期 cron"
            } else {
                "一次性"
            }
        ))
    }
}

// ── loop_status ────────────────────────────────────────────────────

/// 读取绑定 loop 的当前状态（任意状态可读——停车场里也要能看账）。
pub struct LoopStatusTool {
    loops_dir: PathBuf,
}

impl LoopStatusTool {
    pub fn new(loops_dir: PathBuf) -> Self {
        Self { loops_dir }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for LoopStatusTool {
    fn description(&self) -> String {
        "Read the current state of the durable loop bound to this session: \
         status, goal, plan, budget vs spent (turns / wall clock / tokens), \
         next wake-up, and the progress log tail."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, _args: &str, context: &RequestContext) -> Result<String, String> {
        let st = find_bound_loop(&self.loops_dir, &context.session_key)?
            .ok_or_else(|| "本会话未绑定任何 loop".to_string())?;
        serde_json::to_string(&serde_json::json!({
            "id": st.id,
            "status": st.status.as_str(),
            "goal": st.goal,
            "plan": st.plan,
            "next_wake": st.next_wake_ms.map(nemesis_loop::format_wake_time),
            "cron": st.cron,
            "budget": st.budget,
            "spent": st.spent,
            "progress_tail": st.progress_tail(5).iter().map(|e| e.text.clone()).collect::<Vec<_>>(),
            "finish_summary": st.finish_summary,
        }))
        .map_err(|e| format!("serialize failed: {e}"))
    }
}

// ── loop_finish ────────────────────────────────────────────────────

/// 收尾：置 done + 总结（终态，不可逆；复活由人重建新 loop）。
pub struct LoopFinishTool {
    loops_dir: PathBuf,
}

impl LoopFinishTool {
    pub fn new(loops_dir: PathBuf) -> Self {
        Self { loops_dir }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for LoopFinishTool {
    fn description(&self) -> String {
        "Finish the durable loop bound to this session: mark it done with a \
         final summary. Terminal and irreversible — the driver stops waking \
         this session. Use when the goal is complete or genuinely impossible."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "Final summary: what was achieved, what remains." }
            },
            "required": ["summary"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 不是合法 JSON: {e}"))?;
        let summary = parsed
            .get("summary")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("缺少 summary（必填——收尾必须留下总结）")?;

        let mut st = find_bound_loop(&self.loops_dir, &context.session_key)?
            .ok_or_else(|| "本会话未绑定任何 loop".to_string())?;
        if st.status.is_terminal() {
            return Ok(format!(
                "loop {} 已是终态（{}），无需重复收尾{}",
                st.id,
                st.status.as_str(),
                st.finish_summary
                    .as_deref()
                    .map(|s| format!("；已有总结：{s}"))
                    .unwrap_or_default()
            ));
        }
        let ts = now_ms();
        st.status = LoopStatus::Done;
        st.finish_summary = Some(summary.to_string());
        st.next_wake_ms = None;
        st.cron = None;
        st.push_progress(ts, format!("[finish] {summary}"));
        save_loop(&self.loops_dir, &st)?;
        Ok(format!(
            "loop {} 已收尾（done）。驱动器不再唤醒本会话。总结已入进度日志。",
            st.id
        ))
    }
}

#[cfg(all(test, feature = "loop"))]
mod tests;
