//! P4-c（2026-10-17）— `deliveries.*` WSAPI + 交付账本查询/裁决面。
//!
//! 依赖方向（同 [`crate::handlers::agents`] 先例）：nemesis-web 不能反向
//! 依赖 nemesisbot（deliveries 表的 `Arc<TaskDb>` 在那边），所以这里定义
//! [`DeliveriesBridge`] trait + 模块级槽，nemesisbot 在 gateway 装配时把
//! 实现装进槽。handler 全部经 bridge 走；未装配（headless / 独立 web 测试）
//! 时诚实报错。
//!
//! 列表视图不带 patch 体（单条 patch 可达 1MB）；`deliveries.patch` 单独
//! 回文本。confirm/reject 的审计链留痕在 nemesisbot 侧 bridge 实现里做
//! （security 依赖不进 nemesis-web）。

use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Bridge trait + 进程级槽
// ---------------------------------------------------------------------------

/// 交付条目的 web 侧投影（serde 序列化直接进 WSAPI 响应）。不带 payload
/// 原文（含 patch 体）；payload 里的结构化摘要字段（files_changed 等）
/// 由 bridge 实现展开到顶层，`patch_present`/`patch_truncated` 只报形态
/// 不报内容。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeliveryInfo {
    pub id: String,
    pub run_id: Option<String>,
    /// `branch` / `review` / `dossier`（交付种类）。
    pub kind: String,
    pub idempotency_key: String,
    /// `recorded` / `delivered` / `confirmed` / `rejected` / `published`
    /// （项 C 2026-10-22 起：记账即 recorded，通道接受才 delivered；
    /// published = P3 外循环已推远端并开出 PR——终态，PR 号在 payload）。
    pub status: String,
    pub created_at: String,
    pub delivered_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub rejected_at: Option<String>,
    pub reject_reason: Option<String>,
    // -- 通知义务维度（项 C，与 status 正交）--
    /// `pending` / `channel_accepted` / `confirmed` / `none` / `legacy` / `dead`。
    pub notify_state: String,
    /// 通知失败累计次数（重试债记账；0 = 无失败）。
    pub notify_attempts: i64,
    /// 最近一次通知失败原因（None = 无失败记录）。
    pub notify_last_error: Option<String>,
    // -- payload 展开字段（branch 交付；缺失 = null/false）--
    pub branch: Option<String>,
    pub node: Option<String>,
    pub files_changed: Option<i64>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub patch_truncated: Option<bool>,
    pub patch_path: Option<String>,
    /// payload 里有内联 patch 文本 = true（内容走 `deliveries.patch`）。
    pub patch_present: bool,
    // -- 不可变 tree 标识（G4，2026-10-18 批六；旧 payload 缺键 = null）--
    /// 交付时 head commit（`DeliveryBranchCapture.commit`）。
    pub commit: Option<String>,
    /// 交付时 head tree（payload 键 `tree`——完整快照的不可变内容标识）。
    pub head_tree: Option<String>,
    /// 执行前 base tree（diff 的锚点侧；与 head_tree 一起构成可复验的
    /// 变更区间标识）。
    pub base_tree: Option<String>,
    // -- P3 发布留痕（payload 键；未发布 = null）--
    /// PR 号（`pr_number`）。
    pub pr_number: Option<i64>,
    /// PR 链接（`pr_url`）。
    pub pr_url: Option<String>,
    /// 发布时间（`published_at`，RFC3339 本地时区）。
    pub published_at: Option<String>,
}

/// nemesisbot 侧交付账本能力的抽象（实现在 nemesisbot，真相源 tasks.db
/// deliveries 表）。全部同步——SQLite 快查无异步 IO。
pub trait DeliveriesBridge: Send + Sync {
    /// 最近 N 笔（审计/仪表查询面；稳定序）。
    fn list(&self, limit: usize) -> Vec<DeliveryInfo>;
    /// 按执行 ID 列交付（run 维证据包查询面）。
    fn by_run(&self, run_id: &str) -> Vec<DeliveryInfo>;
    /// 按幂等键查单条（不存在 = Err）。
    fn get(&self, idempotency_key: &str) -> Result<DeliveryInfo, String>;
    /// 确认交付（delivered → confirmed）。`Ok(false)` = 已确认（幂等）。
    fn confirm(&self, idempotency_key: &str) -> Result<bool, String>;
    /// 拒绝交付（任何非 rejected → rejected 终态，带原因留痕；拒已
    /// confirmed 合法 = 人工复核推翻自动验收的入口）。`Ok(false)` = 已是
    /// rejected（幂等）。
    fn reject(&self, idempotency_key: &str, reason: &str) -> Result<bool, String>;
    /// 取交付的内联 patch 文本（不存在/无内联 patch = Err 带指引）。
    fn patch(&self, idempotency_key: &str) -> Result<String, String>;
}

static DELIVERIES_BRIDGE: std::sync::RwLock<Option<Arc<dyn DeliveriesBridge>>> =
    std::sync::RwLock::new(None);

/// gateway 装配期安装 bridge（nemesisbot 侧交付账本实现）。
pub fn install_deliveries_bridge(bridge: Arc<dyn DeliveriesBridge>) {
    *DELIVERIES_BRIDGE.write().expect("deliveries bridge lock") = Some(bridge);
}

/// 取当前 bridge（clone Arc 出来，不持锁返回）。
pub fn deliveries_bridge() -> Option<Arc<dyn DeliveriesBridge>> {
    DELIVERIES_BRIDGE
        .read()
        .expect("deliveries bridge lock")
        .clone()
}

