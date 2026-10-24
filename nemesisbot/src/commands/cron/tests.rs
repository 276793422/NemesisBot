//! cron CLI 测试。存储格式统一（docs/REPORT/2026-10-13_cron-store-format-
//! split-cli-vs-gateway-fix.md）后本文件整体重写：CLI 与 gateway 共用
//! nemesis-cron store（canonical envelope），旧「裸数组」形态只以**迁移输入**
//! 的身份出现在测试里。断言一律落在磁盘上的 store 文件 + `CronService` 读回
//! 两个面上，不再断言手拼 JSON 的中间形状。

use super::*;

// ===========================================================================
// run() 全臂 —— env home 隔离 + GLOBAL_STATE_LOCK 串行；store 落在
// {home}/workspace/cron/jobs.json。
// ===========================================================================

fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

fn read_store(home: &std::path::Path) -> nemesis_cron::store::CronStoreData {
    nemesis_cron::store::load(&store_of(home)).expect("store 必须可被共享 loader 解析")
}

fn write_store(home: &std::path::Path, body: &str) {
    let p = store_of(home);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn add_args(name: &str, every: Option<u64>, cron: Option<String>) -> CronAction {
    CronAction::Add {
        name: name.into(),
        message: "m".into(),
        every,
        cron,
        deliver: false,
        to: None,
        channel: None,
    }
}

#[test]
fn list_without_store_shows_hint() {
    with_env_home(|_home| {
        run(CronAction::List, false).expect("无 store → 提示 + Ok");
    });
}

#[test]
fn list_covers_envelope_empty_and_corrupt() {
    with_env_home(|home| {
        // canonical 空 envelope。
        write_store(&home, r#"{"version":1,"jobs":[]}"#);
        run(CronAction::List, false).expect("空 envelope → No scheduled jobs");

        // 损坏存储 → List 也必须 loud（诚实报错，不假装没有任务）。
        write_store(&home, r#"{"not":"an array"}"#);
        let err = run(CronAction::List, false).expect_err("损坏 store 必须 Err");
        assert!(
            err.to_string().contains("已损坏"),
            "错误要点明存储损坏: {err}"
        );
    });
}

#[test]
fn add_interval_job_persists_canonical_envelope() {
    with_env_home(|home| {
        run(add_args("j1", Some(30), None), false).expect("add ok");
        let store = read_store(&home);
        assert_eq!(store.version, 1, "落盘必须是 canonical envelope");
        assert_eq!(store.jobs.len(), 1);
        let job = &store.jobs[0];
        assert_eq!(job.name, "j1");
        assert_eq!(job.schedule.kind, "every", "kind 与 gateway 语义对齐");
        assert_eq!(job.schedule.every_ms, Some(30000));
        assert!(job.enabled);
        assert_eq!(job.payload.message, "m");
        assert_eq!(job.id.len(), 16, "id 与 gateway generate_id 同源（16 hex）");
    });
}

#[test]
fn add_cron_job_deliver_fields_and_validation() {
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
        let job = &read_store(&home).jobs[0];
        assert_eq!(job.schedule.kind, "cron");
        assert_eq!(job.schedule.expr.as_deref(), Some("*/5 * * * *"));
        assert!(job.payload.deliver);
        assert_eq!(job.payload.to.as_deref(), Some("user1"));
        assert_eq!(job.payload.channel.as_deref(), Some("web"));

        // 非法 cron 表达式 → 落盘前拒绝（gateway validate_schedule 同源）。
        let err = run(
            CronAction::Add {
                name: "bad".into(),
                message: "m".into(),
                every: None,
                cron: Some("not a cron".into()),
                deliver: false,
                to: None,
                channel: None,
            },
            false,
        )
        .expect_err("非法表达式必须 Err");
        assert!(err.to_string().contains("cron 表达式无效"), "{err}");
        assert_eq!(read_store(&home).jobs.len(), 1, "失败的 add 不落盘");

        // 两个调度参数都不给 → 错误提示后 Ok（不写文件）。
        run(add_args("j3", None, None), false).expect("缺调度参数 → 打印错误 + Ok");
        assert_eq!(read_store(&home).jobs.len(), 1);
    });
}

#[test]
fn add_every_zero_is_rejected_before_touching_store() {
    with_env_home(|home| {
        let err = run(add_args("zero", Some(0), None), false).expect_err("--every 0 必须 Err");
        assert!(err.to_string().contains("--every"), "{err}");
        assert!(!store_of(&home).exists(), "拒绝路径不得创建 store");
    });
}

#[test]
fn remove_found_not_found_and_no_store() {
    with_env_home(|home| {
        // 无 store。
        run(CronAction::Remove { id: "x".into() }, false).expect("无 store → not found Ok");

        write_store(
            &home,
            r#"{"version":1,"jobs":[{"id":"abc","name":"n","enabled":true,"schedule":{"kind":"every","at_ms":null,"every_ms":60000,"expr":null,"tz":null},"payload":{"kind":"agent_turn","message":"m","command":null,"deliver":false,"channel":null,"to":null,"session_key":null,"max_rounds":null},"state":{"next_run_at_ms":null,"last_run_at_ms":null,"last_status":null,"last_error":null,"history":[]},"created_at_ms":1,"updated_at_ms":1,"delete_after_run":false}]}"#,
        );
        run(CronAction::Remove { id: "abc".into() }, false).expect("remove ok");
        assert!(read_store(&home).jobs.is_empty(), "已删除");

        run(CronAction::Remove { id: "ghost".into() }, false).expect("不存在 → not found Ok");
    });
}

#[test]
fn enable_disable_dispatch_updates_envelope() {
    with_env_home(|home| {
        write_store(
            &home,
            r#"{"version":1,"jobs":[{"id":"abc","name":"n","enabled":true,"schedule":{"kind":"every","at_ms":null,"every_ms":60000,"expr":null,"tz":null},"payload":{"kind":"agent_turn","message":"m","command":null,"deliver":false,"channel":null,"to":null,"session_key":null,"max_rounds":null},"state":{"next_run_at_ms":null,"last_run_at_ms":null,"last_status":null,"last_error":null,"history":[]},"created_at_ms":1,"updated_at_ms":1,"delete_after_run":false}]}"#,
        );
        run(CronAction::Disable { id: "abc".into() }, false).expect("disable ok");
        assert!(!read_store(&home).jobs[0].enabled);

        run(CronAction::Enable { id: "abc".into() }, false).expect("enable ok");
        assert!(read_store(&home).jobs[0].enabled);

        run(CronAction::Enable { id: "ghost".into() }, false).expect("不存在 → not found Ok");
    });
}

// -----------------------------------------------------------------------
// R7（coverage-95 goal）：Add/Remove 的 store-已存在读臂 + 写失败臂。
// - Add 在 store 已存在时必须读旧内容做合并（此前只测过空新建）；
// - {home}/workspace 是普通文件时保存失败 → ? 上抛 Err；
// - store 文件只读（Windows READONLY attr / unix 目录 0555）时 Remove 的
//   收尾原子写失败 → ? 上抛 Err，且原内容保持不变。
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

/// POSIX 侧测试助手：临时把目录设为只读，Drop 恢复原模式。cron 收尾
/// 保存走 `write_file_atomic`（同目录 tmp + rename 覆盖），POSIX 语义
/// 只看目录写权限——store 文件 0o444 拦不住（2026-09-24 nightly run
/// 36001616750 实证假红），拦父目录才是 POSIX 下「保存必失败」的
/// 正确构造；Windows 上 readonly 属性本身即拒绝 replace，无需此件。
/// 同款先例：commands::cors::tests::wave_a::DenyDirWrite。
#[cfg(unix)]
struct DenyDirWrite(std::path::PathBuf, std::fs::Permissions);

#[cfg(unix)]
impl DenyDirWrite {
    fn new(p: &std::path::Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::metadata(p).unwrap().permissions();
        let mut denied = perm.clone();
        denied.set_mode(0o555);
        std::fs::set_permissions(p, denied).unwrap();
        Self(p.to_path_buf(), perm)
    }
}

#[cfg(unix)]
impl Drop for DenyDirWrite {
    fn drop(&mut self) {
        // 恢复必须先于 TempDir 清理：声明序上本 guard 晚于 tmp 创建，
        // 逆序 drop 保证先还原目录再删树。
        std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
    }
}

#[test]
fn add_merges_into_existing_store() {
    with_env_home(|home| {
        write_store(
            &home,
            r#"{"version":1,"jobs":[{"id":"keep1","name":"kept","enabled":true,"schedule":{"kind":"every","at_ms":null,"every_ms":60000,"expr":null,"tz":null},"payload":{"kind":"agent_turn","message":"m","command":null,"deliver":false,"channel":null,"to":null,"session_key":null,"max_rounds":null},"state":{"next_run_at_ms":null,"last_run_at_ms":null,"last_status":null,"last_error":null,"history":[]},"created_at_ms":1,"updated_at_ms":1,"delete_after_run":false}]}"#,
        );
        run(add_args("j9", Some(5), None), false).expect("store 已存在 → 读旧内容合并后追加");
        let store = read_store(&home);
        assert_eq!(store.jobs.len(), 2, "旧 job 保留 + 新 job 追加");
        assert_eq!(store.jobs[0].id, "keep1");
        assert_eq!(store.jobs[1].name, "j9");
        assert_eq!(store.jobs[1].schedule.every_ms, Some(5000));
    });
}

#[test]
fn add_write_failure_when_workspace_is_regular_file_surfaces_error() {
    with_env_home(|home| {
        std::fs::create_dir_all(&home).unwrap();
        // workspace 是普通文件：store 祖先被文件挡住（exists()==false）→
        // svc 走空 store；add_job_ext 落盘时 write_file_atomic 的 mkdir
        // 失败 → Err 上抛（service 侧同步回滚内存）。
        std::fs::write(home.join("workspace"), b"not a directory").unwrap();

        let r = run(add_args("blocked", Some(5), None), false);
        assert!(r.is_err(), "写路径被普通文件阻断必须 Err");
        assert!(!store_of(&home).exists(), "store 没能创建");
    });
}

#[test]
fn remove_write_failure_denied_by_readonly_store_surfaces_error() {
    with_env_home(|home| {
        write_store(
            &home,
            r#"{"version":1,"jobs":[{"id":"abc","name":"n","enabled":true,"schedule":{"kind":"every","at_ms":null,"every_ms":60000,"expr":null,"tz":null},"payload":{"kind":"agent_turn","message":"m","command":null,"deliver":false,"channel":null,"to":null,"session_key":null,"max_rounds":null},"state":{"next_run_at_ms":null,"last_run_at_ms":null,"last_status":null,"last_error":null,"history":[]},"created_at_ms":1,"updated_at_ms":1,"delete_after_run":false}]}"#,
        );
        let store = store_of(&home);
        deny_write(&store);
        // POSIX：rename 型保存只看目录写权限，需父目录一并只读
        //（Drop 恢复，逆序 drop 先于 TempDir 清理）。
        #[cfg(unix)]
        let _dir_deny = DenyDirWrite::new(store.parent().unwrap());

        let r = run(CronAction::Remove { id: "abc".into() }, false);

        allow_write(&store); // 先还原再断言，保证 TempDir 能清理
        assert!(r.is_err(), "readonly store → 收尾写入被拒 → Err");
        // 原内容保持不变（删除没有落盘）。
        assert_eq!(read_store(&home).jobs[0].id, "abc");
    });
}

// ===========================================================================
// wave_b：损坏 store 守卫（BUG 台账 #37 契约延续）。
// ===========================================================================

#[test]
fn wave_b_add_over_corrupt_store_fails_loud_and_keeps_store_untouched() {
    with_env_home(|home| {
        let store = store_of(&home);
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        let corrupt = "not-a-json-array {{{";
        std::fs::write(&store, corrupt).unwrap();

        let err = run(add_args("wb-corrupt", Some(7), None), false)
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

        run(add_args("wb-fresh", Some(5), None), false).expect("缺省存储 → 正常新建");

        let loaded = read_store(&home);
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.jobs.len(), 1);
        assert_eq!(loaded.jobs[0].name, "wb-fresh");
    });
}

