//! 子代理任务注册表单测（生命周期 / 容量 / 落盘回读 / 取消）。本地实例，
//! 不触全局单例（OnceLock 不可重置）。

use super::*;
use std::path::PathBuf;

fn begin_req(id: &str) -> SubagentBegin {
    SubagentBegin {
        id: id.to_string(),
        parent_session_key: "agent_main_session_p1".to_string(),
        parent_chat_id: "web:conn1".to_string(),
        origin: "model_spawn".to_string(),
        agent_id: "test-agent".to_string(),
        task: "做一个任务".to_string(),
        role: String::new(),
        tools_profile: "full".to_string(),
        depth: 1,
        background: false,
        loop_origin: "main".to_string(),
        bg_task_id: None,
    }
}

#[test]
fn lifecycle_begin_finish_states() {
    let reg = SubagentRegistry::new(None);
    reg.begin(begin_req("subagent:u1"));
    assert_eq!(
        reg.get("subagent:u1").unwrap().status,
        SubagentStatus::Running
    );

    reg.finish(
        "subagent:u1",
        SubagentStatus::Done,
        Some(3),
        Some("最终结论".to_string()),
        None,
    );
    let t = reg.get("subagent:u1").unwrap();
    assert_eq!(t.status, SubagentStatus::Done);
    assert_eq!(t.rounds, Some(3));
    assert_eq!(t.summary.as_deref(), Some("最终结论"));
    assert!(t.finished_at_ms.is_some());
}

#[test]
fn finish_missing_entry_is_silent_noop() {
    let reg = SubagentRegistry::new(None);
    reg.finish("subagent:ghost", SubagentStatus::Done, None, None, None);
    assert!(reg.get("subagent:ghost").is_none());
}

#[test]
fn cancel_sets_cancelled_and_consumes_token() {
    let reg = SubagentRegistry::new(None);
    reg.begin(begin_req("subagent:u2"));
    let token = Arc::new(CancellationToken::new());
    reg.attach_token("subagent:u2", token.clone());
    assert!(!token.is_cancelled());

    assert!(reg.cancel("subagent:u2"));
    assert!(token.is_cancelled());
    assert_eq!(
        reg.get("subagent:u2").unwrap().status,
        SubagentStatus::Cancelled
    );

    // 已终态条目再取消：命中但不覆盖终态（保留首次裁决）。
    assert!(reg.cancel("subagent:u2"));
    assert_eq!(
        reg.get("subagent:u2").unwrap().status,
        SubagentStatus::Cancelled
    );

    // 未登记 id：诚实未命中。
    assert!(!reg.cancel("subagent:nope"));
}

#[test]
fn list_is_newest_first_and_excerpts_are_capped() {
    let reg = SubagentRegistry::new(None);
    for i in 0..5 {
        let mut b = begin_req(&format!("subagent:u{i}"));
        b.task = "长".repeat(2000);
        reg.begin(b);
    }
    let list = reg.list();
    assert_eq!(list.len(), 5);
    assert_eq!(list[0].id, "subagent:u4");
    assert_eq!(list[4].id, "subagent:u0");
    for t in &list {
        assert_eq!(t.task.chars().count(), EXCERPT_TASK);
    }
}

#[test]
fn cap_evicts_oldest_terminal_first() {
    let reg = SubagentRegistry::new(None);
    // 499 个终态 + 1 个 Running 打满 500。
    for i in 0..499 {
        let id = format!("subagent:old{i}");
        reg.begin(begin_req(&id));
        reg.finish(&id, SubagentStatus::Done, None, None, None);
    }
    reg.begin(begin_req("subagent:running-keep"));
    assert_eq!(reg.list().len(), 500);

    // 再插 1 个：应逐出最老的终态（subagent:old0），Running 保留。
    reg.begin(begin_req("subagent:new"));
    let list = reg.list();
    assert_eq!(list.len(), 500);
    assert!(reg.get("subagent:old0").is_none());
    assert!(reg.get("subagent:old1").is_some());
    assert!(reg.get("subagent:running-keep").is_some());
    assert!(reg.get("subagent:new").is_some());
}

#[test]
fn persist_roundtrip_coerces_running_to_interrupted() {
    let dir = std::env::temp_dir().join(format!("nb-reg-test-{}", std::process::id()));
    let path = dir.join("tasks.json");
    let _ = std::fs::remove_dir_all(&dir);

    let reg = SubagentRegistry::new(Some(PathBuf::from(&path)));
    reg.begin(begin_req("subagent:p1"));
    reg.finish(
        "subagent:p1",
        SubagentStatus::Done,
        Some(1),
        Some("ok".to_string()),
        None,
    );
    reg.begin(begin_req("subagent:p2")); // 不 finish = 落盘时 Running

    // 独立实例回读（模拟进程重启）。
    let reg2 = SubagentRegistry::new(Some(PathBuf::from(&path)));
    reg2.load_from_disk();
    assert_eq!(
        reg2.get("subagent:p1").unwrap().status,
        SubagentStatus::Done
    );
    assert_eq!(
        reg2.get("subagent:p2").unwrap().status,
        SubagentStatus::Interrupted
    );
    // 回读后列表可查（新的在前：p2 在 p1 前）。
    let list = reg2.list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "subagent:p2");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn begin_overwrites_duplicate_id_without_duplicating_order() {
    let reg = SubagentRegistry::new(None);
    reg.begin(begin_req("subagent:dup"));
    reg.finish("subagent:dup", SubagentStatus::Done, None, None, None);
    reg.begin(begin_req("subagent:dup")); // 同 id 再登记（覆盖）
    assert_eq!(reg.list().len(), 1);
    assert_eq!(
        reg.get("subagent:dup").unwrap().status,
        SubagentStatus::Running
    );
}
