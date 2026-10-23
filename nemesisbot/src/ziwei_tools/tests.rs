//! ziwei_tools 桥测试（外置：`#[cfg(test)] mod tests;` 约定）。
//! 纯名面/投影面（不依赖活目标；注册/执行面在 m2 live 集成测试）。

use super::*;

#[test]
fn dynamic_tool_name_sanitize() {
    assert_eq!(
        dynamic_tool_name("dummy-monitor", "mon_state"),
        "zw_dummy_monitor_mon_state"
    );
    assert_eq!(dynamic_tool_name("ABC", "t1"), "zw_abc_t1");
    // 空段 / 数字开头 → 前导补 z
    assert!(dynamic_tool_name("9x", "t").starts_with("zw_z9x_t"));
}

#[test]
fn dynamic_tool_name_length_bounded() {
    let long = "x".repeat(200);
    let name = dynamic_tool_name(&long, &long);
    assert!(name.len() <= 64, "超长输入应截断（got {}）", name.len());
    assert!(name.starts_with(TOOL_NAME_PREFIX));
}

#[test]
fn tool_projection_min_tier_and_readonly() {
    let tool = ToolDef {
        tool_name: "mon_state".into(),
        description: "d".into(),
        input_schema: serde_json::Value::Null,
        action: nemesis_ziwei::profile::ToolAction {
            primitive: Primitive::PollReport,
            addr: "struct:MonitorState".into(),
            mode: None,
            interval_secs: Some(2),
            max_records: None,
            timeout_ms: None,
            notify: None,
        },
        output: None,
        run_policy: RunPolicy::Auto,
    };
    let t = ZiweiDynamicTool {
        profile_id: "p".into(),
        profile_name: "n".into(),
        tool: tool.clone(),
    };
    assert_eq!(t.min_tier(), Some("normal"));
    assert!(t.is_read_only());
    let ov = t.operation_override("{}").expect("必须有投影");
    assert_eq!(ov.op_type, "ziwei");
    assert!(ov.force_approval.is_none(), "auto = 不强审批");

    // confirm → 强审批
    let t2 = ZiweiDynamicTool {
        profile_id: "p".into(),
        profile_name: "n".into(),
        tool: ToolDef {
            run_policy: RunPolicy::Confirm,
            ..tool
        },
    };
    let ov2 = t2.operation_override("{}").unwrap();
    assert!(ov2.force_approval.is_some(), "confirm = 强审批卡");
}

#[test]
fn unwired_primitives_rejected_without_session() {
    // call_fn/hook_capture 已 M4 实装；无 manager 时仍先报「未装配」（诚实拒绝面不变）。
    let t = ZiweiDynamicTool {
        profile_id: "p".into(),
        profile_name: "n".into(),
        tool: ToolDef {
            tool_name: "fn1".into(),
            description: "d".into(),
            input_schema: serde_json::Value::Null,
            action: nemesis_ziwei::profile::ToolAction {
                primitive: Primitive::CallFn,
                addr: "fn:do_thing".into(),
                mode: None,
                interval_secs: None,
                max_records: None,
                timeout_ms: None,
                notify: None,
            },
            output: None,
            run_policy: RunPolicy::Confirm,
        },
    };
    // 未装配 manager 时 execute 报「未装配」或「未实装」——两者都是诚实拒绝。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ctx = RequestContext {
        channel: "test".into(),
        chat_id: "c".into(),
        user: "u".into(),
        session_key: "s".into(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
    };
    let out = rt.block_on(t.execute("{}", &ctx));
    let msg = out.unwrap_err();
    assert!(
        msg.contains("M4") || msg.contains("未装配"),
        "诚实拒绝面，got: {msg}"
    );
}

