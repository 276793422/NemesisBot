use super::*;
use std::sync::Mutex;

// These tests poke the shared global counters. Without serialization, a
// parallel `pass/fail/skip` increment races with `reset_counters()`+`get_counters()`
// and the reset assertion can see (1,1,1) instead of (0,0,0); symmetrically, a
// parallel naked `reset_counters()` wipes the locked test's increments mid-window
// (`failed: 0`, Windows CI 案例 2026-09-02). The contract is bilateral: EVERY
// test that touches the counters (increment or reset) must hold this lock.
static COUNTER_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn workspace_paths_are_consistent() {
    let ws = TestWorkspace::new().unwrap();
    assert!(ws.path().exists());
    let home = ws.home().to_string_lossy().to_string();
    assert!(home.ends_with(".nemesisbot"), "home: {}", home);
    assert!(ws.config_path().to_string_lossy().ends_with("config.json"));
    assert!(ws.workspace().to_string_lossy().contains("workspace"));
    assert!(ws.forge_dir().to_string_lossy().ends_with("forge"));
    assert!(
        ws.security_config_path()
            .to_string_lossy()
            .ends_with("config.security.json")
    );
}

#[test]
fn cli_output_success_and_contains() {
    let out = CliOutput {
        exit_code: 0,
        stdout: "hello world\nline2".to_string(),
        stderr: "a warning".to_string(),
    };
    assert!(out.success());
    assert!(out.stdout_contains("hello"));
    assert!(!out.stdout_contains("missing"));
    assert!(out.stderr_contains("warning"));
}

#[test]
fn cli_output_nonzero_is_failure() {
    let out = CliOutput {
        exit_code: 1,
        stdout: String::new(),
        stderr: "e".into(),
    };
    assert!(!out.success());
}

#[test]
fn cli_output_first_line_trims_and_truncates() {
    let out = CliOutput {
        exit_code: 0,
        stdout: "   first line   \nsecond".to_string(),
        stderr: String::new(),
    };
    assert_eq!(out.stdout_first_line(), "first line");

    // truncation at 120 chars
    let long = "x".repeat(200);
    let out = CliOutput {
        exit_code: 0,
        stdout: long,
        stderr: String::new(),
    };
    assert_eq!(out.stdout_first_line().len(), 120);

    // empty stdout → ""
    let out = CliOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    };
    assert_eq!(out.stdout_first_line(), "");
}

#[test]
fn counters_reset_to_zero() {
    let _g = COUNTER_TEST_LOCK.lock().unwrap();
    reset_counters();
    assert_eq!(get_counters(), (0, 0, 0));
}

#[test]
fn pass_fail_skip_increment_counters() {
    let _g = COUNTER_TEST_LOCK.lock().unwrap();
    reset_counters();
    let _ = pass("p1", "ok");
    let _ = fail("f1", "bad");
    let _ = skip("s1", "later");
    let (p, f, s) = get_counters();
    assert!(p >= 1, "passed: {}", p);
    assert!(f >= 1, "failed: {}", f);
    assert!(s >= 1, "skipped: {}", s);
}

#[test]
fn test_result_variants_fields() {
    let _g = COUNTER_TEST_LOCK.lock().unwrap();
    let r = pass("n", "m");
    assert!(r.passed);
    assert_eq!(r.name, "n");
    assert_eq!(r.message, "m");

    let r = fail("n", "m");
    assert!(!r.passed);

    let r = skip("n", "reason");
    // skip counts as passed=true but message prefixed SKIP:
    assert!(r.passed);
    assert!(r.message.starts_with("SKIP:"));
}

#[test]
fn print_results_all_passed_returns_true() {
    let _g = COUNTER_TEST_LOCK.lock().unwrap();
    reset_counters();
    let results = vec![pass("a", "ok"), pass("b", "ok")];
    assert!(print_results(&results));
}

#[test]
fn print_results_with_failure_returns_false() {
    let _g = COUNTER_TEST_LOCK.lock().unwrap();
    let results = vec![pass("a", "ok"), fail("b", "bad")];
    assert!(!print_results(&results));
}

#[test]
fn print_results_skip_does_not_fail() {
    let _g = COUNTER_TEST_LOCK.lock().unwrap();
    let results = vec![skip("a", "no-op"), pass("b", "ok")];
    assert!(print_results(&results));
}

