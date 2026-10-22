//! 循环策略样板 e2e（2026-10-22 批 5）：三方 SDK（
//! [`nemesis_plugin_sdk::export_loop_policy!`]）产物走宿主正式 admit 漏斗
//! 实测三策略点接管 / builtin 回退 / 禁用摘桥语义。
//!
//! fixture = `plugins/wasm/policy-sample`（标记驱动的行为夹具：三策略点
//! 各带最小可观测差异 + builtin 回退分支）。产物缺席时测试内按需构建
//! （crate 自有 workspace → 独立 target-dir）；构建失败 = 环境不满足，诚实
//! SKIP。与 sdk_dogfood（tool world）互补：本文件验证的是**策略 world 全链**——
//! 安装期 get-policy-metadata 探针 + ≥1 接管点校验、运行期
//! [`PluginManager::call_policy_point`] 三点裁决投影、宿主 LLM invoker 缺席
//! 时 guest 侧 llm-complete 诚实回退静态文案。
//!
//! ```text
//! cargo test -p nemesis-plugins-wasm --test policy_sample
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_plugins_wasm::PluginTrustState;
use nemesis_plugins_wasm::host_impl::SecretResolver;
use nemesis_plugins_wasm::install::{AutoApprove, PluginInstaller};
use nemesis_plugins_wasm::limits::{PluginLimits, PolicyPoint};
use nemesis_plugins_wasm::registry::PluginManager;

