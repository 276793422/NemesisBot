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

/// L20 门面迁移链（v1-v3 数组化；语义与原散装 if 链等价：每步原子
/// migrate_step + 成功日志 `[DataStore] {note}`，门面按序跳过已应用步）。
static DATA_MIGRATIONS: &[nemesis_utils::sqlite_preflight::Migration] = &[
    nemesis_utils::sqlite_preflight::Migration {
        version: 1,
        sql: SCHEMA_V1,
        log_note: "Schema v1 applied",
    },
    nemesis_utils::sqlite_preflight::Migration {
        version: 2,
        sql: SCHEMA_V2,
        log_note: "Schema v2 applied (A3: pricing/session/timing columns)",
    },
    nemesis_utils::sqlite_preflight::Migration {
        version: 3,
        sql: SCHEMA_V3,
        log_note: "Schema v3 applied (P2B: tool_validation_stats)",
    },
];

/// L20 单一装配点描述（`nemesis_utils::sqlite_preflight::open_sqlite_store`
/// 门面；无 extra pragmas——data.db 单写者，无多进程并开退避需求）。
static DATA_STORE: nemesis_utils::sqlite_preflight::StoreSpec =
    nemesis_utils::sqlite_preflight::StoreSpec {
        label: "[DataStore]",
        db_noun: "database",
        migrate_label: "Schema",
        schema_version: SCHEMA_VERSION,
        migrations: DATA_MIGRATIONS,
    };

/// Open (or create) the database at `db_path` and run pending migrations.
pub fn init_db(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create data directory: {e}"))?;
    }
    // 装配序（能力准入/WAL/漂移对账/迁移前备份/原子迁移链）单一真相源在
    // 共享件 sqlite_preflight::open_sqlite_store（L20 门面）。
    nemesis_utils::sqlite_preflight::open_sqlite_store(db_path, &DATA_STORE, &[])
}

#[cfg(test)]
mod tests;
