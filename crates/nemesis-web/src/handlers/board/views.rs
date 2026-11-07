// 机械拆分：本文件内容由 board.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

pub(super) fn issue_to_view(
    store: &BoardStore,
    issue: &nemesis_board::Issue,
) -> Result<serde_json::Value, String> {
    let comments = store.list_comments(issue.id)?;
    let activity = store.list_activity(issue.id)?;
    let subscribers = store.list_subscribers(issue.id)?;
    // 依赖上游投影（用户需求：列表/详情可见「等 NB-x」徽标）：
    // 每上游 {id, number, status}——前端渲染阻塞态（非 done = 等待）与
    // 跳转文案。单数场景上游 ≤ 拆解上限，逐点查无感；FK 保证上游存在，
    // 查询失败（防御性）降级 unknown 占位不炸视图。
    let mut depends_on = Vec::new();
    for dep_id in store.dependencies_of(issue.id)? {
        let (number, status) = match store.get_issue(dep_id) {
            Ok(up) => (up.number, up.status.as_str().to_string()),
            Err(_) => ("?".to_string(), "unknown".to_string()),
        };
        depends_on.push(serde_json::json!({
            "id": dep_id,
            "number": number,
            "status": status,
        }));
    }
    let mut v = serde_json::to_value(issue).map_err(|e| format!("serialize issue: {e}"))?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "comments".to_string(),
            serde_json::to_value(comments).unwrap_or_default(),
        );
        obj.insert(
            "activity".to_string(),
            serde_json::to_value(activity).unwrap_or_default(),
        );
        obj.insert(
            "subscribers".to_string(),
            serde_json::to_value(subscribers).unwrap_or_default(),
        );
        obj.insert("depends_on".to_string(), serde_json::json!(depends_on));
        // 在途派发投影：撤回派发按钮的显示依据（有在途才显示）；
        // None 不插键（前端 undefined falsy）。只投影撤回需要的最小面。
        if let Some(d) = store.get_active_dispatch(issue.id)? {
            obj.insert(
                "active_dispatch".to_string(),
                serde_json::json!({
                    "task_id": d.task_id,
                    "worker_id": d.worker_id,
                    "state": d.state,
                    "dispatched_at": d.dispatched_at,
                }),
            );
        }
    }
    Ok(v)
}

pub(super) fn build_filter(data: &serde_json::Value) -> Result<IssueFilter, String> {
    let mut filter = IssueFilter {
        query: get_opt_str(data, "query"),
        ..Default::default()
    };
    if let Some(s) = get_opt_str(data, "status") {
        filter.status = Some(IssueStatus::from_str(&s).ok_or_else(|| format!("未知 status: {s}"))?);
    }
    if let Some((at, aid)) = parse_assignee(data)? {
        filter.assignee = Some((at, aid));
    }
    if let Some(p) = data.get("project_id").and_then(|v| v.as_i64()) {
        filter.project_id = Some(p);
    }
    if let Some(p) = data.get("priority").and_then(|v| v.as_i64()) {
        filter.priority = Some(p as i32);
    }
    // /+（看板项目档案 goal）：用户面列表默认排除——归档项目子单 /
    // 已取消单 / hidden 单。前端两个页签（列表/看板）同走 issue.list，一处
    // 默认两处生效；显式 `include_*: true` 才放行。hidden 没有放行口
    // （永久收起语义，goal）；详情接口不走这里（单查不受影响）。
    filter.exclude_archived_projects = !data
        .get("include_archived_projects")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    filter.exclude_cancelled = !data
        .get("include_cancelled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    filter.exclude_hidden = true;
    Ok(filter)
}

/// 拒绝嵌套 patch 形态（`{id, patch:{...}}`）。本模块所有 update 命令只认
/// 扁平形态（字段直接放 data 顶层）；嵌套形态下空 patch 静默不生效还回
/// `updated:true`（前端嵌套形态排障成本高，后端必须 loud
/// 拒绝而不是靠调用方自觉）。
pub(super) fn reject_nested_patch(data: &serde_json::Value) -> Result<(), String> {
    if data.get("patch").is_some_and(|v| v.is_object()) {
        return Err(
            "不支持嵌套 patch 形态：把要更新的字段直接放在 data 顶层（扁平形态），\
             例如 {\"id\":1,\"title\":\"新标题\"}；{\"id\":1,\"patch\":{...}} 不会生效"
                .into(),
        );
    }
    Ok(())
}

pub(super) fn build_patch(data: &serde_json::Value) -> IssuePatch {
    IssuePatch {
        title: get_opt_str(data, "title"),
        description: get_opt_str(data, "description"),
        priority: data
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32),
        project_id: data.get("project_id").and_then(|v| v.as_i64()),
        due_date: data.get("due_date").and_then(|v| v.as_i64()),
        position: data.get("position").and_then(|v| v.as_i64()),
        acceptance_criteria: get_opt_str(data, "acceptance_criteria"),
        parent_issue_id: data.get("parent_issue_id").and_then(|v| v.as_i64()),
    }
}

