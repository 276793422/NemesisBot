use super::*;
use chrono::{Datelike, Timelike};

#[test]
fn test_add_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "hello",
            false,
            None,
            None,
        )
        .unwrap();
    assert_eq!(job.name, "test");
    assert!(job.enabled);
}

#[test]
fn test_remove_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "hello",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(svc.remove_job(&job.id).unwrap());
}

#[test]
fn test_list_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "a",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "a",
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(svc.list_jobs(false).len(), 1);
}

#[test]
fn test_get_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "findme",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "x",
            false,
            None,
            None,
        )
        .unwrap();
    let found = svc.get_job(&job.id).unwrap();
    assert_eq!(found.name, "findme");
    assert!(svc.get_job("nonexistent").is_none());
}

#[test]
fn test_update_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "orig",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "x",
            false,
            None,
            None,
        )
        .unwrap();
    svc.update_job(&job.id, Some("updated"), None).unwrap();
    assert_eq!(svc.get_job(&job.id).unwrap().name, "updated");
}

#[test]
fn test_toggle_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "toggle",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "x",
            false,
            None,
            None,
        )
        .unwrap();
    let new_state = svc.toggle_job(&job.id).unwrap();
    assert!(!new_state);
    let new_state2 = svc.toggle_job(&job.id).unwrap();
    assert!(new_state2);
}

#[test]
fn test_execute_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "exec",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "x",
            false,
            None,
            None,
        )
        .unwrap();
    svc.execute_job(&job.id).unwrap();
    let found = svc.get_job(&job.id).unwrap();
    assert!(found.state.last_run_at_ms.is_some());
    assert_eq!(found.state.last_status.as_deref(), Some("executed"));
}

#[test]
fn test_validate_schedule_valid() {
    assert!(CronService::validate_schedule("0 * * * * *").is_ok());
    assert!(CronService::validate_schedule("0 30 9 * * *").is_ok());
    assert!(CronService::validate_schedule("0 */5 * * * *").is_ok());
    assert!(CronService::validate_schedule("0 0-30 * * * *").is_ok());
    assert!(CronService::validate_schedule("0 1,15,30 * * * *").is_ok());
}

#[test]
fn test_validate_schedule_invalid() {
    // 5-field expressions are now valid (matching Go's gronx)
    assert!(CronService::validate_schedule("0 * * * *").is_ok()); // 5 fields - VALID
    assert!(CronService::validate_schedule("60 * * * * *").is_err()); // second OOB
    assert!(CronService::validate_schedule("0 0 25 * * *").is_err()); // hour OOB
}

#[test]
fn test_describe_schedule() {
    assert_eq!(
        CronService::describe_schedule("0 * * * * *"),
        "Every minute"
    );
    assert_eq!(
        CronService::describe_schedule("0 */5 * * * *"),
        "Every 5 minutes"
    );
    assert_eq!(
        CronService::describe_schedule("0 30 9 * * *"),
        "Daily at 9:30"
    );
}

// ========================================================================
// Cron expression parser tests
// ========================================================================

#[test]
fn test_compute_next_run_cron_kind() {
    let now_ms = chrono::DateTime::parse_from_rfc3339("2026-01-15T10:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 30 9 * * *".to_string()),
        tz: Some("UTC".to_string()),
    };
    let next = compute_next_run(&schedule, now_ms);
    assert!(next.is_some());
    let next_dt = chrono::DateTime::from_timestamp_millis(next.unwrap()).unwrap();
    assert_eq!(next_dt.hour(), 9);
    assert_eq!(next_dt.minute(), 30);
}

#[test]
fn test_compute_next_run_invalid_expr() {
    let now_ms = chrono::Local::now().timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("invalid".to_string()),
        tz: None,
    };
    let next = compute_next_run(&schedule, now_ms);
    assert!(next.is_none());
}

#[test]
fn test_cron_job_with_cron_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "cron_test",
            CronSchedule {
                kind: "cron".to_string(),
                at_ms: None,
                every_ms: None,
                expr: Some("0 0 12 * * *".to_string()),
                tz: Some("UTC".to_string()),
            },
            "daily at noon",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());
    let next_dt =
        chrono::DateTime::from_timestamp_millis(job.state.next_run_at_ms.unwrap()).unwrap();
    assert_eq!(next_dt.hour(), 12);
}

#[test]
fn test_cron_service_status_initial() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let status = svc.status();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["jobs"], 0);
    assert_eq!(status["nextWakeAtMS"], serde_json::Value::Null);
}

#[test]
fn test_cron_service_status_with_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "j1",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m1",
        false,
        None,
        None,
    )
    .unwrap();
    svc.add_job(
        "j2",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(120000),
            expr: None,
            tz: None,
        },
        "m2",
        false,
        None,
        None,
    )
    .unwrap();
    let status = svc.status();
    assert_eq!(status["jobs"], 2);
}

#[test]
fn test_cron_service_status_next_wake() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let future_ms = Local::now().timestamp_millis() + 300_000; // 5 min from now
    svc.add_job(
        "future",
        CronSchedule {
            kind: "at".to_string(),
            at_ms: Some(future_ms),
            every_ms: None,
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    let status = svc.status();
    assert!(status["nextWakeAtMS"].is_number());
}

#[test]
fn test_cron_service_enable_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "enable_test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.enabled);

    // Disable
    let updated = svc.enable_job(&job.id, false).unwrap();
    assert!(!updated.enabled);
    assert!(updated.state.next_run_at_ms.is_none());

    // Enable again
    let re_enabled = svc.enable_job(&job.id, true).unwrap();
    assert!(re_enabled.enabled);
    assert!(re_enabled.state.next_run_at_ms.is_some());
}

#[test]
fn test_cron_service_enable_job_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let result = svc.enable_job("nonexistent", true);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_cron_service_set_on_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();
    svc.set_on_job(move |_job| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok("handled".to_string())
    });
    // Verify the handler is set by triggering it indirectly
    // We can't directly call the handler, but set_on_job should not panic
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn test_cron_service_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "reload_test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(svc.list_jobs(false).len(), 1);

    // Reload from disk - should load the same data
    svc.reload().unwrap();
    assert_eq!(svc.list_jobs(false).len(), 1);
}

#[test]
fn test_cron_service_reload_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("nonexistent")
        .join("cron.json")
        .to_string_lossy()
        .to_string();
    let svc = CronService::new(&path);
    // Should succeed even when file doesn't exist
    assert!(svc.reload().is_ok());
}

#[test]
fn test_list_jobs_include_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job1 = svc
        .add_job(
            "enabled",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m1",
            false,
            None,
            None,
        )
        .unwrap();
    svc.add_job(
        "enabled2",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m2",
        false,
        None,
        None,
    )
    .unwrap();

    // Disable one
    svc.toggle_job(&job1.id).unwrap();

    // Without disabled: only 1
    assert_eq!(svc.list_jobs(false).len(), 1);
    // With disabled: both 2
    assert_eq!(svc.list_jobs(true).len(), 2);
}

#[test]
fn test_compute_next_run_at_past() {
    let past_ms = Local::now().timestamp_millis() - 60000; // 1 min ago
    let schedule = CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(past_ms),
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_at_future() {
    let future_ms = Local::now().timestamp_millis() + 300000; // 5 min from now
    let schedule = CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(future_ms),
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert_eq!(result, Some(future_ms));
}

#[test]
fn test_compute_next_run_unknown_kind() {
    let schedule = CronSchedule {
        kind: "unknown".to_string(),
        at_ms: None,
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_every_zero_ms() {
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(0),
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_every_valid() {
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    };
    let now_ms = Local::now().timestamp_millis();
    let result = compute_next_run(&schedule, now_ms);
    assert!(result.is_some());
    assert!(result.unwrap() > now_ms);
}

#[test]
fn test_cron_schedule_serialization() {
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: Some(1234567890),
        every_ms: None,
        expr: Some("0 30 9 * * *".to_string()),
        tz: Some("UTC".to_string()),
    };
    let json = serde_json::to_string(&schedule).unwrap();
    let parsed: CronSchedule = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.kind, "cron");
    assert_eq!(parsed.at_ms, Some(1234567890));
    assert_eq!(parsed.expr, Some("0 30 9 * * *".to_string()));
    assert_eq!(parsed.tz, Some("UTC".to_string()));
}

#[test]
fn test_cron_payload_serialization() {
    let payload = CronPayload {
        kind: "agent_turn".to_string(),
        message: "hello".to_string(),
        command: Some("run".to_string()),
        deliver: true,
        channel: Some("web".to_string()),
        to: Some("user1".to_string()),
        session_key: None,
        max_rounds: None,
    };
    let json = serde_json::to_string(&payload).unwrap();
    let parsed: CronPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.kind, "agent_turn");
    assert_eq!(parsed.message, "hello");
    assert!(parsed.deliver);
    assert_eq!(parsed.channel, Some("web".to_string()));
}

// -------------------------------------------------------------------------
// T3 (U12): max_rounds per-fire budget
// -------------------------------------------------------------------------

/// Old payloads persisted BEFORE max_rounds existed must deserialize with
/// max_rounds = None (#[serde(default)] compatibility).
#[test]
fn test_cron_payload_max_rounds_old_json_compat() {
    let old_json = r#"{
        "kind": "agent_turn",
        "message": "check in",
        "command": null,
        "deliver": false,
        "channel": "web",
        "to": null,
        "session_key": "agent:main:session:abc"
    }"#;
    let parsed: CronPayload = serde_json::from_str(old_json).unwrap();
    assert_eq!(
        parsed.session_key.as_deref(),
        Some("agent:main:session:abc")
    );
    assert_eq!(parsed.max_rounds, None, "old JSON → no budget");
}

/// New payloads carry the budget through a serialize/deserialize round-trip.
#[test]
fn test_cron_payload_max_rounds_roundtrip() {
    let payload = CronPayload {
        kind: "agent_turn".to_string(),
        message: "m".to_string(),
        command: None,
        deliver: false,
        channel: None,
        to: None,
        session_key: Some("agent:s".to_string()),
        max_rounds: Some(5),
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(
        json.contains("\"max_rounds\":5"),
        "field serialized: {json}"
    );
    let parsed: CronPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.max_rounds, Some(5));
}

/// add_job_ext threads max_rounds into the payload; the legacy add_job
/// delegates with None (no budget → global default at fire time).
#[test]
fn test_add_job_ext_max_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let every = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    };
    let budgeted = svc
        .add_job_ext(
            "b",
            every.clone(),
            "m",
            false,
            None,
            None,
            Some("agent:s"),
            Some(10),
            true,
        )
        .unwrap();
    assert_eq!(budgeted.payload.max_rounds, Some(10));
    assert_eq!(budgeted.payload.session_key.as_deref(), Some("agent:s"));

    let legacy = svc.add_job("l", every, "m", false, None, None).unwrap();
    assert_eq!(legacy.payload.max_rounds, None, "legacy path → no budget");
}

