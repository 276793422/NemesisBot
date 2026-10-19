//! delegate 腿探针与验收驱动（③a）：生产同源 API 直驱（`find_cli_on_path`
//! 与 factory 同一探测函数、`DelegateManager` 与 delegate_* 工具同一注册
//! 表），不在探针里复刻第二条实现——探针验的就是生产行为。
//!
//! - `probe`：CLI 在场（SKIP 判定点）+ `--version` + auth 信息面（codex
//!   `login status`）。auth 输出不认 = 打印原文不猜结论；真可用性由
//!   roundtrip 判定。
//! - `roundtrip`：真 CLI start → prompt 往返（assistant 文本须含回声标记）
//!   → stop → 断言 pid 收尸（无孤儿）。
//! - `estop`：空闲会话 start（不发 prompt，零 token）→ estop.trigger →
//!   断言条目停转 + pid 收尸（estop watcher 的 stop_all 排空语义）。

use std::collections::HashMap;
use std::time::Duration;

use nemesis_agent::EstopState;
use nemesis_agent::delegate_registry::{DelegateManager, DelegateProtocolKind};
use nemesis_agent::loop_tools::cli_delegation::find_cli_on_path;

use crate::{EXIT_FAIL, EXIT_PASS, EXIT_SKIP, wait_pid_gone};

/// CLI 名 → 协议（与 factory 注册形态一致：codex=app-server，
/// claude=stream-json）。
fn protocol_for(cli: &str) -> Option<DelegateProtocolKind> {
    match cli {
        "codex" => Some(DelegateProtocolKind::CodexAppServer),
        "claude" => Some(DelegateProtocolKind::ClaudeStreamJson),
        _ => None,
    }
}

fn default_cwd() -> std::path::PathBuf {
    let base = std::env::temp_dir().join("verify-probe-delegate");
    let _ = std::fs::create_dir_all(&base);
    base
}

/// 打印 auth 信息面（不改变退出码；子命令不存在/输出不认 = 原文照登）。
async fn auth_info(cli: &str, cli_path: &str) {
    let sub: &[&str] = match cli {
        "codex" => &["login", "status"],
        // claude CLI 无稳定 auth status 子命令——跳过信息面。
        _ => return,
    };
    let out = tokio::process::Command::new(cli_path)
        .args(sub)
        .output()
        .await;
    match out {
        Ok(o) => {
            println!("-- auth 信息面（{cli} {}）--", sub.join(" "));
            println!(
                "exit: {}",
                o.status.code().map(|c| c.to_string()).unwrap_or("?".into())
            );
            print!("stdout: {}", String::from_utf8_lossy(&o.stdout));
            print!("stderr: {}", String::from_utf8_lossy(&o.stderr));
        }
        Err(e) => println!("-- auth 信息面执行失败: {e}（原文照登，不猜结论）--"),
    }
}

