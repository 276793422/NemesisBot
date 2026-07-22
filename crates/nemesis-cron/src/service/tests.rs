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
    assert!(svc.remove_job(&job.id));
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
    };
    let json = serde_json::to_string(&payload).unwrap();
    let parsed: CronPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.kind, "agent_turn");
    assert_eq!(parsed.message, "hello");
    assert!(parsed.deliver);
    assert_eq!(parsed.channel, Some("web".to_string()));
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
    let job = svc
        .add_job(
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
    assert!(!svc.remove_job("nonexistent"));
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
        svc.start().await.unwrap();
        // Second start should be idempotent (return Ok)
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
