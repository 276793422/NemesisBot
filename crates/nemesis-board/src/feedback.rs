//! 终判 👍/👎 反馈（计划 §8 P4）：人工在已收口父单评论区发 👍/👎 →
//! 按 Done 子树展开、per-issue 最后一次完成派发归因入 worker
//! fingerprint 画像（forge 消费）。三道闸（人类 / 图元 / 已收口）+
//! 幂等记账键 + 系统评论回执（ack 走 store 直写 System ctype，不经
//! comment.add handler——天然无递归）。任何一闸不满足 = `Ok(None)`
//! 静默跳过：普通评论 / agent 评论 / 非收口单零打扰。

use crate::assignment::Actor;
use crate::models::{CommentType, IssueStatus, NewComment, dispatch_state};
use crate::store::{BoardStore, DescendantEdges};

/// 终判结果（`Some` = 命中并已处理；`None` = 非终判评论）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerdictOutcome {
    /// true = 👍 认可 / false = 👎 否决。
    pub verdict: bool,
    /// 已入画像的记账条数（`record_fingerprint_outcome` true）。
    pub recorded: usize,
    /// 无完成派发记录 / 幂等跳过 / 记账失败的条数。
    pub skipped: usize,
}

/// 评论终判闸。`author_kind` = 评论作者 `Actor.kind`（人类 = admin/user）。
/// 返回 `None` = 不是终判评论（调用方无需任何后续处理）。
pub fn process_final_verdict_comment(
    store: &BoardStore,
    issue_id: i64,
    comment_id: i64,
    author_kind: &str,
    content: &str,
) -> Result<Option<VerdictOutcome>, String> {
    // 闸 ①：人类评论才算终判（agent / system 评论不是主体意志）。
    if !matches!(author_kind, "admin" | "user") {
        return Ok(None);
    }
    // 闸 ②：内容恰好含 👍 或 👎 之一（0/0 = 普通评论；1/1 = 歧义不猜）。
    let up = content.matches('👍').count();
    let down = content.matches('👎').count();
    if up == down {
        return Ok(None);
    }
    let verdict = up > down;

    // 闸 ③：只有已收口（Done）的单接受终判——进行中的单还没交卷，
    // 此刻的 👍/👎 不是对成品的判断。
    let issue = store.get_issue(issue_id)?;
    if issue.status != IssueStatus::Done {
        return Ok(None);
    }

    // Done 子树展开（含本单）：终判语义 = 对整个交付树的评价。
    let mut tree = vec![issue.clone()];
    tree.extend(store.descendants(&[issue_id], DescendantEdges::Subtree));
    tree.retain(|i| i.status == IssueStatus::Done);

    let mut recorded = 0usize;
    let mut skipped = 0usize;
    for issue in &tree {
        // 最后一次完成派发归因（rowid 序 → `rfind` 即最新）：state=done
        // 且 completed_at 在场且 worker 非空才算完成派发。
        let Some(d) = store.list_dispatches(issue.id)?.into_iter().rfind(|d| {
            d.state == dispatch_state::DONE
                && d.completed_at.is_some()
                && !d.worker_id.trim().is_empty()
        }) else {
            skipped += 1;
            continue;
        };
        // 幂等记账键：per (comment, issue) 唯一——同一条评论重复处理
        // 不重复入画像（fingerprint_counted INSERT OR IGNORE 兜底）。
        let task_id = format!("human_fb:{comment_id}:{}", issue.number);
        let task_type = crate::fingerprint::task_type_of(&issue.title, &issue.required_tags);
        match store.record_fingerprint_outcome(&d.worker_id, &task_type, &task_id, verdict) {
            Ok(true) => recorded += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                tracing::warn!(
                    "[FinalVerdict] issue {} 记账失败（worker {}）：{e}",
                    issue.id,
                    d.worker_id
                );
                skipped += 1;
            }
        }
    }

    // 回执：系统评论挂在终判评论下（System ctype 不走 comment.add
    // handler——本闸不会再被 ack 触发）。
    let verdict_mark = if verdict {
        "👍 认可"
    } else {
        "👎 否决"
    };
    let ack = if recorded == 0 {
        format!(
            "🖍 终判收到（{verdict_mark}），但本单及子单均无可记账的完成派发记录——不入 worker 画像。"
        )
    } else if skipped == 0 {
        format!("🖍 终判已记录：{verdict_mark} × {recorded} 条已入 worker 画像。")
    } else {
        format!(
            "🖍 终判已记录：{verdict_mark} × {recorded} 条入画像，{skipped} 条无完成派发/重复记账跳过。"
        )
    };
    store.add_comment(NewComment {
        issue_id,
        author: Actor::system("final-verdict"),
        content: ack,
        parent_id: Some(comment_id),
        ctype: CommentType::System,
    })?;

    Ok(Some(VerdictOutcome {
        verdict,
        recorded,
        skipped,
    }))
}

#[cfg(test)]
mod tests;
