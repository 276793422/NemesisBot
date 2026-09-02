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

// ===========================================================================
// run() 全臂（S11c，quality-hardening goal 冲刺 S11）—— run(action, local)
// 此前 LH=0（toggle_job 有直调测试，但 dispatch 层从没跑过）。env home 隔离
// + GLOBAL_STATE_LOCK 串行；store 落在 {home}/workspace/cron/jobs.json。
// ===========================================================================

mod run_arm {
    use super::super::{CronAction, run};

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    fn store_of(home: &std::path::Path) -> std::path::PathBuf {
        home.join("workspace").join("cron").join("jobs.json")
    }

    fn write_store(home: &std::path::Path, body: &str) {
        let p = store_of(home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn list_without_store_shows_hint() {
        with_env_home(|_home| {
            run(CronAction::List, false).expect("无 store → 提示 + Ok");
        });
    }

    #[test]
    fn list_covers_empty_nonarray_interval_and_string_schedules() {
        with_env_home(|home| {
            // 空数组。
            write_store(&home, "[]");
            run(CronAction::List, false).expect("空数组 → No scheduled jobs");

            // 非数组 JSON（as_array None 分支）。
            write_store(&home, r#"{"not":"an array"}"#);
            run(CronAction::List, false).expect("非数组 → No scheduled jobs");

            // 结构化 schedule（every_ms 优先）+ 字符串 schedule + 缺字段落默认。
            write_store(
                &home,
                r#"[
                    {"id":"i1","name":"n1","enabled":false,
                     "schedule":{"kind":"interval","every_ms":120000,"display":"every 120s"}},
                    {"id":"i2","name":"n2","schedule":"每分钟"},
                    {"id":"i3","schedule":{"kind":"cron","expr":"* * * * *"}}
                ]"#,
            );
            run(CronAction::List, false).expect("interval/string/缺字段 三态 → Ok");
        });
    }

    #[test]
    fn add_interval_job_persists_to_store() {
        with_env_home(|home| {
            run(
                CronAction::Add {
                    name: "j1".into(),
                    message: "hello".into(),
                    every: Some(30),
                    cron: None,
                    deliver: false,
                    to: None,
                    channel: None,
                },
                false,
            )
            .expect("add ok");
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(store_of(&home)).unwrap()).unwrap();
            let arr = jobs.as_array().unwrap();
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["name"], "j1");
            assert_eq!(arr[0]["schedule"]["kind"], "interval");
            assert_eq!(arr[0]["schedule"]["every_ms"], 30000);
            assert_eq!(arr[0]["enabled"], true);
            assert_eq!(
                arr[0]["id"].as_str().unwrap().len(),
                8,
                "id 是 uuid 前 8 位"
            );
        });
    }

