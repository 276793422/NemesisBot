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
//! - 版本身份 = **签名更新清单（R1）**：包内 `manifest.json` 是「JSON 字节 +
//!   CMS + v4 footer」单文件签名载体（`{commit, sequence, exe_sha256,
//!   built_at}`，CI 用与 exe 同一密钥体系签发），客户端 `verify_bytes` 验签
//!   Valid 后取身份，且 `sha256(exe_bytes)` 必须等于清单 `exe_sha256`（完整性
//!   硬拒，不豁免）。版本单调性 = `sequence` 数值比较（CI run_number），「当前」
//!   基准来自 `<exe_dir>/applied_manifest.json`（上次成功更新写入的已验签清单
//!   副本）。**顺序未知 = 默认拒绝**（清单缺失/验签失败/applied 副本缺失——
//!   存量安装首次更新即此形态），`allow_downgrade` 显式豁免；v4 验签与审批卡
//!   永不豁免。外置 version.txt 降级为展示用途（改写它无法伪装版本）；发布
//!   列表顺序判定整体退役。
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
    /// R1：目标更新清单的**已验签载体字节**（base64）。boot `update_success`
    /// 收口时写入 `<exe_dir>/applied_manifest.json`（写前再验签）——下次 apply
    /// 的顺序基准。`#[serde(default)]` 兼容 R1 之前的旧 journal（空 = 无清单，
    /// 不写 applied 副本）。
    #[serde(default)]
    pub target_manifest: String,
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
    let bytes =
        serde_json::to_vec_pretty(journal).map_err(|e| format!("journal 序列化失败：{e}"))?;
    // 原子写走项目权威 helper（唯一临时名 + flush + sync_all + rename + unix
    // 父目录同步）。此前自制 tmp+rename 没有 sync：注释宣称的「防断电半行
    // JSON」防不住断电丢页，且固定 tmp 名在并发实例下互踩。boot 收口读半行
    // = None = 当无事发生，staging 现场无人认领；原子 + 同步写消掉这个缝。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), &bytes, 0o644)
        .map_err(|e| format!("journal 落盘失败：{e}"))
}

