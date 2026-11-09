//! CRUD operations for usage statistics.

use std::path::Path;
#[cfg(feature = "usage-store")]
use std::sync::Mutex;

#[cfg(feature = "usage-store")]
use rusqlite::{Connection, ToSql, params, params_from_iter};

#[cfg(feature = "usage-store")]
use crate::db;
use crate::models::{
    LogFilter, ModelToolHealth, RequestLog, SessionUsageAgg, TrendPoint, UsageSummary,
};
use crate::pricing_store::PricingStore;

/// Thread-safe SQLite data store.
///
/// usage-store 关：空账本 stub——同型 API（写入 no-op、查询恒空、价目层
/// 照常），调用方零改动编译。降级语义见 [`Self::open`]。
pub struct DataStore {
    /// SQLite 连接（usage-store 关：字段不编译）。
    #[cfg(feature = "usage-store")]
    conn: Mutex<Connection>,
    /// 分层价目表（自定义 > 下载 > 内置），与 db 同目录落盘
    /// （`{workspace}/data/pricing_custom.json` 等）。LLM 调用方经
    /// [`Self::compute_cost_usd`] 计价，管理端经 [`Self::pricing`] 增删改。
    pricing: PricingStore,
}

impl DataStore {
    /// Open (or create) the database at `db_path`. The layered pricing store
    /// lives next to the db (`db_path.parent()`).
    ///
    /// usage-store 关：不建/不开任何数据库——空账本 stub（写入 no-op、
    /// 查询恒空），价目层照常。此形态属构建期裁剪（iotsmall/极小镜像），
    /// 响亮 warn 一次，不静默。
    #[cfg(not(feature = "usage-store"))]
    pub fn open(db_path: &Path) -> Result<Self, String> {
        tracing::warn!(
            "[DataStore] usage-store 未编译进本构建：用量账本不落盘，usage 查询恒空（价目层照常）"
        );
        let pricing = PricingStore::open(db_path.parent().unwrap_or_else(|| Path::new(".")))?;
        Ok(Self { pricing })
    }

    #[cfg(feature = "usage-store")]
    pub fn open(db_path: &Path) -> Result<Self, String> {
        let conn = db::init_db(db_path)?;
        let pricing = PricingStore::open(db_path.parent().unwrap_or_else(|| Path::new(".")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            pricing,
        })
    }

    /// Layered pricing store accessor (custom CRUD / download replace / meta).
    pub fn pricing(&self) -> &PricingStore {
        &self.pricing
    }

    /// Layered cost computation for one LLM request
    /// (custom > downloaded > embedded; unknown model → `0.0`).
    pub fn compute_cost_usd(
        &self,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> f64 {
        self.pricing.compute_cost_usd(
            model,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        )
    }

    /// Layered cost computation with per-component breakdown
    /// (明细表分项成本). Unknown model → `None`（调用方记空
    /// `pricing_model` + 全 0 分项）。
    pub fn compute_cost_breakdown(
        &self,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> Option<crate::CostBreakdown> {
        self.pricing.compute_cost_breakdown(
            model,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        )
    }

    /// Record a single LLM request log.
    ///
    /// usage-store 关：no-op（空账本不落盘）。
    #[cfg(not(feature = "usage-store"))]
    pub fn insert_request_log(&self, _log: &RequestLog) -> Result<(), String> {
        Ok(())
    }

    #[cfg(feature = "usage-store")]
    pub fn insert_request_log(&self, log: &RequestLog) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO request_logs
                (trace_id, model, provider_type, input_tokens, output_tokens,
                 cache_creation_tokens, cache_read_tokens, total_cost_usd,
                 latency_ms, status_code, error_message, is_streaming, created_at,
                 pricing_model, input_cost_usd, output_cost_usd,
                 cache_creation_cost_usd, cache_read_cost_usd,
                 first_token_ms, session_key)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![
                log.trace_id,
                log.model,
                log.provider_type,
                log.input_tokens,
                log.output_tokens,
                log.cache_creation_tokens,
                log.cache_read_tokens,
                log.total_cost_usd,
                log.latency_ms,
                log.status_code,
                log.error_message,
                log.is_streaming as i32,
                log.created_at,
                log.pricing_model,
                log.input_cost_usd,
                log.output_cost_usd,
                log.cache_creation_cost_usd,
                log.cache_read_cost_usd,
                log.first_token_ms,
                log.session_key,
            ],
        )
        .map_err(|e| format!("insert_request_log: {e}"))?;
        Ok(())
    }

