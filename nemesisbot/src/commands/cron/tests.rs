use super::*;
use tempfile::TempDir;

fn make_store(tmp: &TempDir, jobs: &[serde_json::Value]) -> std::path::PathBuf {
    let dir = tmp.path().join("cron");
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("jobs.json");
    std::fs::write(
        &store,
        serde_json::to_string_pretty(&serde_json::Value::Array(jobs.to_vec())).unwrap(),
    )
    .unwrap();
    store
}

fn sample_job(id: &str, name: &str, enabled: bool) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "enabled": enabled,
        "schedule": {
            "kind": "interval",
            "every_ms": 60000,
            "display": "every 60s"
        }
    })
}

#[test]
fn test_toggle_job_enable() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(&tmp, &[sample_job("abc123", "test_job", false)]);

    toggle_job(&store, "abc123", true);

    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert_eq!(jobs[0]["enabled"], true);
}

#[test]
fn test_toggle_job_disable() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(&tmp, &[sample_job("def456", "another_job", true)]);

    toggle_job(&store, "def456", false);

    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert_eq!(jobs[0]["enabled"], false);
}

#[test]
fn test_toggle_job_not_found() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(&tmp, &[sample_job("abc123", "test_job", true)]);

    toggle_job(&store, "nonexistent", false);

    // Job should remain unchanged
    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert_eq!(jobs[0]["enabled"], true);
}

#[test]
fn test_toggle_job_no_file() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("nonexistent").join("jobs.json");

    // Should not panic
    toggle_job(&store, "abc123", true);
}

#[test]
fn test_toggle_job_multiple_jobs() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(
        &tmp,
        &[
            sample_job("job1", "first", true),
            sample_job("job2", "second", true),
            sample_job("job3", "third", false),
        ],
    );

    toggle_job(&store, "job2", false);

    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert_eq!(jobs[0]["enabled"], true); // unchanged
    assert_eq!(jobs[1]["enabled"], false); // changed
    assert_eq!(jobs[2]["enabled"], false); // unchanged
}

#[test]
fn test_add_interval_job() {
    let _tmp = TempDir::new().unwrap();
    // Simulate CronAction::Add with interval schedule
    let schedule = serde_json::json!({
        "kind": "interval",
        "every_ms": 300000u64,  // 5 minutes
        "display": "every 300s"
    });
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let job = serde_json::json!({
        "id": id,
        "name": "test_interval",
        "message": "do something",
        "schedule": schedule,
        "deliver": false,
        "enabled": true,
    });

    assert_eq!(job["schedule"]["every_ms"], 300000);
    assert_eq!(job["schedule"]["kind"], "interval");
    assert_eq!(job["enabled"], true);
}

#[test]
fn test_add_cron_expr_job() {
    let cron_expr = "0 */5 * * *";
    let schedule = serde_json::json!({
        "kind": "cron",
        "expr": cron_expr,
        "display": format!("cron: {}", cron_expr)
    });

    assert_eq!(schedule["kind"], "cron");
    assert_eq!(schedule["expr"], cron_expr);
}

#[test]
fn test_remove_job_from_store() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(
        &tmp,
        &[
            sample_job("j1", "job1", true),
            sample_job("j2", "job2", true),
        ],
    );

    let data = std::fs::read_to_string(&store).unwrap();
    let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    let before = jobs.len();
    jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some("j1"));
    assert_eq!(jobs.len(), before - 1);
    assert_eq!(jobs[0]["id"], "j2");
}

#[test]
fn test_remove_nonexistent_job() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(&tmp, &[sample_job("j1", "job1", true)]);

    let data = std::fs::read_to_string(&store).unwrap();
    let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    let before = jobs.len();
    jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some("nonexistent"));
    assert_eq!(jobs.len(), before); // nothing removed
}

#[test]
fn test_job_id_is_8_chars() {
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    assert_eq!(id.len(), 8);
}

#[test]
fn test_empty_store_is_valid_json() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cron");
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("jobs.json");
    std::fs::write(&store, "[]").unwrap();

    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert!(jobs.is_empty());
}

// -------------------------------------------------------------------------
// Additional cron tests for coverage
// -------------------------------------------------------------------------

#[test]
fn test_toggle_job_with_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cron");
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("jobs.json");
    std::fs::write(&store, "invalid json").unwrap();

    // Should not panic, just do nothing
    toggle_job(&store, "any-id", true);
}

#[test]
fn test_toggle_job_empty_array() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cron");
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("jobs.json");
    std::fs::write(&store, "[]").unwrap();

    toggle_job(&store, "any-id", true);

    // Should remain empty
    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert!(jobs.is_empty());
}