mod common;

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
        let crate_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/policy-sample");
        let out = crate_dir.join("target/wasm32-wasip2/release/policy_sample_plugin.wasm");
        if !out.exists() {
            let status = std::process::Command::new(cargo_bin())
                .args(["build", "--target", "wasm32-wasip2", "--release"])
                .current_dir(&crate_dir)
                .status();
            match status {
                Ok(s) if s.success() => {}
                status => {
                    eprintln!(
                        "SKIP policy_sample：policy-sample 构建失败（{status:?}）。\
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
    "SKIP policy_sample：policy-sample fixture 缺席——见构建失败时的环境说明".to_string()
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

const POLICY_SAMPLE_TOML: &str = r#"api-version = 1
slug = "policy-sample"
kind = "policy"
name = "PolicySample"
version = "0.1.0"
description = "SDK dogfood fixture loop-policy"
"#;

async fn installed_manager()
-> Result<(tempfile::TempDir, Arc<PluginManager>), Box<dyn std::error::Error>> {
    let Some(wasm) = fixture_wasm() else {
        return Err(skip_reason().into());
    };
    common::ensure_host_dll();
    let ws = tempfile::tempdir()?;
    let mgr = Arc::new(PluginManager::new_with_dirs(
        ws.path(),
        &ws.path().join("plugins_wasm"),
        PluginLimits::default(),
        Arc::new(NoSecrets),
    )?);
    let tmp = tempfile::tempdir()?;
    let src = stage_plugin(tmp.path(), POLICY_SAMPLE_TOML.to_string(), &wasm)?;
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let outcome = installer.install(&src, true).await?;
    assert_eq!(outcome.trust, PluginTrustState::ReviewRequired);
    assert!(
        outcome.registered.is_none(),
        "新装默认不加载（2026-10-22 语义）"
    );
    // 测试语义 =「用户已开开关」：装配后策略点才可路由。
    installer
        .set_enabled("policy-sample", true)
        .await?
        .expect("loaded");
    Ok((ws, mgr))
}

fn verdict_of(out: &str) -> serde_json::Value {
    serde_json::from_str(out).expect("guest verdict is valid JSON")
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_install_metadata_and_takeover() {
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };

    // 安装期对账：get-policy-metadata 探针三点全报接管。
    let reg = mgr.get("policy-sample").expect("registered");
    let pm = reg.policy_meta.as_ref().expect("policy meta");
    assert!(
        pm.compact && pm.final_judge && pm.validation,
        "meta: {pm:?}"
    );
    assert_eq!(pm.points().len(), 3);

    // 运行期路由：三点的接管者都是本组件（每点唯一激活组件）。
    for point in [
        PolicyPoint::Compact,
        PolicyPoint::FinalJudge,
        PolicyPoint::Validation,
    ] {
        let owner = mgr.policy_owner(point).expect("owner");
        assert_eq!(owner.manifest.slug, "policy-sample", "point {point:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_validation_verdicts() {
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };

    // reject：exec args 含 __REJECT__ 标记。
    let input = serde_json::json!({
        "tool": "exec",
        "schema": serde_json::Value::Null,
        "args": r#"{"cmd":"echo __REJECT__"}"#,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::Validation, input.to_string())
        .await
        .expect("call");
    let v = verdict_of(&out);
    assert_eq!(v["verdict"], "reject", "got: {out}");
    assert!(v["message"].as_str().unwrap_or("").contains("__REJECT__"));

    // fixed：write_file 路径含 __autofix_this__ → 标记改写，args 合法 JSON。
    let input = serde_json::json!({
        "tool": "write_file",
        "schema": serde_json::Value::Null,
        "args": r#"{"path":"notes/__autofix_this__.txt","content":"x"}"#,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::Validation, input.to_string())
        .await
        .expect("call");
    let v = verdict_of(&out);
    assert_eq!(v["verdict"], "fixed", "got: {out}");
    assert_eq!(
        v["args"],
        r#"{"path":"notes/autofixed-by-policy-sample.txt","content":"x"}"#
    );
    serde_json::from_str::<serde_json::Value>(v["args"].as_str().unwrap())
        .expect("fixed args is valid JSON");

    // builtin：无关工具零干扰（高频路径回退演示）。
    let input = serde_json::json!({
        "tool": "read_file",
        "schema": serde_json::Value::Null,
        "args": r#"{"path":"a.txt"}"#,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::Validation, input.to_string())
        .await
        .expect("call");
    assert_eq!(verdict_of(&out)["verdict"], "builtin", "got: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_judge_and_compact_verdicts() {
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };

    // giveup：content 含 __FORCE_GIVEUP__。
    let input = serde_json::json!({
        "session_key": "s1",
        "content": "回答 __FORCE_GIVEUP__ 完",
        "turns_used": 3,
        "validation_failures": 0,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::FinalJudge, input.to_string())
        .await
        .expect("call");
    let v = verdict_of(&out);
    assert_eq!(v["verdict"], "giveup", "got: {out}");
    assert!(
        v["notice"]
            .as_str()
            .unwrap_or("")
            .contains("__FORCE_GIVEUP__")
    );

    // retry：宿主 LLM invoker 未接线 → guest llm-complete 诚实回退静态文案。
    let input = serde_json::json!({
        "session_key": "s1",
        "content": "回答 __FORCE_RETRY__ 完",
        "turns_used": 2,
        "validation_failures": 0,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::FinalJudge, input.to_string())
        .await
        .expect("call");
    let v = verdict_of(&out);
    assert_eq!(v["verdict"], "retry", "got: {out}");
    assert_eq!(
        v["nudge"],
        "policy-sample: 回答包含 __FORCE_RETRY__ 标记，请修正后重新作答"
    );

    // builtin：正常内容不干预终答裁决。
    let input = serde_json::json!({
        "session_key": "s1",
        "content": "正常回答，没有任何标记",
        "turns_used": 1,
        "validation_failures": 0,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::FinalJudge, input.to_string())
        .await
        .expect("call");
    assert_eq!(verdict_of(&out)["verdict"], "builtin", "got: {out}");

    // compact trigger：过软位（soft = 软位 token 位置，非零）压制（skip），
    // 硬位回宿主（builtin）——镜像宿主桥真实投影（全数值）。
    let input = serde_json::json!({
        "point": "trigger",
        "session_key": "s1",
        "tail_tokens": 1000,
        "provider_signal": 0,
        "signal": 1000,
        "threshold": 2000,
        "soft": 1333,
        "window": 8000,
        "covers_up_to": 0,
        "new_boundary": 500,
    });
    let out = mgr
        .call_policy_point(PolicyPoint::Compact, input.to_string())
        .await
        .expect("call");
    assert_eq!(verdict_of(&out)["verdict"], "skip", "got: {out}");

    let mut hard = input.clone();
    hard["soft"] = serde_json::json!(0);
    let out = mgr
        .call_policy_point(PolicyPoint::Compact, hard.to_string())
        .await
        .expect("call");
    assert_eq!(verdict_of(&out)["verdict"], "builtin", "got: {out}");

    // compact exec：确定性占位摘要（条数 = prefix 长度）。
    let input = serde_json::json!({
        "point": "exec",
        "session_key": "s1",
        "prefix": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "yo", "tool_name": null},
        ],
        "existing_summary": "",
        "window": 100,
        "model": "m1",
    });
    let out = mgr
        .call_policy_point(PolicyPoint::Compact, input.to_string())
        .await
        .expect("call");
    let v = verdict_of(&out);
    assert_eq!(v["verdict"], "summary", "got: {out}");
    assert_eq!(
        v["summary"],
        "[policy-sample] 已折叠 2 条历史（window=100，model=m1）"
    );

    // exec 已有 __NO_RECOMPACT__ 摘要 → skip（不再重压）。
    let mut keep = input.clone();
    keep["existing_summary"] = serde_json::Value::String("__NO_RECOMPACT__ 旧摘要".into());
    let out = mgr
        .call_policy_point(PolicyPoint::Compact, keep.to_string())
        .await
        .expect("call");
    assert_eq!(verdict_of(&out)["verdict"], "skip", "got: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_disable_reenable_fallback_chain() {
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };
    let input = serde_json::json!({
        "tool": "exec",
        "schema": serde_json::Value::Null,
        "args": r#"{"cmd":"echo __REJECT__"}"#,
    });

    // 禁用 = 摘注册摘接管（NotAvailable → 宿主桥回退内置哨兵）。
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .set_enabled("policy-sample", false)
        .await
        .expect("disable");
    let err = mgr
        .call_policy_point(PolicyPoint::Validation, input.to_string())
        .await
        .expect_err("disabled = NotAvailable");
    assert!(
        matches!(err, nemesis_plugins_wasm::PluginError::NotAvailable(_)),
        "got: {err:?}"
    );
    assert!(mgr.policy_owner(PolicyPoint::Validation).is_none());

    // 重启用 = 重装载重接管（lockfile 有记录，load_one 原路）。
    installer
        .set_enabled("policy-sample", true)
        .await
        .expect("enable")
        .expect("loaded");
    let out = mgr
        .call_policy_point(PolicyPoint::Validation, input.to_string())
        .await
        .expect("call after re-enable");
    assert_eq!(verdict_of(&out)["verdict"], "reject", "got: {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_validation_latency_probe() {
    // 性能探针（计划批 5④）：validate-args 是高频路径（每次 LLM 工具调用
    // 一次），fresh-store-per-call 的固有开销在此实测。只打印不做墙钟硬
    // 断言（跨机噪声大，报告文档引用实测值；超标治理 = validate 帧低
    // fuel 档）。--nocapture 观看。
    let Ok((_ws, mgr)) = installed_manager().await else {
        eprintln!("{}", skip_reason());
        return;
    };
    let input = serde_json::json!({
        "tool": "read_file",
        "schema": serde_json::Value::Null,
        "args": r#"{"path":"a.txt"}"#,
    })
    .to_string();

    // 预热一轮（首调含组件编译缓存写入）。
    mgr.call_policy_point(PolicyPoint::Validation, input.clone())
        .await
        .expect("warmup");

    let n = 30u32;
    let mut samples: Vec<f64> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let t0 = std::time::Instant::now();
        let out = mgr
            .call_policy_point(PolicyPoint::Validation, input.clone())
            .await
            .expect("call");
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(verdict_of(&out)["verdict"], "builtin");
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() as f64 * 0.95) as usize % samples.len()];
    let max = samples[samples.len() - 1];
    println!(
        "policy validate-args per-call ms (n={n}): mean={mean:.2} p50={p50:.2} p95={p95:.2} max={max:.2}"
    );
    // 病态回归哨兵（单次调用破秒 = 环境/框架级问题，不是性能波动）。
    assert!(max < 1000.0, "per-call {max:.0}ms — pathological");
}