    /// Idempotent insert: skip the write when a row with the same `trace_id`
    /// already exists. Cluster callback usage rows use
    /// `trace_id = "cluster-callback:{task_id}"`（单任务一笔聚合账），worker
    /// 回调重试（send_callback 至多 3 次）/ 恢复轮询重复回调时同一
    /// trace_id 重放——裸 INSERT 会把 token 记账翻倍，预算闸
    /// （`board.budget.max_tokens_per_parent`）提前耗尽错误冻结单据。
    /// 单语句 SELECT-INSERT 原子去重；返回 `false` = 命中重复已跳过。
    ///
    /// 语义边界：只对 `trace_id` 精确相等去重——普通 LLM 请求日志
    /// trace_id 各自独立，互不影响；`cluster-callback:` 前缀为集群回调
    /// 专用，不会与常规请求 trace_id 碰撞。
    ///
    /// usage-store 关：no-op stub（返回 `false` = 未落账，与
    /// [`Self:insert_request_log`] 的空账本语义一致）。
    #[cfg(not(feature = "usage-store"))]
    pub fn insert_request_log_dedup(&self, _log: &RequestLog) -> Result<bool, String> {
        Ok(false)
    }

    #[cfg(feature = "usage-store")]
    pub fn insert_request_log_dedup(&self, log: &RequestLog) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let inserted = conn
            .execute(
                "INSERT INTO request_logs
                    (trace_id, model, provider_type, input_tokens, output_tokens,
                     cache_creation_tokens, cache_read_tokens, total_cost_usd,
                     latency_ms, status_code, error_message, is_streaming, created_at,
                     pricing_model, input_cost_usd, output_cost_usd,
                     cache_creation_cost_usd, cache_read_cost_usd,
                     first_token_ms, session_key)
                SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20
                WHERE NOT EXISTS (
                    SELECT 1 FROM request_logs WHERE trace_id = ?21
                )",
                params![
                    log.trace_id,
                    log.model,
                    log.provider_type,
                    log.input_tokens,
                    log.output_tokens,
                    log.cache_creation_tokens,
                    log.cache_read_tokens,
                    log.total_cost_usd,
                    log.latency_ms,
                    log.status_code,
                    log.error_message,
                    log.is_streaming as i32,
                    log.created_at,
                    log.pricing_model,
                    log.input_cost_usd,
                    log.output_cost_usd,
                    log.cache_creation_cost_usd,
                    log.cache_read_cost_usd,
                    log.first_token_ms,
                    log.session_key,
                    log.trace_id,
                ],
            )
            .map_err(|e| format!("insert_request_log_dedup: {e}"))?;
        Ok(inserted == 1)
    }

    /// Query aggregated summary for a time range.
    ///
    /// usage-store 关：全零聚合（空账本语义，与"库从未写入"一致）。
    #[cfg(not(feature = "usage-store"))]
    pub fn query_summary(&self, _start_ts: i64, _end_ts: i64) -> Result<UsageSummary, String> {
        Ok(UsageSummary {
            total_requests: 0,
            success_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            total_cost_usd: 0.0,
            avg_latency_ms: 0.0,
            cache_hit_rate: 0.0,
        })
    }

    #[cfg(feature = "usage-store")]
    pub fn query_summary(&self, start_ts: i64, end_ts: i64) -> Result<UsageSummary, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT
                    COUNT(*)                  as total_requests,
                    COALESCE(SUM(CASE WHEN status_code = 200 THEN 1 ELSE 0 END), 0) as success_count,
                    COALESCE(SUM(input_tokens), 0)            as total_input_tokens,
                    COALESCE(SUM(output_tokens), 0)           as total_output_tokens,
                    COALESCE(SUM(cache_creation_tokens), 0)   as total_cache_creation_tokens,
                    COALESCE(SUM(cache_read_tokens), 0)       as total_cache_read_tokens,
                    COALESCE(SUM(total_cost_usd), 0.0)        as total_cost_usd,
                    COALESCE(AVG(CASE WHEN status_code = 200 THEN latency_ms END), 0.0) as avg_latency_ms
                 FROM request_logs
                 WHERE created_at >= ?1 AND created_at < ?2",
            )
            .map_err(|e| format!("prepare summary: {e}"))?;

        let summary = stmt
            .query_row(params![start_ts, end_ts], |row| {
                Ok(UsageSummary {
                    total_requests: row.get(0)?,
                    success_count: row.get(1)?,
                    total_input_tokens: row.get(2)?,
                    total_output_tokens: row.get(3)?,
                    total_cache_creation_tokens: row.get(4)?,
                    total_cache_read_tokens: row.get(5)?,
                    total_cost_usd: row.get(6)?,
                    avg_latency_ms: row.get(7)?,
                    cache_hit_rate: 0.0,
                })
            })
            .map_err(|e| format!("query_summary: {e}"))?;

        // Compute cache hit rate
        // 命中率 = 缓存读 / 总输入（input + creation + read），三协议族
        // 统一口径：input_tokens 列是归一后的非缓存纯输入（账本不变量），
        // 缓存读占总输入的比例。旧口径 read/(creation+read) 依赖「OpenAI
        // 族把 miss 落 creation 列」的旧喂料形态——miss 归纯输入后该分母
        // 不复存在，且 Anthropic 族旧口径漏掉纯输入段权重。
        let total_input = summary.total_input_tokens
            + summary.total_cache_creation_tokens
            + summary.total_cache_read_tokens;
        let cache_hit_rate = if total_input > 0 {
            summary.total_cache_read_tokens as f64 / total_input as f64
        } else {
            0.0
        };

        Ok(UsageSummary {
            cache_hit_rate,
            ..summary
        })
    }

    /// Query trend data grouped by hour or day.
    ///
    /// usage-store 关：空序列。
    #[cfg(not(feature = "usage-store"))]
    pub fn query_trends(
        &self,
        _start_ts: i64,
        _end_ts: i64,
        _group_by: &str,
    ) -> Result<Vec<TrendPoint>, String> {
        Ok(Vec::new())
    }

    #[cfg(feature = "usage-store")]
    pub fn query_trends(
        &self,
        start_ts: i64,
        end_ts: i64,
        group_by: &str,
    ) -> Result<Vec<TrendPoint>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let sql = match group_by {
            "hour" => {
                r#"
                SELECT
                    strftime('%Y-%m-%dT%H:00:00', created_at, 'unixepoch') as label,
                    (created_at / 3600) * 3600 as ts,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COUNT(*),
                    COALESCE(SUM(total_cost_usd), 0.0)
                FROM request_logs
                WHERE created_at >= ?1 AND created_at < ?2
                GROUP BY ts ORDER BY ts"#
            }
            _ => {
                r#"
                SELECT
                    strftime('%Y-%m-%d', created_at, 'unixepoch') as label,
                    (created_at / 86400) * 86400 as ts,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COUNT(*),
                    COALESCE(SUM(total_cost_usd), 0.0)
                FROM request_logs
                WHERE created_at >= ?1 AND created_at < ?2
                GROUP BY ts ORDER BY ts"#
            }
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("prepare trends: {e}"))?;

        let points = stmt
            .query_map(params![start_ts, end_ts], |row| {
                Ok(TrendPoint {
                    label: row.get(0)?,
                    timestamp: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    cache_creation_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    request_count: row.get(6)?,
                    total_cost_usd: row.get(7)?,
                })
            })
            .map_err(|e| format!("query_trends: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(points)
    }

    /// Query request logs with pagination and optional orthogonal filters
    /// (model 子串 / status 精确 / session_key 子串， 请求明细 tab)。
    ///
    /// usage-store 关：空列表 + 总数 0。
    #[cfg(not(feature = "usage-store"))]
    pub fn query_logs(
        &self,
        _start_ts: i64,
        _end_ts: i64,
        _page: i32,
        _page_size: i32,
        _filter: &LogFilter,
    ) -> Result<(Vec<RequestLog>, i64), String> {
        Ok((Vec::new(), 0))
    }

    #[cfg(feature = "usage-store")]
    pub fn query_logs(
        &self,
        start_ts: i64,
        end_ts: i64,
        page: i32,
        page_size: i32,
        filter: &LogFilter,
    ) -> Result<(Vec<RequestLog>, i64), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // store 层分页边界对称防护：page 有 .max(1)（下界），page_size 此前
        // 只有 API 层 .min(100) 钳上界——负值透传时 LIMIT -N 是 SQLite
        // 「无上限」语义（BUG-20261108-025）。乘法升 i64：page×page_size
        // 大页值（API query 参数可任意 poke）在 i32 域溢出，release 回绕出
        // 负 OFFSET（SQLite 视 0 → 静默回第一页）/debug panic。
        let page_size = page_size.max(1);
        let offset = (page.max(1) - 1) as i64 * page_size as i64;

        // WHERE 动态拼装：时间范围恒在（?1/?2），过滤条件按需追加。
        // 参数占位序号 = 绑定序（1-based），与 params_vec 压入顺序一致。
        // 用户搜索词 LIKE 字面化（对齐 /BUG-163 board
        // 口径）——`%`/`_` 是 SQL LIKE 通配符，不转义时 `%q%` 包裹的搜索词
        // 会按通配符解释（搜 `_` 命中全部单字符位、搜字面 `%` 变全量）。
        // 转义与 `ESCAPE '\'` 子句成对（统一定义 nemesis_utils::sanitize）。
        let mut conds: Vec<String> = vec![
            "created_at >= ?1".to_string(),
            "created_at < ?2".to_string(),
        ];
        let mut bind: Vec<Box<dyn ToSql>> = vec![Box::new(start_ts), Box::new(end_ts)];
        if let Some(m) = filter.model.as_deref().filter(|s| !s.is_empty()) {
            bind.push(Box::new(format!(
                "%{}%",
                nemesis_utils::sanitize::escape_like(m)
            )));
            conds.push(format!("model LIKE ?{} ESCAPE '\\'", bind.len()));
        }
        if let Some(s) = filter.status {
            bind.push(Box::new(s));
            conds.push(format!("status_code = ?{}", bind.len()));
        }
        if let Some(sk) = filter.session_key.as_deref().filter(|s| !s.is_empty()) {
            bind.push(Box::new(format!(
                "%{}%",
                nemesis_utils::sanitize::escape_like(sk)
            )));
            conds.push(format!("session_key LIKE ?{} ESCAPE '\\'", bind.len()));
        }
        let where_clause = conds.join(" AND ");

        let count_sql = format!("SELECT COUNT(*) FROM request_logs WHERE {where_clause}");
        let total: i64 = conn
            .query_row(
                &count_sql,
                params_from_iter(bind.iter().map(|b| b.as_ref())),
                |row| row.get(0),
            )
            .map_err(|e| format!("count logs: {e}"))?;

        let select_sql = format!(
            "SELECT id, trace_id, model, provider_type, input_tokens, output_tokens,
                    cache_creation_tokens, cache_read_tokens, total_cost_usd,
                    latency_ms, status_code, error_message, is_streaming, created_at,
                    pricing_model, input_cost_usd, output_cost_usd,
                    cache_creation_cost_usd, cache_read_cost_usd,
                    first_token_ms, session_key
             FROM request_logs
             WHERE {where_clause}
             ORDER BY created_at DESC
             LIMIT ?{} OFFSET ?{}",
            bind.len() + 1,
            bind.len() + 2,
        );
        bind.push(Box::new(page_size));
        bind.push(Box::new(offset));

        let mut stmt = conn
            .prepare(&select_sql)
            .map_err(|e| format!("prepare logs: {e}"))?;

        let logs = stmt
            .query_map(
                params_from_iter(bind.iter().map(|b| b.as_ref())),
                row_to_log,
            )
            .map_err(|e| format!("query_logs: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok((logs, total))
    }

    /// Fetch a single request-log row by id（请求明细详情面板）。
    ///
    /// usage-store 关：None（空账本无行）。
    #[cfg(not(feature = "usage-store"))]
    pub fn get_request_log(&self, _id: i64) -> Result<Option<RequestLog>, String> {
        Ok(None)
    }

    #[cfg(feature = "usage-store")]
    pub fn get_request_log(&self, id: i64) -> Result<Option<RequestLog>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, trace_id, model, provider_type, input_tokens, output_tokens,
                        cache_creation_tokens, cache_read_tokens, total_cost_usd,
                        latency_ms, status_code, error_message, is_streaming, created_at,
                        pricing_model, input_cost_usd, output_cost_usd,
                        cache_creation_cost_usd, cache_read_cost_usd,
                        first_token_ms, session_key
                 FROM request_logs WHERE id = ?1",
            )
            .map_err(|e| format!("prepare get_request_log: {e}"))?;
        let mut rows = stmt
            .query_map(params![id], row_to_log)
            .map_err(|e| format!("get_request_log: {e}"))?;
        match rows.next() {
            Some(Ok(log)) => Ok(Some(log)),
            Some(Err(e)) => Err(format!("get_request_log row: {e}")),
            None => Ok(None),
        }
    }

    /// 单会话用量聚合（会话级 cost/context 常驻）。
    ///
    /// `session_key` **精确匹配**（调用方已持完整键，如
    /// `agent:main:session:{sid}`——与 loop.rs 写入 RequestLog 的
    /// `context.session_key` 同源，无需 LIKE 子串宽松匹配）。
    /// 会话无请求行 → 全零聚合（不是错误）。
    ///
    /// `input_tokens` 口径 = 完整输入（prompt + cache_creation +
    /// cache_read； 补全 cache_creation——prompt 列是 provider
    /// 非缓存纯输入，Anthropic 形态漏计 creation 会话预算系统性偏低），
    /// 与 loop 侧 `usage_full_input`（预算 delta / 压缩信号）同口径。
    ///
    /// usage-store 关：恒全零聚合（空账本语义）。
    #[cfg(not(feature = "usage-store"))]
    pub fn aggregate_session_usage(&self, _session_key: &str) -> Result<SessionUsageAgg, String> {
        Ok(SessionUsageAgg {
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            total_cost_usd: 0.0,
        })
    }

    #[cfg(feature = "usage-store")]
    pub fn aggregate_session_usage(&self, session_key: &str) -> Result<SessionUsageAgg, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT
                    COUNT(*),
                    COALESCE(SUM(input_tokens + cache_creation_tokens + cache_read_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(total_cost_usd), 0.0)
                 FROM request_logs WHERE session_key = ?1",
            )
            .map_err(|e| format!("prepare aggregate_session_usage: {e}"))?;
        stmt.query_row(params![session_key], |row| {
            Ok(SessionUsageAgg {
                requests: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                total_cost_usd: row.get(3)?,
            })
        })
        .map_err(|e| format!("aggregate_session_usage: {e}"))
    }

    /// 按任务后缀聚合集群委派用量（维度 4 全链 token 聚合专用）。
    ///
    /// master 记账键 `cluster_rpc:{worker}/{task_id}` 的 worker 段是**传输层
    /// 运行时节点 id**（回调帧 `_rpc.from`），而派发行 `issue_dispatch.worker_id`
    /// 存的是**派发时 peer 名**（如 `Node-B`）——两者不同源，精确键匹配必落空
    /// （真机 run 4 实证：聚合恒 0 → token 闸永不熔断）。task_id（UUID）
    /// 是 per-dispatch 唯一锚，按 `/{task_id}` 后缀匹配对两种身份形态都稳。
    /// 无请求行 → 全零聚合（不是错误）。
    ///
    /// `input_tokens` 口径 = 完整输入（prompt + cache_creation +
    /// cache_read，与 `aggregate_session_usage` 同式， 补全）。
    ///
    /// usage-store 关：恒全零聚合（空账本语义；token 预算闸读到的恒是
    /// 0 占用——闸本身 0=关默认，不受影响）。
    #[cfg(not(feature = "usage-store"))]
    pub fn aggregate_session_usage_by_task(
        &self,
        _task_id: &str,
    ) -> Result<SessionUsageAgg, String> {
        Ok(SessionUsageAgg {
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            total_cost_usd: 0.0,
        })
    }

    #[cfg(feature = "usage-store")]
    pub fn aggregate_session_usage_by_task(
        &self,
        task_id: &str,
    ) -> Result<SessionUsageAgg, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // task_id 是 pub 自由参数：`%`/`_` 进 LIKE pattern 会被解释成
        // 通配符 → 别的 task 的账算进本 task（dossier usage.csv 与
        // token 预算保险丝消费此聚合 = 错账）。与上方 request_logs
        // 过滤（BUG-163 口径）同源：escape_like + `ESCAPE '\'` 成对，
        // pattern 开头的 `%` 是尾部锚定语义、刻意保留。
        let pattern = format!("%/{}", nemesis_utils::sanitize::escape_like(task_id));
        let mut stmt = conn
            .prepare(
                "SELECT
                    COUNT(*),
                    COALESCE(SUM(input_tokens + cache_creation_tokens + cache_read_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(total_cost_usd), 0.0)
                 FROM request_logs WHERE session_key LIKE ?1 ESCAPE '\\'",
            )
            .map_err(|e| format!("prepare aggregate_session_usage_by_task: {e}"))?;
        stmt.query_row(params![pattern], |row| {
            Ok(SessionUsageAgg {
                requests: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                total_cost_usd: row.get(3)?,
            })
        })
        .map_err(|e| format!("aggregate_session_usage_by_task: {e}"))
    }

    /// P2B（根修配套）：记一笔工具调用与参数校验结果
    /// （天×模型 upsert 增量）。`failed` = 本次调用 args_validator 判 Invalid。
    /// Valid/Fixed 都算成功（Fixed 已自动修复，不是模型失败证据）。
    /// 调用方对错误只 warn——审计是增值动作，不反压轮次。
    ///
    /// usage-store 关：no-op（健康账不落盘，`models.health` 恒空）。
    #[cfg(not(feature = "usage-store"))]
    pub fn record_tool_validation(&self, _model: &str, _failed: bool) -> Result<(), String> {
        Ok(())
    }

    #[cfg(feature = "usage-store")]
    pub fn record_tool_validation(&self, model: &str, failed: bool) -> Result<(), String> {
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO tool_validation_stats (day, model, tool_calls, validation_failures)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(day, model) DO UPDATE SET
                tool_calls = tool_calls + 1,
                validation_failures = validation_failures + ?3",
            params![day, model, i64::from(failed)],
        )
        .map_err(|e| format!("record_tool_validation: {e}"))?;
        Ok(())
    }

    /// P2B：近 `days` 天的每模型工具健康聚合（`models.health` 数据源）。
    /// `days <= 0` 视为 1。无记录的模型不出现（无调用即无健康问题可言）。
    ///
    /// usage-store 关：空列表（健康账未编译，无模型出现）。
    #[cfg(not(feature = "usage-store"))]
    pub fn query_model_tool_health(&self, _days: i64) -> Result<Vec<ModelToolHealth>, String> {
        Ok(Vec::new())
    }

    #[cfg(feature = "usage-store")]
    pub fn query_model_tool_health(&self, days: i64) -> Result<Vec<ModelToolHealth>, String> {
        let cutoff = (chrono::Local::now() - chrono::Duration::days(days.max(1)))
            .format("%Y-%m-%d")
            .to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT model,
                        COALESCE(SUM(tool_calls), 0),
                        COALESCE(SUM(validation_failures), 0)
                 FROM tool_validation_stats
                 WHERE day >= ?1
                 GROUP BY model
                 ORDER BY tool_calls DESC",
            )
            .map_err(|e| format!("prepare model tool health: {e}"))?;
        let mut rows: Vec<ModelToolHealth> = stmt
            .query_map(params![cutoff], |row| {
                let model: String = row.get(0)?;
                let calls: i64 = row.get(1)?;
                let failures: i64 = row.get(2)?;
                let rate = if calls > 0 {
                    failures as f64 / calls as f64
                } else {
                    0.0
                };
                Ok(ModelToolHealth {
                    model,
                    tool_calls: calls,
                    validation_failures: failures,
                    failure_rate: rate,
                    guard_events: 0,
                    hint: None,
                })
            })
            .map_err(|e| format!("query_model_tool_health: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        // （引入）：守卫事件并入（同窗口聚合）。
        // 并集语义：守卫事件是独立观测面——只有守卫事件、没有工具调用的
        // 模型（空答复/文本重复型打转）也要出现在健康行里（guard_events
        // 驱动的 hint 正是为它准备的），不允许静默丢弃。
        let mut guard_stmt = conn
            .prepare(
                "SELECT model, COALESCE(SUM(events), 0)
                 FROM guard_event_stats
                 WHERE day >= ?1
                 GROUP BY model",
            )
            .map_err(|e| format!("prepare guard events: {e}"))?;
        let guard_map: std::collections::HashMap<String, i64> = guard_stmt
            .query_map(params![cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("query guard events: {e}"))?
            .filter_map(|r| r.ok())
            .collect();
        for r in rows.iter_mut() {
            if let Some(events) = guard_map.get(&r.model) {
                r.guard_events = *events;
            }
        }
        for (model, events) in &guard_map {
            if !rows.iter().any(|r| &r.model == model) {
                rows.push(ModelToolHealth {
                    model: model.clone(),
                    tool_calls: 0,
                    validation_failures: 0,
                    failure_rate: 0.0,
                    guard_events: *events,
                    hint: None,
                });
            }
        }
        rows.sort_by_key(|b| std::cmp::Reverse(b.tool_calls));
        Ok(rows)
    }

    /// （引入）：记一笔守卫事件计数（天×模型
    /// upsert 增量）。`events` = 本 turn 实际发出的 loop-guard nudge 总数，
    /// 0 不写行（与「无事件即无记录」语义一致，turn 频次远高于守卫事件）。
    /// 调用方对错误只 warn——审计是增值动作，不反压轮次。
    ///
    /// usage-store 关：no-op（健康账不落盘，`models.health` 恒空）。
    #[cfg(not(feature = "usage-store"))]
    pub fn record_guard_events(&self, _model: &str, _events: u32) -> Result<(), String> {
        Ok(())
    }

    #[cfg(feature = "usage-store")]
    pub fn record_guard_events(&self, model: &str, events: u32) -> Result<(), String> {
        if events == 0 {
            return Ok(());
        }
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO guard_event_stats (day, model, events)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(day, model) DO UPDATE SET
                events = events + ?3",
            params![day, model, i64::from(events)],
        )
        .map_err(|e| format!("record_guard_events: {e}"))?;
        Ok(())
    }

    /// Roll up request logs older than 30 days into daily_rollups and delete originals.
    /// Retention sweep（保留策略，可配置）：把 `retention_days` 天前的
    /// 明细行 rollup 进 `daily_rollups` 后删除（`None` = 跳过按天清理，
    /// 对应 config `usage.retention_days = 0`）；`max_rows` 给定时再按
    /// created_at 最旧优先裁掉超额行（防膨胀；BUG-20261107-021 起被裁行
    /// **先聚合再删**——旧形态直接删，保留窗内的行从未进 daily_rollups
    /// 也永远不会进，永久史账少记）。返回删除的总行数。
    ///
    /// 挂载点在 gateway 周期任务（启动时 + 每 6h）；此前 `rollup_old_logs`
    /// 从未被生产调用（只有测试），本次一并接上。
    ///
    /// usage-store 关：Ok(0)（无账可清）。
    #[cfg(not(feature = "usage-store"))]
    pub fn retention_sweep(
        &self,
        _retention_days: Option<i64>,
        _max_rows: Option<i64>,
    ) -> Result<u64, String> {
        Ok(0)
    }

    #[cfg(feature = "usage-store")]
    pub fn retention_sweep(
        &self,
        retention_days: Option<i64>,
        max_rows: Option<i64>,
    ) -> Result<u64, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut deleted_total: u64 = 0;

        // 入口钳制（R735 数字恐慌面）：days 是 config 值——`days * 86400`
        // 在 days > i64::MAX/86400（~1.07e14）时溢出（debug panic / release
        // 回绕出不可预测 cutoff）。钳到 100 年上界（单一真相源
        // time_bounds::MAX_AGE_DAYS），正常保留期恒等。
        if let Some(days) = retention_days
            .filter(|d| *d > 0)
            .map(|d| d.min(nemesis_utils::time_bounds::MAX_AGE_DAYS))
        {
            // cutoff 对齐 UTC 日界（floor 到当日 0 点，与 GROUP BY 的
            // strftime '%Y-%m-%d' unixepoch 同族）。BUG-20261107-021 之后
            // 对齐不再是正确性必需（聚合改加法合并 + 事务原子，同日多批
            // 贡献累加、crash 重放不可能），保留对齐只为批次边界整齐、
            // 与 2026-10-24 的回归锁语义连续。
            let cutoff = (chrono::Local::now().timestamp() - days * 86400) / 86400 * 86400;
            deleted_total += sweep_step(
                &mut conn,
                "created_at < ?1",
                "DELETE FROM request_logs WHERE created_at < ?1",
                cutoff,
            )?;
        }

        // 条数上限：超出部分按最旧优先**先聚合再裁**（BUG-20261107-021）。
        // 旧形态直接删——按天步只聚合 `created_at < cutoff`（保留窗之外），
        // 超限裁的是窗内行，从未进 daily_rollups 也永远不会进（正是
        // cutoff 对齐设计要防的「永久史账系统性少记」形态）。裁剪集 =
        // `ORDER BY created_at DESC, id DESC` 落在最新 cap 行之后的全部行
        // （id 决胜平 tie 确定性）；rollup 与 DELETE 用同一子查询、同一
        // 事务，聚合集合 = 删除集合恒等。
        if let Some(cap) = max_rows.filter(|c| *c > 0) {
            const TRIM_WHERE: &str = "id IN (SELECT id FROM request_logs \
                 ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1)";
            deleted_total += sweep_step(
                &mut conn,
                TRIM_WHERE,
                &format!("DELETE FROM request_logs WHERE {TRIM_WHERE}"),
                cap,
            )?;
        }

        if deleted_total > 0 {
            tracing::info!(
                deleted_total,
                ?retention_days,
                "[DataStore] Retention sweep removed old request logs"
            );
        }

        Ok(deleted_total)
    }
}

