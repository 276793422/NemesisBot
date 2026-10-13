//! L9 selfupdate 版本自切换通道（2026-10-13，三方学习总计划 T4 战略项）。
//!
//! 九步 apply 通道（docs/INFO/2026-10-12_LLM自更新通道设计方案.md §四）：
//! estop/降级/频率三闸 → CRITICAL 审批卡 → release_fetch 下载 → v4 验签
//! Valid-only → Windows rename staging → 受控重启（spawn 新进程等旧进程退场）
//! → 启动自检 → 回滚 → 审计链留痕。nemesis-verify 的第二个生产消费点
//!（第一个 = 启动自验 verify_policy）。
//!
//! 职责与门控：
//! - 本模块住 nemesis-web（release_fetch / 审批 trait 同库可达）；nemesisbot
//!   侧只做装配（gateway run_runtime 建 manager 注入 WSAPI 槽 + 内部命令臂）
//!   与 CLI 命令转发。
//! - `selfupdate` feature（= security + zip + nemesis-verify）门控 apply 编排
//!   （SelfUpdateManager）——IoT 裁剪构建可整体关掉更新通道；journal / fuse /
//!   check / boot 收口不门控（纯 fs + serde_json，recover_on_boot 在任何构建
//!   都要能清残留现场）。
//! - 版本身份 = **git commit**（nightly-build tag 恒定，semver 无意义）：
//!   当前进程短 commit（build.rs 注入）vs Release body 表行 / 包内 version.txt
//!   的全量 sha，`starts_with` 匹配。当前 commit 未知（开发构建）→ 降级闸
//!   要求显式 allow_downgrade。
//! - staging rename dance 与 Chrome/VS Code 同款：Windows 允许 rename 运行中
//!   的 PE、不允许覆盖——exe→.old、.new→exe 两步，窗口毫秒级；journal 先于
//!   rename 落盘，启动期 [`recover_on_boot`] 按相位收口。
//! - 非 Windows v1 诚实边界：主包 tar.gz 无解包依赖（tar/flate2 不在依赖树），
//!   apply 明确报「暂不支持」；check/status/boot 收口全平台可用。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 官方发布源主包资产名（daily-release.yml 产物命名，随平台）。
#[cfg(target_os = "windows")]
pub const TARGET_ASSET: &str = "nightly-windows.zip";
#[cfg(not(target_os = "windows"))]
pub const TARGET_ASSET: &str = "nightly-linux.tar.gz";

/// 更新包读取上限：300MB（全量构建 zip 百 MB 级，裁剪包更小；上限只防
/// 异常巨包，不按最坏包体抠）。
pub const MAX_UPDATE_ASSET_BYTES: usize = 300 * 1024 * 1024;

/// 频率保险丝窗口：24h 一次（防 update loop；审批通过后的失败尝试同样计数）。
pub const APPLY_FUSE_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// 审批卡等待时长（与插件安装卡同口径 300s）。
pub const APPLY_APPROVAL_TIMEOUT_SECS: u64 = 300;

/// 等旧进程退场超时（受控重启：新进程 spawn 后旧进程优雅停机窗口）。
pub const WAIT_OLD_PID_TIMEOUT_MILLIS: u64 = 120 * 1000;

// ---------------------------------------------------------------------------
// journal（崩溃恢复真相源）
// ---------------------------------------------------------------------------

/// staging/restart 期状态日志——**先于任何 rename 落盘**，boot 期按相位收口。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateJournal {
    /// `staging`（rename dance 进行中）| `restarting`（staging 完，等新进程接管）。
    pub phase: String,
    /// 目标 commit（全量 sha；空 = 未知目标，boot 自检必回滚）。
    pub target_commit: String,
    /// 目标版本串（人读留痕，不参与判定）。
    pub target_version: String,
    /// 旧 exe 的绝对路径（.old rename 后所在；回滚源）。
    pub old_path: String,
    pub started_ms: u64,
}

/// journal 现场路径（CLI status / 人工排障用同一路径）。
pub fn journal_path(workspace: &Path) -> PathBuf {
    workspace.join("state").join("selfupdate.json")
}

