//! SQLite schema initialization and migration.

use rusqlite::Connection;
use std::path::Path;

const SCHEMA_VERSION: i32 = 1;

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

    if current_version < 1 {
        conn.execute_batch(SCHEMA_V1)
            .map_err(|e| format!("Schema v1 init failed: {e}"))?;
        set_version(&conn, SCHEMA_VERSION)?;
        tracing::info!(version = SCHEMA_VERSION, "[DataStore] Database initialized");
    }

    Ok(conn)
}

fn set_version(conn: &Connection, version: i32) -> Result<(), String> {
    conn.pragma_update(None, "user_version", version)
        .map_err(|e| format!("Failed to set schema version: {e}"))
}
