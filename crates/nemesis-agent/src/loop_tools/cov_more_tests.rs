//! Wave4 覆盖率补充批次（loop_tools.rs 纯助手与执行原语）：
//! - `exec_output_passed` / `exec_timeout_secs`（B2 失败形态判定单一真相源）
//! - `preview_chars` / `binary_file_summary`（magic 分派三臂）
//! - `resolve_tool_path`（有界/无界两形态）
//! - `count_labeled_numbers`（四标签臂 + any 信号）
//! - `checks_commands` 未映射格 + filter trim
//! - `detect_project_eco` 五生态 + 空目录
//! - `run_one_stage`（成功 / 非零退出 / cwd / 超时收尸）
//! - WebSearch 无 key 早退 + 全关错误臂（不碰真实网络）

use super::*;
use crate::context::RequestContext;

fn cov_ctx() -> RequestContext {
    RequestContext::new("web", "chat-cov", "user-cov", "agent:cov/session")
}

// ---------------------------------------------------------------------------
// exec 判定原语
// ---------------------------------------------------------------------------

/// exec_output_passed：非零退出 / 超时两种失败形态 = false；裸 stdout 与
/// `(no output)` = true（1510-1517 的四个臂）。
#[test]
fn exec_output_passed_truth_table() {
    assert!(!exec_output_passed("Exit code: 1\nsome stderr"));
    assert!(!exec_output_passed("Command timed out after 30s\npartial"));
    assert!(exec_output_passed("all good"));
    assert!(exec_output_passed("(no output)"));
}

/// exec_timeout_secs：None → 30；超上限钳到 600；正常值透传（1501-1503）。
#[test]
fn exec_timeout_secs_none_default_and_clamp() {
    assert_eq!(exec_timeout_secs(None), 30);
    assert_eq!(exec_timeout_secs(Some(100_000)), 600);
    assert_eq!(exec_timeout_secs(Some(5)), 5);
}

/// preview_chars：短串原样 + 总字符数；多字节按字符数截（不切 UTF-8）。
#[test]
fn preview_chars_multibyte_safe() {
    let (s, total) = preview_chars("hello", 10);
    assert_eq!(s, "hello");
    assert_eq!(total, 5);

    let (s, total) = preview_chars("你好世界", 2);
    assert_eq!(s, "你好");
    assert_eq!(total, 4);
}

/// binary_file_summary：PDF magic → PDF 文案；PNG magic → png image 文案；
/// 未知字节 → 仅字节数（297-336 的三分派）。
#[test]
fn binary_file_summary_magic_dispatch() {
    let dir = tempfile::tempdir().unwrap();

    let pdf = dir.path().join("doc.pdf");
    std::fs::write(&pdf, b"%PDF-1.7\nbinary-ish payload").unwrap();
    let s = binary_file_summary(&pdf);
    assert!(s.contains("binary PDF file"), "{s}");
    assert!(s.contains("27 bytes"), "{s}");

    let png = dir.path().join("pic.png");
    let mut png_bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png_bytes.extend_from_slice(b"rest of fake png");
    std::fs::write(&png, &png_bytes).unwrap();
    let s = binary_file_summary(&png);
    assert!(s.contains("binary png image file"), "{s}");
    assert!(s.contains("24 bytes"), "{s}");

    let unknown = dir.path().join("blob.bin");
    std::fs::write(&unknown, [0x00, 0x01, 0x02, 0x03, 0x04]).unwrap();
    let s = binary_file_summary(&unknown);
    assert!(s.contains("5 bytes binary (type unknown)"), "{s}");
}

/// resolve_tool_path：无界 → 原样 PathBuf；有界 restrict + 越界 → Err；
/// 有界非 restrict → join 后放行（186-198）。
#[test]
fn resolve_tool_path_with_and_without_boundary() {
    let p = resolve_tool_path("a/b.txt", None).unwrap();
    assert_eq!(p, PathBuf::from("a/b.txt"));

    let dir = tempfile::tempdir().unwrap();
    let inside = resolve_tool_path(
        "sub/new.txt",
        Some(&WorkspaceBoundary {
            root: dir.path().to_path_buf(),
            restrict: true,
        }),
    )
    .unwrap();
    assert_eq!(inside, dir.path().join("sub/new.txt"));

    let err = resolve_tool_path(
        if cfg!(windows) {
            "Z:/elsewhere/x.txt"
        } else {
            "/elsewhere/x.txt"
        },
        Some(&WorkspaceBoundary {
            root: dir.path().to_path_buf(),
            restrict: true,
        }),
    )
    .unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");

    let ok = resolve_tool_path(
        if cfg!(windows) {
            "Z:/elsewhere/x.txt"
        } else {
            "/elsewhere/x.txt"
        },
        Some(&WorkspaceBoundary {
            root: dir.path().to_path_buf(),
            restrict: false,
        }),
    )
    .unwrap();
    assert_eq!(
        ok,
        PathBuf::from(if cfg!(windows) {
            "Z:/elsewhere/x.txt"
        } else {
            "/elsewhere/x.txt"
        })
    );
}

