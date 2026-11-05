//! M7/M8（2026-10-27 整改批次三）：logs 统一 TTL 清扫器。
//!
//! 首批六族（session_logs / boundary / request_logs / terminal /
//! event_ledger / trash）此前零生命周期管理，只写不删，长跑实例磁盘占用
//! 单调增长。2026-11-06 横扫三轮补族：第一轮 **gateway / cluster_logs /
//! eval**（滚动日志无清理 / 集群请求日志无清理 / 评估报告无清理）；第二
//! 轮 **capture / dreaming / calibration / deliveries**（失败诊断捕获
//! / dreaming 报告 / 安全校准报告 / worker 交付 patch 留底）；第三轮
//! **heartbeat / ziwei / exec_checkpoints + cluster_logs legacy 平铺**
//! （心跳日志单文件不删 / 紫微诊断产物不删 / 交付门影子库崩溃孤儿 /
//! 二层清扫「意外平铺文件保守不动」规则漏掉的 `cluster_{date}.log`
//! 每日事件流）——十六族覆盖 logs/ 子树全部生产写方。本模块按
//! `config.logs`（[`nemesis_config::LogsTtlConfig`]）逐族保留天数清扫：
//! 启动扫一次 + 每 6h 一拍（与 uploads sweeper 同拍），每族每拍限量
//! [`MAX_DELETIONS_PER_SWEEP`] 件防 IO 尖峰（扫不完下次周期继续）。
//!
//! 族语义要点：
//! - **request_logs** 的条目是 `{ts}_{rand}/` 会话**目录**（内含多轮 LLM
//!   请求/响应文件），按目录 mtime 整目录删；**trash**（会话回收站）、
//!   **eval**（评估报告 `{ts}_{slug}/`）与 **calibration**（安全校准报告
//!   `{ts}/`）同构——条目是整目录，目录入站后不再变化，mtime ≈ 入站
//!   时刻；按名排序 = 按时刻升序，超限量时最老的先进刀。其余族是平铺
//!   文件。
//! - **cluster_logs** 与 **capture** 都是**双层**目录（前者
//!   `{device}/{ts_ms}_{task}/`，后者 `{session}/{ts}_{signal}/`）：
//!   TTL 锚必须在子目录层——父目录 mtime 随新条目入站持续刷新，按父
//!   目录龄删永远杀不掉活跃父目录的旧条目；子目录清空后的父目录空壳
//!   一并摘除（非空即败的 rmdir 是竞态护栏），父目录下的意外平铺文件
//!   不动——例外：cluster_logs 根下的 `cluster_{date}.log` 是 legacy
//!   每日事件流的已知形态（[`crate::commands::gateway::post_agent`] 与
//!   双层 observer 同目录装配，`cluster_log.rs` 按日轮换只建不删），
//!   按族 TTL 删（第一轮「保守不动」规则的漏网，第三轮补上）。
//! - **heartbeat**（`logs/heartbeat.log`）与 **ziwei** 的
//!   `notify.jsonl` 是 logs 根/子目录下的**已知单文件**：整文件按自身
//!   mtime 判龄删（写方 `create(true)` 打开，删除后自重建；前者逐写
//!   开合、后者逐次通知开合，无常驻句柄）。
//! - **ziwei** 族另扫 `reports/ziwei-verify/{profile}/` 与
//!   `reports/ziwei-recover/{profile}/` 两棵 profile 报告目录树（诊断
//!   件，按目录龄整删）；`reports/{dest}` 用户点名输出不动。
//! - **exec_checkpoints**（`logs/exec_checkpoints/{task_id}.git/`）是
//!   交付门影子库的**崩溃孤儿**专用族：正常路径全部即用即清
//!   （delivery_gate.rs cleanup），只有进程崩溃窗口残留；默认 1 天短闸
//!   只收孤儿，不与正常交付生命周期竞争。
//! - **deliveries** 是 worker 侧 patch 留底（`{task_id}.patch`，只写不
//!   读——wire JSON 已上行，A 端 deliveries 表自包含），平铺文件族。
//! - **session_logs** 删 `{key}.jsonl` 时**配对同删** `{key}.meta.json`
//!   （不看 sidecar 自身年龄）——只删正文留 sidecar 会在 Logs 页产生
//!   幽灵会话条目。
//! - **event_ledger** 删除条件 = 超龄 **且 末行 op==append**：truncate/fork
//!   行携带被删行全文 = chat_log 的唯一旁证，末行非 append 的账本保留
//!   （存量靠 TTL 自然老化，不做人工清账）。
//! - **terminal** 含回显密码（审计文件头有诚实注记），敏感面最短（默认 7d）。
//! - **trash** 默认 90d（拍板①：30 天太短）——回收站的存在意义就是「删了
//!   还能反悔」，保留期刻意长于所有日志族。条目删除走
//!   [`nemesis_agent::chat_trash::trash_purge_in`] 单点（与手动 purge 同一
//!   收敛）：清单在 = 先清 sessions/ 下 live todo 孤儿再删条目；直接整目录
//!   删会让修复前入站的旧形态条目（todo 副本不在条目内）把孤儿永久滞留。
//!
//! **刻意不清扫的 logs/ 子目录**（诚实边界，2026-11-06 三轮横扫裁决）：
//! `security_logs/`（Merkle 审计链，自动删=自毁证据链）、
//! `sandbox_denials.jsonl`（安全拒绝取证台账，同审计链口径）、
//! `skin_scripts.log`（皮肤脚本同意审计证据，同审计链口径）、`dossiers/`
//! （用户交付物）、board archive `timeline.jsonl`（同 dossiers 口径）、
//! `flows/`（flow 引擎「磁盘=真相」不变量）、
//! `checkpoints/` + `project_checkpoints/`（影子 git 库，「无 gc (v1)」
//! 已声名边界）、`uploads/`（自有 7d sweeper）、`board_assets/`（board
//! 评论引用的图像资产——board 行永续，删资产=评论残缺）、
//! `subagent_tasks/`（live 注册表单文件覆写，不增长）、`vm_sync/`
//! （per-project baseline.json 覆写，项目数有界）、`skills/.cache/`
//! （技能缓存，miss=重取，logs 域外有手动清理面）。logs 域外的裁决：
//! `workspace/ziwei/`（runs/snapshots 自带 TTL+容量清扫）、
//! `workspace/data/receipts/`（connector 可撤销凭据台账，删=破坏撤销
//! 链）、`workspace/.discipline/`（纪律豁免审计证据）、forge 三 store
//! （学习语料数据域，opt-in）、workflow 执行记录 / memory stores（数据
//! 域历史）、voice `.part`（断点续传自愈点）、`reports/{dest}`（用户
//! 点名输出 = 交付物口径）；自管有界面：exec_worker journal（自压实）、
//! vm_bridge 镜像构建日志（每次构建截断重写）、turn_wal（单代轮转 +
//! 7d 恢复 TTL）、workspace_lease 双文件（定长）、guest 浏览器日志
//! （OS /tmp 重启即清）。
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

