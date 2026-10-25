//! P3 外循环（PR 发布 + unknown 对账，2026-10-25）：交付分支推远端 + PR
//! 创建 + 三分支状态机（成功记 PR 号 / 确定失败诚实可重试 / 未知回读对账）。
//!
//! 发布链（全部在 master 侧，操作对象 = P1 物化的任务副本）：
//! 1. **前置核验**：`{task_id}:branch` 交付行在场、非 rejected、payload 无
//!    pr_number（幂等——已发布直接回 already_published）；副本 HEAD ==
//!    payload.repo_base_sha（P2 版本绑定的发布侧消费——基线漂移拒绝发布，
//!    绝不把 patch 糊到漂移基线上）；内联 patch 在场且未截断。
//! 2. **git 半边**：副本仓内 HEAD 建分支 `nemesis/task-<task_id>` + 应用
//!    patch + commit（[`nemesis_board::git_repo::apply_patch_new_branch`]，
//!    libgit2 原生零外部 git）→ `push_branch` 推 origin（vault PAT，同
//!    clone 凭据语义；push 有 TCP 确认，失败重推幂等——同内容快进）。
//! 3. **PR 半边**：connector `create_pull`（confirm=required，走主 loop
//!    handle_tool_call 完整 dispatch 瀑布——estop/审批/安全 8 层全生效）。
//! 4. **unknown 对账**：create_pull 结果不是 PR JSON（超时/传输断/HTTP
//!    错误/截断）→ `pr_list` 按 head 分支回查——找到 = 实际已建成（记号
//!    收敛为成功，不瞎重试）；没找到 = 确定失败（错误原文留 detail，可
//!    重试：重试先撞 payload 幂等再撞对账，不重复建 PR）。
//! 5. **落账**：`mark_delivery_published`（recorded/delivered/confirmed →
//!    published 终态）+ payload 注 pr_number/pr_url/published_*。
//!
//! 诚实边界：副本基线核验只认 repo_base_sha（旧形态交付缺该键 = 拒绝
//! 发布，不静默放行）；fork PR / 跨仓库 head 不在 v1 射程（同仓 head 匹配）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_board::store::BoardStore;
use nemesis_taskstore::TaskDb;
use serde_json::{Value, json};

/// 发布终态词表（响应 `status` 字段）。
const ST_PUBLISHED: &str = "published";
const ST_ALREADY: &str = "already_published";
const ST_FAILED: &str = "failed";

/// 交付发布桥实现（nemesis-web 侧 trait + 槽，依赖反转同 deliveries 查询面）。
pub(crate) struct DeliveryPublisher {
    db: Arc<TaskDb>,
    store: Arc<BoardStore>,
    agent_loop: Arc<AgentLoop>,
    workspace: PathBuf,
}

/// gateway post_agent 装配（幂等覆盖；与 [`crate::external_source::spawn`]
/// 同闸同点——board+security+connectors 三 feature）。
pub(crate) fn install(
    db: Arc<TaskDb>,
    store: Arc<BoardStore>,
    agent_loop: Arc<AgentLoop>,
    workspace: PathBuf,
) {
    nemesis_web::handlers::deliveries::install_delivery_publish_bridge(Arc::new(
        DeliveryPublisher {
            db,
            store,
            agent_loop,
            workspace,
        },
    ));
}

