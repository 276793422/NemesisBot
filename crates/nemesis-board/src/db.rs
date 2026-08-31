//! SQLite schema init + migration（镜像 `nemesis-data/src/db.rs` 的
//! WAL + `user_version` 模式；board.db 自包含，v1 = MVP ★ 表全量）。

use rusqlite::Connection;
use std::path::Path;

const SCHEMA_VERSION: i32 = 4;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS board_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS issue (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    number              TEXT    NOT NULL UNIQUE,
    title               TEXT    NOT NULL,
    description         TEXT    NOT NULL DEFAULT '',
    status              TEXT    NOT NULL DEFAULT 'backlog',
    priority            INTEGER NOT NULL DEFAULT 1,
    assignee_type       TEXT,
    assignee_id         TEXT,
    creator_type        TEXT    NOT NULL DEFAULT 'admin',
    creator_id          TEXT    NOT NULL DEFAULT 'admin',
    parent_issue_id     INTEGER,
    project_id          INTEGER,
    due_date            INTEGER,
    position            INTEGER NOT NULL DEFAULT 0,
    acceptance_criteria TEXT,
    origin_type         TEXT,
    origin_id           TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_issue_status ON issue(status);
CREATE INDEX IF NOT EXISTS idx_issue_assignee ON issue(assignee_type, assignee_id);
CREATE INDEX IF NOT EXISTS idx_issue_project ON issue(project_id);

CREATE TABLE IF NOT EXISTS comment (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id    INTEGER NOT NULL REFERENCES issue(id),
    author_type TEXT    NOT NULL,
    author_id   TEXT    NOT NULL,
    content     TEXT    NOT NULL,
    parent_id   INTEGER,
    ctype       TEXT    NOT NULL DEFAULT 'comment',
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_comment_issue ON comment(issue_id);

CREATE TABLE IF NOT EXISTS activity_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id   INTEGER NOT NULL REFERENCES issue(id),
    actor_type TEXT    NOT NULL,
    actor_id   TEXT    NOT NULL,
    action     TEXT    NOT NULL,
    details    TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_activity_issue ON activity_log(issue_id);

CREATE TABLE IF NOT EXISTS issue_subscriber (
    issue_id        INTEGER NOT NULL REFERENCES issue(id),
    subscriber_type TEXT    NOT NULL,
    subscriber_id   TEXT    NOT NULL,
    reason          TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (issue_id, subscriber_type, subscriber_id)
);

CREATE TABLE IF NOT EXISTS project (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE,
    description TEXT    NOT NULL DEFAULT '',
    status      TEXT    NOT NULL DEFAULT 'active',
    priority    INTEGER NOT NULL DEFAULT 1,
    lead_type   TEXT,
    lead_id     TEXT,
    icon        TEXT    NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS attachment (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id     INTEGER NOT NULL REFERENCES issue(id),
    filename     TEXT    NOT NULL,
    storage_path TEXT    NOT NULL,
    size         INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attachment_issue ON attachment(issue_id);
"#;

/// v2（W2 P2 派发链路）：issue → peer_chat task 绑定表。task_id 主键
/// （peer_chat_callback 写回路由键）；state ∈ dispatched/done/failed。
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS issue_dispatch (
    task_id       TEXT PRIMARY KEY,
    issue_id      INTEGER NOT NULL REFERENCES issue(id),
    worker_id     TEXT    NOT NULL,
    state         TEXT    NOT NULL DEFAULT 'dispatched',
    dispatched_at INTEGER NOT NULL,
    completed_at  INTEGER
);

CREATE INDEX IF NOT EXISTS idx_dispatch_issue ON issue_dispatch(issue_id);
"#;

/// v3（W2 P3 收件箱）：站内通知表。收件人是多态 Actor（recipient_type +
/// recipient_id）；P3 只做 store + dashboard 收件箱（inbox.list 按 admin 收，
/// MVP 单管理员语义），经 21 通道的站外投递留 P4。
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS notification (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    recipient_type TEXT    NOT NULL,
    recipient_id   TEXT    NOT NULL,
    kind           TEXT    NOT NULL,
    title          TEXT    NOT NULL,
    content        TEXT    NOT NULL DEFAULT '',
    issue_id       INTEGER,
    read           INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notification_recipient
    ON notification(recipient_type, recipient_id, read);
"#;

/// v4（W2 P4 autopilot）：定时自动化规则表。run 历史不单独建表——触发的
/// issue 带 origin=autopilot/{id}，按 origin 反查。cron_job_id 回存 live
/// CronService 对应 job id（`board-ap:{id}` 名字约定）。
const SCHEMA_V4: &str = r#"
CREATE TABLE IF NOT EXISTS autopilot (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    cron        TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    priority    INTEGER NOT NULL DEFAULT 1,
    project_id  INTEGER,
    target      TEXT    NOT NULL DEFAULT '',
    enabled     INTEGER NOT NULL DEFAULT 1,
    cron_job_id TEXT,
    last_run_at INTEGER,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
"#;

/// Open (or create) the board database at `db_path` and run pending migrations.
pub fn init_db(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create board directory: {e}"))?;
    }

    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open board database: {e}"))?;

    // WAL + FK enforcement；busy_timeout 让 CLI 与 gateway 并开同库时
    // 写写冲突退避重试而不是立刻报 SQLITE_BUSY（board.db 支持多进程读）。
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
    )
    .map_err(|e| format!("Failed to set pragmas: {e}"))?;

    let current_version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);

    if current_version < 1 {
        conn.execute_batch(SCHEMA_V1)
            .map_err(|e| format!("Board schema v1 init failed: {e}"))?;
        tracing::info!(version = 1, "[BoardStore] Database initialized (v1)");
    }
    if current_version < 2 {
        conn.execute_batch(SCHEMA_V2)
            .map_err(|e| format!("Board schema v2 migration failed: {e}"))?;
        tracing::info!(version = 2, "[BoardStore] Database migrated to v2 (issue_dispatch)");
    }
    if current_version < 3 {
        conn.execute_batch(SCHEMA_V3)
            .map_err(|e| format!("Board schema v3 migration failed: {e}"))?;
        tracing::info!(version = 3, "[BoardStore] Database migrated to v3 (notification)");
    }
    if current_version < 4 {
        conn.execute_batch(SCHEMA_V4)
            .map_err(|e| format!("Board schema v4 migration failed: {e}"))?;
        tracing::info!(version = 4, "[BoardStore] Database migrated to v4 (autopilot)");
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
