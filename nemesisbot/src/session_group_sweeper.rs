//! B38（2026-11-08 七轮调研落地批次②）：残留会话组归档提示 sweep。
//!
//! L6++ 项目分组的两类临时态会话组此前只增不减：**灰组**（会话 sidecar
//! 带 `project_id`，但注册表已无该条目 = 项目被移除——「移除项目=仅解除
//! 分组」语义的产物）与 **inactive 组**（注册表条目还在，但项目目录已
//! 消失）。组本身对用户不可见（前端灰组垫底展示），残留久了既占 UI 又
//! 无从追溯。本 sweep **纯扫描 + 报告，零删除**（会话是用户数据，删除
//! 必须显式——计划文档「拒绝的设计」④钦定）：启动扫一次 + 每 6h 一拍
//! （与 logs/uploads sweeper 同拍），报告滚动覆盖写
//! `<workspace>/logs/maintenance/session-groups.json`，有残留组时 WARN
//! 一句。处置由用户显式进行（会话页逐会话删除 / 重建项目目录收编）。
//!
//! 判定口径（计划文档设计节）：
//! - 扫 `logs/session_logs/*.meta.json`，sidecar 带 `project_id` 的按其
//!   分组（无归属 = 对话组，不在射程）；
//! - 灰组 = `project_id` 无注册表条目；inactive 组 = 条目在但
//!   `entry.path` 目录消失（`is_dir()` 与项目 loop 启动检查同判定）；
//! - 组内**全部**会话正文（`.jsonl`）mtime 超过 [`ORPHAN_AGE_DAYS`] 才
//!   入报告——部分活跃的组不打扰；正文缺失（TTL 清扫配对竞态窗口）以
//!   sidecar 自身 mtime 判龄（sidecar 残留本身就是残留证据）；
//! - 任一会话 mtime 不可读 = 该组按「不超龄」处理（判不了就不提示，
//!   保守不误报）。
//!
//! 报告自包含：组类别 / 项目 id / 项目名与路径（注册表命中时）/ 会话
//! key 清单 / 最新 mtime。测试直接打 [`sweep_session_groups`]（纯函数，
//! 传 `now` 推进时钟——夹具 mtime 不可回拨，与 logs_sweeper 测试同手法）。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

/// 组内全部会话正文超此天数才入报告（部分活跃的组不打扰）。
const ORPHAN_AGE_DAYS: u64 = 30;

/// 清扫周期间隔（6h，与 logs/uploads sweeper 同拍）。
const SWEEP_INTERVAL_SECS: u64 = 6 * 3600;

/// 一天的秒数。
const DAY_SECS: u64 = 86400;

/// 单个残留会话组的发现（报告数组元素）。
#[derive(Debug, serde::Serialize)]
pub struct GroupFinding {
    /// `orphan` = 灰组（project_id 无注册表条目 = 项目已移除）；
    /// `inactive` = 注册表有条目但项目目录消失。
    pub kind: &'static str,
    pub project_id: String,
    /// 项目名（注册表命中时带；灰组无条目可查）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// 项目目录（注册表命中时带；灰组无条目可查）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// 组内会话数。
    pub session_count: usize,
    /// 组内最新正文 mtime（unix 秒；全部不可读 = None——但这形态组不会
    /// 入报告，见模块 doc 判定口径）。
    pub latest_mtime_secs: u64,
    /// 会话 key 清单（用户按名回查会话页）。
    pub sessions: Vec<String>,
}

/// 一次 sweep 的报告（滚动覆盖写 logs/maintenance/session-groups.json）。
#[derive(Debug, Default, serde::Serialize)]
pub struct SessionGroupsReport {
    /// 报告生成时刻（unix 秒）。
    pub generated_at_secs: u64,
    /// 扫到的带归属会话 meta 总数（不含对话组）。
    pub scanned_grouped_sessions: usize,
    /// 灰组（项目已移除）。
    pub orphan_groups: Vec<GroupFinding>,
    /// inactive 组（注册表在、目录消失）。
    pub inactive_groups: Vec<GroupFinding>,
}

impl SessionGroupsReport {
    /// 是否存在任何残留组（决定 WARN 与是否写盘——无残留也写盘，滚动
    /// 覆盖语义让「上次有、这次没有」可见）。
    fn has_findings(&self) -> bool {
        !self.orphan_groups.is_empty() || !self.inactive_groups.is_empty()
    }
}

