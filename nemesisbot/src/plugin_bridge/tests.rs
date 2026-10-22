//! plugin_bridge 单元测试（W5-3 附属；独立测试文件——生产文件零内联测试）。

use super::*;
use nemesis_agent::r#loop::Tool as _;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// 宿主壳 DLL 就绪保障（与 nemesis-plugins-wasm tests/common 同纪律）：
/// 产物缺席按需构建，路径经 `NEMESIS_WASM_HOST_DLL` 注入 loader——
/// 测试二进制在 target/debug/deps/，exe-相邻候选摸不到壳产物。
fn ensure_host_dll() {
    static HOST_DLL: OnceLock<Option<PathBuf>> = OnceLock::new();
    let path = HOST_DLL.get_or_init(|| {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin-wasm");
        let out = crate_dir
            .join("target/release")
            .join(if cfg!(target_os = "windows") {
                "plugin_wasm.dll"
            } else if cfg!(target_os = "macos") {
                "libplugin_wasm.dylib"
            } else {
                "libplugin_wasm.so"
            });
        if !out.exists() {
            let cargo = std::env::var_os("CARGO")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cargo"));
            let mut cmd = std::process::Command::new(cargo);
            cmd.args(["build", "--release"]).current_dir(&crate_dir);
            for k in [
                "CARGO_TARGET_DIR",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
            ] {
                cmd.env_remove(k);
            }
            match cmd.status() {
                Ok(s) if s.success() => {}
                s => {
                    eprintln!("宿主壳 DLL 构建失败：{s:?}");
                    return None;
                }
            }
        }
        out.exists().then_some(out)
    });
    let Some(p) = path else {
        panic!("宿主壳 DLL 构建失败（环境错误）：需 cargo 可用 + 壳 crate 可编译");
    };
    // SAFETY: 单写点（OnceLock 串行化）；读者 = 同进程 loader，均在本函数
    // 返回后触达。
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", p) };
}

fn temp_home() -> (
    std::sync::MutexGuard<'static, ()>,
    tempfile::TempDir,
    std::path::PathBuf,
) {
    // env 注入必须活到用例体内的 loader 读取完成——锁句柄随元组交还调用方
    // 持有至用例结束（WASM_HOST_DLL_LOCK 与 native_plugins_tests 共享，
    // 2026-10-22 竞态实录：两模块并行互踩 NEMESIS_WASM_HOST_DLL）。
    let lock = crate::WASM_HOST_DLL_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    ensure_host_dll();
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    // PluginManager::new 对 workspace root canonicalize——目录必须先在场。
    std::fs::create_dir_all(common::workspace_path(&home)).expect("workspace dir");
    (lock, dir, home)
}

fn meta(name: &str, operation_type: &str, parameters_json: &str) -> ToolMetaSnapshot {
    ToolMetaSnapshot {
        name: name.to_string(),
        base: "base".to_string(),
        title: "t".to_string(),
        description: "d".to_string(),
        parameters_json: parameters_json.to_string(),
        operation_type: operation_type.to_string(),
        min_tier: String::new(),
    }
}

#[test]
fn runtime_config_defaults_when_file_missing() {
    let (_lock, _dir, home) = temp_home();
    let rc = read_runtime_config(&home);
    assert!(rc.enabled, "缺省 enabled=true");
    let d = PluginLimits::default();
    assert_eq!(rc.limits.fuel, d.fuel);
    assert_eq!(rc.limits.memory_bytes, d.memory_bytes);
    assert_eq!(rc.limits.timeout_ms, d.timeout_ms);
}

