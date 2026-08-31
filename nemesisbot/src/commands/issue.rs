//! Issue command — managed-agent 看板 CLI（W2 P1）。
//!
//! 直接操作与 gateway 共享的 SQLite store（`{workspace}/board/board.db`，
//! WAL + busy_timeout 支持多进程并发）。issue 参数接受人读编号（`NB-1`）。

use crate::common;
use anyhow::Result;
use nemesis_board::assignment::{Actor, AssignmentType};
use nemesis_board::models::{IssueFilter, IssueStatus, NewComment, NewIssue};
use nemesis_board::BoardStore;

#[derive(clap::Subcommand)]
pub enum IssueAction {
    /// Create a new issue
    Create {
        /// Issue title (required)
        title: String,
        /// Description
        #[arg(long, default_value = "")]
        description: String,
        /// Priority: 0=low 1=medium 2=high 3=urgent
        #[arg(long, default_value_t = 1)]
        priority: i32,
        /// Assignee as `type:id` (e.g. `worker:node-b`) or bare `manager_self`
        #[arg(long)]
        assignee: Option<String>,
        /// Project id
        #[arg(long)]
        project_id: Option<i64>,
        /// Acceptance criteria
        #[arg(long)]
        accept: Option<String>,
    },
    /// List issues (optionally filtered)
    List {
        /// Filter by status (backlog/todo/in_progress/in_review/done/blocked/cancelled)
        #[arg(long)]
        status: Option<String>,
        /// Filter by assignee as `type:id`
        #[arg(long)]
        assignee: Option<String>,
        /// Substring match on number/title
        #[arg(long)]
        query: Option<String>,
        /// Filter by project id
        #[arg(long)]
        project_id: Option<i64>,
    },
    /// Show one issue (number like NB-1, or numeric id) with comments/activity
    Get { issue: String },
    /// Assign (or with `--clear`, unassign) an issue
    Assign {
        issue: String,
        /// Assignee as `type:id` (e.g. `worker:node-b`) or bare `manager_self`
        #[arg(long)]
        assignee: Option<String>,
        /// Clear the assignee
        #[arg(long)]
        clear: bool,
    },
    /// Transition issue status (state machine enforced)
    Status { issue: String, status: String },
    /// Add a comment to an issue
    Comment { issue: String, content: String },
    /// List projects
    Projects,
    /// Issue counts by status
    Stats,
}

/// Parse `type:id` / bare `manager_self` into (AssignmentType, id).
fn parse_assignee(s: &str) -> Result<(AssignmentType, String)> {
    if let Some((t, id)) = s.split_once(':') {
        let at = AssignmentType::from_str(t)
            .ok_or_else(|| anyhow::anyhow!("未知 assignee 类型: {t}（可选 manager_self/worker）"))?;
        if id.trim().is_empty() {
            anyhow::bail!("assignee id 不能为空（格式 type:id）");
        }
        Ok((at, id.to_string()))
    } else if s == "manager_self" {
        Ok((AssignmentType::ManagerSelf, "local".to_string()))
    } else {
        anyhow::bail!("assignee 格式应为 type:id（如 worker:node-b）或裸 manager_self")
    }
}

/// BoardStore 返回 `Result<_, String>` → anyhow（`?` 无法隐式转换）。
fn err(e: String) -> anyhow::Error {
    anyhow::Error::msg(e)
}

fn open_store(local: bool) -> Result<BoardStore> {
    let home = common::resolve_home(local);
    let db = common::workspace_path(&home).join("board").join("board.db");
    BoardStore::open(&db, "NB").map_err(|e| anyhow::anyhow!("打开看板库失败: {e}"))
}

/// Resolve `NB-1` / `1` → issue id.
fn resolve_issue_id(store: &BoardStore, spec: &str) -> Result<i64> {
    let trimmed = spec.trim();
    if let Ok(id) = trimmed.parse::<i64>() {
        return Ok(id);
    }
    let issue = store
        .get_issue_by_number(trimmed)
        .map_err(|_| anyhow::anyhow!("找不到 issue: {trimmed}（支持编号 NB-1 或数字 id）"))?;
    Ok(issue.id)
}

