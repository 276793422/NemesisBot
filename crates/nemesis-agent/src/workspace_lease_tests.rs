//! WS9/P22：会话租约测试（独立测试文件——禁内联测试仓库门禁）。
//!
//! 覆盖：同工作区双持有者争用（宽限超时诚实拒绝，文案含持有者名）、
//! 释放后可再获取 + sidecar 生命周期、probe 输出形态（含陈旧记录注记）、
//! **真·跨进程死进程释放**（重调自身测试二进制：子进程 `std::process::exit`
//! 跳过 Drop，锁只由 OS 在进程死亡时释放——正是 P22 的核心承诺）、
//! register_shared_tools 包装接线（占锁时写类工具被拒；开关关 = 裸工具
//! 直跑）。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::WorkspaceLease;
use crate::context::RequestContext;
use crate::loop_tools::{SharedToolConfig, register_shared_tools};

/// 轮询等待谓词成立（100ms 步进；异步测试里等锁线程/sidecar 清理窗口）。
async fn wait_until(mut pred: impl FnMut() -> bool, max: Duration) -> bool {
    let deadline = std::time::Instant::now() + max;
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pred()
}

fn test_ctx() -> RequestContext {
    RequestContext {
        channel: "test".to_string(),
        chat_id: "chat".to_string(),
        user: "u".to_string(),
        session_key: "ws9-lease-test".to_string(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
    }
}

/// P22：同工作区双持有者——B 在 A 持有期间按宽限排队，超时诚实拒绝
/// （错误文本含 A 的持有者名）；A 释放后 B 立即可获取。
#[tokio::test]
async fn lease_blocks_second_acquire_until_grace_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let a =
        Arc::new(WorkspaceLease::new(dir.path(), "holder-a").with_grace(Duration::from_secs(5)));
    let b = Arc::new(
        WorkspaceLease::new(dir.path(), "holder-b").with_grace(Duration::from_millis(400)),
    );

    // A 直接获取（无争用）。
    let acq_a = a.acquire().await.expect("首个持有者应直接获取");
    // 持有期 sidecar 在位（含 A 的名字）。
    let sidecar = dir
        .path()
        .join("logs")
        .join("workspace_lease.lock.holder.json");
    let data = std::fs::read_to_string(&sidecar).expect("持有期 sidecar 应存在");
    assert!(data.contains("holder-a"), "sidecar 应记持有者名: {data}");

    // B 排队 400ms 后诚实拒绝，文案含 A 的持有者名。
    let err = b.acquire().await.expect_err("争用中的第二次获取必须失败");
    assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    assert!(
        err.to_string().contains("工作区正被"),
        "拒绝文案应含固定前缀: {err}"
    );
    assert!(
        err.to_string().contains("holder-a"),
        "文案应含持有者名: {err}"
    );

    // A 释放（Drop 发信号，锁线程落 guard + 清 sidecar）→ B 立即可获取。
    drop(acq_a);
    assert!(
        wait_until(|| !sidecar.exists(), Duration::from_secs(3)).await,
        "释放后 sidecar 应被清掉"
    );
    let acq_b = b.acquire().await.expect("释放后应可再获取");
    drop(acq_b);
}

/// P22：probe 输出形态——无 workspace=unsupported；持有期 held+holder；
/// 释放后 free（sidecar 已清，无陈旧注记）。
#[tokio::test]
async fn probe_reports_hold_state_and_stale_sidecar() {
    // 未装配形态。
    let p = WorkspaceLease::probe(None);
    assert_eq!(p["supported"], false);

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let lease = Arc::new(WorkspaceLease::new(dir.path(), "probe-holder"));

    let p = WorkspaceLease::probe(Some(&ws));
    assert_eq!(p["supported"], true);
    assert_eq!(p["held"], false, "无人持锁");
    // F7（2026-09-26 复查）：探针无副作用——首次探测不得创建锁载体文件。
    assert!(
        !dir.path().join("logs").join("workspace_lease.lock").exists(),
        "probe 不得创建锁文件"
    );

    let acq = lease.acquire().await.unwrap();
    let p = WorkspaceLease::probe(Some(&ws));
    assert_eq!(p["held"], true);
    assert_eq!(p["holder"].as_str(), Some("probe-holder"));
    assert!(p["acquired_at"].is_string());
    assert!(
        p["lock_path"]
            .as_str()
            .unwrap()
            .contains("workspace_lease.lock")
    );

    drop(acq);
    assert!(
        wait_until(
            || WorkspaceLease::probe(Some(&ws))["held"].as_bool() == Some(false),
            Duration::from_secs(3)
        )
        .await,
        "释放后 probe 应报 free"
    );
}

