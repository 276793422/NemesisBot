//! S9 覆盖率批次（Batch D：loop_tools.rs 文件工具错误臂 + cron + workflow
//! + web_fetch 本地环回）。
//!
//! 覆盖目标（基线缺口 → 测试）：
//! - 141/149 MessageTool：context channel/chat 为空 → stored 回退臂。
//! - 209/238-240 ReadFileTool：读目录失败臂 + is_read_only。
//! - 277/282/296 WriteFileTool：parent 是文件 → 建目录失败；路径是目录 →
//!   写失败；preview 的 Create 臂。
//! - 363-365 ListDirectoryTool::is_read_only。
//! - 496/514/519-526 EditFileTool：读目录失败；readonly 写失败（探针
//!   门控）；preview。
//! - 550/560/573-581 AppendFileTool：建目录失败；路径是目录 → 打开失败；
//!   preview 双臂。
//! - 614/619-625/651 DeleteFileTool：readonly 删除失败（探针门控）；
//!   不存在 / 是目录臂；preview。
//! - 684 DeleteDirTool：含 readonly 文件的目录删除失败（探针门控）+
//!   不存在 / 非目录臂。
//! - 1207/1260/1268/1321/1339 CronTool：list 序列化；channel/chat 回退；
//!   add_job / add_job_ext 的 save 失败错误臂（store_path 指向目录）。
//! - 1871/1912/1969-1985 WebFetchTool：HTTP 非 2xx；HTML 无可提取文本；
//!   非 HTML 与截断臂；truncate_str 多字节边界（本地 127.0.0.1 环回
//!   服务器，纪律 3 允许的本地假服务）。
//! - 3205-3206 SkillManageTool::do_create：SKILL.md 被目录占位 → 写失败
//!   回滚臂。
//! - 3368/3468/3525/5155/5167/5196/5131-5133：各工具无效 JSON 参数臂 +
//!   WorkflowRunTool input 三臂/description/未知工作流失败。
//! - 3490-3492 GrepTool::is_read_only。
//! - 3571/3597/3603/3617/3630 grep_recursive：预满提前返回 / glob 过滤 /
//!   >1MB 跳过 / 超长多字节行截断 / 行循环封顶（私有函数直调）。

use super::Tool;
use crate::context::RequestContext;

fn ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "chat".to_string(),
        user: "u".to_string(),
        session_key: "agent:test/s9d".to_string(),
        correlation_id: None,
        async_callback: None,
    }
}

fn empty_ctx() -> RequestContext {
    RequestContext {
        channel: String::new(),
        chat_id: String::new(),
        user: "u".to_string(),
        session_key: "agent:test/s9d".to_string(),
        correlation_id: None,
        async_callback: None,
    }
}

/// path → String（供 serde_json::json! 组装，转义自动处理）。
fn ps(p: &std::path::Path) -> String {
    p.to_string_lossy().to_string()
}

fn temp_ws(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_lt_s9d_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 本机是否执行 readonly 语义（ReFS/Dev Drive 可能不执行）。want_block=true
/// 探「删被挡」，false 探「写被挡」。探针文件独立，不影响被测文件。
fn readonly_enforced(dir: &std::path::Path, want_block: bool) -> bool {
    let probe = dir.join(format!("s9probe_{}.probe", std::process::id()));
    std::fs::write(&probe, "x").unwrap();
    let meta = std::fs::metadata(&probe).unwrap();
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&probe, perm).unwrap();
    let blocked = if want_block {
        std::fs::remove_file(&probe).is_err()
    } else {
        std::fs::write(&probe, "y").is_err()
    };
    // 探针清理：文件还在才恢复属性后删除（宽松 fs 下可能已被删掉）。
    if probe.exists() {
        if let Ok(m) = std::fs::metadata(&probe) {
            let mut p = m.permissions();
            p.set_readonly(false);
            let _ = std::fs::set_permissions(&probe, p);
        }
        let _ = std::fs::remove_file(&probe);
    }
    blocked
}

fn unset_readonly(path: &std::path::Path) {
    if path.exists() {
        if let Ok(m) = std::fs::metadata(path) {
            let mut p = m.permissions();
            p.set_readonly(false);
            let _ = std::fs::set_permissions(path, p);
        }
    }
}

// ---------- MessageTool 141/149 回退臂 ----------

