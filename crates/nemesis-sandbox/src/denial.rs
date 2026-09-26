//! P21（2026-09-25 能力扩展 WS1）沙盒拒绝台账（denial ledger）。
//!
//! 沙盒层拒绝事件（landlock EACCES / bwrap 非零退出 / Sandboxie 拒绝 / 后续
//! Windows ACL denied）统一记结构化 JSONL 台账：
//! `<workspace>/logs/sandbox_denials.jsonl`。每行一个 [`DenialRecord`]：
//! 时间戳 / 后端 / 操作类型 / 目标路径或命令 / 原因码 / 是否已回灌模型。
//!
//! 定位（与安全审计链的关系）：台账是沙盒层**自有**观测面——不接安全 8 层
//! 管线的 Merkle 审计链（那条链照旧在 gateway dispatch 前记账）；台账记的
//! 是「沙盒装上之后仍被拦下来的事」，供 Dashboard 沙盒页与 `sandbox.denials.list`
//! 查询。台账故障**永不阻断工具执行**（append 失败 = warn + 原错误照常返回）。
//!
//! ## 写入侧（谁调 append）
//!
//! - `nemesisbot` executor 子进程（`exec_worker`）：工具错误分类为沙盒拒绝 →
//!   记台账 + 把工具结果改写为面向模型的可自纠文案（[`model_facing_text`]）；
//!   engage 严格模式拒绝、bwrap 包装的盒内实例非零退出 → 同账本。
//! - 后续 Windows ACL 档（P24）接同一写入面。
//!
//! ## 诚实边界
//!
//! - v1 无轮转/清扫：文件只增不减，查询侧 `read_denials(limit)` 截断；
//!   需要时人工删文件（下次写入自动重建）。
//! - [`looks_like_denial`] 是**启发式**（错误文本模式匹配），且只在沙盒
//!   engaged 时调用：沙盒开启期间的权限类错误也可能有非沙盒原因（如目标
//!   文件本身只读）——台账按「沙盒层嫌疑」记录，`reason` 保留原始错误文本
//!   供人工甄别，不做二次判断。
//! - 并发语义：每条记录独立 open(append)+write+close，单行远小于 POSIX
//!   `PIPE_BUF`（4KB）/ Windows `FILE_APPEND_DATA` 原子追加上限——多个
//!   executor 子进程并发写不撕裂（行级完整性由追加原子性保证）。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 台账文件名（相对 `<workspace>/logs/`）。
pub const LEDGER_FILE: &str = "sandbox_denials.jsonl";

/// 单条沙盒拒绝记录（JSONL 一行）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DenialRecord {
    /// RFC3339 UTC（毫秒精度，`2026-09-25T08:30:00.123Z` 形态；字典序 =
    /// 时间序）。
    pub ts: String,
    /// 后端：`landlock` / `bwrap` / `seatbelt` / `sandboxie` / `acl` /
    /// `none`（严格模式下无后端可用时的拒绝执行）。
    pub backend: String,
    /// 操作类型：工具名（`write_file` / `exec` …）或沙盒层事件
    /// （`sandbox_engage_refused` / `executor_reexec_failed`）。
    pub op: String,
    /// 目标路径或命令预览（截断，见 [`preview_target`]）。
    pub target: String,
    /// 原因码或原始错误摘录（截断——保留原文供人工甄别，不做二次归类）。
    pub reason: String,
    /// 拒绝文案是否已回灌模型（true = 工具结果 / 协议错误里带了面向模型的
    /// 可自纠文案；false = 模型看不到，如盒内实例退出时代理已无法回注）。
    pub model_visible: bool,
}

/// 目标 / 原因预览截断长度（按 char 计，防 UTF-8 边界劈半；截断后单行远
/// 小于追加原子性上限）。
const PREVIEW_MAX_CHARS: usize = 300;

/// `<workspace>/logs/sandbox_denials.jsonl` 的绝对路径。
pub fn ledger_path(workspace: &Path) -> PathBuf {
    workspace.join("logs").join(LEDGER_FILE)
}

