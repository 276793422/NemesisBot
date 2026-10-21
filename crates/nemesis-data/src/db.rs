//! SQLite schema initialization and migration.

use rusqlite::Connection;
use std::path::Path;

const SCHEMA_VERSION: i32 = 3;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS request_logs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id        TEXT    NOT NULL,
    model           TEXT    NOT NULL,
    provider_type   TEXT    NOT NULL DEFAULT '',
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
    total_cost_usd  REAL    NOT NULL DEFAULT 0.0,
    latency_ms      INTEGER NOT NULL DEFAULT 0,
    status_code     INTEGER NOT NULL DEFAULT 200,
    error_message   TEXT,
    is_streaming     INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_request_logs_created_at
    ON request_logs(created_at);

CREATE INDEX IF NOT EXISTS idx_request_logs_model
    ON request_logs(model);

CREATE TABLE IF NOT EXISTS daily_rollups (
    date                    TEXT    NOT NULL,
    model                   TEXT    NOT NULL,
    request_count           INTEGER NOT NULL DEFAULT 0,
    success_count           INTEGER NOT NULL DEFAULT 0,
    input_tokens            INTEGER NOT NULL DEFAULT 0,
    output_tokens           INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens       INTEGER NOT NULL DEFAULT 0,
    total_cost_usd          REAL    NOT NULL DEFAULT 0.0,
    avg_latency_ms          REAL    NOT NULL DEFAULT 0.0,
    PRIMARY KEY (date, model)
);

CREATE TABLE IF NOT EXISTS model_pricing (
    model_id                        TEXT PRIMARY KEY,
    display_name                    TEXT    NOT NULL DEFAULT '',
    input_cost_per_million          REAL    NOT NULL DEFAULT 0.0,
    output_cost_per_million         REAL    NOT NULL DEFAULT 0.0,
    cache_read_cost_per_million     REAL    NOT NULL DEFAULT 0.0,
    cache_creation_cost_per_million REAL    NOT NULL DEFAULT 0.0
);
"#;

/// v1 → v2（A3 请求明细增强，2026-08-31）：明细行补计价与排查字段。
///
/// 与源规格（`proxy_request_logs` 表）的偏差：**不单设
/// `duration_ms`**——既有 `latency_ms` 就是该请求的真实耗时真相源
/// （写入点语义即"一轮 LLM 调用耗时"），两列同义徒增分歧面。
/// `first_token_ms` 留 NULL：`LLMProvider` trait 目前只有一元 `chat`
/// （无流式通路），TTFT 无从测量——列先落位，流式通路落地后填充。
const SCHEMA_V2: &str = r#"
ALTER TABLE request_logs ADD COLUMN pricing_model TEXT NOT NULL DEFAULT '';
ALTER TABLE request_logs ADD COLUMN input_cost_usd REAL NOT NULL DEFAULT 0.0;
ALTER TABLE request_logs ADD COLUMN output_cost_usd REAL NOT NULL DEFAULT 0.0;
ALTER TABLE request_logs ADD COLUMN cache_creation_cost_usd REAL NOT NULL DEFAULT 0.0;
ALTER TABLE request_logs ADD COLUMN cache_read_cost_usd REAL NOT NULL DEFAULT 0.0;
ALTER TABLE request_logs ADD COLUMN first_token_ms INTEGER;
ALTER TABLE request_logs ADD COLUMN session_key TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_request_logs_session
    ON request_logs(session_key);

CREATE INDEX IF NOT EXISTS idx_request_logs_status
    ON request_logs(status_code);
"#;

/// v2 → v3（P2B 模型工具健康，2026-09-12）：按 天×模型 聚合的工具调用 /
/// 参数校验失败计数。NB-15 根修配套——worker 端 validation_budget 失败
/// （A 端 fail_class 只能看到「失败了」）在此沉淀为可观测的长期趋势，
/// `models.health`（WSAPI/Dashboard）据此给出 tier 校准建议。
///
/// 与 request_logs 的关系：request_logs 一行 = 一次 LLM 请求（模型维度
/// 已有）；本表一行 = 天×模型 的工具调用累计（工具维度此前无账）。
/// upsert 增量写（每次工具调用 ~µs，相对秒级工具耗时忽略不计）。
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS tool_validation_stats (
    day                 TEXT    NOT NULL,
    model               TEXT    NOT NULL,
    tool_calls          INTEGER NOT NULL DEFAULT 0,
    validation_failures INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, model)
);

