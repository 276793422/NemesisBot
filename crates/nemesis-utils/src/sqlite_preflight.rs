//! B10（2026-10-18 存储韧性三手法）：SQLite 打开面前置守卫——能力准入 /
//! schema 漂移对账 / 迁移前轮换备份 / WAL 降级。
//!
//! 供 nemesis-data（usage 账本）与 nemesis-board（board.db）同源消费
//! （feature `sqlite-preflight` 门控 rusqlite 依赖，未启用方零成本）。
//!
//! 动机（P0 盘点，两高危存储的手法版加固）：
//! - `Connection::open` 对任意可读文件惰性成功——被覆盖/截断的垃圾文件
//!   要到第一条语句才炸，且报错面模糊；
//! - `user_version` 高于程序支持值的库（来自更新版本的程序）此前会被
//!   `set_version` 静默降级写回，迁移链跳过、运行期 "no such column"；
//! - 迁移链的 ALTER 无跨语句事务，进程中断会留半迁移库且无备份可回；
//! - `journal_mode = WAL` 在网络盘 / FAT / 只读句柄上失败会把整个启动
//!   炸掉（journal 模式回落 delete 完全可用，不应 fatal）。

use std::path::Path;

/// SQLite 3 文件头魔数（前 16 字节，`https://www.sqlite.org/fileformat2.html`）。
pub const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\x00";

/// 头部探测结果（`header_kind`）。
#[derive(Debug, PartialEq, Eq)]
pub enum HeaderKind {
    /// 文件不存在或 0 字节——SQLite 视作全新库，放行。
    New,
    /// 16 字节魔数命中——合法 SQLite 库，放行。
    Valid,
    /// 非 SQLite 内容（被其他程序覆盖 / 截断 / 编辑器存坏）。
    /// `head_hex` = 实际读到的前 ≤16 字节 hex 摘要（诊断用，不落内容）。
    Foreign { head_hex: String },
}

/// ① 能力准入：读文件头 16 字节判形态。
///
/// IO 错误（权限/被锁等）原样上抛，由调用方决定口径；`NotFound` 与
/// 0 字节文件归 [`HeaderKind::New`]（与 SQLite「空文件 = 合法新库」语义
/// 对齐，空文件也带不出魔数）。
pub fn header_kind(db_path: &Path) -> std::io::Result<HeaderKind> {
    use std::io::Read;

    let mut f = match std::fs::File::open(db_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HeaderKind::New),
        Err(e) => return Err(e),
    };
    if f.metadata()?.len() == 0 {
        return Ok(HeaderKind::New);
    }
    let mut head = [0u8; 16];
    let mut filled = 0usize;
    while filled < 16 {
        let n = f.read(&mut head[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    if filled == 16 && head == *SQLITE_HEADER {
        return Ok(HeaderKind::Valid);
    }
    Ok(HeaderKind::Foreign {
        head_hex: head[..filled].iter().map(|b| format!("{b:02x}")).collect(),
    })
}

/// Foreign 头的 loud 报错文案（带路径 + hex 摘要，便于定性是哪类坏文件）。
pub fn foreign_header_error(db_path: &Path, head_hex: &str) -> String {
    format!(
        "数据库文件不是 SQLite 格式，拒绝打开: {}（文件头 hex: {head_hex}；\
         可能已被其他程序覆盖、截断或写坏——如是误覆盖请从备份恢复，\
         如是磁盘满写坏请先清理磁盘）",
        db_path.display()
    )
}

/// ② schema 漂移对账：库的 `user_version` 高于本程序支持值 = 来自更新
/// 版本的程序，拒绝降级打开（旧版程序继续写会把新版 schema 当旧版
/// 处理，降级写回 version + 运行期 "no such column"）。
///
/// `current <= supported` 一律放行（含 0 = 全新库），真正的迁移链由
/// 调用方既有的 `if current < N` 顺序执行。
pub fn check_schema_drift(current: i32, supported: i32) -> Result<(), String> {
    if current > supported {
        return Err(format!(
            "数据库 schema 版本 {current} 高于本程序支持的 {supported}：\
             该库来自更新版本的程序，拒绝降级打开（请升级程序后再用，\
             或从 .bak 备份还原后让本版本重建）"
        ));
    }
    Ok(())
}

/// 迁移前轮换备份名：`board.db` → `board.db.bak.1` / `board.db.bak.2`。
fn backup_path(db_path: &Path, generation: u8) -> std::path::PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push(format!(".bak.{generation}"));
    std::path::PathBuf::from(s)
}

/// ③ 迁移前轮换备份（`0 < current < SCHEMA_VERSION` 时由调用方触发；
/// 0 = 全新库无数据可保护）。
///
/// 流程：`wal_checkpoint(TRUNCATE)` 把 WAL 帧合并进主文件（best-effort，
/// 失败 warn 继续拷——纯拷主文件在 checkpoint 失败时可能少最新帧，但
/// 旧备份仍远好于无备份）→ 删 bak.2 → bak.1 改名 bak.2 → 拷当前库 →
/// bak.1（留 2 份）。所有备份步骤失败 = warn 不阻断迁移（保护措施
/// 失败不改变库本身的风险，但日志要响——评审可见）。
pub fn backup_before_migrate(conn: &rusqlite::Connection, db_path: &Path, store_label: &str) {
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        tracing::warn!(store = store_label, error = %e, "迁移前 checkpoint 失败，备份可能不含最新 WAL 帧（继续备份）");
    }
    let bak2 = backup_path(db_path, 2);
    if let Err(e) = crate::fsutil::remove_file_retry(&bak2) {
        tracing::warn!(store = store_label, error = %e, "清理旧备份 bak.2 失败（继续轮换）");
    }
    let bak1 = backup_path(db_path, 1);
    // 轮换断了就不再覆盖 bak.1（保住旧一代备份，不双丢）。
    if bak1.exists()
        && let Err(e) = std::fs::rename(&bak1, &bak2)
    {
        tracing::warn!(store = store_label, error = %e, "备份轮换 bak.1→bak.2 失败，保留旧 bak.1 不覆盖（跳过本轮备份）");
        return;
    }
    match std::fs::copy(db_path, &bak1) {
        Ok(n) => tracing::info!(
            store = store_label,
            from = %db_path.display(),
            to = %bak1.display(),
            bytes = n,
            "迁移前备份完成（保留 2 份轮换）"
        ),
        Err(e) => tracing::warn!(
            store = store_label,
            error = %e,
            "迁移前备份失败（继续迁移，本轮无备份保护）"
        ),
    }
}

/// ④ WAL 降级：`journal_mode = WAL` 失败（网络盘 / FAT / 只读句柄）
/// warn 不崩——journal 模式回落 SQLite 默认（delete），功能不受损。
///
/// 与语义性 PRAGMA（`foreign_keys` / `busy_timeout`）刻意分离：那些
/// 保持调用方的硬闸（execute_batch 失败 = Err），journal 模式是性能
/// 优化不是语义。
pub fn set_wal_lenient(conn: &rusqlite::Connection, store_label: &str) {
    if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
        tracing::warn!(
            store = store_label,
            error = %e,
            "journal_mode=WAL 不可用（文件系统/句柄限制），回落默认 journal 模式继续（功能不受损）"
        );
    }
}

#[cfg(test)]
mod tests;
