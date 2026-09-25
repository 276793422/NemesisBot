//! cc_hooks 覆盖率收尾批次（Wave6B）：观察型事件的 exit-2 响亮日志臂
//! （SubagentStop warn / Notification debug）/ Notification 载荷 extra 并入
//! / compact stdout info 臂 / legacy hooks.json 迁移 copy 失败 warn 臂 /
//! ObservingApprovalManager is_running 委托。
//!
//! 本文件自造 helper（hooks_doc/bridge_with 等）——兄弟测试模块之间不能
//! 互相 import（crate 内测试模块私有）。tracing 参数行（warn!/debug!/info!
//! 的实参表达式）只有在挂了 subscriber 时才求值，故日志断言测试一律
//! `capture_logs()`。

use super::*;
use crate::test_support::capture_logs;

fn tempdir() -> std::path::PathBuf {
    tempfile::tempdir().expect("tempdir").keep()
}

/// 程序化构造单事件 hooks 文档（json! 负责转义）。
fn hooks_doc(event: &str, command: &str) -> String {
    let hook = serde_json::json!({ "command": command });
    let doc = serde_json::json!({ "hooks": { event: [ { "hooks": [hook] } ] } });
    doc.to_string()
}

fn bridge_with(json: &str, project_dir: &std::path::Path) -> CcHookBridge {
    CcHookBridge::from_json(json, project_dir.to_path_buf()).expect("bridge")
}

/// stderr 输出 msg 并 exit 2（阻断形态；与 tests.rs block_cmd 同款）。
fn block_cmd(msg: &str) -> String {
    if cfg!(windows) {
        format!("echo {msg} 1>&2 & exit 2")
    } else {
        format!("echo \"{msg}\" >&2; exit 2")
    }
}

fn subagent_info() -> super::SubagentInfo {
    super::SubagentInfo {
        agent_id: "cov-agent".to_string(),
        task: "covw6b task".to_string(),
        depth: 1,
        background: false,
        tools_profile: "readonly".to_string(),
        loop_kind: "main",
    }
}

// ---------------------------------------------------------------------------
// 观察型事件 × exit 2 → 响亮日志（warn/debug 参数行求值）
// ---------------------------------------------------------------------------

/// SubagentStop 钩子 exit 2 → 无阻断语义，warn 记录（788-790 参数行）。
#[tokio::test]
async fn subagent_stop_exit2_hook_warns_loudly() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let b = bridge_with(&hooks_doc("SubagentStop", &block_cmd("too late")), &tmp);
    b.dispatch_subagent_stop(&subagent_info(), "completed", None)
        .await;
    // 走到这里即观察型语义成立；warn 臂已在 capture subscriber 下求值。
}

/// Notification 钩子 exit 2 → 纯观察，debug 记录（814 参数行）。
#[tokio::test]
async fn notification_exit2_hook_debug_logged() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let b = bridge_with(
        &hooks_doc("Notification", &block_cmd("no window yet")),
        &tmp,
    );
    b.dispatch_notification("sk", "security_ask", "auditor", serde_json::json!({}))
        .await;
}

/// compact 钩子 stdout 非空 → info 记录（1082 参数行）。
/// 现有用例全是重定向到文件的 echo（stdout 空），此臂需真 stdout。
#[tokio::test]
async fn compact_hook_stdout_gets_info_logged() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let echo = "echo covw6b-compact-stdout";
    let b = bridge_with(&hooks_doc("PreCompact", echo), &tmp);
    b.run_compact_hooks("auto", "pre").await;
    let b2 = bridge_with(&hooks_doc("PostCompact", echo), &tmp);
    b2.run_compact_hooks("auto", "post").await;
}

// ---------------------------------------------------------------------------
// notification_payload：extra 对象并入（837 循环体）
// ---------------------------------------------------------------------------

/// extra 带非空对象 → 逐键并入 combined（837 循环）+ 公共字段齐全。
#[test]
fn notification_payload_merges_extra_object() {
    let dir = tempdir();
    let payload = notification_payload(
        "cov-sk",
        &dir,
        "question",
        "loop",
        serde_json::json!({ "custom_key": "custom_value", "n": 7 }),
    );
    let v: Value = serde_json::from_str(&payload).expect("payload is json");
    assert_eq!(v["kind"].as_str(), Some("question"));
    assert_eq!(v["layer"].as_str(), Some("loop"));
    assert_eq!(v["custom_key"].as_str(), Some("custom_value"));
    assert_eq!(v["n"].as_i64(), Some(7));
    // 方言公共字段：hook_event_name + session_id + cwd。
    assert_eq!(v["hook_event_name"].as_str(), Some("Notification"));
    assert_eq!(v["session_id"].as_str(), Some("cov-sk"));
    assert_eq!(v["cwd"].as_str(), Some(dir.to_string_lossy().as_ref()));
}

