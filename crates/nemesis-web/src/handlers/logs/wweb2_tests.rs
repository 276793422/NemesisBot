//! logs.rs 安全面覆盖（Phase 3 批次 18，2026-08-25）。
//!
//! 覆盖：`parse_security_audit_file`（管道格式、decision 映射、畸形行跳过）、
//! `security` 命令（多文件合并、risk_level 过滤、时间倒序、分页、_source_file）、
//! `chain_list`/`chain_verify`（真链全绿、篡改定位、分片轮转读取顺序）、
//! `read_meta_title` sidecar、`extract_md_header`/`read_md_section`/
//! `extract_md_first_message` 直测。
//!
//! 磁盘格式以写入端代码为准（nemesis-security/src/audit_log.rs 的管道行、
//! integrity.rs 的 AuditEvent JSONL + `_seg{:04}` 轮转命名），链夹具用真实
//! `AuditChain` 生成——不是手拼的假格式。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use nemesis_security::integrity::{AuditChain, AuditChainConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// -----------------------------------------------------------------------
// parse_security_audit_file（管道格式直测）
// -----------------------------------------------------------------------

#[test]
fn parse_security_audit_file_full_and_edge_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("security_audit_2026-08-25.log");
    // 格式对齐写入端 audit_log.rs：10 字段 " | " 分隔；头部 # 注释行
    std::fs::write(
        &path,
        concat!(
            "# Security audit log\n",
            "# format: ...\n",
            "\n",
            "2026-08-25 10:00:00.123 | evt-1 | allowed | file_write | user1 | ws | a.txt | HIGH | ok | rule-a\n",
            "2026-08-25 11:00:00.456 | evt-2 | denied | process_exec | user2 | ws | b.exe | CRITICAL | bad | rule-b\n",
            "2026-08-25 12:00:00.789 | evt-3 | approved | file_read | user3 | ws | c.txt | LOW | fine | rule-c\n",
            "2026-08-25 13:00:00.000 | evt-4 | warn | file_edit | user4 | ws | d.txt | high | ok | rule-d\n",
            "this line has too few | parts\n",
        ),
    )
    .unwrap();
    let entries = parse_security_audit_file(&path);
    assert_eq!(entries.len(), 4, "header/blank/short lines skipped");
    let e0 = &entries[0];
    assert_eq!(e0["id"], "evt-1");
    assert_eq!(e0["timestamp"], "2026-08-25 10:00:00.123");
    assert_eq!(e0["operation"], "file_write");
    assert_eq!(e0["user"], "user1");
    assert_eq!(e0["target"], "a.txt");
    assert_eq!(e0["risk_level"], "HIGH");
    assert_eq!(e0["reason"], "ok");
    assert_eq!(e0["policy"], "rule-a");
    assert_eq!(e0["raw"]["source"], "ws");
    // decision 映射：allow* / approved（大小写不敏感）→ allow；其余 → deny
    assert_eq!(e0["result"], "allow", "allowed → allow");
    assert_eq!(entries[1]["result"], "deny", "denied → deny");
    assert_eq!(entries[2]["result"], "allow", "approved → allow");
    assert_eq!(entries[3]["result"], "deny", "warn → deny（非 allow 前缀）");
    // decision 原文保留
    assert_eq!(entries[1]["decision"], "denied");
}

#[test]
fn parse_security_audit_file_missing_file_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("security_audit_2099-01-01.log");
    assert!(parse_security_audit_file(&path).is_empty());
}

// -----------------------------------------------------------------------
// security 命令（多文件合并 / 过滤 / 排序 / 分页）
// -----------------------------------------------------------------------