/// 子进程辅助（父测试用当前测试二进制重调 + `--exact` 过滤到本用例）：
/// 获取租约 → 写就绪文件 → `std::process::exit(0)`。exit 不跑 Drop——
/// 锁只能由 OS 在进程死亡时释放（本用例作为常规测试跑时环境变量缺席，
/// 直接空转通过）。
#[tokio::test]
async fn ws9_lease_child_holder() {
    let Ok(ws) = std::env::var("WS9_LEASE_WORKSPACE") else {
        return;
    };
    let lease = Arc::new(WorkspaceLease::new(Path::new(&ws), "dead-child"));
    let _acq = lease.acquire().await.expect("child acquire");
    std::fs::write(Path::new(&ws).join("child_ready"), "1").expect("write ready");
    // 不 drop `_acq`：std::process::exit 直接终结进程（析构不跑）。
    std::process::exit(0);
}

/// P22 核心：死进程的文件锁由 OS 自动释放——子进程持锁后
/// `std::process::exit(0)`（跳过 Drop），父进程观察到锁已释放、sidecar
/// 残留（probe 诚实标注「陈旧」）、随后可立即获取。
#[tokio::test]
async fn lease_dead_process_releases_os_lock() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let exe = std::env::current_exe().expect("current test binary");
    let status = std::process::Command::new(&exe)
        // libtest --exact 匹配全路径（模块路径前缀不可省）。
        .arg("workspace_lease_tests::ws9_lease_child_holder")
        .arg("--exact")
        .arg("--nocapture")
        .env("WS9_LEASE_WORKSPACE", dir.path())
        .status()
        .expect("spawn child test binary");
    assert!(status.success(), "child 应以 0 退出: {status}");
    assert!(
        dir.path().join("child_ready").exists(),
        "child 未走到就绪标记"
    );

    // 死进程的锁被 OS 释放（短暂轮询等内核释放可见）。
    assert!(
        wait_until(
            || WorkspaceLease::probe(Some(&ws))["held"].as_bool() == Some(false),
            Duration::from_secs(5)
        )
        .await,
        "死进程的锁应被 OS 自动释放"
    );
    // sidecar 残留（exit 跳过了清理）→ probe 诚实标注陈旧记录 + 保留名字。
    let p = WorkspaceLease::probe(Some(&ws));
    assert_eq!(
        p["holder"].as_str(),
        Some("dead-child"),
        "残留 sidecar 记死进程名"
    );
    assert!(
        p["note"].as_str().unwrap().contains("陈旧"),
        "probe 应标注陈旧记录: {}",
        p["note"]
    );

    // 新持有者立即可获取；释放后清的是自己的 sidecar（不动别人的记录——
    // 此处 sidecar 已被新持有覆盖，落盘语义见 remove_own_holder 比对）。
    let lease = Arc::new(WorkspaceLease::new(dir.path(), "parent"));
    let acq = lease.acquire().await.expect("死进程释放后应可获取");
    drop(acq);
}