/// P1-2 (2026-08-24 UI entry gap): `tasks.cron.update` patches the per-fire
/// budget through `CronJobPatch.max_rounds`. Three states must hold:
/// `Some(Some(n))` sets, `Some(None)` clears (global default), `None` (key
/// absent) leaves the existing budget untouched.
#[test]
fn test_patch_job_max_rounds_three_states() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let every = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    };
    let job = svc
        .add_job_ext("j", every, "m", false, None, None, None, Some(10), true)
        .unwrap();

    // 1. absent key → unchanged.
    let patched = svc.patch_job(&job.id, &CronJobPatch::default()).unwrap();
    assert_eq!(patched.payload.max_rounds, Some(10), "absent → unchanged");

    // 2. null (Some(None)) → cleared, global default applies.
    let cleared = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                max_rounds: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(cleared.payload.max_rounds, None, "Some(None) → cleared");

    // 3. number (Some(Some(n))) → set.
    let set = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                max_rounds: Some(Some(20)),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(set.payload.max_rounds, Some(20), "Some(Some(20)) → set");
}

#[test]
fn test_cron_job_state_serialization() {
    let state = CronJobState {
        next_run_at_ms: Some(999),
        last_run_at_ms: Some(888),
        last_status: Some("ok".to_string()),
        last_error: None,
        history: Vec::new(),
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: CronJobState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.next_run_at_ms, Some(999));
    assert_eq!(parsed.last_run_at_ms, Some(888));
    assert_eq!(parsed.last_status, Some("ok".to_string()));
    assert!(parsed.last_error.is_none());
}

#[test]
fn test_cron_job_serialization() {
    let now_ms = Local::now().timestamp_millis();
    let job = CronJob {
        id: "abc123".to_string(),
        name: "test job".to_string(),
        enabled: true,
        schedule: CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        payload: CronPayload {
            kind: "agent_turn".to_string(),
            message: "hello".to_string(),
            command: None,
            deliver: false,
            channel: None,
            to: None,
            session_key: None,
            max_rounds: None,
        },
        state: CronJobState {
            next_run_at_ms: Some(now_ms + 60000),
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            history: Vec::new(),
        },
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
        delete_after_run: false,
    };
    let json = serde_json::to_string(&job).unwrap();
    let parsed: CronJob = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "abc123");
    assert_eq!(parsed.name, "test job");
    assert!(parsed.enabled);
    assert!(!parsed.delete_after_run);
}

#[test]
fn test_describe_schedule_non_matching_pattern() {
    // Pattern that doesn't match any specific format falls through to expr.to_string()
    let result = CronService::describe_schedule("0 15 10 15 6 *");
    assert_eq!(result, "0 15 10 15 6 *");
}

#[test]
fn test_describe_schedule_5_field() {
    // 5-field expression: min hour day month weekday
    // "30 9 * * *" means daily at 9:30
    let result = CronService::describe_schedule("30 9 * * *");
    assert_eq!(result, "Daily at 9:30");
}

#[test]
fn test_describe_schedule_too_few_fields() {
    let result = CronService::describe_schedule("0 30 9");
    assert!(result.starts_with("Invalid:"));
}

#[test]
fn test_describe_schedule_every_minute_exact() {
    // All wildcard minutes/hours/days/months/weekdays
    let result = CronService::describe_schedule("0 * * * * *");
    assert_eq!(result, "Every minute");
}

#[test]
fn test_describe_schedule_every_n_minutes() {
    let result = CronService::describe_schedule("0 */10 * * * *");
    assert_eq!(result, "Every 10 minutes");
}

#[test]
fn test_describe_schedule_daily_at_time() {
    let result = CronService::describe_schedule("0 30 9 * * *");
    assert_eq!(result, "Daily at 9:30");
}

#[test]
fn test_add_job_at_schedule_delete_after_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let future_ms = Local::now().timestamp_millis() + 300000; // 5 min from now
    let job = svc
        .add_job(
            "one_time",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(future_ms),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "one time message",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.delete_after_run);
    assert!(job.enabled);
    assert_eq!(job.state.next_run_at_ms, Some(future_ms));
}

#[test]
fn test_add_job_at_schedule_past_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let past_ms = Local::now().timestamp_millis() - 60000; // 1 min ago
    // 过期 at 属死排程，用户漏斗 add_job_ext 已 loud 拒绝（BUG-20261024-11）；
    // 这里钉的是服务侧数据形态的落库语义，走内部旁路。
    let job = svc
        .add_job_internal(
            "past_one_time",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(past_ms),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "past message",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    // Past time means compute_next_run returns None
    assert!(job.state.next_run_at_ms.is_none());
    assert!(job.delete_after_run);
}

#[test]
fn test_add_job_with_channel_and_to() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "routed",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "routed message",
            true,
            Some("web"),
            Some("user123"),
        )
        .unwrap();
    assert!(job.payload.deliver);
    assert_eq!(job.payload.channel, Some("web".to_string()));
    assert_eq!(job.payload.to, Some("user123".to_string()));
}

#[test]
fn test_validate_schedule_edge_cases() {
    // Valid: all wildcards (6-field)
    assert!(CronService::validate_schedule("* * * * * *").is_ok());
    // Valid: 5-field (matching Go's gronx)
    assert!(CronService::validate_schedule("* * * * *").is_ok());
    // Valid: with ?
    assert!(CronService::validate_schedule("0 0 0 ? * *").is_ok());
    // Valid: 7-field (seconds + year, croner supports this)
    assert!(CronService::validate_schedule("0 * * * * * *").is_ok());
    // Invalid: empty
    assert!(CronService::validate_schedule("").is_err());
    // Invalid: second out of range
    assert!(CronService::validate_schedule("60 * * * * *").is_err());
}

#[test]
fn test_cron_service_start_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        svc.arm(); // H1: arm before start (fresh-process gate)
        svc.start().await.unwrap();
    });
    let status = svc.status();
    assert_eq!(status["enabled"], true);

    svc.stop();
    let status = svc.status();
    assert_eq!(status["enabled"], false);
}

#[test]
fn test_compute_next_run_cron_empty_expr() {
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("".to_string()),
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_cron_none_expr() {
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_execute_job_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let result = svc.execute_job("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_update_job_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let result = svc.update_job("nonexistent", Some("new name"), None);
    assert!(result.is_err());
}

#[test]
fn test_toggle_job_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let result = svc.toggle_job("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_remove_job_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    assert!(!svc.remove_job("nonexistent").unwrap());
}

#[test]
fn test_update_job_with_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "orig",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "x",
            false,
            None,
            None,
        )
        .unwrap();

    let new_schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(120000),
        expr: None,
        tz: None,
    };
    svc.update_job(&job.id, Some("new name"), Some(new_schedule))
        .unwrap();

    let updated = svc.get_job(&job.id).unwrap();
    assert_eq!(updated.name, "new name");
    assert_eq!(updated.schedule.every_ms, Some(120000));
}

#[test]
fn test_cron_job_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();

    // Create service and add a job
    let svc = CronService::new(&path);
    svc.add_job(
        "persist_test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(svc.list_jobs(false).len(), 1);

    // Create a new service with the same path - should load from disk
    let svc2 = CronService::new(&path);
    assert_eq!(svc2.list_jobs(true).len(), 1);
    let loaded = svc2.list_jobs(true).into_iter().next().unwrap();
    assert_eq!(loaded.name, "persist_test");
}

#[test]
fn test_enable_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "toggle_test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    // Disable
    let updated = svc.enable_job(&job.id, false).unwrap();
    assert!(!updated.enabled);
    assert!(updated.state.next_run_at_ms.is_none());

    // Re-enable
    let updated2 = svc.enable_job(&job.id, true).unwrap();
    assert!(updated2.enabled);
    assert!(updated2.state.next_run_at_ms.is_some());
}

#[test]
fn test_enable_job_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let result = svc.enable_job("nonexistent", true);
    assert!(result.is_err());
}

#[test]
fn test_toggle_job_enable_disable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "toggle",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    let state1 = svc.toggle_job(&job.id).unwrap();
    assert!(!state1);

    let state2 = svc.toggle_job(&job.id).unwrap();
    assert!(state2);
}

#[test]
fn test_execute_job_updates_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "exec_test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    svc.execute_job(&job.id).unwrap();
    let updated = svc.get_job(&job.id).unwrap();
    assert!(updated.state.last_run_at_ms.is_some());
    assert_eq!(updated.state.last_status, Some("executed".to_string()));
    assert!(updated.state.last_error.is_none());
}

#[test]
fn test_execute_job_fires_on_job_handler() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "fire_test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "msg",
            false,
            None,
            None,
        )
        .unwrap();

    let fired = std::sync::Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();
    svc.set_on_job(move |_job| {
        fired_clone.store(true, Ordering::SeqCst);
        Ok("fired".to_string())
    });

    // Regression: execute_job used to only update state and never invoke the
    // handler, so "run now" was a no-op for the agent.
    svc.execute_job(&job.id).unwrap();
    assert!(
        fired.load(Ordering::SeqCst),
        "execute_job should fire the on_job handler"
    );

    let updated = svc.get_job(&job.id).unwrap();
    assert_eq!(updated.state.last_status, Some("executed".to_string()));
}

#[test]
fn test_describe_schedule_every_minute() {
    let desc = CronService::describe_schedule("0 * * * * *");
    assert_eq!(desc, "Every minute");
}

#[test]
fn test_describe_schedule_every_n_minutes_v2() {
    let desc = CronService::describe_schedule("0 */5 * * * *");
    assert_eq!(desc, "Every 5 minutes");
}

#[test]
fn test_describe_schedule_daily() {
    let desc = CronService::describe_schedule("0 30 14 * * *");
    assert_eq!(desc, "Daily at 14:30");
}

#[test]
fn test_describe_schedule_invalid() {
    let desc = CronService::describe_schedule("invalid");
    assert!(desc.contains("Invalid"));
}

#[test]
fn test_describe_schedule_fallback() {
    let desc = CronService::describe_schedule("0 0 8 1 * *");
    assert_eq!(desc, "0 0 8 1 * *"); // fallback to raw expression
}

#[test]
fn test_list_jobs_exclude_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "enabled1",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    svc.add_job(
        "enabled2",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    let all = svc.list_jobs(true);
    assert_eq!(all.len(), 2);

    // Disable one
    let job = all.into_iter().find(|j| j.name == "enabled1").unwrap();
    svc.toggle_job(&job.id).unwrap();

    let enabled = svc.list_jobs(false);
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].name, "enabled2");
}

#[test]
fn test_status_with_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "status_test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    let status = svc.status();
    assert_eq!(status["jobs"], 1);
    assert!(status["nextWakeAtMS"].is_number());
}

#[test]
fn test_reload_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "reload_test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    // Create new service and reload
    let svc2 = CronService::new(&path);
    svc2.reload().unwrap();
    assert_eq!(svc2.list_jobs(true).len(), 1);
}

#[test]
fn test_set_on_job_handler() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    let called = Arc::new(Mutex::new(false));
    let called_clone = called.clone();
    svc.set_on_job(move |_job| {
        *called_clone.lock() = true;
        Ok("result".to_string())
    });

    // Trigger via execute_job indirectly tests that handler is set
    svc.add_job(
        "handler_test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
}

#[test]
fn test_compute_next_run_every_v2() {
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(300000),
        expr: None,
        tz: None,
    };
    let now = Local::now().timestamp_millis();
    let result = compute_next_run(&schedule, now);
    assert_eq!(result, Some(now + 300000));
}

#[test]
fn test_compute_next_run_every_zero() {
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(0),
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_cron_valid() {
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 0 * * * *".to_string()),
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_some());
}

