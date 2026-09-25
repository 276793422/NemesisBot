//! vault CLI 测试：口令来源优先级、别名存在性判断等纯逻辑部分。

use super::*;

/// argon2 模式：环境变量存在 → 直接使用（不触发交互输入）。
#[test]
fn passphrase_from_env_var() {
    // 本测试进程内设置环境变量，验证取用路径（临时键避免污染）。
    unsafe {
        std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "env-passphrase-test");
    }
    let got = passphrase_for_mode(VaultMode::Argon2id).unwrap();
    unsafe {
        std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE");
    }
    assert_eq!(got.as_deref(), Some("env-passphrase-test"));
}

/// dpapi 模式：永远不需要口令。
#[cfg(windows)]
#[test]
fn dpapi_needs_no_passphrase() {
    assert_eq!(passphrase_for_mode(VaultMode::Dpapi).unwrap(), None);
}

/// CLI 侧 vault 路径解析与 nemesis-path 单一拼接点一致。
#[test]
fn vault_path_matches_path_helper() {
    let home = std::env::temp_dir().join("nemesisbot-vault-cli-path-test");
    let ws = common::workspace_path(&home);
    assert_eq!(
        nemesis_path::resolve_vault_path_in_workspace(&ws),
        ws.join("config").join("vault.enc")
    );
}

// ---------------------------------------------------------------------------
// P0 B2：vault migrate（模型 key 迁移）
// ---------------------------------------------------------------------------

use std::path::Path;

/// migrate 测试经 test_store 触碰 env 口令（argon2 平台）——互斥防踩。
static MIGRATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 写一个最小 config.json + credentials.yaml 到临时 home。
fn seed_config(home: &Path, model_entries: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(home).unwrap();
    let cfg = format!(r#"{{"model_list":[{model_entries}],"session":{{}},"gateway":{{}}}}"#);
    let p = home.join("config.json");
    std::fs::write(&p, cfg).unwrap();
    p
}

/// 明文 key → vault:<alias>，值入 vault，配置回写（typed round-trip）。
#[test]
fn migrate_inline_plaintext_key() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"gpt-main","model":"openai/gpt-4o","api_key":"sk-plain-123"}"#,
    );
    let cred_path = home.join("config").join("credentials.yaml");
    std::fs::create_dir_all(cred_path.parent().unwrap()).unwrap();

    let mut store = test_store(home);
    let report = super::migrate_model_keys(&config_path, &cred_path, &mut store).unwrap();

    assert_eq!(report.migrated.len(), 1);
    assert_eq!(report.migrated[0].1, "gpt-main");
    // 配置回写为引用，明文不在 config.json。
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("vault:gpt-main"), "got: {cfg_text}");
    assert!(!cfg_text.contains("sk-plain-123"));
    // 值在 vault 里（内存 store 可读），不在磁盘明文。
    assert_eq!(store.get("gpt-main").unwrap(), "sk-plain-123");
    let vp = nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(home));
    assert!(
        !vp.exists()
            || !std::fs::read_to_string(&vp)
                .unwrap()
                .contains("sk-plain-123")
    );
}

/// yaml: 引用：值搬进 vault（别名不变），配置回写 vault:，credentials.yaml
/// 条目移除——明文彻底离盘。
#[test]
fn migrate_yaml_reference_moves_value_out_of_yaml() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"claude-main","model":"anthropic/claude","api_key":"yaml:sk-claude"}"#,
    );
    let cred_path = home.join("config").join("credentials.yaml");
    std::fs::create_dir_all(cred_path.parent().unwrap()).unwrap();
    std::fs::write(&cred_path, "keys:\n  sk-claude: \"sk-ant-real\"\n").unwrap();

    let mut store = test_store(home);
    let report = super::migrate_model_keys(&config_path, &cred_path, &mut store).unwrap();

    assert_eq!(report.migrated.len(), 1);
    assert_eq!(report.migrated[0].1, "sk-claude");
    assert_eq!(store.get("sk-claude").unwrap(), "sk-ant-real");
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("vault:sk-claude"));
    let creds: nemesis_config::credentials::CredentialsFile =
        nemesis_config::credentials::load_credentials_file(&cred_path).unwrap();
    assert!(!creds.keys.contains_key("sk-claude"), "yaml 条目应已移除");
}