/// 频率保险丝记账路径。
pub fn fuse_path(workspace: &Path) -> PathBuf {
    workspace.join("state").join("selfupdate_last_attempt.txt")
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn load_journal(workspace: &Path) -> Option<UpdateJournal> {
    let bytes = std::fs::read(journal_path(workspace)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save_journal(workspace: &Path, journal: &UpdateJournal) -> Result<(), String> {
    let path = journal_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 state 目录失败：{e}"))?;
    }
    let bytes =
        serde_json::to_vec_pretty(journal).map_err(|e| format!("journal 序列化失败：{e}"))?;
    // 原子写（临时文件 + rename，防断电半行 JSON——boot 收口读半行 = None =
    // 当无事发生，staging 现场无人认领；原子写消掉这个缝）。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("journal 写入失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("journal 落盘失败：{e}"))
}

pub fn clear_journal(workspace: &Path) {
    let _ = std::fs::remove_file(journal_path(workspace));
}

/// 频率保险丝记账（上次 apply 尝试时刻）。独立于 journal——journal 成功路径
/// 会清，fuse 不可忘。
pub fn stamp_fuse(workspace: &Path) {
    let path = fuse_path(workspace);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, now_ms().to_string());
}

pub fn last_attempt_ms(workspace: &Path) -> Option<u64> {
    std::fs::read_to_string(fuse_path(workspace))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
}

// ---------------------------------------------------------------------------
// commit 身份
// ---------------------------------------------------------------------------

/// 当前 commit 是否命中目标 commit（短 sha 前缀匹配全量 sha）。任一为空 =
/// 未知身份 → false。
pub fn commit_matches(current_short: &str, target_full: &str) -> bool {
    !current_short.is_empty() && !target_full.is_empty() && target_full.starts_with(current_short)
}

/// 包内 version.txt 的 `Commit     : <sha>` 行提取（40 hex 全量）。
/// 「Commit Date : …」行因冒号前多一段不匹配，天然跳过。
pub fn extract_commit_from_version_txt(txt: &str) -> Option<String> {
    for line in txt.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Commit")
            .and_then(|r| r.trim_start().strip_prefix(':'))
        else {
            continue;
        };
        let sha = rest.trim().to_ascii_lowercase();
        if sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(sha);
        }
    }
    None
}

/// Release body 的 markdown 表行 `` | Commit | `<sha>` | `` 提取（check 用，
/// 免下 300MB 主包即可比对身份）。
pub fn extract_commit_from_release_body(body: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        if !(line.starts_with('|') && line.contains("Commit")) {
            continue;
        }
        // 反引号包裹段内找 hex 串。
        for seg in line.split('`') {
            let sha = seg.trim().to_ascii_lowercase();
            if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(sha);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// check（只读，全平台可用；不下载主包）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub current_commit: String,
    pub latest_tag: String,
    pub latest_name: String,
    /// Release body 表行里的全量 sha（body 缺失/格式变 = 空，诚实降级）。
    pub latest_commit: String,
    pub asset: String,
    pub asset_present: bool,
    /// true = 有比当前新的发布产物可更新。commit 双边可知时按 commit 判；
    /// 当前 commit 未知（current_commit_known=false）时按「包在场」判，
    /// 调用方措辞注明身份不明。
    pub update_available: bool,
    pub current_commit_known: bool,
}

pub async fn check(current_version: &str, current_commit: &str) -> Result<UpdateCheck, String> {
    let meta = crate::handlers::release_fetch::fetch_latest_release_meta().await?;
    let asset_present = meta.has_asset(TARGET_ASSET);
    let latest_commit = meta.body_commit().unwrap_or_default();
    let current_known = !current_commit.is_empty();
    // commit 双边可知 → 命中即无更新；latest_commit 空（body 无 sha，格式变）
    // → 不误报「已是最新」，包在场即报可更新（诚实边界）。
    let update_available = if current_known && !latest_commit.is_empty() {
        !commit_matches(current_commit, &latest_commit)
    } else {
        asset_present
    };
    Ok(UpdateCheck {
        current_version: current_version.to_string(),
        current_commit: current_commit.to_string(),
        latest_tag: meta.tag_name,
        latest_name: meta.name,
        latest_commit,
        asset: TARGET_ASSET.to_string(),
        asset_present,
        update_available,
        current_commit_known: current_known,
    })
}

// ---------------------------------------------------------------------------
// boot 收口（recover_on_boot）+ 旧进程等待（不门控：任何构建都要能清现场）
// ---------------------------------------------------------------------------

/// boot 收口结果（gateway 启动期 tracing 可见）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BootRecovery {
    /// none / wait_timeout / update_success / rolled_back
    pub action: String,
    pub detail: String,
}