/// journal 清除。true = 已不在（含本就不存在）；false = 删除失败有残留，
/// 调用方记日志/审计——boot 收口幂等，下次启动可再收，不阻塞主流程。
pub fn clear_journal(workspace: &Path) -> bool {
    match std::fs::remove_file(journal_path(workspace)) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

/// 带日志的清除（boot 收口等无审计插件的路径用）。
fn clear_journal_logged(workspace: &Path) {
    if !clear_journal(workspace) {
        tracing::warn!("[SelfUpdate] journal 清理失败（残留仍在；boot 收口幂等，下次启动可再收）");
    }
}

/// 频率保险丝记账（上次 apply 尝试时刻）。独立于 journal——journal 成功路径
/// 会清，fuse 不可忘。**fail-closed**：记账失败返回 Err，调用方（apply 审批
/// 通过后）必须拒绝继续——state 目录写失败意味着磁盘/权限异常，此刻继续
/// staging 的风险远大于中断更新。
pub fn stamp_fuse(workspace: &Path) -> Result<(), String> {
    let path = fuse_path(workspace);
    nemesis_utils::write_file_atomic(
        &path.to_string_lossy(),
        now_ms().to_string().as_bytes(),
        0o644,
    )
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
// 降级判定（R1：单调基准 = 签名清单 sequence 数值；发布列表位次版整体退役）
// ---------------------------------------------------------------------------

/// 降级判定结论。版本单调性以 CI 清单 `sequence`（run_number）数值比较为
/// 基准；「当前」基准 = `<exe_dir>/applied_manifest.json` 的已验签 sequence。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 生产消费方=Windows-only apply；测试跨平台
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DowngradeVerdict {
    /// 目标 sequence 比当前已安装的大。
    Upgrade,
    /// 目标 sequence 即当前（同版闸已拦，此处防御性表达）。
    Same,
    /// 目标 sequence 比当前已安装的小（= 旧包）。
    Downgrade,
    /// 无法证明（applied 副本缺失/验签失败/目标清单缺失或验签失败）——**默认
    /// 拒绝**，`allow_downgrade` 显式豁免（R1 语义反转：旧实现放行 + 审计）。
    Unknown,
}

/// 纯函数：sequence 数值判序。当前基准缺失（`None`，applied 副本不在场或
/// 不可信）= Unknown，不猜。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn sequence_verdict(current: Option<u64>, target: u64) -> DowngradeVerdict {
    match current {
        None => DowngradeVerdict::Unknown,
        Some(cur) => match target.cmp(&cur) {
            std::cmp::Ordering::Greater => DowngradeVerdict::Upgrade,
            std::cmp::Ordering::Equal => DowngradeVerdict::Same,
            std::cmp::Ordering::Less => DowngradeVerdict::Downgrade,
        },
    }
}

/// 结论 → 拒绝文案（None = 放行）。R1 语义反转：**Unknown 也默认拒绝**——
/// 顺序证据缺失（清单缺失/验签失败/applied 副本缺失）不能默认当成升级；
/// `allow_downgrade` 是「顺序未知」显式豁免。v4 验签与审批卡在任何路径都
/// 不豁免（不变）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn downgrade_gate_error(v: DowngradeVerdict, allow_downgrade: bool) -> Option<String> {
    match v {
        DowngradeVerdict::Upgrade => None,
        DowngradeVerdict::Same => {
            Some("目标更新包与当前安装版本相同（sequence 命中）：无需更新。".to_string())
        }
        DowngradeVerdict::Downgrade if !allow_downgrade => Some(
            "检测到降级：目标更新包的 sequence 比当前已安装版本更小（旧包）。\
             确认要回退请带 --allow-downgrade 重试。"
                .to_string(),
        ),
        DowngradeVerdict::Unknown if !allow_downgrade => Some(
            "无法证明这次更新不是降级（签名更新清单缺失/验签失败，或本机无已安装\
             版本的清单基准——存量安装首次更新属此形态）。确认要继续请带 \
             --allow-downgrade 重试（该豁免仅放开顺序判定，签名验真与审批照常）。"
                .to_string(),
        ),
        // 显式豁免下的 Downgrade/Unknown 放行（调用方审计留痕）。
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 签名更新清单（R1：身份数据源；「JSON 字节 + CMS + v4 footer」单文件载体）
// ---------------------------------------------------------------------------

/// 更新清单（`manifest.json` 载体剥 footer 后的明文 JSON 结构）。CI 签发
/// （与 exe 同一密钥体系）；客户端验签 Valid 后才采信任何字段。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateManifest {
    /// 全量 git commit（40 hex；boot 自检与同版闸身份）。
    pub commit: String,
    /// 构建序列号（CI run_number，单调递增）——降级判定基准。
    pub sequence: u64,
    /// **已签名 exe** 的 SHA-256（CI 在 exe 签名之后生成清单——客户端下载
    /// 解包的字节即此摘要的绑定对象；不匹配 = 完整性问题，无条件拒）。
    pub exe_sha256: String,
    /// 构建时刻（RFC3339 人读留痕，不参与判定）。
    pub built_at: String,
}

/// 小写 hex SHA-256（skins 同型样板；本模块唯一消费点在 selfupdate feature 内）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 清单载体验签 + 剥 footer + 解析 + 字段校验（单一入口）。任何一步失败都
/// 是诚实 Err（调用方按 Unknown 类语义处理：默认拒，allow_downgrade 豁免）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn verify_and_parse_manifest(
    carrier: &[u8],
    anchors: &[[u8; 32]],
    now: u64,
) -> Result<UpdateManifest, String> {
    let outcome = nemesis_verify::verify::verify_bytes(carrier, anchors, now);
    if !matches!(outcome, nemesis_verify::verify::VerifyOutcome::Valid { .. }) {
        return Err(format!("清单验签未过（{}）", outcome_kind(&outcome)));
    }
    // raw 载体：明文 JSON 在前、v4 footer 在后——extract 的 content_len 即
    // 明文长度（digest 交叉核已由 verify_bytes 完成，此处只取字节）。
    let verbatim = nemesis_verify::envelope::extract_carrier_v4(carrier)
        .map_err(|e| format!("清单载体解析失败：{e}"))?;
    let json_bytes = &carrier[..verbatim.content_len];
    let manifest: UpdateManifest =
        serde_json::from_slice(json_bytes).map_err(|e| format!("清单 JSON 解析失败：{e}"))?;
    if manifest.commit.is_empty()
        || !(manifest.commit.len() == 40 && manifest.commit.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(format!(
            "清单 commit 字段非法（需 40 hex，得 {} 字符）",
            manifest.commit.len()
        ));
    }
    if manifest.sequence == 0 {
        return Err("清单 sequence 字段非法（需 >0）".to_string());
    }
    if manifest.exe_sha256.len() != 64
        || !manifest.exe_sha256.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(format!(
            "清单 exe_sha256 字段非法（需 64 hex，得 {} 字符）",
            manifest.exe_sha256.len()
        ));
    }
    Ok(manifest)
}

/// 完整性绑定纯函数：exe 字节摘要与清单绑定值一致（清单验签 Valid 后调用；
/// 不匹配 = 包被中途调包，无条件拒，allow_downgrade 不豁免）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn manifest_exe_digest_matches(manifest: &UpdateManifest, exe_bytes: &[u8]) -> bool {
    manifest
        .exe_sha256
        .eq_ignore_ascii_case(&sha256_hex(exe_bytes))
}