#[test]
fn runtime_config_tightening_applied_and_loosening_ignored() {
    let (_lock, _dir, home) = temp_home();
    let d = PluginLimits::default();
    let cfg = serde_json::json!({
        "plugins": { "wasm": {
            "enabled": false,
            "limits": {
                "fuel": d.fuel / 2,
                "max_memory_bytes": d.memory_bytes / 2,
                "call_timeout_secs": 5,
                "max_host_calls_per_frame": 10,
                // 放宽请求（≥ 默认）——必须被忽略
                "max_instances": d.max_instances as u64 + 10,
                "observer_queue_depth": d.observer_queue_depth as u64 + 10,
            }
        }}
    });
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(common::config_path(&home), cfg.to_string()).unwrap();
    let rc = read_runtime_config(&home);
    assert!(!rc.enabled, "显式关闭生效");
    assert_eq!(rc.limits.fuel, d.fuel / 2, "收紧生效");
    assert_eq!(rc.limits.memory_bytes, d.memory_bytes / 2);
    assert_eq!(rc.limits.timeout_ms, 5000, "秒→毫秒换算");
    assert_eq!(rc.limits.host_call_budget, 10);
    assert_eq!(rc.limits.max_instances, d.max_instances, "放宽被忽略");
    assert_eq!(rc.limits.observer_queue_depth, d.observer_queue_depth);
}

#[test]
fn operation_mapping_covers_wordlist() {
    use nemesis_security::types::OperationType;
    assert_eq!(
        PluginToolBridge::map_operation("read"),
        Some(OperationType::FileRead)
    );
    assert_eq!(
        PluginToolBridge::map_operation("write"),
        Some(OperationType::FileWrite)
    );
    assert_eq!(
        PluginToolBridge::map_operation("exec"),
        Some(OperationType::ProcessExec)
    );
    assert_eq!(
        PluginToolBridge::map_operation("network"),
        Some(OperationType::NetworkRequest)
    );
    // 未声明（空串）/未知 = None：不声明 → dispatch 未注册名 fail-closed
    // CRITICAL（WIT 合同口径，2026-09-30 #3 从紧）。大小写敏感不猜。
    assert_eq!(PluginToolBridge::map_operation(""), None);
    assert_eq!(PluginToolBridge::map_operation("filesystem"), None);
    assert_eq!(PluginToolBridge::map_operation("Read"), None);
}

#[test]
fn bridge_parameters_fallback_on_invalid_json() {
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let bridge = PluginToolBridge::new(meta("plugin.x.base", "read", "not-json"), manager);
    let params = bridge.parameters();
    assert!(params.is_object(), "坏 JSON 回退空 object schema");
    assert!(params.get("properties").is_some());
}

#[test]
fn bridge_is_read_only_follows_declaration() {
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let ro = PluginToolBridge::new(meta("plugin.x.a", "read", "{}"), manager.clone());
    let rw = PluginToolBridge::new(meta("plugin.x.b", "write", "{}"), manager.clone());
    let undeclared = PluginToolBridge::new(meta("plugin.x.c", "", "{}"), manager);
    assert!(ro.is_read_only());
    assert!(!rw.is_read_only());
    assert!(!undeclared.is_read_only(), "未声明不作只读（从紧）");
}

#[test]
fn bridge_min_tier_follows_manifest_snapshot() {
    // min-tier 接线（2026-09-30 #2）：桥按安装期对账快照声明最低档，
    // 数据随 meta 走（无边车表无漂移）。
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let mut m = meta("plugin.x.a", "read", "{}");
    m.min_tier = "normal".to_string();
    let bridge = PluginToolBridge::new(m, manager);
    assert_eq!(bridge.min_tier(), Some("normal"));
}

#[tokio::test]
async fn install_approver_unbound_is_fail_closed() {
    let gate = LateInstallApprover::new(1);
    let review = InstallReview {
        slug: "demo".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
        kind: "tool".into(),
        trust_state: "review-required".into(),
        trust_level: String::new(),
        signer_key: String::new(),
        egress: vec![],
        x_secret: vec![],
        wasm_bytes: 1,
        wasm_sha256: "ab".repeat(32),
        limits_ignored: vec![],
        source_dir: "C:\\tmp".into(),
    };
    let verdict = gate.approve(&review).await;
    assert!(verdict.is_err(), "未 bind = 诚实拒绝而非放行");
}

