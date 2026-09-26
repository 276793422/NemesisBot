//! WS2（能力扩展 P5/P6/P7）compact 域测试。
//!
//! - P5：token 预算尾巴（`token_budget_boundary` + fresh-read 配置 +
//!   `maybe_update_summary` 接线 + 0=旧按条数回退 + tool 对安全）；
//! - P6：六节 schema 指令/解析/回退 + UPDATE 迭代语义（G1 前缀形状不破）；
//! - P7：文件操作台账聚合/注入/累积/上限。
//!
//! 独立测试文件（生产文件只保留声明行，仓库 2026-08-25 纪律）。本文件自建
//! 捕获型 mock provider 与 turn 构造器——`loop/tests.rs` 的同名辅助是模块
//! 私有，跨文件不可复用。

use super::*;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// 本地辅助：turn 构造器 + 捕获型 LLM provider
// ---------------------------------------------------------------------------

/// 与 `loop/tests.rs` 的 `test_config()` 同形态（那份是模块私有，此处自建）。
fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn turn(role: &str, content: &str) -> crate::types::ConversationTurn {
    crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

/// 带 tool_calls 的 assistant 轮（`arguments` 是 JSON 字符串，同线上形态）。
fn assistant_tool_call(name: &str, arguments: &str) -> crate::types::ConversationTurn {
    let mut t = turn("assistant", "");
    t.tool_calls = vec![crate::types::ToolCallInfo {
        id: format!("tc-{}", name),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }];
    t
}

/// 捕获请求消息 + 固定回复的 mock provider（测试断言请求形态用）。
struct CaptureProvider {
    captured: std::sync::Mutex<Vec<Vec<LlmMessage>>>,
    reply: String,
}

impl CaptureProvider {
    fn new(reply: &str) -> Self {
        Self {
            captured: std::sync::Mutex::new(Vec::new()),
            reply: reply.to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for CaptureProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.captured.lock().unwrap().push(messages);
        Ok(LlmResponse {
            content: self.reply.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 一次性回复 "SUMMARY" 的 provider（loop 级接线测试用）。
struct OnceProvider {
    reply: String,
}

#[async_trait]
impl LlmProvider for OnceProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: self.reply.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 六节 schema 合法回复（可带前导客套话）。
fn schema_reply() -> String {
    "## Goal\n实现登录功能\n## Constraints\n（无）\n## Progress\n完成一半\n## Decisions\n用 OAuth\n## Files\n（无）\n## Next Steps\n继续联调\n".to_string()
}

// ---------------------------------------------------------------------------
// P5：token 预算尾巴
// ---------------------------------------------------------------------------

/// 纯函数：预算内尾巴 ≤ 预算；最后一条消息超预算也无条件保留（pi 语义）。
#[test]
fn p5_token_budget_boundary_respects_budget() {
    // 每条 "x".repeat(100) → 100*2/5 = 40 tokens。
    let mk = || turn("user", &"x".repeat(100));
    let history = vec![mk(), mk(), mk()]; // 3 × 40 tok

    // 预算 100：装下两条（80 ≤ 100），第三条会超 → 边界 1。
    assert_eq!(token_budget_boundary(&history, 100), 1);
    // 预算 70：只装得下最后一条（第二条会到 80 > 70）→ 边界 2。
    assert_eq!(token_budget_boundary(&history, 70), 2);
    // 预算 30：最后一条（40）单独就超预算，仍无条件保留 → 边界 2（异常豁免）。
    assert_eq!(token_budget_boundary(&history, 30), 2);
    // 预算大到吞下全部 → 边界 0（无可摘前缀，不触发）。
    assert_eq!(token_budget_boundary(&history, 100_000), 0);
}

/// 纯函数：预算边界落在 tool 结果上时 tool_safe_boundary 退回父 assistant。
#[test]
fn p5_boundary_backs_off_tool_pair() {
    let mut a_tc = turn("assistant", &"x".repeat(100)); // 40 tok
    a_tc.tool_calls = vec![crate::types::ToolCallInfo {
        id: "c1".to_string(),
        name: "t".to_string(),
        arguments: "{}".to_string(),
    }];
    let history = vec![
        turn("system", "s"),            // 0（1 char → 0 tok）
        turn("user", &"x".repeat(100)), // 1（40 tok）
        a_tc,                           // 2（父 assistant，40 tok）
        turn("tool", &"x".repeat(5)),   // 3（5 chars → 2 tok）
    ];
    // 预算 2：从尾装下 tool(2)，再试父 assistant(40) 会超 → raw=3 恰好落在
    // tool 结果上 → tool_safe 退到父 assistant（2）。
    assert_eq!(token_budget_boundary(&history, 2), 3);
    assert_eq!(tool_safe_boundary(&history, 3), 2);
    // 尾巴不再以孤儿 tool 结果开头。
    assert_ne!(history[tool_safe_boundary(&history, 3)].role, "tool");
}

/// fresh-read：无 config_path → 缺省 20000；有 config → 读盘值（运行中改键
/// 下一轮生效的语义锚点）。
#[test]
fn p5_fresh_read_config_default_and_override() {
    let agent_loop = AgentLoop::new(Box::new(OnceProvider { reply: "S".into() }), test_config());
    assert_eq!(
        agent_loop.current_compact_keep_recent_tokens(),
        20_000,
        "standalone（无 config_path）→ 缺省 20000"
    );

    let cfg_path = std::env::temp_dir().join(format!(
        "nemesis_test_ws2_fresh_{}.json",
        std::process::id()
    ));
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {"compact_keep_recent_tokens": 1234}}})
            .to_string(),
    )
    .unwrap();
    let agent_loop = AgentLoop::new(Box::new(OnceProvider { reply: "S".into() }), test_config());
    agent_loop.set_config_path(cfg_path.clone());
    assert_eq!(
        agent_loop.current_compact_keep_recent_tokens(),
        1234,
        "fresh-read config.json（唯一真相源）"
    );
    // 改盘即生效（无缓存/无重启）。
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {"compact_keep_recent_tokens": 0}}}).to_string(),
    )
    .unwrap();
    assert_eq!(agent_loop.current_compact_keep_recent_tokens(), 0);
    let _ = std::fs::remove_file(&cfg_path);
}

