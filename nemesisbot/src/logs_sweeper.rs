//! M7/M8（2026-10-27 整改批次三）：logs 五族统一 TTL 清扫器。
//!
//! 五族日志此前零生命周期管理——session_logs / boundary / request_logs /
//! terminal / event_ledger 只写不删，长跑实例磁盘占用单调增长。本模块按
//! `config.logs`（[`nemesis_config::LogsTtlConfig`]）逐族保留天数清扫：
//! 启动扫一次 + 每 6h 一拍（与 uploads sweeper 同拍），每族每拍限量
//! [`MAX_DELETIONS_PER_SWEEP`] 件防 IO 尖峰（扫不完下次周期继续）。
//!
//! 族语义要点：
//! - **request_logs** 的条目是 `{ts}_{rand}/` 会话**目录**（内含多轮 LLM
//!   请求/响应文件），按目录 mtime 整目录删；其余四族是平铺文件。
//! - **session_logs** 删 `{key}.jsonl` 时**配对同删** `{key}.meta.json`
//!   （不看 sidecar 自身年龄）——只删正文留 sidecar 会在 Logs 页产生
//!   幽灵会话条目。
//! - **event_ledger** 删除条件 = 超龄 **且 末行 op==append**：truncate/fork
//!   行携带被删行全文 = chat_log 的唯一旁证，末行非 append 的账本保留
//!   （存量靠 TTL 自然老化，不做人工清账）。
//! - **terminal** 含回显密码（审计文件头有诚实注记），敏感面最短（默认 7d）。
//!
//! 删除一律走 [`nemesis_utils::fsutil`] 韧性包装（Windows AV/索引器瞬时
//! 句柄是常态）；目录不存在 / 读不到 mtime / 末行不可解析 = 诚实跳过，
//! 不炸不清。`logs.ttl` 全 0 = 完全不装配清扫任务。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::info;

/// 每族每拍删除上限（防 IO 尖峰；一次扫不完下次周期继续）。
const MAX_DELETIONS_PER_SWEEP: usize = 200;

/// 清扫周期间隔（6h，与 uploads TTL sweeper 同拍）。
const SWEEP_INTERVAL_SECS: u64 = 6 * 3600;

/// 一天的秒数。
const DAY_SECS: u64 = 86400;

/// 五族 TTL 快照（gateway 装配期从 config 读一次；改 config 重启生效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogsTtl {
    /// `logs/session_logs/` 保留天数（0=族关）。
    pub session_logs: u64,
    /// `logs/boundary/` 保留天数（0=族关）。
    pub boundary: u64,
    /// `logs/request_logs/` 保留天数（0=族关）。
    pub request_logs: u64,
    /// `logs/terminal/` 保留天数（0=族关）。
    pub terminal: u64,
    /// `logs/event_ledger/` 保留天数（0=族关）。
    pub event_ledger: u64,
}

impl Default for LogsTtl {
    fn default() -> Self {
        // 默认值与 LogsTtlConfig::default 同源口径（terminal 含回显密码，
        // 敏感面最短）。
        Self {
            session_logs: 30,
            boundary: 30,
            request_logs: 14,
            terminal: 7,
            event_ledger: 30,
        }
    }
}

impl LogsTtl {
    /// 从 config `logs` 段取值；None（缺键）= 全默认。
    pub fn from_config(cfg: Option<&nemesis_config::LogsTtlConfig>) -> Self {
        match cfg {
            Some(c) => Self {
                session_logs: c.session_logs,
                boundary: c.boundary,
                request_logs: c.request_logs,
                terminal: c.terminal,
                event_ledger: c.event_ledger,
            },
            None => Self::default(),
        }
    }

    /// 五族全 0 = 完全不装配清扫任务。
    pub fn all_disabled(&self) -> bool {
        self.session_logs == 0
            && self.boundary == 0
            && self.request_logs == 0
            && self.terminal == 0
            && self.event_ledger == 0
    }
}

/// 族判别（决定条目是文件还是会话目录 + 特殊删除条件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    SessionLogs,
    Boundary,
    RequestLogs,
    Terminal,
    EventLedger,
}

