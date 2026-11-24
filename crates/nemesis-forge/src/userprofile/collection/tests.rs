//! userprofile::collection 单测：时间序/证据窗边界（自 profile_service
//! tests 随迁）+ 结构化行（assistant work 工具投影/system_notice 跳过）+
//! todo 采集 + 负反馈事件 IO。

use super::*;

/// tempdir 内摆 session_logs 夹具：`name` 为文件名（.jsonl），rows 为
/// serde json Value 列表（逐行落盘）。
fn seed_rows(ws: &std::path::Path, name: &str, rows: &[serde_json::Value]) {
    let dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = rows.iter().map(|v| format!("{v}\n")).collect();
    std::fs::write(dir.join(name), body).unwrap();
}

fn user_row(ts: &str, content: &str) -> serde_json::Value {
    serde_json::json!({"role": "user", "timestamp": ts, "content": content})
}

/// 时间升序：字典序靠前的文件（a.jsonl）装**新**消息、靠后的（z.jsonl）
/// 装**旧**消息时，返回序必须仍是时间序（旧实现按文件序返回 → 反思采样
/// rev().take(60) 取到旧会话尾巴、漏掉新会话）。
#[test]
fn collect_orders_by_timestamp_not_filename() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "a.jsonl",
        &[
            user_row("2026-10-25T10:00:00Z", "新会话消息一"),
            user_row("2026-10-25T10:01:00Z", "新会话消息二"),
        ],
    );
    seed_rows(
        ws,
        "z.jsonl",
        &[
            user_row("2026-10-25T08:00:00Z", "旧会话消息一"),
            user_row("2026-10-25T08:01:00Z", "旧会话消息二"),
        ],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    let contents: Vec<&str> = out.iter().map(|r| r.content.as_str()).collect();
    assert_eq!(
        contents,
        vec![
            "旧会话消息一",
            "旧会话消息二",
            "新会话消息一",
            "新会话消息二"
        ],
        "按时间升序，与文件名字典序解耦"
    );
}

/// 证据窗边界：早于 window_start 的行被排除（毫秒精度）。
#[test]
fn collect_respects_window_start_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "a.jsonl",
        &[
            user_row("2026-10-25T10:00:00Z", "窗内"),
            user_row("2026-10-25T09:00:00Z", "窗外"),
        ],
    );
    // 25T10:00:00Z = 1792922400000 ms。
    let out = collect_dialogue_after(ws, 1_792_922_400_000).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].content, "窗内");
    // 窗起点 = 窗外消息自身的时间戳（09:00Z）→ 恰好含（>= 语义），时间升序。
    let out = collect_dialogue_after(ws, 1_792_918_800_000).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].content, "窗外");
}

/// 非 jsonl / 坏行 / 非 user/assistant 行跳过；缺目录 = Ok(空)。
#[test]
fn collect_skips_non_dialogue_and_missing_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert_eq!(
        collect_dialogue_after(ws, 0).unwrap(),
        Vec::<DialogueRow>::new()
    );

    let log_dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("a.jsonl"),
        concat!(
            "not json\n",
            "{\"role\":\"boundary\",\"timestamp\":\"2026-10-25T09:59:00Z\"}\n",
            "{\"role\":\"user\",\"timestamp\":\"garbage\",\"content\":\"无时间锚不参与\"}\n",
            "{\"role\":\"user\",\"timestamp\":\"2026-10-25T10:00:00Z\",\"content\":\"有效\"}\n",
        ),
    )
    .unwrap();
    std::fs::write(log_dir.join("b.txt"), "ignored\n").unwrap();
    let out = collect_dialogue_after(ws, 0).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].content, "有效");
}

/// 系统注入行（role=user 带 system_notice——cron tick/审批回灌等）不进
/// 管线：它们不是真人消息，混入会污染统计与反思采样（Forge 化重构批一
/// 根修——旧采集器把它们当成了用户消息）。
#[test]
fn collect_skips_system_notice_rows() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "a.jsonl",
        &[
            user_row("2026-10-25T10:00:00Z", "真人消息"),
            serde_json::json!({
                "role": "user",
                "timestamp": "2026-10-25T10:01:00Z",
                "content": "定时唤醒 tick",
                "system_notice": {"kind": "cron"}
            }),
            serde_json::json!({
                "role": "assistant",
                "timestamp": "2026-10-25T10:02:00Z",
                "content": "助手回复",
                "work": {"rounds": [{"tools": [
                    {"tool": "exec"}, {"tool": "write_file"}
                ]}]}
            }),
        ],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    assert_eq!(out.len(), 2, "system_notice 行不进管线");
    assert_eq!(out[0].role, "user");
    assert_eq!(out[1].role, "assistant");
    // assistant 行带 work 工具名投影（去重保序）。
    assert_eq!(out[1].work_tools, vec!["exec", "write_file"]);
    assert!(out[0].work_tools.is_empty(), "user 行恒无工具投影");
    // 会话名 = 文件名去扩展名。
    assert_eq!(out[0].session, "a");
}

