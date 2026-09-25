//! 项目档案里程碑写入（看板项目档案 goal P2/C）。
//!
//! 五个里程碑（拆解/派发/交付/评审/状态）由**代码触发**（挂接点在
//! nemesis-web `confirm_plan` / `dispatch_issue_core`、nemesisbot
//! `write_back_board_dispatch` / `record_auto_decide`），绝不依赖 LLM 自觉。
//!
//! 写入纪律（goal C）：
//! - **不阻塞任务推进，但必须可见**——任何写失败 = WARN 日志 + 审计
//!   留痕（`archive_write_failed` 活动），调用方不受影响；存量项目
//!   （无 directory）静默跳过（debug 日志，不回填，goal §一 Out）；
//! - 所有时间戳 = master 本机时间（接收/发生时刻，chrono::Local）；
//!   worker 原生时间戳只在 P3 回传的原始记录里保留，排序键一律
//!   master 时间（Bob 时钟慢 7h 教训）。

use std::path::{Path, PathBuf};

use crate::PlannedSubIssue;
use crate::archive;
use crate::assignment::Actor;
use crate::models::Issue;
use crate::store::BoardStore;

/// 审计 action 词表：档案写入失败（诚实可见，不静默）。
pub const ACTIVITY_ARCHIVE_WRITE_FAILED: &str = "archive_write_failed";

/// issue → 项目档案目录解析：无项目 / 存量项目（directory 未绑定）= None。
fn archive_dir_of_issue(store: &BoardStore, issue: &Issue) -> Option<PathBuf> {
    let project_id = issue.project_id?;
    let project = store.get_project(project_id).ok()?;
    project.directory.as_ref().map(PathBuf::from)
}

/// 档案根是否存在且带脚手架（投影缺失 = 目录被用户手删——诚实跳过写
/// 入并留审计，不代为重建整个目录树：重建是 board.db → 投影的显式动作，
/// 不在里程碑写入路径悄悄做）。
fn scaffold_ok(root: &Path) -> bool {
    root.join("project.json").exists() && root.join("timeline.jsonl").exists()
}

/// 统一失败出口：WARN 日志 + 审计留痕（add_activity 失败再降级为日志）。
fn honest_failure(store: &BoardStore, issue: &Issue, what: &str, err: &str) {
    tracing::warn!(
        "[BoardArchive] issue {} 档案写入失败（不阻塞）：{what}: {err}",
        issue.number
    );
    if let Err(e) = store.add_activity(
        issue.id,
        &Actor::system("board-archive"),
        ACTIVITY_ARCHIVE_WRITE_FAILED,
        Some(&format!("{what}: {err}")),
    ) {
        tracing::warn!("[BoardArchive] 审计留痕也失败 issue {}: {e}", issue.number);
    }
}

/// timeline 统一入口（脚手架缺失时跳过并失败留痕）。
fn timeline(store: &BoardStore, root: &Path, issue: &Issue, kind: &str, summary: &str) {
    if let Err(e) =
        archive::append_timeline(root, kind, Some(issue.number.as_str()), "board", summary)
    {
        honest_failure(store, issue, "timeline", &e);
    }
}

// ---------------------------------------------------------------------------
// 里程碑 1：拆解完成 → docs/plan.md（子单清单 + 依赖图 + 锚点 + 标签）
// ---------------------------------------------------------------------------