#[test]
fn rescan_output_redacts_credentials_on_success_text() {
    // 2026-09-30 复查 #1/#3：出站复扫单点行为钉死——命中即脱敏，
    // 审计只进 summary（此处只验文本面）。
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let bridge = PluginToolBridge::new(meta("plugin.x.a", "read", "{}"), manager);
    let secret_text = "key is AKIAIOSFODNN7EXAMPLE end";
    let out = bridge.rescan_output_credentials(secret_text, "session-1");
    assert!(
        out.contains("[REDACTED_CREDENTIAL]"),
        "命中凭据必须脱敏: {out}"
    );
    assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "原文不得残留: {out}");
}

#[test]
fn rescan_output_passthrough_without_matches() {
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let bridge = PluginToolBridge::new(meta("plugin.x.a", "read", "{}"), manager);
    let plain = "普通输出，无凭据。";
    assert_eq!(
        bridge.rescan_output_credentials(plain, "session-1"),
        plain,
        "无命中原样返回（零扰动）"
    );
}

#[test]
fn vault_secrets_missing_vault_resolves_none() {
    let (_lock, _dir, home) = temp_home();
    let secrets = VaultPluginSecrets::for_home(&home);
    assert!(secrets.resolve("vault:plugin/demo/key").is_none());
}

#[tokio::test]
async fn estop_watcher_initial_alignment() {
    // spawn_estop_watcher 首行对齐初始态：已触发的 estop → 子系统冻结。
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let estop = Arc::new(nemesis_agent::EstopState::new());
    estop.trigger();
    spawn_estop_watcher(&manager, &estop);
    assert!(!manager.is_enabled(), "启动时已急停 = 子系统初始冻结");
}

// ---------------------------------------------------------------------------
// 策略桥投影（批 4）：截断纯函数回归面（字符边界——M2 家族）。
// ---------------------------------------------------------------------------

#[test]
fn truncate_chars_short_passthrough() {
    assert_eq!(super::truncate_chars("hello", 10), "hello");
    assert_eq!(super::truncate_chars("", 4), "");
}

#[test]
fn truncate_chars_ascii_cut() {
    let out = super::truncate_chars("abcdefgh", 4);
    assert_eq!(out, "abcd…");
}

#[test]
fn truncate_chars_multibyte_never_panics_and_respects_boundary() {
    // 中文 3 字节/字符：max 恰落在序列中间时切点必须回退到字符边界。
    let s = "一二三四五六七";
    let out = super::truncate_chars(s, 3);
    assert_eq!(out, "一二三…");
    // emoji（4 字节）同理。
    let e = "a\u{1F600}b\u{1F600}c";
    let out = super::truncate_chars(e, 2);
    assert!(out.starts_with("a\u{1F600}"));
    assert!(out.ends_with('…'));
    assert!(
        std::str::from_utf8(out.as_bytes()).is_ok(),
        "输出必须恒为合法 UTF-8"
    );
}

#[test]
fn truncate_chars_exact_count_no_ellipsis() {
    // 恰好等于上限 = 不截断不加省略号（幂等）。
    let s = "abcd";
    assert_eq!(super::truncate_chars(s, 4), "abcd");
    assert_eq!(
        super::truncate_chars(&super::truncate_chars(s, 4), 4),
        "abcd"
    );
}

// ---------------------------------------------------------------------------
// 策略桥全链（批 5）：真组件（policy-sample）经漏斗安装 → PolicyBridge
// trait 方法裁决投影 → register_policy_bridges 装配计数随注册表浮动。
// fixture 缺席（wasm target 未装等）诚实 SKIP——同 nemesis-plugins-wasm
// tests/policy_sample 纪律。
// ---------------------------------------------------------------------------