impl Family {
    fn name(&self) -> &'static str {
        match self {
            Family::SessionLogs => "session_logs",
            Family::Boundary => "boundary",
            Family::RequestLogs => "request_logs",
            Family::Terminal => "terminal",
            Family::EventLedger => "event_ledger",
        }
    }

    /// request_logs 的条目是会话目录（整目录删）；其余族是文件。
    fn entries_are_dirs(&self) -> bool {
        matches!(self, Family::RequestLogs)
    }
}

/// 单族单拍清扫统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepStats {
    /// 本拍删除件数（含 session_logs 的 .meta.json 配对同删）。
    pub deleted: u64,
    /// 删除重试耗尽仍残留的件数（诚实计数，交下拍重试）。
    pub failed: u64,
    /// 释放字节（会话目录按内部文件累计）。
    pub bytes_freed: u64,
}

/// 纯函数：mtime 距 `now` 是否超 `ttl_days`（`ttl_days=0` = 族关，恒否）。
fn is_expired(mtime: SystemTime, ttl_days: u64, now: SystemTime) -> bool {
    if ttl_days == 0 {
        return false;
    }
    now.duration_since(mtime)
        .map(|age| age.as_secs() >= ttl_days * DAY_SECS)
        .unwrap_or(false)
}

/// 账本文件末行 op 探针（LedgerEntry 私有类型——本地最小 serde 形态；
/// LedgerEntry 序列化时 body flatten，`op` 在顶层）。
#[derive(serde::Deserialize)]
struct LedgerOpProbe {
    #[serde(default)]
    op: String,
}

/// 纯函数：账本字节流末个非空行的 op 是否 `append`。
///
/// 末行是 truncate/fork / 不可解析 / 文件为空 → false（诚实保留——
/// truncate 行携带 chat_log 的唯一旁证正文，宁可多留不可误删）。
fn ledger_tail_is_append(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let Some(last) = text.lines().rev().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    serde_json::from_str::<LedgerOpProbe>(last)
        .map(|p| p.op == "append")
        .unwrap_or(false)
}

/// 会话目录字节量（request_logs 条目内部文件累计；读不到的项跳过——
/// 尺寸只服务 INFO 日志，读失败不值得让清扫失败）。
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// 单族单拍：列目录 → 按名字排序（read_dir 顺序是平台实现细节，显式排）
/// → 逐条验龄 → 过限即删（fsutil 韧性包装）→ 超限量即止。
fn sweep_family(dir: &Path, ttl_days: u64, family: Family, now: SystemTime) -> SweepStats {
    let mut stats = SweepStats::default();
    if ttl_days == 0 {
        return stats; // 族关：零触碰（连目录都不读）
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return stats; // 目录不存在 = 无可清扫（诚实静默，不为缺目录刷日志）
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if stats.deleted >= MAX_DELETIONS_PER_SWEEP as u64 {
            break;
        }
        // 条目年龄：读不到 mtime = 跳过（保留，诚实——不确定的东西不动）。
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        if !is_expired(mtime, ttl_days, now) {
            continue;
        }
        if family.entries_are_dirs() {
            // 会话目录：整目录删。尺寸先算（删完就问不到了）。
            let size = dir_size(&path);
            match nemesis_utils::fsutil::remove_dir_all_retry(&path) {
                Ok(()) => {
                    stats.deleted += 1;
                    stats.bytes_freed += size;
                }
                Err(e) => {
                    stats.failed += 1;
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[LogsSweeper] {} 会话目录删除重试耗尽（残留交下拍重试）",
                        family.name()
                    );
                }
            }
            continue;
        }
        // 平铺文件。event_ledger 追加条件：末行非 append（truncate/fork
        // 旁证）= 保留。
        if family == Family::EventLedger {
            let Ok(bytes) = std::fs::read(&path) else {
                continue; // 读不了 = 保留
            };
            if !ledger_tail_is_append(&bytes) {
                continue;
            }
        }
        let size = meta.len();
        match nemesis_utils::fsutil::remove_file_retry(&path) {
            Ok(()) => {
                stats.deleted += 1;
                stats.bytes_freed += size;
                // session_logs：正文删除时配对同删 .meta.json sidecar
                //（不看 sidecar 自身年龄——幽灵会话条目防的正是它比正文新）。
                // 先 metadata 探存在（fsutil 对 NotFound 返 Ok，直接删会让
                // 计数把「本就没有 sidecar」也记成一刀）。
                if family == Family::SessionLogs
                    && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                {
                    let meta_sidecar = path.with_file_name(format!("{stem}.meta.json"));
                    if let Ok(sidecar_size) = std::fs::metadata(&meta_sidecar)
                        && nemesis_utils::fsutil::remove_file_retry(&meta_sidecar).is_ok()
                    {
                        // 配对件计入统计但不受限量卡（正文已删，
                        // sidecar 必须跟走，否则本拍就产生幽灵）。
                        stats.deleted += 1;
                        stats.bytes_freed += sidecar_size.len();
                    }
                }
            }
            Err(e) => {
                stats.failed += 1;
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "[LogsSweeper] {} 文件删除重试耗尽（残留交下拍重试）",
                    family.name()
                );
            }
        }
    }
    stats
}

