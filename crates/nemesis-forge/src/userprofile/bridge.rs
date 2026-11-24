//! userprofile ↔ Forge 双向接口（批 A）。
//!
//! 用户学习与自学习是一个事情的两个方面（同信号、主语不同、产物分流）：
//! - **接口一（本模块落盘面）**：反思通道按主语分类后，主语=助手/LLM 的
//!   教训（`kind="lesson"`）**不丢弃**——落
//!   `<ws>/forge/userprofile/outbound_lessons.jsonl`（pending 状态队列），
//!   Forge 未来消费时自行决定映射形态（experience_store / pattern /
//!   reflector 输入），本模块不替它定；Forge 侧用
//!   [`read_pending_lessons`] 拉取 + [`ack_lessons`] 确认。
//! - **接口二**：Forge 反向抛「关于用户的经验」进画像 =
//!   [`store::ingest_external_entry`]（强制来源标记，见 store）。
//!   **批 D 状态（2026-11-24）**：API 就绪、生产接线**挂账**——Forge 侧
//!   现有产物（反思 insights / 蒸馏技能草稿 / learning patterns）主语
//!   全部=助手，无「关于用户」的已分类经验可抛；造 subject-guessing
//!   回抛 = 单向假设（批 A veto 同款纪律：不造没有消费面的接线）。待
//!   Forge 反思带主语分类时再接。
//!
//! 批 D 已接线（消费面唯一 = 网关周期反思循环 `Forge::run_reflection_cycle`）：
//! - 接口一消费：pending lessons → 语义分析提示词 + 报告「用户会话教训」
//!   节 + 报告落盘成功后 [`ack_lessons`]（at-least-once）；
//! - veto 回传：events.jsonl veto/forget 行 → 语义分析负样本注记
//!   （「勿重提同类」，与 userprofile 反思提示词同款纪律）；
//! - Dashboard 手动反思 / CLI `forge reflect` 只读展示不 ack
//!   （避免 UI 点一下就把队列消费掉）。
//!
//! 纪律：
//! - lesson **永不进用户画像注入面**（[`store::upsert_habit`] 入口硬拒，
//!   双保险——即使反思解析层误路由，agent 教训也进不了用户的配合姿态）；
//! - 队列文件是**学习侧旁路数据**：append 失败静默（不炸 sweep）；ack
//!   是显式消费动作，失败要回报（Result）；
//! - id 用 UTC 纳秒时间戳（同批多条连续 append 不撞；消费方 ack 按 id
//!   精确确认）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 待消费。
pub const LESSON_STATUS_PENDING: &str = "pending";
/// 已被 Forge 消费确认（留痕不删——队列文件同时是分流审计记录）。
pub const LESSON_STATUS_ACKED: &str = "acked";

/// 教训条目（outbound_lessons.jsonl 行）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LessonEntry {
    /// 稳定 id（UTC 纳秒；ack 按此精确确认）。
    pub id: String,
    /// 落盘时刻（epoch 毫秒）。
    pub ts_ms: i64,
    /// 分流标记，恒 `"lesson"`（与 [`super::store::HabitEntry`] 的 kind
    /// 字段同词表——主语判定标准：主语=助手/LLM）。
    pub kind: String,
    /// 类别（与反思条目同白名单：workflow|correction|tool_preference|style|
    /// environment|quality_bar）。
    pub category: String,
    /// 教训陈述（一句话）。
    pub content: String,
    /// 置信度 0.0-1.0。
    pub confidence: f64,
    /// 溯源（会话键等；反思通道当前不产 evidence 时为空）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// 支撑会话数（反思通道单批归纳无精确计数，诚实填 0）。
    #[serde(default)]
    pub session_count: u32,
    /// 生命周期：pending → acked（Forge 消费确认）。
    pub status: String,
}

/// 队列文件路径（唯一拼接点；与 reflections.jsonl 同目录）。
pub fn outbound_lessons_path(workspace: &Path) -> PathBuf {
    workspace
        .join("forge")
        .join("userprofile")
        .join("outbound_lessons.jsonl")
}

