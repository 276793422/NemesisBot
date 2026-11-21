//! 危险闸页面（2026-11-20）：security.exec_deny_list / exec_deny_set
//! WSAPI 契约测试。
//!
//! 路径维度（2026-11-21 修复后契约）：config.json 在 **home 根**——与
//! exec 工具热读路径 `shared.home.join("config.json")` 同一文件。夹具
//! 仿生产装配形态：home = 临时目录、workspace = 其下 workspace/ 子目录
//! （两者是不同文件——write-then-verify 钉死写侧落 home 而非 workspace，
//! 防夹具自洽假绿：早期版本两者同路径时此缺陷测不出来）。测试进程无
//! live store，读侧恒磁盘直读；ctx 构造同 rule_mgmt_tests make_ctx 形态。

use super::*;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

fn make_ctx(home_dir: &str) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let workspace = std::path::Path::new(home_dir)
        .join("workspace")
        .to_string_lossy()
        .to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(workspace.clone()),
        home: Some(home_dir.to_string()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        mail_plugin: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "exec-deny".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(workspace),
        home: Some(home_dir.to_string()),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// list 契约：42 条元数据（name/category/severity/reason/pattern/enabled）
/// + 缺省形态全 enabled + extra 空 + 高级出口态可见。
#[tokio::test]
async fn exec_deny_list_default_shape() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let v = handler
        .handle_cmd("exec_deny_list", None, &make_ctx(&home_dir))
        .await
        .unwrap()
        .unwrap();
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len(),
        "条目数与词表一致"
    );
    let e0 = &entries[0];
    for key in [
        "name", "category", "severity", "reason", "pattern", "enabled",
    ] {
        assert!(
            e0.get(key).is_some(),
            "词条必须带 {key} 字段（页面呈现契约）"
        );
    }
    assert!(
        entries.iter().all(|e| e["enabled"] == true),
        "缺省形态全 enabled"
    );
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ssh_remote_login"), "起因条目必须在场");
    assert_eq!(v["extra"].as_array().unwrap().len(), 0);
    assert_eq!(v["custom_replacement_active"], false);
}

/// list 的 enabled 投影反映 deny_disabled（预先手写 config.json 形态——
/// 页面加载读的是盘上真相源；路径 = **home 根** config.json，
/// handlers::config::config_path 同源，exec 热读同文件）。
#[tokio::test]
async fn exec_deny_list_projects_disabled_state() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"tools":{"exec":{"deny_disabled":["ssh_remote_login","docker_exec"]}}}"#,
    )
    .unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let v = handler
        .handle_cmd("exec_deny_list", None, &make_ctx(&home_dir))
        .await
        .unwrap()
        .unwrap();
    let entries = v["entries"].as_array().unwrap();
    let by_name: std::collections::HashMap<String, bool> = entries
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["enabled"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(!by_name["ssh_remote_login"]);
    assert!(!by_name["docker_exec"]);
    assert!(by_name["sudo"], "未点名的条目保持 enabled");
}

/// set→list 往返：disabled 过滤生效 + extra 落盘 + 高级出口态不动。
/// 回归锁（2026-11-21 路径错位缺陷）：写入落 **home 根** config.json，
/// `{home}/workspace/config.json` 必须保持缺席——早期缺陷版写错维度时
/// 本断言当场红（夹具 home/workspace 同路径时测不出，故两者刻意分开）。
#[tokio::test]
async fn exec_deny_set_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let v = handler
        .handle_cmd(
            "exec_deny_set",
            Some(serde_json::json!({
                "disabled": ["ssh_remote_login", "not_a_real_rule", "ssh_remote_login"],
                "extra": ["forbidden_binary\\.exe", "  evil\\.exe  ", "forbidden_binary\\.exe"]
            })),
            &make_ctx(&home_dir),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["saved"], true);
    assert_eq!(v["hot"], true, "热生效语义必须在响应里声明");

    // 盘上真相源：未知 name 与重复条目被清洗（home 根）
    let cfg = nemesis_config::load_config(&super::super::config::config_path(&home_dir)).unwrap();
    assert_eq!(
        cfg.tools.exec.deny_disabled,
        vec!["ssh_remote_login".to_string()],
        "未知 name 过滤 + 去重"
    );
    assert_eq!(
        cfg.tools.exec.deny_extra,
        vec![
            r"forbidden_binary\.exe".to_string(),
            "evil\\.exe".to_string()
        ],
        "trim + 去重"
    );
    // 错位维度必须保持缺席（写 home 而非 workspace）
    let workspace_cfg = dir.path().join("workspace").join("config.json");
    assert!(
        !workspace_cfg.exists(),
        "写侧必须落 home 根 config.json，不得写 {}",
        workspace_cfg.display()
    );

    // list 回读投影一致
    let v2 = handler
        .handle_cmd("exec_deny_list", None, &make_ctx(&home_dir))
        .await
        .unwrap()
        .unwrap();
    let entries = v2["entries"].as_array().unwrap();
    let off: Vec<&str> = entries
        .iter()
        .filter(|e| e["enabled"] == false)
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(off, vec!["ssh_remote_login"]);
    assert_eq!(v2["extra"].as_array().unwrap().len(), 2);
}

