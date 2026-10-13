//! U-C（2026-10-12 停车现场工程 §9.3）：单子级全链路时间线聚合。
//!
//! 单一真相源：WSAPI `board.timeline`（运行态查询）与 dossier `timeline.md`
//! （收口导出）复用同一聚合入口 [`collect_issue_timeline`]——收口导出与
//! 运行态查询不两套逻辑。素材全部已在（board.db 三表：activity_log /
//! comment / issue_dispatch），纯聚合投影，零新写入面。
//!
//! 集群侧事件（任务结果 / 续行快照在场）与项目档案事件（archive
//! timeline.jsonl）由调用方经 `extra` 注入——nemesis-board 不依赖
//! nemesis-cluster，依赖方向保持干净。本聚合同时是 U-A handoff /
//! U-B 中断档案的数据源（「每个错误可追溯」的查询面）。

use crate::store::BoardStore;

/// 时间线事件（统一投影条目）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimelineEvent {
    /// epoch 秒（board.db 时刻口径）。外部源（rfc3339）解析失败 =
    /// `i64::MIN`：排最前、保序不丢，不因一行坏时间戳拒绝整个投影。
    pub ts: i64,
    /// 事件源：`activity`（含 auto_decide 审计决策）/ `comment` /
    /// `dispatch` / `archive`（项目档案 timeline.jsonl）/
    /// `cluster_task`（集群任务结果）/ `continuation`（续行快照在场）。
    pub source: &'static str,
    /// 类型：activity action / comment ctype / dispatch state /
    /// archive kind / success|failure。
    pub kind: String,
    /// 行为者（`kind/id` 或 worker id）。
    pub actor: String,
    /// 单行摘要。
    pub summary: String,
    /// 附加详情（activity details JSON / 评论全文 / 任务 error）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 同一时刻的稳定 tie-break（沿 dossier 既有分层：档案 < 活动/派发 < 评论）。
fn source_rank(source: &str) -> u8 {
    match source {
        "archive" => 0,
        "activity" | "dispatch" | "cluster_task" | "continuation" => 1,
        "comment" => 2,
        _ => 3,
    }
}

/// rfc3339 → epoch 秒（外部源时间戳归一到 board.db 口径）；解析失败 =
/// `i64::MIN`（排最前、保序不丢）。
pub fn parse_ts_to_epoch(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp())
        .unwrap_or(i64::MIN)
}

/// 聚合单子时间线：activity（含 auto_decide 审计决策）+ comments +
/// dispatches + caller 注入的 `extra`（集群任务状态 / 项目档案事件），
/// 按 (ts, source_rank) 升序。单个数据源读失败降级为缺席（投影不炸）。
pub fn collect_issue_timeline(
    store: &BoardStore,
    issue_id: i64,
    extra: Vec<TimelineEvent>,
) -> Vec<TimelineEvent> {
    let mut events: Vec<TimelineEvent> = Vec::new();

    // board.db 活动（含 auto_decide 审计决策——record_auto_decide 落库即
    // action="auto_decide"，审计与活动同表，天然同源）。
    if let Ok(list) = store.list_activity(issue_id) {
        for a in list {
            events.push(TimelineEvent {
                ts: a.created_at,
                source: "activity",
                kind: a.action.clone(),
                actor: format!("{}/{}", a.actor.kind, a.actor.id),
                summary: a.action,
                detail: a.details,
            });
        }
    }
    // 评论（摘要 = 首行，全文进 detail）。
    if let Ok(list) = store.list_comments(issue_id) {
        for c in list {
            let first_line = c.content.lines().next().unwrap_or("").to_string();
            events.push(TimelineEvent {
                ts: c.created_at,
                source: "comment",
                kind: c.ctype.as_str().to_string(),
                actor: format!("{}/{}", c.author.kind, c.author.id),
                summary: first_line,
                detail: Some(c.content),
            });
        }
    }
    // 派发行（state 即当前状态——运行态投影；task_id 进 detail）。
    if let Ok(list) = store.list_dispatches(issue_id) {
        for d in list {
            events.push(TimelineEvent {
                ts: d.dispatched_at,
                source: "dispatch",
                kind: d.state.clone(),
                actor: d.worker_id.clone(),
                summary: format!(
                    "派发 → {}（task {}，状态 {}）",
                    d.worker_id, d.task_id, d.state
                ),
                detail: Some(d.task_id),
            });
        }
    }

    events.extend(extra);
    events.sort_by_key(|e| (e.ts, source_rank(e.source)));
    events
}

#[cfg(test)]
mod tests;
