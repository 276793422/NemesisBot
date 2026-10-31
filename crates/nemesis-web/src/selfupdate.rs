//! L9 selfupdate 版本自切换通道（2026-10-13，内部专项总计划 T4 战略项；
//! 2026-10-22 两段式改造：审批卡/频率保险丝退役，下载与安装分离）。
//!
//! apply 通道（docs/INFO/2026-10-12_LLM自更新通道设计方案.md §四；两段式
//! 改造后的步骤序）：
//! estop 闸 → 单飞锁 + journal 闸 + 安装目录锁 → 待装包就位（本地
//! `updates/pending.<zip|tar.gz>`，缺席自动先下载）→ 发布源对账（已最新/
//! 过期作废/断网按 pending 兜底）→ 解包 + 身份闸链（签名清单验签/完整性
//! 绑定/plugins 绑定/同版/降级）→ v4 验签 Valid-only → plugins 整包替换
//! staging + rename staging → 受控重启（spawn 新进程等旧进程退场）→ 启动
//! 自检 → 回滚 → 审计链留痕。nemesis-verify 的第二个生产消费点（第一个 =
//! 启动自验 verify_policy）。
//!
//! 人闸位置（用户拍板 2026-10-21）：CRITICAL 审批卡退役——由前端
//! MessageBox 确认框替代（点【开始更新】→「确定更新到 vX？」→ 受理执行，
//! 用户点按钮即主体意志）；真正的安全闸是 v4 签名验签（编译期根锚，
//! Valid-only，任何路径不豁免）。24h 频率保险丝同步退役——两段式后重试
//! 成本趋近零（下载完才装、坏包清空重来），风暴前提不存在；并发治理只留
//! 单飞锁 + 安装目录跨进程锁。
//!
//! 职责与门控：
//! - 本模块住 nemesis-web（release_fetch / 事件 hub 同库可达）；nemesisbot
//!   侧只做装配（gateway run_runtime 建 manager 注入 WSAPI 槽 + 内部命令臂
//!   + 启动自动 check）与 CLI 命令转发。
//! - `selfupdate` feature（= security + zip + tar + flate2 + nemesis-verify）
//!   门控 apply 编排与下载编排（SelfUpdateManager）——IoT 裁剪构建可整体
//!   关掉更新通道；journal / check / boot 收口不门控（纯 fs + serde_json，
//!   recover_on_boot 在任何构建都要能清残留现场）。
//! - 版本身份 = **签名更新清单（R1）**：包内 `manifest.json` 是「JSON 字节 +
//!   CMS + v4 footer」单文件签名载体（`{commit, sequence, exe_sha256,
//!   plugins_sha256, built_at}`，CI 用与 exe 同一密钥体系签发），客户端
//!   `verify_bytes` 验签 Valid 后取身份，且 `sha256(exe_bytes)` 必须等于
//!   清单 `exe_sha256`（完整性硬拒，不豁免）；清单带 `plugins_sha256`
//!   （文件名 → 摘要映射）时包内 plugins/ 逐文件 sha256 + **集合双向匹配**
//!   硬拒（多/少都拒，2026-10-22 用户拍板）；旧清单无此字段 = 诚实降级
//!   不绑定（审计留痕）。版本单调性 = `sequence` 数值比较（CI
//!   run_number），「当前」基准来自 `<exe_dir>/applied_manifest.json`（上次
//!   成功更新写入的已验签清单副本）。**顺序未知 = 默认拒绝**（清单缺失/
//!   验签失败/applied 副本缺失——存量安装首次更新即此形态），
//!   `allow_downgrade` 显式豁免；v4 验签永不豁免。外置 version.txt 降级为
//!   展示用途（改写它无法伪装版本）；发布列表顺序判定整体退役。
//! - staging rename dance 与 Chrome/VS Code 同款：Windows 允许 rename 运行中
//!   的 PE、不允许覆盖——exe→.old、.new→exe 两步，窗口毫秒级；POSIX rename
//!   天然原子覆盖运行中二进制，同一 dance 在 Linux 单步即可（代码同一份，
//!   Windows 臂多一步让位）。journal 先于任何 rename 落盘，启动期
//!   [`recover_on_boot`] 按相位收口。
//! - **staging 抽屉 `back/`**（2026-10-27 用户拍板）：五件套（`.old-{pid}` /
//!   `.new-{pid}` / `.failed` / `plugins.{old,new}-{pid}/`）全部住进 exe 同级
//!   `back/` 目录，安装根目录不再出现散件（收尾清扫失败也不裸露）；
//!   `updates/`（下载缓存）与 `back/`（舞步与回滚源活区）物理隔离——updates/
//!   的任何整清（坏包/取消/过期作废/收口删包）都碰不到回滚源。同卷子目录
//!   rename 与根目录别名 rename 同操作类；journal 记绝对路径，boot 收口按
//!   记录路径消费、不依赖布局（旧布局 journal 照旧路径收口，跨版本安全）；
//!   根目录旧前缀残留由清扫的**过渡兼容层**收掉。成功收口 = 整删 `back/`；
//!   失败零删除（回滚源原样保留）。
//! - **plugins 整包替换**（2026-10-22 用户拍板：主包自带 plugins/ 子树，
//!   整包同版本分发——接口错配根因消失，不需单独打包）：解包产物里
//!   plugins/ 平面文件先落 `back/plugins.new-{pid}/` staging；舞步 = 既有
//!   同名让位 `back/plugins.old-{pid}/` → 新文件逐个 rename 进 `plugins/`
//!   （Windows 运行中 DLL 可 rename 不可覆盖，与 exe 同语义；目录不整迁
//!   ——映射中的 DLL 会让整目录 rename 失败）。**只同名覆盖不删既有**——
//!   用户自装插件不受伤。回滚与 exe **同进同退**（journal 记 staging 目录，
//!   boot 收口连带回滚），杜绝「旧 exe + 新插件」错配。
//! - 两段式持久态（`<exe_dir>/updates/`，自更新私有）：`pending.zip` /
//!   `pending.tar.gz`（`.part` 中转改名）+ `pending.json` 元数据（**后写**
//!   ——见 meta 必有包）。每次读取做 sha256 复验，坏包**整个清空重来**
//!   （不续传不修复，用户拍板）；apply 无 pending 自动先下载；更新收口
//!   删包；发布源前进 → pending 过期作废（清空重下）。
//! - 平台支持（2026-10-22 扩面）：windows/linux = apply 全链路（解包按
//!   包格式 magic 分派 zip/tar.gz，不按平台 cfg 硬分）；macos = TARGET_ASSET
//!   留位（`nightly-macos.tar.gz`，CI 未产出 → check 诚实 asset_present=
//!   false）、apply 显式拒绝桩；android = APK 发行形态不走本通道，显式
//!   拒绝桩。能力位单一真相源 = [`apply_supported`]。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 官方发布源主包资产名（daily-release.yml 产物命名，随平台）。平台表：
/// windows=zip / linux=tar.gz / macos=留位（CI matrix 尚无 darwin 条目，
/// check 对资产缺席诚实报 asset_present=false）/ android=空（APK 发行
/// 形态不走 GitHub 主包通道，apply_supported 同步 false）。
#[cfg(target_os = "windows")]
pub const TARGET_ASSET: &str = "nightly-windows.zip";
#[cfg(target_os = "linux")]
pub const TARGET_ASSET: &str = "nightly-linux.tar.gz";
#[cfg(target_os = "macos")]
pub const TARGET_ASSET: &str = "nightly-macos.tar.gz";
#[cfg(target_os = "android")]
pub const TARGET_ASSET: &str = "";

/// apply 平台支持位（能力表单一真相源，取代旧 `cfg!(windows)` 单点）：
/// windows/linux = 全链路可用；macos = 机制就绪但 CI 无产物且未实测，
/// 诚实 false（留位，打开即真）；android = APK 通道，false。
pub fn apply_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "linux"))
}

/// 更新包读取上限：300MB（全量构建 zip 百 MB 级，裁剪包更小；上限只防
/// 异常巨包，不按最坏包体抠）。
pub const MAX_UPDATE_ASSET_BYTES: usize = 300 * 1024 * 1024;

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
    /// plugins 整包替换 staging 的绝对路径（2026-10-22 扩面）。`plugins_new`
    /// = 新插件落位目录（`plugins.new-{pid}`）；`plugins_backup` = 既有同名
    /// 插件让位目录（`plugins.old-{pid}`）。**双空 = 本次更新无 plugins 舞步**
    /// （旧包无 plugins/ 子树）；boot 回滚按 journal 幂等收口——exe 回滚变
    /// noop 后 plugins 回滚仍可重试。`#[serde(default)]` 兼容旧 journal。
    #[serde(default)]
    pub plugins_backup_dir: String,
    #[serde(default)]
    pub plugins_new_dir: String,
    /// plugins 逐文件清单（2026-10-24 N1 根修）：journal 构造时（任何落盘
    /// 动作之前）按「包内条目 × plugins/ 现状」计算并随 journal 先行落盘
    /// ——回滚以它为唯一真相源，不再从「backup 目录名单差集」推断谁是
    /// 新装（该推断在 backup 缺席、部分归位两个崩溃窗口都会把旧插件误判
    /// 成 new-only 删掉）。`#[serde(default)]` 兼容旧 journal（空清单 =
    /// 保守模式：只归位 backup，不删任何文件）。
    #[serde(default)]
    pub plugins_files: Vec<PluginsFileEntry>,
}

/// plugins 清单单行：`new_install=false` = 替换既有文件（旧版让位进
/// backup）；`new_install=true` = 本次更新新装（plugins/ 原无此文件，回滚
/// = 从 plugins/ 删除归零）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginsFileEntry {
    pub name: String,
    pub new_install: bool,
}

/// 逐文件清单计算（apply 的 journal 构造点专用；纯函数单测钉语义）。
/// `new_install` 判定 = 包内条目在 plugins/ 是否已有同名条目——此刻在任何
/// rename 之前，现状即「旧版是否存在」的准确快照（单飞锁下无并发扰动）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn compute_plugins_manifest(
    plugins_dir: &Path,
    files: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Vec<PluginsFileEntry> {
    files
        .keys()
        .map(|name| PluginsFileEntry {
            name: name.clone(),
            new_install: !plugins_dir.join(name).exists(),
        })
        .collect()
}

/// journal 现场路径（CLI status / 人工排障用同一路径）。
pub fn journal_path(workspace: &Path) -> PathBuf {
    workspace.join("state").join("selfupdate.json")
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
        boot_log_warn("[SelfUpdate] journal 清理失败（残留仍在；boot 收口幂等，下次启动可再收）");
    }
}

/// boot 收口日志三件套。**必须 eprintln + tracing 双写**：boot 收口早于日志
/// 初始化（`init_logger_from_config` 在 `gateway/ctx.rs` 的 assemble 内部，
/// `gateway::run` 在 [`recover_on_boot`] 之后才 assemble）——此处的 tracing
/// 事件无 subscriber 接收，静默丢弃（2026-10-22 彩排实证：update_success
/// 全链跑通但 stdout / 文件双 sink 零痕迹，收口成败完全不可观测）。自更新
/// 最安全关键的一步不允许零痕迹：gateway 是控制台进程，stderr 恒在（与
/// verify_policy 启动自检的 eprintln 同理由）。
fn boot_log_info(msg: &str) {
    eprintln!("{msg}");
    tracing::info!("{msg}");
}

fn boot_log_warn(msg: &str) {
    eprintln!("{msg}");
    tracing::warn!("{msg}");
}

