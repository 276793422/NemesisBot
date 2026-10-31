//! AgentRuntimeSupervisor 测试。
//!
//! 两层：config 现读纯逻辑（无进程）+ 真子进程链路（resolve_test_bin：
//! debug 槽专用解析，缺二进制 panic 提示先构建——与 exec_worker/tests.rs
//! wave_r10 同门）。
//! 真子进程组仅 Windows（.exe 解析 + taskkill 形态）。
//
// 真子进程组用 GLOBAL_STATE_LOCK（std Mutex）跨 await 持有做测试串行——
// spawn/taskkill 与其他动全局状态的测试不得并发，仓库既有惯例显式放行
// （loop/tests.rs、channels web/tests.rs 同款）。
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;

use super::AgentRuntimeSupervisor;

// ---------------------------------------------------------------------------
// config 现读（无进程）
// ---------------------------------------------------------------------------

/// 磁盘是唯一真相源：enabled=true 读到配额；false / 键缺失 / 文件缺失 /
/// executor 段缺失全部归 None（不可用）。
#[test]
fn persistent_settings_follows_config_disk_state() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":60}}}"#,
    )
    .unwrap();
    let sup = AgentRuntimeSupervisor::new(
        PathBuf::from("nemesisbot.exe"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cfg_path.clone(),
    );
    let s = sup.persistent_settings().expect("enabled 配置应读到");
    assert_eq!(s.max_slots, 2);
    assert_eq!(s.call_timeout_secs, 60);
    assert!(sup.persistent_enabled());

    // enabled=false → 关闭
    std::fs::write(
        &cfg_path,
        r#"{"executor":{"persistent":{"enabled":false,"max_slots":9}}}"#,
    )
    .unwrap();
    assert!(!sup.persistent_enabled());

    // executor 段缺失（老 config 形态）→ 关闭
    std::fs::write(&cfg_path, r#"{"gateway":{"port":1}}"#).unwrap();
    assert!(!sup.persistent_enabled());

    // 文件消失 → 关闭（不 panic 不兜底）
    std::fs::remove_file(&cfg_path).unwrap();
    assert!(!sup.persistent_enabled());
}

/// 显式 `call_timeout_secs: 0` 在 config 层原样保留（钳制发生在 execute
/// 消费点，走兜底档——supervision 面没有「不限」）。
#[test]
fn persistent_call_timeout_zero_passes_through_config_layer() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"executor":{"persistent":{"enabled":true,"call_timeout_secs":0}}}"#,
    )
    .unwrap();
    let sup = AgentRuntimeSupervisor::new(
        PathBuf::from("nemesisbot.exe"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cfg_path,
    );
    let s = sup.persistent_settings().unwrap();
    assert_eq!(
        s.call_timeout_secs, 0,
        "config 原样保留；钳制发生在 execute 消费点"
    );
}