/// 截断预览（按 char；超长补省略号）。公开给写入侧复用——模型文案与台账
/// 里的 target 必须同源同形（单一真相源）。
pub fn preview_target(s: &str) -> String {
    if s.chars().count() <= PREVIEW_MAX_CHARS {
        s.to_string()
    } else {
        let cut: String = s.chars().take(PREVIEW_MAX_CHARS).collect();
        format!("{cut}…")
    }
}

/// 构造一条记录（时间戳 now + 预览截断统一收口；写入侧只需填语义字段）。
pub fn new_record(
    backend: &str,
    op: &str,
    target: &str,
    reason: &str,
    model_visible: bool,
) -> DenialRecord {
    DenialRecord {
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        backend: backend.to_string(),
        op: op.to_string(),
        target: preview_target(target),
        reason: preview_target(reason),
        model_visible,
    }
}

/// 追加一条到台账。幂等/并发安全：独立 append 打开 + 单行写入（见模块文档
/// 并发语义）。返回 Err 时由调用方决定（生产路径 warn 放行，永不阻断执行）。
pub fn append_denial(workspace: &Path, record: &DenialRecord) -> std::io::Result<()> {
    let path = ledger_path(workspace);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut line = serde_json::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(line.as_bytes())?;
    f.flush()?;
    Ok(())
}

/// 读台账最近 `limit` 条（文件序 = 时间序，取文件**尾** `limit` 条）。文件
/// 缺失 = 空 vec；损坏行（半截写入 / 手工编辑弄坏）诚实跳过——不炸不整文件
/// 作废。WSAPI 层负责把结果反转为「最新在前」展示。
pub fn read_denials(workspace: &Path, limit: usize) -> Vec<DenialRecord> {
    let Ok(raw) = std::fs::read_to_string(ledger_path(workspace)) else {
        return Vec::new();
    };
    let mut parsed: Vec<DenialRecord> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<DenialRecord>(l).ok())
        .collect();
    if parsed.len() <= limit {
        parsed
    } else {
        parsed.split_off(parsed.len() - limit)
    }
}

/// 错误文本 → 是否沙盒层拒绝（启发式纯函数；Linux errno 文案与 Windows
/// Access-denied 文案都盖到）。**只在沙盒 engaged 时调用**（调用方约束，
/// 见模块文档诚实边界）。
pub fn looks_like_denial(err: &str) -> bool {
    const NEEDLES: [&str; 10] = [
        "os error 13",             // EACCES 数字形态（Rust io::Error Display）
        "permission denied",       // strerror(EACCES)
        "access is denied",        // Windows ERROR_ACCESS_DENIED 文案
        "eacces",                  // errno 符号名
        "eperm",                   // errno 符号名
        "os error 5",              // ERROR_ACCESS_DENIED 数字形态
        "operation not permitted", // strerror(EPERM)
        "denied by sandbox",       // 本仓沙盒层自产文案
        "sandbox",                 // 后端名/包装器名（sandboxie/bwrap 包装错误）
        "bwrap",                   // bwrap 非零退出前缀（bubblewrap 自报失败）
    ];
    let lower = err.to_lowercase();
    NEEDLES.iter().any(|n| lower.contains(n))
}

/// 面向模型的拒绝文案（可自纠：说明被什么拦了 + 模型下一步可以怎么办）。
/// 对齐仓库「错误文案写给模型看」哲学——工具结果里出现的是这段，而不是裸
/// EACCES。原文（reason）保留在文案里，模型/用户都能对账。
pub fn model_facing_text(
    backend: &str,
    op: &str,
    target: &str,
    reason: &str,
    workspace: &str,
) -> String {
    format!(
        "[沙盒拦截] 操作被 executor 沙盒（{backend}）拒绝，未在真实系统生效。\n\
         操作：{op}\n\
         目标：{target}\n\
         原因：{reason}\n\
         下一步（可自纠）：\n\
         1. 写入/修改类操作请把目标改到工作区内：{workspace}\n\
         2. 若确需工作区外写入或网络访问，请向用户说明理由，由用户调整 executor 安全配置（如 allow_network）或走审批；不要原样重试同一操作。"
    )
}