/// 单个会话 sidecar 的扫捕中间态。
struct SessionAge {
    key: String,
    mtime_secs: u64,
    /// mtime 是否可信（不可信 = 按不超龄处理，组不被提示——保守）。
    judgeable: bool,
}

/// 残留会话组 sweep 核心（纯函数：无全局态、时钟入参——测试直接打）。
///
/// 读注册表 + 扫 sidecar + 分组判定；**零删除**（模块 doc 钦定）。
pub fn sweep_session_groups(workspace: &Path, now: SystemTime) -> SessionGroupsReport {
    let mut report = SessionGroupsReport {
        generated_at_secs: unix_secs(now),
        ..Default::default()
    };
    let registry = crate::projects::registry::load_projects_lenient(
        &crate::projects::registry::registry_path(workspace),
    );

    // project_id → (name, path)：注册表命中查表；重复 id 不可能（registry
    // 写入侧保证），末条目胜出的兜底语义与 list 展示一致。
    let mut entries: std::collections::HashMap<String, (String, PathBuf)> =
        std::collections::HashMap::new();
    for e in &registry.projects {
        entries.insert(e.id.clone(), (e.name.clone(), e.path.clone()));
    }

    let dir = nemesis_path::resolve_session_logs_dir_in_workspace(workspace);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return report; // 目录不存在 = 无会话历史，空报告
    };
    // read_dir 顺序是平台实现细节（NTFS 字典序 vs ext4 哈希序）——显式
    // 排序保证报告内会话清单稳定可 diff。
    let mut meta_paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".meta.json"))
        })
        .collect();
    meta_paths.sort();

    // project_id → 组（会话判龄集合）。
    let mut groups: std::collections::BTreeMap<String, Vec<SessionAge>> =
        std::collections::BTreeMap::new();
    let mut scanned = 0usize;
    for mp in meta_paths {
        let Ok(raw) = std::fs::read_to_string(&mp) else {
            continue; // 读失败 = 诚实跳过（瞬态句柄竞争不炸不误报）
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue; // 损坏 sidecar 不入组（不是判定对象）
        };
        let Some(project_id) = v
            .get("project_id")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
        else {
            continue; // 无归属 = 对话组，不在射程
        };
        scanned += 1;
        // 正文配对：sidecar 名 `{safe_key}.meta.json` → `{safe_key}.jsonl`；
        // 正文缺失（TTL 配对竞态窗口）以 sidecar 自身 mtime 判龄——残留
        // sidecar 本身就是残留证据。
        let body = mp.with_file_name(
            mp.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".meta.json"))
                .map(|s| format!("{s}.jsonl"))
                .unwrap_or_default(),
        );
        let (mtime_secs, judgeable) = match file_mtime_secs(&body).or_else(|| file_mtime_secs(&mp))
        {
            Some(s) => (s, true),
            None => (0, false), // mtime 不可读 = 判不了（组按不超龄处理）
        };
        groups.entry(project_id).or_default().push(SessionAge {
            key: session_key_of(&mp),
            mtime_secs,
            judgeable,
        });
    }
    report.scanned_grouped_sessions = scanned;

    let (orphans, inactives) = classify_groups(groups, &entries, now);
    report.orphan_groups = orphans;
    report.inactive_groups = inactives;
    report
}

