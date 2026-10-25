//! P4-c（2026-10-17）— A 端交付账本 web 面（[`nemesis_web::handlers::deliveries`]
//! bridge 实现）。
//!
//! 真相源 = tasks.db `deliveries` 表；本模块只做两件事：
//! - **投影**：`DeliveryRecord` → web 侧 `DeliveryInfo`（payload 结构化
//!   摘要展开到顶层，patch 体不进列表视图）。
//! - **裁决审计**：`confirm`/`reject` 走 `append_direct_audit_event` 留痕
//!   （F8 先例；security 裁剪形态无审计链，仅 DB 状态转移 + tracing）。
//!
//! 审计槽与 cluster_init.rs 回调同款惰性取法：本模块在 post_agent 装配时
//! security 插件槽可能还空着（init_agent 之后才 set），闭包持有槽 Arc、
//! 执行时 `get()` 惰性解析。

use std::sync::Arc;

use nemesis_taskstore::TaskDb;
use nemesis_taskstore::deliveries::DeliveryRecord;
use nemesis_web::handlers::deliveries::{DeliveriesBridge, DeliveryInfo};

/// A 端交付账本 bridge 实现（clone 廉价；与任务真相源共享同一 `Arc<TaskDb>`）。
pub(crate) struct DeliveryLedgerBridge {
    db: Arc<TaskDb>,
    /// 审计插件槽（cfg security）：None = 无槽可取（headless 形态）；
    /// Some 槽内 None = 审计链未启用（tracing 兜底）。
    #[cfg(feature = "security")]
    security_slot:
        Option<Arc<std::sync::OnceLock<Option<Arc<nemesis_security::pipeline::SecurityPlugin>>>>>,
}

impl DeliveryLedgerBridge {
    /// gateway post_agent 装配（幂等覆盖——agent 重启重建走同一函数重装）。
    pub(crate) fn install(
        db: Arc<TaskDb>,
        #[cfg(feature = "security")] security_slot: Option<
            Arc<std::sync::OnceLock<Option<Arc<nemesis_security::pipeline::SecurityPlugin>>>>,
        >,
    ) {
        nemesis_web::handlers::deliveries::install_deliveries_bridge(Arc::new(Self {
            db,
            #[cfg(feature = "security")]
            security_slot,
        }));
    }

    /// 审计插件惰性解析（槽空/审计链关 = None）。
    #[cfg(feature = "security")]
    fn plugin(&self) -> Option<&Arc<nemesis_security::pipeline::SecurityPlugin>> {
        self.security_slot
            .as_ref()
            .and_then(|slot| slot.get())
            .and_then(|p| p.as_ref())
    }

    /// 裁决面审计留痕（operation = delivery_confirm / delivery_reject；
    /// 审计链未启用 = tracing debug 兜底可见）。
    #[cfg(feature = "security")]
    fn audit(&self, operation: &str, target: &str, decision: &str, reason: &str) {
        if let Some(plugin) = self.plugin() {
            plugin.append_direct_audit_event(
                operation,
                "delivery_ledger",
                "system",
                "wsapi",
                target,
                decision,
                reason,
            );
        } else {
            tracing::debug!(
                operation = %operation,
                target = %target,
                "[Deliveries] 审计链未启用，裁决仅 tracing 可见: {decision} {reason}"
            );
        }
    }

