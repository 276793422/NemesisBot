//! B（2026-09-23 多会话并行清账）绑定注册表测试。
//!
//! 隔离：注册表文件走 `workspace/data/`（临时目录），meta/jsonl 存活判定
//! 走 `resolve_session_logs_dir_in_workspace(workspace)`（同临时目录）；
//! `get_or_create_session` 的 meta 写入经 `chat_log::write_session_meta`
//! 触 `default_path_manager()` 进程单例——重定向 home 到同一临时目录
//! （websocket_handler/tests.rs:63 先例），测试完恢复，不碰真实 home。

use super::{get_or_create_session, live_bindings, remove_binding, remove_session, set_binding};
use std::path::Path;

/// home 还原 + 锁释放**同 guard 内定序**：Drop 先还 home、后放锁（结构体
/// Drop 体先于字段释放，字段按声明序释放）。锁 = 全 crate 共享的
/// `test_home::HOME_RACE_LOCK`（模块内私有锁挡不住跨模块的 chat_log
/// 单例读写，2026-09-23 share/s10b export 偶发 count 0 的教训）。
struct HomeGuard {
    old: std::path::PathBuf,
    _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.old.clone());
    }
}

/// 重定向单例 home + 造 workspace 的 logs/session_logs 目录。workspace 取
/// `<tmp>/workspace`——与单例 `workspace()`（home/workspace）同一派生规则，
/// meta 写入（走单例）与存活判定（走入参）落同一目录。
fn isolate(home: &Path) -> HomeGuard {
    let lock = crate::test_home::lock_home();
    let old = nemesis_path::default_path_manager().home_dir();
    nemesis_path::default_path_manager().set_home_dir(home.to_path_buf());
    let logs = nemesis_path::resolve_session_logs_dir_in_workspace(&home.join("workspace"));
    let _ = std::fs::create_dir_all(&logs);
    HomeGuard { old, _lock: lock }
}

/// 造一个"存活"会话：写 meta 侧车（与 get_or_create 的产物同形）。落点 =
/// 单例 home（已重定向到 workspace 同目录），`workspace` 参数仅表意。
#[allow(unused_variables)]
fn make_session(workspace: &str, sid: &str) {
    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    );
    nemesis_agent::chat_log::write_session_meta(&key, "测试会话");
}

#[test]
fn get_or_create_is_idempotent_same_key_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    let a = get_or_create_session(&ws, "wf_agentgen:新建工作流", "对话生成：新建工作流", false)
        .unwrap();
    assert!(a.created);
    // 重复调用同 key → 同会话（零新建——空会话复制机器的回归钉）。
    let b = get_or_create_session(&ws, "wf_agentgen:新建工作流", "对话生成：新建工作流", false)
        .unwrap();
    assert!(!b.created);
    assert_eq!(a.session_id, b.session_id);

    // 不同 key → 不同会话。
    let c =
        get_or_create_session(&ws, "wf_agentgen:别的目标", "对话生成：别的目标", false).unwrap();
    assert!(c.created);
    assert_ne!(a.session_id, c.session_id);
}

#[test]
fn stale_binding_is_replaced_not_resurrected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    let first = get_or_create_session(&ws, "k", "t", false).unwrap();
    // meta 侧车被删（等价：会话被外部清走 / home 重建）→ 绑定陈旧。
    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&first.session_id)
    );
    let meta = nemesis_path::resolve_session_logs_dir_in_workspace(Path::new(&ws)).join(format!(
        "{}.meta.json",
        nemesis_utils::sanitize::sanitize_path_segment(&key)
    ));
    std::fs::remove_file(&meta).unwrap();

    let second = get_or_create_session(&ws, "k", "t", false).unwrap();
    assert!(second.created, "陈旧绑定必须重建，不得复活死会话");
    assert_ne!(first.session_id, second.session_id);
    // 再调一次稳定在新建会话上。
    let third = get_or_create_session(&ws, "k", "t", false).unwrap();
    assert_eq!(second.session_id, third.session_id);
}

#[test]
fn set_binding_rebinds_and_releases_old_key_shape() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    let a = get_or_create_session(&ws, "wf:alpha", "t", false).unwrap();
    make_session(&ws, "11111111-2222-3333-4444-555555555555");

    // 重绑到既有会话（draft_apply 场景：__new__ 目标落到真实工作流名）。
    set_binding(&ws, "wf:alpha", "11111111-2222-3333-4444-555555555555").unwrap();
    let live = live_bindings(&ws);
    assert_eq!(
        live.get("wf:alpha").unwrap(),
        "11111111-2222-3333-4444-555555555555"
    );
    // 旧会话不再持有任何键（键随 upsert 让出）。
    assert!(!live.values().any(|v| *v == a.session_id));

    // 绑到不存在的会话 → 诚实拒绝（孤儿绑定不可能产生）。
    let err = set_binding(&ws, "wf:alpha", "dead-beef");
    assert!(err.is_err());
}

#[test]
fn remove_binding_releases_key_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    let a = get_or_create_session(&ws, "wf_agentgen:__new__", "t", false).unwrap();
    // draft_apply 重绑场景：__new__ 键让给正式工作流键（同会话），再摘掉
    // __new__——下次「新建工作流」不再落进已被接管的会话。
    set_binding(&ws, "wf_agentgen:demo", &a.session_id).unwrap();
    assert!(remove_binding(&ws, "wf_agentgen:__new__").unwrap());
    let live = live_bindings(&ws);
    assert!(!live.contains_key("wf_agentgen:__new__"));
    assert_eq!(live.get("wf_agentgen:demo").unwrap(), &a.session_id);
    // 摘不存在的键：幂等 false。
    assert!(!remove_binding(&ws, "wf_agentgen:__new__").unwrap());
}

#[test]
fn remove_session_cascades_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    let a = get_or_create_session(&ws, "k1", "t", false).unwrap();
    let _b = get_or_create_session(&ws, "k2", "t", false).unwrap();
    // 两个键指向同一会话（模拟历史脏数据）也要全摘。
    set_binding(&ws, "k3", &a.session_id).unwrap();

    let removed = remove_session(&ws, &a.session_id);
    assert_eq!(removed, 2, "k1 + k3 指向同一会话");
    let live = live_bindings(&ws);
    assert!(!live.contains_key("k1"));
    assert!(!live.contains_key("k3"));
    assert!(live.contains_key("k2"));
    // 再删一次：幂等 0。
    assert_eq!(remove_session(&ws, &a.session_id), 0);
}

#[test]
fn bindings_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    let a = get_or_create_session(&ws, "wf:x", "t", false).unwrap();
    // "重启" = 重新读盘（函数即读即写，无进程态缓存）。
    let live = live_bindings(&ws);
    assert_eq!(live.get("wf:x").unwrap(), &a.session_id);
}

#[test]
fn empty_or_overlong_key_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace").to_string_lossy().to_string();
    let _guard = isolate(dir.path());

    assert!(get_or_create_session(&ws, "   ", "t", false).is_err());
    assert!(get_or_create_session(&ws, &"x".repeat(201), "t", false).is_err());
    assert!(set_binding(&ws, "", "abc").is_err());
}