/// 拆解落库后写 `docs/plan.md`（挂接点：`confirm_plan`）。
pub fn write_plan_milestone(store: &BoardStore, parent: &Issue, subs: &[PlannedSubIssue]) {
    let Some(root) = archive_dir_of_issue(store, parent) else {
        tracing::debug!(
            "[BoardArchive] issue {} 无档案目录，跳过 plan.md",
            parent.number
        );
        return;
    };
    if !scaffold_ok(&root) {
        honest_failure(
            store,
            parent,
            "plan.md",
            "档案目录脚手架缺失（project.json/timeline 不存在）",
        );
        return;
    }

    // 子单清单 + 依赖图（文本渲染：`子i → 子j` 依赖边 + 锚点行 + 标签）。
    let mut body = String::from("# 拆解计划\n\n");
    body.push_str(&format!(
        "- 父单：{} {}\n- 子单数：{}\n- 生成：{}（master 本机时间）\n\n## 子单清单\n\n",
        parent.number,
        parent.title,
        subs.len(),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    for (i, sub) in subs.iter().enumerate() {
        body.push_str(&format!("### 子{}：{}\n\n", i, sub.title));
        if !sub.description.trim().is_empty() {
            body.push_str(&format!("{}\n\n", sub.description.trim()));
        }
        if !sub.required_role.is_empty() || !sub.required_tags.is_empty() {
            body.push_str(&format!(
                "- 派发需求：role={} tags={:?}\n",
                if sub.required_role.is_empty() {
                    "—"
                } else {
                    &sub.required_role
                },
                sub.required_tags
            ));
        }
        if !sub.depends_on.is_empty() {
            body.push_str(&format!(
                "- 依赖：子{}\n",
                sub.depends_on
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("、子")
            ));
        }
        if let Some(ac) = sub
            .acceptance_criteria
            .lines()
            .next()
            .filter(|l| !l.trim().is_empty())
        {
            body.push_str(&format!("- 验收首行：{ac}\n"));
        }
        body.push('\n');
    }

    let path = root.join("docs").join("plan.md");
    let write = std::fs::write(&path, &body).map_err(|e| format!("写 {}: {e}", path.display()));
    match write {
        Ok(()) => timeline(
            store,
            &root,
            parent,
            "plan",
            &format!("拆解完成：{} 子单（docs/plan.md）", subs.len()),
        ),
        Err(e) => honest_failure(store, parent, "plan.md", &e),
    }
}

// ---------------------------------------------------------------------------
// 里程碑 2：派发落定 → records/NB-xx/dispatch.md + timeline
// ---------------------------------------------------------------------------

/// 派发落定后写 `records/NB-xx/dispatch.md`（挂接点：`dispatch_issue_core`）。
/// `baseline_marker` 自 P4 起填充（基线 commit），P2 恒 None——字段占位。
pub fn write_dispatch_milestone(
    store: &BoardStore,
    issue: &Issue,
    target: &str,
    task_id: &str,
    baseline_marker: Option<&str>,
) {
    let Some(root) = archive_dir_of_issue(store, issue) else {
        tracing::debug!(
            "[BoardArchive] issue {} 无档案目录，跳过 dispatch.md",
            issue.number
        );
        return;
    };
    if !scaffold_ok(&root) {
        honest_failure(store, issue, "dispatch.md", "档案目录脚手架缺失");
        return;
    }
    let dir = root.join("records").join(&issue.number);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        honest_failure(
            store,
            issue,
            "dispatch.md",
            &format!("创建 records/{} 失败: {e}", issue.number),
        );
        return;
    }
    let body = format!(
        "# 派发记录 {}\n\n- 时间：{}（master 本机时间）\n- 目标 worker：{target}\n- task_id：`{task_id}`\n- 基线：{}\n\n## 任务文本\n\n{}\n",
        issue.number,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        baseline_marker.unwrap_or("（P2 阶段未启用基线；P4 起填充）"),
        issue.acceptance_criteria.as_deref().unwrap_or("—"),
    );
    let path = dir.join("dispatch.md");
    match std::fs::write(&path, &body).map_err(|e| format!("写 {}: {e}", path.display())) {
        Ok(()) => timeline(
            store,
            &root,
            issue,
            "dispatch",
            &format!("派发落定 → {target}（task {task_id}）"),
        ),
        Err(e) => honest_failure(store, issue, "dispatch.md", &e),
    }
}

// ---------------------------------------------------------------------------
// 里程碑 3：交付落定 → records/NB-xx/delivery.md + timeline
// ---------------------------------------------------------------------------

/// 交付写回后写 `records/NB-xx/delivery.md`（挂接点：
/// `write_back_board_dispatch`；成功/失败交付都记录——失败尝试的执行
/// 痕迹同样是档案，goal 零信息丢失）。`worker` = 回报节点可读名。
pub fn write_delivery_milestone(
    store: &BoardStore,
    issue: &Issue,
    worker: &str,
    ok: bool,
    summary: &str,
) {
    let Some(root) = archive_dir_of_issue(store, issue) else {
        tracing::debug!(
            "[BoardArchive] issue {} 无档案目录，跳过 delivery.md",
            issue.number
        );
        return;
    };
    if !scaffold_ok(&root) {
        honest_failure(store, issue, "delivery.md", "档案目录脚手架缺失");
        return;
    }
    let dir = root.join("records").join(&issue.number);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        honest_failure(
            store,
            issue,
            "delivery.md",
            &format!("创建 records/{} 失败: {e}", issue.number),
        );
        return;
    }
    let body = format!(
        "# 交付记录 {}\n\n- 时间：{}（master 本机时间）\n- worker：{worker}\n- 结果：{}\n\n## 摘要\n\n{}\n",
        issue.number,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        if ok { "✅ 成功" } else { "⛔ 失败" },
        summary,
    );
    let path = dir.join("delivery.md");
    match std::fs::write(&path, &body).map_err(|e| format!("写 {}: {e}", path.display())) {
        Ok(()) => timeline(
            store,
            &root,
            issue,
            "delivery",
            &format!(
                "交付落定（{}，worker {worker}）",
                if ok { "成功" } else { "失败" }
            ),
        ),
        Err(e) => honest_failure(store, issue, "delivery.md", &e),
    }
}