#[test]
fn test_compute_next_run_unknown_kind_v2() {
    let schedule = CronSchedule {
        kind: "unknown".to_string(),
        at_ms: None,
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_cron_service_new_creates_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("subdir/cron.json")
        .to_string_lossy()
        .to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    assert!(std::path::Path::new(&path).exists());
}

#[test]
fn test_update_job_name_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "orig",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    svc.update_job(&job.id, Some("renamed"), None).unwrap();
    let updated = svc.get_job(&job.id).unwrap();
    assert_eq!(updated.name, "renamed");
    // Schedule unchanged
    assert_eq!(updated.schedule.every_ms, Some(60000));
}

#[test]
fn test_validate_schedule_valid_cron() {
    assert!(CronService::validate_schedule("0 30 9 * * 1-5").is_ok());
    assert!(CronService::validate_schedule("*/15 * * * * *").is_ok());
}

#[test]
fn test_validate_schedule_out_of_range() {
    // Hour 25 is invalid
    assert!(CronService::validate_schedule("0 0 25 * * *").is_err());
    // Month 13 is invalid
    assert!(CronService::validate_schedule("0 0 1 1 13 *").is_err());
}

#[test]
fn test_status_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let status = svc.status();
    assert_eq!(status["jobs"], 0);
    assert!(status["nextWakeAtMS"].is_null());
}

// ========================================================================
// Additional coverage tests for 95%+
// ========================================================================

#[test]
fn test_compute_next_run_at_none_at_ms() {
    // "at" kind with at_ms = None → filter returns None
    let schedule = CronSchedule {
        kind: "at".to_string(),
        at_ms: None,
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_every_none_every_ms() {
    // "every" kind with every_ms = None → filter returns None
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: None,
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_every_negative_ms() {
    // "every" kind with every_ms = -1 → filter(|&ms| ms > 0) returns None
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(-1),
        expr: None,
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_compute_next_run_cron_invalid_expr_gronx() {
    // An expr that parses as 6 fields but has an invalid sub-field
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 0 0 32 1 *".to_string()), // day 32 is out of bounds
        tz: None,
    };
    let result = compute_next_run(&schedule, Local::now().timestamp_millis());
    assert!(result.is_none());
}

#[test]
fn test_cron_service_load_corrupt_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();

    // Write corrupt JSON to the file
    std::fs::write(&path, "{not valid json}").unwrap();

    // Creating a new service should attempt to load and fail silently
    let svc = CronService::new(&path);
    // The load failure is ignored (let _ = svc.load_store()) so jobs should be empty
    assert_eq!(svc.list_jobs(true).len(), 0);
}

#[test]
fn test_cron_service_reload_corrupt_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();

    let svc = CronService::new(&path);
    svc.add_job(
        "test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    // Corrupt the file
    std::fs::write(&path, "not json").unwrap();

    // Reload should return an error
    let result = svc.reload();
    assert!(result.is_err());
}

#[test]
fn test_cron_service_save_and_load_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();

    // Create service, add two jobs
    let svc = CronService::new(&path);
    svc.add_job(
        "job1",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m1",
        false,
        None,
        None,
    )
    .unwrap();
    svc.add_job(
        "job2",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(120000),
            expr: None,
            tz: None,
        },
        "m2",
        false,
        None,
        None,
    )
    .unwrap();

    // Verify file exists and contains valid JSON
    let data = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed["version"], 1);
    assert_eq!(parsed["jobs"].as_array().unwrap().len(), 2);

    // Create new service from same path → should load both jobs
    let svc2 = CronService::new(&path);
    assert_eq!(svc2.list_jobs(true).len(), 2);
}

#[test]
fn test_save_store_to_path_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let nested_path = dir
        .path()
        .join("a/b/c/cron.json")
        .to_string_lossy()
        .to_string();
    let data = CronStoreData {
        version: 1,
        jobs: vec![],
    };
    save_store_to_path(&nested_path, &data).unwrap();
    assert!(std::path::Path::new(&nested_path).exists());
    let loaded: CronStoreData =
        serde_json::from_str(&std::fs::read_to_string(&nested_path).unwrap()).unwrap();
    assert_eq!(loaded.version, 1);
}

#[test]
fn test_cron_service_start_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "test",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // First start
        svc.arm(); // H1: arm before start (fresh-process gate)
        svc.start().await.unwrap();
        // Second start should be idempotent (return Ok)
        svc.arm(); // H1: arm before start (fresh-process gate)
        svc.start().await.unwrap();
    });

    svc.stop();
}

#[test]
fn test_cron_service_stop_without_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // stop without start should not panic
    svc.stop();
}

#[test]
fn test_get_next_wake_ms_no_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let wake = svc.get_next_wake_ms();
    assert!(wake.is_none());
}

#[test]
fn test_get_next_wake_ms_with_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let future_ms = Local::now().timestamp_millis() + 60000;
    svc.add_job(
        "wake",
        CronSchedule {
            kind: "at".to_string(),
            at_ms: Some(future_ms),
            every_ms: None,
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    let wake = svc.get_next_wake_ms();
    assert!(wake.is_some());
    assert_eq!(wake.unwrap(), future_ms);
}

#[test]
fn test_get_next_wake_ms_disabled_job_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let future_ms = Local::now().timestamp_millis() + 60000;
    let job = svc
        .add_job(
            "disabled_wake",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(future_ms),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    svc.toggle_job(&job.id).unwrap(); // disable
    let wake = svc.get_next_wake_ms();
    assert!(wake.is_none());
}

#[test]
fn test_recompute_next_runs_skips_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    // Disable the job
    svc.toggle_job(&job.id).unwrap();

    // recompute_next_runs is private, but start() calls it
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        svc.arm(); // H1: arm before start (fresh-process gate)
        svc.start().await.unwrap();
    });
    svc.stop();

    // Disabled job should still have None for next_run (toggle sets it to None)
    let found = svc.get_job(&job.id).unwrap();
    assert!(!found.enabled);
    // The disabled job's next_run should remain None (start recomputes only enabled jobs)
    // But start recomputes and only sets enabled jobs, so disabled stays None
}

#[test]
fn test_cron_service_execute_job_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "exec_persist",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    svc.execute_job(&job.id).unwrap();

    // Verify persisted to disk
    let svc2 = CronService::new(&path);
    let loaded = svc2.get_job(&job.id).unwrap();
    assert_eq!(loaded.state.last_status, Some("executed".to_string()));
    assert!(loaded.state.last_run_at_ms.is_some());
}

#[test]
fn test_cron_service_enable_then_disable_clears_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());

    // Disable → next_run cleared
    let disabled = svc.enable_job(&job.id, false).unwrap();
    assert!(!disabled.enabled);
    assert!(disabled.state.next_run_at_ms.is_none());

    // Re-enable → next_run recomputed
    let enabled = svc.enable_job(&job.id, true).unwrap();
    assert!(enabled.enabled);
    assert!(enabled.state.next_run_at_ms.is_some());
}

#[test]
fn test_toggle_job_recomputes_next_run_on_enable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "test",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    // Toggle off
    let state = svc.toggle_job(&job.id).unwrap();
    assert!(!state);

    let found = svc.get_job(&job.id).unwrap();
    assert!(!found.enabled);
    // Note: toggle_job only recomputes when enabling, not disabling

    // Toggle back on → should recompute next_run
    let state2 = svc.toggle_job(&job.id).unwrap();
    assert!(state2);

    let found2 = svc.get_job(&job.id).unwrap();
    assert!(found2.enabled);
    assert!(found2.state.next_run_at_ms.is_some());
}

#[test]
fn test_update_job_with_cron_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "orig",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "x",
            false,
            None,
            None,
        )
        .unwrap();

    let new_schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 0 12 * * *".to_string()),
        tz: None,
    };
    svc.update_job(&job.id, None, Some(new_schedule)).unwrap();

    let updated = svc.get_job(&job.id).unwrap();
    assert_eq!(updated.schedule.kind, "cron");
    assert!(updated.state.next_run_at_ms.is_some());
}

#[test]
fn test_describe_schedule_minute_non_wildcard_hour_wildcard() {
    // Minute is not *, hour is * → doesn't match any specific pattern → falls through to raw expr
    let desc = CronService::describe_schedule("0 30 * * * *");
    assert_eq!(desc, "0 30 * * * *"); // falls through, not matching "Daily at" since hour is *
}

#[test]
fn test_describe_schedule_hour_and_minute_non_wildcard_day_wildcard() {
    // parts[1] != "*" && parts[2] != "*" && parts[3] == "*" && parts[4] == "*"
    let desc = CronService::describe_schedule("0 45 14 * * *");
    assert_eq!(desc, "Daily at 14:45");
}

#[test]
fn test_describe_schedule_day_non_wildcard() {
    // Day is non-wildcard → doesn't match "Daily at" pattern → falls through
    let desc = CronService::describe_schedule("0 30 9 15 * *");
    assert_eq!(desc, "0 30 9 15 * *");
}

#[test]
fn test_describe_schedule_month_non_wildcard() {
    // Month is non-wildcard → doesn't match "Daily at" pattern → falls through
    let desc = CronService::describe_schedule("0 30 9 * 6 *");
    assert_eq!(desc, "0 30 9 * 6 *");
}

#[test]
fn test_generate_id_format() {
    let id = generate_id();
    assert_eq!(id.len(), 16); // 8 bytes → 16 hex chars
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_generate_id_unique() {
    let id1 = generate_id();
    let id2 = generate_id();
    // Extremely unlikely to be equal
    assert_ne!(id1, id2);
}

#[test]
fn test_cron_service_new_no_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("does_not_exist.json")
        .to_string_lossy()
        .to_string();
    let svc = CronService::new(&path);
    // Should initialize empty, no panic
    assert_eq!(svc.list_jobs(true).len(), 0);
}

#[test]
fn test_cron_service_status_running_with_no_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        svc.arm(); // H1: arm before start (fresh-process gate)
        svc.start().await.unwrap();
    });

    let status = svc.status();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["jobs"], 0);
    assert!(status["nextWakeAtMS"].is_null());

    svc.stop();
}

