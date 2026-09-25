//! Tool execution chain integration tests (Phase 3).
//!
//! 2026-09-25 base_url 专项重写：此前套件跑在 testai-1.1（固定回复、从不
//! 发起工具调用）上，「Tool flow completed」实际只是「模型答了一句话」，
//! 断言三层软（Ok 恒过 / 副作用缺失也算过 / Err 也算过）。现改为
//! **testai-5.0 FILE_OP 驱动**：`<FILE_OP>{...}</FILE_OP>` 用户消息使模型
//! 确定性发起对应文件工具调用，断言以 `tool_event` 事件流（ToolFinished
//! 的 tool/ok/result_preview）+ 落盘效果为准。
//!
//! 诚实边界：sleep / message 工具在 TestAI 确定性模型里没有驱动器
//! （testai-5.0 只映射 7 个文件操作），相应套件移除——两工具的行为覆盖
//! 仍在 crate 单测里；edit_file 不在 testai-5.0 操作面内，由同族
//! append_file（变更类）套件顶上。

use test_harness::*;

use crate::wsapi;

/// 期望的工具成功事件断言：events 里必须存在 ToolFinished{tool, ok=true}。
fn assert_tool_ok(suite: &str, name: &str, events: &[wsapi::ToolEvent], tool: &str) -> TestResult {
    match events
        .iter()
        .find(|e| e.kind == "ToolFinished" && e.tool == tool)
    {
        Some(ev) if ev.ok => pass(
            &format!("{suite}/{name}"),
            format!("{tool} ToolFinished ok=true"),
        ),
        Some(ev) => fail(
            &format!("{suite}/{name}"),
            format!("{tool} ok=false: {}", ev.result_preview),
        ),
        None => fail(
            &format!("{suite}/{name}"),
            format!(
                "无 {tool} ToolFinished 事件（共 {} 个事件，模型可能未发起工具调用）",
                events.len()
            ),
        ),
    }
}

/// 热切 testai-5.0 → 发一轮 FILE_OP 聊天 → 恢复 testai-1.1 默认
/// （改共享状态的套件自恢复，下游套件不受残留默认模型影响）。
async fn file_op_round(
    payload: &str,
    timeout_secs: u64,
) -> Result<(String, Vec<wsapi::ToolEvent>), anyhow::Error> {
    let mut api = wsapi::WsApi::connect().await?;
    let switch = api.set_default_model("testai-5.0").await;
    let out = match switch {
        Ok(()) => wsapi::chat_round_collect_tools(payload, timeout_secs).await,
        Err(e) => Err(e),
    };
    let _ = api.set_default_model("testai-1.1").await;
    out
}

// ---------------------------------------------------------------------------
// Test: read_file tool
// ---------------------------------------------------------------------------

pub async fn test_tool_read_file(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/read_file";
    let mut results = Vec::new();
    print_suite_header(suite);

    let test_file = ws.workspace().join("test_read.txt");
    std::fs::write(&test_file, "Hello from read_file test!").unwrap();

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_read","path":"test_read.txt"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((reply, events)) => {
            results.push(assert_tool_ok(suite, "executed", &events, "read_file"));
            // 工具结果（文件内容）经 result_preview 可见 = 读到的是真内容
            match events
                .iter()
                .find(|e| e.kind == "ToolFinished" && e.tool == "read_file")
            {
                Some(ev) if ev.ok && ev.result_preview.contains("Hello from read_file test!") => {
                    results.push(pass(
                        &format!("{suite}/content"),
                        "read_file 结果含预期文件内容",
                    ));
                }
                Some(ev) => results.push(fail(
                    &format!("{suite}/content"),
                    format!("result_preview 无预期内容: {}", ev.result_preview),
                )),
                None => results.push(fail(
                    &format!("{suite}/content"),
                    "无 read_file 事件，内容断言不可达",
                )),
            }
            if reply.is_empty() {
                results.push(fail(&format!("{suite}/reply"), "回复为空"));
            } else {
                results.push(pass(&format!("{suite}/reply"), "轮次回复收尾"));
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/executed"),
            format!("Round failed: {e}"),
        )),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: write_file tool
