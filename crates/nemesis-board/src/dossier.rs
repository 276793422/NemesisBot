//! 任务档案（Task Dossier）导出（2026-09-29 goal）。
//!
//! 把一个任务（含子单树）或整个项目相关的**全部记录**整合成一个自包含
//! 目录，用户拿到手即可离线翻阅，不必再到看板数据库 / 项目档案 / 收件箱
//! / 用量账本四处分别找：
//!
//! ```text
//! <out>/<NB-N|项目名>_<ts>/
//! ├── README.md            总览：任务树、状态、档案清单、缺口注记
//! ├── timeline.md          人读时间线（board.db 活动/评论 + 项目档案
//! │                        timeline.jsonl 合并，按时间排序）
//! ├── audit/               原始记录（jsonl，机器可读全量）
//! │   ├── activities.jsonl  状态转移/决策/回滚等活动漏斗
//! │   ├── comments.jsonl    全部评论（含交付/决策留痕）
//! │   └── dispatches.jsonl  派发行（task_id/worker/状态/时刻）
//! ├── records/             项目档案里该任务的执行档案（worker 推回的
//! │                        逐轮 LLM 原文 + 交付），按 issue 分目录拷贝
//! ├── inbox-unplaced/      收件箱里尚未安置的执行档案（诚实可见）
//! └── usage.csv            per-task token 用量（调用方从 DataStore 聚合
//!                          后传入；本模块不碰账本）
//! ```
//!
//! 数据面**全部来自 master 本机**：board.db（单真相源）+ 项目档案目录
//! （worker 经 outbox/transfer 已推回）+ 收件箱。不发起任何 RPC——跨节
//! 点汇集是既有档案管线的职责（离线节点由 sweep 补拉，补拉落地后再次
//! 导出即含）。节点离线导致的缺口以 README 注记诚实呈现，不静默。
//!
//! 纪律对齐 archive_writer：导出失败向上返回（调用方决定留痕方式）；本
//! 模块只创建目录与拷贝文件，**绝不删除任何东西**；`read_dir` 结果显式
//! 排序（顺序是平台实现细节）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::models::{ActivityLog, Comment, DispatchRecord, Issue};
use crate::store::BoardStore;

/// per-task 用量（调用方闭包从 DataStore `aggregate_session_usage_by_task`
/// 解出；task_id 是唯一稳定锚——worker 段双身份不同源，见 board_review
/// `chain_token_usage_ds` 注释）。闭包返回 None = 该任务无账（usage.csv
/// 缺行，README 总量按有账部分计，诚实）。
#[derive(Debug, Clone)]
pub struct DossierUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 导出结果。
#[derive(Debug, Clone)]
pub struct DossierOutcome {
    /// 生成的档案根目录。
    pub root: PathBuf,
    /// 收进档案的任务编号（排序后）。
    pub issue_numbers: Vec<String>,
    /// 缺口/异常注记（诚实呈现，不静默）。
    pub notes: Vec<String>,
}

/// 单任务树导出入口：`number` = `NB-N`，树 = 该单 + 全部后代。
pub fn export_issue_tree(
    store: &BoardStore,
    number: &str,
    workspace: &Path,
    out_root: &Path,
    usage_of: &dyn Fn(&str) -> Option<DossierUsage>,
) -> Result<DossierOutcome, String> {
    let issue = store
        .get_issue_by_number(number)
        .map_err(|e| format!("读取 issue {number} 失败: {e}"))?;
    let mut issues = vec![issue];
    collect_descendants(store, issues[0].id, &mut issues);
    export_core(
        store,
        issues,
        &format_dossier_title(number),
        workspace,
        out_root,
        usage_of,
    )
}

