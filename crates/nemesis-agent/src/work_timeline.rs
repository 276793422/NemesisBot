//! Work 时间线（ChatPanel 长任务时间线改造，2026-10-30，PLAN：
//! docs/PLAN/2026-10-30_ChatPanel长任务时间线改造.md）。
//!
//! turn 级累加器：按真实时序收拢一个 turn 的「中间轮叙述 + 工具调用」，
//! 最终回复落盘时随行写进 chat_log assistant 行（`"work"` 键）；执行中
//! 经 [`AgentLoop::current_work_timeline`]（WSAPI `chat.work_current`）
//! 只读快照，支撑「切走再切回」的全量恢复（活累加器无滑窗限制）。
//!
//! # 喂入点与单一真相源
//!
//! 喂入与既有事件发射**同点同源**：RoundText 在 [`crate::r#loop`]
//! `emit_round_output`（tool_batch 每轮先叙述后工具的发布序，叙述与思考
//! 皆空的轮不发——纯工具轮由 [`WorkRecorder::tool_started`] 的分组规则
//! 自动成轮）；ToolStarted/ToolFinished 在
//! [`crate::tool_event_hook::ToolEventHook`]。
//! 实时 WS 推送与落盘时间线是同一发射点的两个投影，无第二份转换逻辑。
//!
//! # 分组规则（一轮一个工具批次）
//!
//! LLM 每轮 = 至多一条叙述 + 思考链 + 恰好一个工具批次（execute_tool_batch
//! per round），发布序恒为「叙述 → Started/Finished×k → 下一轮」。据此：
//! 输出（叙述或思考）开新轮；工具进「末轮」当且仅当末轮有叙述或思考且
//! 尚无工具，否则开纯工具轮（两者皆空轮的批次不会误挂上一轮）。
//!
//! # 体积护栏
//!
//! [`MAX_WORK_ROUNDS`] 封顶轮数，超出置 `truncated` 诚实注记并停止追加；
//! preview 沿用发射侧截断（ARGS_PREVIEW_CAP/RESULT_PREVIEW_CAP），此处
//! 不放大。轮叙述为真实内容全量保留（用户拍板全落盘，2026-10-30）。
//!
//! # 槽模型（BUG-20261106-039 修复：单槽 → 按会话键分桶）
//!
//! 同一 AgentLoop 实例上会**并发/嵌套**跑多个 turn：spawn 工具（前台
//! G0/G2 阻塞路径与后台 G4 路径，agent_factory SpawnFn 持同一 loop 的
//! Weak 引用）在同一 loop 上跑 detached 子代理回合，board 评审/冲突消解/
//! forge 测试等宿主流程也借同一 loop 跑 detached 任务（run_detached /
//! run_detached_events）。单槽形态下 detached 的 begin() 互踩进行中父
//! turn 的累积、detached 自身从不 finish（其最终行不落 chat_log），父
//! turn 终局的 finish()（旧签名无键 take）会把**子代理的时间线**错误
//! 归属进父 assistant 行的 work 键。现为按会话键的桶表：begin / 喂入 /
//! finish / 快照全部按键路由，桶间天然隔离。
//!
//! 桶生命周期：begin 重置同键残余（中断/续行未消费照旧丢弃，语义不变）；
//! chat turn 终局 finish 按键取走（turn.rs 最终回复落盘）；detached
//! （run_detached_events）与集群续行（resume_execution_with_token）路径
//! 无落盘点，函数收尾显式 finish 清桶（时间线丢弃——子代理过程可见面是
//! 事件环 P1 详情视图，与改造前一致）。诚实边界：执行中 panic 的桶残留
//! 到进程结束（无 sweep；常态几 KB/桶，uuid 键无同键 begin 可清）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// 轮数上限（病态任务护栏）。超出不再追加，`truncated: true` 诚实注记。
pub const MAX_WORK_ROUNDS: usize = 500;