/// `compact_keep_recent_tokens = 0` → 旧按条数回退（covers = len - K_TARGET）。
#[tokio::test]
async fn p5_zero_budget_falls_back_to_count_semantics() {
    let cfg_path =
        std::env::temp_dir().join(format!("nemesis_test_ws2_zero_{}.json", std::process::id()));
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {"compact_keep_recent_tokens": 0}}}).to_string(),
    )
    .unwrap();

    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(OnceProvider {
            reply: "SUMMARY".into(),
        }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    for i in 0..6 {
        instance.add_user_message(&format!("user msg {} with padding to add tokens", i));
        instance.add_assistant_message(
            &format!("assistant reply {} with padding", i),
            Vec::new(),
            None,
        );
    }
    // history = [sys, 6u, 6a] = 13；旧语义边界 = 13 - 6 = 7。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;

    let cache = instance.get_summary_cache().expect("cache should be set");
    assert_eq!(
        cache.covers_up_to,
        instance.get_history().len() - K_TARGET,
        "0 = 显式回退旧按条数（K_TARGET）路径"
    );
    assert_eq!(cache.text, "SUMMARY");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 默认路径：预算尾巴决定 covers；尾巴不以 tool 结果开头。
#[tokio::test]
async fn p5_maybe_update_summary_token_budget_tail() {
    let cfg_path = std::env::temp_dir().join(format!(
        "nemesis_test_ws2_budget_{}.json",
        std::process::id()
    ));
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {"compact_keep_recent_tokens": 60}}}).to_string(),
    )
    .unwrap();

    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(OnceProvider {
            reply: "SUMMARY".into(),
        }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    // 每条 "x".repeat(125) → 50 tokens。预算 60 只装最后 1 条（第二条 100 > 60）
    // → raw = 8（history = [sys + 4u + 4a]），covers 8。
    for _ in 0..4 {
        instance.add_user_message(&"x".repeat(125));
        instance.add_assistant_message(&"x".repeat(125), Vec::new(), None);
    }
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;

    let cache = instance.get_summary_cache().expect("cache should be set");
    let history = instance.get_history();
    assert_eq!(
        cache.covers_up_to,
        history.len() - 1,
        "预算 60 < 两条 50tok：尾巴只保留最后一条"
    );
    assert_ne!(history[cache.covers_up_to].role, "tool", "无孤儿 tool 结果");
    assert_eq!(cache.text, "SUMMARY");
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// P6：结构化 schema + UPDATE 迭代
// ---------------------------------------------------------------------------

/// schema 解析：六节齐 → Some（剥前导客套）；缺一节 / 无标题 → None。
#[test]
fn p6_parse_structured_summary_valid_and_fallback() {
    let reply = format!("好的，以下是摘要：\n{}", schema_reply());
    let parsed = parse_structured_summary(&reply).expect("六节齐 → Some");
    assert!(parsed.starts_with("## Goal"), "从首个 schema 标题截起");
    assert!(!parsed.contains("好的"), "前导客套剥离");
    assert!(parsed.ends_with("继续联调"));

    // 缺一节 → None（调用方回退自由文本）。
    let missing = "## Goal\ng\n## Constraints\nc\n## Progress\np\n## Decisions\nd\n## Files\nf\n";
    assert_eq!(parse_structured_summary(missing), None);
    // 纯自由文本 → None。
    assert_eq!(parse_structured_summary("plain text"), None);
}

/// 指令构建：fresh vs UPDATE（旧摘要 + 新增条数嵌入），台账块注入。
#[test]
fn p6_build_summary_instruction_fresh_vs_update() {
    let empty: Vec<FileOp> = Vec::new();

    let fresh = build_summary_instruction(&SummaryUpdate {
        existing: "",
        new_segment_turns: 0,
        ledger: &empty,
    });
    assert!(fresh.contains("简明摘要"));
    for s in SUMMARY_SCHEMA_SECTIONS {
        assert!(fresh.contains(&format!("## {s}")), "schema 节 {s} 必须列出");
    }
    assert!(!fresh.contains("上一版摘要"), "fresh 无旧摘要块");
    assert!(!fresh.contains("UPDATE"));

    let update = build_summary_instruction(&SummaryUpdate {
        existing: "OLD SUMMARY",
        new_segment_turns: 7,
        ledger: &empty,
    });
    assert!(update.contains("UPDATE"), "迭代语义标记");
    assert!(update.contains("约 7 条"), "新增段条数告知模型");
    assert!(update.contains("OLD SUMMARY"), "旧摘要原文嵌入");
    assert!(update.contains("简明摘要"), "UPDATE 同样要求简明");

    let with_ledger = build_summary_instruction(&SummaryUpdate {
        existing: "OLD",
        new_segment_turns: 1,
        ledger: &[FileOp {
            path: "/a.rs".to_string(),
            kind: "write",
        }],
    });
    assert!(with_ledger.contains(FILE_LEDGER_HEADING));
    assert!(with_ledger.contains("- [write] /a.rs"));
}

/// UPDATE 走线：existing 非空 → 尾部指令携带旧摘要 + 新增条数；schema 回复
/// 被原样采信。G1 前缀形状不破：覆盖段消息仍是原样结构（指令是最后一条）。
#[tokio::test]
async fn p6_iterative_update_carries_existing_summary() {
    let provider = CaptureProvider::new(&schema_reply());
    let turns = vec![
        turn("system", "SYS"),
        turn("user", "older question"),
        turn("assistant", "older answer"),
        turn("user", "new question"),
        turn("assistant", "new answer"),
    ];
    let refs: Vec<&crate::types::ConversationTurn> = turns.iter().collect();

    let out = summarize_prefix_owned(&refs, "OLD SUMMARY", 2, 32_000, true, &provider, "m", None)
        .await
        .expect("schema 回复必须被采信");
    assert!(out.contains("## Next Steps"));

    let captured = provider.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let msgs = &captured[0];
    // G1 形状：system + 原样覆盖段 + 尾部指令（5 轮）。
    assert_eq!(msgs.len(), 6);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[0].content, "SYS");
    assert_eq!(msgs[1].content, "older question", "覆盖段原样字节");
    // 指令在最后一条：UPDATE 语义 + 旧摘要 + 新增条数（指令不在 warm 前缀，
    // 改文本不破前缀字节不变纪律）。
    let ins = &msgs.last().unwrap().content;
    assert_eq!(msgs.last().unwrap().role, "user");
    assert!(ins.contains("UPDATE"));
    assert!(ins.contains("OLD SUMMARY"));
    assert!(ins.contains("约 2 条"));
}

