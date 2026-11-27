//! File utilities.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// [`write_file_atomic`] 临时文件名前缀。临时名形如
/// `.tmp-{pid}-{seq}-{nanos}`，与目标同目录、无扩展名——消费方的目录
/// 清扫（TTL sweep 等）若需清除**崩溃残留**的临时孤儿（错误路径会自清理，
/// 但进程死亡留在 create↔rename 窗口时无人善后），按本前缀扫描。
pub const ATOMIC_TMP_PREFIX: &str = ".tmp-";

/// 临时孤儿清扫的保守宽限：在写临时文件的生命周期是毫秒~秒级（小 JSON
/// 的 write+fsync+rename），1 小时是 3~4 个数量级的安全余量——清扫方只删
/// mtime 早于 now-grace 的孤儿，绝不误删在写临时文件；刚崩出来的新鲜孤儿
/// 留到下一轮 sweep（清扫节奏通常为启动/每日，无正确性影响）。
pub const ATOMIC_TMP_ORPHAN_GRACE: std::time::Duration = std::time::Duration::from_secs(3600);

/// 生成原子写的临时文件路径（`<parent>/.tmp-{pid}-{seq}-{nanos}`）。
/// 进程内唯一性不能只靠时间戳：Windows SystemTime 刷新粒度粗（毫秒级），
/// 同 tick 内多线程并发出拿相同 nanos → create_new 撞 ERROR_FILE_EXISTS
/// （Windows CI 实证）。叠加进程级单调序数保证同进程内绝对唯一；跨进程
/// 唯一性仍由 pid 承担（低频配置写场景，见 [`write_file_atomic`] 诚实边界）。
/// 独立成函数：命名规则单一真相（前缀常量 + 形态），测试与消费方可直接
/// 断言，无需在写窗口内竞速观测。
#[doc(hidden)]
pub fn atomic_tmp_path(parent: &Path) -> std::path::PathBuf {
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    parent.join(format!(
        "{}{}-{}-{}",
        ATOMIC_TMP_PREFIX,
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

/// 撕裂尾安全读（追加式 jsonl/日志的读侧防线）：`fs::read` +
/// `String::from_utf8_lossy`。
///
/// 为什么不用 [`std::fs::read_to_string`]：追加写（append + write_all，无
/// fsync 的日志惯例）在断电/崩溃下可留下撕裂尾行——尾行劈中多字节 UTF-8
/// 字符（中文内容常态）时整个文件非 UTF-8，`read_to_string` 直接
/// `Err(InvalidData)`，**坏一行毒化整个文件**：读方拿到空/None，前缀完好
/// 行全部陪葬。本 helper 把损坏降级为坏字节位置的 U+FFFD 替换符——下游
/// 逐行 JSON parse 对坏尾行失败即跳过（既有坏行纪律），完好前缀全部保全。
/// 单对象 JSON 文件（meta/trash.json 等）不适用此防线（撕裂尾=对象本身
/// 已坏，逐行语义无意义）——那些文件保持 `read_to_string` + 解析失败
/// 留痕的诚实处理。
pub fn read_lossy_to_string(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 追加式 jsonl/日志的**写侧重开防线**（与 [`read_lossy_to_string`] 读侧
/// 防线配对）：上一 writer 会话写失败（write 中途 ENOSPC/IO 错误）或进程
/// 崩溃可留下**无换行尾巴**——不修复时恢复后第一条记录直接拼接其后成同
/// 一行，逐行 JSON 解析两条全丢。本 helper 在（重）打开 append 句柄后探
/// 测末字节，缺换行补一个 `\n`，保证后续每条记录都是完整行。
///
/// 调用方契约：句柄须以 `read(true) + append(true)` 并申请打开——append
/// 句柄在 Windows 上无读权限，末字节探测会静默失败（先例：nemesis-logger
/// resilient_writer BUG-2「半行不粘连」）。返回是否补了换行；探测/修复自
/// 身故障 fail-open（返回 false 放弃，只影响撞窗口那一条，不阻塞记录主
/// 流程——与这些记录面的 fail-open/尽力而为纪律同构）。
pub fn repair_torn_tail(f: &mut std::fs::File) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(end) = f.seek(SeekFrom::End(0)) else {
        return false;
    };
    if end == 0 {
        return false;
    }
    if f.seek(SeekFrom::End(-1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    if f.read_exact(&mut last).is_ok() && last[0] != b'\n' {
        f.write_all(b"\n").is_ok()
    } else {
        false
    }
}

/// Atomically write data to a file: unique same-dir temp file → write →
/// flush → `sync_all` → rename over target（unix 天然覆盖 / Windows
/// `MOVEFILE_REPLACE_EXISTING`——rename 两平台都执行，见下方 bullet）
/// → (unix) parent-dir sync.
///
/// 关键配置/状态写入的唯一权威 helper——所有配置修改
/// 入口都调这里，不在各模块重复自制 tmp+rename。
///
/// - **唯一临时名** `.tmp-{pid}-{seq}-{nanos}`（seq=进程级单调序数）：
///   并发写同一目标不互踩——时间戳单打独斗在 Windows 粗时钟粒度下会同 tick
///   重复，序数补上进程内绝对唯一。
/// - **`sync_all` 先于 rename**：断电/崩溃后目标要么是完整新内容要么是旧
///   文件，绝不出现半截 JSON/TOML。
/// - **unix 权限在临时文件创建时即挂**（`mode(perm)`）：不存在先宽后收的
///   窗口；Windows 为 no-op（ACL 不处理，诚实边界）。
/// - **失败清理**：任一步失败删除临时文件、原文件不动，错误带路径上下文。
/// - **symlink 保链**：目标已是 symlink → 解析真实目标写入（follow 语义），
///   symlink 本身不被 rename 替换（BUG-20261117-074；与 async 包装同语义）。
/// - Windows `fs::rename` 即 `MOVEFILE_REPLACE_EXISTING`（覆盖已存在目标）；
///   目标被外部占用（AV 扫描/打开句柄）时 rename 失败属预期，由调用方决定
///   重试或上报。
/// - **不做跨进程锁**：配置写入低频，同配置多进程同写场景当前不存在
///   （诚实边界；pid+序数临时名已消除进程内并发互踩）。
pub fn write_file_atomic(path: &str, data: &[u8], perm: u32) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = perm; // Windows：POSIX 权限不存在，ACL 不处理（诚实边界）
    let raw = Path::new(path);
    // symlink 保链（BUG-20261117-074）：目标已存在（含 symlink）→ 解析真实
    // 目标，rename 落在真实文件上，不把 symlink 本身换成普通文件；目标不
    // 存在（含父目录未建）→ 原路径走 create 语义。与 async 包装
    // （`write_file_atomic_async`，BUG-20261116-010 工具面加固）同语义——
    // 此前只有 async 面保链，sync 直调面（config.json / MEMORY.md /
    // 会话 sidecar 等用户文件，dotfiles symlink 模式的常见对象）会把
    // symlink 条目整体替换成普通文件，用户 dotfile 静默脱管。临时文件落
    // 真实目标同目录 ⇒ rename 恒同目录原子（symlink 指向跨设备时也成立）。
    // 规范化走 `canonicalize_write_target`（BUG-20261122-040）：整路径
    // canonicalize 在 Windows 按句柄解析，并发写者替换目标的瞬间会产出
    // `$Extend\$Deleted` 伪路径——并发替换安全形态由该 helper 单点保证。
    let p = canonicalize_write_target(raw);
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("atomic write {}: mkdir: {e}", p.display()))?;

    let tmp_path = atomic_tmp_path(parent);

    let mut created = false;
    let result = (|| -> Result<(), String> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(perm);
        }
        let mut tmp = opts
            .open(&tmp_path)
            .map_err(|e| format!("atomic write {}: create temp: {e}", p.display()))?;
        created = true;
        tmp.write_all(data)
            .map_err(|e| format!("atomic write {}: write temp: {e}", p.display()))?;
        tmp.flush()
            .map_err(|e| format!("atomic write {}: flush: {e}", p.display()))?;
        tmp.sync_all()
            .map_err(|e| format!("atomic write {}: sync: {e}", p.display()))?;
        drop(tmp);
        // 覆盖语义：unix rename 天然替换；Windows std rename 即
        // MOVEFILE_REPLACE_EXISTING。
        fs::rename(&tmp_path, &p)
            .map_err(|e| format!("atomic write {}: rename: {e}", p.display()))?;
        // rename 本身的持久化：尽力 sync 父目录（失败不影响结果正确性）。
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    if result.is_err() && created {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// 目标路径规范化——并发替换安全形态（BUG-20261122-040，sync/async 共用
/// 单一真相源）：
/// - 最后一跳是 **symlink** → 整路径 canonicalize 保链（rename 落真实
///   目标，BUG-20261117-074 语义）。symlink 被并发替换的窗口极窄且解析
///   结果自洽（解析当下真实存在的链接目标），不属本 BUG 修复面。
/// - 目标是**普通文件** → 只规范化**父目录**再拼回原文件名。整路径
///   canonicalize 在 Windows 走按句柄解析（GetFinalPathNameByHandle）：
///   并发写者 `rename(tmp→target)` 替换目标的瞬间，本任务已打开的句柄
///   变成「最后链接已删」态，句柄解析返回
///   `\\?\C:\$Extend\$Deleted\<FRN>` 伪路径并被当成写入目标——mkdir
///   系统目录拒绝访问（诚实报错，实测被并发测试拦出）；若 mkdir 可过
///   则内容写到伪路径、目标静默丢更新。父目录不受并发文件替换影响，
///   窗口关闭。
/// - 目标**不存在**（含父目录未建）→ 原路径直用（create 语义；整路径
///   canonicalize 对不存在路径本就失败回落原路径，行为一致）。
fn canonicalize_write_target(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() => path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        Ok(_) => {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            match (parent.canonicalize(), path.file_name()) {
                (Ok(p), Some(name)) => p.join(name),
                _ => path.to_path_buf(),
            }
        }
        Err(_) => path.to_path_buf(),
    }
}

/// Async 版 [`write_file_atomic`]——agent 文件工具（write_file/edit_file/
/// multiedit）的写盘单一入口（BUG-20261116-010）：裸 `tokio::fs::write`
/// 非原子直写，写中段进程死亡 = 源文件撕裂（半截新内容覆盖旧文件）。
///
/// 与 sync 版的语义差异（本封装的三件事）：
/// - **symlink 保链**：目标已存在先 `canonicalize` 解析——rename 落在真实
///   目标上，不把 symlink 本身换成普通文件（与旧直写 follow 语义一致）；
///   目标不存在（含父目录未建）→ 原路径走 create 语义。
/// - **unix mode 保留**：目标已存在 → 继承其权限位（0o7777），不用调用方
///   perm 覆盖（否则每次编辑把 0600 私密文件改成 0644）；新建文件用调用方
///   perm（sync 版 `mode()` 精确生效、不过 umask——工具面传 0o644 惯例）。
/// - **阻塞 IO 下放 spawn_blocking**：不占 async 工作线程；JoinError
///   （运行期关停）诚实上抛。
///
/// 诚实边界（共通 + 本封装特有）：
/// - **硬链接集合分裂**：rename 替换单个目录项，同 inode 的其他硬链接保留
///   旧内容（直写是全集合更新）；当前工具面无硬链接依赖场景，如出现需
///   回到直写或先解链。
/// - **Windows 隐藏属性丢失**：rename 换入的新文件不带 Hidden 位。
/// - AV 扫描/打开句柄占用目标时 rename 失败属预期（同 sync 版），由调用方
///   决定重试或上报。
pub async fn write_file_atomic_async(path: &Path, data: &[u8], perm: u32) -> Result<(), String> {
    // 规范化（含 symlink 保链）统一交 sync 单一真相源
    // `canonicalize_write_target` 在阻塞线程内完成（BUG-20261122-040）：
    // async 侧自己的整路径 tokio::fs::canonicalize 是同一竞态窗口的另一
    // 入口（并发写者替换目标瞬间产出 $Extend\$Deleted 伪路径），且重复
    // 规范化只剩开销没有增益，已删除。
    #[cfg(unix)]
    let perm = match fs::metadata(path) {
        Ok(meta) => {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o7777
        }
        Err(_) => perm,
    };
    #[cfg(not(unix))]
    let _ = perm;

    let path_str = match path.to_path_buf().into_os_string().into_string() {
        Ok(s) => s,
        Err(os) => {
            return Err(format!(
                "atomic write {}: path is not valid UTF-8",
                Path::new(&os).display()
            ));
        }
    };
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || write_file_atomic(&path_str, &data, perm))
        .await
        .map_err(|e| format!("atomic write task join: {e}"))?
}

/// Ensure a directory exists.
pub fn ensure_dir(path: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("create dir: {}", e))
}

/// Read file to string.
pub fn read_file_string(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("read: {}", e))
}

#[cfg(test)]
mod tests;