/// M2 实弹：动态工具经 agent Tool trait 调用真实会话（#[ignore]——
/// `cargo test -p nemesisbot --features ziwei ziwei_tools -- --ignored`
/// 显式触发；依赖 payload 内嵌 + dummy-monitor 现场构建）。
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_dynamic_tool_invoke() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-tools/dummy-targets");
    let exe = root.join("bin/x64/dummy-monitor.exe");
    let exe = if exe.is_file() {
        exe
    } else {
        let st = std::process::Command::new("cargo")
            .args(["build", "--release"])
            .arg("--manifest-path")
            .arg(root.join("dummy-monitor/Cargo.toml"))
            .status()
            .expect("cargo");
        if !(st.success() && exe.is_file()) {
            eprintln!("SKIP: dummy-monitor 构建失败");
            return;
        }
        exe
    };
    use std::io::BufRead;
    let mut child = std::process::Command::new(&exe)
        .arg("--interval-ms=400")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let (mut pid, mut addr) = (0u32, 0u64);
    {
        let stdout = child.stdout.take().unwrap();
        let mut r = std::io::BufReader::new(stdout);
        let mut line = String::new();
        while pid == 0 || addr == 0 {
            line.clear();
            match r.read_line(&mut line) {
                Ok(0) => panic!("dummy 提前退出"),
                Ok(_) => {
                    if let Some(v) = line.trim().strip_prefix("PID=") {
                        pid = v.parse().unwrap();
                    }
                    if let Some(v) = line.trim().strip_prefix("STATE_ADDR=") {
                        addr = u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap();
                    }
                }
                Err(e) => panic!("read stdout: {e}"),
            }
        }
        std::thread::spawn(move || {
            let mut rest = String::new();
            loop {
                rest.clear();
                match r.read_line(&mut rest) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
    }

    // 隔离 workspace + 全局槽（动态工具经 global() 取管理器）。
    let ws = std::env::temp_dir().join(format!("zw-tool-it-{}", std::process::id()));
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let mgr = std::sync::Arc::new(nemesis_ziwei::session::SessionManager::new(&ws, None, 2));
    assert!(
        nemesis_ziwei::session::set_global(mgr.clone()),
        "全局槽装配"
    );

    // profile 入库（store 布局 = workspace/ziwei/profiles/<id>/profile.json）。
    let profile: nemesis_ziwei::profile::Profile = serde_json::from_str(
        r#"{
        "schema_version": 1,
        "profile_id": "zw-pro-tool",
        "name": "动态工具集成",
        "created_at": "2026-10-22T00:00:00Z",
        "last_verified_at": "2026-10-22T00:00:00Z",
        "status": "active",
        "target": {"exe_name": "dummy-monitor.exe", "machine": "x64", "verify_policy": "hash_or_version"},
        "attach": {"mode": "attach"},
        "verification": {"on_attach": []},
        "data_structures": [
            {"name": "MonitorState", "size": 80, "fields": [
                {"offset": 0, "name": "magic", "type": "u32", "decode": "hex"},
                {"offset": 4, "name": "status", "type": "u32"},
                {"offset": 8, "name": "tick", "type": "u64"},
                {"offset": 16, "name": "message", "type": "wchar[32]", "decode": "utf16"}
            ]}
        ],
        "tools": [
            {"tool_name": "mon_state", "description": "读状态", "input_schema": null,
             "action": {"primitive": "poll_report", "addr": "struct:MonitorState", "mode": "on_change", "interval_secs": 1},
             "run_policy": "auto"}
        ]
    }"#,
    )
    .unwrap();
    nemesis_ziwei::profile::ProfileStore::new(&ws)
        .save(&profile, None)
        .expect("profile 入库");

    // attach + 调度（动态工具不依赖调度，但走同源装配）。
    let session = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        nemesis_ziwei::scheduler::attach_and_schedule(
            &mgr,
            &profile,
            pid,
            [("MonitorState".to_string(), addr)].into_iter().collect(),
        ),
    )
    .await
    .expect("attach 超时")
    .expect("attach 失败");
    let _ = session;

    // 注册 → agent Tool trait 调用（LLM 不在场，直接 execute）。
    let ctx = RequestContext {
        channel: "it".into(),
        chat_id: "c".into(),
        user: "u".into(),
        session_key: "s".into(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
    };
    let tool = ZiweiDynamicTool {
        profile_id: "zw-pro-tool".into(),
        profile_name: "动态工具集成".into(),
        tool: profile.tools[0].clone(),
    };
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), tool.execute("{}", &ctx))
        .await
        .expect("execute 超时")
        .expect("execute 失败");
    let v: serde_json::Value = serde_json::from_str(&out).expect("JSON 输出");
    assert_eq!(v["tool"], "mon_state");
    assert_eq!(v["profile"], "zw-pro-tool");
    assert_eq!(v["struct"], "MonitorState");
    assert_eq!(v["data"]["magic"], "0x4d4f4e31");
    println!("M2 动态工具 invoke：{out}");

    // 工具名 sanitize 面。
    assert_eq!(
        dynamic_tool_name("zw-pro-tool", "mon_state"),
        "zw_zw_pro_tool_mon_state"
    );

    mgr.detach("zw-pro-tool").await.expect("detach");
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

// ── ziwei_bridge 元工具（T3.8）─────────────────────────────────────────

#[test]
fn bridge_approval_gate_mapping() {
    assert!(ZiweiBridgeTool::approval_gate("select_target").is_some());
    assert!(ZiweiBridgeTool::approval_gate("commit").is_some());
    assert!(ZiweiBridgeTool::approval_gate("abort").is_some());
    // 读/标注面动作不强制审批（8 层管线照常）
    for a in [
        "status",
        "snapshot",
        "analyze",
        "candidates",
        "annotate",
        "define_struct",
        "define_tool",
        "diff_window",
    ] {
        assert!(
            ZiweiBridgeTool::approval_gate(a).is_none(),
            "{a} 不应强审批"
        );
    }
    assert!(ZiweiBridgeTool::approval_gate("unknown").is_none());
}