/// 等 PID 退场（受控重启第一步：新进程让旧进程先走）。Windows 用 `tasklist`
/// （无 libc 依赖），Unix 用 `kill -0`。
pub fn wait_for_pid_exit(pid: u32, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if !pid_alive(pid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // tasklist /FI 过滤：进程不在时 stdout 无该 PID 的 CSV 行。
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")),
            // 探测失败按还在（保守：继续等，等价于旧进程未退场）。
            Err(_) => true,
        }
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(true)
    }
}

/// gateway 启动期收口（`gateway::run` 最早点调用，先于端口绑定）：
/// 1. env `NEMESISBOT_SELFUPDATE_WAIT_PID` 在场 → 等旧进程退场（受控重启
///    接力棒；超时继续启动——端口撞上由既有 bind 失败路径诚实报错）；
/// 2. journal 缺席 → 无事；
/// 3. phase=restarting：当前 commit 命中目标 → 更新成功，清 journal；
///    未命中（新包起不来/被换错文件）→ .old 回滚 + 清 journal；
/// 4. phase=staging：rename dance 中途死——.old 在场即回滚，清 journal。
pub fn recover_on_boot(workspace: &Path, current_commit: &str) -> BootRecovery {
    let exe = std::env::current_exe().unwrap_or_default();
    recover_on_boot_in(workspace, current_commit, &exe)
}

/// 内核（exe 路径参数化——单测注入临时路径，不碰测试二进制自身）。
fn recover_on_boot_in(workspace: &Path, current_commit: &str, exe: &Path) -> BootRecovery {
    // ① 旧进程退场等待（env 由 apply 的 respawn 注入）。
    if let Ok(old_pid) = std::env::var("NEMESISBOT_SELFUPDATE_WAIT_PID")
        && let Ok(pid) = old_pid.trim().parse::<u32>()
    {
        let t0 = std::time::Instant::now();
        if wait_for_pid_exit(pid, WAIT_OLD_PID_TIMEOUT_MILLIS) {
            tracing::info!(
                "[SelfUpdate] 旧进程已退场（等待 {}ms），接管启动",
                t0.elapsed().as_millis()
            );
        } else {
            tracing::warn!(
                pid,
                "[SelfUpdate] 等旧进程退场超时（{}s），继续启动（端口若被占由 bind 路径诚实报错）",
                WAIT_OLD_PID_TIMEOUT_MILLIS / 1000
            );
            return BootRecovery {
                action: "wait_timeout".to_string(),
                detail: format!("pid {pid} alive after {WAIT_OLD_PID_TIMEOUT_MILLIS}ms"),
            };
        }
        // 不 return——继续走 journal 收口（本进程可能就是更新后的新进程）。
    }

    let Some(journal) = load_journal(workspace) else {
        return BootRecovery {
            action: "none".to_string(),
            detail: String::new(),
        };
    };

    match journal.phase.as_str() {
        "restarting" => {
            if commit_matches(current_commit, &journal.target_commit) {
                clear_journal(workspace);
                tracing::info!(
                    "[SelfUpdate] 启动自检通过：当前 commit 命中目标 {}，更新收口",
                    &journal.target_commit[..journal.target_commit.len().min(12)]
                );
                BootRecovery {
                    action: "update_success".to_string(),
                    detail: format!("target {}", journal.target_commit),
                }
            } else {
                let detail = rollback_old_exe_to(&journal.old_path, exe);
                clear_journal(workspace);
                tracing::warn!(
                    "[SelfUpdate] 启动自检失败：当前 commit 未命中目标，已回滚（{detail}）"
                );
                BootRecovery {
                    action: "rolled_back".to_string(),
                    detail,
                }
            }
        }
        "staging" => {
            let detail = rollback_old_exe_to(&journal.old_path, exe);
            clear_journal(workspace);
            tracing::warn!("[SelfUpdate] staging 中断现场收口（{detail}）");
            BootRecovery {
                action: "rolled_back".to_string(),
                detail,
            }
        }
        other => {
            clear_journal(workspace);
            tracing::warn!("[SelfUpdate] 未知 journal phase {other:?}，已清除");
            BootRecovery {
                action: "rolled_back".to_string(),
                detail: format!("unknown phase {other:?}"),
            }
        }
    }
}