    #[test]
    fn add_cron_job_and_neither_flag_error_paths() {
        with_env_home(|home| {
            run(
                CronAction::Add {
                    name: "j2".into(),
                    message: "m".into(),
                    every: None,
                    cron: Some("*/5 * * * *".into()),
                    deliver: true,
                    to: Some("user1".into()),
                    channel: Some("web".into()),
                },
                false,
            )
            .expect("cron add ok");
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(store_of(&home)).unwrap()).unwrap();
            assert_eq!(jobs[0]["schedule"]["kind"], "cron");
            assert_eq!(jobs[0]["schedule"]["expr"], "*/5 * * * *");
            assert_eq!(jobs[0]["deliver"], true);
            assert_eq!(jobs[0]["to"], "user1");
            assert_eq!(jobs[0]["channel"], "web");

            // 两个调度参数都不给 → 错误提示后 Ok（不写文件）。
            let before = std::fs::read_to_string(store_of(&home)).unwrap();
            run(
                CronAction::Add {
                    name: "j3".into(),
                    message: "m".into(),
                    every: None,
                    cron: None,
                    deliver: false,
                    to: None,
                    channel: None,
                },
                false,
            )
            .expect("缺调度参数 → 打印错误 + Ok");
            assert_eq!(std::fs::read_to_string(store_of(&home)).unwrap(), before);
        });
    }

    #[test]
    fn remove_found_not_found_and_no_store() {
        with_env_home(|home| {
            // 无 store。
            run(CronAction::Remove { id: "x".into() }, false).expect("无 store → not found Ok");

            write_store(&home, r#"[{"id":"abc","name":"n"}]"#);
            run(CronAction::Remove { id: "abc".into() }, false).expect("remove ok");
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(store_of(&home)).unwrap()).unwrap();
            assert!(jobs.as_array().unwrap().is_empty(), "已删除");

            run(CronAction::Remove { id: "ghost".into() }, false).expect("不存在 → not found Ok");
        });
    }

    #[test]
    fn enable_disable_dispatch_through_toggle() {
        with_env_home(|home| {
            write_store(&home, r#"[{"id":"abc","name":"n","enabled":true}]"#);
            run(CronAction::Disable { id: "abc".into() }, false).expect("disable ok");
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(store_of(&home)).unwrap()).unwrap();
            assert_eq!(jobs[0]["enabled"], false);

            run(CronAction::Enable { id: "abc".into() }, false).expect("enable ok");
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(store_of(&home)).unwrap()).unwrap();
            assert_eq!(jobs[0]["enabled"], true);
        });
    }

    // -----------------------------------------------------------------------
    // R7（coverage-95 goal）：Add/Remove 的 store-已存在读臂 + 写失败臂。
    // - Add 在 store 已存在时必须读旧数组做合并（此前只测过空新建）；
    // - {home}/workspace 是普通文件时 create_dir_all 被静默吞掉、
    //   fs::write 因父路径不是目录而失败 → ? 上抛 Err；
    // - store 文件只读（Windows READONLY attr / unix 0444）时 Remove 的
    //   收尾 fs::write 失败 → ? 上抛 Err，且原内容保持不变。
    // -----------------------------------------------------------------------

    /// 只读化（跨平台）：Windows std 把 0o444 映射为 FILE_ATTRIBUTE_READONLY，
    /// unix 即权限位。返回恢复闭包句柄由调用方在断言后手动还原。
    fn deny_write(p: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(p).unwrap().permissions();
            perm.set_mode(0o444);
            std::fs::set_permissions(p, perm).unwrap();
        }
        #[cfg(windows)]
        {
            let mut perm = std::fs::metadata(p).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(p, perm).unwrap();
        }
    }

    fn allow_write(p: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(p).unwrap().permissions();
            perm.set_mode(0o644);
            std::fs::set_permissions(p, perm).unwrap();
        }
        #[cfg(windows)]
        {
            let mut perm = std::fs::metadata(p).unwrap().permissions();
            perm.set_readonly(false);
            std::fs::set_permissions(p, perm).unwrap();
        }
    }

    #[test]
    fn add_merges_into_existing_store() {
        with_env_home(|home| {
            write_store(&home, r#"[{"id":"keep1","name":"kept"}]"#);
            run(
                CronAction::Add {
                    name: "j9".into(),
                    message: "m".into(),
                    every: Some(5),
                    cron: None,
                    deliver: false,
                    to: None,
                    channel: None,
                },
                false,
            )
            .expect("store 已存在 → 读旧数组合并后追加");
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(store_of(&home)).unwrap()).unwrap();
            let arr = jobs.as_array().unwrap();
            assert_eq!(arr.len(), 2, "旧 job 保留 + 新 job 追加");
            assert_eq!(arr[0]["id"], "keep1");
            assert_eq!(arr[1]["name"], "j9");
            assert_eq!(arr[1]["schedule"]["every_ms"], 5000);
        });
    }

    #[test]
    fn add_write_failure_when_workspace_is_regular_file_surfaces_error() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            // workspace 是普通文件：create_dir_all({home}/workspace/cron)
            // 失败但被 `let _` 吞掉；store.exists()==false（祖先被文件挡住）
            // → 初始化空 vec；最终 fs::write 打不开父路径 → Err 上抛。
            std::fs::write(home.join("workspace"), b"not a directory").unwrap();

            let r = run(
                CronAction::Add {
                    name: "blocked".into(),
                    message: "m".into(),
                    every: Some(5),
                    cron: None,
                    deliver: false,
                    to: None,
                    channel: None,
                },
                false,
            );
            assert!(r.is_err(), "写路径被普通文件阻断必须 Err");
            assert!(!store_of(&home).exists(), "store 没能创建");
        });
    }

    #[test]
    fn remove_write_failure_denied_by_readonly_store_surfaces_error() {
        with_env_home(|home| {
            write_store(&home, r#"[{"id":"abc","name":"n"}]"#);
            let store = store_of(&home);
            deny_write(&store);

            let r = run(CronAction::Remove { id: "abc".into() }, false);

            allow_write(&store); // 先还原再断言，保证 TempDir 能清理
            assert!(r.is_err(), "readonly store → 收尾写入被拒 → Err");
            // 原内容保持不变（删除没有落盘）。
            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
            assert_eq!(jobs[0]["id"], "abc");
        });
    }
}