#[tokio::test]
async fn message_tool_falls_back_to_stored_context_and_fires_callback() {
    let tool = super::MessageTool::new();
    tool.set_context("telegram", "chat-s9d");
    let fired = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = fired.clone();
    tool.set_send_callback(Box::new(move |ch, chat, _content| {
        sink.lock().unwrap().push(format!("{ch}|{chat}"));
    }));

    let out = tool
        .execute(r#"{"content":"hello fallback"}"#, &empty_ctx())
        .await
        .unwrap();
    assert_eq!(out, "hello fallback");
    assert_eq!(*fired.lock().unwrap(), vec!["telegram|chat-s9d".to_string()]);
    assert!(tool.has_sent_in_round());
    tool.reset_sent_in_round();
    assert!(!tool.has_sent_in_round());

    // 顺带：无 callback 注册时不 panic；非 JSON 原样返回。
    let tool2 = super::MessageTool::new();
    let out2 = tool2.execute("plain text content", &ctx()).await.unwrap();
    assert_eq!(out2, "plain text content");
}

// ---------- ReadFileTool 209/238-240 ----------

#[tokio::test]
async fn read_file_tool_directory_read_error_and_readonly_flag() {
    let ws = temp_ws("rf");
    let args = serde_json::json!({"path": ps(&ws)}).to_string();
    let out = super::ReadFileTool.execute(&args, &ctx()).await;
    assert!(out.is_err(), "reading a directory must fail");
    assert!(out.unwrap_err().contains("Failed to read file"));
    assert!(super::ReadFileTool.is_read_only());
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- WriteFileTool 277/282/296 ----------

#[tokio::test]
async fn write_file_tool_error_arms_and_create_preview() {
    let ws = temp_ws("wf");
    let blocker = ws.join("blocker_file");
    std::fs::write(&blocker, "x").unwrap();

    // 277：parent 是文件 → create_dir_all 失败。
    let args = serde_json::json!({"path": ps(&blocker.join("sub").join("new.txt")), "content": "hi"}).to_string();
    let out = super::WriteFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Failed to create directories"));

    // 282：目标路径是目录 → 写失败。
    let args = serde_json::json!({"path": ps(&ws), "content": "hi"}).to_string();
    let out = super::WriteFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Failed to write file"));

    // 296：preview 对不存在文件 → Create（需要 path+content 双字段）。
    let args = serde_json::json!({"path": ps(&ws.join("brand_new.txt")), "content": "x"}).to_string();
    assert!(super::WriteFileTool.preview(&args).is_some());
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- ListDirectoryTool 363-365 ----------

#[test]
fn list_directory_tool_readonly_flag() {
    assert!(super::ListDirectoryTool.is_read_only());
}

// ---------- EditFileTool 496/514/519-526 ----------

#[tokio::test]
async fn edit_file_tool_read_error_write_error_and_preview() {
    let ws = temp_ws("ef");

    // 496：目标是目录 → 读失败。
    let args = serde_json::json!({"path": ps(&ws), "old_text": "a", "new_text": "b"}).to_string();
    let out = super::EditFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Failed to read file"));

    // 514：readonly 文件 → 写失败（探针门控）。
    let ro = ws.join("readonly_target.md");
    std::fs::write(&ro, "alpha beta").unwrap();
    let enforced = readonly_enforced(&ws, false);
    let meta = std::fs::metadata(&ro).unwrap();
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro, perm).unwrap();
    let args = serde_json::json!({"path": ps(&ro), "old_text": "alpha", "new_text": "gamma"}).to_string();
    let out = super::EditFileTool.execute(&args, &ctx()).await;
    if enforced {
        assert!(
            out.clone().unwrap_err().contains("Failed to write file"),
            "readonly write must fail, got {:?}",
            out
        );
    } else {
        let _ = out.unwrap(); // 文件系统宽松：写入成功也接受
    }
    unset_readonly(&ro);

    // 519-526：preview（Modify）。
    let args = serde_json::json!({"path": ps(&ro), "old_text": "a", "new_text": "b"}).to_string();
    assert!(super::EditFileTool.preview(&args).is_some());
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- AppendFileTool 550/560/573-581 ----------

#[tokio::test]
async fn append_file_tool_error_arms_and_preview() {
    let ws = temp_ws("af");
    let blocker = ws.join("blocker_file");
    std::fs::write(&blocker, "x").unwrap();

    // 550：parent 是文件 → create_dir_all 失败。
    let args = serde_json::json!({"path": ps(&blocker.join("leaf.txt")), "content": "hi"}).to_string();
    let out = super::AppendFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Failed to create directories"));

    // 560：目标是目录 → 打开失败。
    let args = serde_json::json!({"path": ps(&ws), "content": "hi"}).to_string();
    let out = super::AppendFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Failed to open file"));

    // 573-581：preview Modify（已存在）与 Create（不存在）。
    let existing = ws.join("exists.txt");
    std::fs::write(&existing, "x").unwrap();
    let a1 = serde_json::json!({"path": ps(&existing), "content": "y"}).to_string();
    let a2 = serde_json::json!({"path": ps(&ws.join("fresh.txt")), "content": "y"}).to_string();
    assert!(super::AppendFileTool.preview(&a1).is_some());
    assert!(super::AppendFileTool.preview(&a2).is_some());
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- DeleteFileTool 614/619-625/651 ----------

#[tokio::test]
async fn delete_file_tool_error_arms_and_preview() {
    let ws = temp_ws("df");

    // 619：不存在。
    let args = serde_json::json!({"path": ps(&ws.join("ghost.txt"))}).to_string();
    let out = super::DeleteFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("File not found"));

    // 621：目标是目录。
    let args = serde_json::json!({"path": ps(&ws)}).to_string();
    let out = super::DeleteFileTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Path is a directory"));

    // 614：readonly → 删除失败（探针门控）。
    let ro = ws.join("readonly_delete.txt");
    std::fs::write(&ro, "x").unwrap();
    let enforced = readonly_enforced(&ws, true);
    let meta = std::fs::metadata(&ro).unwrap();
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro, perm).unwrap();
    let args = serde_json::json!({"path": ps(&ro)}).to_string();
    let out = super::DeleteFileTool.execute(&args, &ctx()).await;
    if enforced {
        assert!(
            out.clone().unwrap_err().contains("Failed to delete file"),
            "readonly delete must fail, got {:?}",
            out
        );
    } else {
        out.unwrap();
    }
    unset_readonly(&ro);

    // 651：preview。
    let args = serde_json::json!({"path": ps(&ws.join("whatever.txt"))}).to_string();
    assert!(super::DeleteFileTool.preview(&args).is_some());
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- DeleteDirTool 684 + 两个 guard 臂 ----------

#[tokio::test]
async fn delete_dir_tool_error_arms() {
    let ws = temp_ws("dd");

    // 不存在。
    let args = serde_json::json!({"path": ps(&ws.join("ghost_dir"))}).to_string();
    let out = super::DeleteDirTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Directory not found"));

    // 非目录。
    let file_path = ws.join("plain.txt");
    std::fs::write(&file_path, "x").unwrap();
    let args = serde_json::json!({"path": ps(&file_path)}).to_string();
    let out = super::DeleteDirTool.execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("Path is not a directory"));

    // 684：含 readonly 文件的目录 → remove_dir_all 失败（探针门控）。
    let victim = ws.join("victim_dir");
    std::fs::create_dir_all(&victim).unwrap();
    let inner = victim.join("ro_child.txt");
    std::fs::write(&inner, "x").unwrap();
    let enforced = readonly_enforced(&ws, true);
    let meta = std::fs::metadata(&inner).unwrap();
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&inner, perm).unwrap();
    let args = serde_json::json!({"path": ps(&victim)}).to_string();
    let out = super::DeleteDirTool.execute(&args, &ctx()).await;
    if enforced {
        assert!(
            out.clone().unwrap_err().contains("Failed to remove directory"),
            "readonly child must block, got {:?}",
            out
        );
    } else {
        out.unwrap();
    }
    unset_readonly(&inner);
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- CronTool 1207/1260/1268/1321/1339 ----------

fn cron_tool_with_store(tag: &str, blocked: bool) -> super::CronTool {
    let dir = temp_ws(tag);
    let store_path = if blocked {
        // store_path 本身是目录 → save_store 恒失败 → add_job 错误臂。
        std::fs::create_dir_all(dir.join("as_dir")).unwrap();
        dir.join("as_dir")
    } else {
        dir.join("cron_store.json")
    };
    super::CronTool::new(std::sync::Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(&ps(&store_path)),
    )))
}