/// 十六族 TTL 快照（gateway 装配期从 config 读一次；改 config 重启生效）。
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
    /// `logs/trash/sessions/`（会话回收站）保留天数（0=族关）。
    pub trash: u64,
    /// `logs/gateway/` 滚动日志保留天数（0=族关）。
    pub gateway: u64,
    /// `logs/cluster_logs/` 集群请求日志保留天数（0=族关；双层目录，
    /// TTL 锚在任务目录层）。
    pub cluster_logs: u64,
    /// `logs/eval/` 评估报告目录保留天数（0=族关）。
    pub eval: u64,
    /// `logs/capture/` 失败诊断捕获保留天数（0=族关；双层目录，
    /// TTL 锚在信号目录层）。
    pub capture: u64,
    /// `logs/dreaming/` dreaming 报告保留天数（0=族关）。
    pub dreaming: u64,
    /// `logs/calibration/` 安全校准报告目录保留天数（0=族关）。
    pub calibration: u64,
    /// `logs/deliveries/` worker 交付 patch 留底保留天数（0=族关）。
    pub deliveries: u64,
    /// `logs/heartbeat.log` 心跳日志单文件保留天数（0=族关；写方
    /// create(true) 逐写开合，删除后自重建）。
    pub heartbeat: u64,
    /// 紫微诊断产物保留天数（0=族关）：`logs/ziwei/notify.jsonl` 平铺 +
    /// `reports/ziwei-verify/{profile}/`、`reports/ziwei-recover/
    /// {profile}/` 目录族；`reports/{dest}` 用户点名输出不在射程。
    pub ziwei: u64,
    /// `logs/exec_checkpoints/{task_id}.git/` 交付门影子库崩溃孤儿保留
    /// 天数（0=族关；默认 1 天短闸——正常路径即用即清，此族只收崩溃
    /// 窗口残留）。
    pub exec_checkpoints: u64,
}