/// policy-sample wasm 产物路径（缺席按需构建；失败 = None → SKIP）。
fn policy_fixture_wasm() -> Option<PathBuf> {
    static OUT: OnceLock<Option<PathBuf>> = OnceLock::new();
    let path = OUT.get_or_init(|| {
        let crate_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins/wasm/policy-sample");
        let out = crate_dir.join("target/wasm32-wasip2/release/policy_sample_plugin.wasm");
        if out.exists() {
            return Some(out);
        }
        let cargo = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cargo"));
        let ok = std::process::Command::new(cargo)
            .args(["build", "--target", "wasm32-wasip2", "--release"])
            .current_dir(&crate_dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            out.exists().then_some(out)
        } else {
            eprintln!("SKIP：policy-sample fixture 构建失败（rustup target add wasm32-wasip2）");
            None
        }
    });
    path.clone()
}

const POLICY_SAMPLE_TOML: &str = r#"api-version = 1
slug = "policy-sample"
kind = "policy"
name = "PolicySample"
version = "0.1.0"
description = "bridge e2e fixture"
"#;

/// 落源目录（plugin.toml + wasm），返回路径供漏斗安装。
fn stage_policy_plugin(root: &std::path::Path, wasm: &std::path::Path) -> PathBuf {
    use sha2::Digest as _;
    let bytes = std::fs::read(wasm).expect("read fixture wasm");
    let sha = format!("{:x}", sha2::Sha256::digest(&bytes));
    let dir = root.join("policy-sample-src");
    std::fs::create_dir_all(&dir).expect("stage dir");
    std::fs::write(
        dir.join("plugin.toml"),
        format!("wasm-sha256 = \"{sha}\"\n{POLICY_SAMPLE_TOML}"),
    )
    .expect("write toml");
    std::fs::write(dir.join("plugin.wasm"), &bytes).expect("write wasm");
    dir
}