/// 整项目导出入口：项目下全部 issue（各顶层单与其子树天然含在内）。
pub fn export_project(
    store: &BoardStore,
    project_id: i64,
    workspace: &Path,
    out_root: &Path,
    usage_of: &dyn Fn(&str) -> Option<DossierUsage>,
) -> Result<DossierOutcome, String> {
    let project = store
        .get_project(project_id)
        .map_err(|e| format!("读取项目 {project_id} 失败: {e}"))?;
    let issues = store.list_issues(&crate::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    })?;
    if issues.is_empty() {
        return Err(format!("项目 {project_id}「{}」下没有任务", project.name));
    }
    let title = crate::archive::sanitize_project_name(&project.name);
    export_core(store, issues, &title, workspace, out_root, usage_of)
}

/// 共享导出核心。`title` 已sanitize（目录名片段）。
fn export_core(
    store: &BoardStore,
    mut issues: Vec<Issue>,
    title: &str,
    workspace: &Path,
    out_root: &Path,
    usage_of: &dyn Fn(&str) -> Option<DossierUsage>,
) -> Result<DossierOutcome, String> {
    // 确定性顺序：编号字典序（NB-N 编号即序）。
    issues.sort_by(|a, b| a.number.cmp(&b.number).then(a.id.cmp(&b.id)));
    let by_id: BTreeMap<i64, &Issue> = issues.iter().map(|i| (i.id, i)).collect();

    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let root = out_root.join(format!("{title}_{ts}"));
    std::fs::create_dir_all(&root).map_err(|e| format!("创建档案目录失败: {e}"))?;
    let audit_dir = root.join("audit");
    std::fs::create_dir_all(&audit_dir).map_err(|e| format!("创建 audit 目录失败: {e}"))?;

    let mut notes: Vec<String> = Vec::new();

    // ---- 项目档案根解析（records/ 拷贝源）----
    let archive_root: Option<PathBuf> = issues
        .iter()
        .find_map(|i| i.project_id)
        .and_then(|pid| store.get_project(pid).ok())
        .and_then(|p| p.directory)
        .map(PathBuf::from)
        .filter(|p| p.is_dir());
    if issues.iter().any(|i| i.project_id.is_some()) && archive_root.is_none() {
        notes
            .push("项目未绑定档案目录（或目录缺失）：records/ 执行档案与项目 timeline 缺席".into());
    }

    // ---- per-issue 原始记录 + records 拷贝 + 收件箱补收 ----
    let inbox_root = workspace.join("cluster").join("inbox");
    let mut all_dispatches: Vec<(String, DispatchRecord)> = Vec::new(); // (number, dispatch)
    let mut activities: Vec<(String, ActivityLog)> = Vec::new();
    let mut comments: Vec<(String, Comment)> = Vec::new();

    for issue in &issues {
        if let Ok(list) = store.list_activity(issue.id) {
            for a in list {
                activities.push((issue.number.clone(), a));
            }
        }
        if let Ok(list) = store.list_comments(issue.id) {
            for c in list {
                comments.push((issue.number.clone(), c));
            }
        }
        if let Ok(list) = store.list_dispatches(issue.id) {
            for d in list {
                all_dispatches.push((issue.number.clone(), d));
            }
        }

        // records/<number>/ 子树拷贝（存在才拷；布局由 ingest_landed 定）。
        if let Some(arch) = &archive_root {
            let src = arch.join("records").join(&issue.number);
            if src.is_dir() {
                copy_dir_sorted(&src, &root.join("records").join(&issue.number))?;
            }
        }
    }

    // 收件箱补收：派发行 task_id 在收件箱里 = 执行档案未安置（无处安置/
    // 存量项目/落地竞态）——拷进档案诚实可见。
    for (number, d) in &all_dispatches {
        let src = inbox_root.join(&d.task_id);
        if src.is_dir() {
            copy_dir_sorted(&src, &root.join("inbox-unplaced").join(&d.task_id))?;
        }
        let _ = number; // 目录按 task_id 组织（与 cluster_logs 目录名同源）
    }

    // ---- 执行档案落位核对（notes）----
    let records_base = root.join("records");
    let unplaced_base = root.join("inbox-unplaced");
    for (number, d) in &all_dispatches {
        let landed = records_base.join(number).join("execution").is_dir()
            || walk_has_dir(&records_base.join(number), "execution")
            || unplaced_base.join(&d.task_id).is_dir();
        if !landed {
            notes.push(format!(
                "执行档案未落地：{} 的任务 {} → {}（状态 {}）——节点可能离线未推送，等补拉后重新导出",
                number, d.task_id, d.worker_id, d.state
            ));
        }
    }

    // ---- audit jsonl（全量原始）----
    write_jsonl(
        &audit_dir.join("activities.jsonl"),
        &activities,
        |(n, a)| {
            serde_json::json!({
                "issue": n,
                "id": a.id,
                "ts": a.created_at,
                "actor": format!("{}/{}", a.actor.kind, a.actor.id),
                "action": a.action.clone(),
                "details": a.details.clone(),
            })
        },
    )?;
    write_jsonl(&audit_dir.join("comments.jsonl"), &comments, |(n, c)| {
        serde_json::json!({
            "issue": n,
            "id": c.id,
            "ts": c.created_at,
            "author": format!("{}/{}", c.author.kind, c.author.id),
            "type": c.ctype.as_str(),
            "content": c.content.clone(),
        })
    })?;
    write_jsonl(
        &audit_dir.join("dispatches.jsonl"),
        &all_dispatches,
        |(n, d)| {
            serde_json::json!({
                "issue": n,
                "task_id": d.task_id.clone(),
                "worker": d.worker_id.clone(),
                "state": d.state.clone(),
                "dispatched_at": d.dispatched_at,
                "completed_at": d.completed_at,
            })
        },
    )?;

    // ---- usage.csv（闭包解账；None = 无账，缺行诚实）----
    let mut usage_rows: Vec<(String, String, i64, i64)> = Vec::new();
    for (_, d) in &all_dispatches {
        if let Some(u) = usage_of(&d.task_id) {
            usage_rows.push((
                d.task_id.clone(),
                d.worker_id.clone(),
                u.input_tokens,
                u.output_tokens,
            ));
        }
    }
    usage_rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut csv = String::from("task_id,worker,input_tokens,output_tokens,total\n");
    for (task_id, worker, input, output) in &usage_rows {
        csv.push_str(&format!(
            "{},{},{},{},{}\n",
            task_id,
            worker,
            input,
            output,
            input + output
        ));
    }
    std::fs::write(root.join("usage.csv"), csv).map_err(|e| format!("写 usage.csv 失败: {e}"))?;

    // ---- timeline.md ----
    let timeline_md = render_timeline(store, &by_id, &issues, archive_root.as_deref())?;
    std::fs::write(root.join("timeline.md"), timeline_md)
        .map_err(|e| format!("写 timeline.md 失败: {e}"))?;

    // ---- README.md ----
    let readme = render_readme(store, &issues, title, &notes, &usage_rows);
    std::fs::write(root.join("README.md"), readme)
        .map_err(|e| format!("写 README.md 失败: {e}"))?;

    Ok(DossierOutcome {
        root,
        issue_numbers: issues.iter().map(|i| i.number.clone()).collect(),
        notes,
    })
}