#[test]
fn resolve_project_root_finds_workspace() {
    // Running from the test binary, walk up should find the workspace root.
    let root = resolve_project_root();
    assert!(root.is_ok(), "expected to resolve workspace root");
    let root = root.unwrap();
    assert!(root.join("Cargo.toml").exists());
    assert!(
        std::fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("[workspace]")
    );
}

#[test]
fn http_client_builds_without_panic() {
    let _client = http_client();
}

#[test]
fn suite_header_prints() {
    // Just ensure it doesn't panic.
    print_suite_header("My Suite");
}

#[test]
fn strip_verbatim_local_drive_prefix() {
    assert_eq!(
        strip_verbatim(r"\\?\C:\Users\foo\AppData\Local\Temp\x"),
        r"C:\Users\foo\AppData\Local\Temp\x"
    );
}

#[test]
fn strip_verbatim_unc_maps_back_to_share_form() {
    // canonicalize 对 UNC 目标返回 \\?\UNC\host\share\...——直接剥前缀会
    // 得到非法的 UNC\host\share（回归锁：企业锁定环境 TEMP 指网络盘）。
    assert_eq!(
        strip_verbatim(r"\\?\UNC\fileserver\share\tmp\ws"),
        r"\\fileserver\share\tmp\ws"
    );
}

#[test]
fn strip_verbatim_passthrough_without_prefix() {
    assert_eq!(strip_verbatim(r"C:\plain\path"), r"C:\plain\path");
    assert_eq!(strip_verbatim("/tmp/plain"), "/tmp/plain");
}

#[test]
fn long_form_path_resolves_short_name_via_verbatim_strip() {
    // 8.3 短名环境（如 LOCALA~1）：canonicalize 得到 verbatim 长名，剥前缀
    // 后必须是存在的常规路径（真实 canonicalize 往返，覆盖纯函数测试之外
    // 的组合路径）。
    let ws = TestWorkspace::new().unwrap();
    let root = ws.root;
    assert!(root.exists(), "root must exist: {}", root.display());
    assert!(!root.as_os_str().to_string_lossy().starts_with(r"\\?\"));
}

#[test]
fn floor_char_boundary_ascii_and_multibyte() {
    // ASCII：恒等。
    assert_eq!(super::floor_char_boundary("abc", 3), 3);
    assert_eq!(super::floor_char_boundary("abc", 1), 1);
    assert_eq!(super::floor_char_boundary("abc", 10), 3); // n ≥ len → floor 到 len
    assert_eq!(super::floor_char_boundary("", 200), 0);
    // 多字节：中文每字符 3 字节，切点 200 落在第 68 个"中"的中间 → 回退到 199。
    let s = format!("{}a{}", "中".repeat(66), "中".repeat(10));
    assert_eq!(s.len(), 229);
    assert_eq!(super::floor_char_boundary(&s, 200), 199);
    // 切点恰在边界 → 原样返回。
    assert_eq!(super::floor_char_boundary(&s, 198), 198);
}

/// BUG-20261105-029 回归锁：`http_get_json` 对非 2xx 响应的预览截断曾用裸
/// `&body[..len.min(200)]` 字节切片——中文错误体（gateway 错误消息原样
/// UTF-8）第 200 字节落在多字节字符中间直接 panic，把「HTTP 错误报告」
/// 变成「测试工具崩溃」。修复后必须诚实 bail（str-slice-multibyte-panic
/// 家族，BUG-20261029-185 同款写法的测试基建漏网点）。
#[tokio::test]
async fn http_get_json_non2xx_multibyte_body_reports_not_panics() {
    // mini HTTP server：返回 500 + 229 字节 body，字节 200 恰在多字节字符中间。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!("{}a{}", "中".repeat(66), "中".repeat(10)); // 229B
    let server_body = body.clone();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = std::io::Read::read(&mut sock, &mut buf); // 读掉请求头
        let resp = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            server_body.len(),
            server_body
        );
        let _ = std::io::Write::write_all(&mut sock, resp.as_bytes());
    });

    let err = super::http_get_json(
        &format!("http://127.0.0.1:{port}/v1/models"),
        std::time::Duration::from_secs(5),
    )
    .await
    .expect_err("500 must be Err");
    let msg = format!("{err:#}");
    assert!(msg.contains("500"), "status in message: {msg}");
    // 截断预览保留到字节边界（66 个中文字符 + 'a' = 199B），而非 panic。
    assert!(msg.contains(&"中".repeat(66)), "preview prefix kept: {msg}");
}
