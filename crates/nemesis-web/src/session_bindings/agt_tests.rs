//! session_bindings.rs AGT 覆盖率批次（2026-09-25）：get_or_create_session
//! 的 manual_title=true 臂（157-158）——显式命名走 write_session_meta_manual
//! （title_manual 旗标，自动标题永不覆盖）。
//!
//! 结构性豁免（见报告）：
//! - 71（save_bindings 的 if-let 收括号）：体（70 create_dir_all）有正计数，
//!   lcov 收括号归因伪零。
//! - 220（remove_session 的锁毒化 return 0）：std Mutex 中毒臂——需先在持
//!   锁期间 panic 毒化全局锁，无安全确定性触发手段。

use super::*;

#[test]
fn agt_get_or_create_manual_title_writes_manual_flag() {
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let key = format!(
        "agt-bind-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let out = get_or_create_session(&ws, &key, "我的手动标题", true).unwrap();
    assert!(out.created, "首次创建必须 created=true");
    assert_eq!(out.title, "我的手动标题");

    // sidecar 已带 manual 旗标（自动标题不得覆盖）。
    let sid = out.session_id.clone();
    let skey = format!("agent:main:session:{sid}");
    let meta = nemesis_agent::chat_log::read_session_meta_full(&skey).expect("meta sidecar");
    assert!(meta.title_manual, "manual_title 必须落 title_manual");

    // 种工作区会话工件（session_artifact_exists 的存活标记），再取同键：
    // 幂等命中 created=false，session_id 不换。
    let logs = nemesis_path::resolve_session_logs_dir_in_workspace(std::path::Path::new(&ws));
    std::fs::create_dir_all(&logs).unwrap();
    let safe = nemesis_utils::sanitize::sanitize_path_segment(&skey);
    std::fs::write(logs.join(format!("{safe}.jsonl")), "").unwrap();

    let again = get_or_create_session(&ws, &key, "不该覆盖的新名", true).unwrap();
    assert!(!again.created, "存活绑定必须幂等命中");
    assert_eq!(again.session_id, sid);

    // 清理全局目录产物（nanos 唯一 key）。
    nemesis_agent::chat_log::delete_chat_log(&skey);
}