#[tokio::test]
async fn cron_tool_list_serializes_jobs() {
    let tool = cron_tool_with_store("cronlist", false);
    let out = tool
        .execute(r#"{"action":"list"}"#, &ctx())
        .await
        .unwrap();
    assert!(!out.is_empty());
}

#[tokio::test]
async fn cron_tool_create_save_failure_and_context_fallback() {
    let tool = cron_tool_with_store("cronfail", true);
    tool.set_context("web", "chat-s9d");

    // add_job（非 continue_session）错误臂（1339）；空 context → stored
    // 回退（1260/1268）。
    let out = tool
        .execute(
            r#"{"action":"create","name":"n1","schedule":"every:5s","content":"hi","deliver":true}"#,
            &empty_ctx(),
        )
        .await;
    assert!(out.is_err(), "blocked store must fail create");

    // add_job_ext（continue_session=true）错误臂（1321）。
    let out = tool
        .execute(
            r#"{"action":"create","name":"n2","schedule":"every:7s","content":"hi","continue_session":true,"max_rounds":3}"#,
            &ctx(),
        )
        .await;
    assert!(out.is_err(), "blocked store must fail ext create");
}

// ---------- WebFetchTool 本地环回 1871/1912/1969-1985 ----------

/// 起一个一次性本地 HTTP 服务器，返回 (url, handle)。应答一条固定响应后
/// 线程退出（127.0.0.1 环回，不触外网）。
fn spawn_local_http(
    status_line: &'static str,
    content_type: &'static str,
    body: String,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf); // 丢掉请求头
            let resp = format!(
                "{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status_line,
                content_type,
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{}/s9d", addr), handle)
}

#[tokio::test]
async fn web_fetch_http_error_status() {
    let (url, h) = spawn_local_http("HTTP/1.1 404 Not Found", "text/plain", "gone".to_string());
    let args = serde_json::json!({"url": url}).to_string();
    let out = super::WebFetchTool::new(2048).execute(&args, &ctx()).await;
    assert!(out.unwrap_err().contains("HTTP 404"));
    let _ = h.join();
}

#[tokio::test]
async fn web_fetch_html_without_extractable_text() {
    let (url, h) = spawn_local_http(
        "HTTP/1.1 200 OK",
        "text/html",
        "<html><script>var x=1;</script><style>.a{}</style></html>".to_string(),
    );
    let args = serde_json::json!({"url": url}).to_string();
    let out = super::WebFetchTool::new(2048)
        .execute(&args, &ctx())
        .await
        .unwrap();
    assert!(out.contains("HTML with no extractable text"), "got: {out}");
    let _ = h.join();
}

#[tokio::test]
async fn web_fetch_plain_text_and_truncation() {
    // 未截断的纯文本。
    {
        let (url, h) =
            spawn_local_http("HTTP/1.1 200 OK", "text/plain", "plain body ok".to_string());
        let args = serde_json::json!({"url": url}).to_string();
        let out = super::WebFetchTool::new(2048)
            .execute(&args, &ctx())
            .await
            .unwrap();
        assert!(out.contains("plain body ok"), "got: {out}");
        let _ = h.join();
    }
    // 截断的纯文本（body 超过 max_size）。
    {
        let long = "z".repeat(300);
        let (url, h) = spawn_local_http("HTTP/1.1 200 OK", "text/plain", long);
        let args = serde_json::json!({"url": url}).to_string();
        let out = super::WebFetchTool::new(64)
            .execute(&args, &ctx())
            .await
            .unwrap();
        assert!(out.contains("truncated to 64 bytes"), "got: {out}");
        let _ = h.join();
    }
    // HTML 提取 + 截断。
    {
        let html = format!("<html><body>{}</body></html>", "w".repeat(500));
        let (url, h) = spawn_local_http("HTTP/1.1 200 OK", "text/html", html);
        let args = serde_json::json!({"url": url}).to_string();
        let out = super::WebFetchTool::new(48)
            .execute(&args, &ctx())
            .await
            .unwrap();
        assert!(out.contains("extracted text truncated"), "got: {out}");
        let _ = h.join();
    }
}

/// truncate_str 多字节边界（1969-1971）：截断点落在多字节字符中间时回退
/// 到字符边界。
#[test]
fn truncate_str_cuts_on_char_boundary() {
    let s = "中文内容测试";
    let (out, truncated) = super::truncate_str(s, 7); // 7 落在第二个字内
    assert!(truncated);
    assert_eq!(out, "中文"); // 回退到 6 字节边界
    let (out2, truncated2) = super::truncate_str("abc", 10);
    assert!(!truncated2);
    assert_eq!(out2, "abc");
}

// ---------- SkillManageTool 3368 + 3205-3206 ----------

#[tokio::test]
async fn skill_manage_invalid_json_and_blocked_write() {
    let ws = temp_ws("sm");
    let tool = super::SkillManageTool::new(ps(&ws), None, false);

    // 3368：无效 JSON。
    let out = tool.execute("not-json", &ctx()).await;
    assert!(out.unwrap_err().contains("Invalid JSON"));

    // 3205-3206：SKILL.md 被目录占位 → 写失败回滚。
    std::fs::create_dir_all(ws.join("skills").join("blocked").join("SKILL.md")).unwrap();
    let args = serde_json::json!({
        "action": "create",
        "name": "blocked",
        "content": "# hello skill\n",
        "overwrite": true
    })
    .to_string();
    let out = tool.execute(&args, &ctx()).await;
    let err = out.unwrap_err();
    assert!(err.contains("failed to write SKILL.md"), "got: {err}");
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- GrepTool 3468/3490-3492 ----------

#[tokio::test]
async fn grep_tool_invalid_json_and_readonly_flag() {
    let ws = temp_ws("grep");
    let tool = super::GrepTool::new(ps(&ws));
    let out = tool.execute("not-json", &ctx()).await;
    assert!(out.unwrap_err().contains("Invalid JSON"));
    assert!(tool.is_read_only());
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- GitTool 3525 ----------

#[tokio::test]
async fn git_tool_invalid_json() {
    let ws = temp_ws("git");
    let tool = super::GitTool::new(ps(&ws));
    let out = tool.execute("not-json", &ctx()).await;
    assert!(out.unwrap_err().contains("Invalid JSON"));
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------- WorkflowRunTool 5131-5133/5155/5167/5196 ----------

#[tokio::test]
async fn workflow_run_tool_metadata_and_input_arms() {
    let engine = std::sync::Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
    let tool = super::WorkflowRunTool::new(engine);
    assert!(!tool.description().is_empty());

    // 5155：无效 JSON。
    let out = tool.execute("not-json", &ctx()).await;
    assert!(out.unwrap_err().contains("invalid JSON args"));

    // 5167：input 是对象 → map 收集；未知工作流 → 5196 错误臂。
    let out = tool
        .execute(r#"{"workflow":"no_such_flow","input":{"a":1}}"#, &ctx())
        .await;
    assert!(
        out.unwrap_err().contains("failed to execute"),
        "unknown workflow must fail"
    );

    // input 非 object → 类型错误臂。
    let out = tool
        .execute(r#"{"workflow":"x","input":5}"#, &ctx())
        .await;
    assert!(out.unwrap_err().contains("must be an object"));
}

// ---------- grep_recursive 3571/3597/3603/3617/3630（私有直调） ----------

#[test]
fn grep_recursive_guards_and_truncation() {
    let ws = temp_ws("gr");
    let re = regex::Regex::new("needle").unwrap();

    // 3571：out 已满 → 立即返回。
    let mut full: Vec<String> = vec!["hit".to_string()];
    super::grep_recursive(&ws, &re, None, 1, &mut full);
    assert_eq!(full.len(), 1, "prefilled out must short-circuit");

    // 3597：glob 过滤（不匹配的文件被跳过）+ 命中。
    std::fs::write(ws.join("keep.rs"), "needle here\n").unwrap();
    std::fs::write(ws.join("skip.txt"), "needle skipped\n").unwrap();
    let mut out: Vec<String> = Vec::new();
    super::grep_recursive(&ws, &re, Some("*.rs"), 10, &mut out);
    assert_eq!(out.len(), 1, "glob must filter skip.txt");

    // 3603：>1MB 文件跳过。
    let big = ws.join("big.rs");
    let mut big_content = String::with_capacity(1_100_000);
    while big_content.len() < 1_050_000 {
        big_content.push_str("needle padding padding padding\n");
    }
    std::fs::write(&big, &big_content).unwrap();
    let mut out2: Vec<String> = Vec::new();
    super::grep_recursive(&ws, &re, Some("big.rs"), 10, &mut out2);
    assert!(out2.is_empty(), "oversized file must be skipped");

    // 3617/3630：超长多字节命中行截断 + 行循环封顶。
    let long_file = ws.join("long.rs");
    let mut long_line = String::from("needle ");
    while long_line.len() < 500 {
        long_line.push_str("中文填充");
    }
    let mut content = String::new();
    for _ in 0..5 {
        content.push_str(&long_line);
        content.push('\n');
    }
    std::fs::write(&long_file, content).unwrap();
    let mut out3: Vec<String> = Vec::new();
    super::grep_recursive(&ws, &re, Some("long.rs"), 2, &mut out3);
    assert_eq!(out3.len(), 2, "max must cap line collection");

    let _ = std::fs::remove_dir_all(&ws);
}
