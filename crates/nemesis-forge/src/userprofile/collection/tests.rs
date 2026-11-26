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

/// 文件 mtime 预过滤（复查 2026-11-25 批 F2 回归锁）：mtime < 窗起点 ⇒
/// 整文件不可能有窗内行（行先写、mtime 后盖）——stat 跳过免全量读；
/// 新鲜 mtime 照常采集（既有窗口测试的 mtime=now 天然在此臂）。
#[test]
fn collect_skips_files_whose_mtime_precedes_window() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "old.jsonl",
        &[user_row("2026-10-25T10:00:00Z", "旧文件行")],
    );
    // 回拨 mtime 到窗起点之前（1ms > UNIX_EPOCH，避开 0 的平台怪癖）。
    let p = ws.join("logs").join("session_logs").join("old.jsonl");
    let f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_000)),
    )
    .unwrap();
    drop(f);
    let out = collect_dialogue_after(ws, 60_000).unwrap();
    assert!(out.is_empty(), "mtime 早于窗起点的文件整文件跳过");
    // 对照臂：同窗起点、新鲜 mtime 文件 → 行照常进管线。
    seed_rows(
        ws,
        "new.jsonl",
        &[user_row("2026-10-25T10:01:00Z", "新文件行")],
    );
    let out = collect_dialogue_after(ws, 60_000).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].content, "新文件行");
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
/// 批 F3 起 append_event 认总闸——测试需 enabled home（与 signals 同纪律）。
#[test]
fn events_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    let home_s = home.to_str().unwrap();
    assert!(read_recent_events(&ws, 10).is_empty(), "缺文件 = 空");
    append_event(
        &ws,
        Some(home_s),
        &serde_json::json!({"kind": "veto", "content": "甲"}),
    );
    append_event(
        &ws,
        Some(home_s),
        &serde_json::json!({"kind": "forget", "content": "乙"}),
    );
    let out = read_recent_events(&ws, 10);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["kind"], "veto");
    assert_eq!(out[1]["kind"], "forget");
    // limit 截尾（取最近 N 条）。
    let out = read_recent_events(&ws, 1);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["kind"], "forget");
}

