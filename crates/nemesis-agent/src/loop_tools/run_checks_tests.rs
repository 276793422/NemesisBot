//! C8（2026-09-06 devtool-upgrade 阶段 4）：run_checks 构建/测试运行器测试。
//!
//! 纯函数层（生态探测 / 命令映射 / 统计解析 / 失败聚焦）+ 真实 cargo
//! fixture 端到端（无依赖 crate → 离线安全）：成功 build 聚焦回灌 + 故意
//! 编译失败的 error 聚焦 + spill 存档 + 无项目类型诚实报错。

use crate::context::RequestContext;
use crate::r#loop::Tool;
use crate::loop_tools::RunChecksTool;
use std::path::PathBuf;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// 独立临时目录（tag + pid + line 保证并行测试不互踩）。
fn temp_ws(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_lt_c8_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp ws");
    dir
}

fn tool_for(ws: &std::path::Path) -> RunChecksTool {
    RunChecksTool::new(&ws.to_string_lossy())
}

// ---------------------------------------------------------------------------
// 纯函数层：detect_project_eco
// ---------------------------------------------------------------------------

#[test]
fn detect_eco_by_marker_files() {
    let ws = temp_ws("detect");
    assert_eq!(super::detect_project_eco(&ws), None, "empty dir = no eco");
    std::fs::write(ws.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    assert_eq!(super::detect_project_eco(&ws), Some("rust"));
    std::fs::write(ws.join("package.json"), "{}").unwrap();
    // 探测顺序固定：Cargo.toml 在 package.json 之前命中。
    assert_eq!(super::detect_project_eco(&ws), Some("rust"));
    let ws2 = temp_ws("detect_node");
    std::fs::write(ws2.join("package.json"), "{}").unwrap();
    assert_eq!(super::detect_project_eco(&ws2), Some("node"));
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&ws2);
}

// ---------------------------------------------------------------------------
// 纯函数层：checks_commands
// ---------------------------------------------------------------------------

#[test]
fn checks_commands_rust_all_order() {
    let stages = super::checks_commands("rust", "all", None).expect("rust all mapped");
    let labels: Vec<_> = stages.iter().map(|(l, _)| *l).collect();
    assert_eq!(labels, vec!["build", "test", "lint"]);
}

#[test]
fn checks_commands_filter_appended_to_test_only() {
    let (_, cmd) = &super::checks_commands("rust", "test", Some("my_test")).unwrap()[0];
    assert_eq!(cmd, "cargo test my_test");
    let (_, cmd) = &super::checks_commands("python", "test", Some("kexpr")).unwrap()[0];
    assert_eq!(cmd, "pytest -q -k kexpr");
    // build 不吃 filter。
    let (_, cmd) = &super::checks_commands("rust", "build", Some("x")).unwrap()[0];
    assert_eq!(cmd, "cargo build --message-format short");
}

#[test]
fn checks_commands_unmapped_cells_honest_none() {
    // python 无标准 build——不猜。
    assert!(super::checks_commands("python", "build", None).is_none());
    // maven 无独立 lint——不假设插件。
    assert!(super::checks_commands("maven", "lint", None).is_none());
    // 未知 scope / 未知生态。
    assert!(super::checks_commands("rust", "deploy", None).is_none());
    assert!(super::checks_commands("haskell", "build", None).is_none());
}

// ---------------------------------------------------------------------------
// 纯函数层：parse_test_stats
// ---------------------------------------------------------------------------

#[test]
fn parse_stats_sums_multiple_cargo_result_lines() {
    // lib + bin 各一段 → 全部累加。
    let out = "\
running 3 tests
test a ... ok
test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;

running 2 tests
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out;
";
    assert_eq!(super::parse_test_stats(out), Some((4, 1, 1)));
}

#[test]
fn parse_stats_pytest_summary_line() {
    let out = "============================= test session starts ==============================
============== 12 passed, 3 failed, 1 skipped in 0.50s ==============";
    assert_eq!(super::parse_test_stats(out), Some((12, 3, 1)));
}

#[test]
fn parse_stats_garbage_is_none_not_fabricated() {
    assert_eq!(
        super::parse_test_stats("hello world\nno numbers here"),
        None
    );
    assert_eq!(super::parse_test_stats(""), None);
}

// ---------------------------------------------------------------------------
// 纯函数层：extract_failure_lines
// ---------------------------------------------------------------------------

#[test]
fn failure_lines_dedupe_with_count_hint() {
    let out = "error[E0308]: mismatched types\nnoise line\nerror[E0308]: mismatched types\n";
    let lines = super::extract_failure_lines(out, 40);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("(×2)"), "dup hint: {lines:?}");
}

#[test]
fn failure_lines_exclude_non_signatures() {
    let out = "warning: unused variable\nCompiling nb v0.1.0\nFinished dev\n";
    assert!(super::extract_failure_lines(out, 40).is_empty());
}

#[test]
fn failure_lines_capped_with_omission_note() {
    let out = (0..50)
        .map(|i| format!("error[E0001]: thing {i}\n"))
        .collect::<String>();
    let lines = super::extract_failure_lines(&out, 40);
    assert_eq!(lines.len(), 41, "cap + omission note");
    assert!(lines[40].contains("省略"), "note: {}", lines[40]);
}