#[test]
fn bridge_projection_and_schema() {
    let t = ZiweiBridgeTool {
        workspace: std::env::temp_dir(),
    };
    assert_eq!(t.min_tier(), Some("normal"));
    assert!(!t.is_read_only(), "桥含写人生动作");
    let ov = t
        .operation_override(r#"{"action":"commit"}"#)
        .expect("投影必有");
    assert_eq!(ov.op_type, "ziwei");
    assert!(ov.force_approval.is_some(), "commit 强审批");
    let ov2 = t.operation_override(r#"{"action":"candidates"}"#).unwrap();
    assert!(ov2.force_approval.is_none());
    // schema：action 必填 + enum 完整
    let schema = t.parameters();
    assert_eq!(schema["required"], serde_json::json!(["action"]));
    assert_eq!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .map(|a| a.len()),
        Some(11)
    );
}

/// 桥文件面全流程（单测串行：global() 单例——状态/草稿跨用例互斥）。
/// 纯文件步骤：seed draft → annotate → define_struct → define_tool →
/// commit（ProfileStore 回读）→ status 恢复空。
#[tokio::test]
async fn bridge_file_plane_flow() {
    let rt = tokio::runtime::Handle::current();
    let _guard = rt.enter(); // execute 内部无 await 依赖 runtime 的调用面
    let ws = std::env::temp_dir().join(format!(
        "zwbridge-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&ws).unwrap();
    let ctx = RequestContext {
        channel: "test".into(),
        chat_id: "c".into(),
        user: "u".into(),
        session_key: "s".into(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
    };
    let tool = ZiweiBridgeTool {
        workspace: ws.clone(),
    };

    // 1) 无向导 → status 诚实拒绝
    let out = tool
        .execute(r#"{"action":"status"}"#, &ctx)
        .await
        .unwrap_err();
    assert!(out.contains("无活跃向导"), "实际：{out}");

    // 2) seed draft（draft.json + anchors.json 手工落盘——select_target
    //    的产物形态；文件面步骤经 require_info 重建可用）
    let id = format!("zw-pro-d{}", chrono::Utc::now().timestamp_millis());
    let dir = ws.join("ziwei").join("drafts").join(&id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("draft.json"),
        serde_json::json!({
            "schema_version": 1,
            "profile_id": id,
            "name": "draft: bridge.exe",
            "created_at": "2026-10-23T00:00:00Z",
            "last_verified_at": "2026-10-23T00:00:00Z",
            "status": "active",
            "target": {
                "exe_name": "bridge.exe",
                "machine": "x64",
                "file_sha256": format!("{:064x}", 7u32),
                "verify_policy": "hash_or_version",
            },
            "verification": {"on_attach": ["file_sha256"]},
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("anchors.json"),
        serde_json::json!({"profile_id": id, "anchors": []}).to_string(),
    )
    .unwrap();

    // 3) annotate（强制锚：无锚拒绝 → 带锚通过）
    let err = tool
        .execute(
            r#"{"action":"annotate","params":{"rva":4096,"semantic":"fn_a","anchors":[]}}"#,
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("≥1"), "无锚拒绝，实际：{err}");
    let out = tool
        .execute(
            r#"{"action":"annotate","params":{"rva":4096,"semantic":"fn_a","anchors":[{"kind":"imm_const","value":305419896}]}}"#,
            &ctx,
        )
        .await
        .expect("annotate");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["annotated"], "fn_a");

    // 4) define_struct + define_tool
    let out = tool
        .execute(
            r#"{"action":"define_struct","params":{"name":"Hdr","size":8,"fields":[{"offset":0,"name":"magic","type":"u32"}]}}"#,
            &ctx,
        )
        .await
        .expect("define_struct");
    assert!(out.contains("Hdr"));
    let out = tool
        .execute(
            r#"{"action":"define_tool","params":{"tool_name":"bridge_read","description":"读","action":{"primitive":"poll_report","addr":"struct:Hdr"}}}"#,
            &ctx,
        )
        .await
        .expect("define_tool");
    assert!(out.contains("tools"));

    // 5) commit（tools_registered 字段 = 桥路径特有）
    let out = tool
        .execute(r#"{"action":"commit"}"#, &ctx)
        .await
        .expect("commit");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["committed"], id);
    assert!(v.get("tools_registered").is_some(), "桥 commit 报注册数");

    // 6) commit 后向导清空；profile 可回读
    assert!(tool.execute(r#"{"action":"status"}"#, &ctx).await.is_err());
    let store = nemesis_ziwei::profile::ProfileStore::new(&ws);
    assert!(store.load(&id).is_ok(), "profile 入库可回读");

    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}