#[async_trait::async_trait]
impl nemesis_web::handlers::deliveries::DeliveryPublishBridge for DeliveryPublisher {
    async fn publish(&self, args: &Value) -> Result<Value, String> {
        publish_task_delivery(
            &self.db,
            &self.store,
            &self.agent_loop,
            &self.workspace,
            args,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// 编排
// ---------------------------------------------------------------------------

/// 前置核验产物（同步、可单测——不触碰 agent loop）。
#[derive(Debug)]
enum Preflight {
    /// 幂等短路：payload 已带 pr_number，直接回 already_published 响应。
    Already(Value),
    Proceed {
        copy_dir: PathBuf,
        patch: String,
        expected_base: String,
        title: String,
    },
}

/// 结构化失败响应（发布结果是第一公民结果非传输错误）。
fn fail_outcome(task_id: &str, branch: &str, detail: String) -> Value {
    json!({
        "task_id": task_id,
        "status": ST_FAILED,
        "pr_number": Value::Null,
        "pr_url": Value::Null,
        "commit": Value::Null,
        "branch": branch,
        "detail": detail,
    })
}

/// 前置核验（① 交付行核验 + ② 副本/基线核验 + patch 提取 + 标题解析）。
/// 参数合法性（task_id/connector/owner/repo/base）由调用方先验后传入——
/// 本函数 `Err` 全部是「确定失败可重试」的前置失败面（编排层包
/// fail_outcome），不再混入请求形态错误。
fn preflight(
    db: &Arc<TaskDb>,
    store: &Arc<BoardStore>,
    task_id: &str,
    args: &Value,
) -> Result<Preflight, String> {
    validate_task_id_for_branch(task_id)?;
    let branch = format!("nemesis/task-{task_id}");
    let branch_key = format!("{task_id}:branch");

    let rec = db
        .delivery_by_key(&branch_key)?
        .ok_or_else(|| format!("无分支交付记录: {branch_key}（任务未走交付管线，无可发布物）"))?;
    if rec.kind != "branch" {
        return Err(format!(
            "交付行 {branch_key} kind={} ≠ branch——键冲突，拒绝发布",
            rec.kind
        ));
    }
    if rec.status == "rejected" {
        return Err(PREFLIGHT_REJECTED.to_string());
    }
    if let Some(n) = rec.payload.get("pr_number").and_then(|v| v.as_i64()) {
        return Ok(Preflight::Already(json!({
            "task_id": task_id,
            "status": ST_ALREADY,
            "pr_number": n,
            "pr_url": rec.payload.get("pr_url").cloned().unwrap_or(Value::Null),
            "commit": rec.payload.get("published_commit").cloned().unwrap_or(Value::Null),
            "branch": rec
                .payload
                .get("published_branch")
                .cloned()
                .unwrap_or(Value::String(branch)),
            "detail": "该交付已发布（幂等跳过，不重复建 PR）",
        })));
    }

    // 副本与基线核验。
    let copy_dir = resolve_copy_dir(store, task_id, args.get("copy_path"))?;
    let head = match nemesis_board::git_repo::head_commit_hex(&copy_dir) {
        Ok(Some(h)) => h,
        Ok(None) => return Err(PREFLIGHT_EMPTY_REPO.to_string()),
        Err(e) => return Err(e),
    };
    let expected_base = rec
        .payload
        .get("repo_base_sha")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PREFLIGHT_NO_BASE_SHA.to_string())?
        .to_string();
    if head != expected_base {
        return Err(base_drift_detail(&head, &expected_base));
    }
    if rec
        .payload
        .get("patch_truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(PREFLIGHT_PATCH_TRUNCATED.to_string());
    }
    let patch = rec
        .payload
        .get("patch")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| PREFLIGHT_NO_PATCH.to_string())?
        .to_string();

    let issue_title = resolve_issue_title(store, task_id);
    let title = arg_str(args, "title")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or(issue_title)
        .unwrap_or_else(|| format!("任务 {task_id} 交付"));
    Ok(Preflight::Proceed {
        copy_dir,
        patch,
        expected_base,
        title,
    })
}

/// 前置失败词表（preflight → 编排层 fail_outcome 的控制信号；tests 逐条
/// 钉语义防文案漂移破坏对账判断）。
const PREFLIGHT_REJECTED: &str = "交付已被拒绝（rejected 终态）——拒绝的交付不发布";
const PREFLIGHT_EMPTY_REPO: &str = "副本仓库无 HEAD（空仓）——副本物化异常，请重新物化";
const PREFLIGHT_NO_BASE_SHA: &str = "交付 payload 缺 repo_base_sha（旧形态交付，无版本绑定）——无法核验副本基线，拒绝发布；请重跑任务产出带版本绑定的交付";
const PREFLIGHT_PATCH_TRUNCATED: &str =
    "交付 patch 已截断（超 1MB 上限）——截断 patch 发布会产出残缺 PR，拒绝发布";
const PREFLIGHT_NO_PATCH: &str =
    "交付 payload 无内联 patch 文本——无可发布物（patch_path 留底在 worker 侧，master 不可达）";

fn base_drift_detail(head: &str, expected_base: &str) -> String {
    format!(
        "副本基线漂移：副本 HEAD {head} ≠ 交付基线 {expected_base}——把 patch 糊到漂移基线会产出错误交付，拒绝发布；请重物化副本或人工处理"
    )
}

/// 发布主流程。返回值恒为结构化 outcome（`status` = published /
/// already_published / failed + detail）——发布结果是第一公民结果不是传输
/// 错误；`Err` 只用于请求形态非法（缺必填参数）。
async fn publish_task_delivery(
    db: &Arc<TaskDb>,
    store: &Arc<BoardStore>,
    agent_loop: &Arc<AgentLoop>,
    workspace: &Path,
    args: &Value,
) -> Result<Value, String> {
    let task_id = arg_str(args, "task_id")?;
    let connector = arg_str(args, "connector")?;
    let owner = arg_str(args, "owner")?;
    let repo = arg_str(args, "repo")?;
    let base = arg_str(args, "base")?;
    validate_task_id_for_branch(&task_id)?;
    let branch = format!("nemesis/task-{task_id}");
    let branch_key = format!("{task_id}:branch");

    let pre = match preflight(db, store, &task_id, args) {
        Ok(Preflight::Already(v)) => return Ok(v),
        Ok(p) => p,
        Err(e) => return Ok(fail_outcome(&task_id, &branch, e)),
    };
    let Preflight::Proceed {
        copy_dir,
        patch,
        expected_base,
        title,
    } = pre
    else {
        unreachable!("preflight 只产 Already/Proceed 两形态");
    };

    // ③ git 半边：副本内建分支 + 应用 patch + commit。
    let commit_hex = match nemesis_board::git_repo::apply_patch_new_branch(
        &copy_dir,
        &branch,
        patch.as_bytes(),
        &format!("NemesisBot task {task_id}: {title}"),
    ) {
        Ok(c) => c,
        Err(e) => {
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("git 半边失败（确定失败，可重试）: {e}"),
            ));
        }
    };
    let body = arg_str(args, "body").unwrap_or_else(|_| {
        format!(
            "NemesisBot 外循环自动交付\n\n- 任务: `{task_id}`\n- 交付分支: `{branch}`\n- 交付 commit: `{commit_hex}`\n- 基于基线: `{expected_base}`"
        )
    });
    // ④ 推送（vault PAT；匿名路径 = 公开仓语义，与 clone 凭据对称）。
    let cred = publish_credentials(workspace, &connector, &owner);
    if let Err(e) = nemesis_board::git_repo::push_branch(
        &copy_dir,
        &branch,
        "origin",
        &format!("refs/heads/{branch}"),
        cred.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
    ) {
        return Ok(fail_outcome(
            &task_id,
            &branch,
            format!("分支推送失败（确定失败，可重试——重推同内容幂等）: {e}"),
        ));
    }

    // ⑤ PR 创建 + unknown 对账。
    let create_args = json!({
        "owner": owner, "repo": repo, "head": branch, "base": base,
        "title": title, "body": body,
    });
    let (pr_number, pr_url) = match invoke(agent_loop, &connector, "create_pull", create_args).await
    {
        Ok(resp) => match parse_pr_response(&resp) {
            Some(found) => found,
            // 非 PR JSON（HTTP 错误页/截断/未知形态）→ 对账兜底。
            None => {
                match reconcile_pr_by_head(agent_loop, &connector, &owner, &repo, &branch).await {
                    Some(found) => found,
                    None => {
                        return Ok(fail_outcome(
                            &task_id,
                            &branch,
                            format!(
                                "PR 创建未确认（结果非 PR JSON），对账无同 head PR——保守判失败可重试。原始响应: {}",
                                head_chars(&resp, 400)
                            ),
                        ));
                    }
                }
            }
        },
        // dispatch 层拒（estop/审批/安全层）——请求未发出，确定失败。
        Err(text) => {
            return Ok(fail_outcome(
                &task_id,
                &branch,
                format!("PR 创建被拒绝（未发出，可重试）: {text}"),
            ));
        }
    };

    // ⑥ 落账：published 终态 + PR 留痕。
    db.mark_delivery_published(&branch_key)?;
    db.merge_delivery_payload(
        &branch_key,
        &json!({
            "pr_number": pr_number,
            "pr_url": pr_url,
            "published_at": chrono::Local::now().to_rfc3339(),
            "published_branch": branch,
            "published_remote": format!("{owner}/{repo}"),
            "published_commit": commit_hex,
        }),
    )?;
    tracing::info!(
        task_id = %task_id,
        pr_number = pr_number,
        branch = %branch,
        "[Publish] 交付已发布"
    );
    Ok(json!({
        "task_id": task_id,
        "status": ST_PUBLISHED,
        "pr_number": pr_number,
        "pr_url": pr_url,
        "commit": commit_hex,
        "branch": branch,
        "remote": format!("{owner}/{repo}"),
        "detail": "已推送交付分支并创建 PR",
    }))
}