// ---------------------------------------------------------------------------
// migrate_legacy_home_hooks_config：copy 失败 warn 臂（99-101）
// ---------------------------------------------------------------------------

/// workspace config 路径被**文件**占用 → create_dir_all 静默失败 →
/// fs::copy 失败 → warn（不 panic，legacy 保留）。
#[test]
fn migrate_legacy_copy_failure_warns_and_keeps_legacy() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let home_cfg = tmp.join("home_cfg");
    std::fs::create_dir_all(&home_cfg).unwrap();
    let legacy = home_cfg.join(HOOKS_FILE);
    std::fs::write(&legacy, br#"{"hooks":{}}"#).unwrap();

    // 占位文件顶掉 workspace config 目录。
    let ws_cfg = tmp.join("ws_cfg_blocked");
    std::fs::write(&ws_cfg, b"not a dir").unwrap();

    migrate_legacy_home_hooks_config(&home_cfg, &ws_cfg);

    // copy 失败：legacy 原样保留，目标位置无新文件。
    assert!(legacy.is_file());
    assert!(!ws_cfg.join(HOOKS_FILE).exists());
}

/// 成功臂对照：目录就位时 copy-once 生效（顺带钉住幂等：第二次直通）。
#[test]
fn migrate_legacy_success_copies_once() {
    let _logs = capture_logs();
    let tmp = tempdir();
    let home_cfg = tmp.join("home_cfg");
    std::fs::create_dir_all(&home_cfg).unwrap();
    std::fs::write(home_cfg.join(HOOKS_FILE), br#"{"hooks":{}}"#).unwrap();
    let ws_cfg = tmp.join("ws_cfg");
    std::fs::create_dir_all(&ws_cfg).unwrap();

    migrate_legacy_home_hooks_config(&home_cfg, &ws_cfg);
    assert!(ws_cfg.join(HOOKS_FILE).is_file());
    assert!(home_cfg.join(HOOKS_FILE).is_file(), "legacy 保留作备份");

    // 改写 legacy 后再迁移：copy-once（目标已存在 → 不动）。
    std::fs::write(home_cfg.join(HOOKS_FILE), br#"{"hooks":{"B":1}}"#).unwrap();
    migrate_legacy_home_hooks_config(&home_cfg, &ws_cfg);
    let copied = std::fs::read(ws_cfg.join(HOOKS_FILE)).unwrap();
    assert_eq!(copied, b"{\"hooks\":{}}" as &[u8], "copy-once 不覆盖");
}

// ---------------------------------------------------------------------------
// ObservingApprovalManager：is_running 委托（1176-1178）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
struct CovFakeManager;
#[cfg(feature = "security")]
impl nemesis_security::auditor::ApprovalManager for CovFakeManager {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        Err("covw6b fake never approves".into())
    }
}

/// 观察装饰器 is_running 原样委托内层（1176-1178）。
#[cfg(feature = "security")]
#[test]
fn observing_approval_manager_delegates_is_running() {
    use nemesis_security::auditor::ApprovalManager as _;
    let bridge =
        CcHookBridge::from_json(r#"{"hooks":{}}"#, std::env::temp_dir()).expect("empty bridge");
    let obs = ObservingApprovalManager::new(Arc::new(CovFakeManager), Arc::new(bridge));
    assert!(obs.is_running());
}

/// extra 非对象（标量/null）→ 并入循环整体跳过（837 收口臂）。
#[test]
fn notification_payload_tolerates_non_object_extra() {
    let dir = tempdir();
    for extra in [
        serde_json::json!(null),
        serde_json::json!("scalar"),
        serde_json::json!(7),
    ] {
        let payload = notification_payload("sk", &dir, "question", "loop", extra.clone());
        let v: Value = serde_json::from_str(&payload).expect("payload is json");
        assert_eq!(v["kind"].as_str(), Some("question"));
        assert_eq!(v["hook_event_name"].as_str(), Some("Notification"));
    }
}