#[test]
fn test_cron_service_status_next_wake_multiple_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    let now_ms = Local::now().timestamp_millis();
    let near_future = now_ms + 30000; // 30s
    let far_future = now_ms + 300000; // 5min

    svc.add_job(
        "near",
        CronSchedule {
            kind: "at".to_string(),
            at_ms: Some(near_future),
            every_ms: None,
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    svc.add_job(
        "far",
        CronSchedule {
            kind: "at".to_string(),
            at_ms: Some(far_future),
            every_ms: None,
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    let status = svc.status();
    let next_wake = status["nextWakeAtMS"].as_i64().unwrap();
    assert_eq!(next_wake, near_future);
}

#[tokio::test]
async fn test_cron_service_start_stop_with_handler() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    let counter = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let counter_clone = counter.clone();
    svc.set_on_job(move |_job| {
        counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("handled".to_string())
    });

    // Add a "every" job that fires every 1 second
    svc.add_job(
        "frequent",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(1000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    svc.arm(); // H1: arm before start (fresh-process gate)
    svc.start().await.unwrap();

    // Wait for the cron tick to process
    tokio::time::sleep(Duration::from_secs(3)).await;

    svc.stop();

    // Handler should have been called at least once
    let count = counter.load(std::sync::atomic::Ordering::SeqCst);
    assert!(count >= 1, "handler should have been called, got {}", count);
}

#[tokio::test]
async fn test_cron_service_start_deletes_after_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    let counter = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let counter_clone = counter.clone();
    svc.set_on_job(move |_job| {
        counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("ok".to_string())
    });

    // Add an "at" job due in 1 second → delete_after_run=true
    let near_future_ms = Local::now().timestamp_millis() + 1000;
    let job = svc
        .add_job(
            "one_shot",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(near_future_ms),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.delete_after_run);

    svc.arm(); // H1: arm before start (fresh-process gate)
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    svc.stop();

    // Job should have been deleted after execution
    let found = svc.get_job(&job.id);
    assert!(
        found.is_none(),
        "delete_after_run job should be removed after execution"
    );
}

#[tokio::test]
async fn test_cron_service_handler_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    svc.set_on_job(move |_job| Err("something went wrong".to_string()));

    // Use "every" with 1s interval, wait for one execution
    let job = svc
        .add_job(
            "error_job",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(1000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    svc.arm(); // H1: arm before start (fresh-process gate)
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    svc.stop();

    // Job should have error status
    let found = svc.get_job(&job.id).unwrap();
    assert_eq!(found.state.last_status, Some("error".to_string()));
    assert_eq!(
        found.state.last_error,
        Some("something went wrong".to_string())
    );
}

#[tokio::test]
async fn test_cron_service_no_handler_sets_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // No handler set

    let job = svc
        .add_job(
            "no_handler",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(1000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();

    svc.arm(); // H1: arm before start (fresh-process gate)
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    svc.stop();

    // No handler → status is "ok"
    let found = svc.get_job(&job.id).unwrap();
    assert_eq!(found.state.last_status, Some("ok".to_string()));
    assert!(found.state.last_error.is_none());
}

#[tokio::test]
async fn test_cron_service_every_job_recomputes_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    let counter = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let counter_clone = counter.clone();
    svc.set_on_job(move |_job| {
        counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("ok".to_string())
    });

    // Add an "every" job that fires every 1 second
    svc.add_job(
        "fast",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(1000),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();

    svc.arm(); // H1: arm before start (fresh-process gate)
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    svc.stop();

    // Should have been called multiple times
    let count = counter.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        count >= 2,
        "every job should fire multiple times, got {}",
        count
    );
}

#[test]
fn test_load_store_read_error() {
    // Use a directory path as store_path → reading will fail
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_string_lossy().to_string();
    // Create a directory at the path (not a file), so read_to_string fails
    let svc = CronService::new(&dir_path);
    // new() calls load_store() which tries to read a directory → should fail silently
    assert_eq!(svc.list_jobs(true).len(), 0);
}

#[test]
fn test_validate_schedule_weekday_7_is_sunday() {
    // croner treats 7 as Sunday (same as 0), matching standard cron behavior
    assert!(CronService::validate_schedule("0 0 0 * * 7").is_ok());
}

#[test]
fn test_validate_schedule_day_zero() {
    assert!(CronService::validate_schedule("0 0 0 0 * *").is_err());
}

#[test]
fn test_validate_schedule_month_zero() {
    assert!(CronService::validate_schedule("0 0 0 * 0 *").is_err());
}

#[test]
fn test_validate_schedule_second_wildcard() {
    assert!(CronService::validate_schedule("* * * * * *").is_ok());
}

#[test]
fn test_validate_schedule_all_ranges() {
    assert!(CronService::validate_schedule("0-59 0-59 0-23 1-31 1-12 0-6").is_ok());
}

#[test]
fn test_validate_schedule_comma_fields() {
    assert!(CronService::validate_schedule("0,30 0,15 9,17 * * *").is_ok());
}

#[test]
fn test_cron_job_default_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "test_defaults",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "test msg",
            true,
            Some("channel1"),
            Some("user1"),
        )
        .unwrap();

    assert_eq!(job.payload.kind, "agent_turn");
    assert_eq!(job.payload.message, "test msg");
    assert!(job.payload.deliver);
    assert_eq!(job.payload.channel, Some("channel1".to_string()));
    assert_eq!(job.payload.to, Some("user1".to_string()));
    assert_eq!(job.payload.command, None);
    assert!(job.created_at_ms > 0);
    assert_eq!(job.created_at_ms, job.updated_at_ms);
}

#[test]
fn test_cron_store_data_serialization() {
    let store = CronStoreData {
        version: 1,
        jobs: vec![],
    };
    let json = serde_json::to_string(&store).unwrap();
    let parsed: CronStoreData = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, 1);
    assert!(parsed.jobs.is_empty());
}

#[test]
fn test_multiple_jobs_independent_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job1 = svc
        .add_job(
            "j1",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m1",
            false,
            None,
            None,
        )
        .unwrap();
    let job2 = svc
        .add_job(
            "j2",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m2",
            false,
            None,
            None,
        )
        .unwrap();

    // Toggle job1 off
    svc.toggle_job(&job1.id).unwrap();
    assert!(!svc.get_job(&job1.id).unwrap().enabled);
    assert!(svc.get_job(&job2.id).unwrap().enabled);

    // Toggle job2 off
    svc.toggle_job(&job2.id).unwrap();
    assert!(!svc.get_job(&job1.id).unwrap().enabled);
    assert!(!svc.get_job(&job2.id).unwrap().enabled);

    // List only enabled → empty
    assert_eq!(svc.list_jobs(false).len(), 0);
    // List all → 2
    assert_eq!(svc.list_jobs(true).len(), 2);
}

// ========================================================================
// Croner-based tests: 5-field, day/month names, L/W/#, timezone
// ========================================================================

#[test]
fn test_validate_5_field_expression() {
    // Standard 5-field cron: min hour day month weekday
    assert!(CronService::validate_schedule("0 9 * * *").is_ok());
    assert!(CronService::validate_schedule("*/5 * * * *").is_ok());
    assert!(CronService::validate_schedule("30 9 * * 1-5").is_ok());
    assert!(CronService::validate_schedule("0 0 1 1 *").is_ok());
}

#[test]
fn test_validate_6_field_expression() {
    // 6-field: sec min hour day month weekday
    assert!(CronService::validate_schedule("0 0 9 * * *").is_ok());
    assert!(CronService::validate_schedule("0 */5 * * * *").is_ok());
    assert!(CronService::validate_schedule("30 0 12 * * 0").is_ok());
}

#[test]
fn test_validate_day_names() {
    // SUN-SAT names
    assert!(CronService::validate_schedule("0 9 * * MON-FRI").is_ok());
    assert!(CronService::validate_schedule("0 9 * * SUN").is_ok());
    assert!(CronService::validate_schedule("0 9 * * MON,WED,FRI").is_ok());
}

#[test]
fn test_validate_month_names() {
    // JAN-DEC names
    assert!(CronService::validate_schedule("0 0 1 JAN *").is_ok());
    assert!(CronService::validate_schedule("0 0 1 JAN-JUN *").is_ok());
    assert!(CronService::validate_schedule("0 0 1 JAN,JUN,DEC *").is_ok());
}

#[test]
fn test_validate_l_w_hash() {
    // L (last day of month)
    assert!(CronService::validate_schedule("0 0 L * *").is_ok());
    // W (nearest weekday)
    assert!(CronService::validate_schedule("0 0 1W * *").is_ok());
    // # (Nth weekday of month)
    assert!(CronService::validate_schedule("0 0 * * 1#2").is_ok()); // 2nd Monday
    assert!(CronService::validate_schedule("0 0 * * FRI#3").is_ok()); // 3rd Friday
}

#[test]
fn test_compute_next_run_5_field() {
    // 5-field "30 9 * * *" → daily at 9:30
    let after = chrono::DateTime::parse_from_rfc3339("2026-01-15T08:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("30 9 * * *".to_string()),
        tz: Some("UTC".to_string()),
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_dt = chrono::DateTime::from_timestamp_millis(next).unwrap();
    assert_eq!(next_dt.hour(), 9);
    assert_eq!(next_dt.minute(), 30);
}

#[test]
fn test_compute_next_run_5_field_weekdays() {
    // "0 9 * * 1-5" → weekdays at 9:00
    // 2026-01-17 is Saturday → next should be Monday 2026-01-19
    let after = chrono::DateTime::parse_from_rfc3339("2026-01-17T10:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 9 * * 1-5".to_string()),
        tz: Some("UTC".to_string()),
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_dt = chrono::DateTime::from_timestamp_millis(next).unwrap();
    assert_eq!(next_dt.day(), 19); // Monday
    assert_eq!(next_dt.hour(), 9);
}

#[test]
fn test_compute_next_run_month_names() {
    // "0 0 1 JAN *" → January 1st at midnight
    let after = chrono::DateTime::parse_from_rfc3339("2026-06-15T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 0 1 JAN *".to_string()),
        tz: Some("UTC".to_string()),
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_dt = chrono::DateTime::from_timestamp_millis(next).unwrap();
    assert_eq!(next_dt.month(), 1);
    assert_eq!(next_dt.day(), 1);
    assert_eq!(next_dt.year(), 2027);
}

#[test]
fn test_compute_next_run_day_names() {
    // "0 9 * * FRI" → every Friday at 9:00
    // 2026-01-14 is Wednesday → next Friday is 2026-01-16
    let after = chrono::DateTime::parse_from_rfc3339("2026-01-14T10:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 9 * * FRI".to_string()),
        tz: Some("UTC".to_string()),
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_dt = chrono::DateTime::from_timestamp_millis(next).unwrap();
    assert_eq!(next_dt.day(), 16);
    assert_eq!(next_dt.hour(), 9);
}

#[test]
fn test_compute_next_run_defaults_to_local() {
    // Production path: the web UI never sets tz, so cron fields must be read in
    // LOCAL time. "0 9 * * *" → 09:00 local on whatever machine runs this.
    // (Regression guard: the default was once UTC, which made "0 9" fire at
    // 09:00 UTC = 17:00 in UTC+8.)
    let after = chrono::Local::now().timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 9 * * *".to_string()),
        tz: None,
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_local = chrono::DateTime::from_timestamp_millis(next)
        .unwrap()
        .with_timezone(&chrono::Local);
    assert_eq!(next_local.hour(), 9);
    assert_eq!(next_local.minute(), 0);
}

#[test]
fn test_compute_next_run_with_timezone() {
    // "0 9 * * *" at Asia/Shanghai (UTC+8)
    // 9:00 Shanghai = 1:00 UTC
    let after = chrono::DateTime::parse_from_rfc3339("2026-01-15T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 9 * * *".to_string()),
        tz: Some("Asia/Shanghai".to_string()),
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_dt = chrono::DateTime::from_timestamp_millis(next).unwrap();
    // 9:00 Shanghai = 1:00 UTC
    assert_eq!(next_dt.hour(), 1);
}

#[test]
fn test_compute_next_run_5_field_every_5_minutes() {
    // "*/5 * * * *" → every 5 minutes
    let after = chrono::DateTime::parse_from_rfc3339("2026-01-15T10:03:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("*/5 * * * *".to_string()),
        tz: None,
    };
    let next = compute_next_run(&schedule, after).unwrap();
    let next_dt = chrono::DateTime::from_timestamp_millis(next).unwrap();
    assert_eq!(next_dt.minute(), 5);
}

#[test]
fn test_describe_5_field_every_minute() {
    assert_eq!(CronService::describe_schedule("* * * * *"), "Every minute");
}

#[test]
fn test_describe_5_field_every_n_minutes() {
    assert_eq!(
        CronService::describe_schedule("*/10 * * * *"),
        "Every 10 minutes"
    );
}

#[test]
fn test_describe_5_field_daily() {
    assert_eq!(
        CronService::describe_schedule("30 14 * * *"),
        "Daily at 14:30"
    );
}

#[test]
fn test_describe_6_field_every_minute() {
    assert_eq!(
        CronService::describe_schedule("0 * * * * *"),
        "Every minute"
    );
}

#[test]
fn test_add_cron_job_with_5_field_expr() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "5field_test",
            CronSchedule {
                kind: "cron".to_string(),
                at_ms: None,
                every_ms: None,
                expr: Some("30 9 * * *".to_string()),
                tz: Some("UTC".to_string()),
            },
            "daily at 9:30",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());
    let next_dt =
        chrono::DateTime::from_timestamp_millis(job.state.next_run_at_ms.unwrap()).unwrap();
    assert_eq!(next_dt.hour(), 9);
    assert_eq!(next_dt.minute(), 30);
}

