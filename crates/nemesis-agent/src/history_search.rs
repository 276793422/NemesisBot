//! Session full-text search.
//!
//! SQLite FTS5 index over the chat-log JSONL files (`session_logs/*.jsonl`),
//! queryable by the `history_search` agent tool and the
//! `nemesisbot history search` CLI. Answers "which conversation said X" with
//! a session key + snippet + timestamp, across sessions (the goal's
//! acceptance criterion).
//!
//! Design (goal §二):
//! - Index DB: `<workspace>/logs/history_index.db`, one FTS5 table
//!   `(session_key, seq, role, timestamp, content, content_bigram)` where
//!   `content_bigram` carries the CJK-2gram expansion of the content.
//!   unicode61 (the bundled FTS5 default tokenizer) treats a run of CJK
//!   chars as ONE token, so a raw Chinese query like「部署」would never match
//!   「部署文档」— the bigram column (「部署 署文 文档」) makes short Chinese
//!   queries hit. Latin text rides along unchanged (bigrams only generated
//!   for CJK runs).
//! - Update: lazy two-stage — [`reindex_session_logs`] full-scans the JSONL
//!   dir on first query (mtime-tracked, so unchanged files are skipped on
//!   later calls), and [`index_append`] inserts single rows as messages are
//!   appended. No WAL/real-time machinery.
//! - Boundary events: those live in the sidecar (`logs/boundary/`), NOT in
//!   session_logs — nothing to exclude here.
//! - Failure semantics: any DB error degrades the tool to a grep-style
//!   linear scan of the JSONL dir (same answers, slower) — search never
//!   blocks the agent flow (goal §八).

use nemesis_path::default_path_manager;
use rusqlite::Connection;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Mutex;

/// One search hit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryHit {
    pub session_key: String,
    /// 0-based line index of the matched message in its session jsonl.
    pub seq: usize,
    pub role: String,
    pub timestamp: String,
    /// Snippet around the first match (already-trimmed tool results included).
    pub snippet: String,
}

/// BUSY/LOCKED 失败后的重试退避窗：conn 缺席期间带 busy_timeout(5s) 的
/// open 尝试全程持 INDEX 锁（`chat_log` 的 append guard 也持它）——每条
/// 消息一撞锁会把秒级等待放大成所有写路径的尾延迟。窗口内诚实让位
/// （零 I/O），过窗再试（复查 2026-11-26）。非 BUSY 失败不退避：那些
/// 重试是零成本快失败，且恢复语义的回归测试依赖「每次调用都真重试」。
const OPEN_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

struct IndexState {
    conn: Option<Connection>,
    /// (session file stem, mtime) of indexed files — skip unchanged on reindex.
    indexed: std::collections::HashMap<String, std::time::SystemTime>,
    /// 连续 open 失败计数（0 = 打开成功或从未失败）。BUG-1（2026-11-25 取证）：
    /// 原实现 open 失败后 `conn: None` 落进静态缓存永久生效——一次 hot journal
    /// 残留/磁盘故障 = FTS 索引永久静默死亡，重试/告警/自愈全无。现在每次
    /// `conn` 缺席都经 [`ensure_conn`] 重试打开（含删库重建恢复）。
    open_failures: u32,
    /// 上次 open 失败 WARN 时刻（限频：首次立即，此后每 10 次或间隔 5 分钟再报）。
    last_warn: Option<std::time::Instant>,
    /// BUSY/LOCKED 失败后的让位截止时刻（None = 不退避）。竞争瞬态专属：
    /// 只有 BUSY 形态的 open 尝试带 5s 锁内等待成本；损坏/CANTOPEN 形态
    /// 是零成本快失败，照旧每次调用真重试。
    skip_open_until: Option<std::time::Instant>,
}

static INDEX: Mutex<Option<IndexState>> = Mutex::new(None);

/// Path of the FTS index DB.
pub fn index_db_path() -> PathBuf {
    default_path_manager()
        .sessions_log_dir()
        .parent()
        .map(|p| p.join("history_index.db"))
        .unwrap_or_else(|| PathBuf::from("history_index.db"))
}