/// set 写侧校验 fail loud：坏正则按序号报错、不落盘（abort 语义）。
#[tokio::test]
async fn exec_deny_set_rejects_bad_regex_and_aborts() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let err = handler
        .handle_cmd(
            "exec_deny_set",
            Some(serde_json::json!({
                "disabled": ["sudo"],
                "extra": ["good_pattern\\.exe", "([unclosed"]
            })),
            &make_ctx(&home_dir),
        )
        .await
        .unwrap_err();
    assert!(err.contains("2"), "错误须指明条目序号: {err}");
    // abort：盘上 config 不存在（闭包 Err 不落盘）
    let cfg_path = super::super::config::config_path(&home_dir);
    assert!(!cfg_path.exists(), "校验失败不得落盘");
    // 空规则同样拒绝
    let err2 = handler
        .handle_cmd(
            "exec_deny_set",
            Some(serde_json::json!({"disabled": [], "extra": ["   "]})),
            &make_ctx(&home_dir),
        )
        .await
        .unwrap_err();
    assert!(!err2.is_empty());
}

/// extra 上限 50：超限 loud 拒绝。
#[tokio::test]
async fn exec_deny_set_rejects_extra_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let many: Vec<String> = (0..51).map(|i| format!("pattern_{i}")).collect();
    let err = handler
        .handle_cmd(
            "exec_deny_set",
            Some(serde_json::json!({ "disabled": [], "extra": many })),
            &make_ctx(&home_dir),
        )
        .await
        .unwrap_err();
    assert!(err.contains("50"), "超限错误须带上限值: {err}");
}

/// 保存即生效（hot 语义端到端）：set 后 exec 热读路径消费新词表——
/// 关 ssh → exec 放行 ssh 命中面（deny 层），其余防线保留。
/// 用 nemesis-agent 的 build_effective_deny 同源合成函数验证盘上 config
/// 的消费语义（同 crate 内 exec_deny_tests 已钉工具行为，这里锁两键的
/// 盘上形态可被同源消费）。
#[tokio::test]
async fn exec_deny_set_feeds_agent_effective_deny() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    handler
        .handle_cmd(
            "exec_deny_set",
            Some(serde_json::json!({
                "disabled": ["ssh_remote_login"],
                "extra": ["hook_probe\\.exe"]
            })),
            &make_ctx(&home_dir),
        )
        .await
        .unwrap()
        .unwrap();
    let cfg = nemesis_config::load_config(&super::super::config::config_path(&home_dir)).unwrap();
    let entries = nemesis_agent::loop_tools::build_effective_deny(&cfg.tools.exec);
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"ssh_remote_login"), "被关条目不在生效表");
    let hit = entries
        .iter()
        .find(|e| e.name == "extra:0")
        .expect("追加条目在生效表");
    assert!(hit.regex.is_match("run hook_probe.exe"));
    assert!(entries.iter().any(|e| e.name == "sudo"), "未关条目防线保留");
}

