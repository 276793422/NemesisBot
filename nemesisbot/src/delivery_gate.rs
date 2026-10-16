//! P4 交付门（持久 Agent 实体 P4-c，2026-10-17）：worker 侧交付分支捕获。
//!
//! PLAN「分支/PR 交付门」的 B 端半边：任务执行前记基线 tree（git 形态才有
//! ——JSON 形态无影子库，诚实跳过不造证据），成功收口时把工作区实时 tree
//! 捕获为交付分支（`refs/heads/nemesis/run-<task_id>`，force 覆盖——重派
//! 重跑同任务同分支），patch 落盘 `logs/deliveries/<task_id>.patch` 留底，
//! capture 元数据 + patch 全文随回调 `delivery` 字段上行（≤1MB，
//! PATCH_MAX_BYTES 截断位）。A 端（master）把整包入 deliveries 表——
//! 证据包自包含，跨节点可审无需二次 RPC。
//!
//! 诚实边界：交付门是增强证据面，任何失败（无 .git / 影子库不可用 /
//! patch 写盘失败）= `None` + warn，绝不阻断任务回调主链路（回调才是
//! 结果交付的唯一路径；证据缺席可观测但不停摆）。

use nemesis_agent::r#loop::AgentLoop;

/// 任务执行前的基线 tree 快照（git 形态才有；JSON 形态 = None）。
///
/// 调用方在 `run_with_trace`/`resume_execution_with_token` **之前**取值，
/// 成功收口时原样传给 [`capture_task_delivery`]——先执行后快照会把任务
/// 自己的改动算进基线（diff 恒空，证据失真）。
pub(crate) fn delivery_base_tree(agent_loop: &AgentLoop) -> Option<String> {
    agent_loop
        .checkpoint_store()
        .and_then(|store| store.current_tree_hex())
}

/// 成功收口时的交付分支捕获（B 端）：capture + patch 落盘 + wire JSON。
///
/// `base_tree` 传 [`delivery_base_tree`] 的执行前快照；`None` = 执行前就
/// 无基线（JSON 形态 / 影子库异常）→ 按空树基线整包 diff（capture 层
/// canonical 语义）。
///
/// 返回 payload["delivery"] 值：`DeliveryBranchCapture` 序列化 + `branch`
/// /`task_id`/`node`/`patch_path` 上下文字段。任何失败 = None + warn。
pub(crate) fn capture_task_delivery(
    agent_loop: &AgentLoop,
    task_id: &str,
    base_tree: Option<&str>,
    self_node_id: &str,
) -> Option<serde_json::Value> {
    let store = agent_loop.checkpoint_store()?;
    capture_task_delivery_with_store(&store, task_id, base_tree, self_node_id)
}

/// [`capture_task_delivery`] 的可测内核：直接收影子库句柄（测试用
/// `CheckpointStore::new(None, root)` + 真实 git 夹具驱动，不必造 AgentLoop）。
pub(crate) fn capture_task_delivery_with_store(
    store: &std::sync::Arc<nemesis_agent::checkpoint::CheckpointStore>,
    task_id: &str,
    base_tree: Option<&str>,
    self_node_id: &str,
) -> Option<serde_json::Value> {
    if task_id.is_empty() {
        return None;
    }
    let branch = format!("run-{task_id}");
    let cap = match store.capture_delivery_branch(base_tree, &branch) {
        Ok(cap) => cap,
        Err(e) => {
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "[DeliveryGate] 交付分支捕获失败（证据缺席，回调照常）"
            );
            return None;
        }
    };

    // patch 落盘留底（worker 本地证据，logs/deliveries/<task_id>.patch）。
    // 失败只 warn——patch 已随 wire JSON 上行，A 端 deliveries 表自包含。
    let patch_path = nemesis_path::logs_dir_in_workspace(store.root())
        .join("deliveries")
        .join(format!("{task_id}.patch"));
    let patch_display = patch_path
        .strip_prefix(store.root())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| format!("logs/deliveries/{task_id}.patch"));
    if let Some(dir) = patch_path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(
            task_id = %task_id,
            error = %e,
            "[DeliveryGate] patch 目录创建失败（wire 证据不受影响）"
        );
    } else if let Err(e) = std::fs::write(&patch_path, cap.patch.as_bytes()) {
        tracing::warn!(
            task_id = %task_id,
            error = %e,
            "[DeliveryGate] patch 落盘失败（wire 证据不受影响）"
        );
    }

    tracing::info!(
        task_id = %task_id,
        branch = %branch,
        files_changed = cap.files_changed,
        insertions = cap.insertions,
        deletions = cap.deletions,
        patch_bytes = cap.patch.len(),
        truncated = cap.patch_truncated,
        "[DeliveryGate] 交付分支已捕获"
    );

    let mut value = serde_json::to_value(&cap).unwrap_or_else(|_| {
        // DeliveryBranchCapture 是纯数据结构，序列化实际不会失败——兜底臂
        // 保类型完整（unwrap 免疫纪律）。
        serde_json::json!({
            "ref_name": cap.ref_name,
            "commit": cap.commit,
            "tree": cap.tree,
            "base_tree": cap.base_tree,
            "patch_truncated": cap.patch_truncated,
        })
    });
    if let Some(obj) = value.as_object_mut() {
        obj.insert("branch".into(), serde_json::Value::String(branch));
        obj.insert(
            "task_id".into(),
            serde_json::Value::String(task_id.to_string()),
        );
        obj.insert(
            "node".into(),
            serde_json::Value::String(self_node_id.to_string()),
        );
        obj.insert(
            "patch_path".into(),
            serde_json::Value::String(patch_display),
        );
    }
    Some(value)
}

#[cfg(test)]
mod tests;