/// 打开索引库（open + busy_timeout + pragma + 建表）。Err 保留 rusqlite
/// 原始错误供调用方分类：BUSY/LOCKED = 并发竞争瞬态（不进恢复阶梯——
/// 复查 2026-11-26 #1：此前所有失败形态一律进删除阶梯，把「对端持续
/// 持锁」误当「库损坏」；POSIX 上 unlink 活库不会失败，删了会孤儿化
/// 对端句柄并毁掉其热回放），其余 = 库体/存储故障（进恢复阶梯）。
fn open_conn_raw() -> Result<Connection, rusqlite::Error> {
    let path = index_db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path)?;
    // BUG-20261115-029：CLI（`nemesisbot history search` / `history reindex`）
    // 与常驻 gateway **并开同一索引库**是常规拓扑，而 rollback-journal
    // （TRUNCATE）模式下读写互斥——无退避的撞锁 = 立即 SQLITE_BUSY：
    // gateway reindex 半途失败仍登记 mtime → 该文件索引尾部缺口持久到
    // mtime 再变；增量插行被吞 → 最近一条消息检索不到。busy_timeout 让
    // 短锁竞争退避重试而不是立刻报错（与 board.db / taskstore / 两个
    // watcher 连接的 5000ms 同值）；本库是可重建缓存，等待上限不伤语义。
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
    // Write performance: this is a REBUILDABLE cache (reindex regenerates it
    // from the jsonl source of truth), so durability pragmas can be relaxed.
    // Default journal (delete-mode, fsync per txn) made each append-hook
    // insert cost a disk sync — visible as multi-second test runs / agent
    // append latency on busy disks.
    // journal_mode 是首个 touch 页面的语句（库体损坏以 NotADatabase 在此
    // 浮出）也是首个取写锁的语句（竞争以 BUSY 在此浮出）。BUSY 立即上报
    // 分类（不再叠一层 5s 等待）；其余失败容忍退化（只读 fs 等牺牲
    // durability，可重建缓存不坚持——与原 `let _ =` 吞错口径一致）。
    if let Err(e) = conn.pragma_update(None, "journal_mode", "TRUNCATE")
        && open_err_is_busy(&e)
    {
        return Err(e);
    }
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    // External-content-less plain FTS5 table. `content_bigram` doubles CJK
    // runs as bigrams (see module doc). tokenize='unicode61' is the default.
    // 建表失败 = 库不可用实锤（表缺席则一切查询/写入全废）→ Err 进分类。
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(\
             session_key UNINDEXED, seq UNINDEXED, role UNINDEXED, \
             timestamp UNINDEXED, content, content_bigram, \
             tokenize='unicode61');\
         ",
    )?;
    Ok(conn)
}

/// open 失败是否为并发竞争瞬态（BUSY/LOCKED）：busy_timeout 窗口耗尽
/// 仍拿不到锁。这类失败 = 文件是活的、有使用者在读写——恢复删除的
/// 「删得掉 ≠ 没人用」论证只在 Windows 成立，POSIX 上属误删活库。
fn open_err_is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ffi::ErrorCode::DatabaseBusy
                || f.code == rusqlite::ffi::ErrorCode::DatabaseLocked
    )
}

/// SQLite 数据库头嗅探：头 16 字节 == `SQLite format 3\0`。读不到 16 字节
/// 或读失败 = 非法头（空文件按 sqlite 规范是合法空库，但既然 open 已以
/// 非 BUSY 真错误失败，重建无害）。
fn looks_like_sqlite_db(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 16];
    f.read_exact(&mut buf).is_ok() && &buf == b"SQLite format 3\x00"
}

