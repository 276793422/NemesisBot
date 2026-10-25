//! tasks.db 打开 / 完整性快检 / user_version 顺序迁移。
//!
//! 模式对齐 `nemesis-board/src/db.rs`（rusqlite 单写者权威先例）：
//! WAL + busy_timeout + user_version 迁移；迁移每步包在
//! `BEGIN IMMEDIATE` 事务里（两个连接同时首次 open 同一文件的竞态
//! 由写锁串行化，重入步幂等）。打开即 `PRAGMA quick_check`——损坏
//! fail-closed 返回 Err，调用方负责响亮降级（PoisonedTaskStore /
//! 台账缺席），不做静默自愈。

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// 当前 schema 版本。P2（agents 表）/P4（交付泛化）/项 C（通知三段）
/// 增表时：追加迁移步 + 本常量 +1，历史步永不改写。
pub(crate) const LATEST_SCHEMA_VERSION: i64 = 4;

/// v1 基线 schema：统一任务层四表。
pub(crate) const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS runs (
    id               TEXT PRIMARY KEY,
    kind             TEXT NOT NULL DEFAULT 'cluster_task',
    title            TEXT NOT NULL DEFAULT '',
    status           TEXT NOT NULL DEFAULT 'pending',
    action           TEXT NOT NULL DEFAULT '',
    peer_id          TEXT NOT NULL DEFAULT '',
    payload          TEXT NOT NULL DEFAULT '{}',
    result           TEXT,
    original_channel TEXT NOT NULL DEFAULT '',
    original_chat_id TEXT NOT NULL DEFAULT '',
    created_at       TEXT NOT NULL,
    completed_at     TEXT,
    updated_at       TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);