/// `applied_manifest.json` 路径（exe 同目录——上次成功更新写入的已验签清单
/// 载体副本，下次 apply 的顺序基准）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn applied_manifest_path(exe: &Path) -> PathBuf {
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("applied_manifest.json")
}

/// 写入 applied 清单副本。**写前再验签**（副本来自 journal 携带字节，boot 期
/// 可能已隔多次重启——不可信输入不落信任锚旁）；验签失败 = Err 不写（下次
/// apply 走 Unknown 拒绝，诚实 fail-closed）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn save_applied_manifest(
    exe: &Path,
    carrier: &[u8],
    anchors: &[[u8; 32]],
    now: u64,
) -> Result<(), String> {
    // 先验签再写（解析校验也顺带跑——半截 JSON 不落盘）。
    verify_and_parse_manifest(carrier, anchors, now)?;
    let path = applied_manifest_path(exe);
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), carrier, 0o644)
        .map_err(|e| format!("applied 清单落盘失败（{}）：{e}", path.display()))
}

/// 读 applied 清单的 sequence（顺序判定基准）。文件缺失 / 验签失败 / 解析
/// 失败一律 None（= Unknown，调用方默认拒）——不区分「从未更新过」与「副本
/// 被篡改」，两者对顺序判定都是证据缺失。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn load_applied_sequence(exe: &Path, anchors: &[[u8; 32]], now: u64) -> Option<u64> {
    let bytes = std::fs::read(applied_manifest_path(exe)).ok()?;
    match verify_and_parse_manifest(&bytes, anchors, now) {
        Ok(m) => Some(m.sequence),
        Err(e) => {
            tracing::warn!("[SelfUpdate] applied 清单验读失败（按顺序未知处理）：{e}");
            None
        }
    }
}

/// journal → applied 副本落盘（boot `update_success` 收口接线；R1 写入链
/// 闭环的最后一步）。carrier 为空（旧 journal）= 无事返回 Ok；锚集空 = 诚实
/// 跳过（下次 apply 走 Unknown 拒绝，不假装有基准）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn write_applied_manifest_from_journal(
    exe: &Path,
    journal: &UpdateJournal,
    anchors: &[[u8; 32]],
) -> Result<(), String> {
    use base64::Engine as _;
    if journal.target_manifest.is_empty() {
        return Ok(());
    }
    if anchors.is_empty() {
        tracing::warn!(
            "[SelfUpdate] 本构建无信任锚，applied 清单副本不落盘（下次 apply 顺序判定=未知）"
        );
        return Ok(());
    }
    let carrier = base64::engine::general_purpose::STANDARD
        .decode(journal.target_manifest.as_bytes())
        .map_err(|e| format!("journal 清单载体 base64 解码失败：{e}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    save_applied_manifest(exe, &carrier, anchors, now)
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
    // 同源解析：与 apply 的下载对象一致（首个含主包资产的 release），消除旧
    // 实现「check 读列表第一条 / apply 下载首个含资产的」双路径分裂——最新
    // 条目尚未产出资产时，展示与安装可能跨版本错位。
    let meta = crate::handlers::release_fetch::fetch_release_for_asset(TARGET_ASSET).await?;
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
    /// none / wait_timeout / update_success / rolled_back / recovery_failed
    /// （R2：recovery_failed = 回滚执行失败，journal 与 .old **保留**，下次
    /// 启动 boot 收口幂等重试——旧实现失败也清 journal，现场丢失不可重试）。
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
                // R1 写入链闭环：update_success 收口时把 journal 携带的已验签
                // 清单载体写入 exe 目录（下次 apply 的顺序基准）。写前再验签，
                // 失败 warn 跳过（副本缺失 = 下次 apply 走 Unknown 拒，诚实
                // 边界，不阻塞收口本身）；回滚路径不写。
                #[cfg(feature = "selfupdate")]
                if let Err(e) = write_applied_manifest_from_journal(
                    exe,
                    &journal,
                    &nemesis_verify::builtin_root_anchors(),
                ) {
                    tracing::warn!("[SelfUpdate] applied 清单副本写入失败（收口继续）：{e}");
                }
                clear_journal_logged(workspace);
                tracing::info!(
                    "[SelfUpdate] 启动自检通过：当前 commit 命中目标 {}，更新收口",
                    &journal.target_commit[..journal.target_commit.len().min(12)]
                );
                BootRecovery {
                    action: "update_success".to_string(),
                    detail: format!("target {}", journal.target_commit),
                }
            } else {
                match rollback_old_exe_to(&journal.old_path, exe) {
                    Ok(detail) => {
                        clear_journal_logged(workspace);
                        // 缺 `.old` = 无可恢复现场的 noop，不是回滚成功——action
                        // 如实分臂，日志表述跟随。
                        let noop = detail.starts_with(ROLLBACK_NOOP_PREFIX);
                        let action = if noop { "rollback_noop" } else { "rolled_back" };
                        let verb = if noop {
                            "无可恢复现场（.old 缺失），journal 已清"
                        } else {
                            "已回滚"
                        };
                        tracing::warn!(
                            "[SelfUpdate] 启动自检失败：当前 commit 未命中目标，{verb}（{detail}）"
                        );
                        BootRecovery {
                            action: action.to_string(),
                            detail,
                        }
                    }
                    // R2：回滚失败不清 journal——现场（journal + .old）保留，
                    // 下次启动重试；旧实现无条件清 journal = 失败即丢现场。
                    Err(fail) => {
                        tracing::error!(
                            "[SelfUpdate] 启动自检失败且回滚执行失败：journal 与 .old \
                             保留，下次启动将重试回滚（{fail}）"
                        );
                        BootRecovery {
                            action: "recovery_failed".to_string(),
                            detail: fail,
                        }
                    }
                }
            }
        }
        "staging" => match rollback_old_exe_to(&journal.old_path, exe) {
            Ok(detail) => {
                clear_journal_logged(workspace);
                let noop = detail.starts_with(ROLLBACK_NOOP_PREFIX);
                let action = if noop { "rollback_noop" } else { "rolled_back" };
                tracing::warn!("[SelfUpdate] staging 中断现场收口（{detail}）");
                BootRecovery {
                    action: action.to_string(),
                    detail,
                }
            }
            Err(fail) => {
                tracing::error!(
                    "[SelfUpdate] staging 中断现场回滚失败：journal 保留，下次启动重试（{fail}）"
                );
                BootRecovery {
                    action: "recovery_failed".to_string(),
                    detail: fail,
                }
            }
        },
        other => {
            clear_journal_logged(workspace);
            tracing::warn!("[SelfUpdate] 未知 journal phase {other:?}，已清除");
            BootRecovery {
                action: "rolled_back".to_string(),
                detail: format!("unknown phase {other:?}"),
            }
        }
    }
}