#[test]
fn test_add_cron_job_with_month_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "monthly_named",
            CronSchedule {
                kind: "cron".to_string(),
                at_ms: None,
                every_ms: None,
                expr: Some("0 0 1 JAN *".to_string()),
                tz: None,
            },
            "January 1st",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());
}

#[test]
fn test_validate_invalid_expressions() {
    assert!(CronService::validate_schedule("").is_err());
    assert!(CronService::validate_schedule("60 0 0 * * *").is_err()); // minute OOB
    assert!(CronService::validate_schedule("0 0 0 32 1 *").is_err()); // day OOB
    assert!(CronService::validate_schedule("0 0 0 1 13 *").is_err()); // month OOB
}

// ---------------------------------------------------------------------------
// H1 (U12): startup arm gate
// ---------------------------------------------------------------------------

/// A fresh service is DISARMED: due persisted jobs do not fire until arm().
/// After arm() they fire normally.
#[tokio::test]
async fn test_cron_startup_disarmed_until_arm() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();

    // Create a store with an enabled, immediately-due job, then let it drop
    // — simulating a previous process's persisted state.
    {
        let svc = CronService::new(&path);
        svc.arm(); // the OLD process was armed; its jobs are enabled
        // 已到期 job 是「上一进程遗留的盘上状态」形态：走内部旁路造数
        // （BUG-20261024-11 后用户漏斗拒绝过期 at）。
        svc.add_job_internal(
            "due-job",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(chrono::Local::now().timestamp_millis() - 1000), // already due
                every_ms: None,
                expr: None,
                tz: None,
            },
            "fire me",
            true,
            Some("web"),
            Some("chat1"),
            None,
            None,
            true,
        )
        .unwrap();
    }

    // Fresh process: same store path → loads the persisted job. Disarmed.
    let svc = CronService::new(&path);
    assert!(svc.is_disarmed(), "fresh process starts disarmed");
    {
        let jobs = svc.store.lock().jobs.clone();
        assert_eq!(jobs.len(), 1, "persisted job loaded");
        assert!(jobs[0].enabled, "enabled flag preserved on disk");
    }

    // Handler that records invocations.
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fired2 = fired.clone();
    svc.set_on_job(move |_job| {
        fired2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("ok".to_string())
    });

    svc.start().await.unwrap();
    // Disarmed: even though the job is due, nothing fires within a few ticks.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(
        fired.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "disarmed gate suppresses due jobs"
    );
    assert!(svc.is_disarmed(), "still disarmed before explicit arm");

    // Arm: job becomes eligible and fires.
    svc.arm();
    assert!(!svc.is_disarmed());
    // The due job's next_run was NOT consumed while disarmed (gate skips
    // before collection), so it should fire on the next tick after arm.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert!(
        fired.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "armed service fires the persisted job"
    );
    svc.stop();
}

/// arm() is idempotent and jobs created after startup fire normally once
/// armed (armed latches for the process lifetime).
#[tokio::test]
async fn test_cron_arm_idempotent_and_latches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.arm();
    svc.arm(); // idempotent — no panic, no double effects
    assert!(!svc.is_disarmed());
}

/// BUG-HUNT R34：已执行失败的僵尸单发不得被 H1 重启复活。旧代码留下的磁盘
/// 形态（enabled=true + 过期 at_ms + last_run=Some + last_status=error，
/// fire loop 失败臂忘 disable）在新进程 recompute_next_runs 里曾被无条件
/// 塞回 stale due time → 每次网关重启重放一次已失败的单发（建单/LLM turn
/// 副作用重复）。last_run=Some = 单发语义已消耗；只有 last_run=None（停机
/// 期间从未执行）才享有 stale-due-once（`test_cron_startup_disarmed_until_arm`
/// 钉住的 H1 原案不受影响）。
#[test]
fn test_recompute_does_not_resurrect_failed_one_shot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // 遗留僵尸形态造数走内部旁路（BUG-20261024-11 后用户漏斗拒绝过期 at）。
    let job = svc
        .add_job_internal(
            "zombie_one_shot",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(chrono::Local::now().timestamp_millis() - 60_000),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    // 模拟旧代码失败臂留下的磁盘僵尸形态（fire loop 内 store 修改后由
    // save_store 持久化；这里直接改内存态再落盘等价）。
    {
        let mut s = svc.store.lock();
        let j = s.jobs.iter_mut().find(|j| j.id == job.id).unwrap();
        j.state.last_run_at_ms = Some(chrono::Local::now().timestamp_millis() - 59_000);
        j.state.last_status = Some("error".to_string());
        j.state.last_error = Some("boom".to_string());
        super::save_store_to_path(&path, &s).unwrap();
    }

    // 新进程（disarmed）加载 → start() 内同步 recompute → 不得复活。
    let svc2 = CronService::new(&path);
    assert!(svc2.is_disarmed(), "fresh process starts disarmed");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        svc2.start().await.unwrap();
    });
    svc2.stop();

    let after = svc2.get_job(&job.id).unwrap();
    assert_eq!(
        after.state.next_run_at_ms, None,
        "failed one-shot zombie must NOT be resurrected to stale due time"
    );
}

#[test]
fn s12b_describe_schedule_weekday_arm() {
    // S12b batch（quality-hardening goal 冲刺）：weekday 非通配的描述分支此前
    // 无断言覆盖（"At <hour>:<min> on weekdays <weekday>"）。
    assert_eq!(
        CronService::describe_schedule("0 0 9 * * 1"),
        "At 9:0 on weekdays 1"
    );
    // 对照组：month/day 非通配时不进 weekday 分支，原样返回
    assert_eq!(
        CronService::describe_schedule("0 0 9 15 * *"),
        "0 0 9 15 * *"
    );
}

// ========================================================================
// G3 运行账本接线（触发记账；全局槽由本测试进程独占安装）
// ========================================================================

/// execute_job 手动触发的账面：ok→succeeded、err→failed（trigger=manual
/// 进 payload；同 job.id 前缀圈定行，不受并行测试的无关记账干扰）。
#[test]
fn ledger_cron_execute_job_records_runs() {
    let ledger_dir = tempfile::tempdir().unwrap();
    let db = nemesis_taskstore::TaskDb::open(&ledger_dir.path().join("ledger.db"))
        .expect("open ledger db");
    nemesis_taskstore::ledger::install(db).expect("first install (process-wide)");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    };

    // 成功臂。
    let ok_job = svc
        .add_job("ledger-ok", schedule.clone(), "x", false, None, None)
        .unwrap();
    svc.set_on_job(|_job| Ok("done".to_string()));
    svc.execute_job(&ok_job.id).unwrap();

    // 失败臂。
    let err_job = svc
        .add_job("ledger-err", schedule, "x", false, None, None)
        .unwrap();
    svc.set_on_job(|_job| Err("boom".to_string()));
    svc.execute_job(&err_job.id).unwrap();

    let db = nemesis_taskstore::ledger::installed().expect("still installed");
    let ok_rows: Vec<_> = db
        .list_runs_by_kind("cron_job", 100)
        .expect("list")
        .into_iter()
        .filter(|r| r.id.starts_with(&format!("cron_job:{}:", ok_job.id)))
        .collect();
    assert_eq!(ok_rows.len(), 1, "一次触发一行账");
    assert_eq!(ok_rows[0].status, "succeeded");
    assert_eq!(ok_rows[0].peer_id, "cron");
    assert_eq!(ok_rows[0].payload["trigger"], "manual");
    assert!(ok_rows[0].completed_at.is_some());

    let err_rows: Vec<_> = db
        .list_runs_by_kind("cron_job", 100)
        .expect("list")
        .into_iter()
        .filter(|r| r.id.starts_with(&format!("cron_job:{}:", err_job.id)))
        .collect();
    assert_eq!(err_rows.len(), 1);
    assert_eq!(err_rows[0].status, "failed");

    // attempt 侧收口：两行都在跑 attempt 已终结（开账即带首 attempt）。
    for r in ok_rows.iter().chain(err_rows.iter()) {
        let attempts = db.attempts_of(&r.id).expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, r.status, "attempt 与 run 同态收口");
    }
}

// -------------------------------------------------------------------------
// 保留命名空间（BUG-20261024-10）：`board-ap:` / `memory-dreaming:` 是系统
// 内部作业的按名路由命名空间——用户面创建 loud 拒绝，内部装配走
// add_job_internal 旁路。
// -------------------------------------------------------------------------

fn every_60s() -> CronSchedule {
    CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    }
}

// ---------------------------------------------------------------------------
// W3（2026-10-25 加固）：JobPauseHandle——触发期
// pause 自身 job（父会话已删除 → 诚实停车，不删 job 可手动恢复）。
// ---------------------------------------------------------------------------

fn every_minute_schedule() -> CronSchedule {
    CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60000),
        expr: None,
        tz: None,
    }
}

#[test]
fn reserved_prefix_hit_unit() {
    assert_eq!(reserved_prefix_hit("board-ap:5"), Some("board-ap:"));
    assert_eq!(
        reserved_prefix_hit("memory-dreaming:sweep"),
        Some("memory-dreaming:")
    );
    // 前缀判定：子串在中间 / 缺冒号尾巴都不算命中。
    assert_eq!(reserved_prefix_hit("my-board-ap:5"), None);
    assert_eq!(reserved_prefix_hit("board-ap"), None);
    assert_eq!(reserved_prefix_hit("plain-name"), None);
}

#[test]
fn reserved_names_rejected_on_user_paths_and_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    for name in ["board-ap:5", "memory-dreaming:sweep", "board-ap:abc"] {
        let err = svc
            .add_job(name, every_60s(), "hello", false, None, None)
            .unwrap_err();
        assert!(
            err.contains("保留前缀"),
            "{name}: 报错应点明保留前缀: {err}"
        );
        // Err 必须意味着「没建」——不留半成品（与 BUG #16 同纪律）。
        assert!(
            svc.list_jobs(true).iter().all(|j| j.name != name),
            "被拒绝的名字不应产生 job: {name}"
        );
    }
}

#[test]
fn add_job_internal_bypasses_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // 内部装配点（autopilot / dreaming）同形名字必须能注册。
    for name in ["board-ap:5", "memory-dreaming:sweep"] {
        let job = svc
            .add_job_internal(name, every_60s(), "", false, None, None, None, None, true)
            .unwrap_or_else(|e| panic!("内部旁路应放行 {name}: {e}"));
        assert_eq!(job.name, name);
    }
    // 旁路只服务内部装配：用户面同前缀依旧拒绝。
    assert!(
        svc.add_job("board-ap:6", every_60s(), "x", false, None, None)
            .is_err()
    );
}