impl Default for LogsTtl {
    fn default() -> Self {
        // 默认值与 LogsTtlConfig::default 同源口径（terminal 含回显密码，
        // 敏感面最短；trash 拍板① 90 天——回收站的存在意义就是「删了还能
        // 反悔」，保留期刻意长于所有日志族）。
        Self {
            session_logs: 30,
            boundary: 30,
            request_logs: 14,
            terminal: 7,
            event_ledger: 30,
            trash: 90,
            gateway: 14,
            cluster_logs: 14,
            eval: 30,
            capture: 30,
            dreaming: 30,
            calibration: 30,
            deliveries: 30,
            heartbeat: 30,
            ziwei: 30,
            exec_checkpoints: 1,
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
                trash: c.trash,
                gateway: c.gateway,
                cluster_logs: c.cluster_logs,
                eval: c.eval,
                capture: c.capture,
                dreaming: c.dreaming,
                calibration: c.calibration,
                deliveries: c.deliveries,
                heartbeat: c.heartbeat,
                ziwei: c.ziwei,
                exec_checkpoints: c.exec_checkpoints,
            },
            None => Self::default(),
        }
    }

    /// 十六族全 0 = 完全不装配清扫任务。
    pub fn all_disabled(&self) -> bool {
        self.session_logs == 0
            && self.boundary == 0
            && self.request_logs == 0
            && self.terminal == 0
            && self.event_ledger == 0
            && self.trash == 0
            && self.gateway == 0
            && self.cluster_logs == 0
            && self.eval == 0
            && self.capture == 0
            && self.dreaming == 0
            && self.calibration == 0
            && self.deliveries == 0
            && self.heartbeat == 0
            && self.ziwei == 0
            && self.exec_checkpoints == 0
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
    Trash,
    Gateway,
    ClusterLogs,
    Eval,
    Capture,
    Dreaming,
    Calibration,
    Deliveries,
    Heartbeat,
    Ziwei,
    ExecCheckpoints,
}