/// assistant work 投影：跨轮去重 + 截 8。
#[test]
fn work_tools_dedupe_and_cap() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let tools: Vec<serde_json::Value> = [
        "exec",
        "write_file",
        "exec",
        "grep",
        "exec",
        "git",
        "exec",
        "web_fetch",
        "exec",
        "read_file",
        "exec",
        "sleep",
    ]
    .iter()
    .map(|t| serde_json::json!({"tool": t}))
    .collect();
    seed_rows(
        ws,
        "a.jsonl",
        &[serde_json::json!({
            "role": "assistant",
            "timestamp": "2026-10-25T10:00:00Z",
            "content": "done",
            "work": {"rounds": [{"tools": tools}]},
        })],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    assert_eq!(
        out[0].work_tools,
        vec![
            "exec",
            "write_file",
            "grep",
            "git",
            "web_fetch",
            "read_file",
            "sleep"
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
        "去重保序 + 截 8"
    );
}

/// todo 采集：解析 Vec<TodoItem> 形态 + 陈旧文件跳过 + 非 todo 前缀跳过。
#[test]
fn collect_todos_reads_fresh_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sessions = ws.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let fresh = serde_json::json!([
        {"content": "实现功能", "status": "completed"},
        {"content": "自测", "status": "in_progress"},
        {"content": "跑测试", "status": "pending"}
    ]);
    std::fs::write(sessions.join("todo_a.json"), fresh.to_string()).unwrap();
    // 陈旧 todo（mtime 拨回 8 天前）→ 跳过。
    let stale_path = sessions.join("todo_stale.json");
    std::fs::write(&stale_path, fresh.to_string()).unwrap();
    let stale_time = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
    filetime::set_file_mtime(
        &stale_path,
        filetime::FileTime::from_system_time(stale_time),
    )
    .unwrap();
    // 非 todo 前缀 / 非 json 后缀 → 跳过。
    std::fs::write(sessions.join("other.json"), fresh.to_string()).unwrap();
    std::fs::write(sessions.join("todo_bad.txt"), "x").unwrap();

    let now = chrono::Utc::now().timestamp_millis();
    let out = collect_todos(ws, now, 7 * 24 * 3600 * 1000).unwrap();
    assert_eq!(out.len(), 1, "陈旧/非 todo 文件不参与");
    assert_eq!(out[0].file, "todo_a.json");
    assert_eq!(out[0].items.len(), 3);
    assert_eq!(out[0].items[0], ("实现功能".into(), "completed".into()));
    assert_eq!(out[0].items[2], ("跑测试".into(), "pending".into()));
    // 损坏 todo 静默跳过（不炸采集）。
    std::fs::write(sessions.join("todo_broken.json"), "{not json").unwrap();
    let out = collect_todos(ws, now, 7 * 24 * 3600 * 1000).unwrap();
    assert_eq!(out.len(), 1);
}

/// 负反馈事件：append → read_recent 尾部取回（升序保序）。
#[test]
fn events_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(read_recent_events(ws, 10).is_empty(), "缺文件 = 空");
    append_event(ws, &serde_json::json!({"kind": "veto", "content": "甲"}));
    append_event(ws, &serde_json::json!({"kind": "forget", "content": "乙"}));
    let out = read_recent_events(ws, 10);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["kind"], "veto");
    assert_eq!(out[1]["kind"], "forget");
    // limit 截尾（取最近 N 条）。
    let out = read_recent_events(ws, 1);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["kind"], "forget");
}

// ---- 批 C 采集增强（2026-11-24）：四投影 + signals.jsonl ----