/// events 闸四臂（复查 2026-11-25 批 F3 回归锁，镜像 signals 闸测试）：
/// config 缺失 fail-closed / home=None / enabled=false / enabled=true 落。
#[test]
fn events_gate_respects_profile_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ev = || serde_json::json!({"ts_ms": 1, "kind": "veto", "content": "x"});
    // home 缺失（无 config.json）→ fail-closed 不落。
    let no_cfg_home = dir.path().join("home_nocfg");
    std::fs::create_dir_all(&no_cfg_home).unwrap();
    append_event(&ws, Some(no_cfg_home.to_str().unwrap()), &ev());
    assert!(
        !ws.join("forge")
            .join("userprofile")
            .join("events.jsonl")
            .exists(),
        "config 缺失 = 闸面不可得，fail-closed 不落"
    );
    // home = None → 不落。
    append_event(&ws, None, &ev());
    assert!(!ws.join("forge").exists(), "home 缺失 = 不落");
    // enabled=false → 不落（独立 home 路径防 GATE 缓存串扰）。
    let off_home = dir.path().join("home_off");
    std::fs::create_dir_all(&off_home).unwrap();
    std::fs::write(
        off_home.join("config.json"),
        r#"{"user_profile": {"enabled": false}}"#,
    )
    .unwrap();
    append_event(&ws, Some(off_home.to_str().unwrap()), &ev());
    assert!(
        !ws.join("forge")
            .join("userprofile")
            .join("events.jsonl")
            .exists(),
        "enabled=false 事件也不落（存储即隐私闸统一）"
    );
    // enabled=true → 落。
    let on_home = dir.path().join("home_on");
    std::fs::create_dir_all(&on_home).unwrap();
    std::fs::write(
        on_home.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    append_event(&ws, Some(on_home.to_str().unwrap()), &ev());
    let content =
        std::fs::read_to_string(ws.join("forge").join("userprofile").join("events.jsonl")).unwrap();
    assert_eq!(content.lines().count(), 1, "只有总闸开启那一条落盘");
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

/// signals.jsonl：append → read_recent 尾部取回（升序保序）；缺文件 = 空；
/// 总闸（复查 2026-11-25）：enabled=false / home 缺失 = 不落。
#[test]
fn signals_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 种 home config（enabled=true——闸面现读）。
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    let home_s = home.to_str().unwrap();
    assert!(read_recent_signals(ws, 10).is_empty(), "缺文件 = 空");
    append_signal(
        ws,
        Some(home_s),
        &serde_json::json!({
            "ts_ms": 1, "kind": "estop", "action": "engage"
        }),
    );
    append_signal(
        ws,
        Some(home_s),
        &serde_json::json!({
            "ts_ms": 2, "kind": "approval", "operation": "exec",
            "risk_level": "HIGH", "approved": true, "duration_ms": 3200,
            "session_key": "s1"
        }),
    );
    append_signal(
        ws,
        Some(home_s),
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

/// 总闸（复查 2026-11-25）：`user_profile.enabled=false` = 信号也不落
/// （存储即隐私闸统一——关闭状态下信号零消费者）；home 缺失 =
/// 闸面不可得，fail-closed 不落。
#[test]
fn signals_gate_respects_profile_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    // home 缺失（无 config.json）→ fail-closed 不落。
    let no_cfg_home = dir.path().join("home_nocfg");
    std::fs::create_dir_all(&no_cfg_home).unwrap();
    append_signal(
        &ws,
        Some(no_cfg_home.to_str().unwrap()),
        &serde_json::json!({"ts_ms": 1, "kind": "estop", "action": "engage"}),
    );
    assert!(
        !ws.join("forge")
            .join("userprofile")
            .join("signals.jsonl")
            .exists(),
        "config 缺失 = 闸面不可得，fail-closed 不落"
    );
    // home = None → 不落。
    append_signal(
        &ws,
        None,
        &serde_json::json!({"ts_ms": 2, "kind": "estop", "action": "engage"}),
    );
    assert!(!ws.join("forge").exists(), "home 缺失 = 不落");
    // enabled=false → 不落（独立 home 路径防 GATE 缓存串扰）。
    let off_home = dir.path().join("home_off");
    std::fs::create_dir_all(&off_home).unwrap();
    std::fs::write(
        off_home.join("config.json"),
        r#"{"user_profile": {"enabled": false}}"#,
    )
    .unwrap();
    append_signal(
        &ws,
        Some(off_home.to_str().unwrap()),
        &serde_json::json!({"ts_ms": 3, "kind": "estop", "action": "engage"}),
    );
    assert!(
        !ws.join("forge")
            .join("userprofile")
            .join("signals.jsonl")
            .exists(),
        "enabled=false 信号也不落（隐私闸统一）"
    );
    // enabled=true（ui_telemetry 缺省）→ 落（信号面只认总闸）。
    let on_home = dir.path().join("home_on");
    std::fs::create_dir_all(&on_home).unwrap();
    std::fs::write(
        on_home.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    append_signal(
        &ws,
        Some(on_home.to_str().unwrap()),
        &serde_json::json!({"ts_ms": 4, "kind": "estop", "action": "engage"}),
    );
    let content =
        std::fs::read_to_string(ws.join("forge").join("userprofile").join("signals.jsonl"))
            .unwrap();
    assert_eq!(content.lines().count(), 1, "只有总闸开启那一条落盘");
}

// ---------- 终端会话采集（2026-11-24 T 批） ----------

/// 解析 happy path：shell 头提取 / 输入帧出命令 / 输出帧不出命令 /
/// 续行归属 / 空行跳过。
#[test]
fn terminal_audit_parse_basic_frames() {
    let audit = "# session log\n\
                 # shell: pwsh\n\
                 # 注：终端原始字节含回显密码，本文件应视为敏感\n\
                 \n\
                 [I] git status\n\
                 \n\
                 [O] On branch main\n\
                 \n\
                 [I] cargo build --release\n\
                 \n\
                 [O] Finished\n\
                 line2-of-output\n\
                 \n\
                 [I] kubectl get pods\n";
    let (shell, commands) = parse_terminal_audit(audit);
    assert_eq!(shell, "pwsh");
    assert_eq!(
        commands,
        vec!["git status", "cargo build --release", "kubectl get pods"]
    );
}

/// 凭据两道防线：输出帧密码提示 → 下一条输入行整行丢弃（时序正确：
/// 提示只压制其后输入，之前已收集的命令不受影响）；秘密形态输入行
/// 直接丢弃；ANSI 行跳过。
#[test]
fn terminal_audit_parse_credential_guards() {
    let audit = "[I] cargo build\n\
                 \n\
                 [O] [sudo] password for zoo:\n\
                 \n\
                 [I] hunter2\n\
                 \n\
                 [I] curl -H \"Authorization: Bearer sk-abc123\" https://x\n\
                 \n\
                 [I] \x1b[31m colored\n\
                 \n\
                 [I] git status\n";
    let (_, commands) = parse_terminal_audit(audit);
    assert_eq!(
        commands,
        vec!["cargo build", "git status"],
        "密码邻接行 + 秘密形态行 + ANSI 行全部不进命令流，其余保留"
    );
}

/// 续行输入帧：终端原始数据换行 → 多条输入行；行长截 120 字符。
#[test]
fn terminal_audit_parse_continuation_and_line_cap() {
    let long = "x".repeat(300);
    let audit = format!(
        "[I] echo one\n\
         tail-of-same-frame\n\
         \n\
         [I] {long}\n"
    );
    let (_, commands) = parse_terminal_audit(&audit);
    assert_eq!(commands.len(), 3);
    assert_eq!(commands[0], "echo one");
    assert_eq!(commands[1], "tail-of-same-frame");
    assert_eq!(commands[2].chars().count(), 120, "超长行截断");
}

/// 单会话命令条数上限 200。
#[test]
fn terminal_audit_parse_session_cap() {
    let mut audit = String::new();
    for i in 0..250 {
        audit.push_str(&format!("[I] cmd{i}\n\n"));
    }
    let (_, commands) = parse_terminal_audit(&audit);
    assert_eq!(commands.len(), 200);
}

/// 采集 IO：正常文件收集（shell + mtime 在窗内）；窗外 mtime 跳过；
/// 非 pty_ 前缀 / 非 .log 后缀跳过；缺目录 = 空。
#[test]
fn collect_terminal_sessions_window_and_shape() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let tdir = ws.join("logs").join("terminal");
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(
        tdir.join("pty_20261124_1.log"),
        "# shell: pwsh\n\n[I] git status\n",
    )
    .unwrap();
    // 非 pty 审计文件（同目录的其它产物）跳过。
    std::fs::write(tdir.join("readme.txt"), "[I] hack\n").unwrap();

    let now_ms = chrono::Utc::now().timestamp_millis();
    let out = collect_terminal_sessions(ws, now_ms - 60_000).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].shell, "pwsh");
    assert_eq!(out[0].commands, vec!["git status"]);
    assert_eq!(out[0].file, "pty_20261124_1.log");

    // mtime 早于窗起点 → 整文件跳过。
    let old = std::fs::OpenOptions::new()
        .append(true)
        .open(tdir.join("pty_20261124_1.log"))
        .unwrap();
    old.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_000)),
    )
    .unwrap();
    drop(old);
    let out = collect_terminal_sessions(ws, now_ms - 60_000).unwrap();
    assert!(out.is_empty(), "窗外文件必须跳过");

    // 缺目录 = 空（从未用内嵌终端的常态）。
    let empty_ws = dir.path().join("no-terminal");
    std::fs::create_dir_all(&empty_ws).unwrap();
    assert!(collect_terminal_sessions(&empty_ws, 0).unwrap().is_empty());
}