#[tokio::test]
async fn security_command_merges_files_filters_sorts_and_paginates() {
    let dir = tempfile::tempdir().unwrap();
    let sec_dir = dir.path().join("logs/security_logs");
    std::fs::create_dir_all(&sec_dir).unwrap();
    std::fs::write(
        sec_dir.join("security_audit_2026-08-25.log"),
        concat!(
            "# h\n",
            "2026-08-25 10:00:00.000 | evt-1 | allowed | file_write | u1 | ws | a.txt | HIGH | ok | r1\n",
            "2026-08-25 12:00:00.000 | evt-3 | warn | file_edit | u3 | ws | d.txt | high | ok | r3\n",
        ),
    )
    .unwrap();
    // 另一天的文件也要合并；其时间戳最早 → 排最后
    std::fs::write(
        sec_dir.join("security_audit_2026-08-24.log"),
        "2026-08-25 09:00:00.000 | evt-0 | denied | exec | u0 | ws | z.exe | CRITICAL | bad | r0\n",
    )
    .unwrap();
    // 非 security_audit_*.log 文件必须被跳过
    std::fs::write(sec_dir.join("audit_chain.jsonl"), "{}\n").unwrap();
    std::fs::write(sec_dir.join("notes.txt"), "x | x\n").unwrap();

    let ctx = make_ctx(&dir);
    let h = LogsHandler;
    let r = h
        .handle_cmd("security", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 3);
    let entries = r["entries"].as_array().unwrap();
    // 时间倒序：evt-3 (12:00) → evt-1 (10:00) → evt-0 (09:00)
    assert_eq!(entries[0]["id"], "evt-3");
    assert_eq!(entries[1]["id"], "evt-1");
    assert_eq!(entries[2]["id"], "evt-0");
    // _source_file 挂来源文件名
    assert_eq!(entries[2]["_source_file"], "security_audit_2026-08-24.log");

    // risk_level 过滤（大小写不敏感）：HIGH 匹配 HIGH + high
    let r = h
        .handle_cmd(
            "security",
            Some(serde_json::json!({ "risk_level": "HIGH" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 2);
    assert_eq!(r["entries"].as_array().unwrap()[0]["id"], "evt-3");

    // 分页：limit=1 offset=1 → 只剩第二条
    let r = h
        .handle_cmd(
            "security",
            Some(serde_json::json!({ "limit": 1, "offset": 1 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 3, "total 是过滤前全量");
    let page = r["entries"].as_array().unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["id"], "evt-1");
}

#[tokio::test]
async fn security_command_missing_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let r = LogsHandler
        .handle_cmd("security", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 0);
    assert_eq!(r["entries"], serde_json::json!([]));
}

// -----------------------------------------------------------------------
// chain_list / chain_verify（真实 AuditChain 夹具）
// -----------------------------------------------------------------------

/// 在 <ws>/logs/security_logs/ 下用真实 AuditChain 生成 4 条事件，
/// max_events_per_segment=2 → 主文件 2 条 + audit_chain_seg0002.jsonl 2 条。
fn make_real_chain(ws: &std::path::Path) -> PathBuf {
    let sec_dir = ws.join("logs/security_logs");
    std::fs::create_dir_all(&sec_dir).unwrap();
    let config = AuditChainConfig {
        storage_path: sec_dir.join("audit_chain.jsonl"),
        max_events_per_segment: 2,
        ..Default::default()
    };
    let chain = AuditChain::new(config);
    for i in 0..4 {
        chain
            .append(
                &format!("op-{i}"),
                &format!("tool-{i}"),
                "tester",
                "ws",
                &format!("target-{i}"),
                "allowed",
                "ok",
            )
            .unwrap();
    }
    sec_dir.join("audit_chain.jsonl")
}

#[tokio::test]
async fn chain_list_real_chain_all_valid_with_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let main = make_real_chain(dir.path());
    // 轮转确实发生了：主文件 + _seg0002 分片都在
    let seg2 = main
        .parent()
        .unwrap()
        .join("audit_chain_seg0002.jsonl");
    assert!(seg2.exists(), "rotation must have produced a _seg0002 file");

    let ctx = make_ctx(&dir);
    let r = LogsHandler
        .handle_cmd("chain_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 4, "main(2) + seg0002(2) 按时间序合并");
    let segs = r["segments"].as_array().unwrap();
    for (i, s) in segs.iter().enumerate() {
        assert_eq!(s["index"], i);
        assert_eq!(s["valid"], true, "segment {i} must be valid");
        assert_eq!(s["breakReason"], serde_json::Value::Null);
        assert!(s["payloadSummary"].as_str().unwrap().contains("allowed"));
    }
    // 链式 prevHash 衔接（i>0 的 prevHash == 前一条 hash）
    assert_eq!(segs[1]["prevHash"], segs[0]["hash"]);
    assert_eq!(segs[3]["prevHash"], segs[2]["hash"]);

    // 分页
    let r = LogsHandler
        .handle_cmd(
            "chain_list",
            Some(serde_json::json!({ "limit": 2, "offset": 2 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 4);
    let page = r["segments"].as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0]["index"], 2);
}

#[tokio::test]
async fn chain_list_tampered_event_marks_hash_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let main = make_real_chain(dir.path());
    // 篡改主文件第 2 行（index 1）的 operation → 该条 hash 失配；
    // 后续事件的 prev_hash 指向的是「存储的」前条 hash，不受影响。
    let lines: Vec<String> = std::fs::read_to_string(&main)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 2);
    let tampered = lines[1].replace("op-1", "TAMPERED");
    std::fs::write(&main, format!("{}\n{}\n", lines[0], tampered)).unwrap();

    let ctx = make_ctx(&dir);
    let r = LogsHandler
        .handle_cmd("chain_list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let segs = r["segments"].as_array().unwrap();
    assert_eq!(r["total"], 4);
    assert_eq!(segs[1]["valid"], false, "tampered index 1 invalid");
    assert_eq!(segs[1]["breakReason"], "hash mismatch");
    assert_eq!(segs[0]["valid"], true);
    assert_eq!(segs[2]["valid"], true, "prev_hash still links to stored hash");
    assert_eq!(segs[3]["valid"], true);
}

#[tokio::test]
async fn chain_verify_valid_and_tampered() {
    // 1) 真链 → valid
    let dir = tempfile::tempdir().unwrap();
    make_real_chain(dir.path());
    let ctx = make_ctx(&dir);
    let r = LogsHandler
        .handle_cmd("chain_verify", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["valid"], true);
    assert_eq!(r["total_segments"], 4);
    assert_eq!(r["first_broken_index"], serde_json::Value::Null);
    assert_eq!(r["broken_count"], 0);

    // 2) 篡改 → valid false + 首断点定位 + 计数
    let dir2 = tempfile::tempdir().unwrap();
    let main = make_real_chain(dir2.path());
    let lines: Vec<String> = std::fs::read_to_string(&main)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let tampered = lines[1].replace("target-1", "EVIL");
    std::fs::write(&main, format!("{}\n{}\n", lines[0], tampered)).unwrap();
    let ctx2 = make_ctx(&dir2);
    let r = LogsHandler
        .handle_cmd("chain_verify", None, &ctx2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["valid"], false);
    assert_eq!(r["total_segments"], 4);
    assert_eq!(r["first_broken_index"], 1);
    assert_eq!(r["broken_count"], 1);

    // 3) 无链文件 → 空 也 valid
    let dir3 = tempfile::tempdir().unwrap();
    let ctx3 = make_ctx(&dir3);
    let r = LogsHandler
        .handle_cmd("chain_verify", None, &ctx3)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["valid"], true);
    assert_eq!(r["total_segments"], 0);
}

// -----------------------------------------------------------------------
// read_meta_title sidecar
// -----------------------------------------------------------------------

#[test]
fn read_meta_title_variants() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("session-1.jsonl");
    std::fs::write(&jsonl, "{}\n").unwrap();
    // 无 sidecar → None
    assert_eq!(read_meta_title(&jsonl), None);
    // 损坏 JSON → None
    std::fs::write(dir.path().join("session-1.meta.json"), "<<<").unwrap();
    assert_eq!(read_meta_title(&jsonl), None);
    // 无 title 字段 → None
    std::fs::write(
        dir.path().join("session-1.meta.json"),
        serde_json::json!({ "other": 1 }).to_string(),
    )
    .unwrap();
    assert_eq!(read_meta_title(&jsonl), None);
    // 正常 title → Some
    std::fs::write(
        dir.path().join("session-1.meta.json"),
        serde_json::json!({ "title": "我的会话" }).to_string(),
    )
    .unwrap();
    assert_eq!(read_meta_title(&jsonl).as_deref(), Some("我的会话"));
}

// -----------------------------------------------------------------------
// markdown helper 直测
// -----------------------------------------------------------------------

#[test]
fn extract_md_header_direct_variants() {
    let md = "# Request\n\n**Model**: gpt-x\n\n- **Round**: 3\n\n* **Status**: done\n\n**Empty**:\n\n普通中文行 **Model**: 不匹配（非行首）\n\n**model**: lower-case-key\n";
    assert_eq!(extract_md_header(md, "Model").as_deref(), Some("gpt-x"));
    assert_eq!(extract_md_header(md, "Round").as_deref(), Some("3"));
    assert_eq!(extract_md_header(md, "Status").as_deref(), Some("done"));
    // key 大小写不敏感（首个匹配行胜出）
    assert_eq!(extract_md_header(md, "model").as_deref(), Some("gpt-x"));
    // 值为空 → None（不返回空串）
    assert_eq!(extract_md_header(md, "Empty"), None);
    // 不存在的 key → None；多字节行不 panic
    assert_eq!(extract_md_header(md, "Missing"), None);
}

#[test]
fn read_md_section_direct_variants() {
    let md = "# H\n\n## Message\n\nline one\nline two\n\n## Next\n\nother\n";
    let got = read_md_section(md, "Message").unwrap();
    // 节内容含标题后空行与下一标题前空行（逐行原样收集）
    assert_eq!(got, "\nline one\nline two\n\n");
    // 到下一个 # 标题截止
    assert!(read_md_section(md, "Next").unwrap().contains("other"));
    assert_eq!(read_md_section(md, "Absent"), None);
    // 仅含一个空行的节 → Some（非空判定按字节而非 trim 后内容）
    let md2 = "## A\n\n## B\n\nx\n";
    assert_eq!(read_md_section(md2, "A"), Some("\n".to_string()));
}

#[test]
fn extract_md_first_message_truncates_at_200_chars() {
    // ASCII 300 字 → 截 200
    let md = format!("## Message\n\n{}\n", "a".repeat(300));
    let got = extract_md_first_message(&md);
    assert_eq!(got.chars().count(), 200);
    assert!(got.chars().all(|c| c == 'a'));
    // 中文 300 字 → 按字符截 200（不panic、不出半个字）
    let md_cn = format!("## Message\n\n{}\n", "中".repeat(300));
    let got = extract_md_first_message(&md_cn);
    assert_eq!(got.chars().count(), 200);
    assert!(got.chars().all(|c| c == '中'));
    // 无 Message 节 → 空串
    assert_eq!(extract_md_first_message("# nothing\n"), "");
}