// ---------------------------------------------------------------------------
// run_checks 解析助手
// ---------------------------------------------------------------------------

/// count_labeled_numbers：passed / failed+error / ignored+skipped 四标签臂
/// + 未匹配标签 any=false（1668-1702）。
#[test]
fn count_labeled_numbers_labels() {
    let (p, f, i, any) = count_labeled_numbers(&["3", "passed"]);
    assert_eq!((p, f, i, any), (3, 0, 0, true));

    let (p, f, i, any) = count_labeled_numbers(&["2", "failed"]);
    assert_eq!((p, f, i, any), (0, 2, 0, true));

    let (p, f, i, any) = count_labeled_numbers(&["1", "error"]);
    assert_eq!((p, f, i, any), (0, 1, 0, true));

    let (p, f, i, any) = count_labeled_numbers(&["4", "skipped"]);
    assert_eq!((p, f, i, any), (0, 0, 4, true));

    let (p, f, i, any) = count_labeled_numbers(&["9", "mysteries"]);
    assert_eq!((p, f, i, any), (0, 0, 0, false));

    // 非数字首 token 整对跳过。
    let (p, f, i, any) = count_labeled_numbers(&["ok.", "5", "passed"]);
    assert_eq!((p, f, i, any), (5, 0, 0, true));
}

/// checks_commands：故意不映射的格子（python build / maven lint / 未知生态）
/// 返回 None；rust test 带 filter 走 trim 后拼进命令（1590-1656 的否臂）。
#[test]
fn checks_commands_unmapped_cells_and_filter_trim() {
    assert!(checks_commands("python", "build", None).is_none());
    assert!(checks_commands("maven", "lint", None).is_none());
    assert!(checks_commands("fortran", "build", None).is_none());

    let stages = checks_commands("rust", "test", Some("  cov_more  ")).unwrap();
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].1, "cargo test cov_more");

    // filter 空白等价于无 filter。
    let stages = checks_commands("rust", "test", Some("   ")).unwrap();
    assert_eq!(stages[0].1, "cargo test");
}

/// detect_project_eco：五个标记文件各自命中对应生态；空目录 → None
/// （1656-1666）。
#[test]
fn detect_project_eco_marker_files() {
    let dir = tempfile::tempdir().unwrap();
    assert!(detect_project_eco(dir.path()).is_none());

    for (marker, eco) in [
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("go.mod", "go"),
        ("pyproject.toml", "python"),
        ("pom.xml", "maven"),
    ] {
        let case = tempfile::tempdir().unwrap();
        std::fs::write(case.path().join(marker), b"marker").unwrap();
        assert_eq!(detect_project_eco(case.path()), Some(eco), "{marker}");
    }
}

// ---------------------------------------------------------------------------
// run_one_stage（执行原语：真实子进程，秒级）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_one_stage_success_and_nonzero_exit() {
    let (code, stdout, stderr, timed_out) = run_one_stage("echo cov_ok", None, 30).await;
    assert_eq!(code, Some(0), "stderr={stderr}");
    assert!(!timed_out);
    let joined = format!("{stdout}{stderr}");
    assert!(joined.contains("cov_ok"), "{joined}");

    let (code, _stdout, _stderr, timed_out) = run_one_stage("exit 7", None, 30).await;
    assert_eq!(code, Some(7));
    assert!(!timed_out);
}

/// cwd 参数透传：在临时目录里创建文件后由相对路径读取（1971-1973 的 cwd 臂）。
#[tokio::test]
async fn run_one_stage_honors_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("needle.txt"), b"cwd_marker").unwrap();
    let cmd = if cfg!(windows) {
        "type needle.txt"
    } else {
        "cat needle.txt"
    };
    let (code, stdout, stderr, timed_out) = run_one_stage(cmd, Some(dir.path()), 30).await;
    assert_eq!(code, Some(0), "stderr={stderr}");
    assert!(!timed_out);
    assert!(stdout.contains("cwd_marker"), "{stdout}");
}