fn boot_log_error(msg: &str) {
    eprintln!("{msg}");
    tracing::error!("{msg}");
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
// 两段式待装包持久态（`<exe_dir>/updates/`；selfupdate feature 门控——
// 唯一写入方是 SelfUpdateManager，IoT 裁剪构建不引入任何读取/清扫开销）
// ---------------------------------------------------------------------------

/// 待装包目录（exe 同目录 `updates/`——portable 发行目录随包走，多实例各自
/// 独立；只读安装目录（Program Files 场景）写失败 = 诚实报错，发行形态以
/// portable 为主，接受此边界）。
#[cfg(feature = "selfupdate")]
pub(crate) fn updates_dir(exe: &Path) -> PathBuf {
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("updates")
}

/// 待装包文件名（按资产名推导：`nightly-windows.zip` → `pending.zip`；
/// `nightly-linux.tar.gz` → `pending.tar.gz`）。同一 updates/ 只存一份待装包
/// ——资产名互斥（每平台只认自己的 TARGET_ASSET），不同资产共存即异常，
/// 由 load_pending 的 sha256/资产对账兜底整清。
#[cfg(feature = "selfupdate")]
fn pending_package_name(asset: &str) -> String {
    format!(
        "pending.{}",
        asset.strip_prefix("nightly-").unwrap_or(asset)
    )
}

#[cfg(feature = "selfupdate")]
pub(crate) fn pending_package_path(exe: &Path, asset: &str) -> PathBuf {
    updates_dir(exe).join(pending_package_name(asset))
}

/// 下载中转名：完成后 rename 成 [`pending_package_path`]——读侧永远只见
/// 完整文件。
#[cfg(feature = "selfupdate")]
pub(crate) fn pending_package_part_path(exe: &Path, asset: &str) -> PathBuf {
    updates_dir(exe).join(format!("{}.part", pending_package_name(asset)))
}

/// 待装包元数据。**后写**（包就位并 sha256 后才落）——`load_pending` 见到
/// meta 即必有完整包，半截下载不产生可装形态。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpdate {
    /// 主包资产名（下载对象 + 待装包文件名推导源，对账用）。
    pub asset: String,
    /// Release tag（人读展示）。
    pub release_tag: String,
    /// 目标 commit（全量 sha；发布 body 缺失 = 空，身份权威归包内签名清单）。
    pub target_commit: String,
    /// 包体 SHA-256（读取时复验——坏包整清，不给坏包安装机会；zip/tar.gz
    /// 通义，字段名沿 zip 首形态）。
    pub zip_sha256: String,
    pub size_bytes: u64,
    pub downloaded_at_ms: u64,
}

#[cfg(feature = "selfupdate")]
pub(crate) fn pending_meta_path(exe: &Path) -> PathBuf {
    updates_dir(exe).join("pending.json")
}

/// 流式 sha256（300MB 包不整读进内存；复用无条件依赖 sha2）。
#[cfg(feature = "selfupdate")]
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::Digest as _;
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("待装包打开失败（{}）：{e}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)
            .map_err(|e| format!("待装包读取失败（{}）：{e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

/// 读待装包（含完整性复验）。meta 缺失 = None（无事）；meta 坏 / 包缺 /
/// sha 不匹配 = **整个 updates/ 清空** + None（用户拍板：坏包宁可重下，不给
/// 安装机会）。
#[cfg(feature = "selfupdate")]
pub fn load_pending(exe: &Path) -> Option<PendingUpdate> {
    let meta_bytes = std::fs::read(pending_meta_path(exe)).ok()?;
    let pending: PendingUpdate = match serde_json::from_slice(&meta_bytes) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[SelfUpdate] 待装包元数据损坏（{e}），清空 updates/ 重来");
            clear_updates_dir(exe);
            return None;
        }
    };
    let package = pending_package_path(exe, &pending.asset);
    if !package.exists() {
        // meta 在包不在 = 异常现场（正常顺序见不到），整清。
        tracing::warn!(
            "[SelfUpdate] 待装包元数据在场但 {} 缺失，清空 updates/ 重来",
            package.display()
        );
        clear_updates_dir(exe);
        return None;
    }
    match sha256_file(&package) {
        Ok(sha) if sha.eq_ignore_ascii_case(&pending.zip_sha256) => Some(pending),
        Ok(sha) => {
            tracing::warn!(
                "[SelfUpdate] 待装包 sha256 复验不匹配（meta {} vs 实际 {sha}），清空 updates/ 重来",
                &pending.zip_sha256[..pending.zip_sha256.len().min(12)]
            );
            clear_updates_dir(exe);
            None
        }
        Err(e) => {
            tracing::warn!("[SelfUpdate] 待装包 sha256 复验失败（{e}），清空 updates/ 重来");
            clear_updates_dir(exe);
            None
        }
    }
}

/// updates/ 全清（下载异常/中断/取消/过期作废/更新收口共用）。韧性删除
///（Windows AV/索引器瞬时句柄是常态）；目录本就不存在 = 成功。
#[cfg(feature = "selfupdate")]
pub(crate) fn clear_updates_dir(exe: &Path) {
    let dir = updates_dir(exe);
    if dir.exists() {
        crate::handlers::release_fetch::remove_dir_all_resilient(&dir);
    }
}

/// 待装包落盘（meta 原子写——唯一写入点，zip 已就位后才调用）。
#[cfg(feature = "selfupdate")]
pub(crate) fn save_pending(exe: &Path, pending: &PendingUpdate) -> Result<(), String> {
    let path = pending_meta_path(exe);
    let bytes =
        serde_json::to_vec_pretty(pending).map_err(|e| format!("待装包元数据序列化失败：{e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), &bytes, 0o644)
        .map_err(|e| format!("待装包元数据落盘失败（{}）：{e}", path.display()))
}

/// SSE 下载/编排进度帧（ephemeral：seq=0 不进环形缓冲——断线补拉重放旧进度
/// 无意义；在线订阅者直收，`useSSE` 需把事件名加进 [`EVENT_TYPES`]）。
/// `message` = 失败类帧的人读原因（apply 编排失败必带；下载类 None）。
#[cfg(feature = "selfupdate")]
pub(crate) fn publish_progress(
    phase: &str,
    downloaded: u64,
    total: Option<u64>,
    message: Option<&str>,
) {
    let pct = total
        .and_then(|t| (downloaded * 100).checked_div(t))
        .map(|v| v.min(100));
    crate::chat_event_log::publish_global_ephemeral(
        crate::events::EVENT_SELFUPDATE_PROGRESS,
        serde_json::json!({
            "phase": phase,
            "downloaded_bytes": downloaded,
            "total_bytes": total,
            "pct": pct,
            "message": message,
        }),
    );
}

// ---------------------------------------------------------------------------
// 降级判定（R1：单调基准 = 签名清单 sequence 数值；发布列表位次版整体退役）
// ---------------------------------------------------------------------------

/// 降级判定结论。版本单调性以 CI 清单 `sequence`（run_number）数值比较为
/// 基准；「当前」基准 = `<exe_dir>/applied_manifest.json` 的已验签 sequence。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 生产消费方=windows/linux apply；macOS/android 桩平台诚实死码
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
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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
/// `allow_downgrade` 是「顺序未知」显式豁免。v4 验签在任何路径都不豁免
///（不变）。
///
/// `force`（强制更新，2026-10-30）：只豁免 `Same` 臂——强制更新的意志是
/// 「重装当前版本」（本地包残了自救），不是降级；`Downgrade`/`Unknown`
/// 仍由 `allow_downgrade` 管辖，force 不混同。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
pub(crate) fn downgrade_gate_error(
    v: DowngradeVerdict,
    allow_downgrade: bool,
    force: bool,
) -> Option<String> {
    match v {
        DowngradeVerdict::Upgrade => None,
        DowngradeVerdict::Same if force => None,
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
             --allow-downgrade 重试（该豁免仅放开顺序判定，签名验真照常）。"
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UpdateManifest {
    /// 全量 git commit（40 hex；boot 自检与同版闸身份）。
    pub commit: String,
    /// 构建序列号（CI run_number，单调递增）——降级判定基准。
    pub sequence: u64,
    /// **已签名 exe** 的 SHA-256（CI 在 exe 签名之后生成清单——客户端下载
    /// 解包的字节即此摘要的绑定对象；不匹配 = 完整性问题，无条件拒）。
    pub exe_sha256: String,
    /// **已签名 plugins** 逐文件 SHA-256 绑定（2026-10-22 用户拍板 ①）：
    /// 文件名（`plugins/` 下平面名）→ 摘要。CI 在插件签名之后生成清单——
    /// 哈希覆盖的是已签名字节，闭环成立。非空时 apply 对包内 plugins/
    /// 逐文件校验 + **集合双向匹配**（包内多文件 / 清单缺条目都拒）；空/缺
    /// = 无绑定诚实降级（旧清单兼容），审计留痕。`#[serde(default)]`。
    #[serde(default)]
    pub plugins_sha256: std::collections::BTreeMap<String, String>,
    /// 构建时刻（RFC3339 人读留痕，不参与判定）。
    pub built_at: String,
}

/// 小写 hex SHA-256（skins 同型样板；本模块唯一消费点在 selfupdate feature 内）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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
    for (name, sha) in &manifest.plugins_sha256 {
        if name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.contains("..")
            || name.contains('/')
        {
            return Err(format!("清单 plugins_sha256 文件名非法（{name:?}）"));
        }
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("清单 plugins_sha256[{name:?}] 非法（需 64 hex）"));
        }
    }
    Ok(manifest)
}

/// plugins 绑定校验纯函数（内存字节面，任何 staging 之前调用——2026-10-22
/// 用户拍板 ①：清单声明 `plugins_sha256` 时逐文件 sha256 + **集合双向匹配**
/// 硬拒：清单有条目包内缺文件 = 拒，包内多出文件 = 拒，摘要不匹配 = 拒；
/// 均大小写不敏感）。清单无绑定（空 map）= Ok（诚实降级，调用方审计留痕
/// ——旧清单无此字段是合法历史形态）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码（测试消费不受影响）
fn verify_plugins_binding_map(
    manifest: &UpdateManifest,
    plugins: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    if manifest.plugins_sha256.is_empty() {
        return Ok(());
    }
    if manifest.plugins_sha256.len() != plugins.len() {
        let missing: Vec<&String> = manifest
            .plugins_sha256
            .keys()
            .filter(|k| !plugins.keys().any(|p| p.eq_ignore_ascii_case(k)))
            .collect();
        let extra: Vec<&String> = plugins
            .keys()
            .filter(|k| {
                !manifest
                    .plugins_sha256
                    .keys()
                    .any(|m| m.eq_ignore_ascii_case(k))
            })
            .collect();
        return Err(format!(
            "更新包 plugins/ 集合与清单绑定不一致（清单 {} 项 / 包内 {} 项；缺失 {:?}；多出 {:?}）",
            manifest.plugins_sha256.len(),
            plugins.len(),
            missing,
            extra
        ));
    }
    for (name, sha) in &manifest.plugins_sha256 {
        let Some(bytes) = plugins.iter().find(|(p, _)| p.eq_ignore_ascii_case(name)) else {
            return Err(format!("更新包缺清单绑定的插件文件：{name}"));
        };
        if !sha.eq_ignore_ascii_case(&sha256_hex(bytes.1)) {
            return Err(format!("插件文件摘要不匹配清单绑定：{name}（完整性问题）"));
        }
    }
    Ok(())
}

/// 完整性绑定纯函数：exe 字节摘要与清单绑定值一致（清单验签 Valid 后调用；
/// 不匹配 = 包被中途调包，无条件拒，allow_downgrade 不豁免）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
fn manifest_exe_digest_matches(manifest: &UpdateManifest, exe_bytes: &[u8]) -> bool {
    manifest
        .exe_sha256
        .eq_ignore_ascii_case(&sha256_hex(exe_bytes))
}

/// `applied_manifest.json` 路径（exe 同目录——上次成功更新写入的已验签清单
/// 载体副本，下次 apply 的顺序基准）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
fn applied_manifest_path(exe: &Path) -> PathBuf {
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("applied_manifest.json")
}

/// 写入 applied 清单副本。**写前再验签**（副本来自 journal 携带字节，boot 期
/// 可能已隔多次重启——不可信输入不落信任锚旁）；验签失败 = Err 不写（下次
/// apply 走 Unknown 拒绝，诚实 fail-closed）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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

/// 本机是否已有「已安装版本」清单基准（applied 副本**文件在场**——只看存在
/// 性，不验签：这里只服务 Dashboard 的「首更形态」提前告知，权威判定永远在
/// apply 的顺序闸里）。false = 存量安装首次更新形态——默认态顺序闸 Unknown
/// 必拒，前端据此在确认框里提前说明豁免语义。
#[cfg(feature = "selfupdate")]
pub(crate) fn sequence_baseline_present(exe: &Path) -> bool {
    applied_manifest_path(exe).exists()
}

/// journal → applied 副本落盘（boot `update_success` 收口接线；R1 写入链
/// 闭环的最后一步）。carrier 为空（旧 journal）= 无事返回 Ok；锚集空 = 诚实
/// 跳过（下次 apply 走 Unknown 拒绝，不假装有基准）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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
// 完整性检查（2026-10-30）：活安装 vs applied_manifest.json 已验签基线。
// 只读不处置——修复路径 = 强制更新（download/apply 带 force）。
// ---------------------------------------------------------------------------

/// 单文件完整性判定。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntegrityFileVerdict {
    /// 与基线一致
    Ok,
    /// sha256 不匹配（损坏/被改）
    Mismatch,
    /// 清单有、盘上无
    Missing,
    /// 读失败（占用/权限）
    Unreadable,
}

/// 完整性总判定。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntegrityVerdict {
    /// 全部一致（extra_files 不影响判定）
    Ok,
    /// 有 mismatch/missing/unreadable
    Mismatch,
    /// 无 applied_manifest.json（手工安装目录/从未走通道更新）——诚实说明，
    /// 不算损坏；「强制更新」可重建基线。
    NoBaseline,
    /// 基线文件在场但读取失败/验签失败/解析失败——基线自身损坏，诚实上报。
    BaselineError,
}

