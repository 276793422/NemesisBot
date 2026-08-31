//! credentials 命令单测：run() 的完整迁移路径 + no-op 路径（真命令函数，
//! 临时 home + 进程锁）。核心迁移逻辑（run_import）在 nemesis-config 的
//! credentials/tests.rs 有自己的单测；这里钉的是 CLI 入口的接线：
//! env home 解析 → 全局 credentials 路径注入 → 报表打印不 Err。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

/// 环境操作（NEMESISBOT_HOME）+ run() 会 set_global_credentials_path
/// （进程全局）——两样都串行 + 事后还原。
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
    let prev_global = nemesis_config::credentials::global_credentials_path();
    f(tmp.path().to_path_buf()).await;
    // 还原：环境 + 全局 credentials 路径。
    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
    match prev_global {
        Some(p) => nemesis_config::credentials::set_global_credentials_path(p),
        None => nemesis_config::credentials::clear_global_credentials_path(),
    }
}

#[tokio::test]
async fn import_without_config_json_is_clean_noop() {
    with_locked_env_home(|home| async move {
        // resolve_home → {tmp}/.nemesisbot；其下没有 config.json →
        // run_import 直接 noop，不得创建任何文件。
        let home = home.join(".nemesisbot");
        run(CredentialsAction::Import, false)
            .await
            .expect("noop 路径 Ok");
        assert!(!home.join("config.json").exists(), "不得反向创建 config.json");
        assert!(
            !home.join("workspace/config/credentials.yaml").exists(),
            "noop 不写 credentials.yaml"
        );
    })
    .await;
}

#[tokio::test]
async fn import_migrates_plaintext_key_and_rewrites_config() {
    with_locked_env_home(|home| async move {
        let home = home.join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            r#"{"model_list": [
                {"model_name": "m1", "model": "prov/m1", "api_key": "sk-plain-123"},
                {"model_name": "m2", "model": "prov/m2", "api_key": "yaml:existing"},
                {"model_name": "m3", "model": "prov/m3", "api_key": ""}
            ]}"#,
        )
        .unwrap();

        run(CredentialsAction::Import, false).await.expect("import ok");

        // credentials.yaml 落盘且含明文 key。
        let creds_path = nemesis_config::credentials::credentials_path_for_home(&home);
        let creds = std::fs::read_to_string(&creds_path).expect("credentials.yaml written");
        assert!(creds.contains("sk-plain-123"), "creds: {creds}");
        assert!(creds.contains("m1"), "alias 来自 model_name");

        // config.json 里明文改写成 yaml: 引用；已是引用/空的保持原样。
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["model_list"][0]["api_key"], "yaml:m1");
        assert_eq!(cfg["model_list"][1]["api_key"], "yaml:existing");
        assert_eq!(cfg["model_list"][2]["api_key"], "");

        // run() 顺带把本进程全局解析路径指向同一文件（接线断言）。
        assert_eq!(
            nemesis_config::credentials::global_credentials_path(),
            Some(creds_path)
        );
    })
    .await;
}

#[tokio::test]
async fn import_is_idempotent_on_second_run() {
    with_locked_env_home(|home| async move {
        let home = home.join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            r#"{"model_list": [
                {"model_name": "m1", "model": "prov/m1", "api_key": "sk-plain-123"}
            ]}"#,
        )
        .unwrap();

        run(CredentialsAction::Import, false).await.expect("first ok");
        let cfg_after_first = std::fs::read_to_string(home.join("config.json")).unwrap();
        let creds_after_first =
            std::fs::read_to_string(nemesis_config::credentials::credentials_path_for_home(&home))
                .unwrap();

        // 第二遍：全部已是引用 → noop，两文件字节不变。
        run(CredentialsAction::Import, false).await.expect("second ok");
        assert_eq!(
            std::fs::read_to_string(home.join("config.json")).unwrap(),
            cfg_after_first,
            "第二遍不得再改 config"
        );
        assert_eq!(
            std::fs::read_to_string(nemesis_config::credentials::credentials_path_for_home(&home))
                .unwrap(),
            creds_after_first,
            "第二遍不得改写 credentials"
        );
    })
    .await;
}

// ===========================================================================
// reused / conflicts 报表行（S11c，quality-hardening goal 冲刺 S11）——
// 此前 MISSED：48（复用 N 个）与 51-55（冲突警告）。预置 credentials.yaml
// 制造同值复用 / 异值冲突两种状态。
// ===========================================================================

#[tokio::test]
async fn import_reuses_existing_alias_with_same_value() {
    with_locked_env_home(|home| async move {
        let home = home.join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            r#"{"model_list": [
                {"model_name": "m1", "model": "prov/m1", "api_key": "sk-shared"}
            ]}"#,
        )
        .unwrap();
        let creds_path = nemesis_config::credentials::credentials_path_for_home(&home);
        std::fs::create_dir_all(creds_path.parent().unwrap()).unwrap();
        // 预置同值 alias → run_import 走 reused 分支（不覆盖文件内容）。
        std::fs::write(&creds_path, "keys:\n  m1: sk-shared\n").unwrap();

        run(CredentialsAction::Import, false).await.expect("import ok");

        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["model_list"][0]["api_key"], "yaml:m1");
        assert!(
            std::fs::read_to_string(&creds_path).unwrap().contains("sk-shared"),
            "同值复用不改动 credentials"
        );
    })
    .await;
}

#[tokio::test]
async fn import_conflicting_alias_gets_suffixed_and_warns() {
    with_locked_env_home(|home| async move {
        let home = home.join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.json"),
            r#"{"model_list": [
                {"model_name": "m1", "model": "prov/m1", "api_key": "sk-plain-9"}
            ]}"#,
        )
        .unwrap();
        let creds_path = nemesis_config::credentials::credentials_path_for_home(&home);
        std::fs::create_dir_all(creds_path.parent().unwrap()).unwrap();
        // 预置同 alias 异值 → 新 key 落到 m1__2，原值不被覆盖。
        std::fs::write(&creds_path, "keys:\n  m1: other-value\n").unwrap();

        run(CredentialsAction::Import, false).await.expect("import ok");

        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            cfg["model_list"][0]["api_key"], "yaml:m1__2",
            "冲突时改用 __2 后缀 alias"
        );
        let creds = std::fs::read_to_string(&creds_path).unwrap();
        assert!(creds.contains("other-value"), "原 alias 值不得被覆盖");
        assert!(creds.contains("sk-plain-9"), "新 key 落在 m1__2");
    })
    .await;
}