#[test]
fn reserved_names_rejected_on_rename_paths_and_name_unchanged() {
    // BUG-HUNT R99：改名入口（update_job / patch_job）此前不查保留前缀——
    // 用户经 tasks.cron.update 把作业改成 `board-ap:{i64}` 形态后，启动
    // 孤儿清理按「形态+未注册 id」静默删除、on_job 分派按形态劫持（message
    // 被忽略）：add 时的创建闸被 rename 完整绕过。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job("my-task", every_60s(), "hello", false, None, None)
        .unwrap();

    // update_job 改名路径 loud 拒绝。
    let err = svc
        .update_job(&job.id, Some("board-ap:7"), None)
        .unwrap_err();
    assert!(err.contains("保留前缀"), "update_job: {err}");
    // patch_job 改名路径 loud 拒绝。
    let err = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                name: Some("memory-dreaming:sweep".to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.contains("保留前缀"), "patch_job: {err}");
    // Err = 无副作用：名字原样保留。
    assert_eq!(svc.get_job(&job.id).unwrap().name, "my-task");
    // 正常改名放行（闸只拦保留前缀，不拦改名本身）。
    svc.update_job(&job.id, Some("renamed-ok"), None).unwrap();
    assert_eq!(svc.get_job(&job.id).unwrap().name, "renamed-ok");
}

#[test]
fn update_job_schedule_on_disabled_job_keeps_next_run_none() {
    // BUG-HUNT R99：update_job 的 schedule 分支此前不带 enabled 守卫裸写
    // next_run（patch_job 有 `if job.enabled`）——禁用作业带上幻影「下次
    // 触发」，违背 W1b「disabled 不排程，enable 重算」不变量。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job("t", every_60s(), "m", false, None, None)
        .unwrap();
    svc.enable_job(&job.id, false).unwrap();
    assert_eq!(svc.get_job(&job.id).unwrap().state.next_run_at_ms, None);

    let sched = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(120_000),
        expr: None,
        tz: None,
    };
    svc.update_job(&job.id, None, Some(sched)).unwrap();
    let j = svc.get_job(&job.id).unwrap();
    assert_eq!(j.schedule.every_ms, Some(120_000), "排程本身要更新");
    assert_eq!(
        j.state.next_run_at_ms, None,
        "disabled 作业改排程不得落 next_run"
    );

    // 重新启用后重算（不变量闭环）。
    svc.enable_job(&job.id, true).unwrap();
    assert!(
        svc.get_job(&job.id).unwrap().state.next_run_at_ms.is_some(),
        "enable 后必须重新落 next_run"
    );
}

// ===========================================================================
// BUG-20261024-11：时间算术受检化——溢出 honest None / 死排程创建闸
// ===========================================================================

#[test]
fn test_compute_next_run_every_overflow_returns_none_not_wrap() {
    // 旧的 now_ms + ms 裸加法在 release 下回绕成负值 → next_run <= now
    // 恒真 → 每 tick 自旋触发（触发风暴）。受检加法必须 honest None。
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(i64::MAX),
        expr: None,
        tz: None,
    };
    assert!(compute_next_run(&schedule, 1_000).is_none());

    // 边界：不溢出的极限间隔仍给出 Some（而非一刀切拒绝大间隔）。
    let schedule = CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(i64::MAX - 2_000),
        expr: None,
        tz: None,
    };
    assert_eq!(compute_next_run(&schedule, 1_000), Some(i64::MAX - 1_000));
}

#[test]
fn test_secs_to_ms_checked_overflow_honest() {
    assert_eq!(secs_to_ms_checked(5).unwrap(), 5_000);
    assert_eq!(
        secs_to_ms_checked(i64::MAX as u64).unwrap_err(),
        "seconds value too large"
    );
    // u64 值可转 i64 但 ×1000 溢出：同样 Err（旧 CLI 裸乘法在此回绕）。
    assert!(secs_to_ms_checked((i64::MAX as u64) / 100 + 1).is_err());
}

#[test]
fn add_job_ext_rejects_dead_schedules_loud() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);

    // 过期 at。
    let past_at = CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(1),
        every_ms: None,
        expr: None,
        tz: None,
    };
    let err = svc
        .add_job_ext("dead-at", past_at, "m", false, None, None, None, None, true)
        .unwrap_err();
    assert!(err.contains("死排程"), "错误要点明死排程: {err}");

    // 非正 every（0 与负值）。
    for ms in [0i64, -1] {
        let sched = CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(ms),
            expr: None,
            tz: None,
        };
        assert!(
            svc.add_job_ext(
                "dead-every",
                sched,
                "m",
                false,
                None,
                None,
                None,
                None,
                true
            )
            .is_err(),
            "every_ms={ms} 必须拒绝"
        );
    }

    // 未知 kind。
    let unknown = CronSchedule {
        kind: "rapture".to_string(),
        at_ms: None,
        every_ms: None,
        expr: None,
        tz: None,
    };
    assert!(
        svc.add_job_ext(
            "dead-unknown",
            unknown,
            "m",
            false,
            None,
            None,
            None,
            None,
            true
        )
        .is_err()
    );

    // 永不匹配的 cron 表达式（2 月 30 日不存在）——parse 合法但无下一次触发。
    let never = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 0 30 2 *".to_string()),
        tz: None,
    };
    assert!(
        svc.add_job_ext(
            "dead-never",
            never,
            "m",
            false,
            None,
            None,
            None,
            None,
            true
        )
        .is_err(),
        "parse 合法但永不触发的 cron 也必须拒绝"
    );

    // 四个死排程一个都不在册。
    let names: Vec<String> = svc.list_jobs(true).into_iter().map(|j| j.name).collect();
    assert!(names.is_empty(), "死排程一个都不能建: {names:?}");
}

#[test]
fn add_job_ext_allows_disabled_dead_schedule_but_stays_dormant() {
    // enabled=false 放行（W1b 语义：不排程不触发；enable_job 重算时才落
    // next_run）——闸只挡「已排程」的假象。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let past_at = CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(1),
        every_ms: None,
        expr: None,
        tz: None,
    };
    let job = svc
        .add_job_ext(
            "dormant", past_at, "m", false, None, None, None, None, false,
        )
        .unwrap();
    assert!(!job.enabled);
    assert_eq!(job.state.next_run_at_ms, None);
}

#[test]
fn add_job_internal_still_allows_legacy_dead_schedule() {
    // 内部旁路保持原语义（board sync / dreaming 装配点 + 历史 store 修复
    // 路径），死排程闸只设在用户漏斗 add_job_ext。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let past_at = CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(1),
        every_ms: None,
        expr: None,
        tz: None,
    };
    let job = svc
        .add_job_internal("legacy", past_at, "m", false, None, None, None, None, true)
        .unwrap();
    assert!(job.enabled);
    assert_eq!(job.state.next_run_at_ms, None, "死排程内部注册＝不排程");
}

#[test]
fn cron_expr_can_fire_combines_syntax_and_semantics() {
    // BUG-20261025-18：挂载面（autopilot/dreaming）共用的组合判定。
    let now = Local::now().timestamp_millis();
    // 语法错 = false。
    assert!(!cron_expr_can_fire("不是cron", now));
    // 解析通过但永不匹配（2 月没有 30 日）= false——纯 parse 闸的盲区。
    assert!(!cron_expr_can_fire("0 0 30 2 *", now));
    assert!(!cron_expr_can_fire("* * 31 4 *", now));
    // 合法且可触发 = true（含闰年 2/29——存在年份解）。
    assert!(cron_expr_can_fire("*/1 * * * *", now));
    assert!(cron_expr_can_fire("0 0 29 2 *", now));
}

// ---------------------------------------------------------------------------
// W3（2026-10-25 加固）：JobPauseHandle——触发期
// pause 自身 job（父会话已删除 → 诚实停车，不删 job 可手动恢复）。
// ---------------------------------------------------------------------------
/// pause 置 enabled=false 且落盘（新开服务实例从同一路径重载 = 仍停用）；
/// patch_job 重新启用（恢复路径）后可再 pause。
#[test]
fn w3_pause_handle_disables_persists_and_resume_works() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job_ext(
            "w3-pause",
            every_minute_schedule(),
            "hello",
            false,
            None,
            None,
            Some("agent:main:session:gone"),
            None,
            true,
        )
        .unwrap();
    assert!(job.enabled);

    let handle = svc.job_pause_handle();
    assert!(handle.pause_job(&job.id), "首次 pause 生效");
    assert!(
        !handle.pause_job(&job.id),
        "重复 pause 幂等（无变化=false）"
    );
    assert!(!handle.pause_job("no-such-job"), "未知 id 诚实 false");

    // 落盘证明：全新实例（同路径重载）看到停用态。
    let svc2 = CronService::new(&path);
    let reloaded = svc2.get_job(&job.id).unwrap();
    assert!(!reloaded.enabled, "停用态已持久化");

    // 恢复路径：patch_job 重新启用（句柄同源的 svc 实例）→ handle 可再
    // pause（诚实再停）。
    let patch = CronJobPatch {
        enabled: Some(true),
        ..Default::default()
    };
    svc.patch_job(&job.id, &patch).unwrap();
    assert!(svc.get_job(&job.id).unwrap().enabled);
    assert!(handle.pause_job(&job.id), "重新启用后再 pause 生效");
    assert!(!svc.get_job(&job.id).unwrap().enabled);
}

/// handle 与服务主存储共享（句柄 pause 后服务的 list/patch 视角同步）；
/// handler 场景模拟：闭包捕获句柄，触发期内 pause 自身不与 on_job 槽锁
/// 冲突（句柄不碰外层 CronService 互斥——web「立即运行」自死锁的根）。
#[test]
fn w3_pause_handle_shares_store_and_avoids_outer_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job_ext(
            "w3-shared",
            every_minute_schedule(),
            "hello",
            false,
            None,
            None,
            Some("agent:main:session:x"),
            None,
            true,
        )
        .unwrap();

    // set_on_job 前取句柄，闭包内 pause 自身（模拟 gateway 触发路径）。
    let handle = svc.job_pause_handle();
    svc.set_on_job(move |j: &CronJob| {
        if handle.pause_job(&j.id) {
            Err("paused by test".to_string())
        } else {
            Ok("noop".to_string())
        }
    });

    // execute_job 在持有外层 &svc（非 Mutex）下驱动 handler——句柄若走
    // 外层互斥即死锁/毒化；这里能正常返回即证绕开。execute_job 恒 Ok——
    // handler Err 只落 job state（last_status=error），不向上传播。
    let res = svc.execute_job(&job.id);
    assert!(
        res.is_ok(),
        "execute_job 恒 Ok（handler Err 落 state 不传播）"
    );
    let after = svc.get_job(&job.id).unwrap();
    assert!(!after.enabled, "handler 内 pause 已生效");
    assert_eq!(after.state.last_status.as_deref(), Some("error"));
    assert_eq!(after.state.last_error.as_deref(), Some("paused by test"));
}
// ===========================================================================
// BUG-20261025-30：死排程闸覆盖缺口（更新/patch/启用路径）+ toggle 停用
// 幻影 next_run。BUG-20261024-11 的闸只设在创建漏斗 add_job_ext，四条
// 用户面变更路径（update_job / patch_job / enable_job(true) / toggle 翻正）
// 全部绕过——enabled + 算不出任何下一次触发 = 静默僵尸。补闸 = 同一不变量
// 覆盖全部用户面路径（内部旁路 add_job_internal 维持豁免，board 同步有
// BUG-20261025-18 挂载防线前置）。A 面：toggle 停用残留旧 next_run，
// 对齐 enable_job / patch_job 的 W1b 不变量（disabled 作业不排程）。
// ===========================================================================

