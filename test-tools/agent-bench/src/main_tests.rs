//! main.rs 装配面单测：契约探针判定核 + REQUIRED_AI_MODELS 单一真相源漂移锁。
//! 背景（2026-11-02 案例）：TestAIServer 二进制过期曾让 3 场景以纯超时假回退，
//! 探针 + 本组测试闭环该缺陷面。

use crate::report::{Scorecard, compare_baseline};
use crate::{REQUIRED_AI_MODELS, STREAM_FORCED_MODELS, missing_models, write_bench_config};
use serde_json::Value;
use test_harness::TestWorkspace;

/// 构造一个带 .nemesisbot home 的最小工作区（等价 onboard 后形态——
/// write_bench_config 直接写 config.json，父目录必须先存在）。
fn fresh_ws() -> TestWorkspace {
    let ws = TestWorkspace::new().expect("temp workspace");
    std::fs::create_dir_all(ws.home()).expect("create .nemesisbot home");
    ws
}

/// 空服务面 → 全部缺失（最旧形态服务器 / 探针打到非 TestAI 端点）。
#[test]
fn missing_models_empty_served_reports_all() {
    let missing = missing_models(&[]);
    assert_eq!(missing.len(), REQUIRED_AI_MODELS.len());
}

/// 陈旧二进制形态（10-22 exe：有基础/codemode 模型，缺 L21 断流与 chain 对；
/// aadf9a27 起又缺诚实性 liar/honest 与不可逆重复 norepeat/repeat 两对；
/// a3fa6f86 起再缺饱和人消息模型 testai-9.2）
/// → 精确差集，不多不少。
#[test]
fn missing_models_stale_server_reports_exact_missing() {
    let stale = [
        "testai-1.1",
        "testai-9.3",
        "testai-sum-1.0",
        "testai-2.1",
        "testai-5.0",
        "testai-vision-1.0",
        "testai-codemode-1.0",
        "testai-docslift-1.0",
    ];
    let missing = missing_models(&stale);
    assert_eq!(
        missing,
        vec![
            "testai-9.2",
            "testai-lenbreak-1.0",
            "testai-chain-step-1.0",
            "testai-chain-code-1.0",
            "testai-break-1.0",
            "testai-heal-1.0",
            "testai-liar-1.0",
            "testai-honest-1.0",
            "testai-norepeat-1.0",
            "testai-repeat-1.0",
        ]
    );
}

/// 全集在服务 → 零缺失（探针放行）。
#[test]
fn missing_models_full_served_reports_none() {
    let served: Vec<&str> = REQUIRED_AI_MODELS.to_vec();
    assert!(missing_models(&served).is_empty());
}

/// 单一真相源漂移锁：REQUIRED_AI_MODELS 必须与 write_bench_config 生成的
/// model_list 完全一致（双向）——新增场景模型只改 const 表，忘改会在
/// config 侧或探针侧当场红。
#[test]
fn required_models_match_generated_config_model_list() {
    let ws = fresh_ws();
    write_bench_config(&ws, "pro").expect("write bench config");
    let raw = std::fs::read_to_string(ws.config_path()).expect("read config");
    let cfg: Value = serde_json::from_str(&raw).expect("parse config");
    let entries = cfg["model_list"].as_array().expect("model_list array");

    let config_names: Vec<&str> = entries
        .iter()
        .map(|e| e["model_name"].as_str().expect("model_name str"))
        .collect();
    assert_eq!(config_names.len(), REQUIRED_AI_MODELS.len());
    for (idx, required) in REQUIRED_AI_MODELS.iter().enumerate() {
        assert_eq!(config_names[idx], *required, "model_list[{idx}] 顺序漂移");
    }
}