/// 回滚本体（old→target rename；target 参数化——单测注入临时路径，不碰
/// 测试二进制自身）。**诚实化（R2）**：Ok=回滚成功 / Err=失败原因——调用方
/// 据 Ok/Err 决定清不清 journal，失败时保留现场等下次 boot 重试。
///
/// Windows 舞步：目标位被运行中的 PE 占用时（boot 收口的常态——当前进程
/// 就跑在待回滚替换的 exe 上），直接 rename(.old→exe) 会撞「文件被占用」。
/// 运行中 PE 可 rename 不可覆盖，所以先把运行中 exe 让位到 `.failed`
/// （保留作诊断现场），再 .old→exe；第二步失败则把 `.failed` 尝试回手
/// 原位并 Err。非 Windows rename 语义天然可覆盖运行中二进制，单步即可。
/// 缺 `.old` 分支（noop）的 detail 前缀：boot 调用方据此把 action 记为
/// `rollback_noop` 而非 `rolled_back`——什么都没回滚就不能宣称回滚完成。
const ROLLBACK_NOOP_PREFIX: &str = "rollback_noop";

fn rollback_old_exe_to(old: &str, target: &Path) -> Result<String, String> {
    let old = PathBuf::from(old);
    if !old.exists() {
        // 缺 `.old` = 无可恢复现场：重试永远修不回来，保留 journal 只会以
        // journal 闸永久阻塞后续 apply。返回可识别前缀，调用方据此把 action
        // 记为 rollback_noop（而非失实的 rolled_back）——journal 照常清理。
        return Ok(format!(
            "{ROLLBACK_NOOP_PREFIX}: .old 不在场（无可恢复现场，无需回滚）"
        ));
    }
    #[cfg(windows)]
    if target.exists() {
        let failed = target.with_extension("failed");
        let _ = std::fs::remove_file(&failed);
        std::fs::rename(target, &failed).map_err(|e| {
            format!(
                "回滚失败：运行中 exe 让位失败（{} → {}）：{e}；.old 现场保留。",
                target.display(),
                failed.display()
            )
        })?;
        if let Err(e) = std::fs::rename(&old, target) {
            // 第二步失败：尽力把运行中 exe 回手原位，诚实上报双现场。
            let restored = std::fs::rename(&failed, target).is_ok();
            return Err(format!(
                "回滚失败（{} ← {}）：{e}；.old 现场保留{}。",
                target.display(),
                old.display(),
                if restored {
                    "（运行中 exe 已回手原位）".to_string()
                } else {
                    format!(
                        "，运行中 exe 让位至 {}（手工恢复时两文件并存）",
                        failed.display()
                    )
                }
            ));
        }
        return Ok(format!(
            "已回滚 {} ← {}（被替换的 exe 让位至 {} 供诊断）",
            target.display(),
            old.display(),
            failed.display()
        ));
    }
    match std::fs::rename(&old, target) {
        Ok(()) => Ok(format!("已回滚 {} ← {}", target.display(), old.display())),
        Err(e) => Err(format!(
            "回滚失败（{} ← {}）：{e}；残留 .old 需手工恢复。",
            target.display(),
            old.display()
        )),
    }
}