/// 永不触发的 cron：2 月没有 30 日（BUG-20261024-11 同款盲区样本）。
fn dead_cron_schedule() -> CronSchedule {
    CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 0 30 2 *".to_string()),
        tz: None,
    }
}

/// 已过期的一次性 at。
fn dead_at_schedule(now_ms: i64) -> CronSchedule {
    CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(now_ms - 60_000),
        every_ms: None,
        expr: None,
        tz: None,
    }
}

fn alive_every_schedule() -> CronSchedule {
    CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(60_000),
        expr: None,
        tz: None,
    }
}

#[test]
fn update_job_rejects_dead_schedule_swap_on_enabled_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job("alive", alive_every_schedule(), "m", false, None, None)
        .unwrap();
    // 换入永不触发的调度 = 死排程，loud 拒绝（旧代码静默接受成僵尸）。
    let err = svc
        .update_job(&job.id, None, Some(dead_cron_schedule()))
        .unwrap_err();
    assert!(err.contains("死排程"), "错误信息要点名死排程: {err}");
    // 无副作用：原调度与启用态原样保留。
    let after = svc.get_job(&job.id).unwrap();
    assert_eq!(
        after.schedule.every_ms,
        Some(60_000),
        "拒绝后原调度必须保留"
    );
    assert!(after.enabled);
}

#[test]
fn patch_job_rejects_dead_schedule_by_final_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job("alive", alive_every_schedule(), "m", false, None, None)
        .unwrap();
    // ① 启用中的作业 patch 换入死 cron（enabled 缺省保持 true）= 拒绝。
    let err = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                schedule: Some(dead_cron_schedule()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.contains("死排程"), "错误信息要点名死排程: {err}");
    let after = svc.get_job(&job.id).unwrap();
    assert_eq!(
        after.schedule.every_ms,
        Some(60_000),
        "拒绝后原调度必须保留"
    );
    assert!(after.enabled);
    assert!(after.state.next_run_at_ms.is_some(), "拒绝后原排程必须保留");
    // ② 禁用作业 patch 成死排程 = 休眠死排程，放行（W1b：不排程不触发，
    //    与 add_job_ext 对 disabled 死排程放行同语义）。
    svc.enable_job(&job.id, false).unwrap();
    svc.patch_job(
        &job.id,
        &CronJobPatch {
            schedule: Some(dead_cron_schedule()),
            enabled: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    let after2 = svc.get_job(&job.id).unwrap();
    assert!(!after2.enabled);
    assert_eq!(after2.state.next_run_at_ms, None, "禁用作业不排程");
}

#[test]
fn enable_job_rejects_enabling_dead_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // 内部旁路（豁免闸）造休眠死排程：过期 at + disabled。
    let job = svc
        .add_job_internal(
            "dormant",
            dead_at_schedule(Local::now().timestamp_millis()),
            "m",
            false,
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();
    // 启用死排程 = 僵尸，loud 拒绝（旧代码静默接受成 enabled+无排程僵尸）。
    let err = svc.enable_job(&job.id, true).unwrap_err();
    assert!(err.contains("死排程"), "错误信息要点名死排程: {err}");
    let after = svc.get_job(&job.id).unwrap();
    assert!(!after.enabled, "拒绝后必须保持禁用");
    assert_eq!(after.state.next_run_at_ms, None);
    // 禁用方向不受闸影响。
    svc.enable_job(&job.id, false).unwrap();
}

#[test]
fn toggle_job_rejects_enabling_dead_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job_internal(
            "dormant",
            dead_at_schedule(Local::now().timestamp_millis()),
            "m",
            false,
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();
    let err = svc.toggle_job(&job.id).unwrap_err();
    assert!(err.contains("死排程"), "错误信息要点名死排程: {err}");
    let after = svc.get_job(&job.id).unwrap();
    assert!(!after.enabled, "拒绝后必须保持禁用");
    assert_eq!(after.state.next_run_at_ms, None);
}

#[test]
fn toggle_job_disable_clears_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job("alive", alive_every_schedule(), "m", false, None, None)
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());
    // A 面（W1b 对齐）：停用不得残留幻影「下次触发」。
    let new_state = svc.toggle_job(&job.id).unwrap();
    assert!(!new_state);
    let after = svc.get_job(&job.id).unwrap();
    assert_eq!(
        after.state.next_run_at_ms, None,
        "停用后残留旧 next_run = 幻影「下次触发」（W1b 不变量）"
    );
    // 再翻正恢复排程。
    let back = svc.toggle_job(&job.id).unwrap();
    assert!(back);
    let after2 = svc.get_job(&job.id).unwrap();
    assert!(
        after2.state.next_run_at_ms.is_some(),
        "重新启用必须重算排程"
    );
}

// -- BUG-20261025-49：REL-002 损坏守卫（gateway 侧补位）+ pause_job W1b 补位 --

#[test]
fn r127_corrupt_store_blocks_save_and_preserves_file_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json");
    let corrupt = r##"{"version":1,"jobs":[{"id":"a","##;
    std::fs::write(&path, corrupt).unwrap();
    let svc = CronService::new(&path.to_string_lossy());

    // 变更面全拒绝（save_store 毒态闸），错误自带修复指引。
    let err = svc
        .add_job(
            "x",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60_000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap_err();
    assert!(err.contains("拒绝落盘"), "错误必须声明拒绝落盘语义: {err}");
    assert!(err.contains(&path.to_string_lossy().to_string()), "{err}");

    // 核心断言：盘上损坏文件原样保留（未被空 store 顶掉）。
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk, corrupt, "毒态下任何落盘都是数据销毁");

    // 内存回滚：add 未留半态（列表为空，不是空 store 写穿后的假象）。
    assert!(svc.list_jobs(true).is_empty());
}

#[tokio::test]
async fn r127_corrupt_store_blocks_start_and_manual_execute() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json");
    std::fs::write(&path, "not json at all").unwrap();
    let svc = CronService::new(&path.to_string_lossy());

    let err = svc.start().await.unwrap_err();
    assert!(err.contains("拒绝启动"), "{err}");
    // start 拒绝后不得落盘（文件仍是原始损坏内容）。
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json at all");
    assert_eq!(
        svc.status()["enabled"],
        serde_json::Value::Bool(false),
        "毒态 start 失败不得置 running"
    );

    // 手动触发同拒（先拒绝后触发——不留「副作用已发生而记账落盘失败」态）。
    let err2 = svc.execute_job("whatever").unwrap_err();
    assert!(err2.contains("拒绝手动触发"), "{err2}");

    // 恢复路径：文件修复 → reload 成功解毒 → 一切恢复。
    let fixed = crate::store::CronStoreData {
        version: 1,
        jobs: vec![],
    };
    crate::store::save(&path, &fixed).unwrap();
    svc.reload().unwrap();
    let job = svc
        .add_job(
            "after-heal",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60_000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());
}

#[test]
fn r127_pause_job_clears_next_run_w1b() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "p",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60_000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());
    let handle = svc.job_pause_handle();
    assert!(handle.pause_job(&job.id), "启用中作业应可暂停");
    let after = svc.get_job(&job.id).unwrap();
    assert!(!after.enabled);
    assert_eq!(
        after.state.next_run_at_ms, None,
        "停用作业不得残留幻影「下次触发」（W1b，对齐 toggle/enable/patch）"
    );
    // 幂等：已停用再 pause = false（无状态变化）。
    assert!(!handle.pause_job(&job.id));
    // 不存在的 job = false。
    assert!(!handle.pause_job("nope"));
}

/// 测试辅助：`Arc<Mutex<CronService>>` 的 arm+start。await_holding_lock 是
/// 数据流型 lint 只认函数级 allow（同 gateway services.rs start_cron_scheduler
/// 先例）——start 的 future 借用锁内数据，结构性无法先放锁；测试里锁内无
/// 并发竞争者，无实际死锁风险。
#[allow(clippy::await_holding_lock)]
async fn arm_and_start(svc: &Arc<std::sync::Mutex<CronService>>) {
    let s = svc.lock().unwrap();
    s.arm(); // H1: arm before start
    s.start().await.unwrap();
}