/// 单次工具调用的记录（`work.rounds[].tools[]` 条目）。
/// 形状单一真相源：本 struct 的 serde 输出 = chat_log 行 `"work"` 键 =
/// 前端 TS 类型镜像（web/src/stores/chat.ts WorkTimeline）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkToolRecord {
    pub tool: String,
    pub call_id: String,
    /// 成败（classify_result 判定）。`running=true` 时占位 false 不参考。
    pub ok: bool,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub args_preview: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub result_preview: String,
    /// 仍在执行（Started 已收、Finished 未收）。落盘形态恒 false（字段省略）。
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub running: bool,
}

/// 一个中间轮：叙述 + 思考链 + 该轮工具批次。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkRound {
    /// 模型该轮过程叙述。空 = 纯工具轮（叙述与思考皆空的轮不发）。
    pub text: String,
    /// 该轮思考链（Provider 归一后的 reasoning_content；旧 jsonl 缺省空
    /// ——serde default 兼容，空则不序列化）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    pub tools: Vec<WorkToolRecord>,
}

/// 落盘 / 快照形态的时间线（chat_log assistant 行 `"work"` 键的值）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkTimeline {
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    pub rounds: Vec<WorkRound>,
    /// 轮数超 [`MAX_WORK_ROUNDS`] 被截断。false 时字段省略。
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub truncated: bool,
}

/// 进行中的累加状态（`WorkRecorder` 桶内物；finish 时转 `WorkTimeline`）。
/// 会话键即桶键（HashMap 键为单一真相源），桶内不再重复存。
#[derive(Debug, Clone)]
struct RecordingWork {
    started_at_ms: u64,
    rounds: Vec<WorkRound>,
    truncated: bool,
}

impl RecordingWork {
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// 工具落点轮：末轮有叙述或思考且尚无工具 = 本批次的叙述轮；否则开
    /// 纯工具轮（两者皆空的轮 / 首轮即工具）。超限 = None（truncated 已置）。
    /// 「或思考」：thinking-only 轮（叙述空、思考有值）照样接收后续工具
    /// 批次——否则思考与工具分离成两轮。
    fn ensure_tool_round(&mut self) -> Option<&mut WorkRound> {
        let reuse_last = matches!(
            self.rounds.last(),
            Some(r) if (!r.text.is_empty() || !r.reasoning.is_empty()) && r.tools.is_empty()
        );
        if !reuse_last {
            if self.rounds.len() >= MAX_WORK_ROUNDS {
                self.truncated = true;
                return None;
            }
            self.rounds.push(WorkRound {
                text: String::new(),
                reasoning: String::new(),
                tools: Vec::new(),
            });
        }
        self.rounds.last_mut()
    }
}

/// turn 级时间线累加器（loop 持有 + ToolEventHook 共享同一 clone）。
/// 按会话键分桶（BUG-20261106-039：单槽在 spawn/评审/续行与父 turn 并发
/// 互踩）。无桶（未 begin / 已 finish）= 该键全部喂入静默忽略——零配置
/// 开销。
#[derive(Clone, Default)]
pub struct WorkRecorder(Arc<parking_lot::Mutex<HashMap<String, RecordingWork>>>);

impl WorkRecorder {
    /// turn 开始：重置该键的桶（同键上一 turn 的残余——中断/续行未消费
    /// ——就此丢弃）；其他键的桶不动（并发/嵌套 turn 互不串台）。
    pub fn begin(&self, session_key: &str) {
        self.0.lock().insert(
            session_key.to_string(),
            RecordingWork {
                started_at_ms: RecordingWork::now_ms(),
                rounds: Vec::new(),
                truncated: false,
            },
        );
    }