/// 非 BUSY open 失败后的恢复阶梯。返回 (conn, recovered)，
/// `recovered=true` 表示经历了删除重建/侧车清理。
///
/// 恢复语义（BUG-1，2026-11-25 取证；复查 2026-11-26 #1 重构）：本库是
/// REBUILDABLE cache——数据源头是 `session_logs/*.jsonl`，库本身可随时全量
/// 重建。BUSY/LOCKED（并发竞争）已在调用方先行分流、不进本阶梯。进来
/// 的都是「open 以真错误失败」：
///
/// 1. 主库头嗅探合法（`SQLite format 3\0`）= 库体不是垃圾——先**只清侧车**
///    （journal/wal/shm）重试。正常 hot journal 回放是 sqlite 内建自愈
///    （open 即回放，不能粗暴删 journal，会丢事务）；走到这里已是回放
///    失败（残留/损坏/存储瞬态），删残留 journal 是唯一自愈手段，且不丢
///    已提交事务（已提交事务的 journal 早已被 sqlite 自删）。11-25 实案
///    形态（journal 残留 + 磁盘故障）走这条臂。
/// 2. 头嗅探非法（垃圾字节/空壳）= 库体损坏实锤 → 全量重建。
/// 3. 头合法但侧车清理后仍打不开 = 页级损坏（头在体坏）→ 也走全量重建。
///    Windows 上若被占用，删除会失败 → 诚实 Err（不硬删）；POSIX 上走到
///    这里已排除 BUSY（无活锁信号），对端即便持句柄其连接也在同报错误
///    ——孤儿化不毁它任何未完成事务。
///
/// 删除顺序（复查 2026-11-26）：侧车先行、主库最后——若主库先删而某个
/// 侧车删除失败（被句柄持有），本轮放弃后下次 open 会建出**新主库**，旧
/// journal 残留在旁可能被 sqlite 当 hot journal 回放进新库（污染）；侧车
/// 先行则任何中途失败都不动主库，旧状态完整保留。
fn recover_and_open(open_err: &rusqlite::Error) -> Result<(Connection, bool), String> {
    let path = index_db_path();
    if looks_like_sqlite_db(&path) {
        let mut sidecars = 0usize;
        for suffix in ["-journal", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{}", path.display(), suffix));
            match std::fs::remove_file(&p) {
                Ok(()) => sidecars += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "索引库侧车 {} 删除失败（可能有并发使用者，本轮放弃）: {e}",
                        p.display()
                    ));
                }
            }
        }
        if let Ok(c) = open_conn_raw() {
            if sidecars > 0 {
                tracing::warn!(
                    "[history_search] FTS 索引库经侧车清理恢复（移除 {sidecars} 个残留 journal/wal/shm）"
                );
            }
            return Ok((c, true));
        }
        // 侧车清理后仍打不开 = 主库页级损坏（头在体坏）→ 落全量重建。
    }
    let mut removed = 0usize;
    for suffix in ["-journal", "-wal", "-shm", ""] {
        let p = if suffix.is_empty() {
            path.clone()
        } else {
            PathBuf::from(format!("{}{}", path.display(), suffix))
        };
        match std::fs::remove_file(&p) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "索引库打开失败（{open_err}）且恢复删除 {} 失败: {e}",
                    p.display()
                ));
            }
        }
    }
    match open_conn_raw() {
        Ok(c) => Ok((c, true)),
        Err(_) => Err(format!(
            "索引库打开失败（{open_err}），删库重建（移除 {removed} 个文件）后仍打不开"
        )),
    }
}

/// 确保 `st.conn` 在场（BUG-1 修复核心）：缺席时重试打开（含损坏恢复），
/// 失败不固化——下次调用再试。失败 WARN 限频（首次立即，此后每 10 次或
/// 间隔 5 分钟），恢复时也 WARN（可观测「曾经降级」）。返回 false = 本轮
/// 仍不可用（调用方诚实让位；下轮重试）。
///
/// 失败分流（复查 2026-11-26）：BUSY/LOCKED = 并发竞争瞬态——不进恢复
/// 阶梯（不碰文件；POSIX 上删除活库不会失败，删了会孤儿化对端句柄），
/// 并置 [`OPEN_RETRY_BACKOFF`] 让位窗（conn 缺席期间每次调用都付 5s
/// busy_timeout 锁内等待，会把秒级成本放大成每消息一次；窗口内零 I/O）。
/// 其余失败照旧每次真重试（零成本快失败，恢复测试依赖该语义）。
fn ensure_conn(st: &mut IndexState) -> bool {
    if st.conn.is_some() {
        return true;
    }
    if st
        .skip_open_until
        .is_some_and(|t| std::time::Instant::now() < t)
    {
        return false;
    }
    st.skip_open_until = None;
    match open_conn_raw() {
        Ok(conn) => {
            adopt_conn(st, conn, false);
            true
        }
        Err(e) if open_err_is_busy(&e) => {
            record_open_failure(
                st,
                "索引库被另一使用者持续占用（busy_timeout 5s 耗尽）：可能并存 \
                 gateway/CLI 正在读写；本轮让位不碰文件，稍后自动重试",
            );
            st.skip_open_until = Some(std::time::Instant::now() + OPEN_RETRY_BACKOFF);
            false
        }
        Err(open_err) => match recover_and_open(&open_err) {
            Ok((conn, recovered)) => {
                adopt_conn(st, conn, recovered);
                true
            }
            Err(e) => {
                record_open_failure(st, &e);
                false
            }
        },
    }
}

