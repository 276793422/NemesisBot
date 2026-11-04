//! [`crate::finish`] 谓词矩阵测试：五条件各臂 + 去重行为，全部纯 store
//! 夹具（无 LLM、无网络）。

use super::*;
use crate::models::{CommentType, NewComment, NewIssue};

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_store(name: &str) -> (BoardStore, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-finishtest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    (store, dir)
}

fn make_child(store: &BoardStore, title: &str) -> Issue {
    let creator = Actor::agent("node-a");
    store
        .create_issue(NewIssue {
            title: title.to_string(),
            creator: creator.clone(),
            ..NewIssue::default()
        })
        .unwrap()
}

fn set_status(store: &BoardStore, id: i64, status: IssueStatus) {
    let actor = Actor::agent("node-a");
    // 状态机直达：backlog → in_progress → 目标（done/in_review/cancelled
    // 均从 in_progress 合法可达）。
    if status != IssueStatus::Backlog {
        store
            .transition_issue(id, IssueStatus::InProgress, &actor)
            .unwrap();
    }
    if status != IssueStatus::InProgress {
        store.transition_issue(id, status, &actor).unwrap();
    }
}

/// transition 后重取（本地 Issue 是创建时的快照，状态会过期）。
fn refresh(store: &BoardStore, issues: &[i64]) -> Vec<Issue> {
    issues
        .iter()
        .map(|id| store.get_issue(*id).unwrap())
        .collect()
}