// ---------------------------------------------------------------------------

pub async fn test_tool_write_file(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/write_file";
    let mut results = Vec::new();
    print_suite_header(suite);

    let written_file = ws.workspace().join("test_write.txt");
    let _ = std::fs::remove_file(&written_file);

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_write","path":"test_write.txt","content":"IT write_file 落盘验证 42"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((reply, events)) => {
            results.push(assert_tool_ok(suite, "executed", &events, "write_file"));
            match std::fs::read_to_string(&written_file) {
                Ok(c) if c.contains("IT write_file 落盘验证 42") => {
                    results.push(pass(
                        &format!("{suite}/file_content"),
                        "文件落盘且内容一致",
                    ));
                }
                Ok(c) => results.push(fail(
                    &format!("{suite}/file_content"),
                    format!("内容不符: {c}"),
                )),
                Err(e) => results.push(fail(
                    &format!("{suite}/file_content"),
                    format!("文件未落盘: {e}"),
                )),
            }
            if reply.is_empty() {
                results.push(fail(&format!("{suite}/reply"), "回复为空"));
            } else {
                results.push(pass(&format!("{suite}/reply"), "轮次回复收尾"));
            }
        }
        Err(e) => results.push(fail(&format!("{suite}/executed"), format!("Round failed: {e}"))),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: append_file tool（testai-5.0 操作面无 edit_file；同族变更类操作
// 由 append_file 顶上——edit_file 行为覆盖在 crate 单测）
// ---------------------------------------------------------------------------

pub async fn test_tool_append_file(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/append_file";
    let mut results = Vec::new();
    print_suite_header(suite);

    let test_file = ws.workspace().join("test_append.txt");
    std::fs::write(&test_file, "line1\n").unwrap();

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_append","path":"test_append.txt","content":"IT append 2"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "executed", &events, "append_file"));
            match std::fs::read_to_string(&test_file) {
                Ok(c) if c.contains("line1") && c.contains("IT append 2") => {
                    results.push(pass(
                        &format!("{suite}/file_content"),
                        "原内容保留 + 追加内容落盘",
                    ));
                }
                Ok(c) => results.push(fail(
                    &format!("{suite}/file_content"),
                    format!("追加结果不符: {c}"),
                )),
                Err(e) => {
                    results.push(fail(&format!("{suite}/file_content"), format!("读取失败: {e}")))
                }
            }
        }
        Err(e) => results.push(fail(&format!("{suite}/executed"), format!("Round failed: {e}"))),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: list_dir tool
// ---------------------------------------------------------------------------