/// 非法 UTF-8 容错（复查 2026-11-25）：pty 写侧落**原始字节**——ConPTY 读
/// 块边界可劈开多字节字符（中文输出高发）、中文 Windows cmd.exe 默认
/// chcp 936 输出直接是 GBK。旧行为 read_to_string 整文件 Err → 整会话
/// 静默丢失；lossy 解码后坏字节变替换符，ASCII 输入命令照常采集。
#[test]
fn collect_terminal_sessions_lossy_invalid_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let tdir = ws.join("logs").join("terminal");
    std::fs::create_dir_all(&tdir).unwrap();
    // GBK「本地目录」（\xB1\xBE\xB5\xD8\xC4\xBF\xC2\xBC）夹在输出帧——
    // 每字节都是 10xxxxxx 连续形态，非法 UTF-8。
    let mut body: Vec<u8> = b"# shell: cmd\n\n[I] git status\n[O] ".to_vec();
    body.extend_from_slice(&[0xB1, 0xBE, 0xB5, 0xD8, 0xC4, 0xBF, 0xC2, 0xBC]);
    body.extend_from_slice(b"\n[I] cd src\n");
    std::fs::write(tdir.join("pty_gbk.log"), &body).unwrap();

    let now_ms = chrono::Utc::now().timestamp_millis();
    let out = collect_terminal_sessions(ws, now_ms - 60_000).unwrap();
    assert_eq!(out.len(), 1, "非法 UTF-8 文件不再整会话丢失");
    assert_eq!(out[0].shell, "cmd");
    assert_eq!(out[0].commands, vec!["git status", "cd src"]);
}

// ---------- 旁路文件清理（复查 2026-11-25 批 D） ----------