// ===========================================================================
// 跨层回归锁（BUG 修复核心，docs/REPORT/2026-10-13_cron-store-format-
// split-cli-vs-gateway-fix.md 方向 3）：旧 CLI 裸数组 store → CLI add 迁移
// 合并 → gateway 侧 CronService 直接可见。修复前这个场景是「gateway 启动
// WARN + 空 store + 首次保存把 CLI 任务顶掉」。
// ===========================================================================

mod cross_layer {
    use super::super::{CronAction, run};
    use super::{add_args, read_store, store_of, with_env_home, write_store};

    /// 旧 CLI 手拼形态的完整 store（真实历史形态：message/deliver/to/channel
    /// 在条目顶层，schedule.kind="interval"，带 display 字段）。
    fn legacy_store_body() -> String {
        r#"[
  {
    "id": "oldjob01",
    "name": "legacy-job",
    "message": "hello from old cli",
    "schedule": { "kind": "interval", "every_ms": 120000, "display": "every 120s" },
    "deliver": true,
    "to": "user1",
    "channel": "web",
    "enabled": true
  }
]"#
        .to_string()
    }

    #[test]
    fn cli_add_over_legacy_store_migrates_and_gateway_service_sees_both_jobs() {
        with_env_home(|home| {
            write_store(&home, &legacy_store_body());

            run(add_args("new-job", Some(5), None), false)
                .expect("旧形态 store 上 add 必须走迁移+合并");

            // 磁盘面：文件已愈合为 canonical envelope，两个任务都在。
            let store = read_store(&home);
            assert_eq!(store.version, 1);
            assert_eq!(store.jobs.len(), 2);
            let legacy = store
                .jobs
                .iter()
                .find(|j| j.id == "oldjob01")
                .expect("旧任务必须被迁移保留");
            assert_eq!(legacy.name, "legacy-job");
            assert_eq!(legacy.schedule.kind, "every", "interval → every");
            assert_eq!(legacy.schedule.every_ms, Some(120000));
            assert_eq!(legacy.payload.message, "hello from old cli");
            assert!(legacy.payload.deliver);
            assert_eq!(legacy.payload.channel.as_deref(), Some("web"));

            // gateway 面：CronService 直接读同一文件，无需任何特殊处理。
            let svc = nemesis_cron::CronService::new(&store_of(&home).to_string_lossy());
            let jobs = svc.list_jobs(true);
            assert_eq!(jobs.len(), 2, "gateway 必须看到 CLI 写入的全部任务");
            assert!(jobs.iter().any(|j| j.name == "new-job"));
            assert!(jobs.iter().any(|j| j.name == "legacy-job"));
        });
    }

    #[test]
    fn gateway_written_store_is_readable_by_cli_list() {
        with_env_home(|home| {
            // gateway 面：CronService 写 store（add_job_ext 落盘）。
            let svc = nemesis_cron::CronService::new(&store_of(&home).to_string_lossy());
            svc.add_job(
                "gw-job",
                nemesis_cron::CronSchedule {
                    kind: "every".into(),
                    at_ms: None,
                    every_ms: Some(60000),
                    expr: None,
                    tz: None,
                },
                "from gateway",
                false,
                None,
                None,
            )
            .expect("service add ok");

            // CLI 面：list 直接消费（修复前 CLI 只认裸数组，读到 envelope
            // 会当成「没有任务」）。
            run(CronAction::List, false).expect("CLI 必须能列 gateway 写的 store");
            assert_eq!(read_store(&home).jobs[0].name, "gw-job");
        });
    }

    #[test]
    fn corrupt_legacy_entry_blocks_add_with_position() {
        with_env_home(|home| {
            // 旧形态但缺 message（不存在于真实历史文件，属损坏）→ 迁移
            // 整体拒绝，CLI 损坏守卫接住。serde 层的结构性错误（缺字段）
            // 只能给行/列定位；条目名定位归 migrate 自身错误（见
            // nemesis-cron store::tests 的 every_ms 用例）。
            write_store(
                &home,
                r#"[{"id":"x","name":"broken","schedule":{"kind":"interval","every_ms":1000}}]"#,
            );
            let err =
                run(add_args("n", Some(5), None), false).expect_err("不可迁移的旧条目必须拒绝 add");
            let msg = err.to_string();
            assert!(
                msg.contains("已损坏") && msg.contains("missing field"),
                "错误要点明损坏与原因: {msg}"
            );
        });
    }
}

