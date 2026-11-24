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
//!   **2026-11-24 更新**：接口二的**用户侧消费面**已落地——反思通道低于
//!   置信度闸的 habit 类边缘结论不再丢弃，落本模块**观察队列**
//!   （`observations.jsonl`，见 [`ObservationEntry`]），Dashboard
//!   「待确认观察」人工裁决：确认 = `store::ingest_external_entry`
//!   （source=`"observation"` 强制标记）入画像；忽略 = 负样本事件回流
//!   （「勿重提同类」）。Forge 侧回抛仍按上文挂账（无已分类用户经验）。
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
//!   精确确认）；
//! - 队列清理走 [`prune_lessons_older_than`] /
//!   [`prune_observations_older_than`]（**底层接口**，WSAPI 消费；UI 按钮
//!   后接——清理是显式用户动作，不挂 sweep 自动删数据）。

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

// ---------- 观察队列（接口二·待确认观察，2026-11-24） ----------

/// 待人工确认。
pub const OBS_STATUS_PENDING: &str = "pending";
/// 用户确认（已经 [`super::store::ingest_external_entry`] 入画像）。
pub const OBS_STATUS_CONFIRMED: &str = "confirmed";
/// 用户忽略（负样本事件已回流）。
pub const OBS_STATUS_DISMISSED: &str = "dismissed";
/// pending 提名上限（反思洪泛防线：到顶 = 新提名不再入队；已入队结论
/// 照常进 ledger 防每轮重提——用户处理积压后通道自然恢复。只把实际
/// 入队的提名写 ledger 的裁决在 [`super`] sweep_once 侧）。
pub const OBS_PENDING_CAP: usize = 20;

/// 观察提名条目（`observations.jsonl` 行）：反思通道**低于置信度闸但高于
/// 观察下限**的 habit 类结论——不够格自动生效，也不值得直接丢弃，交
/// 人工裁决（「待确认观察」）。确认 → `store::ingest_external_entry`
/// （source=`"observation"`）入画像；忽略 → events.jsonl 负样本回流。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservationEntry {
    /// 稳定 id（UTC 纳秒；裁决按此精确匹配）。
    pub id: String,
    /// 落盘时刻（epoch 毫秒）。
    pub ts_ms: i64,
    /// 分流标记，恒 `"observation"`（与 lesson/habit 同族 kind 词表）。
    pub kind: String,
    /// 类别（与反思条目同白名单）。
    pub category: String,
    /// 观察陈述（一句话）。
    pub content: String,
    /// 置信度 0.0-1.0（恒 < 置信度闸——过闸的直接进画像不经本队列）。
    pub confidence: f64,
    /// 仪式步骤（反思通道带出；确认时随条目入画像）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    /// 生命周期：pending → confirmed | dismissed。
    pub status: String,
    /// 裁决时刻（epoch 毫秒；pending = None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ts_ms: Option<i64>,
}

/// 观察队列文件路径（唯一拼接点；与 outbound_lessons.jsonl 同目录）。
pub fn observations_path(workspace: &Path) -> PathBuf {
    workspace
        .join("forge")
        .join("userprofile")
        .join("observations.jsonl")
}

/// 全量读取观察队列（缺失 = 空 vec；损坏行跳过）。
fn all_observations(workspace: &Path) -> Vec<ObservationEntry> {
    let Ok(content) = std::fs::read_to_string(observations_path(workspace)) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<ObservationEntry>(l).ok())
        .collect()
}

/// 追加观察提名（分配 id + pending）。pending 到顶 = 新提名丢弃，返回
/// 实际入队条数（调用方只把入队提名写 ledger——丢弃的结论下轮证据支持
/// 时可重提，不被锁死）。失败静默（学习侧旁路数据，绝不炸 sweep 主路径）。
pub fn append_observations(workspace: &Path, entries: &[ObservationEntry]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    // pending 现量（读不加锁——旁路面的宽松计数；写侧 WRITE_LOCK 保行
    // 完整性，裁决/清理是显式用户动作，与 sweep append 的竞窗代价上限 =
    // 暂时超 cap 几条，无害）。
    let pending_now = all_observations(workspace)
        .iter()
        .filter(|o| o.status == OBS_STATUS_PENDING)
        .count();
    let budget = OBS_PENDING_CAP.saturating_sub(pending_now);
    if budget == 0 {
        return 0;
    }
    let admitted = budget.min(entries.len());
    let path = observations_path(workspace);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return 0;
    }
    let _w = WRITE_LOCK.lock();
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return 0;
    };
    use std::io::Write;
    for mut o in entries.iter().take(admitted).cloned() {
        o.id = next_id();
        o.kind = "observation".into();
        o.status = OBS_STATUS_PENDING.into();
        if let Ok(line) = serde_json::to_string(&o) {
            let _ = writeln!(f, "{line}");
        }
    }
    admitted
}