/// 回滚本体（old→target rename；target 参数化——单测注入临时路径，不碰
/// 测试二进制自身）。
fn rollback_old_exe_to(old: &str, target: &Path) -> String {
    let old = PathBuf::from(old);
    if !old.exists() {
        return ".old 不在场（无需回滚）".to_string();
    }
    match std::fs::rename(&old, target) {
        Ok(()) => format!("已回滚 {} ← {}", target.display(), old.display()),
        Err(e) => format!(
            "回滚失败（{} ← {}）：{e}；残留 .old 需手工恢复。",
            target.display(),
            old.display()
        ),
    }
}

// ---------------------------------------------------------------------------
// SelfUpdateManager（apply 编排；selfupdate feature 门控）
// ---------------------------------------------------------------------------

/// apply 编排器。gateway run_runtime 审批装配点构建（审批 manager / 审计链 /
/// estop 三件同源），注入 WSAPI 槽 + 内部命令臂。`shutdown` 闭包 = 既有优雅
/// 停机路径，注入避免本模块反向依赖 bin 类型。
#[cfg(feature = "selfupdate")]
pub struct SelfUpdateManager {
    pub workspace: PathBuf,
    pub estop: std::sync::Arc<nemesis_agent::estop::EstopState>,
    pub approval: std::sync::Arc<dyn nemesis_security::auditor::ApprovalManager>,
    pub audit: Option<std::sync::Arc<nemesis_security::pipeline::SecurityPlugin>>,
    pub shutdown: std::sync::Arc<dyn Fn() + Send + Sync>,
}

#[cfg(feature = "selfupdate")]
impl SelfUpdateManager {
    fn audit_event(&self, operation: &str, decision: &str, reason: &str) {
        if let Some(plugin) = self.audit.as_ref() {
            let _ = plugin.append_direct_audit_event(
                operation,
                "selfupdate",
                "selfupdate",
                "selfupdate",
                operation,
                decision,
                reason,
            );
        }
    }

