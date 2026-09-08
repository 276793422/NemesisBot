//! L6++（2026-09-08）：scan_session_logs 的项目归属回填——sidecar meta 带
//! `project_id`/`project_path` 的条目补 `projectId`/`projectPath` 两字段；
//! 无绑定（对话组会话/pre-L6++ meta）干净缺省不加字段。前端以 projects.list
//! 联结显示名（注册表在 nemesisbot，后端不解析名字）。
//!
//! 夹具镜像 e4_lineage_tests：workspace=tempdir，手工落 jsonl + meta.json
//! （与 chat_log.rs 写盘同形），不依赖全局 path manager。

use super::*;

/// 在 workspace 的 session_logs 下造一个会话：一行 user jsonl + 可选 meta。
fn seed_session(ws: &Path, stem: &str, first: &str, meta_json: Option<&str>) {
    let dir = session_log_dir(ws.to_str().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let row = serde_json::json!({
        "role": "user",
        "content": first,
        "timestamp": "2026-09-08T00:00:00+08:00"
    });
    std::fs::write(
        dir.join(format!("{stem}.jsonl")),
        serde_json::to_string(&row).unwrap() + "\n",
    )
    .unwrap();
    if let Some(m) = meta_json {
        std::fs::write(dir.join(format!("{stem}.meta.json")), m).unwrap();
    }
}

#[test]
fn scan_backfills_project_id_and_path() {
    let ws = tempfile::tempdir().unwrap();
    // sessions.create 成功路径形态：meta 带 title + 双绑定字段。
    seed_session(
        ws.path(),
        "agent_main_session_b1",
        "bound first",
        Some(
            r#"{"title":"项目会话","project_id":"p-abc12345","project_path":"C:/proj/demo"}"#,
        ),
    );

    let sessions = scan_session_logs(ws.path().to_str().unwrap());
    let bound = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_b1")
        .expect("绑定会话在列表中");
    assert_eq!(bound["projectId"], "p-abc12345");
    assert_eq!(bound["projectPath"], "C:/proj/demo");
}

#[test]
fn scan_unbound_sessions_get_no_project_fields() {
    let ws = tempfile::tempdir().unwrap();
    // pre-L6++ meta：只有 title → 不加项目字段。
    seed_session(
        ws.path(),
        "agent_main_session_old",
        "old first",
        Some(r#"{"title":"old"}"#),
    );
    // 完全无 meta sidecar。
    seed_session(ws.path(), "agent_main_session_bare", "bare first", None);
    // 单边字段（只有 id）：防御性——有啥回填啥，不造半个绑定。
    seed_session(
        ws.path(),
        "agent_main_session_half",
        "half first",
        Some(r#"{"title":"half","project_id":"p-ffffffff"}"#),
    );

    let sessions = scan_session_logs(ws.path().to_str().unwrap());
    for id in ["agent_main_session_old", "agent_main_session_bare"] {
        let s = sessions.iter().find(|s| s["id"] == id).unwrap();
        assert!(s.get("projectId").is_none(), "{id} 不该有 projectId");
        assert!(s.get("projectPath").is_none());
    }
    let half = sessions
        .iter()
        .find(|s| s["id"] == "agent_main_session_half")
        .unwrap();
    assert_eq!(half["projectId"], "p-ffffffff");
    assert!(half.get("projectPath").is_none(), "单边字段诚实缺省");
}
