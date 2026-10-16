//! `SqliteTaskStore` —— 集群任务真相源（runs 表）。
//!
//! 语义逐条对齐 `nemesis-cluster::task_manager::InMemoryTaskStore`
//! （错误文案、completed_at 盖章规则、条件更新先到先得），使
//! `Cluster::with_workspace` 换装后行为面零漂移。`TaskStore` trait 的
//! 适配 impl 住在 nemesis-cluster（trait 所有方，orphan rule）；
//! 本 crate 只暴露固有方法供适配层与 P2/P4 复用。

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use nemesis_types::cluster::{Task, TaskStatus};

use crate::db::TaskDb;

/// TaskStatus ↔ runs.status 字符串（统一状态机口径：
/// `pending/running/succeeded/failed/cancelled`）。
pub(crate) fn status_to_str(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "succeeded",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

/// 未知状态串（外来版本写入）不猜——响亮报错，由调用方决定跳过与否。
pub(crate) fn status_from_str(s: &str) -> Result<TaskStatus, String> {
    match s {
        "pending" => Ok(TaskStatus::Pending),
        "running" => Ok(TaskStatus::Running),
        "succeeded" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        other => Err(format!("tasks.db unknown run status: {other}")),
    }
}

pub(crate) fn is_terminal(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
    )
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

const TASK_COLUMNS: &str = "id, status, action, peer_id, payload, result, original_channel, \
     original_chat_id, created_at, completed_at";

/// 集群任务 SQLite 存储体（clone 廉价，共享同一 `Arc<TaskDb>`）。
#[derive(Clone)]
pub struct SqliteTaskStore {
    db: Arc<TaskDb>,
}

impl SqliteTaskStore {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            db: TaskDb::open(path)?,
        })
    }

    pub fn from_db(db: Arc<TaskDb>) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &Arc<TaskDb> {
        &self.db
    }

    /// 新建。重复 id = Err（与 InMemory 文案一致）。
    pub fn create(&self, task: Task) -> Result<(), String> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO runs (id, kind, title, status, action, peer_id, payload, \
                 result, original_channel, original_chat_id, created_at, completed_at, \
                 updated_at)
                 VALUES (?1, 'cluster_task', '', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    task.id,
                    status_to_str(&task.status),
                    task.action,
                    task.peer_id,
                    task.payload.to_string(),
                    task.result.as_ref().map(|v| v.to_string()),
                    task.original_channel,
                    task.original_chat_id,
                    task.created_at,
                    task.completed_at,
                    now_rfc3339(),
                ],
            )
            .map_err(|e| {
                if is_unique_violation(&e) {
                    format!("task already exists: {}", task.id)
                } else {
                    format!("tasks.db insert task failed: {e}")
                }
            })?;
            Ok(())
        })
    }

    /// 读取。不存在 = Err（与 InMemory 文案一致）。
    pub fn get(&self, task_id: &str) -> Result<Task, String> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!("SELECT {TASK_COLUMNS} FROM runs WHERE id = ?1"))
                .map_err(|e| format!("tasks.db prepare get failed: {e}"))?;
            let mut rows = stmt
                .query(params![task_id])
                .map_err(|e| format!("tasks.db get failed: {e}"))?;
            match rows
                .next()
                .map_err(|e| format!("tasks.db get failed: {e}"))?
            {
                Some(r) => row_task_strict(r),
                None => Err(format!("task not found: {task_id}")),
            }
        })
    }

    /// 无条件更新状态与结果。不存在 = Err；终态盖 completed_at
    /// （本地时间，与 InMemory 同规），非终态保留既有盖章。
    pub fn update_result(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<Value>,
    ) -> Result<(), String> {
        self.db.with_conn(|conn| {
            let bind = UpdateBind::new(task_id, &status, result);
            let n = conn
                .execute(&bind.sql, bind.refs().as_slice())
                .map_err(|e| format!("tasks.db update task failed: {e}"))?;
            if n == 0 {
                return Err(format!("task not found: {task_id}"));
            }
            Ok(())
        })
    }

    /// 条件更新（期望状态集合）。原子：`UPDATE ... WHERE status IN (...)`
    /// 单条语句先到先得。状态不匹配 = `Ok(false)`；任务不存在 = Err
    /// （与 InMemory 语义一致）。
    pub fn update_result_if_status(
        &self,
        task_id: &str,
        expected: &[TaskStatus],
        status: TaskStatus,
        result: Option<Value>,
    ) -> Result<bool, String> {
        self.db.with_conn(|conn| {
            let mut bind = UpdateBind::new(task_id, &status, result);
            let placeholders = vec!["?"; expected.len()].join(",");
            let sql = bind.sql.replace(
                "WHERE id = ?1",
                &format!("WHERE id = ?1 AND status IN ({placeholders})"),
            );
            for e in expected {
                bind.push_extra(status_to_str(e));
            }
            let n = conn
                .execute(&sql, bind.refs().as_slice())
                .map_err(|e| format!("tasks.db conditional update failed: {e}"))?;
            if n > 0 {
                return Ok(true);
            }
            // 0 行 = 状态不匹配或任务不存在；区分两者（InMemory 口径）。
            let exists: Option<String> = conn
                .query_row("SELECT id FROM runs WHERE id = ?1", params![task_id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(|e| format!("tasks.db existence check failed: {e}"))?;
            if exists.is_some() {
                Ok(false)
            } else {
                Err(format!("task not found: {task_id}"))
            }
        })
    }

    /// 删除。不存在 = Err（与 InMemory 文案一致）。
    pub fn delete(&self, task_id: &str) -> Result<(), String> {
        self.db.with_conn(|conn| {
            let n = conn
                .execute("DELETE FROM runs WHERE id = ?1", params![task_id])
                .map_err(|e| format!("tasks.db delete task failed: {e}"))?;
            if n == 0 {
                return Err(format!("task not found: {task_id}"));
            }
            Ok(())
        })
    }

    /// 按状态列举（created_at 升序；坏状态行 warn 跳过，不让单行毒化全表）。
    pub fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>, String> {
        let needle = status_to_str(&status).to_string();
        self.db.with_conn(|conn| {
            list_rows(
                conn,
                &format!(
                    "SELECT {TASK_COLUMNS} FROM runs WHERE status = ?1 ORDER BY created_at, id"
                ),
                params![needle],
            )
        })
    }

    /// 全量列举（created_at 升序）。
    pub fn list_all(&self) -> Result<Vec<Task>, String> {
        self.db.with_conn(|conn| {
            list_rows(
                conn,
                &format!("SELECT {TASK_COLUMNS} FROM runs ORDER BY created_at, id"),
                params![],
            )
        })
    }
}

/// 列举收集：坏状态/坏 JSON 行 warn 跳过（单行毒化全表不可取）。
fn list_rows(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Task>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("tasks.db prepare list failed: {e}"))?;
    let mut rows = stmt
        .query(p)
        .map_err(|e| format!("tasks.db list failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .map_err(|e| format!("tasks.db list failed: {e}"))?
    {
        match row_task_strict(r) {
            Ok(t) => out.push(t),
            Err(e) => tracing::warn!("tasks.db row decode skipped: {e}"),
        }
    }
    Ok(out)
}

/// 行 → Task（严格：未知状态/坏 JSON = Err；调用方决定响亮还是跳过）。
fn row_task_strict(r: &rusqlite::Row<'_>) -> Result<Task, String> {
    let status_str: String = r.get(1).map_err(de)?;
    let payload_str: String = r.get(4).map_err(de)?;
    let result_str: Option<String> = r.get(5).map_err(de)?;
    Ok(Task {
        id: r.get(0).map_err(de)?,
        status: status_from_str(&status_str)?,
        action: r.get(2).map_err(de)?,
        peer_id: r.get(3).map_err(de)?,
        payload: serde_json::from_str(&payload_str)
            .map_err(|e| format!("tasks.db payload decode failed: {e}"))?,
        result: result_str
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| format!("tasks.db result decode failed: {e}"))?,
        original_channel: r.get(6).map_err(de)?,
        original_chat_id: r.get(7).map_err(de)?,
        created_at: r.get(8).map_err(de)?,
        completed_at: r.get(9).map_err(de)?,
    })
}

fn de(e: rusqlite::Error) -> String {
    format!("tasks.db row decode failed: {e}")
}

/// UPDATE 语句与绑定参数的打包（占位序：?1 id, ?2 status, ?3 result,
/// [?4 completed_at 终态才有], 末位 updated_at / 期望状态集）。
struct UpdateBind {
    sql: String,
    params: Vec<Box<dyn rusqlite::ToSql>>,
}

impl UpdateBind {
    fn new(task_id: &str, status: &TaskStatus, result: Option<Value>) -> Self {
        let terminal = is_terminal(status);
        let sql = if terminal {
            "UPDATE runs SET status = ?2, result = ?3, completed_at = ?4, updated_at = ?5 \
             WHERE id = ?1"
                .to_string()
        } else {
            "UPDATE runs SET status = ?2, result = ?3, updated_at = ?4 WHERE id = ?1".to_string()
        };
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(task_id.to_string()),
            Box::new(status_to_str(status).to_string()),
            Box::new(result.map(|v| v.to_string())),
        ];
        if terminal {
            params.push(Box::new(now_rfc3339()));
        }
        params.push(Box::new(now_rfc3339()));
        Self { sql, params }
    }

    fn push_extra(&mut self, s: &str) {
        self.params.push(Box::new(s.to_string()));
    }

    fn refs(&self) -> Vec<&dyn rusqlite::ToSql> {
        self.params.iter().map(|b| b.as_ref()).collect()
    }
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(ffi, _)
        if ffi.code == rusqlite::ErrorCode::ConstraintViolation)
}