/// 逐文件完整性报告（WSAPI 投影单元）。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct IntegrityFileReport {
    /// 文件名（exe 本体 = 进程可执行文件名；plugins/ 下为平面名）。
    pub name: String,
    pub verdict: IntegrityFileVerdict,
    /// 基线绑定摘要（64 hex）。
    pub expected_sha256: String,
    /// 实际摘要（读到字节时给出；missing/unreadable 为空串）。
    pub actual_sha256: String,
}

/// 完整性总报告（`selfupdate.integrity` 的返回体）。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct IntegrityReport {
    pub verdict: IntegrityVerdict,
    /// 基线身份（人读定位是哪个版本的基线；无基线时为空）。
    pub baseline_commit: String,
    pub baseline_sequence: u64,
    /// 逐文件明细（exe 本体 + plugins/ 清单条目；无基线时为空）。
    pub files: Vec<IntegrityFileReport>,
    /// 盘上有、基线没有（用户自装插件）——**信息性，不算损坏**
    ///（`install_plugins_files` 只同名覆盖不删既有是刻意设计）。
    pub extra_files: Vec<String>,
    /// 人读结论/说明（no_baseline / baseline_error 的原因注记等）。
    pub note: String,
}

/// 完整性纯分类器（可测核心）：给 exe 路径 + 已验签清单，对 exe 本体 +
/// `plugins/` 平面文件逐一 sha256 比对。只读；缺文件=Missing、读失败=
/// Unreadable、不匹配=Mismatch，任一非 Ok 即总判定 Mismatch；
/// extra_files（盘上有、基线没有）只收集不定罪。`read_dir` 结果显式排序
///（目录枚举顺序是平台实现细节，报告必须稳定）。
#[cfg(feature = "selfupdate")]
pub(crate) fn integrity_report_from_manifest(exe: &Path, m: &UpdateManifest) -> IntegrityReport {
    let plugins_dir = exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins");
    let mut files = Vec::new();
    let mut damaged = false;

    // ① exe 本体（活字节 vs 清单绑定值）。
    let exe_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let exe_actual = sha256_file(exe).ok();
    let exe_verdict = match &exe_actual {
        None => {
            damaged = true;
            IntegrityFileVerdict::Unreadable
        }
        Some(h) if h.eq_ignore_ascii_case(&m.exe_sha256) => IntegrityFileVerdict::Ok,
        Some(_) => {
            damaged = true;
            IntegrityFileVerdict::Mismatch
        }
    };
    files.push(IntegrityFileReport {
        name: exe_name,
        verdict: exe_verdict,
        expected_sha256: m.exe_sha256.clone(),
        actual_sha256: exe_actual.unwrap_or_default(),
    });

    // ② plugins/ 清单条目 → 盘上必须存在且摘要一致。
    for (name, expected) in &m.plugins_sha256 {
        let path = plugins_dir.join(name);
        let (verdict, actual) = if !path.exists() {
            damaged = true;
            (IntegrityFileVerdict::Missing, String::new())
        } else {
            match sha256_file(&path) {
                Err(_) => {
                    damaged = true;
                    (IntegrityFileVerdict::Unreadable, String::new())
                }
                Ok(h) if h.eq_ignore_ascii_case(expected) => (IntegrityFileVerdict::Ok, h),
                Ok(h) => {
                    damaged = true;
                    // 摘要不匹配但字节读到了：实际值照报（人读比对现场）。
                    (IntegrityFileVerdict::Mismatch, h)
                }
            }
        };
        files.push(IntegrityFileReport {
            name: name.clone(),
            verdict,
            expected_sha256: expected.clone(),
            actual_sha256: actual,
        });
    }

    // ③ 盘上多出的插件文件（用户自装）——信息性收集，显式排序保稳定。
    let mut extra_files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&plugins_dir) {
        for entry in rd.flatten() {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !m
                .plugins_sha256
                .keys()
                .any(|k| k.eq_ignore_ascii_case(&name))
            {
                extra_files.push(name);
            }
        }
    }
    extra_files.sort();

    let (verdict, note) = if damaged {
        (
            IntegrityVerdict::Mismatch,
            "检测到与已验证基线不一致的文件：可用【强制更新】重下重装修复（本地包残了不用手动去云端下载）。"
                .to_string(),
        )
    } else {
        let mut note = String::from("完整性与已验证基线一致。");
        if !extra_files.is_empty() {
            note.push_str(&format!(
                "另有 {} 个非发布包文件（用户自装，保留不动）。",
                extra_files.len()
            ));
        }
        (IntegrityVerdict::Ok, note)
    };

    IntegrityReport {
        verdict,
        baseline_commit: m.commit.clone(),
        baseline_sequence: m.sequence,
        files,
        extra_files,
        note,
    }
}

/// 完整性检查执行体（可测形态：exe 路径 + 锚集注入——生产锚=manager 侧
/// `builtin_root_anchors`，测试锚=keygen 自产，与 save_applied_manifest /
/// load_applied_sequence 的锚穿参模式同源）。基线 = `applied_manifest.json`
///（上次成功更新写入的**已验签**清单副本）：缺失 = NoBaseline；读取失败/
/// 验签失败/解析失败 = BaselineError（基线自身损坏，诚实上报）。
#[cfg(feature = "selfupdate")]
pub(crate) fn integrity_snapshot_at(exe: &Path, anchors: &[[u8; 32]]) -> IntegrityReport {
    let path = applied_manifest_path(exe);
    if !path.exists() {
        return IntegrityReport {
            verdict: IntegrityVerdict::NoBaseline,
            baseline_commit: String::new(),
            baseline_sequence: 0,
            files: Vec::new(),
            extra_files: Vec::new(),
            note: "本机无已验签基线（applied_manifest.json 不在场：手工安装目录或从未走通道更新），无法做完整性比对；「强制更新」可重建基线。"
                .to_string(),
        };
    }
    let carrier = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return IntegrityReport {
                verdict: IntegrityVerdict::BaselineError,
                baseline_commit: String::new(),
                baseline_sequence: 0,
                files: Vec::new(),
                extra_files: Vec::new(),
                note: format!("基线清单存在但读取失败（{}）：{e}", path.display()),
            };
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match verify_and_parse_manifest(&carrier, anchors, now) {
        Ok(m) => integrity_report_from_manifest(exe, &m),
        Err(e) => IntegrityReport {
            verdict: IntegrityVerdict::BaselineError,
            baseline_commit: String::new(),
            baseline_sequence: 0,
            files: Vec::new(),
            extra_files: Vec::new(),
            note: format!(
                "基线清单验签/解析失败（{e}）：applied_manifest.json 自身可能被篡改或损坏——请用「强制更新」重建。"
            ),
        },
    }
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

/// 检查更新（只读，全平台可用；不下载主包）。`proxy` = 检查链路的下载
/// 代理（`config.proxy.json` selfupdate 段，调用时读盘——保存即对下次
/// 检查/下载生效，免重启；None = 直连）。
pub async fn check(
    current_version: &str,
    current_commit: &str,
    proxy: Option<&str>,
) -> Result<UpdateCheck, String> {
    // Android 走 APK 发行形态，主包通道整体不适用（诚实拒绝而非假装查询）。
    if TARGET_ASSET.is_empty() {
        return Err("当前平台（Android）走 APK 发行形态，不走主包自更新通道。".to_string());
    }
    // 同源解析：与 apply 的下载对象一致（首个含主包资产的 release），消除旧
    // 实现「check 读列表第一条 / apply 下载首个含资产的」双路径分裂——最新
    // 条目尚未产出资产时，展示与安装可能跨版本错位。
    let meta = crate::handlers::release_fetch::fetch_release_for_asset(TARGET_ASSET, proxy).await?;
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
            boot_log_info(&format!(
                "[SelfUpdate] 旧进程已退场（等待 {}ms），接管启动",
                t0.elapsed().as_millis()
            ));
        } else {
            boot_log_warn(&format!(
                "[SelfUpdate] 等旧进程 {pid} 退场超时（{}s），继续启动（端口若被占由 bind 路径诚实报错）",
                WAIT_OLD_PID_TIMEOUT_MILLIS / 1000
            ));
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
                    boot_log_warn(&format!(
                        "[SelfUpdate] applied 清单副本写入失败（收口继续）：{e}"
                    ));
                }
                clear_journal_logged(workspace);
                boot_log_info(&format!(
                    "[SelfUpdate] 启动自检通过：当前 commit 命中目标 {}，更新收口",
                    &journal.target_commit[..journal.target_commit.len().min(12)]
                ));
                #[cfg(feature = "selfupdate")]
                sweep_staging_after_boot(exe);
                BootRecovery {
                    action: "update_success".to_string(),
                    detail: format!("target {}", journal.target_commit),
                }
            } else {
                match rollback_update(&journal, exe) {
                    Ok((detail, noop)) => {
                        clear_journal_logged(workspace);
                        // exe 与 plugins 双双无可恢复现场 = noop，不是回滚成功
                        // ——action 如实分臂，日志表述跟随。
                        let action = if noop { "rollback_noop" } else { "rolled_back" };
                        let verb = if noop {
                            "无可恢复现场（.old 与 plugins staging 双缺席），journal 已清"
                        } else {
                            "已回滚"
                        };
                        boot_log_warn(&format!(
                            "[SelfUpdate] 启动自检失败：当前 commit 未命中目标，{verb}（{detail}）"
                        ));
                        BootRecovery {
                            action: action.to_string(),
                            detail,
                        }
                    }
                    // R2：回滚失败不清 journal——现场（journal + .old + plugins
                    // staging）保留，下次启动重试；旧实现无条件清 journal =
                    // 失败即丢现场。
                    Err(fail) => {
                        boot_log_error(&format!(
                            "[SelfUpdate] 启动自检失败且回滚执行失败：journal 与 staging \
                             现场保留，下次启动将重试回滚（{fail}）"
                        ));
                        BootRecovery {
                            action: "recovery_failed".to_string(),
                            detail: fail,
                        }
                    }
                }
            }
        }
        "staging" => match rollback_update(&journal, exe) {
            Ok((detail, noop)) => {
                clear_journal_logged(workspace);
                let action = if noop { "rollback_noop" } else { "rolled_back" };
                boot_log_warn(&format!("[SelfUpdate] staging 中断现场收口（{detail}）"));
                BootRecovery {
                    action: action.to_string(),
                    detail,
                }
            }
            Err(fail) => {
                boot_log_error(&format!(
                    "[SelfUpdate] staging 中断现场回滚失败：journal 保留，下次启动重试（{fail}）"
                ));
                BootRecovery {
                    action: "recovery_failed".to_string(),
                    detail: fail,
                }
            }
        },
        other => {
            clear_journal_logged(workspace);
            boot_log_warn(&format!(
                "[SelfUpdate] 未知 journal phase {other:?}，已清除"
            ));
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
        // 让位诊断现场与回滚源同目录（old 的扩展位换 failed——旧布局 journal
        // old 在根目录则落根目录、新布局 back/ 抽屉则落 back/，跨版本透明）。
        let failed = old.with_extension("failed");
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
// plugins 整包替换（2026-10-22 扩面；主包自带 plugins/ 子树同版本分发——
// 接口错配根因消失，不需单独打包。机制 = 逐文件 rename dance，与 exe 同
// 语义：Windows 运行中 PE/DLL 可 rename 不可覆盖。boot 收口与 exe 同进同退
// ——journal 记 staging 双目录，杜绝「旧 exe + 新插件」错配）
// ---------------------------------------------------------------------------

/// plugins 回滚 noop 判定前缀（与 exe [`ROLLBACK_NOOP_PREFIX`] 同语义；独立
/// 常量防串判——boot 调用方对两者分别分臂，双 noop 才算 rollback_noop）。
const ROLLBACK_PLUGINS_NOOP_PREFIX: &str = "rollback_plugins_noop";

/// plugins 整包替换 staging 三路径：现有目录（安装根，真实目录不动）/
/// 既有文件让位目录 / 新文件 staging 目录（后两者住 `back/` 抽屉；PID
/// 后缀去固定化，与 exe staging 同语义）。**只识别带 `plugins.` 前缀的
/// 目录，不匹配裸 `plugins`**——真实插件目录永不被清扫。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn plugins_staging_paths(exe: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    (
        dir.join("plugins"),
        dir.join("back").join(format!("plugins.old-{pid}")),
        dir.join("back").join(format!("plugins.new-{pid}")),
    )
}

