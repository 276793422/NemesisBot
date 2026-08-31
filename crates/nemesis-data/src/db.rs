//! SQLite schema initialization and migration.

use rusqlite::Connection;
use std::path::Path;

const SCHEMA_VERSION: i32 = 2;

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
/// 与源规格（cc-switch `proxy_request_logs`）的偏差：**不单设
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

/// Open (or create) the database at `db_path` and run pending migrations.
pub fn init_db(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create data directory: {e}"))?;
    }

    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open database: {e}"))?;

    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("Failed to set pragmas: {e}"))?;

    let current_version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);

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
    set_version(&conn, SCHEMA_VERSION)?;

    Ok(conn)
}

fn set_version(conn: &Connection, version: i32) -> Result<(), String> {
    conn.pragma_update(None, "user_version", version)
        .map_err(|e| format!("Failed to set schema version: {e}"))
}

#[cfg(test)]
mod tests;