fn add_comment(store: &BoardStore, issue_id: i64, author: Actor, content: &str) {
    store
        .add_comment(NewComment {
            issue_id,
            author,
            content: content.to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
}

fn unknown_body(items: &[&str]) -> String {
    let mut body = String::from("❓ 评审存疑事项（unknown 清单，自动收口检查会核对裁决状态）：");
    for u in items {
        body.push_str(&format!("\nunknown: {u}"));
    }
    body
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clean_subtree_passes() {
    let (store, dir) = temp_store("clean-pass");
    let a = make_child(&store, "子单甲");
    let b = make_child(&store, "子单乙");
    set_status(&store, a.id, IssueStatus::Done);
    set_status(&store, b.id, IssueStatus::Done);
    let subtree = refresh(&store, &[a.id, b.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(gaps.is_empty(), "全 done 干净子树必须放行: {gaps:?}");
    cleanup(&dir);
}

#[test]
fn mid_flight_child_blocks_cond1() {
    let (store, dir) = temp_store("cond1-midflight");
    let done = make_child(&store, "已完成");
    let flying = make_child(&store, "执行中");
    set_status(&store, done.id, IssueStatus::Done);
    set_status(&store, flying.id, IssueStatus::InProgress);
    let subtree = refresh(&store, &[done.id, flying.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        gaps.iter().any(|g| g.contains("① 子树存在未终态单据")),
        "非终态子单必须触发 ①: {gaps:?}"
    );
    cleanup(&dir);
}

#[test]
fn fail_class_marker_distinguishes_cond2() {
    let (store, dir) = temp_store("cond2-fail-class");
    let flying = make_child(&store, "失败待重派");
    set_status(&store, flying.id, IssueStatus::InProgress);
    // 无标记：① + ② 都触发。
    let subtree = refresh(&store, &[flying.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(gaps.iter().any(|g| g.contains("① ")), "{gaps:?}");
    assert!(
        gaps.iter().any(|g| g.contains("② 子树存在未收尾失败单")),
        "无 fail_class 标记的非 done 单必须触发 ②: {gaps:?}"
    );
    // 落 fail_class 标记行（gateway 写回失败评论形态）：② 满足，① 仍触发。
    add_comment(
        &store,
        flying.id,
        Actor::agent("node-b"),
        "⛔ worker 汇报失败：\n\n环境缺依赖\n\nfail_class: environmental",
    );
    let gaps = evaluate_parent_finish(&store, &[flying]);
    assert!(gaps.iter().any(|g| g.contains("① ")), "{gaps:?}");
    assert!(
        !gaps.iter().any(|g| g.contains("② 子树存在未收尾失败单")),
        "带 fail_class 标记的单不得再触发 ②: {gaps:?}"
    );
    cleanup(&dir);
}

#[test]
fn cancelled_child_triggers_cond5_only() {
    let (store, dir) = temp_store("cond5-cancelled");
    let done = make_child(&store, "已完成");
    let gone = make_child(&store, "已取消");
    set_status(&store, done.id, IssueStatus::Done);
    set_status(&store, gone.id, IssueStatus::Cancelled);
    let subtree = refresh(&store, &[done.id, gone.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        gaps.iter().any(|g| g.contains("⑤ 子树存在 cancelled 缺口")),
        "cancelled 缺口必须触发 ⑤: {gaps:?}"
    );
    assert!(
        !gaps.iter().any(|g| g.contains("① ")),
        "cancelled 是终态，不得触发 ①: {gaps:?}"
    );
    assert!(
        !gaps.iter().any(|g| g.contains("② 子树存在未收尾失败单")),
        "cancelled 视作已收尾，不得触发 ②: {gaps:?}"
    );
    cleanup(&dir);
}

#[test]
fn active_dispatch_blocks_cond3() {
    let (store, dir) = temp_store("cond3-inflight");
    let done = make_child(&store, "已完成但有新派发");
    set_status(&store, done.id, IssueStatus::Done);
    store
        .insert_dispatch("task-finish-1", done.id, "node-c", &Actor::agent("node-a"))
        .unwrap();
    let subtree = refresh(&store, &[done.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        gaps.iter().any(|g| g.contains("③ 子树存在活跃派发")),
        "queued 派发在途必须触发 ③: {gaps:?}"
    );
    assert!(gaps.iter().any(|g| g.contains("task-finish-1")), "{gaps:?}");
    // 终态化派发后放行。
    store
        .finish_dispatch("task-finish-1", crate::models::dispatch_state::DONE)
        .unwrap();
    let subtree = refresh(&store, &[done.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        !gaps.iter().any(|g| g.contains("③ ")),
        "派发终态后 ③ 必须放行: {gaps:?}"
    );
    cleanup(&dir);
}

#[test]
fn pending_unknown_blocks_cond4_and_admin_reply_adjudicates() {
    let (store, dir) = temp_store("cond4-unknown");
    let done = make_child(&store, "已完成存疑");
    set_status(&store, done.id, IssueStatus::Done);
    add_comment(
        &store,
        done.id,
        Actor::system("board-review"),
        &unknown_body(&["副作用落点不明", "耗时统计缺失"]),
    );
    let subtree = refresh(&store, &[done.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    let c4 = gaps
        .iter()
        .find(|g| g.contains("④ 存在未裁决 unknown 清单"))
        .expect("未裁决 unknown 必须触发 ④");
    assert!(c4.contains("副作用落点不明"), "{c4}");
    assert!(c4.contains("耗时统计缺失"), "{c4}");
    assert!(c4.contains(&done.number), "④ 必须标注所在单号: {c4}");

    // agent 评论不裁决。
    add_comment(&store, done.id, Actor::agent("node-b"), "我看过了，没问题");
    let subtree = refresh(&store, &[done.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        gaps.iter().any(|g| g.contains("④ ")),
        "agent 评论不得充当裁决: {gaps:?}"
    );
    // system 评论不裁决。
    add_comment(&store, done.id, Actor::system("board"), "系统备注");
    let subtree = refresh(&store, &[done.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        gaps.iter().any(|g| g.contains("④ ")),
        "system 评论不得充当裁决: {gaps:?}"
    );
    // admin 评论在标记之前不算（本轮先 admin 后标记）。
    let other = make_child(&store, "admin先行");
    set_status(&store, other.id, IssueStatus::Done);
    add_comment(&store, other.id, Actor::admin("boss"), "先记一笔");
    add_comment(
        &store,
        other.id,
        Actor::system("board-review"),
        &unknown_body(&["存疑事项乙"]),
    );
    let subtree = refresh(&store, &[other.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        gaps.iter().any(|g| g.contains("④ ")),
        "admin 评论在标记之前不得视为裁决: {gaps:?}"
    );
    // admin 评论在标记之后 = 裁决成立。
    add_comment(
        &store,
        done.id,
        Actor::admin("boss"),
        "已核实，副作用无影响，裁决通过",
    );
    let subtree = refresh(&store, &[done.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(
        !gaps.iter().any(|g| g.contains("④ ")),
        "admin 评论回应后 ④ 必须放行: {gaps:?}"
    );
    // 上面先行 admin 的单再补一条 admin 回应同样放行。
    add_comment(&store, other.id, Actor::admin("boss"), "补裁决：确认无影响");
    let subtree = refresh(&store, &[other.id]);
    let gaps = evaluate_parent_finish(&store, &subtree);
    assert!(!gaps.iter().any(|g| g.contains("④ ")), "{gaps:?}");
    cleanup(&dir);
}

#[test]
fn entry_predicate_clears_on_inflight_children() {
    let (store, dir) = temp_store("entry-inflight-clear");
    // in_progress 子单：收口面 ①② 触发，入口面（③④⑤）必须放行——
    // in-flight 子单在场时缺陷闸/分流/返工链照常跑（零回归合同）。
    let flying = make_child(&store, "执行中");
    set_status(&store, flying.id, IssueStatus::InProgress);
    let subtree = refresh(&store, &[flying.id]);
    let full = evaluate_parent_finish(&store, &subtree);
    assert!(
        full.iter().any(|g| g.contains("① 子树存在未终态单据")),
        "收口面必须对 in_progress 子单触发 ①: {full:?}"
    );
    let entry = evaluate_parent_entry_finish(&store, &subtree);
    assert!(
        entry.is_empty(),
        "入口面对 in_progress 子单必须放行: {entry:?}"
    );
    cleanup(&dir);
}

#[test]
fn entry_predicate_blocks_on_active_dispatch() {
    let (store, dir) = temp_store("entry-dispatch");
    let done = make_child(&store, "已完成但有新派发");
    set_status(&store, done.id, IssueStatus::Done);
    store
        .insert_dispatch("task-entry-1", done.id, "node-c", &Actor::agent("node-a"))
        .unwrap();
    let subtree = refresh(&store, &[done.id]);
    let entry = evaluate_parent_entry_finish(&store, &subtree);
    assert!(
        entry.iter().any(|g| g.contains("③ 子树存在活跃派发")),
        "入口面必须对在途派发触发 ③: {entry:?}"
    );
    store
        .finish_dispatch("task-entry-1", crate::models::dispatch_state::DONE)
        .unwrap();
    let subtree = refresh(&store, &[done.id]);
    let entry = evaluate_parent_entry_finish(&store, &subtree);
    assert!(entry.is_empty(), "派发终态后入口面必须放行: {entry:?}");
    cleanup(&dir);
}

#[test]
fn entry_predicate_blocks_on_pending_unknown() {
    let (store, dir) = temp_store("entry-unknown");
    let done = make_child(&store, "已完成存疑");
    set_status(&store, done.id, IssueStatus::Done);
    add_comment(
        &store,
        done.id,
        Actor::system("board-review"),
        &unknown_body(&["副作用落点不明"]),
    );
    let subtree = refresh(&store, &[done.id]);
    let entry = evaluate_parent_entry_finish(&store, &subtree);
    assert!(
        entry
            .iter()
            .any(|g| g.contains("④ 存在未裁决 unknown 清单")),
        "入口面必须对未裁决 unknown 触发 ④: {entry:?}"
    );
    add_comment(&store, done.id, Actor::admin("boss"), "裁决：无影响");
    let subtree = refresh(&store, &[done.id]);
    let entry = evaluate_parent_entry_finish(&store, &subtree);
    assert!(entry.is_empty(), "admin 裁决后入口面必须放行: {entry:?}");
    cleanup(&dir);
}

#[test]
fn entry_predicate_blocks_on_cancelled() {
    let (store, dir) = temp_store("entry-cancelled");
    let done = make_child(&store, "已完成");
    let gone = make_child(&store, "已取消");
    set_status(&store, done.id, IssueStatus::Done);
    set_status(&store, gone.id, IssueStatus::Cancelled);
    let subtree = refresh(&store, &[done.id, gone.id]);
    let entry = evaluate_parent_entry_finish(&store, &subtree);
    assert!(
        entry
            .iter()
            .any(|g| g.contains("⑤ 子树存在 cancelled 缺口")),
        "入口面必须对 cancelled 缺口触发 ⑤: {entry:?}"
    );
    assert!(
        !entry.iter().any(|g| g.contains("① ") || g.contains("④ ")),
        "cancelled 是终态且无 unknown，入口面不得夹带其他条件: {entry:?}"
    );
    cleanup(&dir);
}

#[test]
fn gap_comment_dedups_by_exact_content() {
    let (store, dir) = temp_store("gap-dedup");
    let parent = make_child(&store, "父单");
    let gaps = vec![
        "① 子树存在未终态单据：NB-2（in_progress）".to_string(),
        "④ 存在未裁决 unknown 清单：NB-2：副作用不明".to_string(),
    ];
    write_finish_gap_comment(&store, parent.id, &gaps).unwrap();
    write_finish_gap_comment(&store, parent.id, &gaps).unwrap();
    let comments = store.list_comments(parent.id).unwrap();
    let finish_comments: Vec<_> = comments
        .iter()
        .filter(|c| c.content.starts_with(FINISH_GAP_PREFIX))
        .collect();
    assert_eq!(
        finish_comments.len(),
        1,
        "同内容缺口评论必须去重: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 清单变化 → 补写显形。
    let changed = vec!["① 子树存在未终态单据：NB-2（in_progress）".to_string()];
    write_finish_gap_comment(&store, parent.id, &changed).unwrap();
    let comments = store.list_comments(parent.id).unwrap();
    let finish_comments: Vec<_> = comments
        .iter()
        .filter(|c| c.content.starts_with(FINISH_GAP_PREFIX))
        .collect();
    assert_eq!(finish_comments.len(), 2, "内容变化必须补写");
    // 空清单不写。
    write_finish_gap_comment(&store, parent.id, &[]).unwrap();
    let comments = store.list_comments(parent.id).unwrap();
    assert_eq!(
        comments
            .iter()
            .filter(|c| c.content.starts_with(FINISH_GAP_PREFIX))
            .count(),
        2,
        "空清单不得产生新评论"
    );
    cleanup(&dir);
}
