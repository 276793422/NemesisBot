//! U-A（2026-10-12 停车现场工程 §9.1）：停车现场标准化（handoff 交接文档）。
//!
//! 所有停车路径（熔断 / UNSURE 闸 / 预算 / 重评耗尽 / 重拆耗尽 / 派发无
//! 匹配 / LLM 封存）统一产出双轨现场：
//! - 人可读：系统评论（调用方既有形态，追加档案注记）；
//! - 机器可读：`<workspace>/board/handoffs/NB-x.json` 侧车（本模块）。
//!
//! **LLM 接手链路**：用户对节点说「继续 NB-x」→ agent 读 handoff JSON →
//! 按 `suggested_action` 行动——`manual_check` 类才需要人，`retry`/
//! `redispatch` 类走既有入口直接续流。把「转人工」变成「转下一个 LLM
//! 会话」。verdict_chain 由 U-C timeline 聚合提取（单一真相源）；档案随
//! 单子保留（可追溯），由 dossier 收口导出汇总归档，不删。
//!
//! `stop_kind` 枚举稳定（供 Forge Reflect 远期消费，§9.4）：`unsure_streak`
//! `fail_streak` `budget` `retry_exhausted` `worker_loop` `capability_fail`
//! `infra` `defect_gate` `parent_unresolved` `project_unresolved` `no_match`
//! `plan_exhausted` `llm_unavailable`。
//!
//! `suggested_action` 枚举：`manual_check`（真机走查才需要人）/ `retry`
//! （环境修好后原入口重试）/ `redispatch`（换条件重派 worker）。

use crate::store::BoardStore;
use std::path::{Path, PathBuf};

/// 停车现场记录（调用方填写的增量字段；verdict_chain 单独传——数据源是
/// U-C timeline 聚合，由本模块统一提取）。
#[derive(Debug, Clone)]
pub struct HandoffSpec {
    /// stop_kind 枚举（见模块注释）。
    pub stop_kind: &'static str,
    /// suggested_action 枚举：manual_check | retry | redispatch。
    pub suggested_action: &'static str,
    /// 人工/LLM 裁决提示（一句话说清卡点与解法方向）。
    pub hint: String,
    /// 续做入口（CLI / WSAPI / 对话命令形态）。
    pub resume_entry: String,
    /// 环境快照（worker 节点 / 派发状态 / 节点 id 等可 cheaply 取得的字段）。
    pub environment: serde_json::Value,
    /// 已试动作 + 结果（`[{action, result}]`）。
    pub attempted: Vec<serde_json::Value>,
}

/// handoff 档案路径（`<workspace>/board/handoffs/<issue_number>.json`；
/// 每单一文件、最新停车覆盖写——verdict_chain 保全程历史，无需多版本）。
pub fn handoff_path(workspace: &Path, issue_number: &str) -> PathBuf {
    workspace
        .join("board")
        .join("handoffs")
        .join(format!("{issue_number}.json"))
}

/// 写停车现场档案。返回落盘路径；IO 失败 = Err（调用方降级：停车评论照发，
/// 档案缺席诚实 WARN——停车流程不因档案写失败而失败）。
pub fn write_handoff(
    workspace: &Path,
    issue_number: &str,
    issue_id: i64,
    spec: &HandoffSpec,
    verdict_chain: Vec<serde_json::Value>,
) -> Result<PathBuf, String> {
    let doc = serde_json::json!({
        "schema": 1,
        "issue": issue_number,
        "issue_id": issue_id,
        "stopped_at": chrono::Utc::now().to_rfc3339(),
        "stop_kind": spec.stop_kind,
        "verdict_chain": verdict_chain,
        "environment": spec.environment,
        "attempted": spec.attempted,
        "suggested_action": spec.suggested_action,
        "hint": spec.hint,
        "resume_entry": spec.resume_entry,
    });
    let path = handoff_path(workspace, issue_number);
    let parent = path
        .parent()
        .ok_or_else(|| format!("handoff 路径异常：{}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("建 handoff 目录失败: {e}"))?;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写 handoff 失败: {e}"))?;
    Ok(path)
}

/// 读取停车现场档案（「继续 NB-x」LLM 接手链路 / dossier 汇总导出）。
/// 不存在 = Err（调用方诚实告知：该单当前无停车现场）。
pub fn read_handoff(workspace: &Path, issue_number: &str) -> Result<serde_json::Value, String> {
    let path = handoff_path(workspace, issue_number);
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("读 handoff 失败（{}）: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("handoff JSON 解析失败: {e}"))
}

/// verdict_chain 提取（单一真相源 = U-C timeline 聚合）：auto_decide 活动
/// 行 = 每轮决策（决策词 + verdict + reasons 全程链，不只最后一轮）。
pub fn verdict_chain_from(store: &BoardStore, issue_id: i64) -> Vec<serde_json::Value> {
    crate::timeline::collect_issue_timeline(store, issue_id, Vec::new())
        .into_iter()
        .filter(|e| e.source == "activity" && e.kind == "auto_decide")
        .map(|e| {
            let details = e
                .detail
                .as_deref()
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .unwrap_or(serde_json::Value::Null);
            let decision = details
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("auto_decide")
                .to_string();
            let verdict = details
                .get("verdict")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            serde_json::json!({
                "at": e.ts,
                "decision": decision,
                "verdict": verdict,
                "details": details,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