/// plugins 整包替换舞步（journal 先行落盘后的执行体）：
/// 1. 新文件全集先落 `new_dir/`（fsync 逐件——磁盘满等写错误在任何 rename
///    之前暴露；new_dir 保留全集副本供回滚识别 new-only 文件）；
/// 2. 逐文件（名字排序，确定性）：既有同名让位 `plugins/X` → `backup/X`
///    （映射中的 DLL 让位成功——Windows 运行中 PE 可 rename 不可覆盖；
///    目录不整迁同理）→ 新文件 copy 就位（copy 非 rename：new_dir 全集
///    副本不被消耗）。
///
/// **只同名覆盖不删既有**（用户拍板 ②）——用户自装插件不受伤。同名字段
/// 有**类型边界**：包内条目是平面文件，既有同名条目若是**目录**，让位舞步
/// 会把整个目录迁进 backup，而回滚名单只认文件（is_file）、收尾整删
/// backup = 用户目录内容永久丢失——文件-目录类型冲突不属于「同名覆盖」，
/// 先于任何落盘诚实拒绝。返回替换文件数；任何一步失败 = Err（journal 已留
/// 现场，boot 收口回滚）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn install_plugins_files(
    plugins_dir: &Path,
    backup_dir: &Path,
    new_dir: &Path,
    files: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<usize, String> {
    // 目录冲突预检（先于 new_dir/backup 任何落盘）：拒绝时现场零扰动。
    for name in files.keys() {
        let existing = plugins_dir.join(name);
        if existing.is_dir() {
            return Err(format!(
                "既有同名条目是目录（{}）：更新包条目是平面文件，类型冲突不做\
                 覆盖（保护用户自装内容）；请手工移走该目录后重试",
                existing.display()
            ));
        }
    }
    std::fs::create_dir_all(new_dir).map_err(|e| format!("plugins staging 目录创建失败：{e}"))?;
    for (name, bytes) in files {
        let target = new_dir.join(name);
        let mut f = std::fs::File::create(&target)
            .map_err(|e| format!("插件新文件落盘失败（{}）：{e}", target.display()))?;
        std::io::Write::write_all(&mut f, bytes)
            .map_err(|e| format!("插件新文件写入失败（{}）：{e}", target.display()))?;
        f.sync_all()
            .map_err(|e| format!("插件新文件同步失败（{}）：{e}", target.display()))?;
    }
    std::fs::create_dir_all(backup_dir).map_err(|e| format!("plugins 备份目录创建失败：{e}"))?;
    std::fs::create_dir_all(plugins_dir).map_err(|e| format!("plugins 目录创建失败：{e}"))?;
    let mut replaced = 0usize;
    for name in files.keys() {
        let existing = plugins_dir.join(name);
        if existing.exists() {
            let backup = backup_dir.join(name);
            std::fs::rename(&existing, &backup).map_err(|e| {
                format!(
                    "既有插件让位失败（{} → {}）：{e}",
                    existing.display(),
                    backup.display()
                )
            })?;
        }
        let src = new_dir.join(name);
        let dst = plugins_dir.join(name);
        std::fs::copy(&src, &dst).map_err(|e| {
            format!(
                "新插件就位失败（{} → {}）：{e}",
                src.display(),
                dst.display()
            )
        })?;
        replaced += 1;
    }
    Ok(replaced)
}

/// 旁置名计算（纯函数，单测钉形态）：原文件名**追加** `.failed-{pid}` 后缀。
/// 不用 `with_extension`（那是**替换**扩展位）——同词干不同扩展名（`a.dll`/
/// `a.wasm`）会撞同一个 `a.failed-{pid}`：首份映射现场占着旁置名时第二份
/// rename 撞已存在目标直接失败，boot 回滚卡死循环；追加形态每源文件唯一。
fn loaded_sidecar_path(target: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(target.file_name().unwrap_or_default());
    name.push(format!(".failed-{}", std::process::id()));
    target.with_file_name(name)
}

/// 被映射文件删除（boot 回滚场景：目标位可能被运行中进程映射——当前
/// gateway 进程的插件 DLL 常态在内存）。文件不可删时先让位 `.failed-{pid}`
/// 旁置再删旁置件：未映射场景旁置件删得掉；映射中留下（计划钦定的诊断
/// 现场，boot 收口不追逐）。
fn remove_loaded_file(target: &Path) -> Result<(), String> {
    match std::fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            let failed = loaded_sidecar_path(target);
            let _ = std::fs::remove_file(&failed);
            std::fs::rename(target, &failed)
                .map_err(|e| format!("映射文件让位失败（{}）：{e}", target.display()))?;
            let _ = std::fs::remove_file(&failed);
            Ok(())
        }
    }
}

/// plugins 回滚本体（清单驱动，2026-10-24 N1 根修）。幂等 + 诚实化：Ok =
/// 回滚完成（detail 区分 noop）/ Err = 失败原因（调用方保 journal，下次
/// boot 重试）。
///
/// **清单是唯一真相源**（journal 构造时在任何落盘前计算的逐文件意图快照）：
/// - `new_install=false`（替换）：从 backup 归位。**backup 缺席 = 跳过**——
///   两种成因（舞步从未让位到该文件 / 上轮回滚已归位）都意味着 plugins/
///   里已是旧版，无需也不得动作（旧实现按「backup 名单差集」推断 new-only，
///   在这两个窗口会把旧插件当新装删掉——W0′ 与部分失败重试两个崩溃窗口
///   的数据丢失根因）。
/// - `new_install=true`（新装）：plugins/ 有则删（无旧版可回，删除归零）。
///
/// 旧 journal（本修复前写入，清单缺键 = 空）走保守路径：只归位 backup，
/// 不做任何删除（差集推断不可信），new_dir 留待人工核查（detail 带路径）。
///
/// 归位让位舞步：`plugins/X` 被运行中进程映射（boot 收口的常态）时直接
/// rename(backup→plugins) 会撞占用——先把目标 [`remove_loaded_file`] 让位。
/// 收尾双目录韧性删除（失败留清扫，不影响回滚结论——重入时条目均已
/// 收敛，剩余目录按 noop 收口）。
fn rollback_plugins_to(
    backup_dir: &str,
    new_dir: &str,
    manifest: &[PluginsFileEntry],
    plugins_dir: &Path,
) -> Result<String, String> {
    if backup_dir.is_empty() && new_dir.is_empty() {
        return Ok(format!(
            "{ROLLBACK_PLUGINS_NOOP_PREFIX}: 无 plugins 舞步记录（旧 journal/无插件包）"
        ));
    }
    let backup = PathBuf::from(backup_dir);
    let new = PathBuf::from(new_dir);
    if manifest.is_empty() {
        return rollback_plugins_legacy(&backup, &new, plugins_dir);
    }
    // 归位：清单 Replaced 条目逐个从 backup 归位（backup 缺席 = 跳过）。
    let mut restored = 0usize;
    for entry in manifest.iter().filter(|e| !e.new_install) {
        let src = backup.join(&entry.name);
        if !src.exists() {
            continue;
        }
        let dst = plugins_dir.join(&entry.name);
        if dst.exists() {
            remove_loaded_file(&dst)?;
        }
        std::fs::rename(&src, &dst)
            .map_err(|e| format!("插件归位失败（{} → {}）：{e}", src.display(), dst.display()))?;
        restored += 1;
    }
    // 新装清理：清单 NewInstall 条目从 plugins/ 删除（真相源是清单，与
    // new/backup 目录现场形态无关）。
    let mut removed = 0usize;
    for entry in manifest.iter().filter(|e| e.new_install) {
        let dst = plugins_dir.join(&entry.name);
        if dst.exists() {
            remove_loaded_file(&dst)?;
            removed += 1;
        }
    }
    // 收尾：双 staging 目录韧性删除（内容已耗尽/清空；删除失败 = Windows
    // 瞬时句柄常态，留清扫不阻塞回滚结论）。
    if backup.exists() {
        crate::handlers::release_fetch::remove_dir_all_resilient(&backup);
    }
    if new.exists() {
        crate::handlers::release_fetch::remove_dir_all_resilient(&new);
    }
    if restored == 0 && removed == 0 {
        // 无任何文件需要动作（舞步从未开始/已被上轮收口清光/全部条目已
        // 收敛）= noop。
        return Ok(format!(
            "{ROLLBACK_PLUGINS_NOOP_PREFIX}: 无需归位/清理（现场已收敛）"
        ));
    }
    Ok(format!(
        "plugins 已回滚（归位 {restored} 个 + 清理新装 {removed} 个，plugins/ = {}）",
        plugins_dir.display()
    ))
}

