//! W1b batch (Phase 3 batch 16): nemesis-cron gap tests.
//!
//! Targets the uncovered surface of `service.rs` found by auditing the
//! existing 140-test suite:
//! - `patch_job` full field matrix (only `max_rounds` was tested)
//! - `CronJobState::push_history` MAX_HISTORY cap + serde `#[serde(default)]`
//! - `add_job_ext(enabled=false)` next_run semantics
//! - fire-loop branches: `at` job with `delete_after_run=false` (disable after
//!   fire), handler error on a `delete_after_run` job (retain), history append
//! - `execute_job` error branch + "does not advance next_run" contract
//! - `compute_next_run` invalid-tz fallback, save error propagation,
//!   `toggle_job` disable asymmetry (stale next_run), whitespace validate

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn every_schedule(ms: i64) -> CronSchedule {
    CronSchedule {
        kind: "every".to_string(),
        at_ms: None,
        every_ms: Some(ms),
        expr: None,
        tz: None,
    }
}

fn cron_schedule(expr: &str) -> CronSchedule {
    CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(expr.to_string()),
        tz: None,
    }
}

fn at_schedule(at_ms: i64) -> CronSchedule {
    CronSchedule {
        kind: "at".to_string(),
        at_ms: Some(at_ms),
        every_ms: None,
        expr: None,
        tz: None,
    }
}

fn tmp_store_path(tag: &str) -> (tempfile::TempDir, String) {
    // Keep the TempDir alive in the caller; drop cleans up.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(tag).to_string_lossy().to_string();
    (dir, path)
}

// ---------------------------------------------------------------------------
// patch_job field matrix (web `tasks.cron.update` path)
// ---------------------------------------------------------------------------

#[test]
fn test_w1b_patch_job_name_and_message() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job("orig", every_schedule(60000), "orig msg", false, None, None)
        .unwrap();

    let patched = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                name: Some("renamed".to_string()),
                message: Some("new message".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(patched.name, "renamed");
    assert_eq!(patched.payload.message, "new message");
    // Untouched fields survive.
    assert!(!patched.payload.deliver);

    // Persisted: fresh service sees the patched values.
    let svc2 = CronService::new(&path);
    let reloaded = svc2.get_job(&job.id).unwrap();
    assert_eq!(reloaded.name, "renamed");
    assert_eq!(reloaded.payload.message, "new message");
}

#[test]
fn test_w1b_patch_job_channel_to_session_key_set_and_clear() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "j",
            every_schedule(60000),
            "m",
            true,
            Some("web"),
            Some("chat1"),
        )
        .unwrap();
    assert_eq!(job.payload.channel.as_deref(), Some("web"));
    assert_eq!(job.payload.to.as_deref(), Some("chat1"));
    assert_eq!(job.payload.session_key, None);

    // Set session_key, change channel/to.
    let patched = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                channel: Some("telegram".to_string()),
                to: Some("user9".to_string()),
                session_key: Some("agent:main:session:s1".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(patched.payload.channel.as_deref(), Some("telegram"));
    assert_eq!(patched.payload.to.as_deref(), Some("user9"));
    assert_eq!(
        patched.payload.session_key.as_deref(),
        Some("agent:main:session:s1")
    );

    // `Some("")` clears all three nullable string fields.
    let cleared = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                channel: Some(String::new()),
                to: Some(String::new()),
                session_key: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(cleared.payload.channel, None);
    assert_eq!(cleared.payload.to, None);
    assert_eq!(cleared.payload.session_key, None);
}

