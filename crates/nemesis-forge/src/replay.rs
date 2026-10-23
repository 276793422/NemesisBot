//! 失败重放回归（自进化升级 C 线，2026-10-14）。
//!
//! turn WAL 失败记录 = 真实失败信号源：`interrupted`（轮次中断）+
//! `finished` 且 `ok:false`（工具失败，WAL 从不写 "error" 态——提取层
//! 归一显示为 "error"）。提取 → 用例化（模板：原任务重跑 + 执行零错误
//! 隐式断言）→ 入库。
//!
//! 入库开关（v3.1 裁决）：`config.forge.json` ForgeConfig.`replay_auto_ingest`
//! —— 默认 **false = 需评审**（进 `<workspace>/forge/replay/pending.json`
//! 待审区，Dashboard/CLI 确认后转 `confirmed.json` 进回归集）；true = 自动
//! 直入 confirmed。执行侧（进程内 AgentLoop 装配）在集成层（nemesisbot），
//! 本模块只承担纯文件面的提取/用例化/入库/确认/列举——CLI 与 WSAPI 同源。
//!
//! 重放语义：原任务重跑，执行零错误即过（`skill_tests::evaluate_case` 对
//! transcript.errors 隐式判失败——无需显式断言）。

use std::path::{Path, PathBuf};

use crate::skill_tests::{TestCase, TestExpectations, TestTurn};

/// WAL 失败记录（提取产物）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct FailureRecord {
    pub session_key: String,
    pub tool: String,
    pub user: String,
    pub state: String,
    pub ts_ms: u64,
}

/// 扫 turn WAL 目录，提取失败记录（按 session+tool 去重，保留最新）。
///
/// 扫描范围含**轮换代**：`wal.jsonl` 写满轮换为 `wal.jsonl.old`（turn_wal
/// 单点轮换语义），只扫当前代会漏掉轮换点前最近一批失败（四路评审 C-F3
/// 根修）。跨代去重按 ts 保最新——旧代与当前代的重叠记录天然合并。
pub fn extract_failures(workspace: &Path) -> Result<Vec<FailureRecord>, String> {
    let wal_dir = workspace.join("logs").join("turn_wal");
    let mut out: Vec<(String, FailureRecord)> = Vec::new();
    let entries = match std::fs::read_dir(&wal_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()), // 无 WAL = 无失败（常态）
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(name.ends_with(".jsonl") || name.ends_with(".jsonl.old")) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in content.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("");
            let ok = v.get("ok").and_then(|b| b.as_bool());
            // WAL 词表只有 admitted/started/finished/interrupted——**从不写
            // "error"**：工具失败落为 finished + ok:false（tool_dispatch 的
            // wal_ok 判定），是最高频的失败类；interrupted = 轮次中断。
            // 首版过滤 `interrupted|error` 把全部工具失败漏在网外（二轮
            // 复查根修）。state 归一显示：失败工具调用 = "error"，中断
            // 保持 "interrupted"。
            let (is_failure, state_display) = match state {
                "interrupted" => (true, "interrupted"),
                "finished" if ok == Some(false) => (true, "error"),
                _ => (false, ""),
            };
            if !is_failure {
                continue;
            }
            let session_key = v
                .get("session_key")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let tool = v
                .get("tool")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let user = v
                .get("user")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let ts_ms = v.get("ts_ms").and_then(|s| s.as_u64()).unwrap_or(0);
            let rec = FailureRecord {
                session_key: session_key.clone(),
                tool: tool.clone(),
                user,
                state: state_display.to_string(),
                ts_ms,
            };
            let key = format!("{session_key}|{tool}");
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, existing)) => {
                    if existing.ts_ms < ts_ms {
                        *existing = rec;
                    }
                }
                None => out.push((key, rec)),
            }
        }
    }
    let mut recs: Vec<FailureRecord> = out.into_iter().map(|(_, r)| r).collect();
    recs.sort_by_key(|r| std::cmp::Reverse(r.ts_ms));
    Ok(recs)
}

