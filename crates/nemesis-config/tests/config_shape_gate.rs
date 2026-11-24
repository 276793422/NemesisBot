//! BUG-20261120-030 回归锁：`load_config` 读侧形状闸。
//!
//! Config 全字段带 default，`[]` 文档曾从 serde visit_seq 零元素臂溜过 =
//! 「全默认 Config」合法值——整档用户配置（model_list/API key/channels）
//! 被静默无视、出厂默认启动，零诊断。object 闸必须在 typed 解析前拦下
//! （启动即失败优于带错静跑）。语义单一真相源 = nemesis_utils::json_shape。

use nemesis_config::{ConfigError, load_config};
use std::path::PathBuf;

fn write_file(dir: &tempfile::TempDir, content: &str) -> PathBuf {
    let path = dir.path().join("config.json");
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn load_config_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_config(&write_file(&dir, "[]")).unwrap_err();
    let ConfigError::Validation(msg) = err else {
        panic!("expected Validation error, got {err:?}");
    };
    assert!(msg.contains("must be a JSON object"), "msg={msg}");
    assert!(msg.contains("array"), "msg={msg}");
}

#[test]
fn load_config_rejects_scalar_documents_loud() {
    let dir = tempfile::tempdir().unwrap();
    for content in ["null", "\"config\"", "42", "true"] {
        let err = load_config(&write_file(&dir, content)).unwrap_err();
        let ConfigError::Validation(msg) = err else {
            panic!("expected Validation error for {content}, got {err:?}");
        };
        assert!(
            msg.contains("must be a JSON object"),
            "content={content} msg={msg}"
        );
    }
}

#[test]
fn load_config_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    // 最小 object 文档（全字段 default，缺键合法）。
    let cfg = load_config(&write_file(&dir, r#"{"agents": {}}"#)).expect("object doc parses");
    // 判别字段确从文档消费（不是 vacuous Ok）。
    assert!(cfg.agents.image_downscale, "default image_downscale=true");
}

// ---- BUG-20261122-003：独立配置 loader 同族横扫补位 ----
//
// load_config 主入口 030 已闸；config.security.json / config.scanner.json /
// ConfigLoader::load_from_file 同洞漏网（SecurityConfig 25 字段全 default、
// ScannerFullConfig 全 default、Config 全 default——`[]` 走 serde visit_seq
// 零元素臂 = 「全默认」合法值，用户配置静默无视零诊断）。mcp / skills 两
// 位点天然免疫（mcp 自定义 Deserialize 有 object 守卫；skills 带 flatten
// 字段，derive 只生成 visit_map）——免疫锁一并钉住防退化。

fn write_named(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn load_security_config_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err =
        nemesis_config::load_security_config(&write_named(&dir, "config.security.json", "[]"))
            .unwrap_err();
    let ConfigError::Validation(msg) = err else {
        panic!("expected Validation error, got {err:?}");
    };
    assert!(msg.contains("must be a JSON object"), "msg={msg}");
    assert!(msg.contains("array"), "msg={msg}");
}

#[test]
fn load_scanner_config_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err = nemesis_config::load_scanner_config(&write_named(&dir, "config.scanner.json", "[]"))
        .unwrap_err();
    let ConfigError::Validation(msg) = err else {
        panic!("expected Validation error, got {err:?}");
    };
    assert!(msg.contains("must be a JSON object"), "msg={msg}");
    assert!(msg.contains("array"), "msg={msg}");
}

#[test]
fn load_from_file_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err = nemesis_config::ConfigLoader::load_from_file(&write_named(&dir, "config.json", "[]"))
        .unwrap_err();
    let ConfigError::Validation(msg) = err else {
        panic!("expected Validation error, got {err:?}");
    };
    assert!(msg.contains("must be a JSON object"), "msg={msg}");
    assert!(msg.contains("array"), "msg={msg}");
}

#[test]
fn security_config_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    // 最小 object（字段全 default，缺键合法）——闸不误伤合法文档。
    let cfg = nemesis_config::load_security_config(&write_named(
        &dir,
        "config.security.json",
        r#"{"default_action": "allow"}"#,
    ))
    .expect("object doc parses");
    assert_eq!(cfg.default_action, "allow", "判别字段确从文档消费");
}

#[test]
fn scanner_config_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = nemesis_config::load_scanner_config(&write_named(
        &dir,
        "config.scanner.json",
        r#"{"engines": {"clamav": {"enabled": true}}}"#,
    ))
    .expect("object doc parses");
    assert!(cfg.engines.contains_key("clamav"), "判别字段确从文档消费");
}