-- A2（2026-10-21 引入）：loop-guard 事件计数（⑥/④ 失败
-- nudge、⑤/⑤′ 重复成功 nudge、⑦ 退化、⑧ 行文重复、T1 无收据合成失败）。
-- 与 tool_validation_stats 同口径（天×模型 upsert 增量），turn 收尾写一行。
-- `models.health` 按「原地打转」口径消费（tier 校准建议）。
CREATE TABLE IF NOT EXISTS guard_event_stats (
    day      TEXT    NOT NULL,
    model    TEXT    NOT NULL,
    events   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, model)
);
"#;

/// Open (or create) the database at `db_path` and run pending migrations.
pub fn init_db(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create data directory: {e}"))?;
    }

    // B10 手法① 能力准入：Connection::open 对任意可读文件惰性成功——
    // 被覆盖/截断的垃圾文件要到第一条语句才炸且报错面模糊。头 16 字节
    // 前置判定，非 SQLite 内容 loud 拒绝并带 hex 摘要。
    match nemesis_utils::sqlite_preflight::header_kind(db_path)
        .map_err(|e| format!("Failed to inspect database file: {e}"))?
    {
        nemesis_utils::sqlite_preflight::HeaderKind::New
        | nemesis_utils::sqlite_preflight::HeaderKind::Valid => {}
        nemesis_utils::sqlite_preflight::HeaderKind::Foreign { head_hex } => {
            return Err(nemesis_utils::sqlite_preflight::foreign_header_error(
                db_path, &head_hex,
            ));
        }
    }

    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open database: {e}"))?;

    // B10 手法④ WAL 降级：journal_mode 失败（网络盘/FAT/只读）warn 不崩，
    // 回落默认 journal；foreign_keys 是语义闸，保持硬失败。
    nemesis_utils::sqlite_preflight::set_wal_lenient(&conn, "[DataStore]");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("Failed to set pragmas: {e}"))?;

    let current_version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);

    // B10 手法② schema 漂移对账：库来自更新版本的程序 → 拒绝降级打开
    // （旧行为会把 user_version 静默降级写回，迁移链跳过、运行期
    // "no such column"）。
    nemesis_utils::sqlite_preflight::check_schema_drift(current_version, SCHEMA_VERSION)?;

    // B10 手法③ 迁移前轮换备份（0 = 全新库无数据可保护，跳过；备份
    // 失败 warn 不阻断迁移）。
    if current_version > 0 && current_version < SCHEMA_VERSION {
        nemesis_utils::sqlite_preflight::backup_before_migrate(&conn, db_path, "[DataStore]");
    }

    // 版本化迁移链：每步只从上一版前进，旧库逐级升级。
    if current_version < 1 {
        conn.execute_batch(SCHEMA_V1)
            .map_err(|e| format!("Schema v1 init failed: {e}"))?;
        tracing::info!("[DataStore] Schema v1 applied");
    }
    if current_version < 2 {
        conn.execute_batch(SCHEMA_V2)
            .map_err(|e| format!("Schema v2 migration failed: {e}"))?;
        tracing::info!("[DataStore] Schema v2 applied (A3: pricing/session/timing columns)");
    }
    if current_version < 3 {
        conn.execute_batch(SCHEMA_V3)
            .map_err(|e| format!("Schema v3 migration failed: {e}"))?;
        tracing::info!("[DataStore] Schema v3 applied (P2B: tool_validation_stats)");
    }
    set_version(&conn, SCHEMA_VERSION)?;

    Ok(conn)
}

fn set_version(conn: &Connection, version: i32) -> Result<(), String> {
    conn.pragma_update(None, "user_version", version)
        .map_err(|e| format!("Failed to set schema version: {e}"))
}

#[cfg(test)]
mod tests;
