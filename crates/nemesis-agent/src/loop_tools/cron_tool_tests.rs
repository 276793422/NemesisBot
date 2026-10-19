//! CronTool schema↔impl 一致性回归（2026-10-19）。
//!
//! 根因背景：schema 曾声明 at_seconds/every_seconds/cron_expr/message，而
//! execute() 只认 `schedule` 字符串 + `content` —— 按 schema 调用必然失败
//!（"Missing 'schedule' argument"）。修复后 schema 形态是一等公民，旧
//! schedule 字符串保留为向后兼容回退。本文件钉住 schema 形态不再回退，
//! 并钉住 schema 本身不再声明无消费的死参（command/deliver）。

use super::*;
use crate::context::RequestContext;
use std::sync::Arc;

fn fresh_tool(
    tag: &str,
) -> (
    CronTool,
    Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{}.json", tag));
    let svc = Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(&path.to_string_lossy()),
    ));
    (CronTool::new(Arc::clone(&svc)), svc)
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-cron", "u", "agent:convo-cron")
}

async fn one_job(
    svc: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
) -> nemesis_cron::service::CronJob {
    let jobs = svc.lock().unwrap().list_jobs(true);
    assert_eq!(jobs.len(), 1, "exactly one job expected");
    jobs[0].clone()
}

// --- schema 形态 create：三个结构化字段各一则 ---