/// 免疫锁：config.mcp.json 的 `[]` 由自定义 Deserialize 的 object 守卫
/// loud 拒绝（既有防线，钉住防退化）。
#[test]
fn load_mcp_config_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err =
        nemesis_config::load_mcp_config(&write_named(&dir, "config.mcp.json", "[]")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("must be a JSON object"), "msg={msg}");
}

/// 免疫锁：SkillsFullConfig 带 flatten 字段，serde derive 只生成 visit_map
/// ——`[]` 天然 loud 失败（若未来去掉 flatten 字段，本锁红 = 洞重开）。
#[test]
fn load_skills_config_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err = nemesis_config::load_skills_config(&write_named(&dir, "config.skills.json", "[]"))
        .unwrap_err();
    let msg = err.to_string();
    // serde_json 对数组文档喂 deserialize_map 的错误形态（非 object 闸文案）。
    assert!(!msg.is_empty(), "loud 失败即可，文案形态不钉死");
}

// ---- BUG-20261122-006：proxy 独立配置 loader 家族横扫补位 ----
//
// config.proxy.json / config.proxy_keys.json 两个独立 loader（proxy_file.rs /
// proxy_keys_file.rs）同洞漏网：ProxyFileConfig / ProxyKeysFile 全字段
// `#[serde(default)]` 无 flatten，`[]` 走 visit_seq 零元素臂 = 「全默认」
// 合法值。proxy 静默无视零诊断；keys 更重——鉴权 key 表静默清空、调用方
// 无 WARN 回落主 token（权限隔离静默失效）。修复 = ensure_object 前置，
// 失败进既有 Err 臂（调用方 WARN + 按未配置走，不阻断启动）。

#[test]
fn load_proxy_file_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    // load(config_dir) 收目录、内部 join 文件名（proxy_config_path）——写完
    // 文件传目录，不能传文件路径（会再 join 出不存在的嵌套路径 → Ok(None)）。
    let content_path = write_named(&dir, "config.proxy.json", "[]");
    assert!(content_path.exists());
    let err = nemesis_config::proxy_file::load(dir.path()).unwrap_err();
    assert!(err.contains("形状非法"), "got: {err}");
    assert!(err.contains("array") || err.contains("数组"), "got: {err}");
    // 标量文档同闸。
    for content in ["null", "42", "\"p\""] {
        std::fs::write(&content_path, content).unwrap();
        let err = nemesis_config::proxy_file::load(dir.path()).unwrap_err();
        assert!(err.contains("形状非法"), "content={content} got: {err}");
    }
}

#[test]
fn load_proxy_file_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    write_named(
        &dir,
        "config.proxy.json",
        r#"{"env": {"http_proxy": "http://127.0.0.1:8080"}}"#,
    );
    let cfg = nemesis_config::proxy_file::load(dir.path())
        .expect("object doc parses")
        .expect("file exists => Some");
    assert_eq!(
        cfg.env.http_proxy, "http://127.0.0.1:8080",
        "判别字段确从文档消费"
    );
}

#[test]
fn load_proxy_keys_file_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    write_named(&dir, "config.proxy_keys.json", "[]");
    let err = nemesis_config::proxy_keys_file::load(dir.path()).unwrap_err();
    assert!(err.contains("形状非法"), "got: {err}");
    assert!(err.contains("array") || err.contains("数组"), "got: {err}");
}

#[test]
fn load_proxy_keys_file_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    write_named(
        &dir,
        "config.proxy_keys.json",
        r#"{"keys": [{"name": "a", "key": "nbk_test"}]}"#,
    );
    let cfg = nemesis_config::proxy_keys_file::load(dir.path())
        .expect("object doc parses")
        .expect("file exists => Some");
    assert_eq!(cfg.keys.len(), 1, "判别字段确从文档消费");
    assert_eq!(cfg.keys[0].name, "a");
}

// ---- BUG-20261125-008：家族横扫补位（config 树用户配置 loader 第三批）----
//
// load_commands_config / plugin_wasm 开关文件 / plugins_native 开关文件
// （load 读侧 + set 写侧读底）四处 loader 同洞漏网：struct 全字段
// `#[serde(default)]` 无 flatten，`[]` 走 serde visit_seq 零元素臂 = 「全
// 默认」合法值零诊断——commands 命令表静默清空、wasm 开关全关（无 WARN）、
// native 开关全启（已停用插件静默复活）。修复 = ensure_object 前置，违形
// 进各站点既有 WARN 降级臂（签名不动：load_commands_config 是 HotLoadFn
// 契约——必须容忍缺失文件、错误不外泄）。断言 WARN 需捕获 subscriber
// （自制最小 Subscriber 收事件文本进共享缓冲，形态抄 nemesis-agent
// test_support::capture_logs_to_buf；不引 tracing-subscriber dev-dep）。

