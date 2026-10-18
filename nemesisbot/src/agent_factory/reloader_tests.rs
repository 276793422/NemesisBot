//! B4（2026-10-18 凭据 vault §7）ModelProviderReloader 测试。
//!
//! 覆盖：credential_request_fields 三分支（vault 别名轮换 / 明文提议 /
//! 条目缺失提议）+ reload_provider patch 写盘语义（api_key → `vault:<alias>`
//! raw patch 原子写、None 不动文件、条目缺失 Err 且原文不动）。provider
//! 重建用 mini 档离线构造安全条目（create_provider 只建 client 结构）。

use std::path::{Path, PathBuf};

use super::ModelProviderReloader;
// trait 方法（credential_request_fields/reload_provider）经 trait 调用——须在 scope。
use nemesis_agent::r#loop::ProviderReloader as _;

/// 唯一临时目录（Windows SystemTime tick 粒度粗——键加 AtomicU64 序号 +
/// pid，同 projects_factory_tests 夹具纪律）。
fn unique_home(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nb_reloader_{tag}_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        seq
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 单条目 config（明文或 vault 引用 api_key 由调用方给）。
fn write_config(home: &Path, api_key: &str) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [ {
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": api_key,
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        } ]
    });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn read_entry_api_key(home: &Path) -> String {
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    raw["model_list"][0]["api_key"]
        .as_str()
        .unwrap()
        .to_string()
}

fn reloader_for(home: &Path) -> ModelProviderReloader {
    ModelProviderReloader {
        home: home.to_path_buf(),
        workspace: home.to_path_buf(),
    }
}

/// 全局解析器槽位是进程单例——触及它的测试互斥（同
/// nemesis-agent credential_injection_tests 纪律）。
static VAULT_SLOT_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[test]
fn fields_vault_alias_is_rotation() {
    let home = unique_home("rot");
    write_config(&home, "vault:cust/alias");
    let (service, key_name) = reloader_for(&home).credential_request_fields("testai/mini-model");
    assert_eq!(service, "model/testai/mini-model");
    // 已是 vault 引用 = 轮换语义：沿用既有别名（卡片覆盖写）。
    assert_eq!(key_name, "cust/alias");
}

#[test]
fn fields_plaintext_proposes_model_alias() {
    let home = unique_home("plain");
    write_config(&home, "sk-plain");
    let (service, key_name) = reloader_for(&home).credential_request_fields("testai/mini-model");
    assert_eq!(service, "model/testai/mini-model");
    assert_eq!(key_name, "model/testai/mini-model");
}

#[test]
fn fields_missing_entry_or_config_still_proposes() {
    // config 文件存在但无匹配条目。
    let home = unique_home("noentry");
    write_config(&home, "sk-plain");
    let (service, key_name) = reloader_for(&home).credential_request_fields("other-model");
    assert_eq!(service, "model/other-model");
    assert_eq!(key_name, "model/other-model");
    // config 文件不存在（onboard 前异常态）也诚实提议，不发卡失败。
    let empty = unique_home("nofile");
    let (service, key_name) = reloader_for(&empty).credential_request_fields("m2");
    assert_eq!(service, "model/m2");
    assert_eq!(key_name, "model/m2");
}

#[test]
fn reload_patch_writes_vault_ref_and_rebuilds() {
    // rebuild 段 load_config 会解析刚写入的 `vault:` 引用——进程级 resolver
    // 槽装 stub（真实装配件是 vault_runtime::install，gateway 启动时挂）。
    let _g = VAULT_SLOT_LOCK.lock();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        Ok(format!("RESOLVED:{alias}"))
    }));
    let home = unique_home("patch");
    write_config(&home, "sk-plain");
    let r = reloader_for(&home);
    let result = r.reload_provider("testai/mini-model", Some("model/testai/mini-model"));
    nemesis_config::clear_global_vault_resolver();
    let provider = result.expect("patch + rebuild 应成功（离线构造安全条目）");
    // 条目 api_key 已改写为 vault 引用（config 不落明文）。
    assert_eq!(read_entry_api_key(&home), "vault:model/testai/mini-model");
    // 重建产物可用（非空 Arc）。
    let _ = provider;
}

#[test]
fn reload_none_keeps_file_intact() {
    let home = unique_home("nopatch");
    write_config(&home, "sk-plain");
    let r = reloader_for(&home);
    r.reload_provider("testai/mini-model", None)
        .expect("None 路径纯重读装配应成功");
    // 文件未被触碰：api_key 仍是明文原值。
    assert_eq!(read_entry_api_key(&home), "sk-plain");
}

#[test]
fn reload_patch_missing_entry_errs_file_intact() {
    let home = unique_home("badentry");
    write_config(&home, "sk-plain");
    let r = reloader_for(&home);
    let err = match r.reload_provider("no-such-model", Some("model/no-such-model")) {
        Ok(_) => panic!("条目缺失应诚实报错"),
        Err(e) => e,
    };
    assert!(err.contains("no-such-model"), "错误应含模型名: {err}");
    // 原文件不动（patch 失败 = 写盘前短路）。
    assert_eq!(read_entry_api_key(&home), "sk-plain");
}