CREATE TABLE IF NOT EXISTS attempts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id      TEXT NOT NULL REFERENCES runs(id),
    seq         INTEGER NOT NULL,
    worker      TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'running',
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    error       TEXT,
    usage_json  TEXT,
    UNIQUE(run_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_attempts_run ON attempts(run_id);

CREATE TABLE IF NOT EXISTS approvals (
    id            TEXT PRIMARY KEY,
    broker        TEXT NOT NULL,
    short_id      TEXT,
    operation     TEXT NOT NULL,
    target        TEXT NOT NULL DEFAULT '',
    risk_level    TEXT NOT NULL DEFAULT '',
    reason        TEXT NOT NULL DEFAULT '',
    payload       TEXT NOT NULL DEFAULT '{}',
    status        TEXT NOT NULL DEFAULT 'pending',
    created_at    TEXT NOT NULL,
    decided_at    TEXT,
    decision_note TEXT,
    decided_by    TEXT
);

CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals(status);
CREATE INDEX IF NOT EXISTS idx_approvals_short ON approvals(short_id);

CREATE TABLE IF NOT EXISTS deliveries (
    id              TEXT PRIMARY KEY,
    run_id          TEXT,
    kind            TEXT NOT NULL DEFAULT 'result',
    idempotency_key TEXT NOT NULL UNIQUE,
    status          TEXT NOT NULL DEFAULT 'recorded',
    payload         TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL,
    delivered_at    TEXT,
    confirmed_at    TEXT
);
"#;

/// v2（P2）：Agent 持久实体表——持久身份（uuid）/ home 与执行根 /
/// 记忆命名空间 / 凭据引用（vault 别名）/ 工具策略 / 档位 / 运行时
/// 后端 / 多租户预留（principal）/ 归属节点。三类组（main/project/
/// selfbuilt）同表，`kind` 区分；`status` 承载两阶段创建
/// （provisioning → active | failed）。
pub(crate) const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS agents (
    agent_id          TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    kind              TEXT NOT NULL DEFAULT 'selfbuilt',
    status            TEXT NOT NULL DEFAULT 'provisioning',
    persona_refs      TEXT NOT NULL DEFAULT '{}',
    memory_namespace  TEXT NOT NULL DEFAULT '',
    exec_root         TEXT NOT NULL DEFAULT '',
    home_path         TEXT NOT NULL DEFAULT '',
    credential_refs   TEXT NOT NULL DEFAULT '[]',
    tool_policy       TEXT NOT NULL DEFAULT '{}',
    tier              TEXT NOT NULL DEFAULT '',
    runtime_backend   TEXT NOT NULL DEFAULT 'process',
    principal         TEXT NOT NULL DEFAULT '',
    home_node         TEXT NOT NULL DEFAULT '',
    project_id        TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    last_active_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);
CREATE INDEX IF NOT EXISTS idx_agents_kind ON agents(kind);
"#;

/// v3（P4 交付门）：deliveries 拒绝语义——`rejected_at`/`reject_reason`
/// 两列（与 approvals 的 decided_at/decision_note 列式先例对称）。
/// rejected 是终态：从任何非 rejected 状态条件进入（状态迁移走
/// `reject_delivery`，带原因留痕）。
pub(crate) const SCHEMA_V3: &str = r#"
ALTER TABLE deliveries ADD COLUMN rejected_at TEXT;
ALTER TABLE deliveries ADD COLUMN reject_reason TEXT;
"#;

/// v4（项 C 通知三段，2026-10-22）：deliveries 通知义务四列 + 出站失败
/// 重试箱 notify_outbox。`notify_state` 与 `status` 双维正交（status 说
/// 「交付证据到哪一步」，notify_state 说「出站通知义务走到哪一段」）：
/// pending（有义务，初始）→ channel_accepted → confirmed；none = 无义务；
/// legacy = v4 前旧账（不参与通知管线）；dead = 死信终态。
/// 旧账回填 'legacy'：迁移时不给历史交付补通知义务（它们交付时走的是
/// 旧「落库即 delivered」语义，无重试债可追）。
/// notify_outbox：通道 send 失败的出站消息暂存（message = JSON 序列化的
/// OutboundMessage），按 delivery_key UNIQUE（同笔交付同时至多一笔待重试，
/// UPSERT 覆盖 message + 累计 attempts）。
pub(crate) const SCHEMA_V4: &str = r#"
ALTER TABLE deliveries ADD COLUMN notify_state TEXT NOT NULL DEFAULT '';
ALTER TABLE deliveries ADD COLUMN notify_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE deliveries ADD COLUMN notify_last_error TEXT;
ALTER TABLE deliveries ADD COLUMN notify_next_retry_at TEXT;
UPDATE deliveries SET notify_state = 'legacy' WHERE notify_state = '';
CREATE TABLE IF NOT EXISTS notify_outbox (
    id            TEXT PRIMARY KEY,
    delivery_key  TEXT NOT NULL UNIQUE,
    message       TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    next_retry_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notify_outbox_retry ON notify_outbox(next_retry_at);
"#;

/// tasks.db 连接持有者（`Arc<TaskDb>` 共享；内部 Mutex 串行写——
/// 单写者权威与 board.db 同款，WAL 保读不阻塞）。
#[derive(Debug)]
pub struct TaskDb {
    conn: Mutex<Connection>,
}

impl TaskDb {
    /// 打开（不存在则创建）+ 完整性快检 + 迁移到最新版。
    ///
    /// 损坏 / 无法迁移 = Err（fail-closed，调用方响亮降级）。
    pub fn open(path: &Path) -> Result<Arc<TaskDb>, String> {
        let conn = Connection::open(path)
            .map_err(|e| format!("tasks.db open failed ({}): {e}", path.display()))?;
        // busy_timeout 必须最先（连接级 API，先于快检/任何语句）：并发首开
        // 时快检与其他连接的迁移写窗口重叠，无超时预算会立即 busy。
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| format!("tasks.db busy_timeout setup failed: {e}"))?;
        // 完整性快检先行：垃圾字节文件在 PRAGMA 阶段就会以 NOTADB 炸出，
        // 让错误统一走 integrity 语义面（fail-closed 消息可定性）。
        Self::check_integrity(&conn)?;
        // journal_mode 切换有界重试：实测多连接同进程并发首开时，WAL 排他
        // 切换可能立即返回 SQLITE_BUSY 且不进 busy handler 等待（首开竞态
        // 测试高概率复现「database is locked」，套件总耗时 0.08s = 5s 预算
        // 根本没生效）。对端迁移有界必然推进，退避重试即收敛。
        let mut switched = false;
        for attempt in 0..10u32 {
            match conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;") {
                Ok(()) => {
                    switched = true;
                    break;
                }
                Err(e) if attempt < 9 && e.to_string().contains("locked") => {
                    let backoff = 20u64 * u64::from(attempt + 1);
                    std::thread::sleep(std::time::Duration::from_millis(backoff));
                }
                Err(e) => return Err(format!("tasks.db pragma setup failed: {e}")),
            }
        }
        if !switched {
            return Err("tasks.db pragma setup failed: journal_mode switch did not \
                        converge under concurrent open"
                .to_string());
        }
        let mut conn = Self::migrate(conn)?;
        // BUG-20261025-48：连接默认事务行为钉 IMMEDIATE。unchecked_transaction
        // （merge_delivery_payload 的 RMW）继承此默认——DEFERRED 在 WAL 多连接
        // 下「读后快照被并发写推进 → UPDATE 升级写锁 BUSY」，docstring 承诺的
        // 「读与写之间不被插队」不成立；IMMEDIATE 在 BEGIN 时就拿写锁，RMW
        // 才真正串行化（与 migrate 的显式 Immediate 同纪律，本库单写者权威
        // 的安全默认）。
        conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        Ok(Arc::new(TaskDb {
            conn: Mutex::new(conn),
        }))
    }

    /// 在连接上执行一段访问（内部 Mutex 串行化；Mutex 中毒 = 库不可用，
    /// fail-closed 上抛不静默）。
    pub(crate) fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let guard = self
            .conn
            .lock()
            .map_err(|e| format!("tasks.db connection poisoned: {e}"))?;
        f(&guard)
    }

    fn check_integrity(conn: &Connection) -> Result<(), String> {
        let verdict: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| format!("tasks.db integrity check failed to run: {e}"))?;
        if verdict == "ok" {
            Ok(())
        } else {
            Err(format!(
                "tasks.db integrity check failed: {verdict}（损坏熔断，拒绝打开； \
                 参照 ChatSecretStore poisoned 纪律，不做静默自愈）"
            ))
        }
    }

    /// user_version 顺序迁移。当前版本高于本 crate 认识的最新版 =
    /// Err（降级打开会破坏数据，fail-closed）。
    fn migrate(mut conn: Connection) -> Result<Connection, String> {
        let current: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| format!("tasks.db read user_version failed: {e}"))?;
        if current > LATEST_SCHEMA_VERSION {
            return Err(format!(
                "tasks.db was written by a newer version (user_version={current} > \
                 {LATEST_SCHEMA_VERSION}); refusing to touch it"
            ));
        }
        if current == LATEST_SCHEMA_VERSION {
            return Ok(conn);
        }
        // BEGIN IMMEDIATE：拿写锁后再重读版本（双连接并发首开竞态下，
        // 后到者在锁上等待、进事务后发现版本已被推进，直接收尾）。
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| format!("tasks.db migration begin failed: {e}"))?;
        let from: i64 = tx
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| format!("tasks.db re-read user_version failed: {e}"))?;
        for v in (from + 1)..=LATEST_SCHEMA_VERSION {
            let sql = match v {
                1 => SCHEMA_V1,
                2 => SCHEMA_V2,
                3 => SCHEMA_V3,
                4 => SCHEMA_V4,
                _ => unreachable!("migration step missing for schema v{v}"),
            };
            tx.execute_batch(sql)
                .map_err(|e| format!("tasks.db migration to v{v} failed: {e}"))?;
        }
        tx.pragma_update(None, "user_version", LATEST_SCHEMA_VERSION)
            .map_err(|e| format!("tasks.db stamp user_version failed: {e}"))?;
        tx.commit()
            .map_err(|e| format!("tasks.db migration commit failed: {e}"))?;
        Ok(conn)
    }
}