/// P22：register_shared_tools 接线——workspace_lease 配置后，写类工具被
/// LeaseGuardTool 包装：他人持锁时 execute 诚实拒绝（工作区正被 …），
/// 文件不落盘。
#[tokio::test]
async fn register_shared_tools_wraps_write_tools_and_enforces() {
    let dir = tempfile::tempdir().unwrap();

    // 他人先占锁。
    let holder =
        Arc::new(WorkspaceLease::new(dir.path(), "holder-a").with_grace(Duration::from_secs(5)));
    let _held = holder.acquire().await.expect("占锁");

    // 被包装侧短宽限，避免测试等满 30s。
    let config = SharedToolConfig {
        workspace_lease: Some(Arc::new(
            WorkspaceLease::new(dir.path(), "wrapped").with_grace(Duration::from_millis(300)),
        )),
        ..Default::default()
    };
    let tools = register_shared_tools(&config);
    let ctx = test_ctx();
    // Windows 路径反斜杠不能进 JSON 字符串——统一正斜杠（工具侧接受）。
    let target = dir
        .path()
        .join("out.txt")
        .to_string_lossy()
        .replace('\\', "/");
    let args = format!(r#"{{"path":"{target}","content":"x"}}"#);

    let err = tools["write_file"]
        .execute(&args, &ctx)
        .await
        .expect_err("他人持锁时写类工具必须被拒");
    assert!(
        err.contains("工作区正被") && err.contains("holder-a"),
        "拒绝文案应含前缀+持有者名: {err}"
    );
    assert!(!std::path::Path::new(&target).exists(), "被拒的写不能落盘");

    // 协议面透传：包装不改变 description/parameters（prompt cache 前缀）。
    let plain = register_shared_tools(&SharedToolConfig::default());
    assert_eq!(
        tools["write_file"].description(),
        plain["write_file"].description(),
        "包装透传 description"
    );
    assert_eq!(
        tools["write_file"].parameters(),
        plain["write_file"].parameters(),
        "包装透传 parameters"
    );
}

/// P22：开关关（`workspace_lease: None`，即 `agents.lease_enabled=false`
/// 的装配形态）= 裸工具直跑，写照常落盘；config 显式默认值为 true。
#[tokio::test]
async fn lease_disabled_passthrough_unwrapped() {
    // 生产默认路径：config.json agents 段缺键 → serde default_true。注意
    // derive 的 `Config::default()`（全 Default 链）是 false——image_downscale
    // 同款已知 footgun；生产装配走 serde 解析/default_config()，两条都为 true。
    let agents: nemesis_config::AgentsConfig = serde_json::from_str("{}").unwrap();
    assert!(agents.lease_enabled, "serde 缺省（生产真实默认）应为 true");
    assert!(
        nemesis_config::default_config().agents.lease_enabled,
        "default_config 显式构造应为 true"
    );

    let dir = tempfile::tempdir().unwrap();
    let tools = register_shared_tools(&SharedToolConfig::default());
    let ctx = test_ctx();
    let target = dir
        .path()
        .join("free.txt")
        .to_string_lossy()
        .replace('\\', "/");
    let out = tools["write_file"]
        .execute(&format!(r#"{{"path":"{target}","content":"ok"}}"#), &ctx)
        .await
        .expect("无租约形态下写应直跑");
    assert!(!out.contains("工作区正被"));
    assert!(std::path::Path::new(&target).exists(), "文件应落盘");
}

/// F6（2026-09-27）：sidecar 清理按 per-acquire 唯一 id 比对——同 holder
/// 名的**他人**记录不得被本次释放误删，旧格式（无 acq_id）同样不删。
/// 竞态窗口用改写 sidecar 确定性模拟（A 落 guard 后、清理执行前 B 已写
/// 自己那份的中间态），不靠真竞速。
#[tokio::test]
async fn release_removes_only_own_acq_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = dir
        .path()
        .join("logs")
        .join("workspace_lease.lock.holder.json");
    let lease = Arc::new(WorkspaceLease::new(dir.path(), "same-holder"));

    let acq = lease.acquire().await.expect("获取");
    // 模拟同 holder 名「上一任」的中间态：持锁者已换人，sidecar 还是
    // 上一任的（旧实现按 holder 名比对 → 本次释放会把这份误删）。
    std::fs::write(
        &sidecar,
        r#"{"holder":"same-holder","acquired_at":"2026-09-26T00:00:00+08:00","acq_id":"prev-acq"}"#,
    )
    .unwrap();

    drop(acq);
    // 给清理线程充分的（错误）删除窗口，再断言幸存。
    tokio::time::sleep(Duration::from_millis(500)).await;
    let data = std::fs::read_to_string(&sidecar).expect("异 acq_id 的 sidecar 不得被删");
    assert!(data.contains("prev-acq"), "保留的应是上一任记录: {data}");

    // 正常路径：重新获取覆盖 sidecar → 释放 → 自己的（id 自洽）被清。
    let acq2 = lease.acquire().await.expect("再获取");
    drop(acq2);
    assert!(
        wait_until(|| !sidecar.exists(), Duration::from_secs(3)).await,
        "自己的 sidecar 释放时应被清理"
    );

    // 旧格式 sidecar（无 acq_id，升级窗口内旧版本进程写的）不删——
    // 宁留勿误删，残留由 probe 诚实标注为陈旧记录。
    let acq3 = lease.acquire().await.expect("三获取");
    std::fs::write(
        &sidecar,
        r#"{"holder":"same-holder","acquired_at":"2026-09-26T00:00:00+08:00"}"#,
    )
    .unwrap();
    drop(acq3);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        sidecar.exists(),
        "旧格式 sidecar 不得被删（宁留勿误删）: {:?}",
        std::fs::read_to_string(&sidecar)
    );
}