// ---------------------------------------------------------------------------
// 纯函数 + 薄封装（对账/解析/校验/凭据）
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少或空参数: {key}"))
}

/// task_id → git 分支名成分合法性（`nemesis/task-<task_id>` 的 task 段）：
/// ASCII 字母数字 + `-_.`，拒 `/`、空白、`..`、首 `.`——git ref 词表 + 路径
/// 注入双防。集群 task_id 是 uuid 形态，天然合规；防御的是手工传参。
fn validate_task_id_for_branch(task_id: &str) -> Result<(), String> {
    if task_id.is_empty() || task_id.len() > 64 {
        return Err(format!("task_id 长度非法（{}）：1..=64", task_id.len()));
    }
    if !task_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("task_id 含非法字符（只允许字母数字 - _ .）".to_string());
    }
    if task_id.contains("..") || task_id.starts_with('.') {
        return Err("task_id 不得以 . 开头或包含 ..".to_string());
    }
    Ok(())
}

/// 副本目录解析：显式 `copy_path` 优先；缺省走 task_id → dispatch → issue
/// → project → directory 链（P1 物化的任务副本）。终点必须含 `.git`
/// （clone=false 的项目目录没有远端可推，诚实报错）。
fn resolve_copy_dir(
    store: &BoardStore,
    task_id: &str,
    copy_path: Option<&Value>,
) -> Result<PathBuf, String> {
    if let Some(p) = copy_path
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let dir = PathBuf::from(p);
        if !dir.join(".git").exists() {
            return Err(format!("copy_path `{p}` 不是 git 仓库（缺 .git）"));
        }
        return Ok(dir);
    }
    let dispatch = store
        .get_dispatch(task_id)?
        .ok_or_else(|| format!("任务 {task_id} 无派发记录——缺省副本解析需要 dispatch→issue→project 链，请显式传 copy_path"))?;
    let issue = store.get_issue(dispatch.issue_id)?;
    let pid = issue
        .project_id
        .ok_or_else(|| "来源 issue 无所属项目（project_id 空）——请显式传 copy_path".to_string())?;
    let project = store.get_project(pid)?;
    let dir = project
        .directory
        .ok_or_else(|| "项目无目录（directory 空）——请显式传 copy_path".to_string())?;
    let dir = PathBuf::from(dir);
    if !dir.join(".git").exists() {
        return Err(format!(
            "项目目录 `{}` 不是 git 仓库（clone=false 副本无远端可推）——请改用 clone=true 副本或显式传 copy_path",
            dir.display()
        ));
    }
    Ok(dir)
}