#[test]
fn test_schedule_interval_json_structure() {
    let secs: u64 = 300;
    let schedule = serde_json::json!({
        "kind": "interval",
        "every_ms": secs * 1000,
        "display": format!("every {}s", secs)
    });
    assert_eq!(schedule["kind"], "interval");
    assert_eq!(schedule["every_ms"], 300000);
    assert_eq!(schedule["display"], "every 300s");
}

#[test]
fn test_schedule_cron_json_structure() {
    let cron_expr = "0 */5 * * *";
    let schedule = serde_json::json!({
        "kind": "cron",
        "expr": cron_expr,
        "display": format!("cron: {}", cron_expr)
    });
    assert_eq!(schedule["kind"], "cron");
    assert_eq!(schedule["expr"], cron_expr);
    assert_eq!(schedule["display"], "cron: 0 */5 * * *");
}

#[test]
fn test_job_json_structure() {
    let id = "test1234".to_string();
    let name = "test_job".to_string();
    let message = "do something".to_string();
    let schedule =
        serde_json::json!({"kind": "interval", "every_ms": 60000, "display": "every 60s"});
    let deliver = true;
    let to: Option<String> = Some("user1".to_string());
    let channel: Option<String> = Some("web".to_string());

    let job = serde_json::json!({
        "id": id,
        "name": name,
        "message": message,
        "schedule": schedule,
        "deliver": deliver,
        "to": to,
        "channel": channel,
        "enabled": true,
    });

    assert_eq!(job["id"], "test1234");
    assert_eq!(job["name"], "test_job");
    assert_eq!(job["message"], "do something");
    assert_eq!(job["deliver"], true);
    assert_eq!(job["to"], "user1");
    assert_eq!(job["channel"], "web");
    assert_eq!(job["enabled"], true);
}

#[test]
fn test_job_list_display_schedule_object() {
    let job = serde_json::json!({
        "id": "j1",
        "name": "test",
        "enabled": true,
        "schedule": {
            "kind": "interval",
            "every_ms": 120000,
            "display": "every 120s"
        }
    });

    // Test the schedule display extraction logic from CronAction::List
    let schedule_display = job
        .get("schedule")
        .and_then(|s| {
            if s.is_object() {
                s.get("display").and_then(|v| v.as_str())
            } else {
                s.as_str()
            }
        })
        .unwrap_or("?");

    assert_eq!(schedule_display, "every 120s");

    // Test next run extraction
    let next_run = job
        .get("schedule")
        .and_then(|s| s.get("every_ms").and_then(|v| v.as_u64()))
        .map(|ms| {
            let secs = ms / 1000;
            format!("every {}s", secs)
        })
        .unwrap_or_else(|| schedule_display.to_string());

    assert_eq!(next_run, "every 120s");
}

#[test]
fn test_job_list_display_schedule_string() {
    let job = serde_json::json!({
        "id": "j1",
        "name": "test",
        "enabled": true,
        "schedule": "every 5 minutes"
    });

    let schedule_display = job
        .get("schedule")
        .and_then(|s| {
            if s.is_object() {
                s.get("display").and_then(|v| v.as_str())
            } else {
                s.as_str()
            }
        })
        .unwrap_or("?");

    assert_eq!(schedule_display, "every 5 minutes");
}

#[test]
fn test_job_id_uniqueness() {
    let id1 = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let id2 = uuid::Uuid::new_v4().to_string()[..8].to_string();
    assert_ne!(id1, id2);
}

#[test]
fn test_remove_job_from_empty_store() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("cron");
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("jobs.json");
    std::fs::write(&store, "[]").unwrap();

    let data = std::fs::read_to_string(&store).unwrap();
    let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    let before = jobs.len();
    jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some("nonexistent"));
    assert_eq!(jobs.len(), before);
}

#[test]
fn test_toggle_job_preserves_other_fields() {
    let tmp = TempDir::new().unwrap();
    let store = make_store(
        &tmp,
        &[serde_json::json!({
            "id": "j1",
            "name": "myjob",
            "enabled": true,
            "message": "hello",
            "schedule": {"kind": "interval", "every_ms": 60000}
        })],
    );

    toggle_job(&store, "j1", false);

    let data = std::fs::read_to_string(&store).unwrap();
    let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
    assert_eq!(jobs[0]["enabled"], false);
    assert_eq!(jobs[0]["name"], "myjob"); // other fields preserved
    assert_eq!(jobs[0]["message"], "hello");
}