/// 聚合 SQL 头（列清单，截至 `WHERE `；按天步与裁剪步共用单一真相源）。
#[cfg(feature = "usage-store")]
const ROLLUP_SQL_HEAD: &str = "INSERT INTO daily_rollups
    (date, model, request_count, success_count, input_tokens, output_tokens,
     cache_creation_tokens, cache_read_tokens, total_cost_usd, avg_latency_ms)
 SELECT
    strftime('%Y-%m-%d', created_at, 'unixepoch'),
    model,
    COUNT(*),
    SUM(CASE WHEN status_code = 200 THEN 1 ELSE 0 END),
    COALESCE(SUM(input_tokens), 0),
    COALESCE(SUM(output_tokens), 0),
    COALESCE(SUM(cache_creation_tokens), 0),
    COALESCE(SUM(cache_read_tokens), 0),
    COALESCE(SUM(total_cost_usd), 0.0),
    COALESCE(AVG(CASE WHEN status_code = 200 THEN latency_ms END), 0.0)
 FROM request_logs
 WHERE ";

/// 聚合 SQL 尾：加法合并 upsert（BUG-20261107-021）。同一 (date, model)
/// 可被多批合法贡献（裁剪步劈日 + 按天步收尾），各批计数/token/成本累加；
/// `avg_latency_ms` 按 success_count 加权平均（两批 success 均值的合并
/// 均值；success 全零保 0.0）。全行覆盖的 INSERT OR REPLACE 已退役。
#[cfg(feature = "usage-store")]
const ROLLUP_UPSERT_TAIL: &str = "
 GROUP BY strftime('%Y-%m-%d', created_at, 'unixepoch'), model
 ON CONFLICT(date, model) DO UPDATE SET
    request_count = request_count + excluded.request_count,
    success_count = success_count + excluded.success_count,
    input_tokens = input_tokens + excluded.input_tokens,
    output_tokens = output_tokens + excluded.output_tokens,
    cache_creation_tokens = cache_creation_tokens + excluded.cache_creation_tokens,
    cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
    total_cost_usd = total_cost_usd + excluded.total_cost_usd,
    avg_latency_ms = CASE
        WHEN daily_rollups.success_count + excluded.success_count > 0 THEN
            (daily_rollups.avg_latency_ms * daily_rollups.success_count
                + excluded.avg_latency_ms * excluded.success_count)
            / (daily_rollups.success_count + excluded.success_count)
        ELSE 0.0
    END";