/// 旧 journal 保守回滚（清单缺键）：只归位 backup 里真实在场的文件，**不做
/// 任何删除**——「new_dir − backup」差集推断在 backup 缺席/部分归位窗口会
/// 把旧插件误判成新装（N1 数据丢失根因），清单缺失时宁留毋删；new_dir 残
/// 留物在 detail 里报路径供人工核查（删错了找不回，留着最多版本错配）。
fn rollback_plugins_legacy(
    backup: &Path,
    new: &Path,
    plugins_dir: &Path,
) -> Result<String, String> {
    let mut restored = 0usize;
    if backup.exists() {
        let entries = std::fs::read_dir(backup)
            .map_err(|e| format!("plugins 备份目录读取失败（{}）：{e}", backup.display()))?;
        let mut backup_names: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        backup_names.sort();
        for name in &backup_names {
            let dst = plugins_dir.join(name);
            if dst.exists() {
                remove_loaded_file(&dst)?;
            }
            let src = backup.join(name);
            std::fs::rename(&src, &dst).map_err(|e| {
                format!("插件归位失败（{} → {}）：{e}", src.display(), dst.display())
            })?;
            restored += 1;
        }
    }
    if backup.exists() {
        crate::handlers::release_fetch::remove_dir_all_resilient(backup);
    }
    let leftover = if new.exists() {
        crate::handlers::release_fetch::remove_dir_all_resilient(new);
        if new.exists() {
            format!("；new staging 删除失败残留 {}", new.display())
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    if restored == 0 {
        return Ok(format!(
            "{ROLLBACK_PLUGINS_NOOP_PREFIX}: 旧 journal 无清单（保守模式：无可归位文件）\
             {leftover}"
        ));
    }
    Ok(format!(
        "plugins 已回滚（旧 journal 无清单，保守模式：归位 {restored} 个，\
         未做新装清理{leftover}）"
    ))
}

/// journal → plugins 回滚（boot 收口 / apply 内联双回滚共用入口）。双字段
/// 空 = noop（无舞步记录）。
fn rollback_plugins_from_journal(journal: &UpdateJournal, exe: &Path) -> Result<String, String> {
    let plugins_dir = exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins");
    rollback_plugins_to(
        &journal.plugins_backup_dir,
        &journal.plugins_new_dir,
        &journal.plugins_files,
        &plugins_dir,
    )
}

/// boot/apply 失败路径的双回滚统一收口（exe + plugins 同进同退）。
/// Ok((detail, all_noop))：两者皆成功；all_noop = exe `.old` 与 plugins
/// staging 双双缺席（无可恢复现场）。Err：任一回滚失败（journal 保留，
/// 下次 boot 幂等重试——届时已成功的一侧自动 noop 收敛）。
fn rollback_update(journal: &UpdateJournal, exe: &Path) -> Result<(String, bool), String> {
    let exe_res = rollback_old_exe_to(&journal.old_path, exe);
    let plug_res = rollback_plugins_from_journal(journal, exe);
    match (exe_res, plug_res) {
        (Ok(d1), Ok(d2)) => {
            let noop = d1.starts_with(ROLLBACK_NOOP_PREFIX)
                && d2.starts_with(ROLLBACK_PLUGINS_NOOP_PREFIX);
            Ok((format!("{d1}；{d2}"), noop))
        }
        (Err(e1), Err(e2)) => Err(format!("{e1}；{e2}")),
        (Err(e), _) => Err(e),
        (_, Err(e)) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// apply 互斥原语（R2：单飞 + 跨进程锁 + staging 名去固定化）
// ---------------------------------------------------------------------------

/// staging 抽屉（2026-10-27 用户拍板）：`<exe_dir>/back/`——apply 舞步五件套
/// （`.old-{pid}` / `.new-{pid}` / `.failed` / `plugins.{old,new}-{pid}/`）的
/// 专属目录。安装根目录不再出现散件（收尾清扫失败也不裸露）；`updates/`
/// （下载缓存）与 `back/`（舞步与回滚源活区）物理隔离——updates/ 的任何
/// 整清都碰不到回滚源。exe 同级 = 同卷子目录，Windows 运行中 PE rename
/// 语义照旧；不放安装目录外面（Program Files 场景创建不了同级目录且污染
/// 公共位置）。
#[cfg(feature = "selfupdate")]
fn back_dir(exe: &Path) -> PathBuf {
    exe.parent().unwrap_or_else(|| Path::new(".")).join("back")
}

/// staging 临时名（PID 后缀去固定化）：`back/<stem>.old-{pid}` /
/// `back/<stem>.new-{pid}`。双进程并发 apply 不再互踩同一文件名（boot 收口
/// 只认 journal.old_path 记录的全路径，无固定名依赖）。stem 取法与旧
/// `with_extension` 形态逐字节一致（`nemesisbot.exe` → `nemesisbot.old-{pid}`）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn staging_paths(exe: &Path) -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let stem = exe
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("nemesisbot");
    let dir = back_dir(exe);
    (
        dir.join(format!("{stem}.old-{pid}")),
        dir.join(format!("{stem}.new-{pid}")),
    )
}

/// 安装目录级跨进程锁：`<exe_dir>/selfupdate.lock` 的 OS 文件锁
/// （`File::try_lock`，进程死 OS 自动释放——无陈锁清理问题）。Ok=锁文件句柄
/// （**必须保活到 apply 结束**，drop 即释放）；Err=另一进程正在自更新。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
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

/// 陈旧 staging 残留清扫（R2 补口 + 2026-10-27 back/ 抽屉迁位）：
///
/// **新布局**：`back/` 抽屉整目录韧性删除——持安装目录锁窗口内不可能有
/// 并发 apply，journal 闸（apply 1b 先于本清扫）保证在场 journal 已被 boot
/// 收口处理，`back/` 内不存在活回滚源；抽屉内全部内容（含历史 `.failed`
/// 诊断现场——诊断已被 boot 收口 detail 记录过）随整删一起走。AV/索引器
/// 瞬时句柄删不掉 = 整目录保留留给下次（best-effort）。
///
/// **旧布局过渡兼容**（迁位前的安装根目录残留，老用户升级后也收干净）：
/// 根目录 `.old-{pid}`/`.new-{pid}` 死进程遗留逐文件删（仍被在跑旧进程
/// 镜像锁住的 PE 删除失败属预期）；根目录 `.failed` 是旧布局钦定的诊断
/// 现场，过渡清扫不清。**plugins 侧同批清扫**：根目录 `plugins.old-{pid}`/
/// `plugins.new-{pid}` 整目录韧性删除（裸 `plugins` 真实插件目录永不在
/// 清扫面）。返回删除数（审计观测用）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn sweep_stale_staging(exe: &Path) -> usize {
    let Some(dir) = exe.parent() else {
        return 0;
    };
    let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) else {
        return 0;
    };
    let mut removed = 0;
    // 新布局：back/ 抽屉整删（存在才删；不存在 = 0）。
    let back = back_dir(exe);
    if back.is_dir() && crate::handlers::release_fetch::remove_dir_all_resilient(&back) {
        removed += 1;
    }
    let old_prefix = format!("{stem}.old-");
    let new_prefix = format!("{stem}.new-");
    let plugins_old_prefix = "plugins.old-";
    let plugins_new_prefix = "plugins.new-";
    let failed_name = format!("{stem}.failed");
    // 过渡兼容：删除与遍历顺序无关（集合操作），无需排序。
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let is_staging = name.starts_with(&old_prefix) || name.starts_with(&new_prefix);
            if is_staging {
                if std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
                continue;
            }
            if name == failed_name {
                continue; // 旧布局 .failed 诊断现场，过渡清扫不清
            }
            // plugins staging 目录（前缀匹配，后缀须还有 PID 段——裸 `plugins`
            // 与真实插件目录不沾边）。
            let is_plugins_staging = (name.starts_with(plugins_old_prefix)
                || name.starts_with(plugins_new_prefix))
                && name != "plugins.old-"
                && name != "plugins.new-";
            if is_plugins_staging
                && crate::handlers::release_fetch::remove_dir_all_resilient(&entry.path())
            {
                removed += 1;
            }
        }
    }
    removed
}

/// 启动收口成功后的 staging 残留清扫（用户可见面：更新成功重启后 `back/`
/// 抽屉与旧布局根目录残留不该等下次 apply 才消失）。安全论证：走到这里 =
/// 本进程就是刚更新成功的新进程，且旧进程已退场（recover_on_boot_in ① 的
/// wait-for-exit 通过才会到收口）——自己上一轮留下的 `back/`（`.old-{旧pid}`
/// / `plugins.{old,new}-{pid}`）镜像锁已随旧进程消失，此刻整删是安全的。
/// 回滚分支绝不调用（rollback 消耗 `.old`，扫了就丢回滚源）。仍取安装
/// 目录锁：锁被占 = 同目录并发 apply 在飞，跳过（boot 抢扫反而可能删掉
/// 在飞 staging；它的 apply 起步 sweep 会清）。best-effort：AV/索引器瞬时
/// 句柄删不掉属预期，留下次。
#[cfg(feature = "selfupdate")]
fn sweep_staging_after_boot(exe: &Path) {
    let Some(dir) = exe.parent() else {
        return;
    };
    if dir.as_os_str().is_empty() {
        return; // 相对路径 exe（测试形态）——空 dir 会让锁文件落进程 cwd
    }
    let Ok(lock) = acquire_install_lock(dir) else {
        return; // 并发 apply 持锁在飞——不抢扫
    };
    let removed = sweep_stale_staging(exe);
    drop(lock);
    if removed > 0 {
        boot_log_info(&format!(
            "[SelfUpdate] 启动收口清扫 staging 残留 {removed} 项"
        ));
    }
}

// ---------------------------------------------------------------------------
// SelfUpdateManager（apply 编排；selfupdate feature 门控）
// ---------------------------------------------------------------------------

/// 下载进度快照（WSAPI status 轮询兜底 + SSE 之外的第二观测面）。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    /// 是否有下载任务在跑（受理即置 true，成功/失败收口置 false）。
    pub active: bool,
    /// `idle` | `downloading` | `ready` | `error`。
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    /// 最近一次失败文案（phase=error 时非空）。
    pub error: Option<String>,
}

#[cfg(feature = "selfupdate")]
impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            active: false,
            phase: "idle".to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
}

/// 最近一次 apply 编排失败的反馈面（2026-10-23 挂死修复）。apply 是受理即
/// 回的后台编排——失败若只落 gateway 日志，Dashboard 的「更新执行中」粘滞
/// 态永远等不到收口信号。Ok 即清；重试发起时由 apply 包装层先行清空（编排
/// 中不残留旧错）。
#[cfg(feature = "selfupdate")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyErrorRecord {
    /// 人读失败原因（各闸 Err 文案原样）。
    pub message: String,
    /// 失败闸 = 顺序判定拒绝类（清单缺失/验签失败 → Unknown、sequence 降级
    /// /未知）——Dashboard 据此提供显式豁免重试（签名验真照常，不豁免）。
    pub downgrade_denied: bool,
    /// 失败时刻（ms epoch）。
    pub at_ms: u64,
}

/// apply / 下载编排器。gateway run_runtime 装配点构建（审计链 / estop 同源），
/// 注入 WSAPI 槽 + 内部命令臂 + 启动自动 check。`shutdown` 闭包 = 既有优雅
/// 停机路径，注入避免本模块反向依赖 bin 类型。
#[cfg(feature = "selfupdate")]
pub struct SelfUpdateManager {
    pub workspace: PathBuf,
    pub estop: std::sync::Arc<nemesis_agent::estop::EstopState>,
    pub audit: Option<std::sync::Arc<nemesis_security::pipeline::SecurityPlugin>>,
    pub shutdown: std::sync::Arc<dyn Fn() + Send + Sync>,
    /// R2 进程内单飞闸：同一 manager 上并发 apply 恰一进入（try_lock 失败 =
    /// 「已有自更新在进行」，诚实拒绝）。与 [`acquire_install_lock`]（安装
    /// 目录跨进程锁）两层互补。
    pub flight: tokio::sync::Mutex<()>,
    /// 下载单飞闸（与 apply 的 [`SelfUpdateManager::flight`] 分立：apply 会
    /// `.lock().await` 这把锁等在途下载收口后再读 pending，反向则不会——
    /// 下载方永不等待 apply）。下载任务自身 try_lock，占用中 = 幂等受理。
    pub download_flight: tokio::sync::Mutex<()>,
    /// 下载取消旗标（WSAPI cancel_download 置位；进度回调逐 chunk 轮询，
    /// 返 false = 中止并整清 updates/）。
    pub download_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 进度快照槽（SSE 之外的轮询观测面）。
    pub download_progress: std::sync::Arc<parking_lot::Mutex<DownloadProgress>>,
    /// 最近一次 check 结果缓存（gateway 启动 30s 自动 check 写入；WSAPI
    /// status 直接读，前端免再等一次网络往返）。
    pub last_check: std::sync::Arc<parking_lot::Mutex<Option<(UpdateCheck, u64)>>>,
    /// 最近一次 apply 编排失败（None = 无/已清）。受理即回形态下这是
    /// Dashboard 的持久失败信号（SSE error 帧只达在线订阅者，status 轮询
    /// 兜底断线/刷新场景）。
    pub last_apply_error: std::sync::Arc<parking_lot::Mutex<Option<ApplyErrorRecord>>>,
    /// 顺序判定拒绝旗标（闸①清单缺失/验签失败、闸⑤ sequence 拒绝时置位；
    /// apply 包装层收口时折进 [`ApplyErrorRecord::downgrade_denied`]）。
    pub downgrade_denied: std::sync::Arc<std::sync::atomic::AtomicBool>,
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

    /// 最近一次 apply 失败快照（None = 无在案失败）。status 投影用。
    pub fn last_apply_error_snapshot(&self) -> Option<ApplyErrorRecord> {
        self.last_apply_error.lock().clone()
    }

    /// 下载待装包（WSAPI `selfupdate.download` 的执行体；调用方负责放进
    /// tokio 任务——受理即回）。幂等语义：pending 已就绪直接返回；在途下载
    /// 中重复调用幂等受理。estop 生效 = 拒绝（下载段同受急停冻结）。
    ///
    /// `force`（强制更新，2026-10-30）：跳过「pending 已就绪」幂等早退——
    /// 存量 pending 可能就是残包，强制更新必须重拉。新鲜度闸仅此一道；
    /// estop/单飞锁照常。
    pub async fn download(&self, current_commit: &str, force: bool) -> Result<String, String> {
        let exe_path = std::env::current_exe().map_err(|e| format!("取 current_exe 失败：{e}"))?;
        if !force && let Some(p) = load_pending(&exe_path) {
            return Ok(format!(
                "待装包已就绪（{}，commit {}，{}MB）：直接【开始更新】即可。",
                p.release_tag,
                &p.target_commit[..p.target_commit.len().min(12)],
                p.size_bytes / (1024 * 1024)
            ));
        }
        if force {
            self.audit_event(
                "selfupdate",
                "allowed",
                "download: force (freshness gate bypassed)",
            );
        }
        if self.estop.is_engaged() {
            self.audit_event("selfupdate", "denied", "download: estop engaged");
            return Err("急停（estop）生效中：更新下载被冻结。释放急停后重试。".to_string());
        }
        match self.download_flight.try_lock() {
            Ok(_g) => {
                self.download_to_pending(&exe_path, current_commit, force)
                    .await
            }
            Err(_) => {
                if self.progress_snapshot().active {
                    // 幂等受理：同一目标已在下，调用方无需等待。
                    self.audit_event("selfupdate", "allowed", "download idempotent: in-flight");
                    return Ok(
                        "下载已在进行中（本次调用幂等受理）；进度看 SSE selfupdate-progress。"
                            .to_string(),
                    );
                }
                Err("下载通道被占用（锁在但无活跃进度，异常态）：稍后重试。".to_string())
            }
        }
    }

    /// 取消在途下载（WSAPI `selfupdate.cancel_download`）。旗标置位，下载
    /// 回调下一个数据块即中止并整清 updates/——回 [可下载] 态。
    pub fn cancel_download(&self) {
        self.download_cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.audit_event("selfupdate", "allowed", "download cancel requested");
    }