/// signals/events 清理：早于 cutoff 的行整行删除、边界保留（< 严格比较）、
/// 损坏行不过滤（不吃数据）、缺文件 = Ok(0)。
#[test]
fn prune_signals_and_events_older_than_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // events 走 append 需闸开启的 home（批 F3 起 events 与 signals 同闸）。
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    let home_s = home.to_str().unwrap();
    for ts in [1i64, 2, 3] {
        append_event(
            ws,
            Some(home_s),
            &serde_json::json!({"ts_ms": ts, "kind": "veto", "content": "x"}),
        );
        append_signal(
            ws,
            Some(home_s),
            &serde_json::json!({"ts_ms": ts, "kind": "estop", "action": "engage"}),
        );
    }
    // 损坏行（两侧各一条）——清理不过滤损坏行。
    let sp = ws.join("forge").join("userprofile").join("signals.jsonl");
    let mut sig = std::fs::read_to_string(&sp).unwrap();
    sig.push_str("not-json\n");
    std::fs::write(&sp, sig).unwrap();
    let ep = ws.join("forge").join("userprofile").join("events.jsonl");
    let mut ev = std::fs::read_to_string(&ep).unwrap();
    ev.push_str("{\"ts_ms\": \"oops\"}\n");
    std::fs::write(&ep, ev).unwrap();

    // cutoff=2 → ts=1 删（1 < 2），ts=2/3 严格保留（边界不清）。
    let pruned = prune_events_older_than(ws, 2).unwrap();
    assert_eq!(pruned, 1, "events 只删早于 cutoff 的行");
    let pruned = prune_signals_older_than(ws, 2).unwrap();
    assert_eq!(pruned, 1, "signals 只删早于 cutoff 的行");

    let ev_out = read_recent_events(ws, 10);
    assert_eq!(ev_out.len(), 3, "剩余 2 有效行 + 1 损坏行原样保留");
    assert_eq!(ev_out[0]["ts_ms"], 2, "边界行（==cutoff）保留且升序在前");
    assert_eq!(ev_out[1]["ts_ms"], 3);
    assert_eq!(ev_out[2]["ts_ms"], "oops", "损坏行不过滤");

    let sig_out = read_recent_signals(ws, 10);
    assert_eq!(sig_out.len(), 2, "读侧跳过损坏行（ts2/ts3）");
    assert_eq!(sig_out[0]["ts_ms"], 2);
    let raw = std::fs::read_to_string(&sp).unwrap();
    assert!(raw.contains("not-json"), "signals 损坏行原样保留");
    assert_eq!(raw.lines().count(), 3);

    // 缺文件 = Ok(0)。
    let empty_ws = dir.path().join("fresh_ws");
    std::fs::create_dir_all(&empty_ws).unwrap();
    assert_eq!(prune_events_older_than(&empty_ws, 1).unwrap(), 0);
    assert_eq!(prune_signals_older_than(&empty_ws, 1).unwrap(), 0);
}

// ---------- scope 分流（批二 D5，2026-11-25） ----------

/// 在 session_logs 摆 sidecar meta 文件（`{stem}.meta.json`）。
fn seed_meta(ws: &std::path::Path, stem: &str, body: &str) {
    let dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{stem}.meta.json")), body).unwrap();
}