/// 超时臂：慢命令 + 1s 上限 → timed_out=true 且仍回收残余输出（1985-1994）。
#[tokio::test]
async fn run_one_stage_timeout_kills_and_reports() {
    let slow = if cfg!(windows) {
        // ping -n 6 ≈ 5s，比 sleep.exe 更可移植（无外部依赖）。
        "ping -n 6 127.0.0.1 >nul"
    } else {
        "sleep 5"
    };
    let (code, _stdout, _stderr, timed_out) = run_one_stage(slow, None, 1).await;
    assert!(timed_out, "慢命令必须在 1s 预算内被判超时");
    assert_eq!(code, None, "超时收尸后无退出码");
}

// ---------------------------------------------------------------------------
// WebSearchTool：无 key 早退 + 全关错误臂（零网络）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn web_search_all_providers_disabled_errors_honestly() {
    // Default 的 duckduckgo_enabled=true 会真发网络请求——显式全关。
    let cfg = WebSearchConfig {
        duckduckgo_enabled: false,
        ..Default::default()
    };
    let err = WebSearchTool::new(cfg)
        .execute(r#"{"query":"rust coverage"}"#, &cov_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("No search provider configured"), "{err}");
}

/// brave_enabled 但 key 缺失 → 条件短路落到后续 provider，仍全关 → 同一
/// 诚实错误（2815 的 && 短路 + 2825 终局臂）。
#[tokio::test]
async fn web_search_enabled_without_key_falls_through_to_no_provider() {
    let cfg = WebSearchConfig {
        brave_enabled: true,
        brave_api_key: None,
        duckduckgo_enabled: false,
        ..Default::default()
    };
    let err = WebSearchTool::new(cfg)
        .execute(r#"{"query":"q"}"#, &cov_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("No search provider configured"), "{err}");
}

/// search_brave / search_perplexity 的无 key 早退（2840-2842 / 2983-2985）
/// ——直接调私有方法，不发任何网络请求。
#[tokio::test]
async fn search_brave_and_perplexity_without_keys_bail_early() {
    let tool = WebSearchTool::new(WebSearchConfig::default());
    let err = tool.search_brave("q").await.unwrap_err();
    assert_eq!(err, "Brave API key not configured");

    let err = tool.search_perplexity("q").await.unwrap_err();
    assert_eq!(err, "Perplexity API key not configured");
}

/// 空字符串 key 与缺失 key 同判（`Some(k) if !k.is_empty()` 的否臂）。
#[tokio::test]
async fn search_brave_empty_key_equals_missing_key() {
    let cfg = WebSearchConfig {
        brave_api_key: Some("   ".trim().to_string()),
        ..Default::default()
    };
    let err = WebSearchTool::new(cfg).search_brave("q").await.unwrap_err();
    assert_eq!(err, "Brave API key not configured");
}

// ---------------------------------------------------------------------------
// wave5c：apply_edit_to_content count==0 三臂 / fs 工具成功与拒绝臂 /
// 工具元数据（Message/Exec/RunChecks/Cron/TodoWrite）/ drain_pipe None 臂
// ---------------------------------------------------------------------------

/// apply_edit_to_content 的 count==0 决策表：replace_all 未命中带 hint、
/// 非 replace_all 走五级级联（近邻命中 → Ok+级联名；全无 → NoMatch）。
#[test]
fn apply_edit_to_content_zero_count_arms() {
    use super::apply_edit_to_content;

    // replace_all=true 且全无命中 → Err + not-found hint。
    let err = match apply_edit_to_content("hello world", "zzz", "y", true, "f.txt") {
        Ok(_) => panic!("replace_all miss must fail"),
        Err(e) => e,
    };
    assert!(err.contains("old_text not found"), "err: {err}");

    // 行首尾空白差异 → line-trimmed 级联命中 → Ok，cascade_level 有名。
    let ok = apply_edit_to_content("say hello", "  say hello  ", "goodbye", false, "f.txt")
        .expect("line-trimmed cascade hits");
    assert!(ok.new_content.contains("goodbye"), "{:?}", ok.new_content);
    assert!(ok.cascade_level.is_some(), "cascade hit must carry a level");

    // 完全无近邻 → 级联 NoMatch（可能带 span 注记）→ Err。
    let err = match apply_edit_to_content("12345", "zzzqqq", "y", false, "f.txt") {
        Ok(_) => panic!("total miss must fail"),
        Err(e) => e,
    };
    assert!(err.contains("old_text not found"), "err: {err}");
}

/// ListDirectoryTool：目标是文件 → 拒绝；空目录 → 诚实空提示。
#[tokio::test]
async fn list_directory_rejects_file_and_reports_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("plain.txt");
    std::fs::write(&file, b"x").unwrap();

    let tool = ListDirectoryTool::default();
    let err = tool
        .execute(
            &serde_json::json!({ "path": file.to_string_lossy() }).to_string(),
            &cov_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("not a directory"), "err: {err}");

    let empty = dir.path().join("emptydir");
    std::fs::create_dir_all(&empty).unwrap();
    let out = tool
        .execute(
            &serde_json::json!({ "path": empty.to_string_lossy() }).to_string(),
            &cov_ctx(),
        )
        .await
        .expect("empty dir lists fine");
    assert!(out.contains("empty") || out.contains("Empty"), "out: {out}");
}

