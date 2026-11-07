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

// ===========================================================================
// 回归锁：persist 曾手搓固定 tmp 名（tasks.json.tmp）+
// 裸 fs::write——并发 begin/finish/cancel 是功能常态路径（多子代理同时
// 收尾），两个 persist 竞争同一 tmp 路径时交错写可产出坏混合体，rename
// 把好账本顶成坏账本（重启 load 失败 = 整册清空）。修复后走
// write_file_atomic（pid+序数+nanos 唯一 tmp + create_new：并发写者结构
// 上不可能互相踩踏，rename last-wins 必为某一次完整快照）。
// 本测试钉住不变量：
// 1. 并发风暴结束后盘上文件**必然**是合法 JSON 快照（旧实现下此臂偶发
//    红——竞态窗口本质概率性，回归锁守的是结构前提而非确定性复现）。
//    注意只断言「合法」不断言「最新」：persist 是锁内快照 + 锁外写，
//    风暴中早期快照的 rename 可能晚于一切后到（last-wins 保的是原子性
//    不是新鲜度——设计契约如此，条目丢失由下一次 persist 全量重写自愈）。
// 2. 风暴静默后补一次 persist（无并发写者 = 最后一次 rename 确定），此时
//    断言内容完整——全量序列化路径的完整性检查，确定性不再 flake。
// 3. 无残留 tmp 垃圾（write_file_atomic 失败自清理 + rename 带走）。
// ===========================================================================
#[test]
fn persist_concurrent_writers_always_leave_valid_snapshot() {
    let dir = std::env::temp_dir().join(format!(
        "nb-reg-conc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tasks.json");

    let reg = std::sync::Arc::new(SubagentRegistry::new(Some(path.clone())));
    let mut handles = Vec::new();
    // 8 线程 × 30 次 begin/finish 交替：并发写风暴（旧固定 tmp 名下
    //交错窗口随负载真实存在；write_file_atomic 唯一 tmp 名下结构免疫）。
    for t in 0..8 {
        let reg = reg.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..30 {
                let id = format!("subagent:t{t}-i{i}");
                let mut req = begin_req(&id);
                req.task = format!("并发风暴任务 t{t}-i{i}-{}", "x".repeat(200));
                reg.begin(req);
                reg.finish(
                    &id,
                    SubagentStatus::Done,
                    Some(1),
                    Some("done".to_string()),
                    None,
                );
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 不变量 1：盘上文件必须是合法 JSON（旧实现交错写坏账本时此臂红）。
    // 只验合法性不验条目数——last-wins 保证原子性不保证新鲜度（见 mod 注）。
    let bytes = std::fs::read(&path).unwrap();
    serde_json::from_slice::<Vec<SubagentTask>>(&bytes)
        .unwrap_or_else(|e| panic!("persist 产物必须是完整合法快照: {e}"));

    // 不变量 2：风暴静默后补一次 begin（触发 persist；此时无并发写者，
    // 最后一次 rename 确定 = 全量快照）——内容完整，8×30 个 id 全部在册。
    // write_file_atomic 契约明文「目标被外部占用（AV 扫描）时 rename 失败
    // 属预期，由调用方决定重试」（file.rs 头注）——重试归属测试侧：begin
    // 是同 id 幂等 upsert（覆盖 + 重 persist），有界重试不影响断言语义。
    let mut tasks: Vec<SubagentTask> = Vec::new();
    for attempt in 0..5 {
        reg.begin(begin_req("subagent:quiesce"));
        let bytes = std::fs::read(&path).unwrap();
        tasks = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("静默后快照非法（第 {} 次尝试）: {e}", attempt + 1));
        if tasks.len() == 8 * 30 + 1 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert_eq!(
        tasks.len(),
        8 * 30 + 1,
        "静默后全量快照必须含全部条目（重试 5 次后仍缺 = 真回归）"
    );
    // 不变量 3：无残留 tmp 垃圾（write_file_atomic 失败自清理 + rename 带走）。
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "不得残留 tmp 文件: {leftovers:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