/// 失败记录 → 重放用例。重放语义：原任务重跑，执行零错误即过。
///
/// name 的会话段走 [`nemesis_utils::sanitize::sanitize_path_segment`]
/// 单一真相源（SAN-05）：超 80 字符自动带 FNV 短哈希后缀。本地旧实现
/// `take(24) + 非字母数字转 _` 是裸截断身份——cluster 会话键
/// `{node_id}/{chat}` 的节点 id 普遍 12+ 字符，前 24 字符全是公共前缀，
/// 同节点两个不同失败生成同名用例，`ingest` 按 name 幂等去重时第二条
/// 被静默丢弃（回归集漏检）。
pub fn to_test_case(rec: &FailureRecord) -> TestCase {
    let task = if rec.user.trim().is_empty() {
        format!(
            "继续上一次未完成的任务（上次在调用工具 {} 时失败，状态 {}）",
            rec.tool, rec.state
        )
    } else {
        rec.user.trim().to_string()
    };
    TestCase {
        name: format!(
            "replay-{}-{}",
            nemesis_utils::sanitize::sanitize_path_segment(&rec.session_key),
            nemesis_utils::sanitize::sanitize_path_segment(&rec.tool)
        ),
        turns: vec![TestTurn { user: task }],
        fixtures: vec![],
        expect: TestExpectations {
            max_llm_turns: Some(8),
            ..Default::default()
        },
        judge: None,
    }
}

fn replay_dir(workspace: &Path) -> PathBuf {
    workspace.join("forge").join("replay")
}

/// 读用例区文件。缺失 = Ok(空)；**损坏 = Err**（四路评审 C-F2 根修——
/// 静默当空会让 confirm/ingest 以空基线回写，整区用例被销毁）。
fn read_cases(file: &Path) -> Result<Vec<TestCase>, String> {
    let content = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", file.display())),
    };
    serde_json::from_str(&content).map_err(|e| format!("parse {}: {e}", file.display()))
}

fn write_cases(file: &Path, cases: &[TestCase]) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(cases).map_err(|e| format!("serialize: {e}"))?;
    nemesis_utils::write_file_atomic(&file.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| format!("write: {e}"))
}

/// 入库（尊重 auto_ingest 开关）：false = pending 待审区；true = confirmed。
/// 返回 (去向, 用例数)。
pub fn ingest(
    workspace: &Path,
    cases: &[TestCase],
    auto_ingest: bool,
) -> Result<(&'static str, usize), String> {
    let target = if auto_ingest {
        replay_dir(workspace).join("confirmed.json")
    } else {
        replay_dir(workspace).join("pending.json")
    };
    let mut existing = read_cases(&target)?;
    for c in cases {
        if !existing.iter().any(|e| e.name == c.name) {
            existing.push(c.clone());
        }
    }
    write_cases(&target, &existing)?;
    Ok((
        if auto_ingest { "confirmed" } else { "pending" },
        existing.len(),
    ))
}

/// 待审确认：pending → confirmed（按 name；全部 = None）。
///
/// **写序（四路评审 C-F1 根修）：confirmed 先写、pending 后写。** 中间
/// 崩溃的最坏形态 = 用例两区重名（name 幂等去重自愈），不再是
/// 「pending 已截、confirmed 未写」的整批销毁。
pub fn confirm(workspace: &Path, name: Option<&str>) -> Result<usize, String> {
    let pending_file = replay_dir(workspace).join("pending.json");
    let confirmed_file = replay_dir(workspace).join("confirmed.json");
    let pending = read_cases(&pending_file)?;
    let (kept, moving): (Vec<_>, Vec<_>) = pending
        .into_iter()
        .partition(|c| name.map(|n| c.name != n).unwrap_or(false));
    let moved_count = moving.len();
    if moved_count == 0 {
        return Ok(0);
    }
    let mut confirmed = read_cases(&confirmed_file)?;
    for c in moving {
        if !confirmed.iter().any(|e| e.name == c.name) {
            confirmed.push(c);
        }
    }
    write_cases(&confirmed_file, &confirmed)?;
    write_cases(&pending_file, &kept)?;
    Ok(moved_count)
}

/// 待审区/已入库用例全量（Dashboard/CLI 列表面）。区文件损坏 = Err
/// （同 [`read_cases`]，不静默当空）。
pub fn list_full(workspace: &Path) -> Result<(Vec<TestCase>, Vec<TestCase>), String> {
    Ok((
        read_cases(&replay_dir(workspace).join("pending.json"))?,
        read_cases(&replay_dir(workspace).join("confirmed.json"))?,
    ))
}

/// 列出两个区的用例名（CLI 简版）。
pub fn list(workspace: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let (pending, confirmed) = list_full(workspace)?;
    Ok((
        pending.into_iter().map(|c| c.name).collect(),
        confirmed.into_iter().map(|c| c.name).collect(),
    ))
}

/// 已确认回归集（执行侧输入）。区文件损坏 = Err（执行侧诚实失败，
/// 不静默跑空回归集）。
pub fn confirmed_cases(workspace: &Path) -> Result<Vec<TestCase>, String> {
    read_cases(&replay_dir(workspace).join("confirmed.json"))
}

#[cfg(test)]
mod tests;