/// P3 ②：盒 `AllowNetworkAccess` 取自 config `executor.allow_network` 的
/// **现读**（每次 spawn 读盘，磁盘是唯一真相源——与 persistent_settings
/// 同纪律）；缺键/缺段/文件缺失 = false（默认断网，安全向）。仅 Windows +
/// sandbox feature（方法本体 cfg 门控）。
#[cfg(all(feature = "sandbox", windows))]
#[test]
fn executor_allow_network_follows_config_disk_state() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"executor":{"allow_network":true}}"#).unwrap();
    let sup = AgentRuntimeSupervisor::new(
        PathBuf::from("nemesisbot.exe"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cfg_path.clone(),
    );
    assert!(sup.executor_allow_network(), "allow_network=true 应读到");

    // 翻转 → false
    std::fs::write(&cfg_path, r#"{"executor":{"allow_network":false}}"#).unwrap();
    assert!(!sup.executor_allow_network());

    // 键缺失 → false（默认断网）
    std::fs::write(&cfg_path, r#"{"executor":{"persistent":{"enabled":true}}}"#).unwrap();
    assert!(!sup.executor_allow_network(), "缺键默认断网");

    // executor 段缺失 → false
    std::fs::write(&cfg_path, r#"{"gateway":{"port":1}}"#).unwrap();
    assert!(!sup.executor_allow_network());

    // 文件消失 → false（不 panic）
    std::fs::remove_file(&cfg_path).unwrap();
    assert!(!sup.executor_allow_network());
}

/// P3 ①/④ spawn 基础 env 装配点钉死：stdio 与盒两条 spawn 路径共用的
/// 7 键集合——角色、双根（T7：WORKSPACE=exec_root 工具 cwd 锚 / HOME=
/// agent home 隔离根，两根分离是 main/project 形态的正确语义）、agent id、
/// 常驻标记、浏览器身份目录（T6：值恒 = home/browser-profile，与
/// nemesis-path `resolve_agent_browser_profile_in_workspace` 同一布局）、
/// 围栏第二根（B1：home≠exec_root 时透传 home，子进程围栏与主进程同源；
/// 两根重合不写=6 键，selfbuilt 旧行为逐键一致）；路径专属键
/// PIPE/SANDBOX_BACKEND 由调用方追加，不在本装配点。
#[test]
fn base_executor_env_keys_pinned() {
    // fixture 路径必须随平台：装配点的 EXTRA_ROOTS 走 `env::join_paths`，
    // 它拒收含平台分隔符的段（Linux 分隔符是 `:`）——Windows 字面量在
    // Linux 上编码失败 → 按 fail-closed 设计丢第 7 键 → 钉死断言 6≠7 假红
    //（2026-10-31 ET Linux 全量实录）。两平台各用合法路径，7 键钉死共通。
    let exec_root = PathBuf::from(if cfg!(windows) {
        r"C:\tmp\workspace-root"
    } else {
        "/tmp/workspace-root"
    });
    let home = PathBuf::from(if cfg!(windows) {
        r"C:\tmp\agent-home"
    } else {
        "/tmp/agent-home"
    });
    let agent_id = "ag-42";
    let env = super::base_executor_env(&exec_root, &home, agent_id);

    assert_eq!(env.len(), 7, "装配点键集合变化必须显式更新本测试");
    let get = |k: &str| {
        env.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("缺键 {k}"))
    };
    assert_eq!(get("NEMESISBOT_ROLE"), "executor");
    assert_eq!(
        get("NEMESISBOT_EXECUTOR_WORKSPACE"),
        exec_root.to_string_lossy(),
        "WORKSPACE=exec_root（工具 cwd 锚），T7 双根拆分"
    );
    assert_eq!(
        get("NEMESISBOT_EXECUTOR_HOME"),
        home.to_string_lossy(),
        "HOME=agent home（隔离根）"
    );
    assert_eq!(get("NEMESISBOT_EXECUTOR_AGENT_ID"), agent_id);
    assert_eq!(get("NEMESISBOT_EXECUTOR_PERSISTENT"), "1");
    assert_eq!(
        get("NEMESISBOT_BROWSER_PROFILE"),
        home.join("browser-profile").to_string_lossy()
    );
    assert_eq!(
        get("NEMESISBOT_EXECUTOR_EXTRA_ROOTS"),
        home.to_string_lossy(),
        "B1：home≠exec_root → 围栏第二根透传 home"
    );

    // 两根重合（selfbuilt）：不写第二根键，键集合回落 6 键旧行为。
    let env_same = super::base_executor_env(&exec_root, &exec_root, agent_id);
    assert_eq!(
        env_same.len(),
        6,
        "两根重合不写第二根键（selfbuilt 旧行为）"
    );
    assert!(
        !env_same
            .iter()
            .any(|(k, _)| k == "NEMESISBOT_EXECUTOR_EXTRA_ROOTS"),
        "重合形态必须缺第二根键"
    );
}

/// P3（PLAN Linux 项）：persistent stdio 臂的 U11 标记开关取自 config
/// `executor.sandbox` 的**现读**（每次 spawn 读盘）；缺键/缺段/文件缺失 =
/// false（与 `ExecutorSeparationConfig` serde default 一致）。仅非 Windows
/// （方法本体 cfg 门控；Windows 的隔离走 Sandboxie 盒臂）。
#[cfg(not(windows))]
#[test]
fn executor_sandbox_requested_follows_config_disk_state() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"executor":{"sandbox":true}}"#).unwrap();
    let sup = AgentRuntimeSupervisor::new(
        PathBuf::from("nemesisbot"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cfg_path.clone(),
    );
    assert!(sup.executor_sandbox_requested(), "sandbox=true 应读到");

    // 翻转 → false
    std::fs::write(&cfg_path, r#"{"executor":{"sandbox":false}}"#).unwrap();
    assert!(!sup.executor_sandbox_requested());

    // 键缺失 → false
    std::fs::write(&cfg_path, r#"{"executor":{"persistent":{"enabled":true}}}"#).unwrap();
    assert!(!sup.executor_sandbox_requested(), "缺键默认不挂标记");

    // executor 段缺失 / 文件消失 → false（不 panic）
    std::fs::write(&cfg_path, r#"{"gateway":{"port":1}}"#).unwrap();
    assert!(!sup.executor_sandbox_requested());
    std::fs::remove_file(&cfg_path).unwrap();
    assert!(!sup.executor_sandbox_requested());
}

// ---------------------------------------------------------------------------
// R1 隔离策略判读（ExecutorIsolationPolicy，无进程）
// ---------------------------------------------------------------------------

/// R1：策略三键（sandbox/strict/allow_network）同一现读实现——磁盘是唯一
/// 真相源，缺段/缺键/文件缺失/解析失败 = false（与 serde default 同缺省）。
#[test]
fn isolation_policy_reads_config_disk_state() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let policy = crate::exec_world::ExecutorIsolationPolicy::new(cfg_path.clone());

    std::fs::write(
        &cfg_path,
        r#"{"executor":{"sandbox":true,"strict":true,"allow_network":true}}"#,
    )
    .unwrap();
    assert!(policy.sandbox_requested());
    assert!(policy.strict_required());
    assert!(policy.allow_network());

    // 翻转 + strict 键缺失
    std::fs::write(&cfg_path, r#"{"executor":{"sandbox":false}}"#).unwrap();
    assert!(!policy.sandbox_requested());
    assert!(!policy.strict_required(), "strict 键缺失 = false");
    assert!(!policy.allow_network());

    // executor 段缺失（老 config 形态）
    std::fs::write(&cfg_path, r#"{"gateway":{"port":1}}"#).unwrap();
    assert!(!policy.sandbox_requested());
    assert!(!policy.strict_required());

    // 文件消失 / 解析失败 → false（不 panic）
    std::fs::remove_file(&cfg_path).unwrap();
    assert!(!policy.strict_required());
    std::fs::write(&cfg_path, "not-json").unwrap();
    assert!(!policy.strict_required(), "解析失败 = false");
}

/// R1：strict 后端就绪性实测——临时 home 下引擎永远不就绪（测试环境默认态
/// 即「后端不可用」面；真实 Sandboxie 故障注入不可 CI 复现，见模块文档诚实
/// 边界）。
#[cfg(all(feature = "sandbox", windows))]
#[test]
fn strict_backend_not_ready_in_temp_home() {
    let dir = tempfile::tempdir().unwrap();
    let policy = crate::exec_world::ExecutorIsolationPolicy::new(dir.path().join("config.json"));
    assert!(
        !policy.strict_backend_ready(dir.path()),
        "临时 home 下 Sandboxie 引擎不应就绪"
    );
}

// ---------------------------------------------------------------------------
// T9 槽位看护：崩溃自动重拉（预算熔断 / 复位 / 常驻档关闭不拉）
// ---------------------------------------------------------------------------

/// 常驻档已关（运行中改键 / 配置损坏）：auto_relaunch 诚实不拉且**不计数**
/// （槽位就此回收；重开开关后由 lazy spawn 接管，不背旧账）。
#[tokio::test]
async fn auto_relaunch_disabled_config_skips_and_does_not_count() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"executor":{"persistent":{"enabled":false,"max_slots":2}}}"#,
    )
    .unwrap();
    let sup = AgentRuntimeSupervisor::new(
        PathBuf::from("definitely-not-a-real-exe"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cfg_path,
    );
    sup.auto_relaunch("agent-x", dir.path()).await;
    assert_eq!(
        sup.respawn_attempt_count("agent-x"),
        0,
        "常驻档关 = 不拉不计数"
    );
    assert!(sup.slot_states().is_empty());
}

/// 熔断：spawn 恒败（bogus exe）时连续 auto_relaunch 到预算上限停手——
/// 超限后不再尝试（计数封顶）；显式复位（成功 execute / 显式 wake 走的
/// 同一入口）后预算重新可用；计数按 agent 隔离。
#[tokio::test]
async fn auto_relaunch_budget_caps_consecutive_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":60}}}"#,
    )
    .unwrap();
    let sup = AgentRuntimeSupervisor::new(
        PathBuf::from("definitely-not-a-real-exe"),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cfg_path,
    );
    for expected in 1..=3 {
        sup.auto_relaunch("agent-x", dir.path()).await;
        assert_eq!(
            sup.respawn_attempt_count("agent-x"),
            expected,
            "预算内第 {expected} 次尝试应计数"
        );
    }
    // 第 4 次：超预算停手——计数不再增长，也不留半截槽。
    sup.auto_relaunch("agent-x", dir.path()).await;
    assert_eq!(sup.respawn_attempt_count("agent-x"), 3, "超预算后停手");
    assert!(sup.slot_states().is_empty(), "spawn 失败不留半截槽");

    // 复位：预算重新可用（从头计数）。
    sup.clear_relaunch_counter("agent-x");
    sup.auto_relaunch("agent-x", dir.path()).await;
    assert_eq!(sup.respawn_attempt_count("agent-x"), 1, "复位后从头计数");

    // 计数按 agent 隔离：别的 agent 不背账。
    sup.auto_relaunch("agent-y", dir.path()).await;
    assert_eq!(sup.respawn_attempt_count("agent-y"), 1);
    assert_eq!(sup.respawn_attempt_count("agent-x"), 1);
}

// ---------------------------------------------------------------------------
// 真子进程链路（仅 Windows）
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod sup_real_child {
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::GLOBAL_STATE_LOCK;
    use crate::agents::runtime::AgentRuntimeSupervisor;
    use crate::agents::runtime::Isolation;
    use nemesis_agent::context::RequestContext;
    use nemesis_path::resolve_agent_home_in_workspace;

    fn ctx() -> RequestContext {
        RequestContext::new("web", "chat-sup", "tester", "sess-sup")
    }

    /// 测试二进制解析：debug 槽专用，不走 test-harness 的 release 优先序。
    /// 本组测试断言 v2 wire 契约（call_id 回显）——被测二进制必须与源码
    /// 同代；release 槽是任意旧产物（CI 缓存实录：pre-v2 的 release exe
    /// 不回显 call_id → 全组 deterministic 红，debug 二进制钉入即全绿）。
    /// debug 槽由 `cargo test -p nemesisbot`（自动构建本包 bin 目标）与
    /// nextest 纪律的前置 `cargo build -p nemesisbot` 维持新鲜。
    /// `NEMESISBOT_TEST_BIN` 覆盖仍尊重（llvm-cov 独立 target dir 同链）。
    fn resolve_test_bin() -> anyhow::Result<PathBuf> {
        if let Ok(bin) = std::env::var("NEMESISBOT_TEST_BIN") {
            let bin = PathBuf::from(bin);
            if bin.exists() {
                return Ok(bin);
            }
            anyhow::bail!("NEMESISBOT_TEST_BIN={} 不存在；拒绝静默回落", bin.display());
        }
        let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("manifest dir has parent")
            .join("target")
            .join("debug")
            .join(format!("nemesisbot{}", std::env::consts::EXE_SUFFIX));
        if bin.exists() {
            return Ok(bin);
        }
        anyhow::bail!(
            "target/debug/nemesisbot{} 不存在——先 cargo build -p nemesisbot（nextest 不构建常规 bin 目标）",
            std::env::consts::EXE_SUFFIX
        );
    }

    /// 组装一个指向真二进制的 supervisor（config 写入 tempdir）。
    /// 返回的 `exec_root` = tempdir 根（main agent 语义：exec_root=workspace
    /// 根；与 agent home 分离——双根拆分的行为验证锚）。
    fn supervisor(
        tmp: &tempfile::TempDir,
        cfg_json: &str,
    ) -> (Arc<AgentRuntimeSupervisor>, PathBuf, PathBuf) {
        let bin = match resolve_test_bin() {
            Ok(p) => p,
            Err(e) => panic!("nemesisbot 二进制未找到: {e}"),
        };
        let cfg_path = tmp.path().join("config.json");
        std::fs::write(&cfg_path, cfg_json).unwrap();
        // main / project agent 的 home 由 AgentManager 物化；本层测试自建。
        let home = resolve_agent_home_in_workspace(tmp.path(), "agent-a");
        std::fs::create_dir_all(&home).unwrap();
        let sup = Arc::new(AgentRuntimeSupervisor::new(
            bin,
            // home = tempdir（SandboxPaths 派生锚）——临时 home 下引擎永远
            // 不就绪 → spawn 走 stdio 退化臂（真盒臂需真 Sandboxie，见
            // exec_world/tests.rs 同款结构验证纪律）。
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            cfg_path.clone(),
        ));
        (sup, tmp.path().to_path_buf(), cfg_path)
    }

    fn kill_pid(pid: u32) {
        let st = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .expect("taskkill spawn");
        assert!(
            st.status.success(),
            "taskkill /F /PID {pid} 失败: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    async fn exec_cmd(
        sup: &AgentRuntimeSupervisor,
        exec_root: &std::path::Path,
        cmd: &str,
    ) -> Result<String, String> {
        let c = ctx();
        let args = serde_json::json!({ "command": cmd }).to_string();
        sup.execute("agent-a", exec_root, "exec", &args, &c)
            .await
            .map_err(|e| e.to_string())
    }

    /// 日常链路：常驻执行体跑通一次工具调用；杀掉子进程后下一次调用自动
    /// 重建（journal/ home 即状态）；home 重定向生效（USERPROFILE 指 home）；
    /// exec_root 双根拆分生效（T7：工具 cwd = exec_root ≠ agent home）。
    #[tokio::test]
    async fn sup_execute_round_trip_and_respawn_after_kill() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );

        // 首次调用：spawn + 往返
        let out = exec_cmd(&sup, &exec_root, "echo sup-roundtrip")
            .await
            .expect("persistent round trip");
        assert!(out.contains("sup-roundtrip"), "exec 输出异常: {out}");

        // T7 双根拆分：exec 工具 cwd = exec_root（= tempdir 根），而非
        // agent home——main agent 语义（exec_root=workspace 根）。
        let out = exec_cmd(&sup, &exec_root, "cd").await.expect("cwd probe");
        let cwd = out
            .lines()
            .last()
            .unwrap_or("")
            .trim()
            .trim_end_matches('\r');
        assert!(
            cwd.eq_ignore_ascii_case(&exec_root.to_string_lossy()),
            "exec cwd 应为 exec_root（{}），得到: {out}",
            exec_root.display()
        );

        // P3 ①：home 族重定向——子进程内 USERPROFILE 即 agent home
        let out = exec_cmd(&sup, &exec_root, "echo %USERPROFILE%")
            .await
            .expect("home redirect probe");
        assert!(
            out.contains("agent-a"),
            "USERPROFILE 应指向 agent home（…agents/agent-a/home）: {out}"
        );

        // 杀子进程 → 下一次调用自动重建（无在途调用，重建即安全）
        let pid = sup
            .slot_pid("agent-a")
            .await
            .expect("slot pid after first call");
        kill_pid(pid);
        let out = exec_cmd(&sup, &exec_root, "echo after-respawn")
            .await
            .expect("respawn after external kill");
        assert!(out.contains("after-respawn"), "respawn 后 exec 异常: {out}");
        let pid2 = sup
            .slot_pid("agent-a")
            .await
            .expect("slot pid after respawn");
        assert_ne!(pid, pid2, "respawn 应产生新进程");
    }

    /// R4：evict = 注册删除联动停机——进程杀掉、槽清空、重拉预算清零；
    /// 幂等（再 evict 不炸不报）。
    #[tokio::test]
    async fn sup_evict_shuts_down_clears_budget_and_is_idempotent() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        let out = exec_cmd(&sup, &exec_root, "echo before-evict")
            .await
            .expect("persistent round trip");
        assert!(out.contains("before-evict"), "{out}");
        // 制造一次重拉记账（崩溃 → auto_relaunch），证明 evict 清预算。
        let pid = sup.slot_pid("agent-a").await.expect("slot pid");
        kill_pid(pid);
        sup.auto_relaunch("agent-a", &exec_root).await;
        assert_eq!(sup.respawn_attempt_count("agent-a"), 1, "前置：重拉已记账");
        assert!(sup.slot_pid("agent-a").await.is_some());

        sup.evict("agent-a").await;
        assert!(sup.slot_states().is_empty(), "evict 后槽必须清空");
        assert!(sup.slot_pid("agent-a").await.is_none());
        assert_eq!(
            sup.respawn_attempt_count("agent-a"),
            0,
            "evict 必须清重拉预算"
        );
        // 幂等：再 evict 零槽零报错。
        sup.evict("agent-a").await;
        assert!(sup.slot_states().is_empty());
    }

    /// R4：注册表已删除的 agent 崩溃后看护循环不重拉（孤儿进程不复活），
    /// 也不记重拉预算；重新注册后显式 wake 复活（wake 是操作员在场语义，
    /// 不过 registration 闸——闸只拦看护循环的自动重拉）。
    #[tokio::test]
    async fn sup_removed_agent_crash_is_not_relaunched() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        // 可变注册闭包：AtomicBool 模拟「注册 → 删除 → 重新注册」。
        let registered = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let reg = registered.clone();
        sup.set_registration_check(Arc::new(move |_id| {
            reg.load(std::sync::atomic::Ordering::SeqCst)
        }));

        let out = exec_cmd(&sup, &exec_root, "echo alive-check")
            .await
            .expect("spawn");
        assert!(out.contains("alive-check"), "{out}");
        let pid = sup.slot_pid("agent-a").await.expect("slot pid");

        // 「删除」：注册闭包翻 false + 进程外杀（模拟崩溃）→ 真看护 tick
        // （watchdog_tick 的死槽路径 = remove_and_shutdown 清槽 + 注册闸
        // 拦下 auto_relaunch——完整生产顺序，不是裸调 auto_relaunch）。
        registered.store(false, std::sync::atomic::Ordering::SeqCst);
        kill_pid(pid);
        // taskkill /F 返回 ≠ try_wait 立即翻死：TerminateProcess 之后进程
        // 对象清理是异步的，watchdog_tick 若抢在清理完成前探测会把槽当活槽
        // 跳过（全量并行负载下实录假红 2026-10-18）。轮询等监督器同一探测
        // 结论（is_alive=try_wait，slot_states alive 投影）翻死再 tick——
        // 有界 10s，超时按旧路径继续（tick 断言会给出诚实失败）。
        for _ in 0..200 {
            let alive = sup
                .slot_states()
                .iter()
                .any(|s| s.agent_id == "agent-a" && s.alive);
            if !alive {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        sup.watchdog_tick().await;
        assert!(
            sup.slot_states().is_empty(),
            "已删除 agent 崩溃后看护循环不得重拉"
        );
        assert_eq!(
            sup.respawn_attempt_count("agent-a"),
            0,
            "不重拉也不得记账（预算闸在检查之后才动）"
        );

        // 「重新注册」：显式 wake 复活。
        registered.store(true, std::sync::atomic::Ordering::SeqCst);
        sup.wake_agent("agent-a", &exec_root)
            .await
            .expect("重新注册后显式 wake 复活");
        assert!(sup.slot_pid("agent-a").await.is_some());
    }

    /// R4：删除与 execute 并发不脱管——evict 拿槽锁等在途调用收尾（慢调用
    /// 完整返回），收尾后槽清空。evict 若不等锁直接杀，在途调用会中途断线
    /// 返回 Err。
    #[tokio::test]
    async fn sup_evict_waits_for_inflight_execute() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        // 后台慢调用（~2s ping），等槽出现后并发 evict。
        let sup2 = Arc::clone(&sup);
        let er = exec_root.clone();
        let inflight = tokio::spawn(async move {
            exec_cmd(&sup2, &er, "ping -n 2 127.0.0.1 >nul && echo slow-done").await
        });
        let waited = {
            let mut ok = false;
            for _ in 0..200 {
                if sup.slot_pid("agent-a").await.is_some() {
                    ok = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            ok
        };
        assert!(waited, "在途调用未建槽");

        sup.evict("agent-a").await;
        let r = inflight.await.unwrap();
        assert!(
            r.is_ok(),
            "在途调用必须完整收尾（evict 等槽锁，不得中途杀）: {r:?}"
        );
        assert!(sup.slot_states().is_empty(), "收尾后槽清空");
    }

    /// F3（二轮复评）：删除编排窗口内 get_or_spawn 拒新——旗标在位时无槽
    /// 也不 spawn（早退闸），wake 诚实 Unavailable；evict_end 后恢复可用
    /// （真实子进程往返）。
    #[tokio::test]
    async fn sup_evict_flag_refuses_wake_and_recovers_after_end() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        use crate::agents::runtime::SupervisorError;

        sup.evict_begin("agent-a").await; // 无槽：纯置旗
        let r = sup.wake_agent("agent-a", &exec_root).await;
        match r {
            // F1：删除在途 = 生命周期拒绝（原 Unavailable 变体已拆分）。
            Err(SupervisorError::LifecycleRejected(m)) => {
                assert!(m.contains("being removed"), "拒绝须说明删除在途: {m}");
            }
            other => panic!("编排窗口内 wake 必须被拒: {other:?}"),
        }
        assert!(sup.slot_states().is_empty(), "拒绝路径不得留下任何槽");

        sup.evict_end("agent-a");
        sup.wake_agent("agent-a", &exec_root)
            .await
            .expect("evict_end 后恢复可用");
        assert!(sup.slot_pid("agent-a").await.is_some());
        sup.evict("agent-a").await;
        assert!(sup.slot_states().is_empty());
    }

    // R3 回归锁（原 review 探针反转，2026-10-20）：删除完整走完（begin→end、
    // 旗标已清、记录已注销）后，spawn 中途的在途 wake 必须在插入点被代次
    // 复核拦下——诚实 Err、刚 spawn 的会话被关停、零幸存槽。旧行为（ABA）：
    // 插表成功 = 已删身份复活，result Ok 且槽幸存。F1（2026-10-21 三轮复评）：
    // 拒绝变体从 Unavailable 细分为 LifecycleRejected（身份已终结，禁宿主
    // 回落）——match 臂随之收窄，可降级的 Unavailable 不再混入删除语义。
    #[tokio::test]
    async fn completed_delete_cannot_be_overtaken_by_inflight_spawn() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        let registered = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let registered_check = registered.clone();
        sup.set_registration_check(Arc::new(move |_| {
            registered_check.load(std::sync::atomic::Ordering::SeqCst)
        }));
        let barrier = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        *sup.review_spawn_barrier.lock().unwrap() = Some(barrier.clone());
        let bg_sup = sup.clone();
        let bg = tokio::spawn(async move { bg_sup.wake_agent("agent-a", &exec_root).await });
        tokio::time::timeout(std::time::Duration::from_secs(20), barrier.0.notified())
            .await
            .unwrap();
        sup.evict_begin("agent-a").await;
        registered.store(false, std::sync::atomic::Ordering::SeqCst);
        sup.evict_end("agent-a");
        barrier.1.notify_one();
        let result = bg.await.unwrap();
        assert!(sup.slot_states().is_empty(), "已删身份不得留下任何幸存槽");
        match &result {
            Err(crate::agents::runtime::SupervisorError::LifecycleRejected(m)) => {
                assert!(
                    m.contains("being removed") || m.contains("no longer present"),
                    "诚实拒绝须说明删除/注销: {m}"
                );
            }
            other => panic!("删除完成后在途 spawn 必须被拒（ABA 封死）: {other:?}"),
        }
    }

    // F1 回归锁（原三轮复评诊断探针翻转，2026-10-21）：删除完整走完后恢复
    // 的工具调用必须**终止**在生命周期拒绝——不落宿主 per-call 回落。旧行为
    // （探针实证）：拒绝被统一映射成 host_fallback=true，RemoteExecutorTool
    // 经宿主通道照常执行（宿主 marker 文件被读出 = 已删身份的工具成功执行）。
    // 断言面（对齐评估验收 1-3）：Err + 错误表达生命周期拒绝 + 错误不含宿主
    // marker 内容（宿主通道未执行 = 无新工具副作用）+ 零幸存槽。完整链路
    // （RemoteExecutorTool.execute，非仅 wake_agent）。
    #[tokio::test]
    async fn completed_delete_rejection_terminates_call_without_host_fallback() {
        use nemesis_agent::r#loop::Tool;
        use nemesis_agent::loop_tools::ExecTool;
        use nemesis_agent::remote_executor_tool::{
            ExecutorChannel, PersistentRoute, RemoteExecutorTool,
        };
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        let registered = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let check = registered.clone();
        sup.set_registration_check(Arc::new(move |_| {
            check.load(std::sync::atomic::Ordering::SeqCst)
        }));
        // 宿主 marker：若回落发生，per-call read_file 会把内容读出来。
        std::fs::write(
            exec_root.join("review-host-marker.txt"),
            "REVIEW_DELETED_AGENT_HOST_READ",
        )
        .unwrap();
        let barrier = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        *sup.review_spawn_barrier.lock().unwrap() = Some(barrier.clone());
        let channel = Arc::new(
            ExecutorChannel::new(
                resolve_test_bin().unwrap(),
                exec_root.to_string_lossy().to_string(),
                Arc::new(|| false),
            )
            .with_home(tmp.path().to_path_buf()),
        );
        let local = Box::new(ExecTool::new(&exec_root.to_string_lossy(), true));
        let exec_root_for_route = exec_root.clone();
        let tool = RemoteExecutorTool::new("read_file".into(), local, channel).with_persistent(
            PersistentRoute {
                resolve: Arc::new(move || Some(("agent-a".into(), exec_root_for_route.clone()))),
                executor: sup.clone(),
            },
        );
        let bg = tokio::spawn(async move {
            tool.execute(r#"{"path":"review-host-marker.txt"}"#, &ctx())
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(20), barrier.0.notified())
            .await
            .unwrap();
        sup.evict_begin("agent-a").await;
        registered.store(false, std::sync::atomic::Ordering::SeqCst);
        sup.evict_end("agent-a");
        barrier.1.notify_one();
        let result = bg.await.unwrap();
        assert!(sup.slot_states().is_empty(), "已删身份不得留下任何幸存槽");
        let err = match result {
            Ok(out) => panic!("删除后的工具调用必须终止，不得经宿主回落成功执行: {out:?}"),
            Err(e) => e,
        };
        assert!(
            err.contains("lifecycle rejected"),
            "错误必须表达生命周期拒绝: {err}"
        );
        assert!(
            !err.contains("REVIEW_DELETED_AGENT_HOST_READ"),
            "宿主回落不得发生（marker 内容不得出现于任何结果）: {err}"
        );
    }

    /// F3：删除编排与并发 spawn 的竞态不变式——begin 返回后槽必为空（并发
    /// wake 要么被旗标拒绝（修复语义），要么先成槽被排空（等价旧行为）），
    /// 绝不允许幸存「evict 覆盖不到的替代槽」；反复三轮压窗口（spawn 真实
    /// 子进程数百 ms，80ms 后置旗 = 高概率落在 spawn 在途）。旧行为此断言
    /// 红：insert 落在「槽已摘/排空返回」之后 → 幸存槽。
    #[tokio::test]
    async fn sup_evict_window_blocks_replacement_spawn() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        for round in 0..3 {
            let sup2 = Arc::clone(&sup);
            let er = exec_root.clone();
            let bg = tokio::spawn(async move { sup2.wake_agent("agent-a", &er).await });
            // 等 spawn 进场（子进程已起、槽未插表 = 旧行为的可乘窗口）。
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            sup.evict_begin("agent-a").await;
            assert!(
                sup.slot_states().is_empty(),
                "round {round}: 删除编排窗口内不得幸存替代槽"
            );
            let r = bg.await.unwrap();
            if let Err(crate::agents::runtime::SupervisorError::Unavailable(m)) = &r {
                assert!(m.contains("being removed"), "round {round}: {m}");
            }
            sup.evict_end("agent-a");
        }
        // 全程结束后 supervisor 干净（下轮测试复用全局进程预算无残留）。
        assert!(sup.slot_states().is_empty());
    }

    /// P3 ①：env 白名单——宿主注入的秘密变量在常驻执行体内不可见；OS
    /// 基线（PATH）照常可见；运输层内部变量（NEMESISBOT_*）对模型可跑的
    /// 命令 env 不可见——两层防御：① supervisor spawn env_clear+白名单
    /// （宿主秘密剥离），② exec 工具孙进程消毒（nemesis_utils::env_sanitize
    /// 剥 NEMESISBOT_*，子进程拿到的运输标记不漏进模型可见 env，既定纪律）。
    #[tokio::test]
    async fn sup_env_allowlist_strips_host_secrets() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("NEMESISBOT_TEST_SECRET", "nb-secret-leak-marker");
        }
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );

        // `set` = cmd 全量 env dump（比逐个 echo 更难漏）
        let out = exec_cmd(&sup, &exec_root, "set").await.expect("env dump");
        assert!(
            !out.contains("nb-secret-leak-marker"),
            "宿主秘密泄漏进常驻执行体 env！"
        );
        // 行首匹配 + 忽略大小写（Windows env 名大小写不敏感；CI runner 存
        // `Path`、Git Bash 传播 `PATH`——按原样 contains 会按环境确定性分叉，
        // CI 三连红实录 2026-10-18）。
        assert!(
            out.lines().any(|l| l.to_uppercase().starts_with("PATH=")),
            "OS 基线（PATH）应保留: {out}"
        );
        // home 族重定向对孙进程可见（echo %USERPROFILE% 断言在 round-trip
        // 测试；此处 HOME 直读同理）。
        assert!(
            out.contains(&format!(
                "HOME={}",
                resolve_agent_home_in_workspace(tmp.path(), "agent-a").display()
            )),
            "home 重定向应在 env dump 可见: {out}"
        );
        // 运输层内部变量不得漏进孙进程 env（env_sanitize 纪律：模型可跑的
        // 命令观测不到/伪造不了执行体标记——NEMESISBOT_ROLE/PERSISTENT/
        // AGENT_ID 等在孙进程 dump 中必须缺席；子进程侧消费在 T5/T6 接线）。
        // 按「变量名=」行首匹配（大小写不敏感子串会误伤 PATH 里的仓库目录名
        // NemesisBot_Rust_2）。
        assert!(
            !out.lines()
                .any(|l| l.to_uppercase().starts_with("NEMESISBOT_")),
            "运输层内部变量不应漏进孙进程 env（env_sanitize 纪律）: {out}"
        );

        unsafe {
            std::env::remove_var("NEMESISBOT_TEST_SECRET");
        }
    }

    /// 容量闸：max_slots=1 时第二个 agent = Unavailable（调用未发生，可安全
    /// 回落）；sleep 释放槽后可用。
    #[tokio::test]
    async fn sup_slots_full_reports_unavailable_then_sleep_frees() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":1,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        // agent-b 的 home 也要物化（本测试双 agent）
        let home_b = resolve_agent_home_in_workspace(tmp.path(), "agent-b");
        std::fs::create_dir_all(&home_b).unwrap();

        let c = ctx();
        let args = serde_json::json!({ "command": "echo occupy" }).to_string();
        sup.execute("agent-a", &exec_root, "exec", &args, &c)
            .await
            .expect("agent-a occupy slot");

        let err = sup
            .execute("agent-b", &exec_root, "exec", &args, &c)
            .await
            .expect_err("满员必须 Unavailable");
        match err {
            crate::agents::runtime::SupervisorError::Unavailable(m) => {
                assert!(m.contains("slots full"), "满员错误信息异常: {m}");
            }
            other => panic!("应分型为 Unavailable（可回落），得到: {other:?}"),
        }

        // sleep 释放 → agent-b 可用
        assert!(sup.sleep_agent("agent-a").await);
        sup.execute("agent-b", &exec_root, "exec", &args, &c)
            .await
            .expect("sleep 释放槽后 agent-b 应可用");
    }

    /// P3 ⑤ + R2：调用中途杀子进程（kill -9 语义）→ Eof → 一次重建 + 原样
    /// 重发同 call_id → 子进程两相账裁决：有副作用工具（exec）回「结果未知」
    /// 诚实错误（不盲重试——副作用可能已发生；at-most-once 优先于重跑）；
    /// 随后新 call_id 调用照常完成（重建链路健康，诚实错误不拦新调用）。
    /// 只读白名单的安全重执行半边由 exec_worker serve_journal 测试钉
    /// （子进程侧契约，真实中途击杀只读调用不可靠计时）。
    #[tokio::test]
    async fn sup_midcall_kill_side_effect_reports_unknown() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );

        // 先跑一次快调用拿 pid（在途时 slot 锁被调用持有，事后取不到）
        let out = exec_cmd(&sup, &exec_root, "echo warmup")
            .await
            .expect("warmup");
        assert!(out.contains("warmup"));
        let pid = sup.slot_pid("agent-a").await.expect("pid");

        // 慢调用在途 → 杀进程 → 重建重发同 call_id → 诚实「结果未知」
        let sup2 = sup.clone();
        let root2 = exec_root.clone();
        let slow = tokio::spawn(async move {
            let c = ctx();
            let args = serde_json::json!({ "command": "ping -n 3 127.0.0.1 > nul" }).to_string();
            sup2.execute("agent-a", &root2, "exec", &args, &c).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        kill_pid(pid);

        let r = tokio::time::timeout(std::time::Duration::from_secs(60), slow)
            .await
            .expect("重试应在超时前完成")
            .expect("join");
        match r {
            Ok(out) => {
                panic!("有副作用工具中途被杀重发必须报「结果未知」，不得静默重执行: {out:?}")
            }
            Err(crate::agents::runtime::SupervisorError::ToolFailed(m)) => {
                assert!(
                    m.contains("结果未知") || m.contains("勿盲重试"),
                    "重发错误必须是 R2 诚实语义，得到: {m}"
                );
            }
            Err(other) => panic!("应分型为 ToolFailed 而非: {other:?}"),
        }

        // 新 call_id（execute 每次新生成）照常完成——重建链路健康。
        let out = exec_cmd(&sup, &exec_root, "echo after-unknown")
            .await
            .expect("崩溃+诚实错误后新调用应完成");
        assert!(out.contains("after-unknown"), "{out}");
    }

    /// R18 回归锁：慢工具撞调用超时后，带毒会话不得复用。旧 BUG：超时只
    /// 报错不销毁会话，child 仍活着、迟到响应滞留管道——下次调用读到上一
    /// 次的迟到行，call_id 永久错位一格（超时一次执行体对后续所有调用
    /// 报废）。修复语义：非 Eof 失败即判死会话（fail_poisoned），下一次
    /// 调用走既有死进程重建路径拿干净会话。
    #[tokio::test]
    async fn sup_timeout_kills_poisoned_session_next_call_rebuilds() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":1}}}"#,
        );

        // 预热建槽拿 pid
        let out = exec_cmd(&sup, &exec_root, "echo warmup")
            .await
            .expect("warmup");
        assert!(out.contains("warmup"));
        let pid = sup.slot_pid("agent-a").await.expect("pid");

        // 慢工具（ping -n 4 ≈ 3s）撞 1s 调用超时 → ToolFailed；超时≠死亡，
        // child 此刻还活着（这正是带毒的根源）。
        match exec_cmd(&sup, &exec_root, "ping -n 4 127.0.0.1 > nul").await {
            Err(m) => assert!(
                m.contains("timed out"),
                "超时错误信息应可辨识（executor transport timed out）: {m}"
            ),
            Ok(out) => panic!("≈3s 的 ping 应撞 1s 调用超时，却成功了: {out}"),
        }

        // 判死是同步收尾（start_kill+wait 在错误返回前完成）：下一次调用
        // 必须成功——旧 BUG 在此读到迟到响应 → Protocol/call_id 错位失败，
        // 且此后每次调用都读上一次的响应，永久报废。
        let out = exec_cmd(&sup, &exec_root, "echo after-timeout")
            .await
            .expect("超时判死后下一次调用应走重建拿干净会话");
        assert!(out.contains("after-timeout"), "{out}");

        // 复用必须经过**重建**：新 pid 证明拿的是新会话，不是毒槽续命。
        let pid2 = sup.slot_pid("agent-a").await.expect("pid after rebuild");
        assert_ne!(pid, pid2, "超时判死后应重建出全新进程");

        // 重建后的会话健康：再来一次快调用确认链路稳定。
        let out = exec_cmd(&sup, &exec_root, "echo stable-after-rebuild")
            .await
            .expect("重建会话应持续可用");
        assert!(out.contains("stable-after-rebuild"), "{out}");
        sup.sleep_agent("agent-a").await;
    }

    /// sleep/wake 生命周期 + slot_states 观测面：sleep 杀进程清槽（幂等），
    /// wake 产生新进程；T8 宣告消费的快照形态正确。
    #[tokio::test]
    async fn sup_sleep_wake_and_slot_states() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );

        exec_cmd(&sup, &exec_root, "echo lifecycle")
            .await
            .expect("first call");

        let states = sup.slot_states();
        assert_eq!(states.len(), 1, "一个 agent 一个槽: {states:?}");
        assert_eq!(states[0].agent_id, "agent-a");
        assert!(states[0].alive);
        // R1 观测面：无 sandbox 请求 + 临时 home 引擎不就绪 → stdio 槽，
        // 隔离形态诚实记录（不冒标）。
        assert_eq!(
            states[0].isolation,
            Some(Isolation::Stdio),
            "stdio 槽的隔离形态应记 Stdio: {states:?}"
        );

        let pid = sup.slot_pid("agent-a").await.expect("pid");
        assert!(sup.sleep_agent("agent-a").await, "sleep 应清槽");
        assert!(sup.slot_pid("agent-a").await.is_none(), "sleep 后槽应移除");
        assert!(!sup.sleep_agent("agent-a").await, "重复 sleep 幂等 false");

        sup.wake_agent("agent-a", &exec_root).await.expect("wake");
        let pid2 = sup.slot_pid("agent-a").await.expect("pid after wake");
        assert_ne!(pid, pid2, "wake 应是新进程");
        let states = sup.slot_states();
        assert_eq!(states.len(), 1);
        assert!(states[0].alive);

        // 收尾：别让子进程泄到测试进程外
        sup.sleep_agent("agent-a").await;
    }

    /// R1 strict 预检：`executor.strict` 请求在场而后端不可用（临时 home
    /// 引擎永不就绪）→ spawn 前拒绝，执行体零落地（无子进程、无槽——
    /// 「拒绝执行」不能退回裸 stdio）。错误分型 Unavailable（调用未发生，
    /// per-call 回落链可安全处理）。
    #[tokio::test]
    async fn sup_strict_unavailable_refuses_and_spawns_nothing() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"sandbox":true,"strict":true,"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        let c = ctx();
        let args = serde_json::json!({ "command": "echo must-not-run" }).to_string();
        let err = sup
            .execute("agent-a", &exec_root, "exec", &args, &c)
            .await
            .expect_err("strict + 后端不可用必须拒绝");
        match err {
            crate::agents::runtime::SupervisorError::Unavailable(m) => {
                assert!(m.contains("strict"), "拒绝信息应指明 strict 契约: {m}");
            }
            other => panic!("应分型为 Unavailable，得到: {other:?}"),
        }
        assert!(sup.slot_states().is_empty(), "拒绝执行不得留下槽位/子进程");
        assert!(sup.slot_pid("agent-a").await.is_none());
    }

    /// R1：sandbox=true 但引擎未就绪、未开 strict → stdio 兼容回退照常可用
    /// （⑥——strict 缺位时不因沙盒缺失拒服务），槽位隔离形态诚实记 Stdio。
    #[tokio::test]
    async fn sup_sandbox_requested_engine_absent_no_strict_falls_back() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, _cfg) = supervisor(
            &tmp,
            r#"{"executor":{"sandbox":true,"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        let out = exec_cmd(&sup, &exec_root, "echo compat-fallback")
            .await
            .expect("非 strict 的 stdio 兼容回退应可用");
        assert!(out.contains("compat-fallback"), "exec 输出异常: {out}");
        let states = sup.slot_states();
        assert_eq!(states[0].isolation, Some(Isolation::Stdio));
        sup.sleep_agent("agent-a").await;
    }

    /// R1 运行中配置切换：strict 翻开 → 现存 stdio 槽不再满足契约 → execute
    /// 复检触发重建；后端仍不可用 → Unavailable（调用未发生）。关回 strict
    /// → 原槽仍活着，恢复可用（重建失败不损毁在册会话）。
    #[tokio::test]
    async fn sup_runtime_strict_flip_triggers_revalidation() {
        let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let (sup, exec_root, cfg_path) = supervisor(
            &tmp,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        );
        // 基线：无 strict → stdio 槽正常服务
        let out = exec_cmd(&sup, &exec_root, "echo before-flip")
            .await
            .expect("baseline call");
        assert!(out.contains("before-flip"));

        // 运行中翻开 strict（磁盘现读，无需重启）→ 复检重建被拒
        std::fs::write(
            &cfg_path,
            r#"{"executor":{"sandbox":true,"strict":true,"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        )
        .unwrap();
        let c = ctx();
        let args = serde_json::json!({ "command": "echo must-not-run" }).to_string();
        let err = sup
            .execute("agent-a", &exec_root, "exec", &args, &c)
            .await
            .expect_err("strict 翻开后 stdio 槽必须重建且重建被拒");
        match err {
            crate::agents::runtime::SupervisorError::Unavailable(m) => {
                assert!(m.contains("strict"), "拒绝信息应指明 strict 契约: {m}");
            }
            other => panic!("应分型为 Unavailable，得到: {other:?}"),
        }

        // 关回 strict → 原会话未被失败重建破坏，恢复服务
        std::fs::write(
            &cfg_path,
            r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
        )
        .unwrap();
        let out = exec_cmd(&sup, &exec_root, "echo after-unflip")
            .await
            .expect("关回 strict 后应恢复可用");
        assert!(out.contains("after-unflip"), "exec 输出异常: {out}");
        sup.sleep_agent("agent-a").await;
    }
}