    /// 九步 apply（**v1 仅 Windows**；非 Windows 变体见下方 cfg 分体）。
    /// Ok = staging 完成且新进程已拉起、本进程将优雅退出；Err = 人读失败
    /// 原因（每步失败都带现场 + 审计留痕）。
    #[cfg(target_os = "windows")]
    pub async fn apply(
        &self,
        current_version: &str,
        current_commit: &str,
        allow_downgrade: bool,
    ) -> Result<String, String> {
        // ── 步骤 1a：estop 闸 ──
        if self.estop.is_engaged() {
            self.audit_event("selfupdate", "denied", "estop engaged");
            return Err("急停（estop）生效中：自更新被冻结。释放急停后重试。".to_string());
        }
        // ── 步骤 1b：上一次更新未收口 → 拒（现场由 boot 收口处理）──
        if let Some(j) = load_journal(&self.workspace) {
            self.audit_event(
                "selfupdate",
                "denied",
                &format!("journal phase={}", j.phase),
            );
            return Err(format!(
                "上一次自更新未收口（journal phase={}，workspace {}）。请重启 gateway \
                 让启动收口跑完再试。",
                j.phase,
                self.workspace.display()
            ));
        }
        // ── 步骤 1c：频率保险丝（24h）──
        if let Some(last) = last_attempt_ms(&self.workspace) {
            let elapsed = now_ms().saturating_sub(last);
            if elapsed < APPLY_FUSE_MILLIS {
                let remain_h = (APPLY_FUSE_MILLIS - elapsed) / 3_600_000;
                self.audit_event("selfupdate", "denied", "frequency fuse");
                return Err(format!(
                    "频率保险丝：24h 内已尝试过自更新（约 {remain_h}h 后可再试）。"
                ));
            }
        }
        // ── 步骤 1d（早闸）：Release body commit 已命中 → 免下 300MB 主包 ──
        //（body 缺 sha / 格式变 = 跳过早闸，下载后按包内 version.txt 权威判定）
        if !current_commit.is_empty() {
            // 元信息拉取失败不早停——下载路径会给真实错误（含 CI 未产出包）。
            if let Ok(meta) = crate::handlers::release_fetch::fetch_latest_release_meta().await
                && let Some(latest) = meta.body_commit()
                && commit_matches(current_commit, &latest)
            {
                self.audit_event("selfupdate", "denied", "already at target commit");
                return Err(format!(
                    "当前已是最新版本（commit {}），无需更新。",
                    &latest[..latest.len().min(12)]
                ));
            }
        }
        // ── 下载主包 + 解包取新 exe / 身份（在审批前——卡片带真实包体大小）──
        let zip_bytes = crate::handlers::release_fetch::fetch_release_asset(
            TARGET_ASSET,
            MAX_UPDATE_ASSET_BYTES,
        )
        .await
        .map_err(|e| {
            self.audit_event("selfupdate", "denied", &format!("download: {e}"));
            format!("下载 {TARGET_ASSET} 失败：{e}")
        })?;
        let extracted = extract_update_bundle(&zip_bytes)?;
        let target_commit = extracted.commit.clone().unwrap_or_default();
        // ── 步骤 1e：降级/同版闸（包内 version.txt 权威身份）──
        if commit_matches(current_commit, &target_commit) {
            self.audit_event("selfupdate", "denied", "already at target commit");
            return Err(format!(
                "当前已是目标版本（commit {}），无需更新。",
                &target_commit[..target_commit.len().min(12)]
            ));
        }
        if current_commit.is_empty() && !allow_downgrade {
            self.audit_event("selfupdate", "denied", "unknown current commit");
            return Err(
                "当前二进制无 commit 身份（开发构建？）且未指定 --allow-downgrade：无法证明\
                 这次不是降级。确认要切换请带 --allow-downgrade 重试。"
                    .to_string(),
            );
        }
        // ── 步骤 2：CRITICAL 审批卡（版本切换永不免审——respond 侧 always-allow
        //    对 CRITICAL 恒忽略，见 web_approval rule_permitted_for）──
        let req_id = format!("selfupdate-{}", now_ms());
        let target_desc = if target_commit.is_empty() {
            "（包内无 commit 身份）".to_string()
        } else {
            format!("commit {}", &target_commit[..target_commit.len().min(12)])
        };
        let verdict = self.approval.request_approval_sync(
            &req_id,
            "selfupdate_apply",
            &format!("{current_version} → {target_desc}"),
            "CRITICAL",
            &format!(
                "自更新：{current_version} → {target_desc}（{TARGET_ASSET}，{}MB）。\
                 批准后已校验签名的新版本将替换当前二进制并重启 gateway；失败自动回滚。",
                zip_bytes.len() / (1024 * 1024)
            ),
            APPLY_APPROVAL_TIMEOUT_SECS,
        );
        let approved = match verdict {
            Ok(v) => v.approved,
            Err(e) => {
                self.audit_event("approval", "denied", &e);
                return Err(format!("审批通道故障：{e}"));
            }
        };
        if !approved {
            self.audit_event("approval", "denied", "user denied or timeout");
            return Err("自更新审批未通过（拒绝或超时）。".to_string());
        }
        // 审批即承诺：此后所有失败计入 24h 保险丝（防坏包重试风暴）。
        self.audit_event("approval", "allowed", &target_desc);
        stamp_fuse(&self.workspace);

        // ── 步骤 4：v4 验签（Valid-only；锚 = 编译期固化，不依赖包内证书）──
        let anchors = nemesis_verify::builtin_root_anchors();
        if anchors.is_empty() {
            self.audit_event("verify", "denied", "no builtin root anchor");
            return Err(
                "本构建无编译期信任锚（NEMESIS_BUILD_ROOT_ANCHOR 未注入）：无法验签，\
                 自更新拒绝执行。"
                    .to_string(),
            );
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let outcome = nemesis_verify::verify::verify_bytes(&extracted.exe_bytes, &anchors, now);
        if !matches!(outcome, nemesis_verify::verify::VerifyOutcome::Valid { .. }) {
            let reason = format!("v4 验签未过（{}），包已拒收", outcome_kind(&outcome));
            self.audit_event("verify", "denied", &reason);
            return Err(reason);
        }
        self.audit_event("verify", "allowed", &target_desc);

        // ── 步骤 5：staging（journal 先行 → rename dance）──
        let exe_path = std::env::current_exe().map_err(|e| format!("取 current_exe 失败：{e}"))?;
        let old_path = exe_path.with_extension("old");
        // .old/.new 残留清场（上次回滚失败的遗留会让 rename 撞已存在目标）。
        let _ = std::fs::remove_file(&old_path);
        let new_path = exe_path.with_extension("new");
        let _ = std::fs::remove_file(&new_path);
        std::fs::write(&new_path, &extracted.exe_bytes)
            .map_err(|e| format!("新二进制落盘失败（{}）：{e}", new_path.display()))?;
        let journal = UpdateJournal {
            phase: "staging".to_string(),
            target_commit: target_commit.clone(),
            target_version: extracted.version_txt_commit_desc(),
            old_path: old_path.display().to_string(),
            started_ms: now_ms(),
        };
        save_journal(&self.workspace, &journal)?;
        // dance：exe→.old，.new→exe（rename 运行中 PE 合法；窗口毫秒级）。
        std::fs::rename(&exe_path, &old_path)
            .map_err(|e| format!("exe→.old rename 失败：{e}（staging 中断，journal 已留现场）"))?;
        if let Err(e) = std::fs::rename(&new_path, &exe_path) {
            // 回手：.old 归位，journal 清除——现场不留半套。
            let _ = std::fs::rename(&old_path, &exe_path);
            clear_journal(&self.workspace);
            let reason = format!(".new→exe rename 失败：{e}（已回滚 .old）");
            self.audit_event("staging", "denied", &reason);
            return Err(reason);
        }
        self.audit_event("staging", "allowed", &old_path.display().to_string());

        // ── 步骤 6：journal 相位推进 + 受控重启 ──
        let mut restarting = journal.clone();
        restarting.phase = "restarting".to_string();
        save_journal(&self.workspace, &restarting)?;
        if let Err(e) = spawn_new_version(&exe_path) {
            // 新进程没起来：journal 回 staging 相位，本进程重启后 boot 收口
            // 按 .old 回滚——自愈路径闭环。
            let mut back = restarting.clone();
            back.phase = "staging".to_string();
            let _ = save_journal(&self.workspace, &back);
            self.audit_event("restart", "denied", &e);
            return Err(format!("{e}（staging 现场已留，重启本进程即自动回滚）"));
        }
        self.audit_event("restart", "allowed", &target_desc);

        // ── 步骤 6b：旧进程优雅停机（新进程在等本进程退场）──
        let shutdown = self.shutdown.clone();
        std::thread::Builder::new()
            .name("selfupdate-shutdown".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300)); // 给响应留路
                shutdown();
            })
            .map_err(|e| format!("shutdown 线程启动失败：{e}"))?;

        Ok(format!(
            "自更新 staging 完成（{target_desc}），新进程已拉起，本进程将优雅退出；\
             启动自检与回滚由新进程 boot 收口执行。"
        ))
    }

    /// 非 Windows 变体：诚实拒绝。v1 边界必须有显式闸兑现——否则会先下完
    /// 300MB 主包再撞「不是合法 zip」（tar.gz 解包依赖不在树）。整个九步
    /// 主体不参与非 Windows 编译（避免 unreachable 段），Linux 侧 rename
    /// 语义其实可行，v2 移植时以真实现替换此变体即可；check/status/boot
    /// 收口全平台可用。
    #[cfg(not(target_os = "windows"))]
    pub async fn apply(
        &self,
        current_version: &str,
        current_commit: &str,
        allow_downgrade: bool,
    ) -> Result<String, String> {
        let _ = (current_version, current_commit, allow_downgrade);
        self.audit_event("selfupdate", "denied", "non-windows v1 boundary");
        Err("自更新 apply 暂不支持当前平台（v1 仅 Windows）：\
             请手动下载 Release 包替换。"
            .to_string())
    }
}

