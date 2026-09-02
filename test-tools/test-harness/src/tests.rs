use super::*;
use std::sync::Mutex;

// These tests poke the shared global counters. Without serialization, a
// parallel `pass/fail/skip` increment races with `reset_counters()`+`get_counters()`
// and the reset assertion can see (1,1,1) instead of (0,0,0); symmetrically, a
// parallel naked `reset_counters()` wipes the locked test's increments mid-window
// (`failed: 0`, Windows CI 实录 2026-09-02). The contract is bilateral: EVERY
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