#[test]
fn test_w1b_patch_job_enabled_recomputes_next_run() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job("j", every_schedule(60000), "m", false, None, None)
        .unwrap();
    assert!(job.state.next_run_at_ms.is_some());

    // Disable via patch: next_run cleared.
    let off = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!off.enabled);
    assert_eq!(off.state.next_run_at_ms, None);
    // Disabled job hidden from default listing.
    assert!(svc.list_jobs(false).is_empty());
    assert_eq!(svc.list_jobs(true).len(), 1);

    // Re-enable via patch: next_run recomputed fresh (>= now).
    let before_ms = Local::now().timestamp_millis();
    let on = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(on.enabled);
    let next = on
        .state
        .next_run_at_ms
        .expect("re-enabled recomputes next_run");
    assert!(
        next >= before_ms,
        "next_run must be recomputed, got {}",
        next
    );
}

#[test]
fn test_w1b_patch_job_schedule_enabled_recomputes_disabled_does_not() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job("j", every_schedule(60000), "m", false, None, None)
        .unwrap();
    let old_next = job.state.next_run_at_ms.unwrap();

    // Enabled job: schedule patch recomputes next_run against the new schedule.
    let patched = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                schedule: Some(every_schedule(3_600_000)),
                ..Default::default()
            },
        )
        .unwrap();
    let new_next = patched.state.next_run_at_ms.unwrap();
    assert!(
        new_next > old_next,
        "1h schedule must schedule further out than 1m: {} vs {}",
        new_next,
        old_next
    );

    // Disabled job: schedule patch must NOT arm a next_run.
    svc.patch_job(
        &job.id,
        &CronJobPatch {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    let patched_disabled = svc
        .patch_job(
            &job.id,
            &CronJobPatch {
                schedule: Some(cron_schedule("* * * * *")),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!patched_disabled.enabled);
    assert_eq!(
        patched_disabled.state.next_run_at_ms, None,
        "schedule patch on a disabled job must not arm next_run"
    );
}

#[test]
fn test_w1b_patch_job_not_found_err() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let err = svc.patch_job("nope", &CronJobPatch::default()).unwrap_err();
    assert!(err.contains("job not found"), "got: {}", err);
}

#[test]
fn test_w1b_cron_job_patch_serde_roundtrip() {
    // The web handler deserializes CronJobPatch from JSON — pin the wire shape.
    let patch = CronJobPatch {
        name: Some("n".to_string()),
        schedule: Some(cron_schedule("0 9 * * *")),
        message: Some("m".to_string()),
        channel: Some("web".to_string()),
        to: Some("t".to_string()),
        session_key: Some("sk".to_string()),
        enabled: Some(true),
        max_rounds: Some(Some(5)),
    };
    let json_str = serde_json::to_string(&patch).unwrap();
    let back: CronJobPatch = serde_json::from_str(&json_str).unwrap();
    assert_eq!(back.name.as_deref(), Some("n"));
    assert_eq!(
        back.schedule.as_ref().unwrap().expr.as_deref(),
        Some("0 9 * * *")
    );
    assert_eq!(back.enabled, Some(true));
    assert_eq!(back.max_rounds, Some(Some(5)));

    // Full empty patch (web sends `{}`) → Default::default() equivalent.
    let empty: CronJobPatch = serde_json::from_str("{}").unwrap();
    assert_eq!(empty.name, None);
    assert_eq!(empty.max_rounds, None);
    assert_eq!(empty.enabled, None);

    // Three-state max_rounds wire forms.
    let cleared: CronJobPatch = serde_json::from_str(r#"{"max_rounds": null}"#).unwrap();
    assert_eq!(
        cleared.max_rounds, None,
        "absent key and null are both None"
    );
    let set: CronJobPatch = serde_json::from_str(r#"{"max_rounds": 7}"#).unwrap();
    assert_eq!(set.max_rounds, Some(Some(7)));
}

// ---------------------------------------------------------------------------
// History cap + serde defaults
// ---------------------------------------------------------------------------

#[test]
fn test_w1b_push_history_caps_at_max_history() {
    let mut st = CronJobState {
        next_run_at_ms: None,
        last_run_at_ms: None,
        last_status: None,
        last_error: None,
        history: Vec::new(),
    };
    for i in 0..(MAX_HISTORY + 10) {
        st.push_history(i as i64, "ok".to_string(), None);
    }
    assert_eq!(st.history.len(), MAX_HISTORY);
    // Oldest survivors are the newest MAX_HISTORY entries: first kept = #10.
    assert_eq!(st.history[0].at_ms, 10);
    assert_eq!(st.history.last().unwrap().at_ms, (MAX_HISTORY + 9) as i64);
    // Error entries carry their error text.
    st.push_history(999, "error".to_string(), Some("boom".to_string()));
    assert_eq!(st.history.last().unwrap().status, "error");
    assert_eq!(st.history.last().unwrap().error.as_deref(), Some("boom"));
    assert_eq!(st.history.len(), MAX_HISTORY, "cap holds after error push");
}

#[test]
fn test_w1b_history_serde_default_old_store_json() {
    // Jobs persisted before `history` existed must load with empty history
    // (the `#[serde(default)]` contract), same for payload session_key/max_rounds.
    let (_dir, path) = tmp_store_path("cron.json");
    let legacy = serde_json::json!({
        "version": 1,
        "jobs": [{
            "id": "old1",
            "name": "legacy",
            "enabled": true,
            "schedule": {"kind": "every", "at_ms": null, "every_ms": 60000, "expr": null, "tz": null},
            "payload": {"kind": "agent_turn", "message": "m", "command": null,
                         "deliver": false, "channel": null, "to": null},
            "state": {"next_run_at_ms": 4102444800000i64, "last_run_at_ms": null,
                       "last_status": null, "last_error": null},
            "created_at_ms": 1,
            "updated_at_ms": 1,
            "delete_after_run": false
        }]
    });
    std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();

    let svc = CronService::new(&path);
    let jobs = svc.list_jobs(true);
    assert_eq!(jobs.len(), 1);
    assert!(
        jobs[0].state.history.is_empty(),
        "old store → empty history"
    );
    assert_eq!(jobs[0].payload.session_key, None);
    assert_eq!(jobs[0].payload.max_rounds, None);
}

#[test]
fn test_w1b_store_roundtrip_preserves_session_key_max_rounds_history() {
    let (_dir, path) = tmp_store_path("cron.json");
    {
        let svc = CronService::new(&path);
        let job = svc
            .add_job_ext(
                "j",
                every_schedule(60000),
                "m",
                true,
                Some("web"),
                Some("chat1"),
                Some("agent:main:session:sid42"),
                Some(20),
                true,
            )
            .unwrap();
        // Two manual runs → history ["executed", "error"].
        svc.set_on_job(|_j| Ok("done".to_string()));
        svc.execute_job(&job.id).unwrap();
        svc.set_on_job(|_j| Err("late failure".to_string()));
        svc.execute_job(&job.id).unwrap();
        let j = svc.get_job(&job.id).unwrap();
        assert_eq!(j.state.history.len(), 2);
        assert_eq!(j.state.history[0].status, "executed");
        assert_eq!(j.state.history[1].status, "error");
    }

    // Fresh service from the same store: everything survives.
    let svc2 = CronService::new(&path);
    let job = svc2.get_job(&svc2.list_jobs(true)[0].id.clone()).unwrap();
    assert_eq!(
        job.payload.session_key.as_deref(),
        Some("agent:main:session:sid42")
    );
    assert_eq!(job.payload.max_rounds, Some(20));
    assert_eq!(job.state.history.len(), 2);
    assert_eq!(job.state.history[1].status, "error");
    assert_eq!(job.state.history[1].error.as_deref(), Some("late failure"));
    assert_eq!(job.state.last_status.as_deref(), Some("error"));
}

// ---------------------------------------------------------------------------
// add_job_ext enabled=false
// ---------------------------------------------------------------------------

#[test]
fn test_w1b_add_job_ext_disabled_no_next_run_and_hidden_from_default_list() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job_ext(
            "paused",
            every_schedule(60000),
            "m",
            false,
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();
    assert!(!job.enabled);
    assert_eq!(
        job.state.next_run_at_ms, None,
        "disabled job must not be scheduled at creation"
    );
    assert!(
        svc.list_jobs(false).is_empty(),
        "hidden from default listing"
    );
    assert_eq!(
        svc.list_jobs(true).len(),
        1,
        "visible with include_disabled"
    );
    // status() must not report a nextWake for the disabled job.
    assert_eq!(svc.status()["nextWakeAtMS"], serde_json::json!(null));
}

// ---------------------------------------------------------------------------
// compute_next_run / validate edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_w1b_compute_next_run_invalid_tz_uses_local() {
    let now_ms = Local::now().timestamp_millis();
    let bogus_tz = CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some("0 9 * * *".to_string()),
        tz: Some("Definitely/Not_AZone".to_string()),
    };
    let next = compute_next_run(&bogus_tz, now_ms)
        .expect("invalid tz falls back to local, still schedules");
    assert!(next > now_ms);
}

#[test]
fn test_w1b_validate_schedule_whitespace_only() {
    assert!(CronService::validate_schedule("   ").is_err());
    assert!(CronService::validate_schedule("\t\n").is_err());
}

#[test]
fn test_w1b_add_job_store_write_error_propagates() {
    // Parent of the store path is a regular FILE → create_dir_all fails →
    // add_job must surface the error instead of silently dropping the job.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker.txt");
    std::fs::write(&blocker, "i am a file").unwrap();
    let bad_path = blocker.join("cron.json").to_string_lossy().to_string();

    let svc = CronService::new(&bad_path);
    let err = svc
        .add_job("j", every_schedule(60000), "m", false, None, None)
        .unwrap_err();
    assert!(err.contains("mkdir"), "expected mkdir error, got: {}", err);
    // The job must NOT be in the in-memory store after a failed persist.
    assert!(svc.list_jobs(true).is_empty());
}

#[test]
fn test_w1b_mutators_roll_back_on_save_failure() {
    // BUG #16 sweep: update/patch/toggle/enable must roll their in-memory
    // mutation back when persistence fails — otherwise a caller retrying
    // after Err sees the "failed" mutation applied (toggle would flip back).
    let (_dir, path) = tmp_store_path("cron.json");
    let mut svc = CronService::new(&path);
    let job = svc
        .add_job("j", every_schedule(60000), "m", false, None, None)
        .unwrap();
    // `store_path` is a private field of CronService, reachable from this
    // child module of `service`.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker.txt");
    std::fs::write(&blocker, "x").unwrap();
    svc.store_path = blocker.join("cron.json").to_string_lossy().to_string();

    // toggle: Err + enabled unchanged (still true).
    assert!(svc.toggle_job(&job.id).is_err());
    assert!(
        svc.get_job(&job.id).unwrap().enabled,
        "toggle rollback must restore pre-toggle state"
    );

    // patch: Err + fields unchanged.
    assert!(
        svc.patch_job(
            &job.id,
            &CronJobPatch {
                name: Some("ghost".to_string()),
                enabled: Some(false),
                ..Default::default()
            }
        )
        .is_err()
    );
    let after = svc.get_job(&job.id).unwrap();
    assert_eq!(after.name, "j", "patch rollback must restore name");
    assert!(after.enabled, "patch rollback must restore enabled");

    // update: Err + name/schedule unchanged.
    assert!(
        svc.update_job(&job.id, Some("ghost2"), Some(cron_schedule("* * * * *")))
            .is_err()
    );
    let after = svc.get_job(&job.id).unwrap();
    assert_eq!(after.name, "j", "update rollback must restore name");
    assert_eq!(
        after.schedule.kind, "every",
        "update rollback must restore schedule"
    );

    // enable(false): Err + still enabled with next_run intact.
    assert!(svc.enable_job(&job.id, false).is_err());
    let after = svc.get_job(&job.id).unwrap();
    assert!(after.enabled, "enable rollback must restore enabled");
    assert!(
        after.state.next_run_at_ms.is_some(),
        "enable rollback must restore next_run"
    );
}

// ---------------------------------------------------------------------------
// execute_job (manual "run now") contracts
// ---------------------------------------------------------------------------

#[test]
fn test_w1b_execute_job_error_status_and_history() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job("j", every_schedule(60000), "m", false, None, None)
        .unwrap();
    svc.set_on_job(|_j| Err("manual boom".to_string()));

    svc.execute_job(&job.id).unwrap();
    let updated = svc.get_job(&job.id).unwrap();
    assert_eq!(updated.state.last_status.as_deref(), Some("error"));
    assert_eq!(updated.state.last_error.as_deref(), Some("manual boom"));
    assert_eq!(updated.state.history.len(), 1);
    assert_eq!(updated.state.history[0].status, "error");
    assert_eq!(
        updated.state.history[0].error.as_deref(),
        Some("manual boom")
    );
}