/// VerifyOutcome 无 Display——人读短文按变体出（skins classify 同口径）。
/// 唯一非测试消费方 = Windows-only apply 主体；非 Windows 生产编译诚实
/// 允许死码（跨平台测试仍消费，cfg 掉会连九态钉测试一起裁出 Linux）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn outcome_kind(o: &nemesis_verify::verify::VerifyOutcome) -> &'static str {
    match o {
        nemesis_verify::verify::VerifyOutcome::Valid { .. } => "Valid",
        nemesis_verify::verify::VerifyOutcome::NoSignature => "NoSignature",
        nemesis_verify::verify::VerifyOutcome::Tampered(_) => "Tampered",
        nemesis_verify::verify::VerifyOutcome::SignatureInvalid => "SignatureInvalid",
        nemesis_verify::verify::VerifyOutcome::UnsupportedVersion(_) => "UnsupportedVersion",
        nemesis_verify::verify::VerifyOutcome::Malformed(_) => "Malformed",
        nemesis_verify::verify::VerifyOutcome::Untrusted => "Untrusted",
        nemesis_verify::verify::VerifyOutcome::Revoked { .. } => "Revoked",
        nemesis_verify::verify::VerifyOutcome::Expired(_) => "Expired",
    }
}

/// 更新包解包产物（version.txt 只在解包路径内解析成 commit，不外带原文）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
#[derive(Debug)]
struct ExtractedBundle {
    exe_bytes: Vec<u8>,
    commit: Option<String>,
}