    /// 进度快照（WSAPI status 轮询面）。
    pub fn progress_snapshot(&self) -> DownloadProgress {
        self.download_progress.lock().clone()
    }

    /// check 结果缓存写入（gateway 启动自动 check / WSAPI check 成功后调）。
    pub fn store_check(&self, c: UpdateCheck) {
        *self.last_check.lock() = Some((c, now_ms()));
    }

    /// check 结果缓存读取（WSAPI status 用；带写入时刻供 age 展示）。
    pub fn cached_check(&self) -> Option<(UpdateCheck, u64)> {
        self.last_check.lock().clone()
    }

    /// 完整性检查（WSAPI `selfupdate.integrity`；只读、本地哈希、同步秒级，
    /// 无网络依赖）。基线与判定语义见 [`integrity_snapshot_at`]。
    pub(crate) fn integrity_snapshot(&self) -> IntegrityReport {
        match std::env::current_exe() {
            Ok(exe) => {
                let anchors = nemesis_verify::builtin_root_anchors();
                integrity_snapshot_at(&exe, &anchors)
            }
            Err(e) => IntegrityReport {
                verdict: IntegrityVerdict::BaselineError,
                baseline_commit: String::new(),
                baseline_sequence: 0,
                files: Vec::new(),
                extra_files: Vec::new(),
                note: format!("取 current_exe 失败：{e}"),
            },
        }
    }

    /// 下载编排：同源解析 → 干净起步 → 流式落 .part → rename 就位 →
    /// sha256 → meta 后写。任一步失败 = 整清 updates/ + error 进度帧 +
    /// 审计（调用方拿 Err）。进度经 SSE（100ms 节流）+ 进度槽双面可见。
    async fn download_to_pending(
        &self,
        exe_path: &Path,
        current_commit: &str,
        force: bool,
    ) -> Result<String, String> {
        *self.download_progress.lock() = DownloadProgress {
            active: true,
            phase: "downloading".to_string(),
            ..Default::default()
        };
        self.download_cancel
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let result = self
            .download_to_pending_inner(exe_path, current_commit, force)
            .await;
        match result {
            Ok(msg) => {
                let (done, total) = {
                    let mut p = self.download_progress.lock();
                    p.active = false;
                    p.phase = "ready".to_string();
                    (p.downloaded_bytes, p.total_bytes)
                };
                publish_progress("ready", done, total, None);
                self.audit_event("selfupdate", "allowed", &msg);
                Ok(msg)
            }
            Err(e) => {
                clear_updates_dir(exe_path);
                let mut p = self.download_progress.lock();
                p.active = false;
                p.phase = "error".to_string();
                p.error = Some(e.clone());
                drop(p);
                publish_progress("error", 0, None, Some(&e));
                self.audit_event("selfupdate", "denied", &format!("download: {e}"));
                Err(e)
            }
        }
    }