/// 幂等：第二次运行不再动已迁移条目；env: 引用保持原样。
#[test]
fn migrate_is_idempotent_and_keeps_env_refs() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"a","model":"openai/a","api_key":"env:MY_KEY"},
{"model_name":"b","model":"openai/b","api_key":"vault:existing"}"#,
    );

    let mut store = test_store(home);
    let report = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store,
    )
    .unwrap();
    assert!(report.is_noop());
    assert_eq!(report.skipped_env, 1);
    assert_eq!(report.already_vault, 1);
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("env:MY_KEY"));
}

/// 别名冲突：同名异值加后缀，绝不覆盖；同值复用。
#[test]
fn migrate_conflict_suffix_never_overwrites() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"dup","model":"openai/dup","api_key":"sk-new-value"}"#,
    );

    let mut store = test_store(home);
    store.set("dup", "sk-old-value", "model", "").unwrap();
    let report = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store,
    )
    .unwrap();
    assert_eq!(report.migrated[0].1, "dup__2");
    assert_eq!(store.get("dup").unwrap(), "sk-old-value", "原值未被覆盖");
    assert_eq!(store.get("dup__2").unwrap(), "sk-new-value");

    // 同值复用：再跑一次同值条目 → reused。
    store.set("same", "sk-same", "model", "").unwrap();
    let config_path = seed_config(
        home,
        r#"{"model_name":"same","model":"openai/same","api_key":"sk-same"}"#,
    );
    let mut store2 = store;
    let rep = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store2,
    )
    .unwrap();
    assert_eq!(rep.reused, 1);
    assert_eq!(rep.migrated[0].1, "same");
}

/// 断引 yaml: 引用：不迁移不丢引用，报告里指名。
#[test]
fn migrate_reports_broken_yaml_reference() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"ghost","model":"openai/ghost","api_key":"yaml:no-such-alias"}"#,
    );
    std::fs::create_dir_all(home.join("config")).unwrap();
    let mut store = test_store(home);
    let report = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store,
    )
    .unwrap();
    assert!(report.migrated.is_empty());
    assert_eq!(report.yaml_broken.len(), 1);
    // 配置未被改动（引用保留，等用户处理）。
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("yaml:no-such-alias"));
}

/// 测试用 vault store（默认模式；argon2 走固定口令）。
fn test_store(home: &Path) -> VaultStore {
    let vp = nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(home));
    std::fs::create_dir_all(vp.parent().unwrap()).unwrap();
    let mode = VaultStore::default_mode();
    let pw = if mode == VaultMode::Argon2id {
        Some("migrate-test-pw")
    } else {
        None
    };
    VaultStore::create(&vp, mode, pw).unwrap()
}

// ---------------------------------------------------------------------------
// wave_a（2026-09-25）：run() 各子命令臂（此前只有 migrate 核心与纯逻辑
// 被测，run() 的 Set/List/Remove/Migrate 分支 0 覆盖）。
// 纪律：
// - NEMESISBOT_HOME 指向临时目录 + crate::GLOBAL_STATE_LOCK 互斥；
// - NEMESISBOT_VAULT_PASSPHRASE 固定为 "migrate-test-pw"（与 test_store 的
//   argon2 口令一致，保证 run() 重开 test_store 建的 vault 能解锁）；
// - 交互输入一律走 stdin EOF（跑套件须 `cargo test ... < /dev/null`，
//   与 eval_rules cmd_reset_non_force_aborts_on_eof_stdin 同约定）；
// - run() 是 async，但全程无真正异步点——用独立 runtime block_on 包住，
//   避免跨 await 持锁问题。
// ---------------------------------------------------------------------------

mod run_arms {
    use super::super::{VaultAction, VaultMode, VaultStore};
    use super::test_store;

    /// env/home 夹具：持 GLOBAL_STATE_LOCK，drop 时清 env。
    struct RunArmEnv {
        _guard: std::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
        home: std::path::PathBuf,
    }