/// 恢复/重开成功的统一收口：WARN（可观测「曾经降级」）+ 清空 mtime 登记
/// （失败期间错过的增量插入/文件变化靠全量重扫补齐，幂等 DELETE-first
/// 重建）+ 失败记账复位。
fn adopt_conn(st: &mut IndexState, conn: Connection, recovered: bool) {
    if st.open_failures > 0 {
        let how = if recovered {
            "删库重建"
        } else {
            "重新打开"
        };
        tracing::warn!(
            "[history_search] FTS 索引库恢复（{how}），此前连续失败 {} 次；已清空 mtime 登记，下次 reindex 全量重扫补齐缺口",
            st.open_failures
        );
    }
    st.indexed.clear();
    st.open_failures = 0;
    st.last_warn = None;
    st.skip_open_until = None;
    st.conn = Some(conn);
}

/// open 失败记账 + WARN 限频（首次立即，此后每 10 次或间隔 5 分钟）。
fn record_open_failure(st: &mut IndexState, e: &str) {
    st.open_failures += 1;
    let should_warn = st.open_failures == 1
        || st.open_failures.is_multiple_of(10)
        || st
            .last_warn
            .is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(300));
    if should_warn {
        tracing::warn!(
            "[history_search] FTS 索引库不可用（连续失败 {} 次）：{e}；搜索降级为线性扫描，索引将在库恢复后自动重建",
            st.open_failures
        );
        st.last_warn = Some(std::time::Instant::now());
    }
}

/// Expand CJK runs into overlapping bigrams for the `content_bigram` column.
/// Latin/digit/punct words pass through whole; CJK runs become bigram
/// sequences. Single-char CJK runs emit the char itself (a 1-gram can't
/// form a bigram and must stay findable).
pub fn cjk_bigrams(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut latin: Vec<char> = Vec::new(); // current non-CJK word run
    let mut cjk: Vec<char> = Vec::new(); // current CJK run
    let flush_cjk = |run: &mut Vec<char>, out: &mut String| {
        if run.is_empty() {
            return;
        }
        if run.len() == 1 {
            out.push(run[0]);
        } else {
            for w in run.windows(2) {
                out.push(w[0]);
                out.push(w[1]);
                out.push(' ');
            }
            // windows loop leaves a trailing space per pair; trim handled by
            // the final String builder — push trailing space for single too.
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        }
        run.clear();
    };
    let flush_latin = |run: &mut Vec<char>, out: &mut String| {
        if !run.is_empty() {
            out.extend(run.iter());
            out.push(' ');
            run.clear();
        }
    };
    for ch in text.chars() {
        let is_cjk = matches!(ch as u32,
            0x3400..=0x4DBF   |  // CJK ext A
            0x4E00..=0x9FFF   |  // CJK unified
            0xF900..=0xFAFF   |  // CJK compat
            0x20000..=0x2A6DF); // CJK ext B
        if is_cjk {
            flush_latin(&mut latin, &mut out);
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut out);
            latin.push(ch);
        } else {
            // separator: end both runs
            flush_latin(&mut latin, &mut out);
            flush_cjk(&mut cjk, &mut out);
        }
    }
    flush_latin(&mut latin, &mut out);
    flush_cjk(&mut cjk, &mut out);
    out.trim_end().to_string()
}