#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl ExtractedBundle {
    fn version_txt_commit_desc(&self) -> String {
        self.commit.clone().unwrap_or_else(|| "unknown".to_string())
    }
}

/// zip 包解包：取 `nemesisbot.exe`（扁平结构：包内 nemesisbot.exe / plugins/ /
/// version.txt，见 daily-release Package (Windows) 段）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
fn extract_update_bundle(zip_bytes: &[u8]) -> Result<ExtractedBundle, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("更新包不是合法 zip：{e}"))?;
    let mut exe_bytes: Option<Vec<u8>> = None;
    let mut version_txt: Option<String> = None;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("更新包条目 {i} 不可读：{e}"))?;
        let name = entry.name().to_string();
        if name == "nemesisbot.exe" || name.ends_with("/nemesisbot.exe") {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            std::io::Read::read_to_end(&mut entry, &mut buf)
                .map_err(|e| format!("读取包内 nemesisbot.exe 失败：{e}"))?;
            exe_bytes = Some(buf);
        } else if name == "version.txt" || name.ends_with("/version.txt") {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut entry, &mut buf)
                .map_err(|e| format!("读取包内 version.txt 失败：{e}"))?;
            version_txt = Some(buf);
        }
    }
    let exe_bytes = exe_bytes
        .ok_or_else(|| "更新包内未找到 nemesisbot.exe（包结构异常），已拒收".to_string())?;
    let commit = version_txt
        .as_deref()
        .and_then(extract_commit_from_version_txt);
    Ok(ExtractedBundle { exe_bytes, commit })
}

/// 拉起新版本（detached；等旧进程的接力棒经 env 注入）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
fn spawn_new_version(new_exe: &Path) -> Result<u32, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = std::process::Command::new(new_exe);
    cmd.args(&args).env(
        "NEMESISBOT_SELFUPDATE_WAIT_PID",
        std::process::id().to_string(),
    );
    #[cfg(windows)]
    {
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP：脱离当前控制台，
        // 旧进程退出不牵连新进程。
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .map(|c| c.id())
        .map_err(|e| format!("新版本进程拉起失败（{}）：{e}", new_exe.display()))
}

#[cfg(test)]
mod tests;