    async fn download_to_pending_inner(
        &self,
        exe_path: &Path,
        current_commit: &str,
        force: bool,
    ) -> Result<String, String> {
        // Android 走 APK 发行形态，主包通道整体不适用。
        if TARGET_ASSET.is_empty() {
            return Err("当前平台（Android）走 APK 发行形态，不走主包自更新通道。".to_string());
        }
        // 同源解析（check / 早闸 / 下载同一对象）。下载代理调用时读盘
        //（selfupdate 段热生效语义：下载是逐次操作，保存即对下次生效）。
        let proxy =
            nemesis_config::proxy_file::load_selfupdate_proxy(&self.workspace.join("config"));
        let meta =
            crate::handlers::release_fetch::fetch_release_for_asset(TARGET_ASSET, proxy.as_deref())
                .await?;
        if !meta.has_asset(TARGET_ASSET) {
            return Err(format!(
                "最新 Release（{}）尚未产出 {TARGET_ASSET} 附件（CI 尚未产出该包）",
                meta.tag_name
            ));
        }
        // 同版新鲜度闸：force（强制更新）跳过——同版也重下（重装当前版本
        // 是强制更新的合法意志）。
        if !force
            && let Some(latest) = meta.body_commit()
            && commit_matches(current_commit, &latest)
        {
            return Err(format!(
                "当前已是最新版本（commit {}），无需下载。",
                &latest[..latest.len().min(12)]
            ));
        }
        let url = meta
            .asset_url(TARGET_ASSET)
            .ok_or_else(|| format!("Release {} 缺少 {TARGET_ASSET} 下载地址", meta.tag_name))?
            .to_string();
        let release_tag = meta.tag_name.clone();
        let target_commit = meta.body_commit().unwrap_or_default();
        // total 优先源 = GitHub API asset size（S3 预签 URL 也稳定给数）>
        // Content-Length > 未知（进度条显示已下载字节数）。先种子进度槽，
        // 回调只在其缺位时才采信 Content-Length（优先级不倒挂）。
        let hint_total = meta.asset_size(TARGET_ASSET);
        {
            let mut p = self.download_progress.lock();
            p.total_bytes = hint_total;
        }
        // 干净起步（用户拍板：异常/中断不恢复不续传，整清重来；.part 残留同清）。
        clear_updates_dir(exe_path);
        let part = pending_package_part_path(exe_path, TARGET_ASSET);
        // 进度回调（Fn 闭包——可变状态全部走 Arc 槽；SSE 100ms 节流用
        // AtomicU64 存上次发射时刻，compare_exchange 防并发抖动）。
        let cancel = self.download_cancel.clone();
        let estop = self.estop.clone();
        let progress_slot = self.download_progress.clone();
        let last_emit = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last_emit_cb = last_emit.clone();
        let downloaded = crate::handlers::release_fetch::download_to_file(
            &url,
            &part,
            MAX_UPDATE_ASSET_BYTES,
            move |done, total| {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) || estop.is_engaged() {
                    return false; // 中止：下载已取消 / 急停冻结
                }
                let effective_total = hint_total.or(total);
                {
                    let mut p = progress_slot.lock();
                    p.downloaded_bytes = done;
                    if p.total_bytes.is_none() {
                        p.total_bytes = total;
                    }
                }
                let now = now_ms();
                let last = last_emit_cb.load(std::sync::atomic::Ordering::SeqCst);
                if now.saturating_sub(last) >= 100
                    && last_emit_cb
                        .compare_exchange(
                            last,
                            now,
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                        )
                        .is_ok()
                {
                    publish_progress("downloading", done, effective_total, None);
                }
                true
            },
            proxy.as_deref(),
        )
        .await
        .map_err(|e| format!("下载 {TARGET_ASSET} 失败：{e}"))?;
        // .part → pending 包正式名（读侧只见完整文件）。
        let final_pkg = pending_package_path(exe_path, TARGET_ASSET);
        std::fs::rename(&part, &final_pkg).map_err(|e| {
            format!(
                "待装包就位失败（{} → {}）：{e}",
                part.display(),
                final_pkg.display()
            )
        })?;
        let pkg_sha256 = sha256_file(&final_pkg)?;
        let pending = PendingUpdate {
            asset: TARGET_ASSET.to_string(),
            release_tag,
            target_commit,
            zip_sha256: pkg_sha256,
            size_bytes: downloaded,
            downloaded_at_ms: now_ms(),
        };
        // meta 后写：见 meta 必有完整 zip。
        save_pending(exe_path, &pending)?;
        let desc = format!(
            "待装包已就绪（{}，commit {}，{}MB）",
            pending.release_tag,
            &pending.target_commit[..pending.target_commit.len().min(12)],
            pending.size_bytes / (1024 * 1024)
        );
        tracing::info!("[SelfUpdate] {desc}");
        Ok(desc)
    }

    /// apply（**Windows / Linux** 全链路；macOS 留位未接 CI 产物、Android 走
    /// APK 形态，两者由 stub 变体诚实拒绝——见下方 cfg 分体）。Ok = staging
    /// 完成且新进程已拉起、本进程将优雅退出；Err = 人读失败原因（每步失败都
    /// 带现场 + 审计留痕）。
    ///
    /// 外层 [`SelfUpdateManager::apply`] 是全平台失败反馈面包装；本体在此
    /// （`apply_inner`）。
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    async fn apply_inner(
        &self,
        current_version: &str,
        current_commit: &str,
        allow_downgrade: bool,
        force: bool,
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
        // ── 步骤 1d-2：安装目录跨进程锁（R2）——从下载到 staging 全程持有，
        //    双 gateway 进程共享安装目录时只允许一个自更新执行。闸在快闸
        //    之后（estop/journal 拒绝路径不碰锁文件），pending 就位之前
        //    （自动下载期就占锁，串行化整个 apply）。
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
        // ── 步骤 1e：pending 就位（两段式 apply 起点 = 本地待装包）。无
        //    pending 自动先下载（CLI 兼容：旧入口一步式调用无需先行
        //    download）；持 download_flight 串行——与在途后台下载互斥，收口
        //    后再读盘（两把锁独立：apply 持 flight 时再取 download_flight
        //    不构成死锁环）。
        let pending = {
            let _df = self.download_flight.lock().await;
            let mut pending = load_pending(&exe_path);
            if pending.is_none() {
                self.audit_event("selfupdate", "allowed", "no pending: auto-download");
                self.download_to_pending(&exe_path, current_commit, force)
                    .await?;
                pending = load_pending(&exe_path);
            }
            pending
        }
        .ok_or_else(|| "下载完成但待装包元数据读取失败（updates/ 已清空）：请重试。".to_string())?;
        // ── 步骤 1e-2：发布源对账（轻量元信息拉取）。发布源比 pending 新 →
        //    作废重下；已最新 → 清空拒绝（force 豁免此臂——强制更新的意志是
        //    重装，同版照装）。列表拉取失败 → 诚实 warn 按 pending 继续不给
        //    断网用户装不了已下载更新的死锁——身份权威始终是包内签名清单，
        //    此处只是新鲜度对账。对账拉取同样走下载代理（调用时读盘）。
        let proxy =
            nemesis_config::proxy_file::load_selfupdate_proxy(&self.workspace.join("config"));
        match crate::handlers::release_fetch::fetch_release_list(proxy.as_deref()).await {
            Ok(list) => {
                let latest =
                    crate::handlers::release_fetch::resolve_release_for_asset(&list, TARGET_ASSET)
                        .and_then(|meta| meta.body_commit())
                        .unwrap_or_default();
                if !force && !latest.is_empty() && commit_matches(current_commit, &latest) {
                    clear_updates_dir(&exe_path);
                    self.audit_event("selfupdate", "denied", "reconcile: already at latest");
                    return Err(format!(
                        "当前已是最新版本（commit {}），待装包已清空。",
                        &latest[..latest.len().min(12)]
                    ));
                }
                if !latest.is_empty()
                    && !pending.target_commit.is_empty()
                    && !commit_matches(&pending.target_commit, &latest)
                {
                    clear_updates_dir(&exe_path);
                    self.audit_event(
                        "selfupdate",
                        "denied",
                        "reconcile: pending stale vs release source",
                    );
                    return Err(format!(
                        "本地待装包（{}）已过期（发布源已有更新版本 {}）：已清空，\
                         请重新下载。",
                        pending.release_tag,
                        &latest[..latest.len().min(12)]
                    ));
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[SelfUpdate] 发布源对账拉取失败：按本地待装包继续（身份权威=包内签名清单）"
                );
                self.audit_event(
                    "selfupdate",
                    "allowed",
                    "reconcile: list fetch failed, proceed with pending",
                );
            }
        }
        // ── R1 单候选链退役：包字节从本地待装包读入（下载段已前移至
        //    download_to_pending）。字节入内存后，后续任何清 updates/ 的路径
        //    都不再影响本次 apply。解包取新 exe / version.txt / 签名清单 /
        //    plugins/ 子树（zip=Windows 主包，tar.gz=Linux/macOS 主包，按
        //    魔数分派不按平台）。
        let pkg_path = pending_package_path(&exe_path, &pending.asset);
        let pkg_bytes = std::fs::read(&pkg_path).map_err(|e| {
            clear_updates_dir(&exe_path);
            let msg = format!(
                "读取待装包失败（{}）：{e}（updates/ 已清空）",
                pkg_path.display()
            );
            self.audit_event("selfupdate", "denied", &msg);
            msg
        })?;
        // 解包失败 = 包结构损坏（sha 对但 zip/tar 坏）：与 load_pending 的
        // 「坏包宁可重下」同策略——整清 updates/，不给永久失败循环留现场。
        let extracted = extract_update_bundle(&pkg_bytes).map_err(|e| {
            clear_updates_dir(&exe_path);
            let msg = format!("解包更新包失败：{e}（updates/ 已清空，请重新下载）");
            self.audit_event("selfupdate", "denied", &msg);
            msg
        })?;

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
                    self.downgrade_denied
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    self.audit_event("manifest", "denied", &e);
                    return Err(format!(
                        "{e}：无法证明版本顺序，更新已拒绝。确认要继续请带 \
                         --allow-downgrade 重试（该豁免仅放开顺序判定，签名验真\
                         照常执行）。"
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
                    self.downgrade_denied
                        .store(true, std::sync::atomic::Ordering::SeqCst);
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
        // ②-b plugins 绑定硬拒（2026-10-22 扩面，用户拍板 ①）：清单在场且带
        //    非空 plugins_sha256 表时，包内 plugins/ 子树逐文件 sha256 + 集合
        //    双向匹配，任一失配 = 包被调包/下载损坏，无条件拒（不受
        //    allow_downgrade 豁免）。空表/缺失 = 无绑定诚实降级（旧清单兼容，
        //    审计留痕）——绑定面与主包 sha256 同批生成于 CI 签名脚本。
        if let Some(m) = &manifest {
            match verify_plugins_binding_map(m, &extracted.plugins) {
                Ok(()) => {
                    if !m.plugins_sha256.is_empty() {
                        self.audit_event(
                            "manifest",
                            "allowed",
                            &format!("plugins 绑定校验通过（{} 文件）", m.plugins_sha256.len()),
                        );
                    }
                }
                Err(e) => {
                    self.audit_event("manifest", "denied", &format!("plugins binding: {e}"));
                    return Err(format!(
                        "更新包内 plugins/ 子树与签名清单绑定值不一致（{e}），已无条件拒收。"
                    ));
                }
            }
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
        // ④ 同版闸（保留：拒绝性优化闸——命中即免走 staging；待装包同时
        //    作废清空，回干净态）。force（强制更新）跳过——同版重装是强制
        //    更新的合法意志（本地包残了自救）。
        if !force && commit_matches(current_commit, &target_commit) {
            clear_updates_dir(&exe_path);
            self.audit_event("selfupdate", "denied", "already at target commit");
            return Err(format!(
                "当前已是目标版本（commit {}），待装包已清空。",
                &target_commit[..target_commit.len().min(12)]
            ));
        }
        // ⑤ 顺序降级闸：sequence 数值比较，基准 = `<exe_dir>/applied_manifest.json`
        //    （上次成功更新收口写入的已验签清单副本）。applied 缺失（存量安装
        //    首次更新）/副本验签失败/清单本侧缺失（豁免放行）= Unknown——默认
        //    拒绝（R1 语义反转），allow_downgrade 显式豁免 + 审计留痕。v4 验签
        //    对任何路径都不豁免。force 只豁免 Same 臂（见 downgrade_gate_error）。
        if let Some(m) = &manifest {
            let current_seq = load_applied_sequence(&exe_path, &anchors, now);
            let verdict = sequence_verdict(current_seq, m.sequence);
            if let Some(err) = downgrade_gate_error(verdict, allow_downgrade, force) {
                self.downgrade_denied
                    .store(true, std::sync::atomic::Ordering::SeqCst);
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
        // 审批卡已退役（用户拍板：Dashboard 确认框即主体意志表达；真正的
        // 安全闸 = v4 签名验签 + sequence 单调闸 + estop + 单飞锁）。
        let target_desc = format!("commit {}", &target_commit[..target_commit.len().min(12)]);

        // ── 步骤 4：v4 验签（Valid-only；锚 = 编译期固化，不依赖包内证书；
        //    锚/时钟已在步骤 1e 构造，此处复用同一份）──
        let outcome = nemesis_verify::verify::verify_bytes(&extracted.exe_bytes, &anchors, now);
        if !matches!(outcome, nemesis_verify::verify::VerifyOutcome::Valid { .. }) {
            let reason = format!("v4 验签未过（{}），包已拒收", outcome_kind(&outcome));
            self.audit_event("verify", "denied", &reason);
            return Err(reason);
        }
        self.audit_event("verify", "allowed", &target_desc);

        // ── 步骤 5：staging（journal 先行 → plugins 舞步 → exe rename dance；
        //    exe_path 已在 1e 取）──
        let (old_path, new_path) = staging_paths(&exe_path);
        let (plugins_dir, plugins_backup, plugins_new) = plugins_staging_paths(&exe_path);
        // back/ 抽屉创建（先于任何落盘与改名；失败 = 现场零扰动的诚实报错，
        // 后续 journal/rename 都不会发生）。
        let staging_back = back_dir(&exe_path);
        std::fs::create_dir_all(&staging_back).map_err(|e| {
            format!(
                "staging 抽屉创建失败（{}）：{e}（更新中止，未动任何既有文件）",
                staging_back.display()
            )
        })?;
        // 残留清场（同名 PID 复用的罕见遗留会让 rename 撞已存在目标；PID 后缀
        // 名下通常本来就不存在）。
        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(&new_path);
        // plugins staging 同名 PID 残留清场（整目录韧性删除；清扫面只认
        // `plugins.old-{pid}`/`plugins.new-{pid}` 前缀，裸 `plugins` 真实插件
        // 目录永不被碰）。
        if plugins_backup.exists() {
            crate::handlers::release_fetch::remove_dir_all_resilient(&plugins_backup);
        }
        if plugins_new.exists() {
            crate::handlers::release_fetch::remove_dir_all_resilient(&plugins_new);
        }
        // .new 先落盘再 fsync（sync_all 之后才轮到 journal 与 rename dance）：
        // 崩溃窗口不再留下「新二进制内容空/半截」的现场；磁盘满等写错误也在
        // staging 之前暴露。unix 带上包内 exe 权限位（tar 条目 mode；无位/
        // 无执行位回落 0755——Linux 无执行位新进程拉不起来）。
        let mut new_file = std::fs::File::create(&new_path)
            .map_err(|e| format!("新二进制创建失败（{}）：{e}", new_path.display()))?;
        std::io::Write::write_all(&mut new_file, &extracted.exe_bytes)
            .map_err(|e| format!("新二进制落盘失败（{}）：{e}", new_path.display()))?;
        new_file
            .sync_all()
            .map_err(|e| format!("新二进制落盘同步失败（{}）：{e}", new_path.display()))?;
        drop(new_file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = extracted
                .exe_unix_mode
                .filter(|m| m & 0o111 != 0)
                .unwrap_or(0o755);
            std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(mode))
                .map_err(|e| format!("新二进制权限位设置失败（{}）：{e}", new_path.display()))?;
        }
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
            // plugins 舞步双目录（同进同退的回滚现场；包不带插件 = 双空 = boot
            // 回滚 noop 分支）。
            plugins_backup_dir: if extracted.plugins.is_empty() {
                String::new()
            } else {
                plugins_backup.display().to_string()
            },
            plugins_new_dir: if extracted.plugins.is_empty() {
                String::new()
            } else {
                plugins_new.display().to_string()
            },
            // 逐文件清单（N1）：此刻在任何 rename 之前——「包内条目 × 现状」
            // 的准确快照随 journal 先行落盘，回滚按它收口（不再从 backup
            // 名单差集反推）。
            plugins_files: compute_plugins_manifest(&plugins_dir, &extracted.plugins),
        };
        save_journal(&self.workspace, &journal)?;

        // plugins 舞步先于 exe 舞步：失败时 exe 未触碰——内联回滚 plugins，
        // 归零成功才清 journal 拒绝（回滚失败则 journal 留现场，boot 收口重试）。
        if !extracted.plugins.is_empty() {
            if let Err(e) = install_plugins_files(
                &plugins_dir,
                &plugins_backup,
                &plugins_new,
                &extracted.plugins,
            ) {
                match rollback_plugins_from_journal(&journal, &exe_path) {
                    Ok(detail) => {
                        if !clear_journal(&self.workspace) {
                            self.audit_event("staging", "denied", "journal 清理失败，残留仍在");
                        }
                        let reason = format!("plugins 替换失败：{e}（已回滚：{detail}）");
                        self.audit_event("staging", "denied", &reason);
                        return Err(reason);
                    }
                    Err(fail) => {
                        let reason = format!(
                            "plugins 替换失败：{e}；且回滚失败：{fail}\
                             （journal 已留现场，重启本进程即自动回滚）"
                        );
                        self.audit_event("staging", "denied", &reason);
                        return Err(reason);
                    }
                }
            }
            self.audit_event(
                "staging",
                "allowed",
                &format!(
                    "plugins 整包替换完成（{} 个文件，只同名覆盖不删既有）",
                    extracted.plugins.len()
                ),
            );
        }

        // exe dance：exe→.old，.new→exe（rename 运行中 PE 合法；窗口毫秒级；
        // POSIX rename 天然原子覆盖）。失败回手 = exe + plugins 双回滚
        // （plugins 可能已换新，只归位 exe 会留下「旧 exe + 新插件」错配）。
        std::fs::rename(&exe_path, &old_path)
            .map_err(|e| format!("exe→.old rename 失败：{e}（staging 中断，journal 已留现场）"))?;
        if let Err(e) = std::fs::rename(&new_path, &exe_path) {
            // 双回滚走与 boot 收口同一回滚本体（幂等诚实化）：两者皆成功才清
            // journal；任一失败保留 staging 现场，下次启动 boot 收口重试
            // （届时已成功的一侧自动 noop 收敛）。
            match rollback_update(&journal, &exe_path) {
                Ok((detail, _noop)) => {
                    if !clear_journal(&self.workspace) {
                        self.audit_event("staging", "denied", "journal 清理失败，残留仍在");
                    }
                    let reason = format!(".new→exe rename 失败：{e}（已双回滚：{detail}）");
                    self.audit_event("staging", "denied", &reason);
                    return Err(reason);
                }
                Err(fail) => {
                    let reason = format!(
                        ".new→exe rename 失败：{e}；且回手双回滚失败：{fail}\
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
        // 更新一次性完成 → 删包收口（用户拍板：更新完删除；journal/安装收口
        // 照旧由新进程 boot 承担）。
        clear_updates_dir(&exe_path);

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
            "自更新 staging 完成（{current_version} → {target_desc}），新进程已拉起，\
             本进程将优雅退出；启动自检与回滚由新进程 boot 收口执行。"
        ))
    }

    /// 桩平台变体（macOS 留位 / Android APK 形态）：诚实拒绝。apply 全链路
    /// 主体不参与桩平台编译（避免 unreachable 段）；check/status/boot 收口
    /// 全平台可用。macOS CI 产物（nightly-macos.tar.gz）就位后，把
    /// `apply_supported()` 与本 cfg 门一起放开即接通。
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    async fn apply_inner(
        &self,
        current_version: &str,
        current_commit: &str,
        allow_downgrade: bool,
        force: bool,
    ) -> Result<String, String> {
        let _ = (current_version, current_commit, allow_downgrade, force);
        self.audit_event("selfupdate", "denied", "stub platform boundary");
        Err(if TARGET_ASSET.is_empty() {
            "当前平台（Android）走 APK 发行形态，不走主包自更新通道：\
             请通过 APK 侧更新。"
                .to_string()
        } else {
            "自更新 apply 暂不支持当前平台（macOS 留位未接通）：\
             请手动下载 Release 包替换。"
                .to_string()
        })
    }

    /// apply 受理入口（全平台）：失败反馈面单一收口包装。Err → 记
    /// [`ApplyErrorRecord`]（status 轮询面，断线/刷新兜底）+ SSE error 帧
    /// （在线即时）；Ok → 清旧错。发起时先行清空旗标与旧记录——编排进行中
    /// status 不残留上一轮失败（前端粘滞态以此判「本轮尚未收口」）。
    pub async fn apply(
        &self,
        current_version: &str,
        current_commit: &str,
        allow_downgrade: bool,
        force: bool,
    ) -> Result<String, String> {
        self.downgrade_denied
            .store(false, std::sync::atomic::Ordering::SeqCst);
        *self.last_apply_error.lock() = None;
        let result = self
            .apply_inner(current_version, current_commit, allow_downgrade, force)
            .await;
        if let Err(e) = &result {
            *self.last_apply_error.lock() = Some(ApplyErrorRecord {
                message: e.clone(),
                downgrade_denied: self
                    .downgrade_denied
                    .load(std::sync::atomic::Ordering::SeqCst),
                at_ms: now_ms(),
            });
            publish_progress("error", 0, None, Some(e));
            tracing::warn!("[SelfUpdate] apply 编排失败（已反馈 status/SSE）：{e}");
        }
        result
    }
}

/// VerifyOutcome 无 Display——人读短文按变体出（skins classify 同口径）。
/// 唯一非测试消费方 = windows/linux apply 主体；桩平台生产编译诚实允许死码
/// （跨平台测试仍消费，cfg 掉会连九态钉测试一起裁出）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
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
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
#[derive(Debug)]
struct ExtractedBundle {
    exe_bytes: Vec<u8>,
    commit: Option<String>,
    /// R1：包内签名更新清单的原始载体字节（JSON + CMS + v4 footer）。缺失 =
    /// None（旧 CI 包 / 用户自打包）——apply 侧按 Unknown 类处理。
    manifest_bytes: Option<Vec<u8>>,
    /// unix 包内 exe 权限位（tar 条目 mode；zip 臂恒 None——zip 无通用执行位
    /// 语义，unix 下 .new 落盘走 0755 回落）。读点在 apply 的 `#[cfg(unix)]`
    /// chmod 臂——windows 构建诚实死字段。
    #[cfg_attr(not(unix), allow(dead_code))]
    exe_unix_mode: Option<u32>,
    /// plugins/ 子树文件（平面名 → 字节；子目录嵌套条目诚实拒收）。zip/tar
    /// 双臂共同捕获；空 = 包不带插件（journal 双字段空，回滚 noop 分支）。
    plugins: std::collections::BTreeMap<String, Vec<u8>>,
}

#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
impl ExtractedBundle {
    fn version_txt_commit_desc(&self) -> String {
        self.commit.clone().unwrap_or_else(|| "unknown".to_string())
    }
}

/// 更新包解包：按魔数分派（不按平台 cfg——测试跨平台可测双格式；macOS
/// 未来 tar.gz 直接复用同一臂）。`PK\x03\x04` = zip（Windows 主包形态）、
/// `\x1f\x8b` = tar+gzip（Linux/macOS 主包形态）；其余诚实拒收。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn extract_update_bundle(pkg_bytes: &[u8]) -> Result<ExtractedBundle, String> {
    if pkg_bytes.starts_with(b"PK\x03\x04") {
        extract_zip_bundle(pkg_bytes)
    } else if pkg_bytes.starts_with(b"\x1f\x8b") {
        extract_tar_gz_bundle(pkg_bytes)
    } else {
        let m0 = pkg_bytes.first().copied().unwrap_or(0);
        let m1 = pkg_bytes.get(1).copied().unwrap_or(0);
        let m2 = pkg_bytes.get(2).copied().unwrap_or(0);
        let m3 = pkg_bytes.get(3).copied().unwrap_or(0);
        Err(format!(
            "更新包格式不可识别（魔数 {m0:02x} {m1:02x} {m2:02x} {m3:02x}，\
             支持 zip / tar.gz）：已拒收"
        ))
    }
}

/// 包内条目名归一：`\` → `/`、循环剥 `./` 前缀；绝对路径与 `..` 段诚实拒收
/// （解包产物只进内存映射不落盘，但名字要进 `install_plugins_files` 的
/// join，路径逃逸在入口收口，零容忍）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn normalize_entry_name(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("包内存在空条目名：已拒收".to_string());
    }
    let mut name = raw.replace('\\', "/");
    while let Some(stripped) = name.strip_prefix("./") {
        name = stripped.to_string();
    }
    if name.starts_with('/') {
        return Err(format!("包内存在绝对路径条目（{raw}）：已拒收"));
    }
    if name.split('/').any(|seg| seg == "..") {
        return Err(format!("包内存在 `..` 逃逸条目（{raw}）：已拒收"));
    }
    Ok(name)
}

/// 包内条目分类（平面名匹配；`plugins/` 前缀 = 插件文件，要求单层平面名）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
enum BundleEntry {
    Exe,
    VersionTxt,
    Manifest,
    Plugin(String),
    Ignored,
}

#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn classify_entry(name: &str) -> Result<BundleEntry, String> {
    // 主程序条目：windows 主包 = nemesisbot.exe；linux 主包（macOS 留位同型
    // ）= 裸名 nemesisbot（daily-release「解压后运行 ./nemesisbot gateway」
    // 形态）。两个名字都认——解包分派按魔数不按平台，双形态免费共存。
    if name == "nemesisbot.exe"
        || name == "nemesisbot"
        || name.ends_with("/nemesisbot.exe")
        || name.ends_with("/nemesisbot")
    {
        return Ok(BundleEntry::Exe);
    }
    if name == "version.txt" || name.ends_with("/version.txt") {
        return Ok(BundleEntry::VersionTxt);
    }
    if name == "manifest.json" || name.ends_with("/manifest.json") {
        return Ok(BundleEntry::Manifest);
    }
    if let Some(rest) = name.strip_prefix("plugins/") {
        if rest.is_empty() {
            return Ok(BundleEntry::Ignored); // `plugins/` 目录条目本身
        }
        if rest.contains('/') {
            return Err(format!(
                "包内 plugins 子目录嵌套条目（{name}）不支持（插件平面名清单）：已拒收"
            ));
        }
        return Ok(BundleEntry::Plugin(rest.to_string()));
    }
    Ok(BundleEntry::Ignored)
}