pub async fn test_tool_list_dir(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/list_dir";
    let mut results = Vec::new();
    print_suite_header(suite);

    std::fs::create_dir_all(ws.workspace().join("testdir")).unwrap();
    std::fs::write(ws.workspace().join("testdir/a.txt"), "a").unwrap();
    std::fs::write(ws.workspace().join("testdir/b.txt"), "b").unwrap();

    match file_op_round(
        r#"<FILE_OP>{"operation":"dir_list","path":"testdir"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "executed", &events, "list_dir"));
            match events
                .iter()
                .find(|e| e.kind == "ToolFinished" && e.tool == "list_dir")
            {
                Some(ev) if ev.ok && ev.result_preview.contains("a.txt") => {
                    results.push(pass(
                        &format!("{suite}/content"),
                        "列目录结果含预期条目 a.txt",
                    ));
                }
                Some(ev) => results.push(fail(
                    &format!("{suite}/content"),
                    format!("result_preview 无 a.txt: {}", ev.result_preview),
                )),
                None => results.push(fail(
                    &format!("{suite}/content"),
                    "无 list_dir 事件，内容断言不可达",
                )),
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/executed"),
            format!("Round failed: {e}"),
        )),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: create_dir + delete_dir（两轮独立会话：testai-5.0 单消息单 FILE_OP）
// ---------------------------------------------------------------------------

pub async fn test_tool_create_delete_dir(_ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/create_delete_dir";
    let mut results = Vec::new();
    print_suite_header(suite);

    let dir = _ws.workspace().join("temp_it_dir");
    let _ = std::fs::remove_dir_all(&dir);

    // Round 1: create
    match file_op_round(
        r#"<FILE_OP>{"operation":"dir_create","path":"temp_it_dir"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "created", &events, "create_dir"));
            if dir.is_dir() {
                results.push(pass(&format!("{suite}/dir_exists"), "目录已创建"));
            } else {
                results.push(fail(&format!("{suite}/dir_exists"), "目录未创建"));
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/created"),
            format!("Round failed: {e}"),
        )),
    }

    // Round 2: delete
    match file_op_round(
        r#"<FILE_OP>{"operation":"dir_delete","path":"temp_it_dir"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "deleted", &events, "delete_dir"));
            if !dir.exists() {
                results.push(pass(&format!("{suite}/dir_gone"), "目录已删除"));
            } else {
                results.push(fail(&format!("{suite}/dir_gone"), "目录仍存在"));
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/deleted"),
            format!("Round failed: {e}"),
        )),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: delete_file tool
// ---------------------------------------------------------------------------

pub async fn test_tool_delete_file(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/delete_file";
    let mut results = Vec::new();
    print_suite_header(suite);

    let del_file = ws.workspace().join("to_delete.txt");
    std::fs::write(&del_file, "delete me").unwrap();

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_delete","path":"to_delete.txt"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "executed", &events, "delete_file"));
            if !del_file.exists() {
                results.push(pass(&format!("{suite}/file_gone"), "文件已删除"));
            } else {
                results.push(fail(&format!("{suite}/file_gone"), "文件仍存在"));
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/executed"),
            format!("Round failed: {e}"),
        )),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Multi-step tool chain（写 → 读回验；testai-5.0 单消息单 FILE_OP，
// 双步拆两轮独立会话，链路语义 = 写后读回内容一致）
// ---------------------------------------------------------------------------

pub async fn test_tool_multi_step(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/multi_step";
    let mut results = Vec::new();
    print_suite_header(suite);

    let multi_file = ws.workspace().join("multi_it.txt");
    let _ = std::fs::remove_file(&multi_file);

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_write","path":"multi_it.txt","content":"hello chain"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "write_step", &events, "write_file"));
            match std::fs::read_to_string(&multi_file) {
                Ok(c) if c.contains("hello chain") => {
                    results.push(pass(&format!("{suite}/write_effect"), "写步落盘内容一致"));
                }
                Ok(c) => results.push(fail(
                    &format!("{suite}/write_effect"),
                    format!("内容不符: {c}"),
                )),
                Err(e) => results.push(fail(
                    &format!("{suite}/write_effect"),
                    format!("文件未落盘: {e}"),
                )),
            }
        }
        Err(e) => results.push(fail(&format!("{suite}/write_step"), format!("Round failed: {e}"))),
    }

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_read","path":"multi_it.txt"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((_reply, events)) => {
            results.push(assert_tool_ok(suite, "read_step", &events, "read_file"));
            match events
                .iter()
                .find(|e| e.kind == "ToolFinished" && e.tool == "read_file")
            {
                Some(ev) if ev.ok && ev.result_preview.contains("hello chain") => {
                    results.push(pass(
                        &format!("{suite}/read_back"),
                        "读步取回写步内容（链路闭环）",
                    ));
                }
                Some(ev) => results.push(fail(
                    &format!("{suite}/read_back"),
                    format!("读回内容不符: {}", ev.result_preview),
                )),
                None => results.push(fail(
                    &format!("{suite}/read_back"),
                    "无 read_file 事件，读回断言不可达",
                )),
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/read_step"),
            format!("Round failed: {e}"),
        )),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Error recovery (read non-existent file)——工具真实执行并失败，
