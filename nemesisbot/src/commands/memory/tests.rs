//! memory 命令单测（S11c 重写：原文件在 cmd_* 签名重构后成为孤儿 +
//! 陈旧——调用已不存在的 `common::Paths::from_home` 与 2 参 cmd_status/
//! cmd_disable，编译不过，BUG S11c-1。现按现行 1 参签名重写并补
//! cmd_enable 插件缺失错误路径 + run() 分发）。

use super::*;
use tempfile::TempDir;

fn setup_home(config: &serde_json::Value) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("config.json");
    std::fs::write(&cfg_path, serde_json::to_string_pretty(config).unwrap()).unwrap();
    (tmp, home)
}

/// config.enhanced_memory.json 的目录（现行布局：{home}/workspace/config）。
fn em_config_dir(home: &Path) -> std::path::PathBuf {
    common::enhanced_memory_config_path(home)
        .parent()
        .unwrap()
        .to_path_buf()
}

/// 旧 Paths::from_home 测试的等价断言（common 路径函数布局钉死）。
#[test]
fn test_common_memory_paths_layout() {
    let (_tmp, home) = setup_home(&serde_json::json!({}));
    let em = common::enhanced_memory_config_path(&home);
    assert!(em.ends_with("workspace\\config\\config.enhanced_memory.json")
        || em.ends_with("workspace/config/config.enhanced_memory.json"));
    assert!(em.starts_with(&home));
    let dir = em_config_dir(&home);
    assert!(dir.ends_with("config"));
}

// ===========================================================================
// read_main_switch / set_main_switch / has_onnx_files（纯函数，原样保留）
// ===========================================================================

#[test]
fn test_read_main_switch_no_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    assert!(!cfg_path.exists());
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_enabled() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({"memory": {"enabled": true}})).unwrap(),
    )
    .unwrap();
    assert_eq!(read_main_switch(&cfg_path), true);
}

#[test]
fn test_read_main_switch_disabled() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({"memory": {"enabled": false}})).unwrap(),
    )
    .unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_missing_field() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({ "agents": {} })).unwrap(),
    )
    .unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_memory_object_without_enabled() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({ "memory": { "some_other_key": "value" } }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(&cfg_path, "not valid json{{{").unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_set_main_switch_enable() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({ "agents": {} })).unwrap(),
    )
    .unwrap();

    set_main_switch(&cfg_path, true).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], true);
    assert!(updated.get("agents").is_some());
}

#[test]
fn test_set_main_switch_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({ "memory": { "enabled": true } })).unwrap(),
    )
    .unwrap();

    set_main_switch(&cfg_path, false).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], false);
}

#[test]
fn test_set_main_switch_creates_memory_object() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(&cfg_path, serde_json::to_string(&serde_json::json!({})).unwrap()).unwrap();

    set_main_switch(&cfg_path, true).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], true);
}

#[test]
fn test_set_main_switch_preserves_existing_memory_fields() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(
            &serde_json::json!({"memory": {"enabled": false, "some_setting": "preserve_me"}}),
        )
        .unwrap(),
    )
    .unwrap();

    set_main_switch(&cfg_path, true).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], true);
    assert_eq!(updated["memory"]["some_setting"], "preserve_me");
}

#[test]
fn test_set_main_switch_no_config_file() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("nonexistent").join("config.json");
    let result = set_main_switch(&cfg_path, true);
    assert!(result.is_err());
}

#[test]
fn test_set_main_switch_toggle_multiple_times() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::to_string(&serde_json::json!({ "memory": { "enabled": false } })).unwrap(),
    )
    .unwrap();

    set_main_switch(&cfg_path, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["memory"]["enabled"], true);

    set_main_switch(&cfg_path, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["memory"]["enabled"], false);

    set_main_switch(&cfg_path, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["memory"]["enabled"], true);
}

#[test]
fn test_has_onnx_files_with_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), "dummy").unwrap();
    assert!(has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_nested() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    let nested = dir.join("subdir");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("model.onnx"), "dummy").unwrap();
    assert!(has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_empty_dir() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(!has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_wrong_extension() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.bin"), "dummy").unwrap();
    assert!(!has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_nonexistent_dir() {
    assert!(!has_onnx_files(std::path::Path::new("/nonexistent/path")));
}

#[test]
fn test_detect_plugin_path_returns_option() {
    // 测试 exe 旁无 plugins/ 目录 → None（确定性）；只验证不 panic。
    let _ = detect_plugin_path();
}

// ===========================================================================
// cmd_status（1 参签名）
// ===========================================================================

#[test]
fn test_cmd_status_no_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    cmd_status(&home).expect("无 config.json → Ok（打印 DISABLED）");
}

#[test]
fn test_cmd_status_enabled_and_sub_enabled() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": true}}));
    let dir = em_config_dir(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        common::enhanced_memory_config_path(&home),
        serde_json::to_string(&serde_json::json!({ "enabled": true })).unwrap(),
    )
    .unwrap();
    cmd_status(&home).expect("status ok");
    assert_eq!(read_main_switch(&home.join("config.json")), true);
}

#[test]
fn test_cmd_status_main_on_sub_off_prints_degraded() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": true}}));
    let dir = em_config_dir(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        common::enhanced_memory_config_path(&home),
        serde_json::to_string(&serde_json::json!({ "enabled": false })).unwrap(),
    )
    .unwrap();
    cmd_status(&home).expect("main on / sub off → DEGRADED 分支");
}