// ---------------------------------------------------------------------------
// apply 互斥原语（R2：单飞 + 跨进程锁 + staging 名去固定化）
// ---------------------------------------------------------------------------

/// staging 临时名（PID 后缀去固定化）：`.old-{pid}` / `.new-{pid}`。双进程
/// 并发 apply 不再互踩同一文件名（boot 收口只认 journal.old_path 记录的
/// 全路径，无固定名依赖）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
fn staging_paths(exe: &Path) -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    (
        exe.with_extension(format!("old-{pid}")),
        exe.with_extension(format!("new-{pid}")),
    )
}

/// 安装目录级跨进程锁：`<exe_dir>/selfupdate.lock` 的 OS 文件锁
/// （`File::try_lock`，进程死 OS 自动释放——无陈锁清理问题）。Ok=锁文件句柄
/// （**必须保活到 apply 结束**，drop 即释放）；Err=另一进程正在自更新。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
fn acquire_install_lock(exe_dir: &Path) -> Result<std::fs::File, String> {
    let path = exe_dir.join("selfupdate.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("自更新锁文件创建失败（{}）：{e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(
            "另一进程正在自更新（安装目录锁被占）：同一安装目录同时只允许一个 \
             自更新在执行。"
                .to_string(),
        ),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(format!("自更新锁获取失败（{}）：{e}", path.display()))
        }
    }
}

