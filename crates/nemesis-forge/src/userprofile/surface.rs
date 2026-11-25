//! 用户习惯 API 面（Forge 化重构批一）：web handler 与 CLI 的共源。
//!
//! veto/forget 是**负样本**入口（G7 证据面）：操作落 store 的同时落
//! events.jsonl（`{kind, content, ts}`），反思通道下轮把它们当「不要再提
//! 同类结论」的注记回灌 prompt——用户的否定从「只删一条」升级为「教学习
//! 器一个反例」。

use std::path::Path;

use super::collection::append_event;
use super::store::{self, UserProfile};

/// 否决（按内容子串匹配 → vetoed 驻留抑制）+ 负反馈事件落盘。
/// `home` = append_event 总闸（复查 2026-11-25 批 F3，学习关闭时不落）。
pub fn veto(workspace: &Path, home: Option<&str>, content_sub: &str) -> Result<bool, String> {
    let hit = store::veto_habit(workspace, content_sub)?;
    if hit {
        append_event(
            workspace,
            home,
            &serde_json::json!({
                "kind": "veto",
                "content": content_sub,
                "ts_ms": chrono::Utc::now().timestamp_millis(),
            }),
        );
    }
    Ok(hit)
}

/// 忘记（按内容子串整条删除）+ 负反馈事件落盘。
pub fn forget(workspace: &Path, home: Option<&str>, content_sub: &str) -> Result<bool, String> {
    let removed = store::forget_habit(workspace, content_sub)?;
    if removed {
        append_event(
            workspace,
            home,
            &serde_json::json!({
                "kind": "forget",
                "content": content_sub,
                "ts_ms": chrono::Utc::now().timestamp_millis(),
            }),
        );
    }
    Ok(removed)
}

/// pin / unpin（钉住 = 不衰减、渲染置顶；无事件面——正反馈不影响学习器）。
pub fn pin(workspace: &Path, content_sub: &str, pinned: bool) -> Result<bool, String> {
    store::set_pinned(workspace, content_sub, pinned)
}

// ---------- 观察队列用户侧消费面（接口二·待确认观察，2026-11-24） ----------

/// 确认观察提名入画像：resolve(confirmed) → [`store::ingest_external_entry`]
/// （source=`"observation"` 强制标记——画像条目带来源，UI 徽标「来自
/// 反思观察」；stats 归 external 桶）。id 不存在 / 已裁决 = Ok(None)。
/// 确认走 ingest 而非直写 = 安全边界过滤（security 语义内容照拒）与
/// 合并语义与 Forge 抛入同源。
///
/// 安全禁词预检（BUG-20261125-053）：含禁词的观察在 resolve **之前**
/// loud 报错拒绝——resolve 是不可逆裁决，先 resolve 再让 upsert 层静默
/// 丢弃 = 观察被消费、画像无条目、UI 显示成功三方失配（BUG-20261125-052
/// 同族）。拒绝时条目留 pending，人工走「忽略」转负样本回流。
pub fn confirm_observation(workspace: &Path, id: &str) -> Result<Option<()>, String> {
    if let Some(o) = super::bridge::peek_pending_observation(workspace, id)
        && nemesis_types::safety::violates_security_boundary(&o.content)
    {
        return Err(format!(
            "观察 {} 含安全边界禁词，拒绝入画像（审批放行语义只走 approval 机制；可改用「忽略」转为负样本）",
            id
        ));
    }
    let Some(o) = super::bridge::resolve_observation(workspace, id, true)? else {
        return Ok(None);
    };
    let now = chrono::Local::now().to_rfc3339();
    let entry = store::HabitEntry {
        category: o.category,
        content: o.content,
        confidence: o.confidence,
        hits: 1,
        first_seen: now.clone(),
        last_seen: now,
        status: store::STATUS_ACTIVE.into(),
        pinned: false,
        steps: o.steps,
        scope: String::new(),
        evidence: vec![format!("observation:{}", o.id)],
        auto: false,
        kind: store::KIND_HABIT.into(),
        source: String::new(), // ingest_external_entry 强制覆盖为 "observation"
    };
    store::ingest_external_entry(workspace, entry, "observation")?;
    Ok(Some(()))
}

/// 忽略观察提名（负样本回流）：resolve(dismissed) + events.jsonl
/// `kind="dismiss"` 行——反思通道下轮当「勿重提同类」注记回灌（与
/// veto/forget 同族纪律）；忽略的结论同时进 Forge 语义分析的负样本面。
/// id 不存在 / 已裁决 = Ok(None)。
pub fn dismiss_observation(
    workspace: &Path,
    home: Option<&str>,
    id: &str,
) -> Result<Option<String>, String> {
    let Some(o) = super::bridge::resolve_observation(workspace, id, false)? else {
        return Ok(None);
    };
    append_event(
        workspace,
        home,
        &serde_json::json!({
            "kind": "dismiss",
            "content": o.content,
            "ts_ms": chrono::Utc::now().timestamp_millis(),
        }),
    );
    Ok(Some(o.content))
}