    /// 中间轮输出（emit_round_output 同点喂入；叙述 + 思考链同轮）。
    /// 两者皆空不建轮（trim 判定与观察面 emit_round_output 同口径；
    /// 存储保留原文不修剪）。纯工具轮由 [`WorkRecorder::tool_started`]
    /// 的分组规则自动成轮。无桶（未 begin / 已 finish）忽略。
    pub fn push_round_output(&self, session_key: &str, text: &str, reasoning: &str) {
        let mut buckets = self.0.lock();
        let Some(w) = buckets.get_mut(session_key) else {
            return;
        };
        if text.trim().is_empty() && reasoning.trim().is_empty() {
            return;
        }
        if w.rounds.len() >= MAX_WORK_ROUNDS {
            w.truncated = true;
            return;
        }
        w.rounds.push(WorkRound {
            text: text.to_string(),
            reasoning: reasoning.to_string(),
            tools: Vec::new(),
        });
    }

    /// 工具开始（ToolEventHook around 进入链前）。按分组规则落轮；
    /// 同 call_id 幂等（重复 Started 只更新首条字段）。
    pub fn tool_started(&self, session_key: &str, call_id: &str, tool: &str, args_preview: &str) {
        let mut buckets = self.0.lock();
        let Some(w) = buckets.get_mut(session_key) else {
            return;
        };
        // 幂等：同 call_id 已在（异常重复 Started）→ 只刷新 args。
        if let Some(rec) = w
            .rounds
            .iter_mut()
            .flat_map(|r| r.tools.iter_mut())
            .find(|t| t.call_id == call_id)
        {
            rec.args_preview = args_preview.to_string();
            return;
        }
        let Some(round) = w.ensure_tool_round() else {
            return;
        };
        round.tools.push(WorkToolRecord {
            tool: tool.to_string(),
            call_id: call_id.to_string(),
            ok: false,
            duration_ms: 0,
            args_preview: args_preview.to_string(),
            result_preview: String::new(),
            running: true,
        });
    }

    /// 工具结束（ToolEventHook around 返回后）。全轮倒序找 call_id 更新；
    /// 无匹配（Started 被截断丢弃等）诚实忽略。
    pub fn tool_finished(
        &self,
        session_key: &str,
        call_id: &str,
        ok: bool,
        duration_ms: u64,
        result_preview: &str,
    ) {
        let mut buckets = self.0.lock();
        let Some(w) = buckets.get_mut(session_key) else {
            return;
        };
        let rec = w
            .rounds
            .iter_mut()
            .flat_map(|r| r.tools.iter_mut())
            .find(|t| t.call_id == call_id);
        let Some(rec) = rec else {
            return;
        };
        rec.ok = ok;
        rec.duration_ms = duration_ms;
        rec.result_preview = result_preview.to_string();
        rec.running = false;
    }

    /// turn 终局：按会话键取走该键的时间线（桶移除）。无桶 / 无中间轮
    /// （直答）= None → 行不带 `work` 键。ended_at 取此刻。
    pub fn finish(&self, session_key: &str) -> Option<WorkTimeline> {
        let w = self.0.lock().remove(session_key)?;
        if w.rounds.is_empty() {
            return None;
        }
        Some(WorkTimeline {
            started_at_ms: w.started_at_ms,
            ended_at_ms: RecordingWork::now_ms(),
            rounds: w.rounds,
            truncated: w.truncated,
        })
    }

    /// 执行中只读快照（WSAPI `chat.work_current`）：不取桶、不扰动累加。
    /// 无桶（无进行中 turn）/ 尚无任何轮 = None。
    pub fn snapshot_json(&self, session_key: &str) -> Option<serde_json::Value> {
        let buckets = self.0.lock();
        let w = buckets.get(session_key)?;
        if w.rounds.is_empty() {
            return None;
        }
        let timeline = WorkTimeline {
            started_at_ms: w.started_at_ms,
            ended_at_ms: RecordingWork::now_ms(),
            rounds: w.rounds.clone(),
            truncated: w.truncated,
        };
        serde_json::to_value(timeline).ok()
    }
}

#[cfg(test)]
mod work_timeline_tests;