/// scope 分流（D5 回归锁）：sidecar `project_id` 非空 = 项目会话；
/// sidecar 缺席 / project_id 空 / project_id 非字符串 / sidecar 损坏 JSON
/// = 对话会话（诚实缺省）。对话 scope 收前三类，项目 scope 只收项目会话
/// ——行互不串流。
#[test]
fn collect_scope_routes_by_sidecar_project_id() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "plain.jsonl",
        &[user_row("2026-10-25T10:00:00Z", "对话 Plain")],
    );
    seed_rows(
        ws,
        "proj.jsonl",
        &[user_row("2026-10-25T10:01:00Z", "项目 Proj")],
    );
    seed_meta(ws, "proj", r#"{"project_id": "1", "project_path": "D:/w"}"#);
    seed_rows(
        ws,
        "empty.jsonl",
        &[user_row("2026-10-25T10:02:00Z", "对话 Empty")],
    );
    seed_meta(ws, "empty", r#"{"project_id": ""}"#);
    seed_rows(
        ws,
        "nullid.jsonl",
        &[user_row("2026-10-25T10:03:00Z", "对话 NullId")],
    );
    seed_meta(ws, "nullid", r#"{"project_id": 7}"#);
    seed_rows(
        ws,
        "broken.jsonl",
        &[user_row("2026-10-25T10:04:00Z", "对话 Broken")],
    );
    seed_meta(ws, "broken", "not-json");
    // 含点会话键（回归锁 2026-11-25）：sidecar 路径必须一步
    // `with_extension("meta.json")` 构造——两步「先剥后接」会把
    // `dot.key.jsonl` 错位成 `dot.meta.json`（`key` 被当扩展名替换掉），
    // sidecar 永远读不见、项目会话被误归对话。
    seed_rows(
        ws,
        "dot.key.jsonl",
        &[user_row("2026-10-25T10:05:00Z", "项目 Dotted")],
    );
    seed_meta(ws, "dot.key", r#"{"project_id": "1"}"#);
    seed_rows(
        ws,
        "dot.plain.jsonl",
        &[user_row("2026-10-25T10:06:00Z", "对话 Dotted")],
    );

    let dialogue =
        collect_dialogue_after_scope(ws, 0, super::super::store::ProfileScope::Dialogue).unwrap();
    let mut d_contents: Vec<&str> = dialogue.iter().map(|r| r.content.as_str()).collect();
    d_contents.sort();
    assert_eq!(
        d_contents,
        vec![
            "对话 Broken",
            "对话 Dotted",
            "对话 Empty",
            "对话 NullId",
            "对话 Plain"
        ],
        "对话 scope = 无 sidecar/空/非串/损坏 meta 的会话（含含点键无 sidecar 会话）"
    );
    assert!(
        dialogue
            .iter()
            .all(|r| r.session != "proj" && r.session != "dot.key"),
        "项目会话不进对话流（含含点键项目会话）"
    );

    let project =
        collect_dialogue_after_scope(ws, 0, super::super::store::ProfileScope::Project).unwrap();
    assert_eq!(project.len(), 2, "项目 scope 只收项目会话");
    // 行按时间升序（out.sort_by_key(ts_ms)）。
    assert_eq!(project[0].session, "proj");
    assert_eq!(project[0].content, "项目 Proj");
    assert_eq!(project[1].session, "dot.key");
    assert_eq!(project[1].content, "项目 Dotted");
}

/// 便捷包装等价：`collect_dialogue_after` == scope 版 Dialogue（存量消费
/// 方零迁移的行为锚）。
#[test]
fn collect_dialogue_after_wrapper_is_dialogue_scope() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_rows(
        ws,
        "proj.jsonl",
        &[user_row("2026-10-25T10:00:00Z", "项目行")],
    );
    seed_meta(ws, "proj", r#"{"project_id": "1"}"#);
    seed_rows(
        ws,
        "plain.jsonl",
        &[user_row("2026-10-25T10:01:00Z", "对话行")],
    );
    let wrapper = collect_dialogue_after(ws, 0).unwrap();
    let scoped =
        collect_dialogue_after_scope(ws, 0, super::super::store::ProfileScope::Dialogue).unwrap();
    assert_eq!(wrapper, scoped);
    assert_eq!(wrapper.len(), 1);
    assert_eq!(wrapper[0].content, "对话行");
}

/// sidecar 路径构造不变量（回归锁 2026-11-25，穷举键形态）：读端一步
/// `with_extension("meta.json")` 必须与写端 `format!("{safe}.meta.json")`
/// （chat_log::meta_path）对**所有 sanitize 合法 safe key** 逐字节相等。
/// sanitize 值域特征：无前导点、点只出现在字母数字之后（含尾点）——这里
/// 按值域直接取代表形态；两步「先剥后接」对含点键错位（`a.b.jsonl` →
/// `a.meta.json`），此测试防该形态以任何变体回潮。
/// （`session_evidence::sidecar_scope` 同族不变量由其 tests 含点键端到端
/// 用例钉住；读端实现同形。）
#[test]
fn sidecar_path_one_step_equals_writer_construction_for_all_key_forms() {
    let forms = [
        "abc",                    // 无点（uuid/数字键常态）
        "web_550e8400-e29b-41d4", // 下划线+连字符
        "a.b",                    // 内部点（email/域名型键）
        "user@qq.com",            // email 型（两步错位重灾区）
        "abc.",                   // 尾点（sanitize 允许：点跟在字母数字后）
        "x.jsonl",                // 键里带 ".jsonl"（双扩展名）
        "会话.1",                 // Unicode 字母数字 + 点
        "a.b.c.d",                // 多点
    ];
    for s in forms {
        let log_str = format!("{s}.jsonl");
        let reader_form = std::path::Path::new(&log_str).with_extension("meta.json");
        let writer_form = format!("{s}.meta.json");
        assert_eq!(
            reader_form,
            std::path::Path::new(&writer_form),
            "键形态 {s:?}: 读端一步构造必须等于写端 format! 形态"
        );
    }
}