#[test]
fn test_w1b_execute_job_does_not_advance_next_run() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job("j", every_schedule(3_600_000), "m", false, None, None)
        .unwrap();
    let before = job.state.next_run_at_ms.unwrap();

    svc.set_on_job(|_j| Ok("ok".to_string()));
    svc.execute_job(&job.id).unwrap();

    let updated = svc.get_job(&job.id).unwrap();
    assert_eq!(
        updated.state.next_run_at_ms,
        Some(before),
        "manual run must NOT advance the schedule (documented contract)"
    );
    assert_eq!(updated.state.last_status.as_deref(), Some("executed"));
    assert_eq!(
        updated.state.history.last().unwrap().status,
        "executed",
        "manual run history status is 'executed', not 'ok'"
    );
}

// ---------------------------------------------------------------------------
// toggle_job disable = clear next_run（BUG-20261025-30 反转旧钉死语义）
// ---------------------------------------------------------------------------

#[test]
fn test_w1b_toggle_disable_clears_next_run() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job("j", every_schedule(3_600_000), "m", false, None, None)
        .unwrap();
    let next_before = job.state.next_run_at_ms.unwrap();

    // BUG-20261025-30：旧实现 toggle_job 停用不清 next_run（当时评估为
    // 「无害：fire loop + status 都按 enabled 过滤」并钉死）；实际 tasks.rs
    // 的作业 JSON 直接透传 next_run_at_ms，禁用作业带 stale 值 = UI 幻影
    // 「下次触发」。现与 enable_job / patch_job 对齐（W1b 不变量：disabled
    // 作业不排程），停用即清除。
    let now_disabled = svc.toggle_job(&job.id).unwrap();
    assert!(!now_disabled);
    let disabled = svc.get_job(&job.id).unwrap();
    assert_eq!(
        disabled.state.next_run_at_ms, None,
        "toggle_job disable must clear next_run (W1b alignment)"
    );
    // status() keeps ignoring disabled jobs either way.
    assert_eq!(svc.status()["nextWakeAtMS"], serde_json::json!(null));

    // Re-toggle recomputes fresh (gate passes: schedule can fire).
    svc.toggle_job(&job.id).unwrap();
    let re_enabled = svc.get_job(&job.id).unwrap();
    let fresh = re_enabled.state.next_run_at_ms.unwrap();
    assert!(fresh >= next_before);
    assert!(svc.status()["nextWakeAtMS"].is_number());
}

