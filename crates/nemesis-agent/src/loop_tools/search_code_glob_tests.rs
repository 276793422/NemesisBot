//! search_code glob 过滤回归（BUG-20261117-050）。
//!
//! 根因背景：glob 过滤此前只有后缀形态可用——`trim_start_matches('*')` 把
//! `test_*.rs` 剥成字面 `ends_with("test_*.rs")` 永不匹配 → 目录走查把
//! 全部文件静默筛掉 → 裸「No matches」假阴性（模型误读为内容不存在，
//! 反复换 pattern 烧轮次——与单文件路径一等公民改造根治的是同一族失败）。
//! 修复 = fnmatch 子集（`*` 任意串 / `?` 单字符 / 其余字面）。

use super::*;
use crate::context::RequestContext;
use crate::loop_tools::search_code::glob_matches_file;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-glob", "u", "agent:convo-glob")
}

// --- glob_matches_file 单元面 ---

#[test]
fn glob_suffix_form_still_matches() {
    // 旧形态行为保持：*.rs 后缀匹配。
    assert!(glob_matches_file("*.rs", "main.rs"));
    assert!(!glob_matches_file("*.rs", "main.rsx"));
    assert!(!glob_matches_file("*.rs", "main.txt"));
    // `**/*.rs` / `src/*.rs`：目录部分无语义，按最后一段取（文件名过滤器）。
    assert!(glob_matches_file("**/*.rs", "lib.rs"));
    assert!(glob_matches_file("src/*.rs", "lib.rs"));
}

#[test]
fn glob_mid_star_form_matches() {
    // 修复目标形态：中间星号此前字面失配永不命中。
    assert!(glob_matches_file("test_*.rs", "test_utils.rs"));
    assert!(glob_matches_file("*_test.go", "foo_test.go"));
    assert!(glob_matches_file("Cargo*.toml", "Cargo.toml"));
    assert!(!glob_matches_file("test_*.rs", "src_utils.rs"));
    assert!(!glob_matches_file("*_test.go", "foo_tests.go"));
}

#[test]
fn glob_question_mark_and_literal() {
    // `?` 恰好一个字符；字面元字符（点）不越位匹配。
    assert!(glob_matches_file("file?.txt", "file1.txt"));
    assert!(!glob_matches_file("file?.txt", "file10.txt"));
    assert!(!glob_matches_file("file?.txt", "file.txt"));
    // 字面 `.` 必须转义：`.rs` 不吞任意字符。
    assert!(!glob_matches_file("a.rs", "aXrs"));
    assert!(glob_matches_file("a.rs", "a.rs"));
}

#[test]
fn glob_empty_pattern_is_noop_filter() {
    // 旧语义保持：空 glob = 不过滤（旧实现 ends_with("") 恒真）。
    assert!(glob_matches_file("", "anything.txt"));
    // 裸 `*` = 全匹配。
    assert!(glob_matches_file("*", "whatever"));
}

// --- GrepTool execute 行为面（端到端：中间星号 glob 不再假阴性） ---

#[tokio::test]
async fn grep_tool_mid_star_glob_finds_matches() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(ws.join("test_a.rs"), "needle here\n").unwrap();
    std::fs::write(ws.join("src_b.rs"), "needle there\n").unwrap();
    std::fs::write(ws.join("notes.txt"), "needle elsewhere\n").unwrap();

    let tool = GrepTool::new(ws.to_string_lossy().to_string());

    // 中间星号 glob：只命中 test_a.rs（修复前全文件被筛 → 裸 No matches）。
    let out = tool
        .execute(r#"{"pattern": "needle", "glob": "test_*.rs"}"#, &ctx())
        .await
        .unwrap();
    assert!(
        out.contains("test_a.rs"),
        "中间星号 glob 必须命中 test_a.rs，实际输出: {out}"
    );
    assert!(
        !out.contains("src_b.rs") && !out.contains("notes.txt"),
        "glob 过滤不得漏进其他文件，实际输出: {out}"
    );

    // 后缀 glob：旧行为保持，命中两个 .rs。
    let out2 = tool
        .execute(r#"{"pattern": "needle", "glob": "*.rs"}"#, &ctx())
        .await
        .unwrap();
    assert!(
        out2.contains("test_a.rs") && out2.contains("src_b.rs"),
        "实际输出: {out2}"
    );
    assert!(!out2.contains("notes.txt"), "实际输出: {out2}");
}

#[tokio::test]
async fn grep_tool_nonmatching_glob_reports_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(ws.join("a.txt"), "needle\n").unwrap();
    let tool = GrepTool::new(ws.to_string_lossy().to_string());

    // glob 筛掉一切时仍是诚实过滤语义（目录走查空 = No matches 文案），
    // 但修复后合法中间星号 glob 不会走到这臂（上一测试钉死）。
    let out = tool
        .execute(r#"{"pattern": "needle", "glob": "*.nomatch"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("No matches"), "实际输出: {out}");
}