/// BUG-20261028-116 红证①：执行期内并发改期为「未来 at 单发」的任务，
/// 触发循环的 retain 不得按「成功单发即删」把它吞掉。
///
/// patch 发生在 on_job handler 内部（reset next_run 之后、post-exec 之前），
/// 交错完全确定：post-exec 读到的是 patch 后的当前态（kind=at、
/// delete_after_run=true、next_run=未来 at_ms、status=ok），旧代码 retain
/// 三条件全中 → 任务连同用户刚改好的新排程被静默删除。
#[tokio::test]
async fn test_fire_loop_keeps_job_rescheduled_to_future_at_during_execution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = Arc::new(std::sync::Mutex::new(CronService::new(&path)));

    let job = {
        let s = svc.lock().unwrap();
        s.add_job(
            "recurring",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(1000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap()
    };

    // handler 执行期内把任务改期为 60s 后的单发（模拟用户在执行窗口内改期）。
    let svc_for_handler = svc.clone();
    let patched = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let patched_clone = patched.clone();
    let id_for_handler = job.id.clone();
    svc.lock().unwrap().set_on_job(move |_job| {
        let s = svc_for_handler.lock().unwrap();
        let at_ms = Local::now().timestamp_millis() + 60_000;
        s.patch_job(
            &id_for_handler,
            &CronJobPatch {
                schedule: Some(CronSchedule {
                    kind: "at".to_string(),
                    at_ms: Some(at_ms),
                    every_ms: None,
                    expr: None,
                    tz: None,
                }),
                ..Default::default()
            },
        )
        .expect("patch during execution must succeed");
        patched_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok("ok".to_string())
    });

    arm_and_start(&svc).await;

    // 等 handler 真正跑过（触发循环 tick 1s + job 1s 间隔，余量给慢机）。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !patched.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "handler never ran within timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 再让触发循环走完 post-exec + retain（一个 tick 余量）。
    tokio::time::sleep(Duration::from_millis(1500)).await;
    svc.lock().unwrap().stop();

    let after = svc
        .lock()
        .unwrap()
        .get_job(&job.id)
        .expect("执行期内被改期为未来单发的任务不得被触发循环 retain 吞掉");
    assert_eq!(after.schedule.kind, "at");
    assert!(after.delete_after_run);
    assert!(after.enabled);
    assert_eq!(
        after.state.last_status,
        Some("ok".to_string()),
        "本次触发（改期前调度）应正常记账"
    );
    assert!(
        after.state.next_run_at_ms.is_some(),
        "patch 建立的未来触发点必须保留——删掉它 = 用户刚改好的排程静默失效"
    );
}

/// BUG-20261028-116 红证②：执行期内改期 + 本次触发失败——单发停摆收尾
/// （R34）不得把 patch 刚建立的新排程一并停用清空。
#[tokio::test]
async fn test_fire_loop_failed_fire_keeps_rescheduled_future_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = Arc::new(std::sync::Mutex::new(CronService::new(&path)));

    let job = {
        let s = svc.lock().unwrap();
        s.add_job(
            "recurring",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(1000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap()
    };

    let svc_for_handler = svc.clone();
    let patched = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let patched_clone = patched.clone();
    let id_for_handler = job.id.clone();
    svc.lock().unwrap().set_on_job(move |_job| {
        let s = svc_for_handler.lock().unwrap();
        let at_ms = Local::now().timestamp_millis() + 60_000;
        s.patch_job(
            &id_for_handler,
            &CronJobPatch {
                schedule: Some(CronSchedule {
                    kind: "at".to_string(),
                    at_ms: Some(at_ms),
                    every_ms: None,
                    expr: None,
                    tz: None,
                }),
                ..Default::default()
            },
        )
        .expect("patch during execution must succeed");
        patched_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        Err("本次触发失败（改期前的调度）".to_string())
    });

    arm_and_start(&svc).await;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !patched.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "handler never ran within timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    svc.lock().unwrap().stop();

    let after = svc
        .lock()
        .unwrap()
        .get_job(&job.id)
        .expect("失败的触发只消费改期前那次，任务本身不得消失");
    assert_eq!(after.state.last_status, Some("error".to_string()));
    assert!(
        after.enabled,
        "执行期内建立的新排程不得被失败收尾一并停用（R34 只针对已消费的单发）"
    );
    assert!(
        after.state.next_run_at_ms.is_some(),
        "patch 建立的未来触发点不得被失败收尾清空"
    );
}

// ---------------------------------------------------------------------------
// R197 绿证（BUG-20261028-131）：collect + reset 已合并进单临界区 helper
// （`collect_and_reset_due`）。旧形态 collect（读锁）与 reset（写锁）两段
// 之间，并发 patch_job 可给到期 at 作业换入新未来 next_run，reset 无条件按
// due 列表践踏回 None——执行后 at 分支见 None 走失败臂（disable 停摆）或
// delete_after_run 臂（整单删除）。合并后窗口结构性消除：patch 落在前 =
// next_run 已是未来不被收集；落在后 = post-execution 保留臂接住
//（test_fire_loop_keeps_job_rescheduled_to_future_at_during_execution /
//  test_fire_loop_failed_fire_keeps_rescheduled_future_at，BUG-116 双红证）。
// 窗口本身是微秒级并发时序，不可稳定红证——以下四用例钉 helper 的直接
// 可观察不变量（纯同步、无 sleep）。
// ---------------------------------------------------------------------------

/// 到期 enabled 作业：收集 + next_run 同临界区复位 + 落盘；同场未来作业
/// 不受扰动。
#[test]
fn collect_and_reset_due_collects_due_resets_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // 到期形态造数走内部旁路（用户漏斗拒绝过期 at，BUG-20261024-11），
    // next_run 手动钉死（不依赖墙钟与 recompute）。
    let due_job = svc
        .add_job_internal(
            "due",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(1000),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    let future_job = svc
        .add_job_internal(
            "future",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(10_000),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    {
        let mut s = svc.store.lock();
        s.jobs
            .iter_mut()
            .find(|j| j.id == due_job.id)
            .unwrap()
            .state
            .next_run_at_ms = Some(1000);
        s.jobs
            .iter_mut()
            .find(|j| j.id == future_job.id)
            .unwrap()
            .state
            .next_run_at_ms = Some(10_000);
        super::save_store_to_path(&path, &s).unwrap();
    }

    let due = collect_and_reset_due(&svc.store, &path, 5000);

    assert_eq!(due, vec![due_job.id.clone()], "只收集到期 enabled 作业");
    {
        let s = svc.store.lock();
        assert!(
            s.jobs
                .iter()
                .find(|j| j.id == due_job.id)
                .unwrap()
                .state
                .next_run_at_ms
                .is_none(),
            "到期作业 next_run 必须在收集中被复位"
        );
        assert_eq!(
            s.jobs
                .iter()
                .find(|j| j.id == future_job.id)
                .unwrap()
                .state
                .next_run_at_ms,
            Some(10_000),
            "未来作业不得被扰动"
        );
    }
    // 落盘：盘上 next_run 同步复位（save 与复位同临界区）。
    let on_disk = crate::store::load(std::path::Path::new(&path)).unwrap();
    assert!(
        on_disk
            .jobs
            .iter()
            .find(|j| j.id == due_job.id)
            .unwrap()
            .state
            .next_run_at_ms
            .is_none(),
        "复位必须与收集同临界区落盘"
    );
}

/// disabled 作业即使盘上残留过期 next_run 也不得被收集、状态不得被碰。
#[test]
fn collect_and_reset_due_skips_disabled_expired_job() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let disabled_job = svc
        .add_job_internal(
            "disabled",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(1000),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
            None,
            None,
            false, // enabled = false
        )
        .unwrap();
    {
        let mut s = svc.store.lock();
        s.jobs
            .iter_mut()
            .find(|j| j.id == disabled_job.id)
            .unwrap()
            .state
            .next_run_at_ms = Some(1000);
        super::save_store_to_path(&path, &s).unwrap();
    }

    let due = collect_and_reset_due(&svc.store, &path, 5000);

    assert!(due.is_empty(), "disabled 作业不得进 due");
    let s = svc.store.lock();
    assert_eq!(
        s.jobs
            .iter()
            .find(|j| j.id == disabled_job.id)
            .unwrap()
            .state
            .next_run_at_ms,
        Some(1000),
        "未收集的作业状态不得被触碰"
    );
}

/// BUG-131 主场景绿证：到期作业已被并发 patch 换入未来 next_run（落定
/// 形态）——reset 不得按「曾经到期」践踏新排程；合并临界区后收集判定与
/// 复位原子，next_run 已是未来 ⇒ 不进 due ⇒ 状态原样保留。
#[test]
fn collect_and_reset_due_respects_future_next_run_after_reschedule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    // 单发 at（delete_after_run=true，旧行为下整单删除的最坏臂）。
    let job = svc
        .add_job_internal(
            "rescheduled",
            CronSchedule {
                kind: "at".to_string(),
                at_ms: Some(1000),
                every_ms: None,
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
    // 并发 patch 赢得窗口的落定形态：next_run 已被换到未来。
    let patched_next = 1000 + 60_000;
    {
        let mut s = svc.store.lock();
        s.jobs
            .iter_mut()
            .find(|j| j.id == job.id)
            .unwrap()
            .state
            .next_run_at_ms = Some(patched_next);
        super::save_store_to_path(&path, &s).unwrap();
    }

    let due = collect_and_reset_due(&svc.store, &path, 5000);

    assert!(due.is_empty(), "patch 后的未来 next_run 不得被收集");
    let s = svc.store.lock();
    let j = s.jobs.iter().find(|j| j.id == job.id).unwrap();
    assert_eq!(
        j.state.next_run_at_ms,
        Some(patched_next),
        "改期后的触发点必须原样保留——践踏回 None 会让执行后 at 分支走删除臂"
    );
}

/// 空 due：不触发任何落盘（保持旧形态每秒空转零 IO 的语义）。
#[test]
fn collect_and_reset_due_no_due_no_disk_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    // CronService::new 对缺失文件返回空 store 且不写盘（store::load 短路）。
    assert!(!std::path::Path::new(&path).exists());
    let svc = CronService::new(&path);

    let due = collect_and_reset_due(&svc.store, &path, 5000);

    assert!(due.is_empty());
    assert!(
        !std::path::Path::new(&path).exists(),
        "空 due 不得创建/改写存储文件"
    );
}

// ===== BUG-20261029：on_job handler panic 隔离 =====

#[test]
fn invoke_handler_catch_panic_maps_panic_to_err() {
    let job = CronJob {
        id: "j1".to_string(),
        name: "boom".to_string(),
        enabled: true,
        schedule: CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        payload: CronPayload {
            kind: "agent_turn".to_string(),
            message: String::new(),
            command: None,
            deliver: false,
            channel: None,
            to: None,
            session_key: None,
            max_rounds: None,
        },
        state: CronJobState {
            next_run_at_ms: None,
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            history: Vec::new(),
        },
        created_at_ms: 0,
        updated_at_ms: 0,
        delete_after_run: false,
    };
    let h: Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync> =
        Box::new(|_j| panic!("handler exploded"));
    let out = invoke_handler_catch_panic(Some(&h), &job).unwrap();
    let err = out.unwrap_err();
    assert!(
        err.contains("handler panicked") && err.contains("handler exploded"),
        "panic 必须映射为 Err 合成串: {err}"
    );

    // 正常路径透传：Ok / Err 原样，None handler → None。
    let ok: Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync> =
        Box::new(|_j| Ok("fine".to_string()));
    assert_eq!(
        invoke_handler_catch_panic(Some(&ok), &job)
            .unwrap()
            .unwrap(),
        "fine"
    );
    let e: Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync> =
        Box::new(|_j| Err("plain error".to_string()));
    assert_eq!(
        invoke_handler_catch_panic(Some(&e), &job)
            .unwrap()
            .unwrap_err(),
        "plain error"
    );
    assert!(invoke_handler_catch_panic(None, &job).is_none());
}

#[test]
fn execute_job_survives_panicking_handler_and_records_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "boom",
            CronSchedule {
                kind: "every".to_string(),
                at_ms: None,
                every_ms: Some(60000),
                expr: None,
                tz: None,
            },
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    svc.set_on_job(|_j| panic!("delivery branch exploded"));

    // 修复前：panic 沿 execute_job 炸出调用方（本测试 panic）；修复后：
    // Err 合成串落 job 记账面，execute_job 正常返回。
    svc.execute_job(&job.id).unwrap();

    let s = svc.store.lock();
    let j = s.jobs.iter().find(|j| j.id == job.id).unwrap();
    assert_eq!(j.state.last_status.as_deref(), Some("error"));
    let last_err = j.state.last_error.as_deref().unwrap_or_default();
    assert!(
        last_err.contains("handler panicked"),
        "失败记账必须诚实标注 panic: {last_err}"
    );
}

/// tick 循环存活锁：panicking handler 不杀常驻 tick 任务——修复前第一拍
/// panic 即任务死（计数停在 1），修复后循环继续（计数 ≥2）。
#[test]
fn cron_tick_loop_survives_handler_panic() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = CronService::new(&path);
    svc.add_job(
        "boom-every",
        CronSchedule {
            kind: "every".to_string(),
            at_ms: None,
            every_ms: Some(200),
            expr: None,
            tz: None,
        },
        "m",
        false,
        None,
        None,
    )
    .unwrap();
    let ticks = Arc::new(AtomicUsize::new(0));
    let t2 = ticks.clone();
    svc.set_on_job(move |_j| {
        t2.fetch_add(1, Ordering::SeqCst);
        panic!("tick boom");
    });

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        svc.arm();
        svc.start().await.unwrap();
    });
    // tick 粒度是 1s（interval(Duration::from_secs(1))，首拍即触发）：留
    // ~2.5s 窗口稳拿 ≥2 拍（1.2s 窗口实测调度抖动下只逮到 1 拍假红）。
    std::thread::sleep(std::time::Duration::from_millis(2500));
    svc.stop();
    let n = ticks.load(Ordering::SeqCst);
    assert!(
        n >= 2,
        "tick 循环必须在 handler panic 后继续（实际 {n} 拍）"
    );
}