impl Family {
    fn name(&self) -> &'static str {
        match self {
            Family::SessionLogs => "session_logs",
            Family::Boundary => "boundary",
            Family::RequestLogs => "request_logs",
            Family::Terminal => "terminal",
            Family::EventLedger => "event_ledger",
            Family::Trash => "trash",
            Family::Gateway => "gateway",
            Family::ClusterLogs => "cluster_logs",
            Family::Eval => "eval",
            Family::Capture => "capture",
            Family::Dreaming => "dreaming",
            Family::Calibration => "calibration",
            Family::Deliveries => "deliveries",
            Family::Heartbeat => "heartbeat",
            Family::Ziwei => "ziwei",
            Family::ExecCheckpoints => "exec_checkpoints",
        }
    }

    /// request_logs / trash / eval / calibration 的条目是目录（整目录删）；
    /// exec_checkpoints（`{task_id}.git/`）与 ziwei 的 profile 报告树
    /// （`{profile}/`）同构目录族（ziwei 的 notify.jsonl 走
    /// [`sweep_single_file`]，不经此判定）；其余族是文件（cluster_logs /
    /// capture 双层走专用函数，不经此判定）。
    fn entries_are_dirs(&self) -> bool {
        matches!(
            self,
            Family::RequestLogs
                | Family::Trash
                | Family::Eval
                | Family::Calibration
                | Family::Ziwei
                | Family::ExecCheckpoints
        )
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
///
/// `trash_loc` 仅 [`Family::Trash`] 消费：条目删除走
/// [`nemesis_agent::chat_trash::trash_purge_in`] 单点（先清 live todo 孤儿
/// 再删条目，与手动 purge 同语义）；None（非 trash 族）回落整目录直删。
fn sweep_family(
    dir: &Path,
    ttl_days: u64,
    family: Family,
    now: SystemTime,
    trash_loc: Option<&nemesis_agent::chat_trash::TrashLocations>,
) -> SweepStats {
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
            // Trash 族走 trash_purge_in 单点：绕过它会让 TTL 到期的旧形态
            // 条目（修复前入站、todo 副本不在条目内）把 sessions/ 下 live
            // 孤儿永久滞留——session_logs sidecar 配对同删的同构问题。
            // 清单缺失 = 原键不可信寻址，purge 内部跳过孤儿照删条目。
            // live todo 字节微小，不计入 bytes_freed。
            let res: Result<(), String> = if family == Family::Trash {
                match trash_loc.zip(path.file_name().and_then(|n| n.to_str())) {
                    Some((loc, name)) => nemesis_agent::chat_trash::trash_purge_in(loc, name),
                    None => nemesis_utils::fsutil::remove_dir_all_retry(&path)
                        .map_err(|e| e.to_string()),
                }
            } else {
                nemesis_utils::fsutil::remove_dir_all_retry(&path).map_err(|e| e.to_string())
            };
            match res {
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

/// 双层目录清扫（cluster_logs `{device}/{ts_ms}_{task}/` 与 capture
/// `{session}/{ts}_{signal}/` 同构共用）。
///
/// TTL 锚必须在**子目录层**：父目录 mtime 随新条目入站持续刷新，按父
/// 目录龄删会永远杀不掉活跃父目录的旧条目。子目录清空后父目录成空壳，
/// 一并摘除（列表页不积灰壳）；非空即败的 rmdir 是竞态护栏——摘除窗口
/// 内新条目落进来时诚实失败，新条目无恙。父目录下的意外平铺文件不动
/// （保守：未知形态不删）。
///
/// **ClusterLogs 族例外**（2026-11-06 三轮横扫）：根下的
/// `cluster_{date}.log` 是 legacy 每日事件流的**已知形态**
/// （`cluster_log.rs` 按日轮换只建不删，与双层 observer 同目录装配），
/// 按族 TTL 删；其余根级意外平铺文件维持保守不动。Capture 族根级文件
/// 一律不动。
fn sweep_two_level(dir: &Path, ttl_days: u64, now: SystemTime, family: Family) -> SweepStats {
    let mut stats = SweepStats::default();
    if ttl_days == 0 {
        return stats;
    }
    let Ok(parents) = std::fs::read_dir(dir) else {
        return stats;
    };
    let mut parent_paths: Vec<PathBuf> = parents.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    parent_paths.sort();
    for parent in parent_paths {
        if stats.deleted >= MAX_DELETIONS_PER_SWEEP as u64 {
            break;
        }
        if parent.is_file() {
            // 根级平铺文件：仅 ClusterLogs 族认领 legacy 每日事件流形态。
            if family == Family::ClusterLogs && is_legacy_cluster_daily_log(&parent) {
                let Ok(meta) = std::fs::metadata(&parent) else {
                    continue;
                };
                let Ok(mtime) = meta.modified() else {
                    continue;
                };
                if !is_expired(mtime, ttl_days, now) {
                    continue;
                }
                let size = meta.len();
                match nemesis_utils::fsutil::remove_file_retry(&parent) {
                    Ok(()) => {
                        stats.deleted += 1;
                        stats.bytes_freed += size;
                    }
                    Err(e) => {
                        stats.failed += 1;
                        tracing::warn!(
                            path = %parent.display(),
                            error = %e,
                            "[LogsSweeper] {} legacy 每日事件流删除重试耗尽（残留交下拍重试）",
                            family.name()
                        );
                    }
                }
            }
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&parent) else {
            continue;
        };
        let mut child_paths: Vec<PathBuf> =
            entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        child_paths.sort();
        for child in child_paths {
            if stats.deleted >= MAX_DELETIONS_PER_SWEEP as u64 {
                break;
            }
            if !child.is_dir() {
                continue;
            }
            // 子目录年龄：读不到 mtime = 跳过（保留，诚实）。
            let Ok(meta) = std::fs::metadata(&child) else {
                continue;
            };
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            if !is_expired(mtime, ttl_days, now) {
                continue;
            }
            let size = dir_size(&child);
            match nemesis_utils::fsutil::remove_dir_all_retry(&child) {
                Ok(()) => {
                    stats.deleted += 1;
                    stats.bytes_freed += size;
                }
                Err(e) => {
                    stats.failed += 1;
                    tracing::warn!(
                        path = %child.display(),
                        error = %e,
                        "[LogsSweeper] {} 子目录删除重试耗尽（残留交下拍重试）",
                        family.name()
                    );
                }
            }
        }
        // 空壳摘除：父目录已无条目时 rmdir（非空即败 = 竞态护栏）。
        // 失败 = 空壳留到下一拍（六小时后重试），不计 failed（无残留
        // 数据，只是名字壳）。内容口径：空壳判断用 read_dir 枚举。
        if dir_is_empty(&parent)
            && let Err(e) = nemesis_utils::fsutil::remove_dir_retry(&parent)
        {
            tracing::debug!(
                path = %parent.display(),
                error = %e,
                "[LogsSweeper] {} 父目录空壳摘除失败（下拍重试）",
                family.name()
            );
        }
    }
    stats
}

/// 目录是否无条目（空壳判断；读失败 = 非空保守）。
fn dir_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut rd| rd.next().is_none())
        .unwrap_or(false)
}

/// legacy 每日集群事件流文件名形态（`cluster_{YYYY-MM-DD}.log`，
/// `cluster_log.rs` 按日轮换；与双层 observer 同目录装配的唯一已知
/// 平铺形态）。
fn is_legacy_cluster_daily_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.starts_with("cluster_") && name.ends_with(".log")
}

