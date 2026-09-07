//! C3（devtool-upgrade 阶段 2）：编辑后诊断回灌测试。
//!
//! 用 planted fake gopls（Python stdio LSP server，沿 nemesis-lsp
//! `manager/tests.rs` 的确定性形态）驱动 [`AgentLoop::apply_diagnostics_feedback`]
//! 全决策表：ERROR 追加（格式/行号 1-based）、WARN-only 静默、开关关、
//! 非 write|edit 工具、无 manager、未注册语言、touch 失败、max_errors 截断。
//! 全部失败路径都断言**原样返回**——诊断永不改写工具结果语义。

use std::sync::Mutex;
use std::time::Duration;

use super::AgentLoop;
use nemesis_config::DiagnosticsLoopConfig;

/// Process-global env writers must share one lock（env-test-race-lock-pattern）。
static FAKE_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Holds the env lock and restores PATH on drop（即使断言失败也恢复）。
struct PathRestore {
    _lock: std::sync::MutexGuard<'static, ()>,
    orig: String,
}
impl Drop for PathRestore {
    fn drop(&mut self) {
        unsafe {
            std::env::set_var("PATH", &self.orig);
        }
    }
}

/// 最小 fake gopls：initialize 握手 + didOpen/didChange 后按模式推一条
/// publishDiagnostics（随后安静——wait_for_diagnostics 的 quiet 窗口收得住）。
/// - `error`：1 条 severity 1（L2:C4 0-based → 显示 L3:5），source=fake
/// - `warn`：1 条 severity 2（不该被回灌）
/// - `multi`：3 条 severity 1（max_errors 截断测试）
const FAKE_GOPLS_PY: &str = r#"
import sys, json

MODE = "default"
for a in sys.argv[1:]:
    if a.startswith("--mode="):
        MODE = a.split("=", 1)[1]

def read_msg():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(obj):
    body = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def push(uri, msgs):
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
          "params": {"uri": uri, "diagnostics": msgs}})

def diag(line, char, msg, sev=1):
    return {"range": {"start": {"line": line, "character": char},
                      "end": {"line": line, "character": char + 3}},
            "severity": sev, "source": "fake", "message": msg}

while True:
    m = read_msg()
    if m is None:
        break
    method = m.get("method")
    mid = m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"capabilities": {}}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        uri = m.get("params", {}).get("textDocument", {}).get("uri")
        if MODE == "error":
            push(uri, [diag(2, 4, "undefined: foo")])
        elif MODE == "warn":
            push(uri, [diag(0, 0, "only a warning", sev=2)])
        elif MODE == "multi":
            push(uri, [diag(1, 0, "err one"), diag(2, 0, "err two"),
                       diag(3, 0, "err three")])
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "result": None})
"#;

