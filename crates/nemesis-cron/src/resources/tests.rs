//! cron 资源盘点 provider 测试——三态 / 会话过滤 / 摘要形状。

use super::*;
use crate::service::{CronSchedule, CronService};
use std::path::Path;

fn ctx_for(session_key: &str) -> ResourceCtx<'_> {
    ResourceCtx {
        session_key,
        bare_sid: "s1",
        workspace_root: Path::new("C:\\ws"),
    }
}

fn every_schedule() -> CronSchedule {
    CronSchedule {
        kind: "every".into(),
        at_ms: None,
        every_ms: Some(60_000),
        expr: None,
        tz: None,
    }
}

#[test]
fn unavailable_when_service_absent() {
    let p = CronResourceProvider::new(None);
    let view = p.collect(&ctx_for("agent:main:session:s1"));
    assert_eq!(view.status.as_str(), "unavailable");
    assert!(view.status.unavailable_reason().is_some());
    assert_eq!(p.meta().category_id, "cron");
    assert_eq!(p.meta().label, "定时器");
    assert!(!p.meta().transient);
}

#[test]
fn empty_when_no_jobs_match_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = Arc::new(Mutex::new(CronService::new(&path)));
    // 不绑会话的 job（session_key=None）与绑别的会话的 job 都不该出现。
    svc.lock()
        .unwrap()
        .add_job("free", every_schedule(), "m", false, None, None)
        .unwrap();
    svc.lock()
        .unwrap()
        .add_job_ext(
            "bound",
            every_schedule(),
            "m",
            false,
            None,
            None,
            Some("agent:main:session:other"),
            None,
            true,
        )
        .unwrap();

    let p = CronResourceProvider::new(Some(svc));
    let view = p.collect(&ctx_for("agent:main:session:s1"));
    assert_eq!(view.status.as_str(), "empty");
    assert_eq!(view.total, 0);
}

#[test]
fn ok_lists_only_this_session_jobs_with_summary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = Arc::new(Mutex::new(CronService::new(&path)));
    let key = "agent:main:session:s1";
    let j1 = svc
        .lock()
        .unwrap()
        .add_job_ext(
            "mine-a",
            every_schedule(),
            "m",
            false,
            None,
            None,
            Some(key),
            None,
            true,
        )
        .unwrap();
    let j2 = svc
        .lock()
        .unwrap()
        .add_job_ext(
            "mine-b",
            every_schedule(),
            "m",
            false,
            None,
            None,
            Some(key),
            None,
            false,
        )
        .unwrap();
    svc.lock()
        .unwrap()
        .add_job_ext(
            "not-mine",
            every_schedule(),
            "m",
            false,
            None,
            None,
            Some("agent:main:session:other"),
            None,
            true,
        )
        .unwrap();

    let p = CronResourceProvider::new(Some(svc));
    let view = p.collect(&ctx_for(key));
    assert_eq!(view.status.as_str(), "ok");
    assert_eq!(
        view.total, 2,
        "禁用的定时器也照常盘点（enabled 进 summary）"
    );
    let jobs = view.summary["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 2);
    let names: Vec<&str> = jobs.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"mine-a") && names.contains(&"mine-b"));
    assert_eq!(jobs[0]["id"], json!(j1.id.as_str()));
    // disabled 位投影在行内。
    let b = jobs.iter().find(|r| r["name"] == "mine-b").unwrap();
    assert_eq!(b["enabled"], json!(false));
    let _ = j2;
    // 无关会话的 job 不进行。
    assert!(!names.contains(&"not-mine"));
    assert_eq!(view.summary["omitted"], json!(0));
}

#[test]
fn summary_capped_at_five_rows_with_omitted_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cron.json").to_string_lossy().to_string();
    let svc = Arc::new(Mutex::new(CronService::new(&path)));
    let key = "agent:main:session:s1";
    for i in 0..7 {
        svc.lock()
            .unwrap()
            .add_job_ext(
                &format!("job-{i}"),
                every_schedule(),
                "m",
                false,
                None,
                None,
                Some(key),
                None,
                true,
            )
            .unwrap();
    }
    let p = CronResourceProvider::new(Some(svc));
    let view = p.collect(&ctx_for(key));
    assert_eq!(view.total, 7);
    assert_eq!(view.summary["jobs"].as_array().unwrap().len(), 5);
    assert_eq!(view.summary["omitted"], json!(2));
}