#[tokio::test]
async fn cron_schema_shape_create_via_every_seconds() {
    let (tool, svc) = fresh_tool("cron-every");
    let out = tool
        .execute(
            &serde_json::json!({
                "action": "create",
                "name": "heartbeat",
                "every_seconds": 300,
                "message": "ping"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("heartbeat"), "out: {out}");
    let job = one_job(&svc).await;
    assert_eq!(job.schedule.kind, "every");
    assert_eq!(job.schedule.every_ms, Some(300_000));
    assert_eq!(job.payload.message, "ping");
}

#[tokio::test]
async fn cron_schema_shape_create_via_at_seconds() {
    let (tool, svc) = fresh_tool("cron-at");
    let before = chrono::Local::now().timestamp_millis();
    tool.execute(
        &serde_json::json!({
            "action": "create",
            "name": "once",
            "at_seconds": 3600,
            "message": "remind me"
        })
        .to_string(),
        &ctx(),
    )
    .await
    .unwrap();
    let job = one_job(&svc).await;
    assert_eq!(job.schedule.kind, "at");
    let at_ms = job.schedule.at_ms.expect("at_ms set");
    assert!(
        at_ms >= before + 3_600_000,
        "at_ms {at_ms} should be >= now+1h ({before})"
    );
    assert_eq!(job.payload.message, "remind me");
}

#[tokio::test]
async fn cron_schema_shape_create_via_cron_expr() {
    let (tool, svc) = fresh_tool("cron-expr");
    tool.execute(
        &serde_json::json!({
            "action": "create",
            "name": "daily",
            "cron_expr": "0 9 * * *",
            "message": "standup"
        })
        .to_string(),
        &ctx(),
    )
    .await
    .unwrap();
    let job = one_job(&svc).await;
    assert_eq!(job.schedule.kind, "cron");
    assert_eq!(job.schedule.expr.as_deref(), Some("0 9 * * *"));
    assert_eq!(job.payload.message, "standup");
}

// --- message / content 双读 ---

#[tokio::test]
async fn cron_create_message_wins_over_content_alias() {
    let (tool, svc) = fresh_tool("cron-both");
    tool.execute(
        &serde_json::json!({
            "action": "create",
            "every_seconds": 60,
            "message": "declared",
            "content": "legacy"
        })
        .to_string(),
        &ctx(),
    )
    .await
    .unwrap();
    assert_eq!(one_job(&svc).await.payload.message, "declared");
}

#[tokio::test]
async fn cron_create_content_alias_still_works() {
    let (tool, svc) = fresh_tool("cron-content");
    tool.execute(
        &serde_json::json!({
            "action": "create",
            "every_seconds": 60,
            "content": "legacy-only"
        })
        .to_string(),
        &ctx(),
    )
    .await
    .unwrap();
    assert_eq!(one_job(&svc).await.payload.message, "legacy-only");
}

// --- 诚实错误面 ---

#[tokio::test]
async fn cron_create_without_schedule_fields_errors_honestly() {
    let (tool, _svc) = fresh_tool("cron-nosched");
    let err = tool
        .execute(
            &serde_json::json!({"action": "create", "name": "x", "message": "hi"}).to_string(),
            &ctx(),
        )
        .await
        .unwrap_err(); // no schedule info must error
    assert!(
        err.contains("at_seconds") && err.contains("every_seconds") && err.contains("cron_expr"),
        "error should name the three schedule fields, got: {err}"
    );
}

#[tokio::test]
async fn cron_create_oversized_seconds_errors_honestly() {
    let (tool, _svc) = fresh_tool("cron-huge");
    let err = tool
        .execute(
            &serde_json::json!({
                "action": "create",
                "every_seconds": 18446744073709551615u64,
                "message": "hi"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap_err(); // overflow seconds must error
    assert!(err.contains("too large"), "got: {err}");
}

#[tokio::test]
async fn cron_create_legacy_schedule_negative_interval_rejected() {
    // legacy 回退臂与新结构化路径同源拒绝负数（i64 parse 曾放行
    // "every:-5s" → 负 every_ms 直通 CronService）。
    let (tool, _svc) = fresh_tool("cron-legacy-neg");
    let err = tool
        .execute(
            &serde_json::json!({
                "action": "create",
                "schedule": "every:-5s",
                "message": "hi"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Invalid interval"), "got: {err}");
}

#[tokio::test]
async fn cron_create_legacy_schedule_oversized_interval_rejected() {
    // legacy 回退臂溢出诚实拒绝（裸 secs*1000 曾在 debug 构建溢出 panic）。
    let (tool, _svc) = fresh_tool("cron-legacy-huge");
    let err = tool
        .execute(
            &serde_json::json!({
                "action": "create",
                "schedule": "every:9223372036854775807s",
                "message": "hi"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("too large"), "got: {err}");
}

// --- delete：schema 声明的 id 全链 ---

#[tokio::test]
async fn cron_delete_via_declared_id_schema_shape() {
    let (tool, svc) = fresh_tool("cron-del");
    tool.execute(
        &serde_json::json!({"action": "create", "every_seconds": 60, "message": "m"}).to_string(),
        &ctx(),
    )
    .await
    .unwrap();
    let id = one_job(&svc).await.id;

    let out = tool
        .execute(
            &serde_json::json!({"action": "delete", "id": id}).to_string(),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("Deleted"), "out: {out}");
    assert!(svc.lock().unwrap().list_jobs(true).is_empty());

    let err = tool
        .execute(
            &serde_json::json!({"action": "delete", "id": id}).to_string(),
            &ctx(),
        )
        .await
        .unwrap_err(); // second delete must fail
    assert!(err.contains("Job not found"), "got: {err}");
}

// --- schema 本身钉死：死参不再声明，真参齐全 ---

#[test]
fn cron_schema_declares_real_interface() {
    let (tool, _svc) = fresh_tool("cron-schema");
    let p = tool.parameters();
    let props = &p["properties"];
    for declared in [
        "action",
        "name",
        "at_seconds",
        "every_seconds",
        "cron_expr",
        "message",
        "continue_session",
        "max_rounds",
        "id",
    ] {
        assert!(
            props[declared].is_object(),
            "schema must declare {declared}"
        );
    }
    for dead in ["command", "deliver", "schedule"] {
        assert!(
            props[dead].is_null(),
            "schema must NOT declare dead param {dead}"
        );
    }
    let d = tool.description();
    assert!(
        d.contains("at_seconds") && d.contains("message"),
        "desc: {d}"
    );
}
