//! cli_reference 静态目录漂移守卫（BUG-20261110 → REPORT 归档）。
//!
//! 真相源 = `crate::Cli::command()`（clap 内省，feature cfg 自动跟随——
//! feature 裁掉的命令不进编译也不进断言；手写目录是超集，多出的臂无害）。
//! 不变量（nemesis-agent 的 `cli_overview`/`cli_detail` 手写目录 vs 实际 CLI）：
//! 1. 每个非 hidden 顶层命令名在概览文本中出现（词边界）；
//! 2. `cli_detail(name)` 对每个非 hidden 顶层命令返回 Ok（match 臂在场）；
//! 3. 每个非 hidden 顶层命令的直接子命令名在对应详情文本中出现（词边界）。
//!
//! 新增/改名命令漏更新目录时本测试红——把 mcp.rs 里「Keep data in sync」
//! 的注释自警变成机器闸。默认 feature 集 = 全量构建，本测试即覆盖全命令面。

use clap::CommandFactory;

/// 词边界包含判定（`vault` 不得被 `vaultish` 等子串假阳性命中；`-` 视为
/// 词字符——`serve` 不误命中 `mcp-serve`，`run` 不误命中 `run-time`）。
fn contains_word(haystack: &str, word: &str) -> bool {
    fn is_word_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
    }
    let hay = haystack.as_bytes();
    let needle = word.as_bytes();
    if needle.is_empty() {
        return true;
    }
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(word) {
        let at = from + rel;
        let end = at + needle.len();
        let before_ok = at == 0 || !is_word_byte(hay[at - 1]);
        let after_ok = end == hay.len() || !is_word_byte(hay[end]);
        if before_ok && after_ok {
            return true;
        }
        from = at + 1;
    }
    false
}

#[test]
fn contains_word_boundaries() {
    assert!(contains_word("run and run-time checks", "run"));
    assert!(contains_word("| vault |", "vault"));
    assert!(!contains_word("run-time", "run"));
    assert!(!contains_word("vaultish", "vault"));
    assert!(!contains_word("mcp-serve", "serve"));
    assert!(contains_word("mcp-serve", "mcp-serve"));
}

#[test]
fn cli_reference_catalog_covers_all_commands() {
    let cli = crate::Cli::command();
    let overview = nemesis_agent::loop_tools::cli_overview();
    let mut problems: Vec<String> = Vec::new();

    // 权威命令树打印（`-- --nocapture` 可见）：目录作者的对照源，也是
    // 失败现场的第一手证据（哪些命令/子命令在真实 CLI 里）。
    println!("=== clap CLI tree (authoritative; hidden lines skipped by assertions) ===");
    for cmd in cli.get_subcommands() {
        let name = cmd.get_name();
        let hidden = cmd.is_hide_set();
        let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
        let aliases: Vec<&str> = cmd.get_all_aliases().collect();
        println!(
            "CMD\t{name}\thidden={hidden}\taliases=[{}]\tabout={about}",
            aliases.join(",")
        );
        for sub in cmd.get_subcommands() {
            let sabout = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            println!(
                "  SUB\t{}\thidden={}\tabout={sabout}",
                sub.get_name(),
                sub.is_hide_set()
            );
        }
    }

    for cmd in cli.get_subcommands() {
        let name = cmd.get_name();
        if cmd.is_hide_set() || name == "help" {
            continue;
        }
        if !contains_word(&overview, name) {
            problems.push(format!("cli_overview 缺命令 `{name}`"));
        }
        match nemesis_agent::loop_tools::cli_detail(name) {
            Ok(detail) => {
                for sub in cmd.get_subcommands() {
                    let sub_name = sub.get_name();
                    if sub.is_hide_set() || sub_name == "help" {
                        continue;
                    }
                    if !contains_word(&detail, sub_name) {
                        problems.push(format!("cli_detail[{name}] 缺子命令 `{sub_name}`"));
                    }
                }
            }
            Err(_) => problems.push(format!("cli_detail 缺命令 `{name}` 的臂")),
        }
    }

    assert!(
        problems.is_empty(),
        "cli_reference 静态目录漂移（真相源=clap Cli 枚举，共 {} 处缺口）:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}