    impl Drop for RunArmEnv {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("NEMESISBOT_HOME") };
            unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
        }
    }

    fn run_arm_env() -> RunArmEnv {
        let guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        // workspace/config 预建（vault 落点；VaultStore::create 不建父目录）。
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "migrate-test-pw") };
        RunArmEnv {
            _guard: guard,
            _tmp: tmp,
            home,
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> <F as std::future::Future>::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }

    fn vault_file(home: &std::path::Path) -> std::path::PathBuf {
        nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(home))
    }

    /// 预置一个含单别名的已存盘 vault，返回别名值供回读。
    fn seed_vault(home: &std::path::Path, alias: &str, value: &str) {
        let mut store = test_store(home);
        store.set(alias, value, "model", "seeded-desc").unwrap();
        store.save().unwrap();
    }

    /// 重开已存在的 vault（test_store 只能建一次；二次 create 会 FileExists）。
    fn reopen_store(home: &std::path::Path) -> VaultStore {
        let vp = vault_file(home);
        let mut s = VaultStore::open(&vp).unwrap();
        if s.mode() == VaultMode::Argon2id {
            s.unlock("migrate-test-pw").unwrap();
        }
        s
    }

    #[test]
    fn list_on_missing_vault_creates_it_and_reports_empty() {
        let th = run_arm_env();
        assert!(!vault_file(&th.home).exists());
        block_on(super::super::run(VaultAction::List, false)).unwrap();
        // open_or_create 走 create 分支：vault 文件落盘。
        assert!(
            vault_file(&th.home).exists(),
            "List 于缺失 vault 时应创建空 vault"
        );
    }

    #[test]
    fn list_with_entries_prints_table_and_footer() {
        let th = run_arm_env();
        seed_vault(&th.home, "alpha-main", "v-alpha");
        let mut store = reopen_store(&th.home);
        store.set("beta-alt", "v-beta", "telegram", "d2").unwrap();
        store.save().unwrap();
        block_on(super::super::run(VaultAction::List, false)).unwrap();
        // 值与别名仍在（List 只读）。
        let store = reopen_store(&th.home);
        assert!(store.aliases().iter().any(|a| a == "alpha-main"));
        assert!(store.aliases().iter().any(|a| a == "beta-alt"));
    }

    #[test]
    fn remove_missing_alias_bails() {
        let th = run_arm_env();
        seed_vault(&th.home, "real-alias", "v");
        let err = block_on(super::super::run(
            VaultAction::Remove {
                alias: "ghost".into(),
            },
            false,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("别名不存在"), "实际：{err}");
    }

    #[test]
    fn remove_existing_cancelled_on_eof_stdin_keeps_alias() {
        let th = run_arm_env();
        seed_vault(&th.home, "keep-me", "v-keep");
        // prompt_confirm 读到 EOF → false → 「已取消」路径。
        block_on(super::super::run(
            VaultAction::Remove {
                alias: "keep-me".into(),
            },
            false,
        ))
        .unwrap();
        let store = reopen_store(&th.home);
        assert!(
            store.aliases().iter().any(|a| a == "keep-me"),
            "取消后别名必须仍在"
        );
    }

    #[test]
    fn set_stdin_eof_empty_secret_bails() {
        let _th = run_arm_env();
        // --stdin + EOF ⇒ 空 secret ⇒ bail（不写不存）。
        let err = block_on(super::super::run(
            VaultAction::Set {
                alias: "new-alias".into(),
                domain: None,
                description: None,
                force: false,
                stdin: true,
            },
            false,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("secret 为空"), "实际：{err}");
    }

    #[test]
    fn set_existing_alias_cancelled_on_eof_confirm() {
        let th = run_arm_env();
        seed_vault(&th.home, "dup-alias", "old-value");
        // existed && !force → 确认提示读 EOF → 取消。
        block_on(super::super::run(
            VaultAction::Set {
                alias: "dup-alias".into(),
                domain: Some("model".into()),
                description: None,
                force: false,
                stdin: true,
            },
            false,
        ))
        .unwrap();
        let store = reopen_store(&th.home);
        assert_eq!(store.get("dup-alias").unwrap(), "old-value", "取消不轮换");
    }

    #[test]
    fn migrate_via_run_writes_refs_and_full_report_then_noop_rerun() {
        let th = run_arm_env();
        // 种 config.json（两条明文）+ 空 credentials.yaml。
        std::fs::write(
            th.home.join("config.json"),
            r#"{"model_list":[
                {"model_name":"m1","model":"openai/m1","api_key":"sk-one"},
                {"model_name":"m2","model":"openai/m2","api_key":"env:KEEP"}],
                "session":{},"gateway":{}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(th.home.join("config")).unwrap();
        // 第一次：迁移 1 条（sk-one），env: 跳过。
        block_on(super::super::run(VaultAction::Migrate, false)).unwrap();
        let cfg_text = std::fs::read_to_string(th.home.join("config.json")).unwrap();
        assert!(cfg_text.contains("vault:m1"), "实际：{cfg_text}");
        assert!(cfg_text.contains("env:KEEP"), "env 引用必须原样");
        assert!(!cfg_text.contains("sk-one"));
        assert!(vault_file(&th.home).exists(), "迁移后 vault 落盘");
        // 第二次：全部已迁移/跳过 → is_noop 早退臂。
        block_on(super::super::run(VaultAction::Migrate, false)).unwrap();
        let cfg_text2 = std::fs::read_to_string(th.home.join("config.json")).unwrap();
        assert_eq!(cfg_text, cfg_text2, "幂等：二次迁移不得改配置");
    }
}

// ---------------------------------------------------------------------------
// cov 补测（2026-09-25）：`vault_alias_for` 别名冲突裁决四臂。
// （此前只有间接路径——Migrate 全链路——该裁决函数本体无直测。）
// ---------------------------------------------------------------------------
mod alias_arbiter {
    use super::super::{MigrateReport, VaultMode, VaultStore};

    /// tempdir + 独立已解锁 vault store（落 tempdir，无 env 操作，不持全局锁）。
    fn arb_store() -> (tempfile::TempDir, VaultStore) {
        let tmp = tempfile::tempdir().unwrap();
        let vp = tmp.path().join("vault.enc");
        let mode = VaultStore::default_mode();
        let pw = if mode == VaultMode::Argon2id {
            Some("arb-test-pw")
        } else {
            None
        };
        let mut store = VaultStore::create(&vp, mode, pw).unwrap();
        if mode == VaultMode::Argon2id {
            store.unlock("arb-test-pw").unwrap();
        }
        (tmp, store)
    }

    #[test]
    fn free_base_alias_is_returned_as_is() {
        let (_tmp, store) = arb_store();
        let mut report = MigrateReport::default();
        let alias = super::super::vault_alias_for(&store, "openai", "sk-lit-1", &mut report);
        assert_eq!(alias, "openai");
        assert_eq!(report.reused, 0);
        assert!(report.conflicts.is_empty(), "空闲别名不得记冲突");
    }

    #[test]
    fn same_literal_under_taken_alias_is_reused() {
        let (_tmp, mut store) = arb_store();
        store.set("openai", "sk-lit-1", "model", "").unwrap();
        let mut report = MigrateReport::default();
        let alias = super::super::vault_alias_for(&store, "openai", "sk-lit-1", &mut report);
        assert_eq!(alias, "openai", "同值复用现别名");
        assert_eq!(report.reused, 1);
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn different_literal_gets_numbered_suffix() {
        let (_tmp, mut store) = arb_store();
        store.set("openai", "sk-other", "model", "").unwrap();
        let mut report = MigrateReport::default();
        let alias = super::super::vault_alias_for(&store, "openai", "sk-lit-2", &mut report);
        assert_eq!(alias, "openai__2", "异值走 __2 后缀");
        assert_eq!(report.reused, 0);
        assert_eq!(
            report.conflicts,
            vec![("openai".to_string(), "openai__2".to_string())]
        );
    }

    #[test]
    fn suffix_chain_reuses_matching_value_and_skips_taken() {
        let (_tmp, mut store) = arb_store();
        store.set("openai", "v-base", "model", "").unwrap();
        store.set("openai__2", "v-two", "model", "").unwrap();
        store.set("openai__3", "v-target", "model", "").unwrap();
        let mut report = MigrateReport::default();
        // 目标字面量与 __3 相同：__2 被异值占位跳过，__3 同值复用。
        let alias = super::super::vault_alias_for(&store, "openai", "v-target", &mut report);
        assert_eq!(alias, "openai__3");
        assert_eq!(report.reused, 1, "命中同值后缀记复用");
        assert!(report.conflicts.is_empty(), "复用不记冲突");
    }
}

// ---------------------------------------------------------------------------
// wave6（2026-09-25）：run() 层吃剩的报告臂——List 行级 domain/description
// 非空臂、Migrate 的 conflicts/reused/yaml_broken 打印臂。
// 纪律同 run_arms：NEMESISBOT_HOME + NEMESISBOT_VAULT_PASSPHRASE 重定向 +
// GLOBAL_STATE_LOCK 互斥；vault 以 test_store 库 API 预置（绕 stdin）。
// ---------------------------------------------------------------------------
mod wave6 {
    use super::super::{VaultAction, VaultMode, VaultStore};
    use super::test_store;
    use std::path::Path;

    struct W6Env {
        _guard: std::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
        home: std::path::PathBuf,
    }
    impl Drop for W6Env {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("NEMESISBOT_HOME") };
            unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
        }
    }
    fn w6_env() -> W6Env {
        let guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "migrate-test-pw") };
        W6Env {
            _guard: guard,
            _tmp: tmp,
            home,
        }
    }
    fn block_on<F: std::future::Future>(f: F) -> <F as std::future::Future>::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(f)
    }
    fn write_model_config(home: &Path, api_key: &str) -> std::path::PathBuf {
        let p = home.join("config.json");
        std::fs::write(
            &p,
            r#"{"model_list":[{"model_name":"gpt-main","model":"openai/gpt-4o","api_key":"KEY"}],"session":{},"gateway":{}}"#
                .replace("KEY", api_key),
        )
        .unwrap();
        p
    }

    /// List：vault 已有条目（domain/description 非空）→ 表格行打印走
    /// 非空臂（"-" 空臂的镜像）。
    #[test]
    fn w6_list_rows_print_domain_and_description_arms() {
        let th = w6_env();
        let mut store = test_store(&th.home);
        store
            .set("w6-alias", "w6-value", "model", "w6-desc")
            .unwrap();
        store.save().unwrap();
        drop(store);
        block_on(super::super::run(VaultAction::List, false)).expect("List Ok");
    }

    /// Migrate：config 明文 key 撞上 vault 里已有**异值**别名 →
    /// conflicts 打印臂（原值不覆盖，改用后缀别名）。
    #[test]
    fn w6_migrate_conflicting_alias_prints_warning_and_suffixes() {
        let th = w6_env();
        {
            let mut store = test_store(&th.home);
            store
                .set("gpt-main", "sk-OTHER-value", "model", "seeded")
                .unwrap();
            store.save().unwrap();
        }
        write_model_config(&th.home, "sk-plain-123");
        block_on(super::super::run(VaultAction::Migrate, false)).expect("Migrate Ok");
        // 回读：原别名值保持不变（绝不覆盖）。
        let vp =
            nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(&th.home));
        let mut s = VaultStore::open(&vp).unwrap();
        if s.mode() == VaultMode::Argon2id {
            s.unlock("migrate-test-pw").unwrap();
        }
        assert_eq!(s.get("gpt-main").unwrap(), "sk-OTHER-value", "异值绝不覆盖");
        let cfg_text = std::fs::read_to_string(th.home.join("config.json")).unwrap();
        assert!(
            cfg_text.contains("vault:gpt-main__"),
            "配置必须改用后缀别名：{cfg_text}"
        );
    }

    /// Migrate：config 明文 key 与 vault 已有别名**同值** → reused 复用臂
    ///（不新建别名，配置回写既有别名）。
    #[test]
    fn w6_migrate_same_value_alias_reused() {
        let th = w6_env();
        {
            let mut store = test_store(&th.home);
            store
                .set("gpt-main", "sk-plain-123", "model", "seeded")
                .unwrap();
            store.save().unwrap();
        }
        write_model_config(&th.home, "sk-plain-123");
        block_on(super::super::run(VaultAction::Migrate, false)).expect("Migrate Ok");
        let cfg_text = std::fs::read_to_string(th.home.join("config.json")).unwrap();
        assert!(
            cfg_text.contains("vault:gpt-main"),
            "配置回写既有别名：{cfg_text}"
        );
        assert!(!cfg_text.contains("sk-plain-123"), "明文必须离盘");
    }

    /// Migrate：config 引用 yaml:ghost 但 credentials.yaml 无此条目 →
    /// yaml_broken 打印臂（未迁移、诚实报告）。
    #[test]
    fn w6_migrate_broken_yaml_reference_prints_warning() {
        let th = w6_env();
        write_model_config(&th.home, "yaml:ghost-alias");
        block_on(super::super::run(VaultAction::Migrate, false)).expect("Migrate Ok");
        let cfg_text = std::fs::read_to_string(th.home.join("config.json")).unwrap();
        assert!(
            cfg_text.contains("yaml:ghost-alias"),
            "断裂引用必须原样保留：{cfg_text}"
        );
    }
}
