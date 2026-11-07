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