// ===========================================================================
// gateway 在位守卫：运行中的 gateway 以内存态写穿 store，CLI 增删改会在
// gateway 下次保存时被整体覆盖（静默丢失）。守卫复用 estop 探活链
// （state/gateway.json + /api/health），只在**确认** gateway 在跑时拦。
// ===========================================================================
mod live_gateway_guard {
    // GLOBAL_STATE_LOCK 必须横跨 .await 持有：NEMESISBOT_HOME 是进程全局，
    // 守卫探活的 await 期间同样不能让其他测试看到本测试的 home。单线程
    // tokio 测试 runtime + 探活目标是不碰锁的本地应答线程，无死锁面。
    #![allow(clippy::await_holding_lock)]
    use super::super::{CronAction, ensure_no_live_gateway};
    use std::io::{Read as _, Write as _};

    fn state_path_of(home: &std::path::Path) -> std::path::PathBuf {
        home.join("workspace").join("state").join("gateway.json")
    }

    fn plant_state(home: &std::path::Path, port: i64) {
        let p = state_path_of(home);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            format!(r#"{{"pid": 1, "web_host": "127.0.0.1", "web_port": {port}}}"#),
        )
        .unwrap();
    }

    /// 最小 HTTP 200 应答器（loopback 随机端口，服务 `count` 个请求后退出；
    /// 守卫每次只发一个 GET /api/health）。
    fn spawn_health_responder(count: usize) -> i64 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port() as i64;
        std::thread::spawn(move || {
            for _ in 0..count {
                let Ok((mut s, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
        port
    }

    fn set_env_home() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        (tmp, home)
    }

    #[tokio::test]
    async fn blocks_add_when_gateway_alive_but_list_exempt() {
        let _serial = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_tmp, home) = set_env_home();

        let port = spawn_health_responder(1);
        plant_state(&home, port);

        let err = ensure_no_live_gateway(
            &CronAction::Add {
                name: "g".into(),
                message: "m".into(),
                every: Some(5),
                cron: None,
                deliver: false,
                to: None,
                channel: None,
            },
            false,
        )
        .await
        .expect_err("gateway 确认在跑时增改必须被拦");
        let msg = err.to_string();
        assert!(msg.contains("gateway 正在运行"), "{msg}");
        assert!(msg.contains("Dashboard"), "错误要指引替代路径: {msg}");

        // List 只读：守卫在 action 判定处短路，不探活不拦（responder 计数
        // 1 已被上一臂消费，此处即便走到探活也会因无人应答而放行——断言
        // 的是「无 Err」这一外部契约）。
        ensure_no_live_gateway(&CronAction::List, false)
            .await
            .expect("List 只读不拦");

        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    #[tokio::test]
    async fn passes_when_state_missing_or_stale() {
        let _serial = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_tmp, home) = set_env_home();

        // 无 state 文件 → 放行。
        ensure_no_live_gateway(&CronAction::Remove { id: "x".into() }, false)
            .await
            .expect("无 state 文件必须放行");

        // state 指向无人监听的端口 → 探活失败放行（stale/已死进程不拦）。
        plant_state(&home, 1);
        ensure_no_live_gateway(&CronAction::Enable { id: "x".into() }, false)
            .await
            .expect("探活失败必须放行");

        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }
}