/// WSAPI issue 载荷 → NewIssue（pub：agent 工具 `board_issue create` 复用
/// 同一份字段解析/默认值/assignee 配对语义——统一定义）。
pub fn build_new_issue(data: &serde_json::Value, actor: Actor) -> Result<NewIssue, String> {
    let mut ni = NewIssue {
        title: get_str(data, "title")?,
        description: get_opt_str(data, "description").unwrap_or_default(),
        priority: data
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32)
            .unwrap_or(nemesis_board::models::priority::MEDIUM),
        creator: actor,
        acceptance_criteria: get_opt_str(data, "acceptance_criteria"),
        due_date: data.get("due_date").and_then(|v| v.as_i64()),
        parent_issue_id: data.get("parent_issue_id").and_then(|v| v.as_i64()),
        project_id: data.get("project_id").and_then(|v| v.as_i64()),
        ..NewIssue::default()
    };
    if let Some((at, aid)) = parse_assignee(data)? {
        ni.assignee = Some(at);
        ni.assignee_id = Some(aid);
    }
    // 职能 slug 直建单面（planner 之外的手工入口；空串/空白 = 不限）。
    if let Some(p) = get_opt_str(data, "required_profession") {
        ni.required_profession = (!p.trim().is_empty()).then_some(p);
    }
    // 环境能力硬条件（批）：直建单面手工入口；字符串数组、空白项
    // 剔除（形态校验只在 planner 拆解链，这里宽容收纳）。
    if let Some(arr) = data.get("required_capabilities").and_then(|v| v.as_array()) {
        ni.required_capabilities = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::trim))
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
    }
    if let Some(o) = get_opt_str(data, "origin_type") {
        // origin_meta 透传（v21；JSON object of string → map，非字符串值
        // 诚实忽略——坐标映射约定 string 值域）。
        let mut origin_meta = std::collections::BTreeMap::new();
        if let Some(m) = data.get("origin_meta").and_then(|v| v.as_object()) {
            for (k, v) in m {
                if let Some(s) = v.as_str() {
                    origin_meta.insert(k.clone(), s.to_string());
                }
            }
        }
        ni.origin = Some(nemesis_board::models::TaskOrigin {
            origin_type: o,
            origin_id: get_opt_str(data, "origin_id").unwrap_or_default(),
            origin_meta,
        });
    }
    Ok(ni)
}

// B35「需要人的事」（注意力经济：聚合面只显示需要人的信号，避免前端
// 四处信号四连发拉全量）---

/// 单子级「需要人」归组（排他，优先级 escalated > fused > plan_failed；
/// 终态 done/cancelled 一律不归组——已收口的单不需要人）。
/// 判据（最后一条 system 评论是转人工/熔断类终态动作的落点，之后不会再有
/// system 评论直到人工介入，子串判据可靠）：
/// - 🤷 评审侧无法定案/故障转人工（board_review 三落点共用的标记）
/// - 🛑 自治重派熔断冻结停车（sweep 冻结漏斗标记）
/// - 「拆解失败」/「拆解超时」planner 链耗尽（可手动重拆或人工建子单）
fn needs_human_group_for(
    last_system_comment: Option<&str>,
    status: nemesis_board::IssueStatus,
) -> Option<&'static str> {
    use nemesis_board::IssueStatus;
    if matches!(status, IssueStatus::Done | IssueStatus::Cancelled) {
        return None;
    }
    let c = last_system_comment?;
    if c.contains('🤷') {
        Some("escalated")
    } else if c.contains('🛑') {
        Some("fused")
    } else if c.contains("拆解失败") || c.contains("拆解超时") {
        Some("plan_failed")
    } else {
        None
    }
}

/// 截断系统评论为 reason 摘要（单行 + 160 字上限）。
fn truncate_reason(c: &str) -> String {
    let one_line: String = c
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .collect();
    one_line.chars().take(160).collect()
}

/// B35 聚合（四组一次返回）：
/// - **escalated 待裁决**：停车档案在场（✋ 强信号，与 needs_user 同源投影；
///   状态闸 InReview|InProgress——档案是漏归档残留则不亮牌）或最后
///   system 评论含 🤷。
/// - **fused 冻结**：最后 system 评论含 🛑（重派熔断，转人工处置）。
/// - **plan_failed 拆解失败**：最后 system 评论含「拆解失败」/「拆解超时」。
/// - **shadow 影子待 promote**（规则维度，B34/D2-D3 拍板产物）：mode=shadow
///   且触发过（last_run_at 在场）且启用的规则——单子历史按 origin 反查，
///   这里给规则清单。
pub(super) fn aggregate_needs_human(
    store: &BoardStore,
    handoffs: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let issues = store.list_issues(&nemesis_board::models::IssueFilter::default())?;
    let mut escalated = Vec::new();
    let mut fused = Vec::new();
    let mut plan_failed = Vec::new();
    for issue in &issues {
        // ① 待裁决：档案在场（needs_user_projection 自带状态闸）优先。
        let nu = needs_user_projection(handoffs, issue);
        let last_sys = store.last_system_comment(issue.id)?;
        let group = if nu.is_some() {
            Some("escalated")
        } else {
            needs_human_group_for(last_sys.as_deref(), issue.status)
        };
        let Some(group) = group else { continue };
        let row = serde_json::json!({
            "id": issue.id,
            "number": issue.number,
            "title": issue.title,
            "status": issue.status.as_str(),
            "priority": issue.priority,
            "reason": last_sys.as_deref().map(truncate_reason).unwrap_or_default(),
            "needs_user": nu,
        });
        match group {
            "escalated" => escalated.push(row),
            "fused" => fused.push(row),
            _ => plan_failed.push(row),
        }
    }
    let shadow: Vec<serde_json::Value> = store
        .list_autopilots()?
        .into_iter()
        .filter(|ap| {
            ap.enabled
                && ap.mode == nemesis_board::models::AUTOPILOT_MODE_SHADOW
                && ap.last_run_at.is_some()
        })
        .map(|ap| {
            serde_json::json!({
                "id": ap.id,
                "name": ap.name,
                "title": ap.title,
                "cron": ap.cron,
                "last_run_at": ap.last_run_at,
            })
        })
        .collect();
    let total = escalated.len() + fused.len() + plan_failed.len() + shadow.len();
    Ok(serde_json::json!({
        "groups": {
            "escalated": escalated,
            "fused": fused,
            "plan_failed": plan_failed,
            "shadow": shadow,
        },
        "total": total,
    }))
}