/// 单个 sweep 步：聚合 + 删除**同一事务**（BUG-20261107-021）。事务原子
/// 性使 crash 重放不可能——已删行绝不会被二次聚合（删除本身就是水位线，
/// 无需水位表）；步骤内部任一语句失败整体回滚，绝不出现「rollup 已写、
/// 明细仍在」或反向半态。
#[cfg(feature = "usage-store")]
fn sweep_step(
    conn: &mut Connection,
    rollup_where: &str,
    delete_sql: &str,
    param: i64,
) -> Result<u64, String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("retention sweep tx: {e}"))?;
    tx.execute(
        &format!("{ROLLUP_SQL_HEAD}{rollup_where}{ROLLUP_UPSERT_TAIL}"),
        params![param],
    )
    .map_err(|e| format!("rollup upsert: {e}"))?;
    let deleted = tx
        .execute(delete_sql, params![param])
        .map_err(|e| format!("sweep delete: {e}"))?;
    tx.commit()
        .map_err(|e| format!("retention sweep commit: {e}"))?;
    Ok(deleted as u64)
}

/// Row → [`RequestLog`]（列序与 query_logs / get_request_log 的 SELECT 一致）。
#[cfg(feature = "usage-store")]
fn row_to_log(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestLog> {
    Ok(RequestLog {
        id: row.get(0)?,
        trace_id: row.get(1)?,
        model: row.get(2)?,
        provider_type: row.get(3)?,
        input_tokens: row.get(4)?,
        output_tokens: row.get(5)?,
        cache_creation_tokens: row.get(6)?,
        cache_read_tokens: row.get(7)?,
        total_cost_usd: row.get(8)?,
        latency_ms: row.get(9)?,
        status_code: row.get(10)?,
        error_message: row.get(11)?,
        is_streaming: row.get::<_, i32>(12)? != 0,
        created_at: row.get(13)?,
        pricing_model: row.get(14)?,
        input_cost_usd: row.get(15)?,
        output_cost_usd: row.get(16)?,
        cache_creation_cost_usd: row.get(17)?,
        cache_read_cost_usd: row.get(18)?,
        first_token_ms: row.get(19)?,
        session_key: row.get(20)?,
    })
}

// SQLite 行为测试随 usage-store feature 进出（off 态 stub 无库可测；
// standalone / --workspace 构建经 default / dev-deps / unification 恒带此 feature）。
#[cfg(all(test, feature = "usage-store"))]
mod tests;

// Wave6B 覆盖率收尾：见 cov_wave6b_tests.rs 头注释。
#[cfg(all(test, feature = "usage-store"))]
mod cov_wave6b_tests;