// ---------------------------------------------------------------------------
// BUG-20261025-41 回归锁：触发期停用 + delete_after_run 派生
// ---------------------------------------------------------------------------

/// BUG-20261025-41a：作业在 handler 执行期内被停用（W3 pause_job 生产路径
/// ——触发期父会话已删除自动暂停，pause 落盘先于 fire loop 的 post-handler
/// 段）时，post-handler 不得为已停用作业重算 next_run——W1b 不变量
/// （disabled 作业不排程）必须跨「触发路径」同样成立，幻影 next_run 会
/// 持久化且跨重启存留（recompute_next_runs 只碰 enabled 作业）。
#[tokio::test]
async fn test_bug41_fire_loop_pause_in_handler_leaves_no_phantom_next_run() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let pause_handle = svc.job_pause_handle();
    let fired = Arc::new(AtomicUsize::new(0));
    let paused_out = Arc::new(AtomicUsize::new(0));
    let fired2 = fired.clone();
    let paused2 = paused_out.clone();
    svc.set_on_job(move |job: &CronJob| {
        fired2.fetch_add(1, Ordering::SeqCst);
        // 生产 W3 语义镜像：pause 自己（enabled=false 落盘）+ 诚实 Err。
        let paused = pause_handle.pause_job(&job.id);
        paused2.store(if paused { 1 } else { 2 }, Ordering::SeqCst);
        Err("[定时任务] 已自动暂停".to_string())
    });

    let job = svc
        .add_job("pausable", every_schedule(1000), "m", false, None, None)
        .unwrap();
    assert!(job.enabled);
    assert!(job.state.next_run_at_ms.is_some());

    svc.arm();
    svc.start().await.unwrap();
    // 最多等 8s 观察首次触发（1s tick + 1s 间隔，留足慢机余量）。
    let mut observed = false;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if svc
            .get_job(&job.id)
            .is_some_and(|j| j.state.last_run_at_ms.is_some())
        {
            observed = true;
            break;
        }
    }
    svc.stop();

    assert!(observed, "job must have fired within the wait window");
    assert_eq!(
        paused_out.load(Ordering::SeqCst),
        1,
        "handler must have paused the job via the pause handle"
    );
    let after = svc.get_job(&job.id).expect("paused job must be retained");
    assert!(
        !after.enabled,
        "pause during handler must leave job disabled"
    );
    assert_eq!(
        after.state.next_run_at_ms, None,
        "post-handler must NOT recompute next_run for a job disabled during its own handler (W1b)"
    );
    assert_eq!(after.state.last_status.as_deref(), Some("error"));
}