/// 总览行快照（security.overview `exec_deny` 键）语义锁（2026-11-21）：
/// disabled 按**唯一内置名**计数——手编 config 重复名不去重会把
/// disabled 顶过 total，前端行显示负数「-N/42 条生效」；未知名不计入
/// （与消费侧 build_effective_deny 的按名过滤一致）。
#[test]
fn exec_deny_snapshot_counts_unique_disabled() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"tools":{"exec":{"deny_disabled":["ssh_remote_login","ssh_remote_login","docker_exec","not_a_real_rule"]}}}"#,
    )
    .unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    let v = SecurityHandler::exec_deny_snapshot(&make_ctx(&home_dir));
    assert_eq!(v["available"], true);
    assert_eq!(
        v["total"].as_u64().unwrap(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len() as u64
    );
    assert_eq!(v["disabled"], 2, "重复名去重 + 未知名不计入");
    assert_eq!(v["extra"], 0);
    assert_eq!(v["custom_replacement"], false);
}

/// 替换态判定 flag 门槛（镜像 build_effective_deny / exec_deny_list
/// `custom_replacement_active` 语义——2026-11-21 复查修复）：custom
/// 非空但 `enable_deny_patterns` 缺席（serde default false）或 false =
/// 替换**不**激活（内置条目开关仍参与生效表）；flag=true + 非空才报
/// true。缺此闸时总览页在 flag 关 + custom 残留的 config 上误报
/// 「自定义词表」。
#[test]
fn exec_deny_snapshot_custom_replacement_needs_flag() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();

    // flag 缺席 + custom 非空 → 不激活（serde default false）
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"tools":{"exec":{"custom_deny_patterns":["evil\\.exe"]}}}"#,
    )
    .unwrap();
    let v = SecurityHandler::exec_deny_snapshot(&make_ctx(&home_dir));
    assert_eq!(v["custom_replacement"], false, "flag 缺席 = 不激活");

    // flag=false + custom 非空 → 不激活
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"tools":{"exec":{"enable_deny_patterns":false,"custom_deny_patterns":["evil\\.exe"]}}}"#,
    )
    .unwrap();
    let v = SecurityHandler::exec_deny_snapshot(&make_ctx(&home_dir));
    assert_eq!(v["custom_replacement"], false, "flag=false = 不激活");

    // flag=true + custom 非空 → 激活
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"tools":{"exec":{"enable_deny_patterns":true,"custom_deny_patterns":["evil\\.exe"]}}}"#,
    )
    .unwrap();
    let v = SecurityHandler::exec_deny_snapshot(&make_ctx(&home_dir));
    assert_eq!(v["custom_replacement"], true, "flag=true + 非空 = 激活");
}

/// flatten 保真端到端（2026-11-21）：exec_deny_set 走 typed update_config
/// 经手 tools.exec 段——保存后段内未建模键（手编实验参数 / 未来版本新增
/// 键）必须原样保留（ExecConfig.extra flatten 兜底；没有它时 typed 序列化
/// 会静默抹键）。回归锁 = config 层 roundtrip 测试的管理面消费端。
#[tokio::test]
async fn exec_deny_set_preserves_untyped_exec_keys() {
    let dir = tempfile::tempdir().unwrap();
    let home_dir = dir.path().to_string_lossy().to_string();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"tools":{"exec":{"some_future_key":{"tuning":[1,2]}}}}"#,
    )
    .unwrap();
    let handler = SecurityHandler::new();
    handler
        .handle_cmd(
            "exec_deny_set",
            Some(serde_json::json!({
                "disabled": ["ssh_remote_login"],
                "extra": []
            })),
            &make_ctx(&home_dir),
        )
        .await
        .unwrap()
        .unwrap();
    let raw = crate::handlers::config::read_raw_config(&home_dir).unwrap();
    let exec = &raw["tools"]["exec"];
    assert_eq!(
        exec["deny_disabled"],
        serde_json::json!(["ssh_remote_login"]),
        "管辖键正常落盘"
    );
    assert_eq!(
        exec["some_future_key"]["tuning"][1], 2,
        "保存后未建模键必须保留（flatten extra 兜底）"
    );
}