/// append 写串行（反思 sweep 与未来 Forge 消费 ack 并发的行交错防线；
/// telemetry.rs 同族模式）。
static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
/// id 单调分配器（上次分配的 nanos）：时钟粒度碰撞/回拨防线——同批连续
/// append 或并发分配时保证严格递增不重复（撞 id 会让 ack 误确认多条）。
static LAST_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 分配唯一递增 id（UTC 纳秒；与上次分配值比较取大者，撞了 +1）。
fn next_id() -> String {
    use std::sync::atomic::Ordering;
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let mut prev = LAST_ID.load(Ordering::Relaxed);
    loop {
        let candidate = now.max(prev.saturating_add(1));
        match LAST_ID.compare_exchange_weak(prev, candidate, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return candidate.to_string(),
            Err(p) => prev = p,
        }
    }
}

/// 追加教训入队（分配 id + pending；失败静默——学习侧旁路数据，绝不炸
/// sweep 主路径）。
pub fn append_lessons(workspace: &Path, lessons: &[LessonEntry]) {
    if lessons.is_empty() {
        return;
    }
    let path = outbound_lessons_path(workspace);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _w = WRITE_LOCK.lock();
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write;
    for mut l in lessons.iter().cloned() {
        l.id = next_id();
        l.kind = "lesson".into();
        l.status = LESSON_STATUS_PENDING.into();
        if let Ok(line) = serde_json::to_string(&l) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// 读待消费教训（status=pending，时间升序尾部取 `limit` 条；缺失/损坏行
/// 跳过）。Forge 未来消费入口。
pub fn read_pending_lessons(workspace: &Path, limit: usize) -> Vec<LessonEntry> {
    let mut pending: Vec<LessonEntry> = all_lessons(workspace)
        .into_iter()
        .filter(|l| l.status == LESSON_STATUS_PENDING)
        .collect();
    let start = pending.len().saturating_sub(limit);
    pending.split_off(start)
}

/// 确认消费：按 id 置 status=acked（留痕不删）。返回确认条数；id 不存在
/// 不报错（消费方重试语义友好）。写回原子落盘，损坏行原样保留。
pub fn ack_lessons(workspace: &Path, ids: &[String]) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    let path = outbound_lessons_path(workspace);
    let content = std::fs::read_to_string(&path).map_err(|e| format!("read lessons: {e}"))?;
    let mut acked = 0usize;
    let mut out = String::new();
    for line in content.lines() {
        match serde_json::from_str::<LessonEntry>(line) {
            Ok(mut l) => {
                if l.status == LESSON_STATUS_PENDING && ids.iter().any(|i| i == &l.id) {
                    l.status = LESSON_STATUS_ACKED.into();
                    acked += 1;
                }
                if let Ok(line) = serde_json::to_string(&l) {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            Err(_) => {
                // 损坏行原样保留（ack 不吃数据；下轮消费方自见）。
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let _w = WRITE_LOCK.lock();
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write lessons: {e}"))?;
    Ok(acked)
}

/// 全量读取（缺失 = 空 vec；损坏行跳过）。
fn all_lessons(workspace: &Path) -> Vec<LessonEntry> {
    let Ok(content) = std::fs::read_to_string(outbound_lessons_path(workspace)) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<LessonEntry>(l).ok())
        .collect()
}

/// 队列分流统计（批 D Dashboard 统计面：pending/acked 计数，只展示
/// 不注入）。缺失文件 = (0, 0)。
pub fn lesson_stats(workspace: &Path) -> (usize, usize) {
    let mut pending = 0usize;
    let mut acked = 0usize;
    for l in all_lessons(workspace) {
        if l.status == LESSON_STATUS_PENDING {
            pending += 1;
        } else if l.status == LESSON_STATUS_ACKED {
            acked += 1;
        }
    }
    (pending, acked)
}

#[cfg(test)]
mod tests;