mod warn_capture {
    use std::sync::{Arc, Mutex};

    struct EventText(Arc<Mutex<Vec<String>>>);

    struct MsgVisitor(String);
    impl tracing::field::Visit for MsgVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(&format!("{value:?}"));
            } else {
                self.0.push_str(&format!(" {}={value:?}", field.name()));
            }
        }
    }

    impl tracing::Subscriber for EventText {
        fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut v = MsgVisitor(String::new());
            event.record(&mut v);
            self.0
                .lock()
                .unwrap()
                .push(format!("[{}] {}", event.metadata().level(), v.0));
        }
        fn enter(&self, _span: &tracing::Id) {}
        fn exit(&self, _span: &tracing::Id) {}
    }

    /// thread-local 装捕获 subscriber；返回 (guard, 缓冲)，guard 持有至断言完。
    pub fn capture() -> (tracing::subscriber::DefaultGuard, Arc<Mutex<Vec<String>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(EventText(buf.clone()));
        (guard, buf)
    }
}

#[test]
fn load_commands_config_array_document_warns_and_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_named(&dir, "config.commands.json", "[]");
    let (_guard, buf) = warn_capture::capture();
    let cfg = nemesis_config::load_commands_config(&path);
    assert!(cfg.commands.is_empty(), "`[]` 零条目 → 空表");
    let logs = buf.lock().unwrap().join("\n");
    assert!(
        logs.contains("must be a JSON object"),
        "违形必须 WARN：logs={logs}"
    );
    assert!(logs.contains("array"), "形状必须可见：logs={logs}");
    assert!(
        logs.contains("config.commands.json"),
        "路径必须可见：logs={logs}"
    );
}

#[test]
fn load_commands_config_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_named(
        &dir,
        "config.commands.json",
        r#"{"commands":[{"name":"greet","prompt":"hi $ARGUMENTS"}]}"#,
    );
    let cfg = nemesis_config::load_commands_config(&path);
    assert_eq!(cfg.commands.len(), 1, "判别字段确从文档消费");
    assert_eq!(cfg.commands[0].name, "greet");
}

#[test]
fn wasm_switch_file_array_document_warns_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    write_named(&dir, "config.plugin.wasm.json", "[]");
    let (_guard, buf) = warn_capture::capture();
    let switches = nemesis_config::plugin_wasm::load_switches(dir.path());
    assert!(switches.is_empty(), "`[]` 零条目 → 空 map（fail-closed）");
    let logs = buf.lock().unwrap().join("\n");
    assert!(
        logs.contains("must be a JSON object"),
        "违形必须 WARN（此前零诊断）：logs={logs}"
    );
    assert!(logs.contains("array"), "形状必须可见：logs={logs}");
}

#[test]
fn wasm_switch_file_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    write_named(
        &dir,
        "config.plugin.wasm.json",
        r#"{"plugins":{"demo":{"enabled":true,"config":{"k":"v"}}}}"#,
    );
    let switches = nemesis_config::plugin_wasm::load_switches(dir.path());
    assert_eq!(switches.len(), 1, "判别字段确从文档消费");
    assert!(switches["demo"].enabled);
    assert_eq!(
        switches["demo"].config.get("k").and_then(|v| v.as_str()),
        Some("v")
    );
}

#[test]
fn native_switch_file_array_document_warns_fail_open() {
    let dir = tempfile::tempdir().unwrap();
    write_named(&dir, "config.plugin.native.json", "[]");
    let (_guard, buf) = warn_capture::capture();
    let on = nemesis_config::plugins_native::load_enabled(
        dir.path(),
        nemesis_config::plugins_native::NativePluginId::PluginUi,
    );
    assert!(on, "`[]` 零条目 → 缺省启用（fail-open）");
    let logs = buf.lock().unwrap().join("\n");
    assert!(
        logs.contains("must be a JSON object"),
        "违形必须 WARN（此前零诊断）：logs={logs}"
    );
    assert!(logs.contains("array"), "形状必须可见：logs={logs}");
}

#[test]
fn native_switch_file_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    write_named(
        &dir,
        "config.plugin.native.json",
        r#"{"plugins":{"plugin_ui":{"enabled":false}}}"#,
    );
    let on = nemesis_config::plugins_native::load_enabled(
        dir.path(),
        nemesis_config::plugins_native::NativePluginId::PluginUi,
    );
    assert!(!on, "判别字段确从文档消费（disabled 生效）");
}