/// 有界解压上限（BUG 2026-10-29 横扫角度 B：更新包条目解压无界入内存，
/// 且旧 zip 臂按条目头撒谎 size 预分配）。exe 实测 ~93 MB，留 2.5x 余量；
/// version.txt 是一行文本，1 MB 已是荒谬余量。超限 = 解压炸弹/损坏包，
/// 诚实拒收（在撑爆内存之前）。
#[cfg(feature = "selfupdate")]
const SELFUPDATE_ENTRY_CAP: usize = 256 * 1024 * 1024;
#[cfg(feature = "selfupdate")]
const SELFUPDATE_TEXT_ENTRY_CAP: usize = 1024 * 1024;

/// 解包产物收口（exe 必在；commit 从 version.txt 解析）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn finish_bundle(
    exe_bytes: Option<Vec<u8>>,
    version_txt: Option<String>,
    manifest_bytes: Option<Vec<u8>>,
    exe_unix_mode: Option<u32>,
    plugins: std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<ExtractedBundle, String> {
    let exe_bytes = exe_bytes
        .ok_or_else(|| "更新包内未找到主程序 nemesisbot(.exe)（包结构异常），已拒收".to_string())?;
    let commit = version_txt
        .as_deref()
        .and_then(extract_commit_from_version_txt);
    Ok(ExtractedBundle {
        exe_bytes,
        commit,
        manifest_bytes,
        exe_unix_mode,
        plugins,
    })
}

/// zip 臂（Windows 主包）：扁平结构 nemesisbot.exe / plugins/ / version.txt /
/// manifest.json（见 daily-release Package (Windows) 段）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn extract_zip_bundle(zip_bytes: &[u8]) -> Result<ExtractedBundle, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("更新包不是合法 zip：{e}"))?;
    let mut exe_bytes: Option<Vec<u8>> = None;
    let mut version_txt: Option<String> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut plugins = std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("更新包条目 {i} 不可读：{e}"))?;
        // zip 9：条目名解码可失败——诚实报错拒绝（更新包条目名必须可判读）。
        let raw = entry
            .name()
            .map_err(|e| format!("更新包条目 {i} 名字不可解码：{e}"))?
            .to_string();
        let name = normalize_entry_name(&raw)?;
        match classify_entry(&name)? {
            BundleEntry::Exe => {
                let buf =
                    nemesis_utils::zip_util::read_entry_capped(&mut entry, SELFUPDATE_ENTRY_CAP)
                        .map_err(|e| format!("读取包内 nemesisbot.exe 失败：{e}"))?;
                exe_bytes = Some(buf);
            }
            BundleEntry::VersionTxt => {
                let buf = nemesis_utils::zip_util::read_entry_capped(
                    &mut entry,
                    SELFUPDATE_TEXT_ENTRY_CAP,
                )
                .map_err(|e| format!("读取包内 version.txt 失败：{e}"))?;
                version_txt = Some(
                    String::from_utf8(buf)
                        .map_err(|_| "读取包内 version.txt 失败：非 UTF-8".to_string())?,
                );
            }
            BundleEntry::Manifest => {
                let buf =
                    nemesis_utils::zip_util::read_entry_capped(&mut entry, SELFUPDATE_ENTRY_CAP)
                        .map_err(|e| format!("读取包内 manifest.json 失败：{e}"))?;
                manifest_bytes = Some(buf);
            }
            BundleEntry::Plugin(pname) => {
                let buf =
                    nemesis_utils::zip_util::read_entry_capped(&mut entry, SELFUPDATE_ENTRY_CAP)
                        .map_err(|e| format!("读取包内插件 {pname} 失败：{e}"))?;
                plugins.insert(pname, buf);
            }
            BundleEntry::Ignored => {}
        }
    }
    finish_bundle(exe_bytes, version_txt, manifest_bytes, None, plugins)
}

/// tar.gz 臂（Linux/macOS 主包）：CI `tar -czf - -C staging .` 产出 `./`
/// 前缀条目——normalize 剥前缀。Directory 条目跳过、Regular 处理、其他
/// （symlink/hardlink）诚实拒收（解包不跟随链接——链接逃逸是 tar 投毒
/// 常态面）；exe 权限位随条目 mode 进 [`ExtractedBundle::exe_unix_mode`]。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
fn extract_tar_gz_bundle(pkg_bytes: &[u8]) -> Result<ExtractedBundle, String> {
    let gz = flate2::read::GzDecoder::new(pkg_bytes);
    let mut archive = tar::Archive::new(gz);
    let mut exe_bytes: Option<Vec<u8>> = None;
    let mut version_txt: Option<String> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut exe_unix_mode: Option<u32> = None;
    let mut plugins = std::collections::BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|e| format!("更新包不是合法 tar.gz：{e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("更新包条目不可读：{e}"))?;
        let raw = entry
            .path()
            .map_err(|e| format!("更新包条目名不可解码：{e}"))?
            .to_string_lossy()
            .to_string();
        let name = normalize_entry_name(&raw)?;
        use tar::EntryType;
        match entry.header().entry_type() {
            EntryType::Directory => continue,
            EntryType::Regular => {}
            other => {
                return Err(format!(
                    "包内存在非常规条目（{raw}，类型 {other:?}；symlink/hardlink 不收）：已拒收"
                ));
            }
        }
        let mode = entry.header().mode().ok();
        match classify_entry(&name)? {
            BundleEntry::Exe => {
                let buf =
                    nemesis_utils::zip_util::read_entry_capped(&mut entry, SELFUPDATE_ENTRY_CAP)
                        .map_err(|e| format!("读取包内 nemesisbot.exe 失败：{e}"))?;
                exe_bytes = Some(buf);
                exe_unix_mode = mode;
            }
            BundleEntry::VersionTxt => {
                let buf = nemesis_utils::zip_util::read_entry_capped(
                    &mut entry,
                    SELFUPDATE_TEXT_ENTRY_CAP,
                )
                .map_err(|e| format!("读取包内 version.txt 失败：{e}"))?;
                version_txt = Some(
                    String::from_utf8(buf)
                        .map_err(|_| "读取包内 version.txt 失败：非 UTF-8".to_string())?,
                );
            }
            BundleEntry::Manifest => {
                let buf =
                    nemesis_utils::zip_util::read_entry_capped(&mut entry, SELFUPDATE_ENTRY_CAP)
                        .map_err(|e| format!("读取包内 manifest.json 失败：{e}"))?;
                manifest_bytes = Some(buf);
            }
            BundleEntry::Plugin(pname) => {
                let buf =
                    nemesis_utils::zip_util::read_entry_capped(&mut entry, SELFUPDATE_ENTRY_CAP)
                        .map_err(|e| format!("读取包内插件 {pname} 失败：{e}"))?;
                plugins.insert(pname, buf);
            }
            BundleEntry::Ignored => {}
        }
    }
    finish_bundle(
        exe_bytes,
        version_txt,
        manifest_bytes,
        exe_unix_mode,
        plugins,
    )
}

/// 拉起新版本（detached；等旧进程的接力棒经 env 注入）。
#[cfg(feature = "selfupdate")]
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))] // 消费方=windows/linux apply；macOS/android 桩平台诚实死码
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
    #[cfg(unix)]
    {
        // 独立进程组：外部按进程树杀旧进程时（kill -pgid 同类语义）不牵连
        // 新进程——与 Windows 臂 CREATE_NEW_PROCESS_GROUP 同语义；安全 API
        // 非 unsafe。
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd.spawn()
        .map(|c| c.id())
        .map_err(|e| format!("新版本进程拉起失败（{}）：{e}", new_exe.display()))
}

#[cfg(test)]
mod tests;