/// delegate 腿预检。退出码：0 = CLI 在场（腿可跑）；3 = CLI 不在 SKIP。
pub async fn probe(cli: &str) -> i32 {
    let Some(protocol) = protocol_for(cli) else {
        eprintln!("未知 CLI '{cli}'（支持 codex / claude）");
        return EXIT_FAIL;
    };
    match find_cli_on_path(cli) {
        Some(path) => println!("cli_path: {path}"),
        None => {
            println!("SKIP: {cli} CLI 不在 PATH（factory 同源探测 find_cli_on_path 判定）");
            return EXIT_SKIP;
        }
    }
    // 版本信息面（尽力而为，失败不降级——版本拿不到不算腿失败）。
    let cli_path = find_cli_on_path(cli).unwrap_or_else(|| cli.to_string());
    if let Ok(o) = tokio::process::Command::new(&cli_path)
        .arg("--version")
        .output()
        .await
    {
        print!(
            "-- version --\n{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let _ = protocol; // 协议映射在 probe 阶段只做合法性校验（上面已完成）。
    auth_info(cli, &cli_path).await;
    EXIT_PASS
}

/// delegate 腿验收：真实 prompt 往返 + stop 树杀无孤儿。
pub async fn roundtrip(cli: &str, cwd: Option<&str>, wait_secs: u64, marker: &str) -> i32 {
    let Some(protocol) = protocol_for(cli) else {
        eprintln!("未知 CLI '{cli}'（支持 codex / claude）");
        return EXIT_FAIL;
    };
    let Some(cli_path) = find_cli_on_path(cli) else {
        println!("SKIP: {cli} CLI 不在 PATH（先跑 delegate-probe 留痕）");
        return EXIT_SKIP;
    };
    let cwd = cwd
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_cwd);

    let manager = std::sync::Arc::new(DelegateManager::new());
    let estop = std::sync::Arc::new(EstopState::new());
    manager.attach_estop(estop);

    let prompt = format!("请原样输出这个标记（不要加任何其他文字）：{marker}");
    let started = manager
        .start(
            protocol,
            &cli_path,
            Some(&prompt),
            &cwd.to_string_lossy(),
            None,
            "", // permission_mode 缺省 = CLI 默认（与生产缺省一致）
            HashMap::new(),
            Duration::from_secs(1800),
        )
        .await;
    let start_info = match started {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "FAIL: start 失败（CLI 在场但启动/握手失败——auth 或环境问题，现场如下）：\n{e}"
            );
            return EXIT_FAIL;
        }
    };
    let id = start_info["delegate_id"].as_u64().unwrap_or(0);
    let pid = start_info["pid"].as_u64().unwrap_or(0) as u32;
    println!("delegate_id: {id}, pid: {pid}");

    // 轮询 assistant 输出直到含标记或超时。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
    let mut hit = false;
    let mut tail = String::new();
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        match manager.output(id, 0).await {
            Ok(out) => {
                let text = out["assistant"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let char_count = text.chars().count();
                tail = text.chars().skip(char_count.saturating_sub(800)).collect();
                if text.contains(marker) {
                    hit = true;
                    break;
                }
            }
            Err(e) => {
                eprintln!("FAIL: output 查询失败（条目可能中途死掉）：\n{e}");
                break;
            }
        }
    }
    if !hit {
        eprintln!("FAIL: {wait_secs}s 内 assistant 输出未含回声标记 '{marker}'；输出尾：\n{tail}");
        // 收尾仍要 stop（不留孤儿再 FAIL 一层）。
        let _ = manager.stop(id).await;
        if pid != 0 {
            let _ = wait_pid_gone(pid, 15).await;
        }
        return EXIT_FAIL;
    }
    println!("roundtrip: 标记回声命中（{wait_secs}s 窗口内）");

    // stop → pid 收尸断言。
    let stopped = manager.stop(id).await;
    match stopped {
        Ok(v) => println!("stop: {v}"),
        Err(e) => eprintln!("WARN: stop 返回错误（收尸断言仍继续）：{e}"),
    }
    if pid != 0
        && let Err(e) = wait_pid_gone(pid, 15).await
    {
        eprintln!("FAIL: {e}");
        return EXIT_FAIL;
    }
    println!("PASS: 往返命中 + stop 后无孤儿进程");
    EXIT_PASS
}

/// delegate 腿验收：空闲会话 + 半程急停（零 token：不发 prompt）。
pub async fn estop(cli: &str, cwd: Option<&str>, wait_secs: u64) -> i32 {
    let Some(protocol) = protocol_for(cli) else {
        eprintln!("未知 CLI '{cli}'（支持 codex / claude）");
        return EXIT_FAIL;
    };
    let Some(cli_path) = find_cli_on_path(cli) else {
        println!("SKIP: {cli} CLI 不在 PATH（先跑 delegate-probe 留痕）");
        return EXIT_SKIP;
    };
    let cwd = cwd
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_cwd);

    let manager = std::sync::Arc::new(DelegateManager::new());
    let estop = std::sync::Arc::new(EstopState::new());
    manager.attach_estop(estop.clone());

    let start_info = match manager
        .start(
            protocol,
            &cli_path,
            None, // 空闲会话：stdin 挂起等 steer，不烧 token
            &cwd.to_string_lossy(),
            None,
            "",
            HashMap::new(),
            Duration::from_secs(1800),
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: 空闲 start 失败：\n{e}");
            return EXIT_FAIL;
        }
    };
    let id = start_info["delegate_id"].as_u64().unwrap_or(0);
    let pid = start_info["pid"].as_u64().unwrap_or(0) as u32;
    println!("delegate_id: {id}, pid: {pid}");

    // 半程急停：watcher 应触发 stop_all 排空（含本条目）。
    estop.trigger();
    println!("estop: 已触发，等待条目停转 + pid 收尸…");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
    let mut stopped_seen = false;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        match manager.output(id, 0).await {
            Ok(out) => {
                if out["running"].as_bool() == Some(false) {
                    stopped_seen = true;
                    break;
                }
            }
            // 条目已被 stop_all 摘表 = 也是一种排空证据（不算失败）。
            Err(_) => {
                stopped_seen = true;
                break;
            }
        }
    }
    if !stopped_seen {
        eprintln!("FAIL: 急停后 {wait_secs}s 条目仍 running=true（stop_all 未排空）");
        // 兜底收尸，不留孤儿。
        estop.release();
        let _ = manager.stop(id).await;
        if pid != 0 {
            let _ = wait_pid_gone(pid, 15).await;
        }
        return EXIT_FAIL;
    }
    if pid != 0
        && let Err(e) = wait_pid_gone(pid, 15).await
    {
        eprintln!("FAIL: {e}");
        return EXIT_FAIL;
    }
    // 探针自身的 estop 态对进程内 manager 有意义，但探针即将退出——无需
    // release（不存在跨用例复用）。
    println!("PASS: 急停排空 + 无孤儿进程");
    EXIT_PASS
}