/// 已装 policy-sample 的 manager（temp home 纪律同现有用例）。锁窗口只护
/// env 注入 + `PluginManager::new` 的壳 DLL 解析（load-once 进程级缓存，
/// 此后 env 翻转无影响）——不跨 await（clippy await_holding_lock）。
async fn manager_with_policy_sample() -> Option<(tempfile::TempDir, Arc<PluginManager>)> {
    let Some(wasm) = policy_fixture_wasm() else {
        eprintln!("SKIP：policy-sample fixture 缺席");
        return None;
    };
    let (lock, dir, home) = temp_home();
    // 目录注入形态：生产 `new` 解析 exe 旁真实 plugins_wasm/（并行用例
    // 共享真实目录 = 删载荷/摘注册串扰根因），载荷目录落 temp 内。
    let plugins_dir = dir.path().join("plugins_wasm");
    let manager = Arc::new(
        PluginManager::new_with_dirs(
            &common::workspace_path(&home),
            &plugins_dir,
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    drop(lock);
    let staged = stage_policy_plugin(dir.path(), &wasm);
    let installer = nemesis_plugins_wasm::install::PluginInstaller::new(
        manager.clone(),
        Arc::new(nemesis_plugins_wasm::install::AutoApprove),
        None,
    );
    installer.install(&staged, true).await.expect("install");
    // 新纪律（2026-10-22）：安装 ≠ 加载——测试语义 =「用户已开开关」，
    // 开关开后策略点才可路由（PolicyBridge 断言全按已装载态写）。
    installer
        .set_enabled("policy-sample", true)
        .await
        .expect("enable")
        .expect("loaded");
    Some((dir, manager))
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_bridge_routes_guest_verdicts() {
    let Some((_dir, manager)) = manager_with_policy_sample().await else {
        return;
    };
    use nemesis_agent::r#loop::policy::{
        CompactExecInput, CompactPolicy as _, CompactTriggerInput, FinalJudgeInput,
        FinalJudgePolicy as _, ValidationInput, ValidationPolicy as _,
    };
    let bridge = Arc::new(super::PolicyBridge::new(manager.clone()));

    // validation：reject / fixed / builtin 三分支（真组件经壳往返）。
    let v = bridge
        .validate(&ValidationInput {
            tool: "exec".into(),
            schema: None,
            args_json: r#"{"cmd":"echo __REJECT__"}"#.into(),
        })
        .await;
    assert!(
        matches!(
            v,
            nemesis_agent::r#loop::policy::ValidationVerdict::Reject { .. }
        ),
        "got {v:?}"
    );
    let v = bridge
        .validate(&ValidationInput {
            tool: "write_file".into(),
            schema: None,
            args_json: r#"{"path":"notes/__autofix_this__.txt"}"#.into(),
        })
        .await;
    match v {
        nemesis_agent::r#loop::policy::ValidationVerdict::Fixed(a) => {
            assert_eq!(a, r#"{"path":"notes/autofixed-by-policy-sample.txt"}"#);
        }
        other => panic!("expected Fixed, got {other:?}"),
    }
    let v = bridge
        .validate(&ValidationInput {
            tool: "read_file".into(),
            schema: None,
            args_json: "{}".into(),
        })
        .await;
    assert!(
        matches!(v, nemesis_agent::r#loop::policy::ValidationVerdict::Builtin),
        "got {v:?}"
    );

    // final judge：giveup / retry（无 LLM invoker → 静态 nudge）/ builtin。
    let v = bridge
        .judge(&FinalJudgeInput {
            session_key: "s".into(),
            content: "x __FORCE_GIVEUP__".into(),
            turns_used: 2,
            validation_failures: 0,
        })
        .await;
    assert!(
        matches!(
            v,
            nemesis_agent::r#loop::policy::FinalJudgeVerdict::GiveUp(_)
        ),
        "got {v:?}"
    );
    let v = bridge
        .judge(&FinalJudgeInput {
            session_key: "s".into(),
            content: "x __FORCE_RETRY__".into(),
            turns_used: 2,
            validation_failures: 0,
        })
        .await;
    match v {
        nemesis_agent::r#loop::policy::FinalJudgeVerdict::RetryWithNudge(n) => {
            assert!(n.contains("__FORCE_RETRY__"), "nudge: {n}");
        }
        other => panic!("expected RetryWithNudge, got {other:?}"),
    }

    // compact trigger：过软位 skip / 硬位 builtin（soft = 软位 token 位置
    // usize，镜像 policy.rs 真实字段形态）；exec：确定性摘要。
    let v = bridge
        .should_compact(&CompactTriggerInput {
            session_key: "s".into(),
            tail_tokens: 1000,
            provider_signal: 0,
            signal: 1000,
            threshold: 2000,
            soft: 1333,
            window: 8000,
            covers_up_to: 0,
            new_boundary: 500,
        })
        .await;
    assert!(
        matches!(
            v,
            nemesis_agent::r#loop::policy::CompactTriggerVerdict::Skip
        ),
        "got {v:?}"
    );
    let turns = [
        nemesis_agent::types::ConversationTurn {
            role: "user".into(),
            content: "hi".into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
        nemesis_agent::types::ConversationTurn {
            role: "assistant".into(),
            content: "yo".into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
    ];
    let v = bridge
        .generate_summary(&CompactExecInput {
            session_key: "s",
            prefix: &turns,
            existing_summary: "",
            window: 100,
            model: "m1",
        })
        .await;
    match v {
        nemesis_agent::r#loop::policy::CompactExecVerdict::Summary(s) => {
            assert_eq!(s, "[policy-sample] 已折叠 2 条历史（window=100，model=m1）");
        }
        other => panic!("expected Summary, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn register_policy_bridges_count_follows_registry() {
    let Some((_dir, manager)) = manager_with_policy_sample().await else {
        return;
    };
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopLlmProvider),
        nemesis_agent::types::AgentConfig::default(),
    ));
    let estop = Arc::new(nemesis_agent::EstopState::new());

    // 三接管点 → 装配计数 3（llm 面注入 + 三槽装桥）。
    let n = super::register_policy_bridges(&agent_loop, &manager, &estop);
    assert_eq!(n, 3, "三点全接管 = 计数 3");

    // 禁用 → 再装配 = 0（摘桥回内置哨兵的决策面；开关走 installer 单一
    // 真相源——registry 的 set_enabled_plugin 已退役）。
    let installer = nemesis_plugins_wasm::install::PluginInstaller::new(
        manager.clone(),
        Arc::new(nemesis_plugins_wasm::install::AutoApprove),
        None,
    );
    installer
        .set_enabled("policy-sample", false)
        .await
        .expect("disable");
    let n = super::register_policy_bridges(&agent_loop, &manager, &estop);
    assert_eq!(n, 0, "禁用后无接管点");
}

// —— 空 provider：register_policy_bridges 只需要 loop 形（不走 LLM）——

struct NoopLlmProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NoopLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 工具接管层（2026-10-23 批 B）：wrapper 双态路由 + 凭据槽纪律 + 漂移钉 +
// 对账/装配。侧表进程级共享——状态类用例串行锁 + `tt_` 专名前缀防并行互踩。
// ---------------------------------------------------------------------------

static TAKEOVER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// deny-list ⊇ MOVE_TOOLS 漂移钉（计划 §风险）：执行体分离高危族 + 写类
/// 工具全被 manifest deny 覆盖（两清单分居两 crate，本断言是唯一缝线）。
#[test]
fn takeover_deny_list_covers_move_tools() {
    for t in nemesis_agent::MOVE_TOOLS {
        assert!(
            nemesis_plugins_wasm::manifest::takeover_denied(t),
            "MOVE_TOOLS 的 {t} 必须在 TAKEOVER_DENIED_TOOLS（漂移）"
        );
    }
    // 进程/注册表/系统面（8 层 CRITICAL/HIGH 家族）同被钉。
    for t in [
        "process_spawn",
        "process_kill",
        "process_exec",
        "registry_write",
        "system_shutdown",
    ] {
        assert!(nemesis_plugins_wasm::manifest::takeover_denied(t));
    }
}

/// 接管目标桩（内置执行体替身）：execute 恒返回 `builtin:<tag>`，可声明
/// 凭据槽位（验 wrapper 凭据槽双态纪律：接管态空 / 回退态透出）。
struct StubBuiltin {
    tag: &'static str,
    cred_keys: &'static [&'static str],
    read_only: bool,
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for StubBuiltin {
    async fn execute(
        &self,
        _args: &str,
        _ctx: &nemesis_agent::context::RequestContext,
    ) -> Result<String, String> {
        Ok(format!("builtin:{}", self.tag))
    }
    fn description(&self) -> String {
        format!("stub-{}", self.tag)
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn credential_arg_keys(&self) -> &[&str] {
        self.cred_keys
    }
}

fn stub_meta(name: &str) -> ToolMetaSnapshot {
    ToolMetaSnapshot {
        name: name.to_string(),
        base: "base".to_string(),
        title: "t".to_string(),
        description: "d".to_string(),
        parameters_json: "{}".to_string(),
        operation_type: "read".to_string(),
        min_tier: String::new(),
    }
}

fn takeover_plugin_manifest(
    slug: &str,
    targets: &[&str],
) -> nemesis_plugins_wasm::manifest::PluginManifest {
    let takeover = if targets.is_empty() {
        String::new()
    } else {
        format!(
            "takeover = [{}]\n",
            targets
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"{slug}\"\nkind = \"tool\"\n\
         name = \"T\"\nversion = \"0.1.0\"\ndescription = \"d\"\n{takeover}",
        "a".repeat(64)
    );
    nemesis_plugins_wasm::manifest::PluginManifest::parse(&raw)
        .expect("parse manifest")
        .0
}

#[tokio::test]
// temp_home 的 std 锁是测试 home 隔离串行（守卫必须活满用例）；被 await 的
// execute 只读 AtomicBool 不取该锁，无死锁面——测试内刻意持有，非生产形态。
#[allow(clippy::await_holding_lock)]
async fn takeover_wrapper_routes_by_switch_and_hides_credential_slots() {
    let (_lock, _dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let inner = Arc::new(StubBuiltin {
        tag: "v1",
        cred_keys: &["headers"],
        read_only: true,
    });
    let bridge = PluginToolBridge::new(stub_meta("plugin.p.base"), manager);
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let wrapper = super::TakeoverToolBridge {
        inner: inner.clone(),
        bridge,
        active: active.clone(),
    };
    let ctx = nemesis_agent::context::RequestContext::new("test", "c1", "tester", "");

    // 开 → wasm 执行体路径（manager 无此插件 → 诚实 Err 而非内置回声）。
    let out = nemesis_agent::r#loop::Tool::execute(&wrapper, "{}", &ctx).await;
    assert!(
        out.is_err(),
        "active=true 必须走 wasm 桥（无插件 = 诚实报错）"
    );

    // guest 回环闸（评审 #1 回归锁）：active + 宿主调用面标记 context
    // （channel="plugin"，GatewayHostInvoker 全仓唯一构造形态）→ 拒绝，
    // 绝不回内置（回内置 = guest 借宿主网络出沙盒）。
    let guest_ctx = nemesis_agent::context::RequestContext::new("plugin", "", "wasm-plugin", "");
    let out = nemesis_agent::r#loop::Tool::execute(&wrapper, "{}", &guest_ctx).await;
    let err = out.expect_err("active + guest 标记 = 回环闸拒绝");
    assert!(err.contains("recursion gate"), "诚实拒绝带原因: {err}");

    // 接管态凭据槽恒空（评审 #2 回归锁半边）：inner 声明 headers，接管态
    // 不透出——vault 注入面不进沙盒。
    assert!(wrapper.credential_arg_keys().is_empty());

    // 关 → 内置执行体（零窗口回退）。
    active.store(false, std::sync::atomic::Ordering::SeqCst);
    let out = nemesis_agent::r#loop::Tool::execute(&wrapper, "{}", &ctx).await;
    assert_eq!(out.expect("builtin path"), "builtin:v1");

    // 开关关 + guest 标记 → 内置执行体（guest→builtin 既有语义保留：
    // 内置不再发起工具调用，链天然终止——回环只在 active 边存在）。
    let out = nemesis_agent::r#loop::Tool::execute(&wrapper, "{}", &guest_ctx).await;
    assert_eq!(out.expect("guest→builtin"), "builtin:v1");

    // 元数据全 delegate 内置（prompt cache 字节不变的断言面）。
    assert_eq!(
        nemesis_agent::r#loop::Tool::description(&wrapper),
        "stub-v1"
    );
    assert!(nemesis_agent::r#loop::Tool::is_read_only(&wrapper));
    // 回退态凭据槽逐字节恢复内置（评审 #2 回归锁另半边）：恒空会连回退态
    // 的内置注入面一起杀掉——槽位随开关双态路由。
    assert_eq!(wrapper.credential_arg_keys(), &["headers"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_takeover_wraps_flips_and_releases() {
    let _serial = TAKEOVER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (_lock, dir, home) = temp_home();
    let plugins_dir = dir.path().join("plugins_wasm");
    let manager = Arc::new(
        PluginManager::new_with_dirs(
            &common::workspace_path(&home),
            &plugins_dir,
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let mut agent_loop = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopLlmProvider),
        nemesis_agent::types::AgentConfig::default(),
    );
    agent_loop.register_tool(
        "tt_recon_probe".to_string(),
        Box::new(StubBuiltin {
            tag: "recon",
            cred_keys: &[],
            read_only: true,
        }),
    );

    // 无接管者 + 未在管 → false 无动作（不 panic 不误激活）。
    assert!(!super::reconcile_takeover(
        &agent_loop,
        &manager,
        "tt_recon_probe"
    ));
    assert!(super::takeover_switch_state("tt_recon_probe").is_none());

    // 注册接管者（直注册 = 在场启用）→ reconcile wrap + 侧表登记。
    manager
        .register(
            takeover_plugin_manifest("reconer", &["tt_recon_probe"]),
            nemesis_plugins_wasm::PluginTrustState::ReviewRequired,
            1,
            Some(stub_meta("plugin.reconer.base")),
            None,
        )
        .expect("register");
    assert!(
        super::reconcile_takeover(&agent_loop, &manager, "tt_recon_probe"),
        "接管者启用 + 目标在场 = wrap 成功"
    );
    assert_eq!(
        super::takeover_switch_state("tt_recon_probe"),
        Some(true),
        "侧表登记 + 激活"
    );

    // 禁用 → reconcile 解除（wrapper 留守委托内置，侧表仍在）。
    let reg = manager
        .list()
        .into_iter()
        .find(|r| r.manifest.slug == "reconer")
        .expect("registered");
    reg.enabled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(!super::reconcile_takeover(
        &agent_loop,
        &manager,
        "tt_recon_probe"
    ));
    assert_eq!(
        super::takeover_switch_state("tt_recon_probe"),
        Some(false),
        "解除后侧表留守（开关位 false）"
    );

    // 复启用 → 翻位即达（零 swap 路径：已 wrap 直接翻 active）。
    reg.enabled.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(super::reconcile_takeover(
        &agent_loop,
        &manager,
        "tt_recon_probe"
    ));
    assert_eq!(super::takeover_switch_state("tt_recon_probe"), Some(true));

    // 目标缺席（未注册内置）→ 不激活 + 无侧表残留。
    assert!(!super::reconcile_takeover(
        &agent_loop,
        &manager,
        "tt_absent_probe"
    ));
    assert!(super::takeover_switch_state("tt_absent_probe").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_takeovers_gate_and_rebuild_semantics() {
    let _serial = TAKEOVER_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (_lock, dir, home) = temp_home();
    let plugins_dir = dir.path().join("plugins_wasm");
    let manager = Arc::new(
        PluginManager::new_with_dirs(
            &common::workspace_path(&home),
            &plugins_dir,
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let build_loop = || {
        let mut l = nemesis_agent::r#loop::AgentLoop::new(
            Box::new(NoopLlmProvider),
            nemesis_agent::types::AgentConfig::default(),
        );
        l.register_tool(
            "tt_apply_probe".to_string(),
            Box::new(StubBuiltin {
                tag: "apply",
                cred_keys: &[],
                read_only: true,
            }),
        );
        l
    };

    // 总闸关 → 0 激活，无侧表。
    let l = build_loop();
    assert_eq!(super::apply_takeovers(&l, &manager, false), 0);
    assert!(super::takeover_switch_state("tt_apply_probe").is_none());

    // 注册接管者；闸开 → 1 激活。
    manager
        .register(
            takeover_plugin_manifest("applyer", &["tt_apply_probe"]),
            nemesis_plugins_wasm::PluginTrustState::ReviewRequired,
            1,
            Some(stub_meta("plugin.applyer.base")),
            None,
        )
        .expect("register");
    let l = build_loop();
    assert_eq!(super::apply_takeovers(&l, &manager, true), 1);
    assert_eq!(super::takeover_switch_state("tt_apply_probe"), Some(true));

    // loop 重建语义：侧表整表重建（旧 loop 开关 Arc 不泄漏到新表）——
    // fresh loop 无接管者可见变化时 rebuild 后仍按注册表重新激活。
    let l2 = build_loop();
    assert_eq!(super::apply_takeovers(&l2, &manager, true), 1);
    assert_eq!(super::takeover_switch_state("tt_apply_probe"), Some(true));
}