/// 深度优先收集全部后代（防环：visited 集合）。
fn collect_descendants(store: &BoardStore, id: i64, out: &mut Vec<Issue>) {
    let Ok(children) = store.list_children(id) else {
        return;
    };
    for c in children {
        if out.iter().any(|i| i.id == c.id) {
            continue;
        }
        let cid = c.id;
        out.push(c);
        collect_descendants(store, cid, out);
    }
}

/// 递归拷贝目录（read_dir 结果显式排序；只拷常规文件与子目录）。
fn copy_dir_sorted(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录 {} 失败: {e}", dst.display()))?;
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(src)
        .map_err(|e| format!("读目录 {} 失败: {e}", src.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("遍历目录 {} 失败: {e}", src.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let target = dst.join(e.file_name());
        if p.is_dir() {
            copy_dir_sorted(&p, &target)?;
        } else if p.is_file() {
            std::fs::copy(&p, &target)
                .map_err(|err| format!("拷贝 {} 失败: {err}", p.display()))?;
        }
    }
    Ok(())
}

/// 目录树下任意深度存在名为 `name` 的子目录（执行档案落位核对用；
/// ingest 布局是 records/<issue>/execution/<ts>/，深度探测不依赖层数）。
fn walk_has_dir(root: &Path, name: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else {
        return false;
    };
    let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if e.file_name() == name {
            return true;
        }
        if walk_has_dir(&p, name) {
            return true;
        }
    }
    false
}

/// jsonl 落盘（按 (issue, ts, id) 排序 = 跨 issue 稳定时间线）。
fn write_jsonl<T>(
    path: &Path,
    rows: &[(String, T)],
    render: impl Fn(&(String, T)) -> serde_json::Value,
) -> Result<(), String> {
    let mut sorted: Vec<&(String, T)> = rows.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut body = String::new();
    for row in sorted {
        body.push_str(&render(row).to_string());
        body.push('\n');
    }
    std::fs::write(path, body).map_err(|e| format!("写 {} 失败: {e}", path.display()))
}

/// unix 秒 → 本地时间字符串（board.db 全部时刻 = Utc timestamp 秒）。
fn fmt_ts(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| secs.to_string())
}