/// issue 标题（PR 默认标题源；链路任一环缺席 = None，落回调用方 fallback）。
fn resolve_issue_title(store: &BoardStore, task_id: &str) -> Option<String> {
    let dispatch = store.get_dispatch(task_id).ok()??;
    let issue = store.get_issue(dispatch.issue_id).ok()?;
    Some(issue.title)
}

/// vault 凭据（`connector/<id>/pat` → (owner, pat)）：与 clone 凭据同源同
/// 别名。缺席 = None（匿名推送——公开仓合法路径；私有仓 push 会诚实失败）。
fn publish_credentials(workspace: &Path, connector: &str, owner: &str) -> Option<(String, String)> {
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(workspace);
    match crate::vault_runtime::resolve_alias_at(&vault_path, &format!("connector/{connector}/pat"))
    {
        Ok(pat) if !pat.trim().is_empty() => Some((owner.to_string(), pat)),
        _ => None,
    }
}

/// connector op 调用（connector_invoke 工具走主 loop 完整 dispatch 瀑布，
/// 非旁路——estop/hidden/安全 8 层/凭据注入/回执全生效）。
async fn invoke(
    agent_loop: &AgentLoop,
    connector: &str,
    op: &str,
    args: Value,
) -> Result<String, String> {
    let call = nemesis_agent::types::ToolCallInfo {
        id: format!("publish-{}", uuid::Uuid::new_v4()),
        name: crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
        arguments: json!({"connector": connector, "op": op, "args": args}).to_string(),
    };
    let ctx = RequestContext::new(
        "delivery-publish",
        "delivery-publish",
        "delivery-publish",
        "delivery-publish",
    );
    let text = agent_loop.handle_tool_call(&call, &ctx).await;
    if crate::external_source::is_tool_error(&text) {
        return Err(text);
    }
    Ok(text)
}