fn require_bridge() -> Result<Arc<dyn DeliveriesBridge>, String> {
    deliveries_bridge().ok_or_else(|| {
        "交付账本未装配（deliveries bridge 未接线，网关需为装配 gateway 模式）".to_string()
    })
}

#[cfg(test)]
pub(crate) fn set_deliveries_bridge_for_test(bridge: Option<Arc<dyn DeliveriesBridge>>) {
    *DELIVERIES_BRIDGE.write().expect("deliveries bridge lock") = bridge;
}

/// bridge 触碰类测试的共享串行锁（进程级槽是共享态——并行测试互相置换
/// 会 flake；同 AGENTS_BRIDGE 家族纪律）。只在 tests 里 acquire。
#[cfg(test)]
pub(crate) static DELIVERIES_BRIDGE_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ---------------------------------------------------------------------------
// P3 外循环（PR 发布）bridge——异步能力面（与查询/裁决 bridge 分槽）
// ---------------------------------------------------------------------------

/// 交付发布桥（P3 外循环，2026-10-25）：board 模块 `delivery.publish` 命令
/// 消费。发布是异步重活（connector invoke 走 agent loop dispatch 瀑布 +
/// git push + 状态机落账），实现落 nemesisbot 侧（本 crate 不能反向依赖
/// ——同 [`DeliveriesBridge`] 依赖反转模式）。请求/响应按 JSON 值透传：
/// 参数解析权在实现侧（task_id/connector/owner/repo/base/copy_path/title/
/// body），返回 `{status, pr_number, pr_url, commit, branch, detail}`。
#[async_trait::async_trait]
pub trait DeliveryPublishBridge: Send + Sync {
    async fn publish(&self, args: &serde_json::Value) -> Result<serde_json::Value, String>;
}

static DELIVERY_PUBLISH_BRIDGE: std::sync::RwLock<Option<Arc<dyn DeliveryPublishBridge>>> =
    std::sync::RwLock::new(None);

/// gateway 装配期安装发布桥（nemesisbot 侧实现；agent 重启重装同槽覆盖）。
pub fn install_delivery_publish_bridge(bridge: Arc<dyn DeliveryPublishBridge>) {
    *DELIVERY_PUBLISH_BRIDGE
        .write()
        .expect("delivery publish bridge lock") = Some(bridge);
}

/// 取当前发布桥（clone Arc 出来，不持锁返回）。缺席 = 发布面未装配
/// （headless / 独立 web 测试），调用方诚实报错。
pub fn delivery_publish_bridge() -> Option<Arc<dyn DeliveryPublishBridge>> {
    DELIVERY_PUBLISH_BRIDGE
        .read()
        .expect("delivery publish bridge lock")
        .clone()
}

// ---------------------------------------------------------------------------
// WSAPI handler
// ---------------------------------------------------------------------------

/// list 视图缺省上限（防一次拉全表；bridge 侧再兜底）。
const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 500;

pub struct DeliveriesHandler;

#[async_trait::async_trait]
impl ModuleHandler for DeliveriesHandler {
    fn module_name(&self) -> &str {
        "deliveries"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "by_run", "get", "confirm", "reject", "patch"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let bridge = require_bridge()?;
        match cmd {
            "list" => {
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(DEFAULT_LIST_LIMIT)
                    .clamp(1, MAX_LIST_LIMIT);
                let deliveries = bridge.list(limit);
                let count = deliveries.len();
                Ok(Some(
                    serde_json::json!({ "deliveries": deliveries, "count": count }),
                ))
            }
            "by_run" => {
                let data = data.ok_or("missing data")?;
                let run_id = crate::handlers::get_str(&data, "run_id")?;
                let deliveries = bridge.by_run(&run_id);
                let count = deliveries.len();
                Ok(Some(
                    serde_json::json!({ "run_id": run_id, "deliveries": deliveries, "count": count }),
                ))
            }
            "get" => {
                let data = data.ok_or("missing data")?;
                let key = crate::handlers::get_str(&data, "idempotency_key")?;
                let info = bridge.get(&key)?;
                Ok(Some(serde_json::json!({ "delivery": info })))
            }
            "confirm" => {
                let data = data.ok_or("missing data")?;
                let key = crate::handlers::get_str(&data, "idempotency_key")?;
                let confirmed = bridge.confirm(&key)?;
                Ok(Some(serde_json::json!({
                    "idempotency_key": key,
                    "confirmed": confirmed,
                    "status": "confirmed",
                    "note": if confirmed {
                        "交付已确认"
                    } else {
                        "已是 confirmed（幂等，无变化）"
                    },
                })))
            }
            "reject" => {
                let data = data.ok_or("missing data")?;
                let key = crate::handlers::get_str(&data, "idempotency_key")?;
                let reason = crate::handlers::get_str(&data, "reason").unwrap_or_default();
                let reason = if reason.trim().is_empty() {
                    "(未填写原因)"
                } else {
                    reason.as_str()
                };
                let rejected = bridge.reject(&key, reason)?;
                Ok(Some(serde_json::json!({
                    "idempotency_key": key,
                    "rejected": rejected,
                    "status": "rejected",
                    "reason": reason,
                    "note": if rejected {
                        "交付已拒绝（终态，原因留痕）"
                    } else {
                        "已是 rejected（幂等，原因不覆盖）"
                    },
                })))
            }
            "patch" => {
                let data = data.ok_or("missing data")?;
                let key = crate::handlers::get_str(&data, "idempotency_key")?;
                let patch = bridge.patch(&key)?;
                Ok(Some(
                    serde_json::json!({ "idempotency_key": key, "patch": patch }),
                ))
            }
            _ => Err(format!("unknown deliveries cmd: {cmd}")),
        }
    }
}

#[cfg(test)]
mod deliveries_ws_tests;