/// 陈旧 staging 残留清扫（R2 补口）：staging 名去固定化（PID 后缀）后，旧
/// 固定名方案「下次 apply 的 pre-clean 隐式清掉上一份 `.old`」的隐式回收
/// 随之失效——不补显式清扫，每次成功更新都会永久残留一份完整旧 exe。
/// 语义：持安装目录锁期间同目录不可能有并发 apply，任何非本进程的
/// `.old-{pid}`/`.new-{pid}` 都是死进程/崩溃遗留，尽力删除（仍被在跑旧
/// 进程镜像锁住的 PE 删除失败属预期，留给下次 apply 再清，best-effort）。
/// `.failed` 是计划钦定的诊断现场，不清。返回删除数（审计观测用）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
fn sweep_stale_staging(exe: &Path) -> usize {
    let Some(dir) = exe.parent() else {
        return 0;
    };
    let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) else {
        return 0;
    };
    let old_prefix = format!("{stem}.old-");
    let new_prefix = format!("{stem}.new-");
    let mut removed = 0;
    // 删除与遍历顺序无关（集合操作），无需排序。
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let is_staging = name.starts_with(&old_prefix) || name.starts_with(&new_prefix);
            if is_staging && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
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
    /// R2 进程内单飞闸：同一 manager 上并发 apply 恰一进入（try_lock 失败 =
    /// 「已有自更新在进行」，诚实拒绝）。与 [`acquire_install_lock`]（安装
    /// 目录跨进程锁）两层互补。
    pub flight: tokio::sync::Mutex<()>,
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
        // ── 步骤 1a-2：进程内单飞（R2）——并发 apply 恰一进入 ──
        let _flight = match self.flight.try_lock() {
            Ok(g) => g,
            Err(_) => {
                self.audit_event("selfupdate", "denied", "another apply in-flight");
                return Err("已有自更新在进行中：同一实例同时只允许一个 apply。".to_string());
            }
        };
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
        // ── 步骤 1d（早闸）：发布列表同源解析——含主包资产的 release 的 body
        //    commit 已命中 → 免下 300MB 主包（**优化闸**：只做拒绝性短路，身
        //    份权威在签名清单；body 缺 sha/格式变 = 跳过早闸，下载后按清单判
        //    定；列表拉取失败不早停——下载路径回落独立解析给真实错误）。
        //    列表留在此作用域供下载步定位同一对象的资产 URL（一次拉取两用）。
        let resolved_list = crate::handlers::release_fetch::fetch_release_list()
            .await
            .ok();
        if !current_commit.is_empty()
            && let Some(list) = resolved_list.as_deref()
        {
            let early =
                crate::handlers::release_fetch::resolve_release_for_asset(list, TARGET_ASSET)
                    .and_then(|meta| meta.body_commit());
            if let Some(latest) = early
                && commit_matches(current_commit, &latest)
            {
                self.audit_event("selfupdate", "denied", "already at target commit");
                return Err(format!(
                    "当前已是最新版本（commit {}），无需更新。",
                    &latest[..latest.len().min(12)]
                ));
            }
        }
        // ── 步骤 1d-2：安装目录跨进程锁（R2）——从下载到 staging 全程持有，
        //    双 gateway 进程共享安装目录时只允许一个自更新执行。闸在三个快闸
        //    之后（estop/journal/fuse 拒绝路径不碰锁文件），下载之前（300MB
        //    下载期就占锁，串行化整个 apply）。
        let exe_path = std::env::current_exe().map_err(|e| format!("取 current_exe 失败：{e}"))?;
        let _install_lock = {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .ok_or_else(|| "取 current_exe 目录失败：无法确定安装目录锁位置".to_string())?;
            match acquire_install_lock(&exe_dir) {
                Ok(f) => f,
                Err(e) => {
                    self.audit_event("selfupdate", "denied", "install dir lock held");
                    return Err(e);
                }
            }
        };
        // 持锁窗口内清扫历史 staging 残留（`.old-{pid}`/`.new-{pid}` 死进程
        // 遗留；best-effort，删除失败=仍在被占用，留给下次）。锁独占保证无
        // 并发 apply，不存在误删在途 staging 的窗口。
        let swept = sweep_stale_staging(&exe_path);
        if swept > 0 {
            self.audit_event(
                "staging",
                "allowed",
                &format!("swept {swept} stale staging file(s)"),
            );
        }
        // ── R1 单候选下载链：1d 已同源解析的 release 对象直接定位资产 URL
        //    （check / 早闸 / 下载三处同一对象——旧实现「下载函数内部重抓一次
        //    列表」的双路径漂移就此消除）；列表拉取失败才回落独立解析（真实
        //    错误路径兜底）。解包取新 exe / version.txt / 签名清单。
        let zip_bytes = {
            let download = if let Some(list) = resolved_list.as_deref() {
                match crate::handlers::release_fetch::resolve_release_for_asset(list, TARGET_ASSET)
                    .and_then(|meta| meta.asset_url(TARGET_ASSET).map(str::to_string))
                {
                    Some(url) => {
                        crate::handlers::release_fetch::fetch_bytes(&url, MAX_UPDATE_ASSET_BYTES)
                            .await
                    }
                    None => Err(format!(
                        "近期 Release 均未找到 {TARGET_ASSET} 附件（CI 尚未产出该包）"
                    )),
                }
            } else {
                match crate::handlers::release_fetch::fetch_release_for_asset(TARGET_ASSET).await {
                    Ok(meta) => match meta.asset_url(TARGET_ASSET) {
                        Some(url) => {
                            crate::handlers::release_fetch::fetch_bytes(url, MAX_UPDATE_ASSET_BYTES)
                                .await
                        }
                        None => Err(format!(
                            "近期 Release 均未找到 {TARGET_ASSET} 附件（CI 尚未产出该包）"
                        )),
                    },
                    Err(e) => Err(e),
                }
            };
            download.map_err(|e| {
                self.audit_event("selfupdate", "denied", &format!("download: {e}"));
                format!("下载 {TARGET_ASSET} 失败：{e}")
            })?
        };
        let extracted = extract_update_bundle(&zip_bytes)?;

        // ── 步骤 1e：身份与顺序闸（R1：签名更新清单为唯一权威）──
        // 锚与时间在闸前构造（清单验签在此需要；步骤 4 的 exe 本体验签复用
        // 同锚同时钟）。
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
        // ① 签名清单验签 + 解析。清单缺失/验签失败/字段非法都归 Unknown 类——
        //    默认拒绝（R1 语义反转），allow_downgrade 显式豁免后回落 version.txt
        //    身份继续（身份弱化透明：审计留痕，后续闸按 Unknown 记）。
        let manifest = match extracted
            .manifest_bytes
            .as_deref()
            .map(|carrier| verify_and_parse_manifest(carrier, &anchors, now))
        {
            Some(Ok(m)) => Some(m),
            Some(Err(e)) => {
                if !allow_downgrade {
                    self.audit_event("manifest", "denied", &e);
                    return Err(format!(
                        "{e}：无法证明版本顺序，更新已拒绝。确认要继续请带 \
                         --allow-downgrade 重试（该豁免仅放开顺序判定，签名验真\
                         与审批照常执行）。"
                    ));
                }
                self.audit_event(
                    "manifest",
                    "allowed",
                    &format!("清单不可信但已显式豁免：{e}"),
                );
                None
            }
            None => {
                if !allow_downgrade {
                    self.audit_event("manifest", "denied", "manifest missing");
                    return Err("更新包内无签名更新清单（manifest.json 缺失）：无法证明版本\
                         顺序与 exe 完整性绑定，更新已拒绝。确认要继续请带 \
                         --allow-downgrade 重试。"
                        .to_string());
                }
                self.audit_event("manifest", "allowed", "清单缺失但已显式豁免");
                None
            }
        };
        // ② 完整性绑定硬拒：清单在场时 exe 字节摘要必须与清单绑定值一致——
        //    不匹配 = 包被调包/下载损坏（check 之后远端换包含在此形态），完整
        //    性问题无条件拒（不受 allow_downgrade 豁免）。
        if let Some(m) = &manifest
            && !manifest_exe_digest_matches(m, &extracted.exe_bytes)
        {
            self.audit_event("manifest", "denied", "exe sha256 mismatch vs manifest");
            return Err(
                "更新包内 exe 摘要与签名清单绑定值不一致（包被调包或下载损坏），\
                 已无条件拒收。"
                    .to_string(),
            );
        }
        // ③ 身份权威：清单 commit 优先；清单缺失且已豁免时回落 version.txt。
        let target_commit = manifest
            .as_ref()
            .map(|m| m.commit.clone())
            .unwrap_or_else(|| extracted.commit.clone().unwrap_or_default());
        if target_commit.is_empty() {
            // 无条件拒（不豁免）：目标无身份 → journal.target_commit 空 →
            // boot 自检 commit_matches 恒 false → 必回滚死循环。
            self.audit_event("selfupdate", "denied", "target identity missing");
            return Err(
                "更新包内无任何 commit 身份（清单缺失/不可信且 version.txt 也缺失）：\
                 安装后启动自检必回滚，拒绝执行。"
                    .to_string(),
            );
        }
        // ④ 同版闸（保留：拒绝性优化闸——命中即免走审批与 staging）。
        if commit_matches(current_commit, &target_commit) {
            self.audit_event("selfupdate", "denied", "already at target commit");
            return Err(format!(
                "当前已是目标版本（commit {}），无需更新。",
                &target_commit[..target_commit.len().min(12)]
            ));
        }
        // ⑤ 顺序降级闸：sequence 数值比较，基准 = `<exe_dir>/applied_manifest.json`
        //    （上次成功更新收口写入的已验签清单副本）。applied 缺失（存量安装
        //    首次更新）/副本验签失败/清单本侧缺失（豁免放行）= Unknown——默认
        //    拒绝（R1 语义反转），allow_downgrade 显式豁免 + 审计留痕。v4 验签
        //    与审批卡对任何路径都不豁免。
        if let Some(m) = &manifest {
            let current_seq = load_applied_sequence(&exe_path, &anchors, now);
            let verdict = sequence_verdict(current_seq, m.sequence);
            if let Some(err) = downgrade_gate_error(verdict, allow_downgrade) {
                self.audit_event(
                    "selfupdate",
                    "denied",
                    &format!("downgrade gate: {verdict:?}"),
                );
                return Err(err);
            }
            if matches!(verdict, DowngradeVerdict::Unknown) {
                self.audit_event(
                    "selfupdate",
                    "allowed",
                    "downgrade verdict unknown（applied 基准缺失/不可信），已显式豁免放行",
                );
            }
        }
        // ── 步骤 2：CRITICAL 审批卡（版本切换永不免审——respond 侧 always-allow
        //    对 CRITICAL 恒忽略，见 web_approval rule_permitted_for）──
        let req_id = format!("selfupdate-{}", now_ms());
        let target_desc = format!("commit {}", &target_commit[..target_commit.len().min(12)]);
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
        if let Err(e) = stamp_fuse(&self.workspace) {
            self.audit_event("selfupdate", "denied", &format!("fuse stamp: {e}"));
            return Err(format!(
                "频率保险丝记账失败（{e}）：state 目录不可写，拒绝继续自更新（fail-closed）。"
            ));
        }

        // ── 步骤 4：v4 验签（Valid-only；锚 = 编译期固化，不依赖包内证书；
        //    锚/时钟已在步骤 1e 构造，此处复用同一份）──
        let outcome = nemesis_verify::verify::verify_bytes(&extracted.exe_bytes, &anchors, now);
        if !matches!(outcome, nemesis_verify::verify::VerifyOutcome::Valid { .. }) {
            let reason = format!("v4 验签未过（{}），包已拒收", outcome_kind(&outcome));
            self.audit_event("verify", "denied", &reason);
            return Err(reason);
        }
        self.audit_event("verify", "allowed", &target_desc);

        // ── 步骤 5：staging（journal 先行 → rename dance；exe_path 已在 1e 取）──
        let (old_path, new_path) = staging_paths(&exe_path);
        // 残留清场（同名 PID 复用的罕见遗留会让 rename 撞已存在目标；PID 后缀
        // 名下通常本来就不存在）。
        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(&new_path);
        // .new 先落盘再 fsync（sync_all 之后才轮到 journal 与 rename dance）：
        // 崩溃窗口不再留下「新二进制内容空/半截」的现场；磁盘满等写错误也在
        // staging 之前暴露。
        let mut new_file = std::fs::File::create(&new_path)
            .map_err(|e| format!("新二进制创建失败（{}）：{e}", new_path.display()))?;
        std::io::Write::write_all(&mut new_file, &extracted.exe_bytes)
            .map_err(|e| format!("新二进制落盘失败（{}）：{e}", new_path.display()))?;
        new_file
            .sync_all()
            .map_err(|e| format!("新二进制落盘同步失败（{}）：{e}", new_path.display()))?;
        drop(new_file);
        let journal = UpdateJournal {
            phase: "staging".to_string(),
            target_commit: target_commit.clone(),
            target_version: extracted.version_txt_commit_desc(),
            old_path: old_path.display().to_string(),
            started_ms: now_ms(),
            // R1：已验签清单载体随 journal 走——boot `update_success` 收口写
            // `<exe_dir>/applied_manifest.json`，构成下次 apply 的顺序基准。
            // 清单缺失（豁免放行）= 空串，收口不写副本（顺序闸届时 Unknown）。
            target_manifest: match (&manifest, extracted.manifest_bytes.as_deref()) {
                (Some(_), Some(carrier)) => {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(carrier)
                }
                _ => String::new(),
            },
        };
        save_journal(&self.workspace, &journal)?;
        // dance：exe→.old，.new→exe（rename 运行中 PE 合法；窗口毫秒级）。
        std::fs::rename(&exe_path, &old_path)
            .map_err(|e| format!("exe→.old rename 失败：{e}（staging 中断，journal 已留现场）"))?;
        if let Err(e) = std::fs::rename(&new_path, &exe_path) {
            // 回手走与 boot 收口同一回滚本体（R2 诚实化）：归位成功才清
            // journal；归位失败保留 staging 现场，下次启动 boot 收口重试。
            match rollback_old_exe_to(&old_path.display().to_string(), &exe_path) {
                Ok(detail) => {
                    if !clear_journal(&self.workspace) {
                        self.audit_event("staging", "denied", "journal 清理失败，残留仍在");
                    }
                    let reason = format!(".new→exe rename 失败：{e}（已回滚：{detail}）");
                    self.audit_event("staging", "denied", &reason);
                    return Err(reason);
                }
                Err(fail) => {
                    let reason = format!(
                        ".new→exe rename 失败：{e}；且回手归位失败：{fail}\
                         （journal 已留 staging 现场，重启本进程即自动回滚）"
                    );
                    self.audit_event("staging", "denied", &reason);
                    return Err(reason);
                }
            }
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

/// 更新包解包产物（version.txt 只在解包路径内解析成 commit，不外带原文；
/// manifest.json 以**原样字节**外带——验签必须针对线上签名时的字节，任何
/// serde 往返都可能破坏签名覆盖面）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
#[derive(Debug)]
struct ExtractedBundle {
    exe_bytes: Vec<u8>,
    commit: Option<String>,
    /// R1：包内签名更新清单的原始载体字节（JSON + CMS + v4 footer）。缺失 =
    /// None（旧 CI 包 / 用户自打包）——apply 侧按 Unknown 类处理。
    manifest_bytes: Option<Vec<u8>>,
}

#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl ExtractedBundle {
    fn version_txt_commit_desc(&self) -> String {
        self.commit.clone().unwrap_or_else(|| "unknown".to_string())
    }
}