fn with_conn<R>(
    f: impl FnOnce(&mut Connection, &mut std::collections::HashMap<String, std::time::SystemTime>) -> R,
) -> Option<R> {
    let mut guard = INDEX.lock().ok()?;
    let st = guard.get_or_insert_with(|| IndexState {
        conn: None,
        indexed: std::collections::HashMap::new(),
        open_failures: 0,
        last_warn: None,
        skip_open_until: None,
    });
    if !ensure_conn(st) {
        return None;
    }
    let conn = st.conn.as_mut()?;
    Some(f(conn, &mut st.indexed))
}

/// 增量插入的互斥句柄：`chat_log` 的 append 持它跨「文件写入 + 索引插行」，
/// 让 reindex / 查询（同锁互斥）永远看不到「已写盘、未插行」的中间态。
/// 若不持锁，reindex 的 mtime 采样在锁外、全量重读在锁内——append 落进
/// 该缝隙时 reindex 已把新行按文件行号索引进去，随后的增量插入仍按
/// MAX(seq)+1 再插一行同内容（seq 指向不存在的行号）；且 racing reindex
/// 已登记新 mtime，后续 reindex 恒跳过该文件，幻影重复行跨查询存活到
/// 该会话下一次 append 才自愈。锁毒化时 [`append_guard`] 返回 None，调用方
/// 回落无锁的 [`index_append`]（best-effort，下次全量 reindex 自愈）。
pub struct AppendGuard(std::sync::MutexGuard<'static, Option<IndexState>>);

/// 取增量插入互斥句柄。None = 锁毒化（进程内历史 panic），调用方回落
/// [`index_append`]。
pub fn append_guard() -> Option<AppendGuard> {
    INDEX.lock().ok().map(AppendGuard)
}

impl AppendGuard {
    /// 持锁形态的单行增量插入（语义与 [`index_append`] 相同；宿主持有的
    /// 互斥已覆盖本调用，锁内不可再走 [`with_conn`]——std Mutex 不可重入）。
    /// conn 缺席（上次打开失败）时经 [`ensure_conn`] 重试恢复，不再静默吞。
    pub fn index_append(&mut self, session_key: &str, role: &str, content: &str, timestamp: &str) {
        let stem = nemesis_utils::sanitize::sanitize_path_segment(session_key);
        let Some(st) = self.0.as_mut() else {
            return;
        };
        if !ensure_conn(st) {
            return;
        }
        let Some(conn) = st.conn.as_mut() else {
            return;
        };
        append_index_row(conn, &mut st.indexed, &stem, role, content, timestamp);
    }
}

/// Full/lazy (re)index of `session_logs/*.jsonl`. mtime-incremental: files
/// whose mtime matches the recorded value are skipped. Returns the number of
/// rows indexed this call (0 = everything fresh).
pub fn reindex_session_logs() -> usize {
    let dir = default_path_manager().sessions_log_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut rows = 0usize;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Skip our own index db / meta files by extension filter above.
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        seen.insert(stem.to_string());
        let mut did_index = false;
        let mut skipped_fresh = false;
        with_conn(|conn, indexed| {
            if indexed.get(stem) == Some(&mtime) {
                skipped_fresh = true;
                return;
            }
            // Re-indexing a changed file: clear its rows first.
            // BUG-20261115-029 横向加固：清旧行失败 = 库写锁被外部持有
            // （另一进程并开同库且锁未被 busy_timeout 窗口消化）。此时
            // 继续 INSERT 会在未清空的旧行上叠加重复行（DELETE-first
            // 契约破坏）——诚实让位：本文件本轮不索引、不登记 mtime，
            // 消费方下一次查询的 reindex 在锁释放后整文件重建。
            if conn
                .execute(
                    "DELETE FROM history_fts WHERE session_key = ?1",
                    rusqlite::params![stem],
                )
                .is_err()
            {
                return;
            }
            if let Ok(file) = std::fs::File::open(&path) {
                for (seq, line) in std::io::BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .enumerate()
                {
                    // BUG-20261115-029 横向加固：插行失败（BUSY 残余/
                    // 磁盘满等瞬态）不登记 mtime——登记会让后续 reindex
                    // 恒跳过本文件，半索引尾部缺口持久到 mtime 再变；
                    // 不登记则下轮 DELETE-first 整文件重建自愈（幂等）。
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                        && insert_row(conn, stem, seq, &v).is_err()
                    {
                        return;
                    }
                }
            }
            indexed.insert(stem.to_string(), mtime);
            did_index = true;
        });
        if !skipped_fresh && did_index {
            rows += 1;
        }
    }

    // Orphan purge: rows whose source file no longer exists must
    // go — pre-fix, deleted sessions' rows lingered forever and eventually
    // filled the search window with ghosts. The comparison is DB-vs-disk
    // directly: gating on the in-memory `indexed` map (`known == seen`) made
    // the purge unreachable exactly when it was needed — a fresh process's
    // full scan registers every disk file into `indexed`, so the map always
    // equals the disk set afterwards, and files deleted while no process was
    // watching (leaked test files, sessions deleted in a prior run) kept
    // their rows. One SELECT DISTINCT per reindex is cheap (stems only).
    with_conn(|conn, indexed| {
        let indexed_keys: Vec<String> = conn
            .prepare("SELECT DISTINCT session_key FROM history_fts")
            .map(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))
                    .map(|ks| ks.flatten().collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        for k in indexed_keys {
            if !seen.contains(&k) {
                let _ = conn.execute(
                    "DELETE FROM history_fts WHERE session_key = ?1",
                    rusqlite::params![k],
                );
            }
        }
        indexed.retain(|k, _| seen.contains(k));
    });
    rows
}

