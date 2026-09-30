//! W7 SDK dogfood e2e：三方 SDK（[`nemesis_plugin_sdk::export_tool!`]）产物
//! 走宿主正式 admit 漏斗实测执行。
//!
//! fixture = `plugins/wasm/textstat`（三方起点示例：serde_json 正经
//! 解析 + config-schema 消费 + /data 持久化）。产物缺席时测试内按需构建
//! （crate 自有 workspace → 独立 target-dir）；构建失败 = 环境不满足，诚实
//! SKIP。与 translate（原始 wit-bindgen 写法）互补：本文件验证的是 **SDK
//! 宏路径**——generate! 由 SDK 转发、runtime_path 钉 SDK、无需直接依赖
//! wit-bindgen。
//!
//! ```text
//! cargo test -p nemesis-plugins-wasm --test sdk_dogfood
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_plugins_wasm::PluginTrustState;
use nemesis_plugins_wasm::host_impl::SecretResolver;
use nemesis_plugins_wasm::install::{AutoApprove, PluginInstaller};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::PluginManager;
use nemesis_plugins_wasm::runtime::ExecContext;

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// fixture 按需构建（产物缓存 OnceLock；并发用例共享同一次构建）
// ---------------------------------------------------------------------------

fn cargo_bin() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

fn fixture_wasm() -> Option<PathBuf> {
    static OUT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    OUT.get_or_init(|| {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/textstat");
        let out = crate_dir.join("target/wasm32-wasip2/release/textstat_plugin.wasm");
        if !out.exists() {
            let status = std::process::Command::new(cargo_bin())
                .args(["build", "--target", "wasm32-wasip2", "--release"])
                .current_dir(&crate_dir)
                .status();
            match status {
                Ok(s) if s.success() => {}
                status => {
                    eprintln!(
                        "SKIP sdk_dogfood：textstat 构建失败（{status:?}）。\
环境要求：rustup target add wasm32-wasip2 + 网络可取依赖"
                    );
                    return None;
                }
            }
        }
        out.exists().then_some(out)
    })
    .clone()
}

fn skip_reason() -> String {
    "SKIP sdk_dogfood：textstat fixture 缺席——见构建失败时的环境说明".to_string()
}

/// 写插件源目录（plugin.toml + wasm），返回目录路径。
fn stage_plugin(
    root: &Path,
    toml_body: String,
    wasm_src: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let wasm_bytes = std::fs::read(wasm_src)?;
    let sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&wasm_bytes))
    };
    let dir = root.join("src");
    std::fs::create_dir_all(&dir)?;
    // sha 放文件头（TOML 根键必须在任何 table 段之前）。
    let manifest = format!("wasm-sha256 = \"{sha}\"\n{toml_body}\n");
    std::fs::write(dir.join("plugin.toml"), manifest)?;
    std::fs::write(dir.join("plugin.wasm"), &wasm_bytes)?;
    Ok(dir)
}

const TEXTSTAT_TOML: &str = r#"api-version = 1
slug = "textstat"
kind = "tool"
name = "TextStat"
version = "0.1.0"
description = "SDK dogfood fixture tool"

[limits]
fuel = 20_000_000
timeout-ms = 10_000

[config-schema]
include_spaces = "字符计数是否含空格（true/false，缺省含）"
"#;

fn ctx() -> ExecContext {
    ExecContext {
        session_key: "sess-dogfood".into(),
        call_id: "call-dogfood".into(),
    }
}

async fn installed_manager()
-> Result<(tempfile::TempDir, Arc<PluginManager>), Box<dyn std::error::Error>> {
    let Some(wasm) = fixture_wasm() else {
        return Err(skip_reason().into());
    };
    let ws = tempfile::tempdir()?;
    let mgr = Arc::new(PluginManager::new(
        ws.path(),
        PluginLimits::default(),
        Arc::new(NoSecrets),
    )?);
    let tmp = tempfile::tempdir()?;
    let src = stage_plugin(tmp.path(), TEXTSTAT_TOML.to_string(), &wasm)?;
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let reg = installer.install(&src, true).await?;
    assert_eq!(reg.trust, PluginTrustState::ReviewRequired);
    Ok((ws, mgr))
}

/// 样本文本：「hello world\nsecond line」= 23 字符（3 空白）/ 4 词 / 2 行。
const SAMPLE: &str = "hello world\nsecond line";

#[tokio::test(flavor = "multi_thread")]
async fn sdk_tool_execute_and_data_persistence() {
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };

    // 注册面对账（SDK 产物与原始 bindgen 产物同形态）。
    let found = mgr
        .find_by_tool_name("plugin.textstat.textstat")
        .expect("tool registered");
    let meta = found.tool_meta.as_ref().expect("tool meta");
    assert_eq!(meta.name, "plugin.textstat.textstat");
    assert_eq!(meta.operation_type, "read");
    assert_eq!(meta.min_tier, "big");

    // 基线执行：统计 + config 缺省（含空格）+ 首次调用计数。
    let out = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            format!(r#"{{"text":{SAMPLE:?}}}"#),
            ctx(),
        )
        .await
        .expect("execute");
    assert!(!out.is_error, "content: {}", out.content);
    assert!(
        out.content.contains(r#""chars":23"#),
        "got: {}",
        out.content
    );
    assert!(out.content.contains(r#""words":4"#), "got: {}", out.content);
    assert!(out.content.contains(r#""lines":2"#), "got: {}", out.content);
    assert!(
        out.content.contains(r#""include_spaces":true"#),
        "got: {}",
        out.content
    );
    assert!(out.content.contains(r#""calls":1"#), "got: {}", out.content);

    // 二次调用：fresh store 每次全新，但 /data 落数据目录 → 计数跨调用持久。
    let out = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            format!(r#"{{"text":{SAMPLE:?}}}"#),
            ctx(),
        )
        .await
        .expect("execute again");
    assert!(out.content.contains(r#""calls":2"#), "got: {}", out.content);
    let counter = mgr.plugin_data_dir("textstat").join("call_count");
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap_or_default().trim(),
        "2",
        "call_count file at {}",
        counter.display()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sdk_config_include_spaces_hot_effect() {
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };

    // 实例配置热生效：include_spaces=false → 空白不计（23 - 3 = 20），
    // 词/行不受影响。
    mgr.set_config_key("textstat", "include_spaces", "false")
        .expect("set config");
    let out = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            format!(r#"{{"text":{SAMPLE:?}}}"#),
            ctx(),
        )
        .await
        .expect("execute");
    assert!(
        out.content.contains(r#""include_spaces":false"#),
        "got: {}",
        out.content
    );
    assert!(
        out.content.contains(r#""chars":20"#),
        "got: {}",
        out.content
    );
    assert!(out.content.contains(r#""words":4"#), "got: {}", out.content);

    // guest 结构化日志通道（execute 里的 host::log 一条 Info）。
    let reg = mgr.get("textstat").expect("registered");
    let (lines, _dropped) = reg.logs.snapshot();
    assert!(
        lines.iter().any(|l| l.message.contains("textstat:")),
        "log lines: {:?}",
        lines.iter().map(|l| &l.message).collect::<Vec<_>>()
    );
}
