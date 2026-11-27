//! 用户习惯 API 面（Forge 化重构批一）：web handler 与 CLI 的共源。
//!
//! veto/forget 是**负样本**入口（G7 证据面）：操作落 store 的同时落
//! events.jsonl（`{kind, content, ts}`），反思通道下轮把它们当「不要再提
//! 同类结论」的注记回灌 prompt——用户的否定从「只删一条」升级为「教学习
//! 器一个反例」。

use std::path::Path;

use super::collection::append_event;
use super::store::{self, ProfileScope, UserProfile};

/// 否决（按内容子串匹配 → vetoed 驻留抑制）+ 负反馈事件落盘。
/// `scope` 指定操作哪份画像（对话/项目）；`home` = append_event 总闸
/// （复查 2026-11-25 批 F3，学习关闭时不落）。
pub fn veto(
    workspace: &Path,
    scope: ProfileScope,
    home: Option<&str>,
    content_sub: &str,
) -> Result<bool, String> {
    let hit = store::veto_habit(workspace, scope, content_sub)?;
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
pub fn forget(
    workspace: &Path,
    scope: ProfileScope,
    home: Option<&str>,
    content_sub: &str,
) -> Result<bool, String> {
    let removed = store::forget_habit(workspace, scope, content_sub)?;
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
pub fn pin(
    workspace: &Path,
    scope: ProfileScope,
    content_sub: &str,
    pinned: bool,
) -> Result<bool, String> {
    store::set_pinned(workspace, scope, content_sub, pinned)
}

// ---------- 观察队列用户侧消费面（接口二·待确认观察，2026-11-24） ----------

/// 确认观察提名入画像：resolve(confirmed) → [`store::ingest_external_entry`]
/// （source=`"observation"` 强制标记——画像条目带来源，UI 徽标「来自
/// 反思观察」；stats 归 external 桶）。id 不存在 / 已裁决 = Ok(None)。确认
/// 走 ingest 而非直写 = 安全边界过滤（security 语义内容照拒）与合并语义
/// 与 Forge 抛入同源。
///
/// 路由真相源 = 条目自带 `scope`（提名时按来源反思钉死；横扫
/// 2026-11-26 根修：此前只按 handler 参数路由，项目反思提名的观察会
/// 误落对话画像）。参数 `scope` 只做旧队列文件（无 scope 键 = 空串）的
/// 位置兜底。条目 scope 非法值 = **resolve 之前** loud 拒绝（不可逆裁决
/// 前先验参，BUG-20261125-052 同族纪律）。
///
/// 安全禁词预检（BUG-20261125-053）：含禁词的观察在 resolve **之前**
/// loud 报错拒绝——resolve 是不可逆裁决，先 resolve 再让 upsert 层静默
/// 丢弃 = 观察被消费、画像无条目、UI 显示成功三方失配（BUG-20261125-052
/// 同族）。拒绝时条目留 pending，人工走「忽略」转负样本回流。
pub fn confirm_observation(
    workspace: &Path,
    scope: ProfileScope,
    id: &str,
) -> Result<Option<()>, String> {
    // peek 一次做两道 resolve 前置检查：安全禁词 + scope 合法性。
    if let Some(o) = super::bridge::peek_pending_observation(workspace, id) {
        if nemesis_types::safety::violates_security_boundary(&o.content) {
            return Err(format!(
                "观察 {} 含安全边界禁词，拒绝入画像（审批放行语义只走 approval 机制；可改用「忽略」转为负样本）",
                id
            ));
        }
        if !o.scope.is_empty() && super::store::ProfileScope::from_slug(&o.scope).is_none() {
            return Err(format!(
                "观察 {} 的归属画像标记非法（{}），拒绝确认——请走「忽略」或修正队列条目",
                id, o.scope
            ));
        }
    }
    let Some(o) = super::bridge::resolve_observation(workspace, id, true)? else {
        return Ok(None);
    };
    // 条目自带 scope 权威（非空 = 提名时已钉死）；空 = 旧条目回落参数。
    let target_scope = super::store::ProfileScope::from_slug(&o.scope).unwrap_or(scope);
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
        rule_id: String::new(),
        legacy_backfilled: false,
    };
    store::ingest_external_entry(workspace, target_scope, entry, "observation")?;
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

/// 读取画像（只读展示面：缺失/损坏 = 空画像容错形态）。`scope` 指定读
/// 对话画像还是项目画像（双 scope 拆分 2026-11-25）。
pub fn list(workspace: &Path, scope: ProfileScope) -> UserProfile {
    store::load_profile_scope(workspace, scope)
}

/// 条目来源五态派生（话术 AI 化批三 §4.5 的单一真相源；WSAPI
/// `profile.list` 逐条投影 `source_kind` + [`collect_stats`] 五桶统计
/// 共用——前端不再自行从 auto/source 推导徽标，两处口径永不漂移）：
/// - `"manual"`：手动条目（source=manual——用户在「注入内容」页编辑器
///   手写的习惯，双 scope 拆分 2026-11-25 新增；用户意志锚点级保护）；
/// - `"template"`：模板兜底（auto=true——确定性挖掘器固定产物，措辞 =
///   静态模板表原文）；
/// - `"observation"`：观察转正（source=observation——接口二人工确认
///   入画像的反思观察）；
/// - `"external"`：外部回抛（source 其他标记，如 learn_dialogue——
///   Forge 经验抛入）；
/// - `"ai"`：AI 总结（auto=false 且 source 空——反思自发结论、批二
///   原地改写产物、legacy 反思条目同徽标）。
pub fn source_kind(h: &store::HabitEntry) -> &'static str {
    if h.source == store::SOURCE_MANUAL {
        return "manual";
    }
    match h.source.as_str() {
        "observation" => "observation",
        s if !s.is_empty() => "external",
        _ if h.auto => "template",
        _ => "ai",
    }
}

/// 挖掘/分流统计面（批 D Dashboard 消费；**统计面永不注入**——注入克制
/// 原则，排行/热力/统计类数据只在这里展示，防上下文膨胀）。聚合：
/// - `todos`：todo 快照完成度（[`super::mining::todo_completion_stats`]，
///   与 sweep 同一 7 天窗）；
/// - `lessons`：教训队列分流计数（pending/acked，接口一消费进度）；
/// - `observations`：观察队列分流计数（pending/confirmed/dismissed，接口二
///   待确认观察进度）；
/// - `habits_by_category` / `habits_by_source`：画像条目分布（五桶来源
///   统计，manual/ai/template/observation/external——[`source_kind`]
///   同源；对话+项目两份画像合并聚合）；
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

    // 画像条目分布（按类别 / 按来源五桶——[`source_kind`] 单一派生，
    // 与 WSAPI list 徽标同口径；双 scope 拆分起合并对话+项目两份画像
    // 聚合——统计面是全局读数，scope 归属在「注入内容」页按卡片展示）。
    let mut by_category = std::collections::BTreeMap::<String, usize>::new();
    let mut by_source = std::collections::BTreeMap::<&'static str, usize>::new();
    for scope in [ProfileScope::Dialogue, ProfileScope::Project] {
        let profile = list(workspace, scope);
        for h in &profile.habits {
            *by_category.entry(h.category.clone()).or_default() += 1;
            *by_source.entry(source_kind(h)).or_default() += 1;
        }
    }
    let bucket = |k: &str| by_source.get(k).copied().unwrap_or(0);

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
            "manual": bucket("manual"),
            "ai": bucket("ai"),
            "template": bucket("template"),
            "observation": bucket("observation"),
            "external": bucket("external"),
        },
        "signals_recent": {
            "approval_cards": approval_cards,
            "estop": estop_count,
        },
    })
}
