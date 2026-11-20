//! #2 三档基线（W2，2026-11-20）：档位 → 默认对话模式的效果面测试。
//!
//! 装配链路 `build_agent_loop` → `apply_default_chat_mode`：
//! `security.level = "strict"` → loop 以 plan 模式起步（写类工具闸先
//! 收紧，`/build` 或 `chat.set_mode` 放开）；basic（缺席 = 出厂形态）与
//! standard → build（升级零行为突变）。独立夹具（不复用 tests.rs 的
//! cluster 门夹具——本模块无 feature 门，默认构建必跑）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{SharedResources, build_agent_loop};
use nemesis_agent::AgentMode;

// ---------------------------------------------------------------------------
// 夹具（projects_factory_tests 同款：唯一临时目录 + 离线安全模型条目）
// ---------------------------------------------------------------------------

fn unique_home(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nb_seclvl_factory_{tag}_{}_{}_{}",
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

fn write_config(home: &Path, level: Option<&str>) {
    let mut sec = serde_json::json!({});
    if let Some(l) = level {
        sec["level"] = serde_json::json!(l);
    }
    let cfg = serde_json::json!({
        "security": sec,
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [ {
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
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

fn shared_for(home: &Path) -> Arc<SharedResources> {
    Arc::new(SharedResources {
        home: home.to_path_buf(),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// 效果面：档位 → loop 起步模式
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_level_starts_loop_in_plan_mode() {
    let home = unique_home("strict");
    write_config(&home, Some("strict"));
    let loop_ = build_agent_loop(&shared_for(&home)).expect("loop builds");
    assert_eq!(
        loop_.mode(),
        AgentMode::Plan,
        "strict 档必须以 plan 模式起步（写类工具闸先收紧）"
    );
}

#[tokio::test]
async fn basic_and_standard_levels_start_loop_in_build_mode() {
    // basic = 出厂缺省（键缺席同语义）。
    let home = unique_home("absent");
    write_config(&home, None);
    let loop_ = build_agent_loop(&shared_for(&home)).expect("loop builds");
    assert_eq!(
        loop_.mode(),
        AgentMode::Build,
        "security.level 缺席（= basic）必须保持 build 起步（升级零行为突变）"
    );

    let home2 = unique_home("basic");
    write_config(&home2, Some("basic"));
    let loop2 = build_agent_loop(&shared_for(&home2)).expect("loop builds");
    assert_eq!(loop2.mode(), AgentMode::Build);

    let home3 = unique_home("standard");
    write_config(&home3, Some("standard"));
    let loop3 = build_agent_loop(&shared_for(&home3)).expect("loop builds");
    assert_eq!(
        loop3.mode(),
        AgentMode::Build,
        "standard 档不收紧对话模式（plan 是 strict 专属）"
    );
}

#[tokio::test]
async fn invalid_level_falls_back_to_build_mode() {
    // 非法档位 typed 读回落 Basic → build 起步；装配 WARN 在 gateway 侧
    // （resolve_security_level_checked），这里锁 loop 侧行为不炸不猜。
    let home = unique_home("invalid");
    write_config(&home, Some("paranoid"));
    let loop_ = build_agent_loop(&shared_for(&home)).expect("loop builds");
    assert_eq!(loop_.mode(), AgentMode::Build);
}
