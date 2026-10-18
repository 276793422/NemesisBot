//! runs / attempts 统一层通用 API（最小面；P2/P4 在此扩展）。
//!
//! 状态机：`pending → running → succeeded | failed | cancelled`。
//! 终态不可再转移（恢复重入语义由上层 status 语义承载；这里守住
//! 「终态不被静默改写」的统一层底线）。

use rusqlite::{OptionalExtension, params};

use crate::db::TaskDb;
use crate::tasks::now_rfc3339;

/// runs.status 合法值集（TaskStore 适配层之外的通用面用字符串口径）。
pub const RUN_STATUSES: [&str; 5] = ["pending", "running", "succeeded", "failed", "cancelled"];

fn is_terminal_str(s: &str) -> bool {
    matches!(s, "succeeded" | "failed" | "cancelled")
}

#[derive(Debug, Clone, Default)]
pub struct NewRun {
    /// 领域归类：`cluster_task` / `board_dispatch` / `agent_run` / ...
    pub kind: String,
    pub title: String,
    pub action: String,
    pub peer_id: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub action: String,
    pub peer_id: String,
    pub payload: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AttemptRecord {
    pub id: i64,
    pub run_id: String,
    pub seq: i64,
    pub worker: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub usage: Option<serde_json::Value>,
}

impl TaskDb {
    /// 建run（status=pending）。返回新 run id（uuid）。
    pub fn create_run(&self, run: &NewRun) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        self.create_run_with_id(&id, run)
    }