/// 人读时间线：每单一段 + 合并事件流（board.db 活动/评论 + 项目档案
/// timeline.jsonl 中该单条目），按时间排序。
fn render_timeline(
    store: &BoardStore,
    by_id: &BTreeMap<i64, &Issue>,
    issues: &[Issue],
    archive_root: Option<&Path>,
) -> Result<String, String> {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# 任务时间线");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "> 时刻全部为 master 本机时间；原始 jsonl 在 `audit/`。"
    );

    // 项目档案 timeline.jsonl 按 issue 聚合（存在才读；坏行跳过——投影
    // 文件允许手改，不因一行坏 JSON 拒绝整个档案）。
    let mut archive_events: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    if let Some(arch) = archive_root
        && let Ok(body) = std::fs::read_to_string(arch.join("timeline.jsonl"))
    {
        for line in body.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(issue) = v.get("issue").and_then(|x| x.as_str()) else {
                continue;
            };
            archive_events.entry(issue.to_string()).or_default().push((
                v.get("ts")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                v.get("kind")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?")
                    .to_string(),
                v.get("summary")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            ));
        }
    }

    for issue in issues {
        let _ = writeln!(
            out,
            "\n## {} {}  `[{}]`",
            issue.number, issue.title, issue.status
        );

        #[derive(Clone)]
        struct Ev {
            sort_key: String,
            line: String,
        }
        let mut events: Vec<Ev> = Vec::new();

        // 项目档案 timeline 事件（ts 是 rfc3339 字符串，字典序即时间序）。
        for (ts, kind, summary) in archive_events.get(&issue.number).into_iter().flatten() {
            events.push(Ev {
                sort_key: format!("0{ts}"),
                line: format!("- `{ts}` **{kind}** {summary}"),
            });
        }
        // board.db 活动。
        if let Ok(list) = store.list_activity(issue.id) {
            for a in list {
                let who = format!("{}/{}", a.actor.kind, a.actor.id);
                let detail = a.details.unwrap_or_default();
                events.push(Ev {
                    sort_key: format!("1{}", a.created_at),
                    line: format!(
                        "- `{}` **{}**（{who}）{}",
                        fmt_ts(a.created_at),
                        a.action,
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!(" — {detail}")
                        }
                    ),
                });
            }
        }
        // 评论（类型标注；全文在 audit/comments.jsonl）。
        if let Ok(list) = store.list_comments(issue.id) {
            for c in list {
                let who = format!("{}/{}", c.author.kind, c.author.id);
                let first_line = c.content.lines().next().unwrap_or("").to_string();
                events.push(Ev {
                    sort_key: format!("2{}", c.created_at),
                    line: format!(
                        "- `{}` 💬[{}]（{who}）{}",
                        fmt_ts(c.created_at),
                        c.ctype.as_str(),
                        first_line
                    ),
                });
            }
        }
        // 派发行。
        if let Ok(list) = store.list_dispatches(issue.id) {
            for d in list {
                events.push(Ev {
                    sort_key: format!("1{}.{}", d.dispatched_at, d.task_id),
                    line: format!(
                        "- `{}` 📤 派发 → {}（task {}，状态 {}）",
                        fmt_ts(d.dispatched_at),
                        d.worker_id,
                        d.task_id,
                        d.state
                    ),
                });
            }
        }

        events.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        if events.is_empty() {
            let _ = writeln!(out, "\n（无记录）");
        }
        for ev in events {
            let _ = writeln!(out, "{}", ev.line);
        }
    }
    let _ = by_id;
    Ok(out)
}