/// 流式强制子集锁：STREAM_FORCED_MODELS 的 config 条目必须带
/// `stream: "yes"`（断流契约前提），其余条目不得带。
#[test]
fn stream_forced_entries_carry_stream_yes() {
    let ws = fresh_ws();
    write_bench_config(&ws, "pro").expect("write bench config");
    let raw = std::fs::read_to_string(ws.config_path()).expect("read config");
    let cfg: Value = serde_json::from_str(&raw).expect("parse config");
    for entry in cfg["model_list"].as_array().expect("model_list array") {
        let name = entry["model_name"].as_str().expect("model_name str");
        let should_force = STREAM_FORCED_MODELS.contains(&name);
        let has_stream = entry["stream"].as_str() == Some("yes");
        assert_eq!(
            has_stream, should_force,
            "模型 {name} 的 stream 键与 STREAM_FORCED_MODELS 不一致"
        );
    }
}

/// BUG-20261104-369 回归锁：基线与本轮 bench 构建戳不一致 → [note] 可见
/// 且不判门（2026-11-04 案例：陈旧 bench 二进制对合法 L29 拼接形态报
/// stream_break 假回退——戳可见性让这类漂移在下结论前暴露）。
#[test]
fn bench_build_mismatch_emits_note_without_regression() {
    let ws = fresh_ws();
    let path = ws.home().join("baseline.json");
    let base = serde_json::json!({
        "schema": "agent-bench/v1",
        "scenarios": [],
        "bench_build_unix": 1000u64,
    });
    std::fs::write(&path, serde_json::to_string(&base).expect("serialize")).expect("write");

    let mut card = Scorecard::new("gw".into());
    card.bench_build_unix = 2000;
    let findings = compare_baseline(&card, &path, 0.30).expect("compare");
    assert_eq!(findings[0], "[ok] 无回退", "构建戳差异不判门");
    assert!(
        findings.iter().any(|f| f.contains("bench 构建戳不一致")),
        "构建戳差异必须给 note: {findings:?}"
    );
}

/// BUG-20261104-369 静默臂：构建戳一致 → 无 note；老基线缺字段 → 静默
/// 跳过（对齐 tool_defs 老格式宽容语义，不产生噪音）。
#[test]
fn bench_build_match_or_absent_stays_silent() {
    let ws = fresh_ws();
    let path = ws.home().join("baseline.json");

    // 臂 1：一致。
    std::fs::write(
        &path,
        serde_json::json!({
            "schema": "agent-bench/v1",
            "scenarios": [],
            "bench_build_unix": 42u64,
        })
        .to_string(),
    )
    .expect("write");
    let mut card = Scorecard::new("gw".into());
    card.bench_build_unix = 42;
    let findings = compare_baseline(&card, &path, 0.30).expect("compare");
    assert!(
        !findings.iter().any(|f| f.contains("bench 构建戳")),
        "一致时不应有 note: {findings:?}"
    );

    // 臂 2：老基线缺 bench_build_unix 字段。
    std::fs::write(
        &path,
        serde_json::json!({
            "schema": "agent-bench/v1",
            "scenarios": [],
        })
        .to_string(),
    )
    .expect("write");
    let findings = compare_baseline(&card, &path, 0.30).expect("compare");
    assert!(
        !findings.iter().any(|f| f.contains("bench 构建戳")),
        "缺字段时静默跳过: {findings:?}"
    );
}

/// BUG-20261104-369 注入链锁：build.rs 的 rustc-env 必须真实可达
/// option_env!——注入断 = 恒 0（未知）→ 构建戳对比静默失效，特性死
/// 到达（单元测试手动赋值字段覆盖不了这一段）。release 优化会把
/// ASCII 字面量 const-fold 成立即数，二进制 grep 验证法不可靠，此
/// API 级断言是注入链的钉。
#[test]
fn bench_build_unix_injected_from_build_rs() {
    let card = Scorecard::new("gw".into());
    assert!(
        card.bench_build_unix > 0,
        "build.rs 构建戳未注入（恒 0=未知，构建戳对比已静默失效）"
    );
}