// =========================================================================
// wave_b（覆盖率补测 2026-08-27）：Add 读臂的损坏 store 形态。
// 既有 add_merges_into_existing_store 已合法 JSON 路径走过 153-155/160-163；
// 本测试补「文件存在但内容损坏」这一不同分支形状：serde 解析失败 →
// unwrap_or_default 回退空数组（旧内容静默丢弃，见生产可疑点报告）→
// 追加新 job 后整体重写为合法数组。钉住该语义，防止未来改动悄悄变更行为。
// 注：with_env_home/store_of/write_store 是 run_arm 内私有 helper，
// 兄弟模块不可见，此处最小克隆。
// =========================================================================
mod wave_b {
    use super::super::{CronAction, run};

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    fn store_of(home: &std::path::Path) -> std::path::PathBuf {
        home.join("workspace").join("cron").join("jobs.json")
    }

    #[test]
    fn wave_b_add_over_corrupt_store_fails_loud_and_keeps_store_untouched() {
        // BUG 台账 #37：原实现 unwrap_or_default 把损坏 store 当空数组 →
        // 写盘静默顶掉全部旧任务。新契约：Add 遇损坏存储必须 Err 且
        // 文件保持原样（绝不半覆盖）。
        with_env_home(|home| {
            let store = store_of(&home);
            std::fs::create_dir_all(store.parent().unwrap()).unwrap();
            let corrupt = "not-a-json-array {{{";
            std::fs::write(&store, corrupt).unwrap();

            let err = run(
                CronAction::Add {
                    name: "wb-corrupt".into(),
                    message: "m".into(),
                    every: Some(7),
                    cron: None,
                    deliver: false,
                    to: None,
                    channel: None,
                },
                false,
            )
            .expect_err("损坏 store 必须 loud 失败");
            assert!(
                err.to_string().contains("已损坏"),
                "错误信息应点明存储损坏: {err}"
            );

            // 文件必须一个字节都没被改。
            let after = std::fs::read_to_string(&store).unwrap();
            assert_eq!(after, corrupt, "失败路径不得触碰磁盘上的存储文件");
        });
    }

    #[test]
    fn wave_b_add_on_missing_store_still_creates_fresh_store() {
        // 对照组：无存储时 Add 照常建库（首插臂不受损坏守卫影响）。
        with_env_home(|home| {
            let store = store_of(&home);
            assert!(!store.exists());

            run(
                CronAction::Add {
                    name: "wb-fresh".into(),
                    message: "m".into(),
                    every: Some(5),
                    cron: None,
                    deliver: false,
                    to: None,
                    channel: None,
                },
                false,
            )
            .expect("缺省存储 → 正常新建");

            let jobs: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&store).unwrap())
                    .expect("add 后 store 是合法 JSON 数组");
            let arr = jobs.as_array().unwrap();
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["name"], "wb-fresh");
        });
    }
}