/// 读待裁决观察提名（UI 列表面；尾部 `limit` 条 pending）。
pub fn pending_observations(
    workspace: &Path,
    limit: usize,
) -> Vec<super::bridge::ObservationEntry> {
    super::bridge::read_pending_observations(workspace, limit)
}

/// 开启画像（enabled_at 锚点；幂等）。`max_entries` 由调用方从 config
/// 现读传入。
pub fn enable(workspace: &Path, max_entries: usize) -> Result<UserProfile, String> {
    store::enable_profile(workspace, max_entries)
}

/// 读取画像（只读展示面：缺失/损坏 = 空画像容错形态）。
pub fn list(workspace: &Path) -> UserProfile {
    store::load_profile(workspace)
}

/// 挖掘/分流统计面（批 D Dashboard 消费；**统计面永不注入**——注入克制
/// 原则，排行/热力/统计类数据只在这里展示，防上下文膨胀）。聚合：
/// - `todos`：todo 快照完成度（[`super::mining::todo_completion_stats`]，
///   与 sweep 同一 7 天窗）；
/// - `lessons`：教训队列分流计数（pending/acked，接口一消费进度）；
/// - `observations`：观察队列分流计数（pending/confirmed/dismissed，接口二
///   待确认观察进度）；
/// - `habits_by_category` / `habits_by_source`：画像条目分布（分流统计）；
/// - `signals_recent`：交互强信号近 200 行计数（审批卡/急停——批 C 采集
///   面存活的直观读数）。
///
/// 各通道独立容错（缺文件 = 空统计），统计失败不互相陪葬。
pub fn collect_stats(workspace: &Path) -> serde_json::Value {
    use super::{bridge, collection, mining};

    // todo 完成度（读失败 = 空统计诚实降级）。
    let todos = match collection::collect_todos(
        workspace,
        chrono::Utc::now().timestamp_millis(),
        super::TODO_MAX_AGE_MS,
    ) {
        Ok(snapshots) => {
            let s = mining::todo_completion_stats(&snapshots);
            serde_json::json!({
                "snapshot_count": s.snapshot_count,
                "item_count": s.item_count,
                "completed_count": s.completed_count,
                "completion_ratio": s.completion_ratio,
                "snapshots_with_pending": s.snapshots_with_pending,
            })
        }
        Err(_) => serde_json::json!({
            "snapshot_count": 0, "item_count": 0, "completed_count": 0,
            "completion_ratio": 0.0, "snapshots_with_pending": 0,
        }),
    };

    // 教训队列分流计数。
    let (pending, acked) = bridge::lesson_stats(workspace);

    // 观察队列分流计数（接口二·待确认观察）。
    let (obs_pending, obs_confirmed, obs_dismissed) = bridge::observation_stats(workspace);

    // 画像条目分布（按类别 / 按来源分流）。
    let profile = list(workspace);
    let mut by_category = std::collections::BTreeMap::<String, usize>::new();
    let mut by_source_auto = 0usize;
    let mut by_source_manual = 0usize;
    let mut by_source_external = 0usize;
    for h in &profile.habits {
        *by_category.entry(h.category.clone()).or_default() += 1;
        if !h.source.is_empty() {
            by_source_external += 1;
        } else if h.auto {
            by_source_auto += 1;
        } else {
            by_source_manual += 1;
        }
    }

    // 交互强信号近 200 行计数（审批卡 / 急停）。
    let mut approval_cards = 0usize;
    let mut estop_count = 0usize;
    for sig in collection::read_recent_signals(workspace, 200) {
        match sig.get("kind").and_then(|k| k.as_str()) {
            Some("approval") => approval_cards += 1,
            Some("estop") => estop_count += 1,
            _ => {}
        }
    }

    serde_json::json!({
        "todos": todos,
        "lessons": { "pending": pending, "acked": acked },
        "observations": {
            "pending": obs_pending,
            "confirmed": obs_confirmed,
            "dismissed": obs_dismissed,
        },
        "habits_by_category": by_category,
        "habits_by_source": {
            "auto": by_source_auto,
            "manual": by_source_manual,
            "external": by_source_external,
        },
        "signals_recent": {
            "approval_cards": approval_cards,
            "estop": estop_count,
        },
    })
}