/// create_pull 成功响应 → (number, html_url)。直接对象或单元素数组都认；
/// 认不出的形态交给对账（不猜）。
fn parse_pr_response(text: &str) -> Option<(i64, Option<String>)> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let obj = v.as_object().or_else(|| {
        v.as_array()
            .and_then(|a| a.first())
            .and_then(|x| x.as_object())
    })?;
    let n = obj.get("number")?.as_i64()?;
    let url = obj
        .get("html_url")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    Some((n, url))
}

/// unknown 对账：pr_list 按 head 分支回查同仓 PR（state=all——open/closed
/// 一并认，报告既有 PR 比重复建一个诚实）。响应解析失败（截断/非 JSON）=
/// None（保守判失败，不造 PR 号）。
async fn reconcile_pr_by_head(
    agent_loop: &Arc<AgentLoop>,
    connector: &str,
    owner: &str,
    repo: &str,
    head_branch: &str,
) -> Option<(i64, Option<String>)> {
    let text = invoke(
        agent_loop,
        connector,
        "pr_list",
        json!({"owner": owner, "repo": repo, "state": "all", "limit": 20}),
    )
    .await
    .ok()?;
    find_pr_by_head(&text, head_branch)
}

/// pr_list JSON 数组中找 `head.ref == head_branch` 的首条 → (number, url)。
fn find_pr_by_head(list_text: &str, head_branch: &str) -> Option<(i64, Option<String>)> {
    let v: Value = serde_json::from_str(list_text.trim()).ok()?;
    for pr in v.as_array()? {
        let r = pr
            .get("head")
            .and_then(|h| h.get("ref"))
            .and_then(|x| x.as_str());
        if r == Some(head_branch) {
            let n = pr.get("number")?.as_i64()?;
            let url = pr
                .get("html_url")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            return Some((n, url));
        }
    }
    None
}

fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests;