fn insert_row(
    conn: &Connection,
    session_key: &str,
    seq: usize,
    v: &serde_json::Value,
) -> rusqlite::Result<usize> {
    let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let timestamp = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
    conn.execute(
        "INSERT INTO history_fts (session_key, seq, role, timestamp, content, content_bigram)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            session_key,
            seq as i64,
            role,
            timestamp,
            content,
            cjk_bigrams(content)
        ],
    )
}

/// Incremental hook for `append_chat_log_full` — index one freshly appended
/// message. Best-effort: DB failures are swallowed (warn) — the next full
/// reindex picks the row up. Track seq by file line count only when the
/// index already knows the file; otherwise skip (full reindex handles it).
pub fn index_append(session_key: &str, role: &str, content: &str, timestamp: &str) {
    let stem = stem_of(session_key);
    let _ =
        with_conn(|conn, indexed| append_index_row(conn, indexed, &stem, role, content, timestamp));
}

/// SAN-01/SAN-05：与 chat_log 落盘同源的白名单消毒（stem 即文件名）。
fn stem_of(session_key: &str) -> String {
    nemesis_utils::sanitize::sanitize_path_segment(session_key)
}

/// 单行增量插入的共享实现（`index_append` 与 `AppendGuard::index_append`
/// 同源）。`indexed` 不含本会话 = 从未被全量索引（或整体重写/删除后登记
/// 已被 [`invalidate_session`] 摘除），诚实让位——下一次全量 reindex 会
/// 连本行一起索引。
fn append_index_row(
    conn: &mut Connection,
    indexed: &mut std::collections::HashMap<String, std::time::SystemTime>,
    stem: &str,
    role: &str,
    content: &str,
    timestamp: &str,
) {
    if !indexed.contains_key(stem) {
        return;
    }
    let seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM history_fts WHERE session_key = ?1",
            rusqlite::params![stem],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let v = serde_json::json!({"role": role, "content": content, "timestamp": timestamp});
    if let Err(e) = insert_row(conn, stem, seq as usize, &v) {
        tracing::warn!("[history_search] index_append failed: {e}");
    }
}

/// 会话日志被整体重写或删除（截断回退、清空、删除）后摘除其索引登记：
/// 增量插入按 MAX(seq)+1 推 seq，重写后 DB 里的旧行 seq 与新文件行号已
/// 失配——继续增量插会把新行插到与文件行号错位的 seq 上，且重写掉的旧
/// 内容仍留在检索结果里。摘除登记后增量插入按「从未全量索引」短路，
/// 落盘态由下一次全量 reindex 重建（消费方查询前恒先 reindex，自愈即时）。
pub fn invalidate_session(session_key: &str) {
    let stem = stem_of(session_key);
    let _ = with_conn(|_, indexed| {
        indexed.remove(&stem);
    });
}