/// 五族全量扫一轮（纯文件系统操作，无全局态——测试直接打临时 workspace）。
/// 返回每族统计（零删除的族也在列，调用方决定日志口径）。
fn sweep_workspace_logs(ws: &Path, ttl: &LogsTtl, now: SystemTime) -> Vec<(Family, SweepStats)> {
    let logs_root = nemesis_path::logs_dir_in_workspace(ws);
    let session_logs_dir = nemesis_path::resolve_session_logs_dir_in_workspace(ws);
    let boundary_dir = nemesis_path::resolve_boundary_events_dir_in_workspace(ws);
    let request_logs_dir = nemesis_path::resolve_request_logs_dir_in_workspace(ws);
    // terminal / event_ledger 无独立 resolver（pty.rs / event_ledger.rs
    // 各自 join）——这里与写端同构拼装，路径真相源仍是 logs 根 + 固定段。
    let terminal_dir = logs_root.join("terminal");
    let event_ledger_dir = logs_root.join("event_ledger");
    [
        (Family::SessionLogs, session_logs_dir, ttl.session_logs),
        (Family::Boundary, boundary_dir, ttl.boundary),
        (Family::RequestLogs, request_logs_dir, ttl.request_logs),
        (Family::Terminal, terminal_dir, ttl.terminal),
        (Family::EventLedger, event_ledger_dir, ttl.event_ledger),
    ]
    .into_iter()
    .map(|(family, dir, days)| (family, sweep_family(&dir, days, family, now)))
    .collect()
}

/// 单次清扫入口（生产路径）：默认 workspace + 当前时钟，有动作才打 INFO。
fn run_sweep_once(ttl: &LogsTtl) {
    let ws = nemesis_path::default_path_manager().workspace();
    let now = SystemTime::now();
    for (family, stats) in sweep_workspace_logs(&ws, ttl, now) {
        if stats.deleted > 0 {
            info!(
                family = family.name(),
                deleted = stats.deleted,
                failed = stats.failed,
                bytes_freed = stats.bytes_freed,
                ttl_days = match family {
                    Family::SessionLogs => ttl.session_logs,
                    Family::Boundary => ttl.boundary,
                    Family::RequestLogs => ttl.request_logs,
                    Family::Terminal => ttl.terminal,
                    Family::EventLedger => ttl.event_ledger,
                },
                "[LogsSweeper] 清扫完成"
            );
        }
        if stats.failed > 0 {
            tracing::warn!(
                family = family.name(),
                failed = stats.failed,
                "[LogsSweeper] 有件数删除重试耗尽（残留交下拍重试）"
            );
        }
    }
}

/// 装配清扫任务：启动扫一次 + 每 6h 一拍。全 0 = 不装配（info 一句收场）。
pub fn spawn_logs_sweeper(ttl: LogsTtl) {
    if ttl.all_disabled() {
        info!("[LogsSweeper] logs.ttl 五族全 0 —— 清扫不装配");
        return;
    }
    info!(
        session_logs_days = ttl.session_logs,
        boundary_days = ttl.boundary,
        request_logs_days = ttl.request_logs,
        terminal_days = ttl.terminal,
        event_ledger_days = ttl.event_ledger,
        interval_secs = SWEEP_INTERVAL_SECS,
        "[LogsSweeper] logs 五族 TTL 清扫装配"
    );
    tokio::spawn(async move {
        run_sweep_once(&ttl);
        let mut interval = tokio::time::interval(Duration::from_secs(SWEEP_INTERVAL_SECS));
        interval.tick().await; // 首 tick 立即返回，跳过（启动已扫过）
        loop {
            interval.tick().await;
            run_sweep_once(&ttl);
        }
    });
}

#[cfg(test)]
mod tests;