/// 已知单文件族清扫（heartbeat.log / ziwei notify.jsonl 形态）：整文件
/// 按自身 mtime 判龄删，删除后写方 `create(true)` 自重建（两者都是逐写
/// 开合，无常驻句柄，删除不打断写方）。
fn sweep_single_file(path: &Path, ttl_days: u64, now: SystemTime, family: Family) -> SweepStats {
    let mut stats = SweepStats::default();
    if ttl_days == 0 {
        return stats; // 族关：零触碰
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return stats; // 文件不存在 = 无可清扫
    };
    let Ok(mtime) = meta.modified() else {
        return stats; // 读不到 mtime = 保留（诚实）
    };
    if !is_expired(mtime, ttl_days, now) {
        return stats;
    }
    let size = meta.len();
    match nemesis_utils::fsutil::remove_file_retry(path) {
        Ok(()) => {
            stats.deleted = 1;
            stats.bytes_freed = size;
        }
        Err(e) => {
            stats.failed = 1;
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "[LogsSweeper] {} 单文件删除重试耗尽（残留交下拍重试）",
                family.name()
            );
        }
    }
    stats
}

/// ziwei 族混合形态清扫（三根）：`logs/ziwei/notify.jsonl` 平铺单文件 +
/// `reports/ziwei-verify/{profile}/`、`reports/ziwei-recover/{profile}/`
/// 两棵 profile 诊断报告目录树（走 [`sweep_family`] 目录族语义）。
/// `reports/{dest}`（用户点名输出 = 交付物口径）不在射程。
fn sweep_ziwei(ws: &Path, ttl_days: u64, now: SystemTime) -> SweepStats {
    let mut stats = sweep_single_file(
        &nemesis_path::logs_dir_in_workspace(ws)
            .join("ziwei")
            .join("notify.jsonl"),
        ttl_days,
        now,
        Family::Ziwei,
    );
    for root in [
        ws.join("reports").join("ziwei-verify"),
        ws.join("reports").join("ziwei-recover"),
    ] {
        let part = sweep_family(&root, ttl_days, Family::Ziwei, now, None);
        stats.deleted += part.deleted;
        stats.failed += part.failed;
        stats.bytes_freed += part.bytes_freed;
    }
    stats
}