// 循环存活且收尾回复（ToolFinished ok=false 事件为证）
// ---------------------------------------------------------------------------

pub async fn test_tool_error_recovery() -> Vec<TestResult> {
    let suite = "tool/error_recovery";
    let mut results = Vec::new();
    print_suite_header(suite);

    match file_op_round(
        r#"<FILE_OP>{"operation":"file_read","path":"nonexistent_file_xyz.txt"}</FILE_OP>"#,
        60,
    )
    .await
    {
        Ok((reply, events)) => {
            match events
                .iter()
                .find(|e| e.kind == "ToolFinished" && e.tool == "read_file")
            {
                Some(ev) if !ev.ok => {
                    results.push(pass(
                        &format!("{suite}/tool_failed"),
                        format!("read_file 失败如实上报: {}", ev.result_preview),
                    ));
                }
                Some(ev) => results.push(fail(
                    &format!("{suite}/tool_failed"),
                    format!("读不存在文件竟 ok=true（错误被吞）: {}", ev.result_preview),
                )),
                None => results.push(fail(
                    &format!("{suite}/tool_failed"),
                    "无 read_file 事件（模型未发起调用，错误路径未被触达）",
                )),
            }
            if reply.is_empty() {
                results.push(fail(&format!("{suite}/recovered"), "工具失败后回复为空"));
            } else {
                results.push(pass(
                    &format!("{suite}/recovered"),
                    "工具失败后循环存活并收尾回复",
                ));
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/tool_failed"),
            format!("Round failed: {e}"),
        )),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Workspace restriction——出界写**效果面**断言（bench 同款）：
// restrict_to_workspace=true 围栏下，testai-5.0 经 FILE_OP 对工作区外
// 绝对路径发起 file_write，轮次结束后目标文件不存在 = 拦截成立。
// 诚实边界：安全层拦停的调用不产生 tool_event（around 链在安全闸之后），
// 故此处不断言事件，只断言磁盘效果。
// ---------------------------------------------------------------------------

pub async fn test_tool_workspace_restriction(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool/workspace_restriction";
    let mut results = Vec::new();
    print_suite_header(suite);

    let _ = ws; // 围栏根 = agent workspace；目标在工作区外
    let target = std::env::temp_dir().join("it_boundary_pwned.txt");
    let _ = std::fs::remove_file(&target);
    let payload = format!(
        r#"<FILE_OP>{{"operation":"file_write","path":"{}","content":"pwn"}}</FILE_OP>"#,
        target.to_string_lossy().replace('\\', "\\\\")
    );

    match file_op_round(&payload, 60).await {
        Ok((reply, events)) => {
            if events
                .iter()
                .any(|e| e.kind == "ToolFinished" && e.tool == "write_file" && e.ok)
            {
                results.push(fail(
                    &format!("{suite}/blocked"),
                    "出界写收到了成功事件（围栏失效）",
                ));
            }
            if target.exists() {
                results.push(fail(
                    &format!("{suite}/blocked"),
                    format!("出界写入未被拦截：{} 已落盘", target.display()),
                ));
            } else {
                results.push(pass(
                    &format!("{suite}/blocked"),
                    "出界写未落盘（围栏成立）",
                ));
            }
            if reply.is_empty() {
                results.push(fail(&format!("{suite}/reply"), "拦截后回复为空"));
            } else {
                results.push(pass(&format!("{suite}/reply"), "拦截后轮次正常收尾"));
            }
        }
        Err(e) => results.push(fail(
            &format!("{suite}/blocked"),
            format!("Round failed: {e}"),
        )),
    }

    results
}