/// BUG-20261025-41b：delete_after_run 是 kind=="at" 的派生旗标，换调度必须
/// 重算——at→周期 改期若保留旧 true，任务首次成功触发即被 retain 静默删除
/// （周期任务无声消失）。update_job 与 patch_job 两条改期路径都钉。
#[test]
fn test_bug41_schedule_change_rederives_delete_after_run() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let job = svc
        .add_job(
            "one_shot",
            at_schedule(Local::now().timestamp_millis() + 3_600_000),
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.delete_after_run, "at 创建即带单发删除旗标");

    // update_job：at → every，旗标必须随 kind 重算为 false。
    svc.update_job(&job.id, None, Some(every_schedule(60_000)))
        .unwrap();
    let upd = svc.get_job(&job.id).unwrap();
    assert_eq!(upd.schedule.kind, "every");
    assert!(
        !upd.delete_after_run,
        "update at→every must re-derive delete_after_run=false (else first ok fire deletes the recurring job)"
    );

    // patch_job：every → cron，旗标保持 false。
    svc.patch_job(
        &job.id,
        &CronJobPatch {
            schedule: Some(cron_schedule("*/5 * * * *")),
            ..Default::default()
        },
    )
    .unwrap();
    let patched = svc.get_job(&job.id).unwrap();
    assert!(
        !patched.delete_after_run,
        "patch to cron keeps delete_after_run=false"
    );

    // 反向：周期 → at（未来时刻，过死排程闸），旗标重算回 true（单发语义恢复）。
    svc.update_job(
        &job.id,
        None,
        Some(at_schedule(Local::now().timestamp_millis() + 3_600_000)),
    )
    .unwrap();
    let back = svc.get_job(&job.id).unwrap();
    assert_eq!(back.schedule.kind, "at");
    assert!(
        back.delete_after_run,
        "update back to at re-derives delete_after_run=true"
    );
}