/// Append/Delete/CreateDir/DeleteDir 四件套成功路径（绝对路径，不碰 cwd）。
#[tokio::test]
async fn file_mutation_tools_success_paths() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name: &str| dir.path().join(name).to_string_lossy().to_string();

    // append：创建 + 追加 + flush（"Appended N bytes"）。
    let out = AppendFileTool::default()
        .execute(
            &serde_json::json!({ "path": p("log.txt"), "content": "more\n" }).to_string(),
            &cov_ctx(),
        )
        .await
        .expect("append succeeds");
    assert!(out.contains("Appended"), "out: {out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("log.txt")).unwrap(),
        "more\n"
    );

    // create_dir：含多级父目录。
    let out = CreateDirTool::default()
        .execute(
            &serde_json::json!({ "path": p("a/b/c") }).to_string(),
            &cov_ctx(),
        )
        .await
        .expect("create_dir succeeds");
    assert!(out.contains("created"), "out: {out}");
    assert!(dir.path().join("a/b/c").is_dir());

    // delete_file。
    let out = DeleteFileTool::default()
        .execute(
            &serde_json::json!({ "path": p("log.txt") }).to_string(),
            &cov_ctx(),
        )
        .await
        .expect("delete file succeeds");
    assert!(out.contains("Deleted"), "out: {out}");
    assert!(!dir.path().join("log.txt").exists());

    // delete_dir：递归删目录树。
    let out = DeleteDirTool::default()
        .execute(
            &serde_json::json!({ "path": p("a") }).to_string(),
            &cov_ctx(),
        )
        .await
        .expect("delete dir succeeds");
    assert!(out.contains("removed"), "out: {out}");
    assert!(!dir.path().join("a").exists());
}

/// MessageTool 元数据：mass_message 限额类别声明 + parameters schema。
#[test]
fn message_tool_metadata() {
    let tool = MessageTool::new();
    assert_eq!(tool.limit_categories(), &["mass_message"]);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert_eq!(p["type"], "object");
}

/// ExecTool 元数据：exec 限额类别 + parameters schema（不执行命令）。
#[test]
fn exec_tool_metadata() {
    let tool = ExecTool::new("C:\\ws", false);
    assert_eq!(tool.limit_categories(), &["exec"]);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert_eq!(p["type"], "object");
}

/// RunChecksTool 元数据（description/parameters 全量）。
#[test]
fn run_checks_tool_metadata() {
    let tool = RunChecksTool::new("C:\\ws");
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert_eq!(p["type"], "object");
}

/// CronTool 元数据（store 指向临时目录，不触碰真实配置）。
#[test]
fn cron_tool_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("cron.json");
    let service = std::sync::Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(&store.to_string_lossy()),
    ));
    let tool = CronTool::new(service);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert_eq!(p["type"], "object");
}

/// TodoWriteTool 元数据（workspace 指向临时目录，不广播事件）。
#[test]
fn todo_write_tool_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let tool = TodoWriteTool::new(dir.path().to_path_buf(), None);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert_eq!(p["type"], "object");
}

/// drain_pipe 的 None 臂：管道缺失 → 空缓冲（不挂起）。
#[tokio::test]
async fn drain_pipe_none_returns_empty() {
    let buf = super::drain_pipe(None::<tokio::io::Empty>).await;
    assert!(buf.is_empty());
}