/// [`invalidate_session`] 的 stem 直输形态（logs_sweeper TTL 清扫用，
/// 2026-11-19 删除残留修复）：sweeper 只有文件 stem（=
/// `sanitize_path_segment(key)` 的产物，DB session_key 列存的就是 stem——
/// insert_row 以 stem 入库），无原始 key 可走 `stem_of` 反推。幂等；
/// DB 打不开（`with_conn` 失败）静默跳过——下次查询前 reindex 的孤儿
/// 清理兜底（DB-vs-disk 对比同族）。
pub fn invalidate_session_stem(stem: &str) {
    // DB 缺席短路：索引从未建立（fresh home / 无 gateway 活动的测试进程）
    // = 无行可删，免开 DB（open_conn 会创建空 DB 文件 = 无谓落盘）。
    // 语义等价：无 DB 必无行。
    if !index_db_path().exists() {
        return;
    }
    let _ = with_conn(|conn, indexed| {
        let _ = conn.execute(
            "DELETE FROM history_fts WHERE session_key = ?1",
            rusqlite::params![stem],
        );
        indexed.remove(stem);
    });
}

/// Build the FTS5 match expression: the raw query as a phrase against
/// `content`, OR every bigram token of the query against `content_bigram`
/// (any-hit semantics — a phrase match on the bigram column would require
/// the source text to contain the query as a contiguous bigram subsequence,
/// which is stricter than "the words appear"). Tokens are quoted to avoid
/// FTS5 syntax injection.
fn match_expr(query: &str) -> String {
    let q = query.replace('"', "\"\"");
    let mut parts = vec![format!("content : \"{q}\"")];
    let bigram_str = cjk_bigrams(query);
    let bigrams: Vec<&str> = bigram_str
        .split_whitespace()
        // latin words already covered by the phrase arm; keep only tokens
        // containing CJK chars (the bigrams proper).
        .filter(|t| t.chars().any(|c| (c as u32) >= 0x3400))
        .collect();
    if !bigrams.is_empty() {
        let ors: Vec<String> = bigrams
            .iter()
            .map(|t| format!("content_bigram : \"{}\"", t))
            .collect();
        parts.push(ors.join(" OR "));
    }
    parts.join(" OR ")
}

/// Snippet around the first occurrence of any query word in the content.
fn snippet_around(content: &str, query: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(32);
    let lower = content.to_lowercase();
    let ql = query.trim().to_lowercase();
    let lower_pos = ql
        .split_whitespace()
        .find_map(|w| lower.find(w).filter(|_| !w.is_empty()))
        .unwrap_or(0);
    // to_lowercase 对个别字符变长（İ U+0130 2B→3B、ẞ U+1E9E 3B→2B），
    // lower 里的字节偏移不能直接当 content 偏移用——否则命中点之后的
    // snippet 窗口错位（切掉命中词或显示错位上下文）。先映射回原文偏移。
    let pos = lower_offset_to_source(content, lower_pos);
    // Char-boundary-safe window (multibyte panic discipline).
    let start = pos.saturating_sub(max_chars / 3);
    let start = floor_char_boundary(content, start);
    let end = (start + max_chars).min(content.len());
    let end = ceil_char_boundary(content, end);
    let mut s: String = content[start..end].to_string();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < content.len() {
        s.push('…');
    }
    s
}

/// `content.to_lowercase()` 域中的字节偏移 → 原文 content 字节偏移。
/// 对齐到命中处所在源字符的起点；pos 超出 lower 总长时回落 content.len()。
fn lower_offset_to_source(content: &str, pos: usize) -> usize {
    let mut lower_off = 0usize;
    for (i, c) in content.char_indices() {
        if lower_off >= pos {
            return i;
        }
        // ASCII 字符折叠恒 1 字节（快路径零分配）；非 ASCII 才实际展开。
        let span = if c.is_ascii() {
            1
        } else {
            c.to_lowercase().collect::<String>().len()
        };
        if lower_off + span > pos {
            return i;
        }
        lower_off += span;
    }
    content.len()
}

fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Query across sessions. FTS path first; on any DB failure degrade to a
/// linear grep-style scan of the JSONL dir (goal §八 fallback).
pub fn search(query: &str, limit: usize) -> Vec<HistoryHit> {
    let limit = limit.clamp(1, 100);
    if query.trim().is_empty() {
        return Vec::new();
    }
    if let Some(hits) = search_fts(query, limit) {
        return hits;
    }
    search_linear(query, limit)
}

fn search_fts(query: &str, limit: usize) -> Option<Vec<HistoryHit>> {
    with_conn(|conn, _| {
        let expr = match_expr(query);
        let mut stmt = conn
            .prepare(
                "SELECT session_key, seq, role, timestamp, content \
                 FROM history_fts WHERE history_fts MATCH ?1 \
                 ORDER BY rank LIMIT ?2",
            )
            .ok()?;
        let rows = stmt
            .query_map(rusqlite::params![expr, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .ok()?;
        let mut hits = Vec::new();
        for row in rows.flatten() {
            hits.push(HistoryHit {
                session_key: row.0,
                seq: row.1 as usize,
                role: row.2,
                timestamp: row.3,
                snippet: snippet_around(&row.4, query, 160),
            });
        }
        Some(hits)
    })
    .flatten()
}

/// Degraded path: scan `session_logs/*.jsonl` directly (no index). Same
/// answers, slower — the DB failing must not break the tool.
fn search_linear(query: &str, limit: usize) -> Vec<HistoryHit> {
    let dir = default_path_manager().sessions_log_dir();
    let mut hits = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return hits;
    };
    let ql = query.trim().to_lowercase();
    if ql.is_empty() {
        return hits;
    }
    // 确定性迭代序：limit 截断让返回的命中子集依赖遍历序，
    // 而 read_dir 顺序是平台实现细节（NTFS 字典序 vs ext4 哈希序）。先按
    // mtime 降序 + 文件名升序 tiebreak 排序（最新会话优先），同一数据
    // 跨平台同一结果。
    let files = jsonl_files_latest_first(rd);
    for path in &files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        for (seq, line) in std::io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .enumerate()
        {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if content.to_lowercase().contains(&ql) {
                hits.push(HistoryHit {
                    session_key: stem.clone(),
                    seq,
                    role: v
                        .get("role")
                        .and_then(|r| r.as_str())
                        .unwrap_or("")
                        .to_string(),
                    timestamp: v
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string(),
                    snippet: snippet_around(content, query, 160),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

/// session_logs 目录的 JSONL 清单，最新 mtime 优先（文件名升序 tiebreak）。
/// 只收常规文件（`*.jsonl` 目录不进清单）；非常规 mtime 的条目跳过
/// （后续 File::open 也会失败，语义一致）。
fn jsonl_files_latest_first(rd: std::fs::ReadDir) -> Vec<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .filter_map(|e| {
            let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((mtime, e.path()))
        })
        .collect();
    order_session_files_latest_first(&mut files);
    files.into_iter().map(|(_, p)| p).collect()
}

/// 排序纯函数（mtime 降序 + 路径升序 tiebreak），独立出来便于单测钉死。
fn order_session_files_latest_first(files: &mut [(std::time::SystemTime, PathBuf)]) {
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
}

/// Render hits for the LLM (the `history_search` tool result body).
pub fn render_hits(hits: &[HistoryHit]) -> String {
    if hits.is_empty() {
        return "没有找到匹配的历史消息。".to_string();
    }
    let mut out = format!("找到 {} 条匹配：\n", hits.len());
    for h in hits {
        out.push_str(&format!(
            "- [{}] session={} seq={} time={}\n  {}\n",
            h.role, h.session_key, h.seq, h.timestamp, h.snippet
        ));
    }
    out.push_str("\n可用 read_chat_log 式会话定位（session_key）进一步查看上下文。");
    out
}

#[cfg(test)]
mod tests;