/// Plant fake_lsp_server.py + `gopls` shim（mode 烧进 shim）到临时目录并
/// 前插 PATH。go.mod marker 把 find_root 钉在该目录。同 nemesis-lsp 测试
/// 的前提：本机无真 gopls；纵然有，前插目录赢得解析序。
fn plant_fake_gopls(mode: &str) -> (tempfile::TempDir, PathRestore) {
    let lock = FAKE_ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let py_path = dir.path().join("fake_gopls.py");
    std::fs::write(&py_path, FAKE_GOPLS_PY).unwrap();
    let py = py_path.to_string_lossy().to_string();
    #[cfg(windows)]
    std::fs::write(
        dir.path().join("gopls.cmd"),
        format!("@python \"{py}\" --mode={mode}\r\n"),
    )
    .unwrap();
    #[cfg(not(windows))]
    {
        std::fs::write(
            dir.path().join("gopls"),
            format!("#!/bin/sh\nexec python3 \"{py}\" --mode={mode}\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("gopls"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    std::fs::write(dir.path().join("go.mod"), "module faketest\n\ngo 1.25\n").unwrap();
    std::fs::write(
        dir.path().join("main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .unwrap();

    let orig = std::env::var("PATH").unwrap_or_default();
    let new_path = std::env::join_paths(
        std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(&orig)),
    )
    .unwrap()
    .to_string_lossy()
    .to_string();
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    (dir, PathRestore { _lock: lock, orig })
}

fn cfg_enabled() -> DiagnosticsLoopConfig {
    DiagnosticsLoopConfig {
        enabled: true,
        max_errors: 20,
        wait_max_ms: 5000,
    }
}

/// ERROR 诊断以 1-based 行列 + source 追加，原文保留在头部。
#[tokio::test]
async fn error_diag_appended_with_one_based_position() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "edit_file",
        &go.to_string_lossy(),
        "edit applied",
    )
    .await;

    assert!(out.starts_with("edit applied"), "原文必须保留: {out}");
    assert!(
        out.contains("[LSP] 1 error(s) detected in"),
        "缺回灌头: {out}"
    );
    assert!(
        out.contains("L3:5 undefined: foo (fake)"),
        "缺定位行: {out}"
    );
    let _ = mgr.shutdown_all().await;
}

/// 只有 WARNING（severity 2）→ 原样返回，一个字节都不加。
#[tokio::test]
async fn warning_only_leaves_result_unchanged() {
    let (dir, _path) = plant_fake_gopls("warn");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "write_file",
        &go.to_string_lossy(),
        "written ok",
    )
    .await;

    assert_eq!(out, "written ok");
    let _ = mgr.shutdown_all().await;
}

/// 多条 ERROR 按 max_errors 截断（3 条 → 头部计数与 bullet 都只有 2）。
#[tokio::test]
async fn max_errors_caps_listed_diagnostics() {
    let (dir, _path) = plant_fake_gopls("multi");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");
    let cfg = DiagnosticsLoopConfig {
        enabled: true,
        max_errors: 2,
        wait_max_ms: 5000,
    };

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg,
        "edit_file",
        &go.to_string_lossy(),
        "r",
    )
    .await;

    assert!(
        out.contains("[LSP] 2 error(s) detected in"),
        "截断到 2: {out}"
    );
    assert!(out.contains("err one"), "{out}");
    assert!(out.contains("err two"), "{out}");
    assert!(!out.contains("err three"), "第 3 条必须被截掉: {out}");
    let _ = mgr.shutdown_all().await;
}

/// 开关关 → 不碰服务器，原样返回。
#[tokio::test]
async fn disabled_returns_unchanged() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        DiagnosticsLoopConfig::default(), // enabled=false
        "edit_file",
        &go.to_string_lossy(),
        "untouched",
    )
    .await;

    assert_eq!(out, "untouched");
    assert_eq!(
        mgr.session_count().await,
        0,
        "开关关时不得 spawn 任何 server 会话"
    );
}

/// 非 write_file/edit_file 工具 → 原样返回（read 等不触发诊断等待）。
#[tokio::test]
async fn non_edit_tool_returns_unchanged() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);
    let go = dir.path().join("main.go");

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "read_file",
        &go.to_string_lossy(),
        "read ok",
    )
    .await;

    assert_eq!(out, "read ok");
    assert_eq!(mgr.session_count().await, 0);
}

/// 无 manager（standalone / 未注入）→ 原样返回。
#[tokio::test]
async fn no_manager_returns_unchanged() {
    let out = AgentLoop::apply_diagnostics_feedback(
        None,
        cfg_enabled(),
        "edit_file",
        "C:\\does\\not\\matter.go",
        "still ok",
    )
    .await;
    assert_eq!(out, "still ok");
}

/// 未注册语言（.txt）→ 原样返回。
#[tokio::test]
async fn unsupported_lang_returns_unchanged() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "write_file",
        &dir.path().join("note.txt").to_string_lossy(),
        "txt ok",
    )
    .await;

    assert_eq!(out, "txt ok");
    assert_eq!(mgr.session_count().await, 0);
}

/// 语言有服务器但 touch 失败（文件不存在，读盘炸）→ 原样返回。
#[tokio::test]
async fn touch_failure_returns_unchanged() {
    let (dir, _path) = plant_fake_gopls("error");
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "edit_file",
        &dir.path().join("missing.go").to_string_lossy(),
        "still fine",
    )
    .await;

    assert_eq!(out, "still fine");
    let _ = mgr.shutdown_all().await;
}