/// README 总览。
fn render_readme(
    store: &BoardStore,
    issues: &[Issue],
    title: &str,
    notes: &[String],
    usage_rows: &[(String, String, i64, i64)],
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# 任务档案：{title}",);
    let _ = writeln!(
        out,
        "\n生成于 {}（master 本机时间）。自包含导出：离线可读，不依赖系统在线。",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );

    // 项目信息。
    if let Some(pid) = issues.iter().find_map(|i| i.project_id)
        && let Ok(p) = store.get_project(pid)
    {
        let _ = writeln!(
            out,
            "\n## 项目\n\n- {}（状态 {}，id {}）",
            p.name, p.status, p.id
        );
    }

    let _ = writeln!(out, "\n## 任务树\n");
    let _ = writeln!(out, "| 编号 | 状态 | 优先级 | 指派 | 标题 |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for i in issues {
        let assignee = match (&i.assignee, &i.assignee_id) {
            (Some(a), Some(id)) => format!("{a}/{id}"),
            _ => "-".into(),
        };
        let _ = writeln!(
            out,
            "| {} | {} | P{} | {} | {} |",
            i.number,
            i.status,
            i.priority,
            assignee,
            i.title.replace('|', "\\|")
        );
    }

    let total_in: i64 = usage_rows.iter().map(|(_, _, i, _)| *i).sum();
    let total_out: i64 = usage_rows.iter().map(|(_, _, _, o)| *o).sum();
    let _ = writeln!(
        out,
        "\n## 用量\n\n- 记录到账的任务 {} 个：input {total_in} + output {total_out} = {} tokens（明细 usage.csv）",
        usage_rows.len(),
        total_in + total_out
    );

    if !notes.is_empty() {
        let _ = writeln!(out, "\n## 缺口注记\n");
        for n in notes {
            let _ = writeln!(out, "- ⚠ {n}");
        }
    }

    let _ = writeln!(out, "\n## 目录说明\n");
    let _ = writeln!(
        out,
        "- `timeline.md` — 人读时间线（活动/评论/派发/项目档案事件合并）"
    );
    let _ = writeln!(
        out,
        "- `audit/*.jsonl` — 原始记录全量（activities / comments / dispatches）"
    );
    let _ = writeln!(
        out,
        "- `records/` — 项目档案中本任务的执行档案（worker 逐轮 LLM 原文 + 交付）"
    );
    let _ = writeln!(
        out,
        "- `inbox-unplaced/` — 收件箱里尚未安置的执行档案（诚实可见）"
    );
    let _ = writeln!(out, "- `usage.csv` — per-task token 用量");
    out
}

/// 档案目录名片段：编号原样（NB-N 形态安全），项目名已经 sanitize。
fn format_dossier_title(number: &str) -> String {
    number
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