// ---------------------------------------------------------------------------
// Fire-loop branches (async, short waits)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w1b_fire_loop_at_job_without_delete_disables_after_fire() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    let fired = Arc::new(AtomicUsize::new(0));
    let fired2 = fired.clone();
    svc.set_on_job(move |_j| {
        fired2.fetch_add(1, Ordering::SeqCst);
        Ok("ok".to_string())
    });

    // "at" job due ~1.2s out; add_job forces delete_after_run=true for kind
    // "at", so flip it to false to hit the disable-after-fire branch.
    let job = svc
        .add_job(
            "one_shot_keep",
            at_schedule(Local::now().timestamp_millis() + 1200),
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.delete_after_run);
    {
        let mut s = svc.store.lock();
        s.jobs
            .iter_mut()
            .find(|j| j.id == job.id)
            .unwrap()
            .delete_after_run = false;
    }

    svc.arm();
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(3500)).await;
    svc.stop();

    assert!(fired.load(Ordering::SeqCst) >= 1, "job must have fired");
    let after = svc.get_job(&job.id).expect("non-delete at job is retained");
    assert!(
        !after.enabled,
        "fired at-job without delete_after_run disables itself"
    );
    assert_eq!(after.state.next_run_at_ms, None);
    assert_eq!(after.state.last_status.as_deref(), Some("ok"));
    assert_eq!(after.state.history.len(), 1);
}