/// zip 包解包：取 `nemesisbot.exe`（扁平结构：包内 nemesisbot.exe / plugins/ /
/// version.txt / manifest.json，见 daily-release Package (Windows) 段）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // 消费方=Windows-only apply；测试跨平台
fn extract_update_bundle(zip_bytes: &[u8]) -> Result<ExtractedBundle, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("更新包不是合法 zip：{e}"))?;
    let mut exe_bytes: Option<Vec<u8>> = None;
    let mut version_txt: Option<String> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("更新包条目 {i} 不可读：{e}"))?;
        // zip 9：条目名解码可失败——诚实报错拒绝（更新包条目名必须可判读）。
        let name = entry
            .name()
            .map_err(|e| format!("更新包条目 {i} 名字不可解码：{e}"))?
            .to_string();
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
        } else if name == "manifest.json" || name.ends_with("/manifest.json") {
            let mut buf = Vec::with_capacity(entry.size() as usize);
            std::io::Read::read_to_end(&mut entry, &mut buf)
                .map_err(|e| format!("读取包内 manifest.json 失败：{e}"))?;
            manifest_bytes = Some(buf);
        }
    }
    let exe_bytes = exe_bytes
        .ok_or_else(|| "更新包内未找到 nemesisbot.exe（包结构异常），已拒收".to_string())?;
    let commit = version_txt
        .as_deref()
        .and_then(extract_commit_from_version_txt);
    Ok(ExtractedBundle {
        exe_bytes,
        commit,
        manifest_bytes,
    })
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