/// 读待裁决观察提名（status=pending，时间升序尾部取 `limit` 条）。
pub fn read_pending_observations(workspace: &Path, limit: usize) -> Vec<ObservationEntry> {
    let mut pending: Vec<ObservationEntry> = all_observations(workspace)
        .into_iter()
        .filter(|o| o.status == OBS_STATUS_PENDING)
        .collect();
    let start = pending.len().saturating_sub(limit);
    pending.split_off(start)
}

/// 裁决观察提名：pending → confirmed/dismissed（留痕不删——队列文件同时
/// 是提名审计记录）。返回裁决后的条目（供确认路径构造画像条目）；id 不
/// 存在 / 已裁决 = Ok(None)。写回原子落盘，损坏行原样保留。
pub fn resolve_observation(
    workspace: &Path,
    id: &str,
    confirm: bool,
) -> Result<Option<ObservationEntry>, String> {
    let path = observations_path(workspace);
    let content = std::fs::read_to_string(&path).map_err(|e| format!("read observations: {e}"))?;
    let mut resolved: Option<ObservationEntry> = None;
    let mut out = String::new();
    for line in content.lines() {
        match serde_json::from_str::<ObservationEntry>(line) {
            Ok(mut o) => {
                if o.status == OBS_STATUS_PENDING && o.id == id {
                    o.status = if confirm {
                        OBS_STATUS_CONFIRMED.into()
                    } else {
                        OBS_STATUS_DISMISSED.into()
                    };
                    o.resolved_ts_ms = Some(chrono::Utc::now().timestamp_millis());
                    resolved = Some(o.clone());
                }
                if let Ok(line) = serde_json::to_string(&o) {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            Err(_) => {
                // 损坏行原样保留（裁决不吃数据）。
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let _w = WRITE_LOCK.lock();
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write observations: {e}"))?;
    Ok(resolved)
}

/// 观察队列分流统计（pending/confirmed/dismissed 计数，只展示不注入）。
/// 缺失文件 = (0, 0, 0)。
pub fn observation_stats(workspace: &Path) -> (usize, usize, usize) {
    let mut pending = 0usize;
    let mut confirmed = 0usize;
    let mut dismissed = 0usize;
    for o in all_observations(workspace) {
        match o.status.as_str() {
            OBS_STATUS_PENDING => pending += 1,
            OBS_STATUS_CONFIRMED => confirmed += 1,
            OBS_STATUS_DISMISSED => dismissed += 1,
            _ => {}
        }
    }
    (pending, confirmed, dismissed)
}

// ---------- 队列清理（底层接口，2026-11-24；UI 按钮后接） ----------

/// 清理教训队列：acked 且落盘时刻早于 `cutoff_ms` 的条目整行删除
/// （**pending 永不动**——未消费数据不是清理对象；损坏行原样保留）。
/// 返回删除条数。缺文件 = Ok(0)。
pub fn prune_lessons_older_than(workspace: &Path, cutoff_ms: i64) -> Result<usize, String> {
    let path = outbound_lessons_path(workspace);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("read lessons: {e}")),
    };
    let mut pruned = 0usize;
    let mut out = String::new();
    for line in content.lines() {
        match serde_json::from_str::<LessonEntry>(line) {
            Ok(l) => {
                if l.status == LESSON_STATUS_ACKED && l.ts_ms < cutoff_ms {
                    pruned += 1;
                    continue; // 整行删除
                }
                out.push_str(line);
                out.push('\n');
            }
            Err(_) => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if pruned == 0 {
        return Ok(0); // 无删除不重写（省一次原子写）
    }
    let _w = WRITE_LOCK.lock();
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write lessons: {e}"))?;
    Ok(pruned)
}

/// 清理观察队列：已裁决（confirmed/dismissed）且裁决时刻早于 `cutoff_ms`
/// 的提名整行删除（**pending 永不动**——待人工处理的数据不是清理对象；
/// 损坏行原样保留）。裁决时刻缺省回落落盘时刻。返回删除条数。缺文件 =
/// Ok(0)。
pub fn prune_observations_older_than(workspace: &Path, cutoff_ms: i64) -> Result<usize, String> {
    let path = observations_path(workspace);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("read observations: {e}")),
    };
    let mut pruned = 0usize;
    let mut out = String::new();
    for line in content.lines() {
        match serde_json::from_str::<ObservationEntry>(line) {
            Ok(o) => {
                let resolved_at = o.resolved_ts_ms.unwrap_or(o.ts_ms);
                let is_resolved =
                    o.status == OBS_STATUS_CONFIRMED || o.status == OBS_STATUS_DISMISSED;
                if is_resolved && resolved_at < cutoff_ms {
                    pruned += 1;
                    continue;
                }
                out.push_str(line);
                out.push('\n');
            }
            Err(_) => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if pruned == 0 {
        return Ok(0);
    }
    let _w = WRITE_LOCK.lock();
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write observations: {e}"))?;
    Ok(pruned)
}

#[cfg(test)]
mod tests;