// ---------------------------------------------------------------------------
// 端到端（真实 cargo，无依赖 fixture → 离线安全）
// ---------------------------------------------------------------------------

const OK_CARGO_TOML: &str = "\
[package]
name = \"nb_c8_fixture_ok\"
version = \"0.1.0\"
edition = \"2021\"
";

const BAD_MAIN_RS: &str = "fn main() { let x: i32 = \"not a number\"; }";

fn write_ok_fixture(ws: &std::path::Path) {
    std::fs::write(ws.join("Cargo.toml"), OK_CARGO_TOML).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();
}

#[tokio::test]
async fn e2e_build_success_focused_reply_with_archive() {
    let ws = temp_ws("ok");
    write_ok_fixture(&ws);
    let out = tool_for(&ws)
        .execute(&serde_json::json!({"scope": "build"}).to_string(), &ctx())
        .await
        .expect("build must be Ok-shaped");
    assert!(
        out.contains("[run_checks] scope=build · rust"),
        "hdr: {out}"
    );
    assert!(out.contains("▶ build: exit 0"), "success line: {out}");
    assert!(out.contains("全部通过"), "no-failure note: {out}");
    // 全量无条件存档：locator 指向 workspace 内 spill 树，文件真实存在。
    let marker = "[完整输出（";
    let idx = out.find(marker).expect("archive note: {out}");
    let path_str = out[idx + marker.len()..]
        .split("）已保存：")
        .nth(1)
        .and_then(|s| s.split(" ——").next())
        .expect("path in note: {out}")
        .trim();
    assert!(
        std::path::Path::new(path_str).is_file(),
        "archived: {path_str}"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn e2e_deliberate_compile_failure_focus_and_stats() {
    // 验收：故意编译失败的 fixture 仓 → 回灌含 error 聚焦 + 失败提示 + 存档。
    let ws = temp_ws("bad");
    std::fs::write(ws.join("Cargo.toml"), OK_CARGO_TOML).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/main.rs"), BAD_MAIN_RS).unwrap();
    let out = tool_for(&ws)
        .execute(&serde_json::json!({"scope": "build"}).to_string(), &ctx())
        .await
        .expect("failure is still Ok-shaped (focused report), not Err");
    assert!(out.contains("✗ build: exit"), "fail line: {out}");
    assert!(
        out.contains("error[E0308]") || out.contains("error:"),
        "focused error lines: {out}"
    );
    assert!(out.contains("提示："), "repair tip: {out}");
    assert!(out.contains("已保存"), "archive note: {out}");
    // 存档文件里有完整原始输出（$ 命令回显 + stderr 段）。
    let spill_root = nemesis_path::resolve_spill_dir_in_workspace(&ws);
    let sess_dir = spill_root.join("sess1");
    let entries: Vec<_> = std::fs::read_dir(&sess_dir)
        .expect("session spill dir")
        .flatten()
        .collect();
    assert!(!entries.is_empty(), "spill written");
    let archived = std::fs::read_to_string(entries[0].path()).unwrap();
    assert!(
        archived.contains("--- stderr ---"),
        "full layout: {archived}"
    );
    assert!(archived.contains("E0308"), "raw error in archive");
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn e2e_no_project_type_honest_error() {
    let ws = temp_ws("noproj");
    let err = tool_for(&ws)
        .execute("{}", &ctx())
        .await
        .expect_err("no markers = honest Err");
    assert!(err.contains("Cargo.toml"), "marker list: {err}");
    assert!(err.contains("package.json"), "marker list: {err}");
    assert!(err.contains("exec"), "recovery hint: {err}");
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn e2e_unmapped_scope_honest_error() {
    // pyproject.toml 仓 + scope=build → python 无标准 build，诚实拒绝。
    let ws = temp_ws("unmapped");
    std::fs::write(ws.join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
    let err = tool_for(&ws)
        .execute(&serde_json::json!({"scope": "build"}).to_string(), &ctx())
        .await
        .expect_err("unmapped scope = honest Err");
    assert!(err.contains("not mapped"), "msg: {err}");
    assert!(err.contains("python"), "eco named: {err}");
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn e2e_test_scope_stats_line_present() {
    // scope=test 带一个必然通过的测试 → 统计行出现且 passed>=1。
    let ws = temp_ws("tstat");
    write_ok_fixture(&ws);
    std::fs::write(
        ws.join("src/main.rs"),
        "#[cfg(test)]\nmod t { #[test] fn ok() { assert!(true); } }\nfn main() {}\n",
    )
    .unwrap();
    let out = tool_for(&ws)
        .execute(&serde_json::json!({"scope": "test"}).to_string(), &ctx())
        .await
        .expect("test run Ok");
    assert!(out.contains("▶ test: exit 0"), "stage line: {out}");
    assert!(
        out.contains("passed 1 / failed 0"),
        "cargo stats parsed: {out}"
    );
    let _ = std::fs::remove_dir_all(&ws);
}