#[tokio::test]
async fn test_w1b_fire_loop_delete_after_run_error_keeps_job() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    svc.set_on_job(|_j| Err("handler exploded".to_string()));

    // delete_after_run "at" job whose handler FAILS must be retained —
    // the retain guard requires last_status == "ok" before removal.
    let job = svc
        .add_job(
            "failing_one_shot",
            at_schedule(Local::now().timestamp_millis() + 1200),
            "m",
            false,
            None,
            None,
        )
        .unwrap();
    assert!(job.delete_after_run);

    svc.arm();
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(3500)).await;
    svc.stop();

    let after = svc
        .get_job(&job.id)
        .expect("failed delete_after_run job must NOT be removed");
    assert_eq!(after.state.last_status.as_deref(), Some("error"));
    assert_eq!(after.state.last_error.as_deref(), Some("handler exploded"));
    assert_eq!(after.state.history.len(), 1);
    assert_eq!(after.state.history[0].status, "error");
    // BUG-HUNT R34：留证必须伴随停摆——enabled=true + 过期 at_ms 的僵尸
    // 会在下次启动被 recompute_next_runs 的 H1 子句复活，每次重启重放。
    assert!(
        !after.enabled,
        "failed delete_after_run job must be disabled (evidence kept, no restart replay)"
    );
    assert_eq!(after.state.next_run_at_ms, None);
}

#[tokio::test]
async fn test_w1b_fire_loop_appends_history_entries() {
    let (_dir, path) = tmp_store_path("cron.json");
    let svc = CronService::new(&path);
    svc.set_on_job(|_j| Ok("ok".to_string()));
    let job = svc
        .add_job("ticker", every_schedule(1000), "m", false, None, None)
        .unwrap();

    svc.arm();
    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(3300)).await;
    svc.stop();

    let after = svc.get_job(&job.id).unwrap();
    assert!(
        after.state.history.len() >= 2,
        "1s-interval job over ~3s must record >=2 history entries, got {}",
        after.state.history.len()
    );
    for rec in &after.state.history {
        assert_eq!(rec.status, "ok");
        assert!(rec.error.is_none());
    }
    // History is chronological (newest last): at_ms non-decreasing.
    for w in after.state.history.windows(2) {
        assert!(w[0].at_ms <= w[1].at_ms, "history must be ordered");
    }
    // last_run tracks the newest history entry.
    assert_eq!(
        after.state.last_run_at_ms,
        Some(after.state.history.last().unwrap().at_ms)
    );
}