/// 富行投影：assistant 行的 file_changes/deliverables 分类去重、work 轮
/// 叙述（trim + 截 80 + 工具截 3、总条数 ≤3）、channel/model 键原样透传；
/// user 行即使带同款键也恒空（批 C 四投影 assistant 行专属）。
#[test]
fn batch_c_rich_row_projections() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "a.jsonl",
        &[serde_json::json!({
            "role": "assistant",
            "timestamp": "2026-11-24T10:00:00Z",
            "content": "完成了",
            "model": "glm-5.3-flash",
            "file_changes": [
                {"path": "src/lib.rs"},
                {"path": "docs/PLAN/x.md"},
                {"path": "src/lib.rs"},
            ],
            "deliverables": [{"path": "images/out.png"}],
            "work": {"rounds": [
                {"text": "  先读了入口模块  ", "tools": [{"tool": "read_file"}, {"tool": "grep"}, {"tool": "exec"}, {"tool": "write_file"}]},
                {"text": ""},
                {"tools": [{"tool": "exec"}]},
                {"text": "再改配置并跑测试"},
                {"text": "最后写文档"},
                {"text": "第四条叙述不该出现"},
            ]},
        })],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    assert_eq!(out.len(), 1);
    let r = &out[0];
    // 文件种类：rs→code、md→doc、png→image，重复 rs 去重保序。
    assert_eq!(r.file_kinds, ["code", "doc", "image"]);
    // 叙述：trim + 工具截 3；空叙述轮/纯工具轮跳过；总条数截 3。
    assert_eq!(
        r.work_notes,
        vec![
            "先读了入口模块｜工具: read_file, grep, exec".to_string(),
            "再改配置并跑测试".to_string(),
            "最后写文档".to_string(),
        ]
    );
    assert_eq!(r.channel, "");
    assert_eq!(r.model, "glm-5.3-flash");
    assert_eq!(r.work_tools.len(), 4);

    // user 行带同款键 → 四投影恒空（含 channel 透传）。
    seed_rows(
        ws,
        "b.jsonl",
        &[serde_json::json!({
            "role": "user",
            "timestamp": "2026-11-24T10:01:00Z",
            "content": "继续",
            "channel": "telegram",
            "file_changes": [{"path": "x.rs"}],
            "work": {"rounds": [{"text": "叙述不该投影", "tools": [{"tool": "exec"}]}]},
        })],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    let u = out.last().unwrap();
    assert!(u.file_kinds.is_empty());
    assert!(u.work_notes.is_empty());
    assert!(u.work_tools.is_empty());
    assert_eq!(u.channel, "telegram");
    assert_eq!(u.model, "");
}

/// 历史兼容：旧条目缺新键 → 缺省值（channel/model = ""、vecs 空）照常
/// 解析，零迁移。
#[test]
fn batch_c_legacy_rows_compat() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "old.jsonl",
        &[
            user_row("2026-06-01T08:00:00Z", "老格式用户消息"),
            serde_json::json!({
                "role": "assistant",
                "timestamp": "2026-06-01T08:00:05Z",
                "content": "老格式助手回复",
            }),
        ],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    assert_eq!(out.len(), 2);
    for r in &out {
        assert_eq!(r.channel, "", "缺 channel 键 = 未知通道");
        assert_eq!(r.model, "", "缺 model 键 = 空模型");
        assert!(r.file_kinds.is_empty());
        assert!(r.work_notes.is_empty());
    }
}

/// 叙述截 80 字符（rune 安全：中文 40 字内不截、41+ 截）。
#[test]
fn batch_c_work_note_char_cap() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let long = "甲".repeat(100);
    let expect: String = "甲".repeat(80);
    seed_rows(
        ws,
        "a.jsonl",
        &[serde_json::json!({
            "role": "assistant",
            "timestamp": "2026-11-24T10:00:00Z",
            "content": "done",
            "work": {"rounds": [{"text": long}]},
        })],
    );
    let out = collect_dialogue_after(ws, 0).unwrap();
    assert_eq!(out[0].work_notes, vec![expect]);
}

/// signals.jsonl：append → read_recent 尾部取回（升序保序）；缺文件 = 空。
#[test]
fn signals_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(read_recent_signals(ws, 10).is_empty(), "缺文件 = 空");
    append_signal(
        ws,
        &serde_json::json!({
            "ts_ms": 1, "kind": "estop", "action": "engage"
        }),
    );
    append_signal(
        ws,
        &serde_json::json!({
            "ts_ms": 2, "kind": "approval", "operation": "exec",
            "risk_level": "HIGH", "approved": true, "duration_ms": 3200,
            "session_key": "s1"
        }),
    );
    append_signal(
        ws,
        &serde_json::json!({
            "ts_ms": 3, "kind": "approval", "operation": "write_file",
            "risk_level": "HIGH", "approved": false, "duration_ms": 15000,
            "session_key": "s1"
        }),
    );
    let out = read_recent_signals(ws, 10);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["kind"], "estop");
    assert_eq!(out[2]["approved"], false);
    // limit 截尾（取最近 N 条）。
    let out = read_recent_signals(ws, 2);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["kind"], "approval");
    assert_eq!(out[1]["duration_ms"], 15000);
}
