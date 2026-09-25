// manager.rs 覆盖率补充测试（LspOp::method 全操作臂 45-52）。
//
// 豁免：其余 miss 行集中在请求泵的活服务器会话路径（server-request 应答
// 213-219、诊断推送等待 232-238、spawn 失败清理等）——需要真实语言服务器
// 子进程走完整 initialize/握手，单测环境不可达，归入平台豁免。

use super::*;

/// 六个操作的 wire 方法名逐一对照（45-52）。
#[test]
fn lsp_op_method_names_are_stable_wire_contract() {
    assert_eq!(LspOp::Definition.method(), "textDocument/definition");
    assert_eq!(LspOp::References.method(), "textDocument/references");
    assert_eq!(
        LspOp::Implementation.method(),
        "textDocument/implementation"
    );
    assert_eq!(LspOp::Hover.method(), "textDocument/hover");
    assert_eq!(LspOp::Rename.method(), "textDocument/rename");
    assert_eq!(LspOp::CodeAction.method(), "textDocument/codeAction");
}

// ===========================================================================
// Wave4 覆盖批次：真实 rust-analyzer 活会话路径（此前标注「单测环境不可达
// 归入平台豁免」的 spawn_session/initialize 握手/请求泵/didOpen 同步/诊断
// 等待/收尸与 shutdown 全链路——本机 rustup component add rust-analyzer 后
// 已可达）。rust-analyzer 不在 PATH 或 60s 内未就绪时按 SKIP 约定跳过，
// 套件保持绿。
// ===========================================================================

const LIVE_LIB_SRC: &str = "pub fn live_target_fn() -> i32 {\n    42\n}\n";

fn live_scratch_project() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir().ok()?;
    std::fs::create_dir_all(dir.path().join("src")).ok()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"lsp_live_scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .ok()?;
    let lib = dir.path().join("src").join("lib.rs");
    std::fs::write(&lib, LIVE_LIB_SRC).ok()?;
    Some((dir, lib))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_session_spawn_query_and_shutdown_with_rust_analyzer() {
    if registry::find_command("rust-analyzer").is_none() {
        eprintln!("SKIP: rust-analyzer not on PATH");
        return;
    }
    let Some((_dir, lib)) = live_scratch_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };

    let mgr = LspManager::new(Some(Duration::from_secs(60)), Some(Duration::from_secs(60)));
    assert_eq!(mgr.session_count().await, 0);

    // touch_file：spawn + initialize 握手 + didOpen（尽力而为语义）。
    mgr.touch_file(&lib)
        .await
        .expect("rust-analyzer session must spawn and sync doc");
    assert_eq!(
        mgr.session_count().await,
        1,
        "同一 (lang,root) 只建一个会话"
    );

    // hover：等分析就绪（工作区加载是异步的），60s 预算内轮询；
    // 「no hover information」空文案 = 尚未就绪的瞬态，同样重试。
    let mut hover = String::new();
    for _ in 0..120 {
        match mgr.query(LspOp::Hover, &lib, 0, 10).await {
            Ok(h) if !h.contains("no hover information at this position") => {
                hover = h;
                break;
            }
            Ok(_) | Err(_) => {
                let _ = mgr.touch_file(&lib).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    if hover.is_empty() {
        eprintln!("SKIP: rust-analyzer hover never became ready within 60s");
    } else {
        assert!(
            hover.contains("live_target_fn") || hover.contains("i32"),
            "hover 应含符号信息：{hover}"
        );
    }

    // 诊断缓存被动读 + 等待窗口（启动后服务器即推；空/非空皆诚实，
    // 关键是 quiet/max 窗口内必须返回）。
    let _ = mgr.diagnostics_for(&lib).await;
    let start = Instant::now();
    let _diags = mgr.wait_for_diagnostics(&lib, 150, 2000).await;
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "wait_for_diagnostics 必须在窗口内返回"
    );

    // 会话刚用过 → reap 不收；shutdown_all 收尸 1 个会话。
    assert_eq!(mgr.reap_idle().await, 0, "刚触过的会话不得被收尸");
    assert_eq!(mgr.shutdown_all().await, 1);
    assert_eq!(mgr.session_count().await, 0, "shutdown 后会话清零");
}

#[tokio::test]
async fn touch_file_best_effort_semantics_for_unsupported_paths() {
    let mgr = LspManager::new(Some(Duration::from_secs(5)), Some(Duration::from_secs(5)));
    // 不支持的类型 / 无对应服务器：尽力而为 = Ok(())，绝不 Err，也不建会话。
    assert!(
        mgr.touch_file(std::path::Path::new("/x/notes.txt"))
            .await
            .is_ok()
    );
    assert_eq!(mgr.session_count().await, 0);
}

// ===========================================================================
// Wave5 覆盖批次：query 的三个前置拒绝臂（解析未知 op / 不支持的文件类型 /
// 文件不存在）——全部在 spawn 之前短路，无服务器依赖，确定性覆盖。
// ===========================================================================

/// LspOp::parse 未知字符串 → None（41 臂）；六个合法名 → 各自变体。
#[test]
fn lsp_op_parse_rejects_unknown_op() {
    assert_eq!(LspOp::parse("bogus_op"), None);
    assert_eq!(LspOp::parse(""), None);
    assert_eq!(LspOp::parse("definition"), Some(LspOp::Definition));
    assert_eq!(LspOp::parse("hover"), Some(LspOp::Hover));
}

/// 不支持的文件类型（无语言映射）→ 诚实报错并列出支持语言（362-369）。
#[tokio::test]
async fn query_rejects_unsupported_file_type() {
    let mgr = LspManager::new(Some(Duration::from_secs(5)), Some(Duration::from_secs(5)));
    let err = mgr
        .query(LspOp::Hover, std::path::Path::new("/x/notes.txt"), 0, 0)
        .await
        .unwrap_err();
    assert!(err.contains("unsupported file type"), "{err}");
    assert!(err.contains("notes.txt"), "{err}");
    assert!(err.contains("supported languages"), "{err}");
}

/// 文件不存在（.rs 有语言映射但路径无效）→ file does not exist（376-378）。
#[tokio::test]
async fn query_reports_missing_file() {
    let mgr = LspManager::new(Some(Duration::from_secs(5)), Some(Duration::from_secs(5)));
    let missing = std::env::temp_dir().join(format!("nb_lsp_missing_{}.rs", std::process::id()));
    let _ = std::fs::remove_file(&missing);
    let err = mgr.query(LspOp::Hover, &missing, 0, 0).await.unwrap_err();
    assert!(err.contains("file does not exist"), "{err}");
}

/// 诊断两入口对不支持的文件类型诚实返空（773 / 796 的 None 臂）——
/// 无 spawn、无会话，纯短路路径。
#[tokio::test]
async fn diagnostics_entries_return_empty_for_unsupported_file_type() {
    let mgr = LspManager::new(Some(Duration::from_secs(5)), Some(Duration::from_secs(5)));
    let txt = std::path::Path::new("/x/notes.txt");
    assert!(mgr.diagnostics_for(txt).await.is_empty());
    assert!(mgr.wait_for_diagnostics(txt, 50, 200).await.is_empty());
}
