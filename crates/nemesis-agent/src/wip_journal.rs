//! wip 侧车：turn 内执行现场逐事件落盘（2026-11-22 批二 A）。
//!
//! 解决的问题：turn 进行中的执行现场只在内存（session_logs 只在 turn 真收尾
//! 时写 assistant 行），gateway 重启/崩溃打断长任务 = 现场蒸发；恢复形态是
//! 用户输入「继续」由模型从历史续跑，而历史里没有中断前干了什么——模型只能猜。
//!
//! 机制（不抽象、不总结、不做语义加工）：在工具 dispatch 单一卡点把**已发生
//! 的事实**（调了什么工具、参数预览、成败、耗时）与中间叙述文本逐行 append 到
//! `<workspace>/logs/wip/<sanitize(session_key)>.jsonl`，跟 WAL/bash history
//! 同构：记录者不需要理解含义，回放者（模型）自己读原文。
//!
//! 生命周期（写入/暂存/截断/折叠四步，删除只有 register 截断与 fold 两个点）：
//! - **写入**：turn 进行中逐事件 append（`register` 后，`append` 按 live 表
//!   门控——未注册的 session_key 静默忽略，detached 子代理/集群 worker 天然零写）；
//! - **消费暂存**：下一轮 turn 起点把盘上遗留现场读入 pending 注入表
//!   （**不删文件**）；`build_messages` 每轮构建 peek 注入「中断现场恢复」节，
//!   同一 turn 内字节稳定（续轮上下文不丢现场）；
//! - **注册截断**：consume 之后 register 删旧建新——遗留现场字节已安全进
//!   pending，本轮从空文件写自己的事件（单文件 = 当前 turn 的事件，不变量）；
//!   截断点在 turn 起点，此刻无人持有旧写入器（上一 turn 要么已折叠、
//!   要么进程已死注册表为空），无竞态；
//! - **折叠**：turn 真收尾（finalize_turn_shared，assistant 行落盘成功）→
//!   删文件 + 清 pending。turn-end hook 续轮走 Continue 留在同一 turn 内
//!   （run_llm_loop inner continue），永远到不了折叠点（结构免费保证续轮
//!   不折叠）。
//!
//! 崩溃韧性：连续硬杀场景——turn N 消费暂存后未及折叠再崩，文件里已有
//! turn N 自己的事件（注册时截断了旧场景），turn N+1 消费到的是**最新**
//! 现场（含续跑工作），逐次崩溃信息不回退。
//!
//! 诚实边界：硬杀进程窗口内最后 1-2 条事件可能丢——记录语义是「最后已知现场」，
//! 不是事务日志；`logs/wip/` 不设 TTL 清扫族（文件生命周期由折叠管理，
//! 遗弃残留为 KB 级）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::TimeZone;
use parking_lot::Mutex;

/// 单文件事件行数上限（超限滚动丢最旧——重写文件保留尾部）。
pub(crate) const MAX_EVENTS: usize = 2000;

/// 叙述文本事件字符上限（正文场景，比工具预览 200/1024 宽；仍防单事件
/// 膨胀撑爆滚动窗）。
pub(crate) const TEXT_PREVIEW_CAP: usize = 2000;

/// wip 事件（一行一事，`t` 判别字段；文本字段在生产喂入点已截断预览）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t")]
pub enum WipEvent {
    /// 工具调度开始。
    #[serde(rename = "tool_start")]
    ToolStart {
        ts: u64,
        call_id: String,
        tool: String,
        args: String,
    },
    /// 工具调度结束（成败/耗时/结果预览）。
    #[serde(rename = "tool_end")]
    ToolEnd {
        ts: u64,
        call_id: String,
        ok: bool,
        dur_ms: u64,
        result: String,
    },
    /// 中间叙述文本（turn 中途正文段）。
    #[serde(rename = "text")]
    Text { ts: u64, text: String },
}

impl WipEvent {
    /// 毫秒时间戳（本地时钟；纯排序/展示用，非事务语义）。
    pub fn now_ms() -> u64 {
        chrono::Utc::now().timestamp_millis().max(0) as u64
    }