/// 分组判定（纯函数：无文件系统——测试直接构造混合 mtime 的组打判定）。
/// 活跃组（条目在且目录在）跳过；灰组 / inactive 组内**全部**会话超龄
/// 才入报告；判不了的会话（judgeable=false）按不超龄处理（保守不误报）。
fn classify_groups(
    groups: std::collections::BTreeMap<String, Vec<SessionAge>>,
    entries: &std::collections::HashMap<String, (String, PathBuf)>,
    now: SystemTime,
) -> (Vec<GroupFinding>, Vec<GroupFinding>) {
    let mut orphans = Vec::new();
    let mut inactives = Vec::new();
    let age_gate_secs = ORPHAN_AGE_DAYS * DAY_SECS;
    for (project_id, sessions) in groups {
        let entry = entries.get(&project_id);
        // 组类别：灰组（无条目）/ inactive（条目在、目录消失）；条目在且
        // 目录在 = 活跃组，不打扰。
        let (kind, name, path) = match entry {
            None => ("orphan", None, None),
            Some((n, p)) => {
                if p.is_dir() {
                    continue;
                }
                ("inactive", Some(n.clone()), Some(p.display().to_string()))
            }
        };
        // 全部会话超龄才入报告：判不了的会话按不超龄处理（保守不误报）。
        let all_old = sessions
            .iter()
            .all(|s| s.judgeable && now_secs_gap(now, s.mtime_secs) >= age_gate_secs);
        if !all_old {
            continue;
        }
        let latest = sessions.iter().map(|s| s.mtime_secs).max().unwrap_or(0);
        let mut keys: Vec<String> = sessions.iter().map(|s| s.key.clone()).collect();
        keys.sort();
        let finding = GroupFinding {
            kind,
            project_id,
            project_name: name,
            project_path: path,
            session_count: sessions.len(),
            latest_mtime_secs: latest,
            sessions: keys,
        };
        if finding.kind == "orphan" {
            orphans.push(finding);
        } else {
            inactives.push(finding);
        }
    }
    (orphans, inactives)
}

/// sidecar 文件名 → 会话 key（去 `.meta.json` 后缀；sidecar 名是
/// sanitize 后的 key，报告展示用——无法逆 sanitize，按名回查会话页足够）。
fn session_key_of(meta_path: &Path) -> String {
    meta_path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".meta.json"))
        .unwrap_or_default()
        .to_string()
}

/// mtime → unix 秒（不可读 = None）。
fn file_mtime_secs(p: &Path) -> Option<u64> {
    let md = std::fs::metadata(p).ok()?;
    md.modified().ok().map(unix_secs)
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// now − mtime 的秒差（时钟回拨形态 mtime 在未来 = 负差 → 0 = 不超龄）。
fn now_secs_gap(now: SystemTime, mtime_secs: u64) -> u64 {
    unix_secs(now).saturating_sub(mtime_secs)
}

/// 报告滚动覆盖写（`<workspace>/logs/maintenance/session-groups.json`；
/// 原子写统一 helper）。写失败 warn 不炸 sweep（报告是提示面，不是数据）。
fn write_report(workspace: &Path, report: &SessionGroupsReport) {
    let path = nemesis_path::logs_dir_in_workspace(workspace)
        .join("maintenance")
        .join("session-groups.json");
    let Ok(body) = serde_json::to_vec_pretty(report) else {
        return;
    };
    if let Err(e) = nemesis_utils::write_file_atomic(
        &path.to_string_lossy(),
        &body,
        0o600, // Windows：helper 内部忽略 POSIX 权限（诚实边界，shared.rs 同形）
    ) {
        warn!("[SessionGroupSweeper] 报告写盘失败: {e}");
    }
}

/// 单拍入口（生产路径）：默认 workspace + 当前时钟；有残留组 WARN 一句。
fn run_sweep_once() {
    let ws = nemesis_path::default_path_manager().workspace();
    let report = sweep_session_groups(&ws, SystemTime::now());
    write_report(&ws, &report);
    if report.has_findings() {
        warn!(
            orphans = report.orphan_groups.len(),
            inactive = report.inactive_groups.len(),
            report = "logs/maintenance/session-groups.json",
            "[SessionGroupSweeper] 发现残留会话组（纯提示，零删除——\
             会话是用户数据，请在会话页或重建项目目录后自行处置）"
        );
    }
}

/// 装配：启动扫一次 + 每 6h 一拍（与 logs/uploads sweeper 同拍）。
pub fn spawn_session_group_sweeper() {
    info!(
        interval_secs = SWEEP_INTERVAL_SECS,
        age_days = ORPHAN_AGE_DAYS,
        "[SessionGroupSweeper] 残留会话组归档提示 sweep 装配（纯扫描零删除）"
    );
    tokio::spawn(async move {
        run_sweep_once();
        let mut interval = tokio::time::interval(Duration::from_secs(SWEEP_INTERVAL_SECS));
        interval.tick().await; // 首 tick 立即返回，跳过（启动已扫过）
        loop {
            interval.tick().await;
            run_sweep_once();
        }
    });
}

#[cfg(test)]
mod tests;