fn print_issue(issue: &nemesis_board::Issue) {
    let assignee = match (&issue.assignee, &issue.assignee_id) {
        (Some(a), Some(id)) => format!("{a}/{id}"),
        _ => "-".to_string(),
    };
    println!(
        "#{} [{}] P{} {} — {}（创建者 {}/{}）",
        issue.number, issue.status, issue.priority, issue.title, assignee,
        issue.creator.kind, issue.creator.id
    );
}

pub fn run(action: IssueAction, local: bool) -> Result<()> {
    let store = open_store(local)?;
    let actor = Actor::admin("cli");
    match action {
        IssueAction::Create {
            title,
            description,
            priority,
            assignee,
            project_id,
            accept,
        } => {
            let mut ni = NewIssue {
                title,
                description,
                priority,
                project_id,
                acceptance_criteria: accept,
                creator: actor,
                ..NewIssue::default()
            };
            if let Some(a) = assignee {
                let (at, aid) = parse_assignee(&a)?;
                ni.assignee = Some(at);
                ni.assignee_id = Some(aid);
            }
            let issue = store.create_issue(ni).map_err(err)?;
            println!("已创建 {}", issue.number);
            print_issue(&issue);
        }
        IssueAction::List {
            status,
            assignee,
            query,
            project_id,
        } => {
            let filter = IssueFilter {
                status: status
                    .as_deref()
                    .map(|s| {
                        IssueStatus::from_str(s)
                            .ok_or_else(|| anyhow::anyhow!("未知 status: {s}"))
                    })
                    .transpose()?,
                assignee: assignee
                    .as_deref()
                    .map(parse_assignee)
                    .transpose()?
                    .map(|(a, id)| (a, id)),
                project_id,
                priority: None,
                query,
            };
            let issues = store.list_issues(&filter).map_err(err)?;
            if issues.is_empty() {
                println!("（无匹配 issue）");
                return Ok(());
            }
            for issue in &issues {
                print_issue(issue);
            }
            println!("共 {} 条", issues.len());
        }
        IssueAction::Get { issue } => {
            let id = resolve_issue_id(&store, &issue)?;
            let issue = store.get_issue(id).map_err(err)?;
            print_issue(&issue);
            if !issue.description.is_empty() {
                println!("描述: {}", issue.description);
            }
            for c in store.list_comments(id).map_err(err)? {
                println!(
                    "  💬 [{}/{}] {}",
                    c.author.kind, c.author.id, c.content
                );
            }
            for a in store.list_activity(id).map_err(err)? {
                println!(
                    "  🕘 {} {} {} {}",
                    a.created_at, a.actor.kind, a.action,
                    a.details.as_deref().unwrap_or("")
                );
            }
        }
        IssueAction::Assign {
            issue,
            assignee,
            clear,
        } => {
            let id = resolve_issue_id(&store, &issue)?;
            let (at, aid) = if clear {
                (None, None)
            } else {
                let a = assignee
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("需要 --assignee type:id 或 --clear"))?;
                let (t, i) = parse_assignee(a)?;
                (Some(t), Some(i))
            };
            let issue = store.assign_issue(id, at, aid, &actor).map_err(err)?;
            println!("已更新指派");
            print_issue(&issue);
        }
        IssueAction::Status { issue, status } => {
            let id = resolve_issue_id(&store, &issue)?;
            let to = IssueStatus::from_str(&status)
                .ok_or_else(|| anyhow::anyhow!("未知 status: {status}"))?;
            let issue = store.transition_issue(id, to, &actor).map_err(err)?;
            println!("状态已转移");
            print_issue(&issue);
        }
        IssueAction::Comment { issue, content } => {
            let id = resolve_issue_id(&store, &issue)?;
            store
                .add_comment(NewComment {
                issue_id: id,
                author: actor,
                content,
                parent_id: None,
                ctype: nemesis_board::models::CommentType::Comment,
            })
                .map_err(err)?;
            println!("评论已添加");
        }
        IssueAction::Projects => {
            for p in store.list_projects().map_err(err)? {
                println!("#{} [{}] {}（{}）", p.id, p.status, p.name, p.description);
            }
        }
        IssueAction::Stats => {
            for (st, n) in store.count_by_status().map_err(err)? {
                println!("{st}: {n}");
            }
        }
    }
    Ok(())
}