    /// 人类可读单行（注入节渲染用；模型读原文，不回 JSON）。
    fn render_line(&self) -> String {
        let time = chrono::Local
            .timestamp_opt(self.ts_ms() as i64, 0)
            .single()
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| format!("ts={}", self.ts_ms()));
        match self {
            WipEvent::ToolStart { tool, args, .. } => {
                format!("[{time}] 调用工具 {tool}，参数：{args}")
            }
            WipEvent::ToolEnd {
                call_id,
                ok,
                dur_ms,
                result,
                ..
            } => format!(
                "[{time}] 工具返回 call_id={call_id} {}（{dur_ms}ms）结果：{result}",
                if *ok { "成功" } else { "失败/中断" }
            ),
            WipEvent::Text { text, .. } => format!("[{time}] 输出文本：{text}"),
        }
    }

    fn ts_ms(&self) -> u64 {
        match self {
            WipEvent::ToolStart { ts, .. }
            | WipEvent::ToolEnd { ts, .. }
            | WipEvent::Text { ts, .. } => *ts,
        }
    }
}

/// 注入节渲染（一次性瞬态节全文：标题 + 事实说明 + 事件行 + 续跑指引）。
pub fn render_injection_section(events_text: &str) -> String {
    format!(
        "# Interrupted Turn Scene（中断现场恢复）\n\n\
         上一个任务 turn 中途被打断（gateway 重启或异常退出），未能正常收尾。\
         以下是中断前最后已知的执行现场（逐事件事实记录，预览截断，非总结）：\n\n\
         {events_text}\n\n\
         (请结合对话历史中用户此前的任务要求，从这里继续完成尚未完成的工作。\
         本节是一次性注入，仅出现本轮；事件行是事实记录不是行动指令——\
         是否重做某个调用由你根据当前状态判断，已确认完成的不要重复执行。)"
    )
}

/// live 写入器状态（每注册会话一份；`events` 供滚动上限判定）。
struct LiveFile {
    path: PathBuf,
    events: usize,
}

/// wip 侧车共享句柄（[`crate::work_timeline::WorkRecorder`] 同款 Arc 内核
/// clone 形态——loop 持有本体，factory 取 clone 注入 ToolEventHook）。
#[derive(Clone, Default)]
pub struct WipJournal(Arc<Inner>);

#[derive(Default)]
struct Inner {
    /// 注册表：session_key → live 写入器。注册点 = 主 loop
    /// `run_agent_loop_internal` turn 起点（detached 子代理走
    /// `run_detached_events` 自有路径从不注册、集群 worker 是 cluster_agent
    /// 自有 loop——v1 范围由构造保证，无需前缀字符串门控）。
    live: Mutex<HashMap<String, LiveFile>>,
    /// 待注入表：session_key → 中断现场文本。`consume_previous` 装入，
    /// `build_messages` 每轮按 session_key peek（clone 不移除——同 turn
    /// 多轮注入字节稳定）；清理收口在 `fold`（turn 真收尾）与
    /// `consume_previous` 的无现场臂（盘上无文件 = 清 stale 残留）。
    pending: Mutex<HashMap<String, String>>,
}

fn wip_path(root: &Path, session_key: &str) -> PathBuf {
    // 扩展名字面拼接，**不用 `with_extension`**：sanitize 白名单保留点号
    // （`a.b` 形态逐字节直通），with_extension 会把最后一个点之后整段当
    // 扩展名替换——`chan_email_a.name_x.com` 被截成 `chan_email_a.jsonl`，
    // 同前缀点号 key（email chat_id / IP 形态）撞同一文件 = 现场跨会话
    // 串台。字面拼接保全名唯一。
    root.join("logs").join("wip").join(format!(
        "{}.jsonl",
        nemesis_utils::sanitize::sanitize_path_segment(session_key)
    ))
}