/// 十六族全量扫一轮（纯文件系统操作，无全局态——测试直接打临时 workspace）。
/// 返回每族统计（零删除的族也在列，调用方决定日志口径）。
fn sweep_workspace_logs(ws: &Path, ttl: &LogsTtl, now: SystemTime) -> Vec<(Family, SweepStats)> {
    let logs_root = nemesis_path::logs_dir_in_workspace(ws);
    let session_logs_dir = nemesis_path::resolve_session_logs_dir_in_workspace(ws);
    let boundary_dir = nemesis_path::resolve_boundary_events_dir_in_workspace(ws);
    let request_logs_dir = nemesis_path::resolve_request_logs_dir_in_workspace(ws);
    let trash_dir = nemesis_path::resolve_trash_sessions_dir_in_workspace(ws);
    // terminal / event_ledger / eval / capture / dreaming / calibration /
    // deliveries / exec_checkpoints 无独立 resolver（pty.rs / event_ledger.rs
    // / eval_worker.rs / capture_sink.rs / dreaming_job.rs /
    // security_calibrate.rs / delivery_gate.rs 各自 join）——这里与写端同构
    // 拼装，路径真相源仍是 logs 根 + 固定段。
    let terminal_dir = logs_root.join("terminal");
    let event_ledger_dir = logs_root.join("event_ledger");
    let eval_dir = logs_root.join("eval");
    let capture_dir = logs_root.join("capture");
    let dreaming_dir = logs_root.join("dreaming");
    let calibration_dir = logs_root.join("calibration");
    let deliveries_dir = logs_root.join("deliveries");
    let exec_checkpoints_dir = logs_root.join("exec_checkpoints");
    let gateway_dir = nemesis_path::resolve_gateway_logs_dir_in_workspace(ws);
    let cluster_logs_dir = nemesis_path::resolve_cluster_logs_dir_in_workspace(ws);
    // trash 族删除走 chat_trash::trash_purge_in 单点（与手动 purge 同一
    // 收敛：清单在 = 先清 sessions/ 下 live todo 孤儿再删条目）。寻址面
    // 用 with_dirs 按 ws 真相源拼装（sweeper 无全局态，测试打临时 ws），
    // 文件名格式与生产寻址同源。
    let trash_loc = nemesis_agent::chat_trash::TrashLocations::with_dirs(
        trash_dir.clone(),
        session_logs_dir.clone(),
        boundary_dir.clone(),
        nemesis_path::resolve_sessions_dir_in_workspace(ws),
    );
    let mut swept: Vec<(Family, SweepStats)> = [
        (Family::SessionLogs, session_logs_dir, ttl.session_logs),
        (Family::Boundary, boundary_dir, ttl.boundary),
        (Family::RequestLogs, request_logs_dir, ttl.request_logs),
        (Family::Terminal, terminal_dir, ttl.terminal),
        (Family::EventLedger, event_ledger_dir, ttl.event_ledger),
        (Family::Trash, trash_dir, ttl.trash),
        (Family::Gateway, gateway_dir, ttl.gateway),
        (Family::ClusterLogs, cluster_logs_dir, ttl.cluster_logs),
        (Family::Eval, eval_dir, ttl.eval),
        (Family::Capture, capture_dir, ttl.capture),
        (Family::Dreaming, dreaming_dir, ttl.dreaming),
        (Family::Calibration, calibration_dir, ttl.calibration),
        (Family::Deliveries, deliveries_dir, ttl.deliveries),
        (
            Family::ExecCheckpoints,
            exec_checkpoints_dir,
            ttl.exec_checkpoints,
        ),
    ]
    .into_iter()
    .map(|(family, dir, days)| {
        let stats = if family == Family::ClusterLogs || family == Family::Capture {
            sweep_two_level(&dir, days, now, family)
        } else {
            let loc = (family == Family::Trash).then_some(&trash_loc);
            sweep_family(&dir, days, family, now, loc)
        };
        (family, stats)
    })
    .collect();
    // 非目录形态的三族后补（单文件 / 混合根），保持十六族统计齐全。
    swept.push((
        Family::Heartbeat,
        sweep_single_file(
            &logs_root.join("heartbeat.log"),
            ttl.heartbeat,
            now,
            Family::Heartbeat,
        ),
    ));
    swept.push((Family::Ziwei, sweep_ziwei(ws, ttl.ziwei, now)));
    swept
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
                    Family::Trash => ttl.trash,
                    Family::Gateway => ttl.gateway,
                    Family::ClusterLogs => ttl.cluster_logs,
                    Family::Eval => ttl.eval,
                    Family::Capture => ttl.capture,
                    Family::Dreaming => ttl.dreaming,
                    Family::Calibration => ttl.calibration,
                    Family::Deliveries => ttl.deliveries,
                    Family::Heartbeat => ttl.heartbeat,
                    Family::Ziwei => ttl.ziwei,
                    Family::ExecCheckpoints => ttl.exec_checkpoints,
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
        info!("[LogsSweeper] logs.ttl 十六族全 0 —— 清扫不装配");
        return;
    }
    info!(
        session_logs_days = ttl.session_logs,
        boundary_days = ttl.boundary,
        request_logs_days = ttl.request_logs,
        terminal_days = ttl.terminal,
        event_ledger_days = ttl.event_ledger,
        trash_days = ttl.trash,
        gateway_days = ttl.gateway,
        cluster_logs_days = ttl.cluster_logs,
        eval_days = ttl.eval,
        capture_days = ttl.capture,
        dreaming_days = ttl.dreaming,
        calibration_days = ttl.calibration,
        deliveries_days = ttl.deliveries,
        heartbeat_days = ttl.heartbeat,
        ziwei_days = ttl.ziwei,
        exec_checkpoints_days = ttl.exec_checkpoints,
        interval_secs = SWEEP_INTERVAL_SECS,
        "[LogsSweeper] logs 十六族 TTL 清扫装配"
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