// ---------------------------------------------------------------------------
// 里程碑 4：评审决策 → docs/review/NB-xx.md + timeline
// ---------------------------------------------------------------------------

/// 自动决策落审计后写 `docs/review/NB-xx.md`（挂接点：`record_auto_decide`）。
/// `details` = 决策流同一份 details JSON（单一事实，两种投影）。
pub fn write_review_milestone(
    store: &BoardStore,
    issue: &Issue,
    node_id: &str,
    decision: &str,
    verdict: &str,
    details: &serde_json::Value,
) {
    let Some(root) = archive_dir_of_issue(store, issue) else {
        tracing::debug!(
            "[BoardArchive] issue {} 无档案目录，跳过 review/{}.md",
            issue.number,
            issue.number
        );
        return;
    };
    if !scaffold_ok(&root) {
        honest_failure(store, issue, "review/NB-xx.md", "档案目录脚手架缺失");
        return;
    }
    let body = format!(
        "# 评审记录 {}\n\n- 时间：{}（master 本机时间）\n- 决策者节点：{node_id}\n- 决策：{decision}（verdict={verdict}）\n\n```json\n{}\n```\n",
        issue.number,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        serde_json::to_string_pretty(details).unwrap_or_else(|_| details.to_string()),
    );
    let path = root
        .join("docs")
        .join("review")
        .join(format!("{}.md", issue.number));
    match std::fs::write(&path, &body).map_err(|e| format!("写 {}: {e}", path.display())) {
        Ok(()) => timeline(
            store,
            &root,
            issue,
            "review",
            &format!("评审决策 {decision}（{verdict}）"),
        ),
        Err(e) => honest_failure(store, issue, "review/NB-xx.md", &e),
    }
}

// ---------------------------------------------------------------------------
// 里程碑 5：状态投影 → project.json 刷新
// ---------------------------------------------------------------------------

/// 刷新 project.json（name/status 投影自 board.db；无目录/目录缺失静默
/// 跳过——本函数在 project.update 等通用路径调用，不值得为投影缺失打扰）。
pub fn sync_project_manifest(store: &BoardStore, project_id: i64) {
    let Ok(project) = store.get_project(project_id) else {
        return;
    };
    let Some(dir) = project.directory.as_ref() else {
        return;
    };
    let root = PathBuf::from(dir);
    let Some(mut manifest) = archive::read_manifest(&root) else {
        tracing::debug!("[BoardArchive] project {project_id} 档案 manifest 缺失，跳过刷新");
        return;
    };
    manifest.project_id = project.id; // 回填：脚手架先建（id=0），创建后才拿到真实 id
    manifest.name = project.name.clone();
    manifest.status = project.status.clone();
    if let Err(e) = archive::write_manifest(&root, &manifest) {
        tracing::warn!("[BoardArchive] project {project_id} project.json 刷新失败（不阻塞）：{e}");
    }
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