    /// 以调用方指定的 id 建 run（status=pending）。G3 运行账本漏斗的
    /// 确定性 id 入口（`board_dispatch:{task_id}` / `cron_job:{job}:{ms}` /
    /// `workflow_run:{execution_id}`）——id 先于内容确定，跨重启凭 id 直解，
    /// 不需要内存关联表。幂等：id 已在账（防御性重入/并发建账）→ 原样
    /// 返回既有行，不覆盖（内容由同源确定性输入推导，重复建账无意义）。
    pub fn create_run_with_id(&self, id: &str, run: &NewRun) -> Result<String, String> {
        if self.get_run(id).is_ok() {
            return Ok(id.to_string());
        }
        let inserted = self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO runs (id, kind, title, status, action, peer_id, payload, \
                 created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?7)",
                params![
                    id,
                    run.kind,
                    run.title,
                    run.action,
                    run.peer_id,
                    run.payload.to_string(),
                    now_rfc3339(),
                ],
            )
            .map_err(|e| format!("tasks.db insert run failed: {e}"))?;
            Ok(())
        });
        match inserted {
            Ok(()) => Ok(id.to_string()),
            // 并发窗口（另一连接先建同 id）：幂等放行——行已在账。
            Err(e) if e.contains("UNIQUE") => Ok(id.to_string()),
            Err(e) => Err(e),
        }
    }

    /// 转移状态。终态不可再转移（幂等重入同值放行）；非法值响亮拒绝。
    /// R5：守卫下推到单语句 CAS——SELECT 后裸 UPDATE 在两连接并发时后写
    /// 会覆盖先到的终态；现在放行条件（同值幂等 / 当前非终态）编码进
    /// WHERE，终态互斥由单条 UPDATE 的原子性保证，先到的终态写赢。
    pub fn set_run_status(&self, run_id: &str, status: &str) -> Result<(), String> {
        if !RUN_STATUSES.contains(&status) {
            return Err(format!("invalid run status: {status}"));
        }
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    // completed_at 只在「非终态 → 终态」的转移时落笔
                    // （同终态幂等重标记不改写首次收口时间）；非终态转移
                    // 保持 NULL 原样。
                    "UPDATE runs SET status = ?2, \
                     completed_at = CASE WHEN ?2 IN \
                     ('succeeded','failed','cancelled') AND status != ?2 \
                     THEN ?3 ELSE completed_at END, updated_at = ?3 \
                     WHERE id = ?1 AND (status = ?2 OR status NOT IN \
                     ('succeeded','failed','cancelled'))",
                    params![run_id, status, now_rfc3339()],
                )
                .map_err(|e| format!("tasks.db update run failed: {e}"))?;
            if n == 1 {
                return Ok(());
            }
            // n=0：不存在 / 当前是不同终态 / 竞争窗口内刚被并发改写。
            // 重读定性好报错（R5 回归锁：竞争败方 Err，胜方唯一）。
            let current: Option<String> = conn
                .query_row(
                    "SELECT status FROM runs WHERE id = ?1",
                    params![run_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("tasks.db read run failed: {e}"))?;
            match current {
                None => Err(format!("run not found: {run_id}")),
                Some(c) if is_terminal_str(&c) && c != status => Err(format!(
                    "run {run_id} is terminal ({c}); refusing transition to {status}"
                )),
                // 并发对端已把状态写成本次目标值 = 幂等收口。
                Some(_) => Ok(()),
            }
        })
    }

    fn run_from_row(r: &rusqlite::Row) -> rusqlite::Result<RunRecord> {
        let payload_str: String = r.get(6)?;
        let result_str: Option<String> = r.get(7)?;
        Ok(RunRecord {
            id: r.get(0)?,
            kind: r.get(1)?,
            title: r.get(2)?,
            status: r.get(3)?,
            action: r.get(4)?,
            peer_id: r.get(5)?,
            payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
            result: result_str.and_then(|s| serde_json::from_str(&s).ok()),
            created_at: r.get(8)?,
            completed_at: r.get(9)?,
        })
    }

    fn get_run_row(conn: &rusqlite::Connection, run_id: &str) -> Result<RunRecord, String> {
        conn.query_row(
            "SELECT id, kind, title, status, action, peer_id, payload, result, \
             created_at, completed_at FROM runs WHERE id = ?1",
            params![run_id],
            Self::run_from_row,
        )
        .optional()
        .map_err(|e| format!("tasks.db read run failed: {e}"))?
        .ok_or_else(|| format!("run not found: {run_id}"))
    }

    /// 读 run。不存在 = Err。
    pub fn get_run(&self, run_id: &str) -> Result<RunRecord, String> {
        self.with_conn(|conn| Self::get_run_row(conn, run_id))
    }

    /// 记一次执行尝试（seq 自动递增；重派语义的账面）。
    /// R5：终态 run 拒绝新尝试（收口后不再重派——重开须新 run）。
    pub fn begin_attempt(&self, run_id: &str, worker: &str) -> Result<i64, String> {
        self.with_conn(|conn| {
            // run 存在性先验（外键在 PRAGMA foreign_keys 未开时不拦——
            // 本库不依赖外键强制，靠先验）。
            let run = Self::get_run_row(conn, run_id)?;
            if is_terminal_str(&run.status) {
                return Err(format!(
                    "run {run_id} is terminal ({}); refusing new attempt",
                    run.status
                ));
            }
            let seq: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM attempts WHERE run_id = ?1",
                    params![run_id],
                    |r| r.get(0),
                )
                .map_err(|e| format!("tasks.db next attempt seq failed: {e}"))?;
            conn.execute(
                "INSERT INTO attempts (run_id, seq, worker, status, started_at) \
                 VALUES (?1, ?2, ?3, 'running', ?4)",
                params![run_id, seq, worker, now_rfc3339()],
            )
            .map_err(|e| format!("tasks.db insert attempt failed: {e}"))?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// 收口一次尝试（只能终态收口：succeeded|failed|cancelled；pending/
    /// running/非法值拒绝——R5 对齐本注释与实现）。
    pub fn finish_attempt(
        &self,
        attempt_id: i64,
        status: &str,
        error: Option<&str>,
        usage: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        if !is_terminal_str(status) {
            return Err(format!(
                "invalid attempt finish status: {status}（只能终态收口：\
                 succeeded/failed/cancelled）"
            ));
        }
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE attempts SET status = ?2, finished_at = ?3, error = ?4, \
                     usage_json = ?5 WHERE id = ?1 AND status = 'running'",
                    params![
                        attempt_id,
                        status,
                        now_rfc3339(),
                        error,
                        usage.map(|v| v.to_string()),
                    ],
                )
                .map_err(|e| format!("tasks.db finish attempt failed: {e}"))?;
            if n == 0 {
                return Err(format!(
                    "attempt not found or already finished: {attempt_id}"
                ));
            }
            Ok(())
        })
    }

    /// 某 run 的尝试史（seq 升序）。
    pub fn attempts_of(&self, run_id: &str) -> Result<Vec<AttemptRecord>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, run_id, seq, worker, status, started_at, finished_at, \
                     error, usage_json FROM attempts WHERE run_id = ?1 ORDER BY seq",
                )
                .map_err(|e| format!("tasks.db list attempts failed: {e}"))?;
            let rows = stmt
                .query_map(params![run_id], |r| {
                    let usage_str: Option<String> = r.get(8)?;
                    Ok(AttemptRecord {
                        id: r.get(0)?,
                        run_id: r.get(1)?,
                        seq: r.get(2)?,
                        worker: r.get(3)?,
                        status: r.get(4)?,
                        started_at: r.get(5)?,
                        finished_at: r.get(6)?,
                        error: r.get(7)?,
                        usage: usage_str.and_then(|s| serde_json::from_str(&s).ok()),
                    })
                })
                .map_err(|e| format!("tasks.db list attempts failed: {e}"))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| format!("tasks.db attempt decode failed: {e}"))?);
            }
            Ok(out)
        })
    }

    /// 某类目下未收口的 run（pending/running，created_at 升序）。
    /// G3 启动 sweep 的扫描面：重启孤儿按 kind 圈定（cron_job 单发语义
    /// 的在途触发），不越界到他类目（board/cluster/workflow 的在途 run
    /// 由各自生产收口路径负责，重启不等于死亡）。
    pub fn list_open_runs(&self, kind: &str) -> Result<Vec<RunRecord>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, title, status, action, peer_id, payload, result, \
                     created_at, completed_at FROM runs \
                     WHERE kind = ?1 AND status IN ('pending','running') ORDER BY created_at",
                )
                .map_err(|e| format!("tasks.db list open runs failed: {e}"))?;
            let rows = stmt
                .query_map(params![kind], Self::run_from_row)
                .map_err(|e| format!("tasks.db list open runs failed: {e}"))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| format!("tasks.db run decode failed: {e}"))?);
            }
            Ok(out)
        })
    }

    /// 某类目最近 N 条 run（created_at 降序）。记账面的查询口
    /// （回归锁/后续 Dashboard 记账视图同源）。
    pub fn list_runs_by_kind(&self, kind: &str, limit: usize) -> Result<Vec<RunRecord>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, title, status, action, peer_id, payload, result, \
                     created_at, completed_at FROM runs \
                     WHERE kind = ?1 ORDER BY created_at DESC LIMIT ?2",
                )
                .map_err(|e| format!("tasks.db list runs failed: {e}"))?;
            let rows = stmt
                .query_map(params![kind, limit as i64], Self::run_from_row)
                .map_err(|e| format!("tasks.db list runs failed: {e}"))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| format!("tasks.db run decode failed: {e}"))?);
            }
            Ok(out)
        })
    }
}