/// 服务器不在 PATH → server_available 闸原样放行（不 spawn 会话）。
///
/// PATH 钉死为无 gopls 的系统目录并**全程持 env 锁**：检查与 apply 内的
/// server_available 是两次独立观测，若不持锁，并行测试的 plant 窗口可以
/// 插在中间（TOCTOU）——首跑实锤（fake 会话被 spawn，session_count=1）。
#[tokio::test]
async fn no_server_on_path_returns_unchanged() {
    let lock = FAKE_ENV_LOCK.lock().unwrap();
    let orig = std::env::var("PATH").unwrap_or_default();
    #[cfg(windows)]
    let bare = r"C:\Windows\System32";
    #[cfg(not(windows))]
    let bare = "/usr/bin";
    unsafe {
        std::env::set_var("PATH", bare);
    }
    // PathRestore 持 guard + drop 恢复 PATH（断言失败也恢复，不毒化锁；
    // guard 藏结构体里——裸 MutexGuard 绑定跨 await 会被 clippy 拒）。
    let _path = PathRestore { _lock: lock, orig };
    // 持锁状态直接断言前提成立（系统目录里不会有 gopls）。
    assert!(
        nemesis_lsp::registry::find_command("gopls").is_none(),
        "{bare} 里不应有 gopls"
    );

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.go"), "package main\n").unwrap();
    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(30)), None);

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg_enabled(),
        "edit_file",
        &dir.path().join("main.go").to_string_lossy(),
        "plain",
    )
    .await;

    assert_eq!(out, "plain");
    assert_eq!(mgr.session_count().await, 0, "无服务器时不得 spawn 会话");
}

/// 门 2「诊断闭环实机验证」载体：**真 rust-analyzer** 全链——坏 .rs 文件
/// edit → didOpen → 真服务器分析 → publishDiagnostics(ERROR) → 回灌。
/// `#[ignore]`：依赖本机 rust-analyzer + 冷启动秒级耗时，门禁不跑；
/// 实机验证时 `cargo test -p nemesis-agent --lib diagnostics_feedback --
/// --ignored` 按需执行。机器无 rust-analyzer 时诚实跳过（非误报绿）。
#[tokio::test]
#[ignore = "real rust-analyzer closed loop; run manually for gate verification"]
async fn real_rust_analyzer_closed_loop_gates() {
    if nemesis_lsp::registry::find_command("rust-analyzer").is_none() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // 最小合法 cargo 工程——rust-analyzer 不加载 deps 也能推语法级 ERROR。
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"ra_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let main_rs = dir.path().join("src").join("main.rs");
    std::fs::write(&main_rs, "fn main() {\n    let x = ;\n}\n").unwrap();

    let mgr = nemesis_lsp::LspManager::new(Some(Duration::from_secs(120)), None);
    // 实测（2026-09-05 裸客户端 + 本测试）：rust-analyzer 冷启动首析
    // ~10-25s（工作区加载完成才推首批诊断，且首推可能是空数组）→ 等待
    // 上限放宽到 90s。生产热路径（服务器已运行）毫秒级，默认
    // wait_max_ms=2000 面向热路径；冷启动 miss 属 best-effort 边界
    // （该轮不回灌，服务器已热后下轮编辑正常回灌）。
    let cfg = DiagnosticsLoopConfig {
        enabled: true,
        max_errors: 20,
        wait_max_ms: 90_000,
    };

    let out = AgentLoop::apply_diagnostics_feedback(
        Some(&mgr),
        cfg,
        "edit_file",
        &main_rs.to_string_lossy(),
        "edited",
    )
    .await;
    eprintln!("real-chain feedback:\n{out}");

    assert!(
        out.contains("[LSP]") && out.contains("please fix:"),
        "真 rust-analyzer 应回灌语法 ERROR，实际输出: {out}"
    );
    assert!(
        out.contains("(rust-analyzer)") || out.contains("(rustc)") || out.contains("(lsp"),
        "诊断应带 source 标注: {out}"
    );
    let _ = mgr.shutdown_all().await;
}