/// schema 解析失败 → 自由文本原样采信（不是 None、不 panic、不丢摘要）。
#[tokio::test]
async fn p6_schema_failure_falls_back_to_free_text() {
    let provider = CaptureProvider::new("plain free-form summary");
    let turns = vec![turn("user", "q"), turn("assistant", "a")];
    let refs: Vec<&crate::types::ConversationTurn> = turns.iter().collect();

    let out = summarize_prefix_owned(&refs, "", 0, 32_000, true, &provider, "m", None)
        .await
        .expect("回退不是 None");
    assert_eq!(out, "plain free-form summary");

    // 收尾函数直测：无台账无省略 → 原样。
    assert_eq!(finalize_summary_text("free", &[], false), "free");
}

// ---------------------------------------------------------------------------
// P7：文件操作台账
// ---------------------------------------------------------------------------

/// 工具名 → kind 映射 + 忽略面 + 去重（同 path 后声明 kind 覆盖、首现顺序）。
#[test]
fn p7_ledger_tool_mappings_and_dedup() {
    let msgs = vec![
        turn("system", "s"),
        assistant_tool_call("write_file", r#"{"path":"/a.rs"}"#),
        assistant_tool_call("edit_file", r#"{"path":"/b.rs"}"#),
        assistant_tool_call("append_file", r#"{"path":"/c.log"}"#),
        assistant_tool_call("delete_file", r#"{"path":"/d.tmp"}"#),
        assistant_tool_call(
            "multiedit",
            r#"{"edits":[{"path":"/e1.rs","old_text":"x","new_text":"y"},{"path":"/e2.rs"}]}"#,
        ),
        // 忽略面：读工具 / 非文件工具 / 空路径 / 坏 JSON / 非 assistant 轮。
        assistant_tool_call("read_file", r#"{"path":"/f.rs"}"#),
        assistant_tool_call("exec", r#"{"command":"rm -rf /"}"#),
        assistant_tool_call("write_file", r#"{"path":""}"#),
        assistant_tool_call("write_file", "not-json"),
        {
            let mut u = turn("user", "hi");
            u.tool_calls = vec![crate::types::ToolCallInfo {
                id: "tc-user".to_string(),
                name: "write_file".to_string(),
                arguments: r#"{"path":"/should-not-count"}"#.to_string(),
            }];
            u
        },
    ];
    let refs: Vec<&crate::types::ConversationTurn> = msgs.iter().collect();
    let ledger = collect_file_ops_ledger(&refs);
    assert_eq!(
        ledger,
        vec![
            FileOp {
                path: "/a.rs".to_string(),
                kind: "write"
            },
            FileOp {
                path: "/b.rs".to_string(),
                kind: "edit"
            },
            FileOp {
                path: "/c.log".to_string(),
                kind: "edit"
            },
            FileOp {
                path: "/d.tmp".to_string(),
                kind: "delete"
            },
            FileOp {
                path: "/e1.rs".to_string(),
                kind: "edit"
            },
            FileOp {
                path: "/e2.rs".to_string(),
                kind: "edit"
            },
        ]
    );

    // 去重：同 path 后声明 kind 覆盖，首次出现顺序保留。
    let dedup = vec![
        assistant_tool_call("write_file", r#"{"path":"/x"}"#),
        assistant_tool_call("edit_file", r#"{"path":"/x"}"#),
        assistant_tool_call("edit_file", r#"{"path":"/y"}"#),
        assistant_tool_call("write_file", r#"{"path":"/y"}"#),
    ];
    let drefs: Vec<&crate::types::ConversationTurn> = dedup.iter().collect();
    assert_eq!(
        collect_file_ops_ledger(&drefs),
        vec![
            FileOp {
                path: "/x".to_string(),
                kind: "edit"
            },
            FileOp {
                path: "/y".to_string(),
                kind: "write"
            },
        ]
    );
}

/// 压缩产物含台账节：模型回复没写 Files 细目时，宿主确定性追加
/// （不依赖模型自觉）。
#[tokio::test]
async fn p7_summary_contains_ledger_section() {
    let provider = CaptureProvider::new("plain summary");
    let msgs = vec![
        turn("system", "SYS"),
        turn("user", "fix the bug"),
        assistant_tool_call("write_file", r#"{"path":"/a.rs"}"#),
        turn("user", "thanks"),
    ];
    let refs: Vec<&crate::types::ConversationTurn> = msgs.iter().collect();

    let out = summarize_prefix_owned(&refs, "", 0, 32_000, true, &provider, "m", None)
        .await
        .expect("summary produced");
    assert!(out.contains(FILE_LEDGER_HEADING), "宿主追加台账节：{out}");
    assert!(out.contains("- [write] /a.rs"));

    // 指令里同样有台账块（模型侧引导 + 宿主兜底双保险）。
    let captured = provider.captured.lock().unwrap();
    let ins = &captured[0].last().unwrap().content;
    assert!(ins.contains(FILE_LEDGER_HEADING));
}

/// 台账累积：前缀全量重算——旧摘要已覆盖段的操作随迭代保留在新台账里。
#[tokio::test]
async fn p7_ledger_accumulates_across_iterations() {
    let provider = CaptureProvider::new("S");
    let old_op = assistant_tool_call("write_file", r#"{"path":"/old.rs"}"#);
    let new_op = assistant_tool_call("write_file", r#"{"path":"/new.rs"}"#);

    // 第一次压缩：只覆盖旧段。
    let first = vec![turn("system", "SYS"), old_op.clone(), turn("user", "u1")];
    let frefs: Vec<&crate::types::ConversationTurn> = first.iter().collect();
    let out1 = summarize_prefix_owned(&frefs, "", 0, 32_000, true, &provider, "m", None)
        .await
        .unwrap();
    assert!(out1.contains("/old.rs"));
    assert!(!out1.contains("/new.rs"));

    // 第二次压缩（UPDATE）：前缀 = 旧段 + 新段，台账同时含两段操作。
    let second = vec![
        turn("system", "SYS"),
        old_op,
        turn("user", "u1"),
        new_op,
        turn("user", "u2"),
    ];
    let srefs: Vec<&crate::types::ConversationTurn> = second.iter().collect();
    let out2 = summarize_prefix_owned(&srefs, "OLD SUMMARY", 2, 32_000, true, &provider, "m", None)
        .await
        .unwrap();
    assert!(
        out2.contains("/old.rs") && out2.contains("/new.rs"),
        "台账随迭代单调累积（旧段操作不丢）：{out2}"
    );
}

/// 台账节渲染：空 → None；超上限截断 + 诚实注记；摘要已含标题不重复追加。
#[test]
fn p7_ledger_section_cap_and_no_duplicate() {
    assert_eq!(format_file_ledger_section(&[]), None);

    let ledger: Vec<FileOp> = (0..55)
        .map(|i| FileOp {
            path: format!("/f{i}.rs"),
            kind: "write",
        })
        .collect();
    let section = format_file_ledger_section(&ledger).unwrap();
    assert!(section.contains("- [write] /f49.rs"));
    assert!(!section.contains("/f50.rs"), "超上限截断");
    assert!(section.contains("（另有 5 个文件操作未逐一列出）"));

    // finalize：摘要已含台账标题 → 不二次追加。
    let reply_with_ledger = format!("{}\n- [write] /a.rs", FILE_LEDGER_HEADING);
    let done = finalize_summary_text(&reply_with_ledger, &ledger, false);
    assert_eq!(done.matches(FILE_LEDGER_HEADING).count(), 1);
}