#[test]
fn test_cmd_status_main_off_prints_disabled() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": false}}));
    cmd_status(&home).expect("main off → DISABLED 分支");
}

#[test]
fn test_status_with_corrupt_enhanced_memory_config() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": true}}));
    let dir = em_config_dir(&home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        common::enhanced_memory_config_path(&home),
        "not valid json{{{",
    )
    .unwrap();
    cmd_status(&home).expect("PARSE ERROR 分支不 panic");
}

#[test]
fn test_status_with_read_error_enhanced_memory_config() {
    // 路径存在但是目录 → read_to_string Err → READ ERROR 分支。
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": true}}));
    std::fs::create_dir_all(common::enhanced_memory_config_path(&home)).unwrap();
    cmd_status(&home).expect("READ ERROR 分支不 panic");
}

// ===========================================================================
// cmd_disable（1 参签名）
// ===========================================================================

#[test]
fn test_cmd_disable_turns_off_both_switches() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": true}}));
    let dir = em_config_dir(&home);
    std::fs::create_dir_all(&dir).unwrap();
    let em_path = common::enhanced_memory_config_path(&home);
    std::fs::write(&em_path, serde_json::to_string(&serde_json::json!({ "enabled": true })).unwrap())
        .unwrap();

    cmd_disable(&home).expect("disable ok");

    let cfg_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg_data["memory"]["enabled"], false);
    let em_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&em_path).unwrap()).unwrap();
    assert_eq!(em_data["enabled"], false);
}

#[test]
fn test_cmd_disable_no_config_errors() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    assert!(cmd_disable(&home).is_err(), "缺 config.json → bail");
}

#[test]
fn test_cmd_disable_no_enhanced_memory_config_creates_it_off() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": true}}));
    cmd_disable(&home).expect("disable ok（em 配置缺省 → 写 enabled=false）");
    let em_path = common::enhanced_memory_config_path(&home);
    assert!(em_path.exists(), "load+save 会落盘 em 配置");
    let em_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&em_path).unwrap()).unwrap();
    assert_eq!(em_data["enabled"], false);
    let cfg_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg_data["memory"]["enabled"], false);
}

// ===========================================================================
// cmd_enable —— 测试环境 {exe_dir}/plugins/ 必无 plugin_onnx（确定性）→
// Step 1 bail。插件就绪路径（Step 2-4）结构性豁免（需真实 dll+模型）。
// ===========================================================================

#[tokio::test]
async fn test_cmd_enable_bails_without_plugin_and_writes_no_config() {
    let (_tmp, home) = setup_home(&serde_json::json!({"memory": {"enabled": false}}));
    let err = cmd_enable(&home).await.expect_err("无插件 → bail");
    assert!(err.to_string().contains("Plugin"), "got: {err:#}");
    // bail 在任何开关写入之前：config.json 不变、em 配置不落盘。
    let cfg_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg_data["memory"]["enabled"], false, "主开关不得被提前置 true");
    assert!(
        !common::enhanced_memory_config_path(&home).exists(),
        "em 配置不得落盘"
    );
    // 但 config 目录已建（bail 前 create_dir_all 是可接受的副作用）。
    assert!(em_config_dir(&home).exists());
}

// ===========================================================================
// run() 分发（env home 隔离 + 进程锁）
// ===========================================================================

async fn with_locked_env_home<F, Fut>(f: F)
where
    F: FnOnce(std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", tmp.path());
    }
    f(tmp.path().to_path_buf()).await;
    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
}

#[tokio::test]
async fn run_status_and_disable_dispatch_via_env_home() {
    with_locked_env_home(|tmp| async move {
        let home = tmp.join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            serde_json::to_string(&serde_json::json!({"memory": {"enabled": true}})).unwrap(),
        )
        .unwrap();

        run(MemoryAction::Status, false).await.expect("status ok");
        run(MemoryAction::Disable, false).await.expect("disable ok");
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["memory"]["enabled"], false, "run Disable 经分发生效");
    })
    .await;
}

#[tokio::test]
async fn run_enable_without_plugin_errors_via_env_home() {
    with_locked_env_home(|tmp| async move {
        let home = tmp.join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            serde_json::to_string(&serde_json::json!({})).unwrap(),
        )
        .unwrap();

        let err = run(MemoryAction::Enable, false)
            .await
            .expect_err("插件缺失 → run Enable Err");
        assert!(err.to_string().contains("Plugin"), "got: {err:#}");
    })
    .await;
}