    /// DeliveryRecord → web 投影（payload 结构化摘要展开；patch 体不进视图）。
    fn project(rec: &DeliveryRecord) -> DeliveryInfo {
        let patch_present = rec.payload.get("patch").is_some_and(|v| v.is_string());
        DeliveryInfo {
            id: rec.id.clone(),
            run_id: rec.run_id.clone(),
            kind: rec.kind.clone(),
            idempotency_key: rec.idempotency_key.clone(),
            status: rec.status.clone(),
            created_at: rec.created_at.clone(),
            delivered_at: rec.delivered_at.clone(),
            confirmed_at: rec.confirmed_at.clone(),
            rejected_at: rec.rejected_at.clone(),
            reject_reason: rec.reject_reason.clone(),
            notify_state: rec.notify_state.clone(),
            notify_attempts: rec.notify_attempts,
            notify_last_error: rec.notify_last_error.clone(),
            branch: str_field(&rec.payload, "branch"),
            node: str_field(&rec.payload, "node"),
            files_changed: num_field(&rec.payload, "files_changed"),
            insertions: num_field(&rec.payload, "insertions"),
            deletions: num_field(&rec.payload, "deletions"),
            patch_truncated: rec.payload.get("patch_truncated").and_then(|v| v.as_bool()),
            patch_path: str_field(&rec.payload, "patch_path"),
            patch_present,
            // 不可变 tree 标识（G4）：worker 交付 payload 自带（capture 时
            // 写入），旧格式 payload 缺键 = null 诚实降级。
            commit: str_field(&rec.payload, "commit"),
            head_tree: str_field(&rec.payload, "tree"),
            base_tree: str_field(&rec.payload, "base_tree"),
            // P3 发布留痕：发布落账后 payload 注入的 PR 号/链接/时间。
            pr_number: num_field(&rec.payload, "pr_number"),
            pr_url: str_field(&rec.payload, "pr_url"),
            published_at: str_field(&rec.payload, "published_at"),
        }
    }
}

fn str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn num_field(payload: &serde_json::Value, key: &str) -> Option<i64> {
    payload
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
}

impl DeliveriesBridge for DeliveryLedgerBridge {
    fn list(&self, limit: usize) -> Vec<DeliveryInfo> {
        match self.db.deliveries_recent(limit) {
            Ok(rows) => rows.iter().map(Self::project).collect(),
            Err(e) => {
                tracing::warn!("[Deliveries] recent 查询失败: {e}");
                Vec::new()
            }
        }
    }

    fn by_run(&self, run_id: &str) -> Vec<DeliveryInfo> {
        match self.db.deliveries_by_run(run_id) {
            Ok(rows) => rows.iter().map(Self::project).collect(),
            Err(e) => {
                tracing::warn!(run_id = %run_id, "[Deliveries] by_run 查询失败: {e}");
                Vec::new()
            }
        }
    }

    fn get(&self, idempotency_key: &str) -> Result<DeliveryInfo, String> {
        self.db
            .delivery_by_key(idempotency_key)?
            .map(|rec| Self::project(&rec))
            .ok_or_else(|| format!("交付不存在: {idempotency_key}"))
    }

    fn confirm(&self, idempotency_key: &str) -> Result<bool, String> {
        // 先查存在性（确认不存在的键 = 诚实报错，不是幂等成功）。
        self.get(idempotency_key)?;
        let confirmed = self.db.confirm_delivery(idempotency_key)?;
        #[cfg(feature = "security")]
        self.audit(
            "delivery_confirm",
            idempotency_key,
            "allow",
            if confirmed {
                "delivery confirmed via wsapi"
            } else {
                "delivery already confirmed (idempotent, no change)"
            },
        );
        Ok(confirmed)
    }

    fn reject(&self, idempotency_key: &str, reason: &str) -> Result<bool, String> {
        self.get(idempotency_key)?;
        let rejected = self.db.reject_delivery(idempotency_key, reason)?;
        #[cfg(feature = "security")]
        self.audit(
            "delivery_reject",
            idempotency_key,
            "allow",
            if rejected {
                reason
            } else {
                "delivery already rejected (idempotent, reason not overwritten)"
            },
        );
        Ok(rejected)
    }

    fn patch(&self, idempotency_key: &str) -> Result<String, String> {
        let rec = self
            .db
            .delivery_by_key(idempotency_key)?
            .ok_or_else(|| format!("交付不存在: {idempotency_key}"))?;
        if let Some(patch) = rec.payload.get("patch").and_then(|v| v.as_str()) {
            return Ok(patch.to_string());
        }
        let hint = str_field(&rec.payload, "patch_path")
            .map(|p| format!("完整 patch 在 worker 侧留底: {p}"))
            .unwrap_or_else(|| "该交付无 patch 留底（可能截断或旧格式）".to_string());
        Err(format!("交付 payload 无内联 patch 文本。{hint}"))
    }
}