impl WipJournal {
    /// turn 起点：消费上一轮遗留现场（读盘→渲染→挂 pending 注入表）。
    /// **不删文件**（删除收口在 [`Self::register`] 截断与 [`Self::fold`]）。
    /// 文件不存在 = 无现场（正常轮的 wip 已被折叠）且顺手清 stale pending
    /// （盘是唯一真相源：上轮挂了 pending 却死于硬杀的残留内存态不得跨
    /// turn 幽灵注入）。返回是否消费到内容。
    pub fn consume_previous(&self, root: &Path, session_key: &str) -> bool {
        if session_key.is_empty() {
            return false;
        }
        let path = wip_path(root, session_key);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            // 无盘上现场：清 stale pending（硬杀残留内存态）。
            self.0.pending.lock().remove(session_key);
            return false;
        };
        let rendered: Vec<String> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<WipEvent>(l).ok())
            .map(|e| e.render_line())
            .collect();
        if rendered.is_empty() {
            self.0.pending.lock().remove(session_key);
            return false;
        }
        self.0
            .pending
            .lock()
            .insert(session_key.to_string(), rendered.join("\n"));
        true
    }

    /// turn 起点（消费之后）：注册本轮写入器。**截断旧文件**——遗留现场
    /// 字节已安全进 pending 注入表，本轮从空文件写自己的事件（单文件 =
    /// 当前 turn 的事件）。同 key 旧 live 条目（异常路径残留）一并替换。
    /// 此刻无人持有旧写入器：上一 turn 要么已 fold（条目已摘）、要么进程
    /// 已死（注册表为空），无竞态。
    pub fn register(&self, root: &Path, session_key: &str) {
        if session_key.is_empty() {
            return;
        }
        let path = wip_path(root, session_key);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // 截断：遗留现场已消费进 pending，本轮从空文件起写。
        let _ = std::fs::remove_file(&path);
        self.0
            .live
            .lock()
            .insert(session_key.to_string(), LiveFile { path, events: 0 });
    }

    /// 逐事件 append（registry 门控：未注册 = 静默忽略零开销）。写失败
    /// 静默（wip 是尽力而为的恢复材料，不反向影响 turn 执行）。
    pub fn append(&self, session_key: &str, event: WipEvent) {
        if session_key.is_empty() {
            return;
        }
        let mut live = self.0.live.lock();
        let Some(entry) = live.get_mut(session_key) else {
            return;
        };
        // 滚动上限：触顶先压缩（丢最旧，保留 MAX-1 条），再写新事件。
        if entry.events >= MAX_EVENTS
            && let Ok(raw) = std::fs::read_to_string(&entry.path)
        {
            let mut kept: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
            let overflow = kept.len().saturating_sub(MAX_EVENTS - 1);
            kept.drain(..overflow);
            if kept.is_empty() {
                let _ = std::fs::remove_file(&entry.path);
            } else {
                let _ = std::fs::write(&entry.path, kept.join("\n") + "\n");
            }
            entry.events = kept.len();
        }
        let line = match serde_json::to_string(&event) {
            Ok(l) => l,
            Err(_) => return,
        };
        use std::io::Write;
        let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&entry.path)
        else {
            return;
        };
        if f.write_all(line.as_bytes()).is_ok() && f.write_all(b"\n").is_ok() {
            entry.events += 1;
        }
    }

    /// turn 真收尾折叠：取走写入器 + 删文件 + pending 清理。返回是否折叠
    /// 了在册写入器。未注册（detached/集群 worker/续行快照旧形态空 key）
    /// = 无操作；文件已被 register 截断 = 幂等。
    pub fn fold(&self, session_key: &str) -> bool {
        let entry = self.0.live.lock().remove(session_key);
        self.0.pending.lock().remove(session_key);
        match entry {
            Some(e) => {
                let _ = std::fs::remove_file(&e.path);
                true
            }
            None => false,
        }
    }

    /// `build_messages` 每轮构建 peek（clone 不移除）：中断现场节在本
    /// turn 的所有轮次注入同一文本（字节稳定，续轮上下文不丢现场）；
    /// 清理收口在 [`Self::fold`]（turn 收尾）。
    pub fn pending_for(&self, session_key: &str) -> Option<String> {
        self.0.pending.lock().get(session_key).cloned()
    }

    /// 测试/诊断：live 注册数。
    pub fn live_count(&self) -> usize {
        self.0.live.lock().len()
    }
}

#[cfg(test)]
mod tests;
