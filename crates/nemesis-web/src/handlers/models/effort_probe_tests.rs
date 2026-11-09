//! effort 探针（probe 二期，2026-11-09）测试——判定纯函数 + 对照判别
//! 聚合 + 记录写盘 round-trip + 标记/注记渲染矩阵 + 启动 sweep 选取 +
//! probe_effort_entry 回执形态（缺条目/图像/非 HttpCompat/基准失败）。
//!
//! 纯函数全直测；触网臂只用不可达端口（127.0.0.1:1 → connect refused，
//! 快且确定性）验 fail/baseline 形态——ok 形态依赖真实端点行为，归手动
//! CLI / UAT，不进单测。与同目录测试批同一形态：声明在 models.rs 内、
//! tempfile 临时目录 + 直接写 config.json。

use super::*;

fn write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn read_config_raw(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

// classify_probe_error ----------------------------------------------------

#[test]
fn classify_400_body_is_rejected() {
    // http lane 400 真实 Display 形态（thiserror Unknown + "status N: body"）。
    let v = classify_probe_error(
        "unknown error from provider openai_compat/glm-5.3-flash: status 400: \
         {\"error\":{\"code\":\"1210\",\"msg\":\"请使用 low、high 或 max\"}}",
    );
    match v {
        EffortProbeVerdict::Rejected(body) => {
            assert!(body.contains("1210"), "端点原文随判定保留：{body}");
            assert!(body.contains("low"), "枚举提示随判定保留：{body}");
        }
        other => panic!("400 应判 Rejected，got {other:?}"),
    }
}

#[test]
fn classify_auth_401_is_inconclusive() {
    // Auth 类词表先行——账户面问题不得污染档位判定。
    let v = classify_probe_error("auth failure for provider openai_compat/m: status 401");
    assert!(
        matches!(v, EffortProbeVerdict::Inconclusive(_)),
        "401 应判 Inconclusive，got {v:?}"
    );
}

#[test]
fn classify_connect_error_is_inconclusive() {
    // "connect error" 前缀（诚实传输错误映射）→ Timeout 类 → 未定。
    let v = classify_probe_error(
        "unknown error from provider openai_compat/m: connect error (model m): tcp refuse",
    );
    assert!(matches!(v, EffortProbeVerdict::Inconclusive(_)));
}

#[test]
fn classify_429_is_inconclusive() {
    // \b429\b 词边界 → RateLimit 类 → 未定（限流不是档位拒绝）。
    let v = classify_probe_error("unknown error from provider p/m: status 429: slow down");
    assert!(matches!(v, EffortProbeVerdict::Inconclusive(_)));
}

#[test]
fn classify_5xx_overloaded_is_inconclusive() {
    let v = classify_probe_error("provider p is overloaded (status: 503)");
    assert!(matches!(v, EffortProbeVerdict::Inconclusive(_)));
}

#[test]
fn classify_plain_timeout_is_inconclusive() {
    let v = classify_probe_error("timeout calling provider p/m");
    assert!(matches!(v, EffortProbeVerdict::Inconclusive(_)));
}

// aggregate_effort_probe ----------------------------------------------------

/// 6 档全 Accepted 的便捷构造。
fn all_accepted() -> Vec<(&'static str, EffortProbeVerdict)> {
    EFFORT_PROBE_CANDIDATES
        .iter()
        .map(|w| (*w, EffortProbeVerdict::Accepted))
        .collect()
}

#[test]
fn aggregate_strict_keeps_accepted() {
    let results = all_accepted();
    let (record, summary) = aggregate_effort_probe(
        &results,
        &EffortProbeVerdict::Rejected("status 400: unknown value".into()),
    )
    .expect("对照拒绝 = strict = 结果可信");
    assert_eq!(record["control"], "strict");
    let acc = record["accepted"].as_array().unwrap();
    assert_eq!(acc.len(), 6, "strict 下 6 档全入 accepted");
    assert_eq!(record["rejected"].as_object().unwrap().len(), 0);
    assert!(record["probed_at"].as_str().is_some(), "时间戳在场");
    assert!(
        summary.contains("strict") || summary.contains("接受"),
        "摘要提及形态：{summary}"
    );
}

#[test]
fn aggregate_ignore_voids_accepted_keeps_rejected() {
    // 拍板原文：对照 200（宽容端点）→ 「接受」全部作废（acceptance ≠
    // effective），真实拒绝保留。
    let mut results = all_accepted();
    results.push((
        "low",
        EffortProbeVerdict::Rejected("status 400: bad effort".into()),
    ));
    let (record, summary) = aggregate_effort_probe(&results, &EffortProbeVerdict::Accepted)
        .expect("对照接受 = ignore 模式仍出记录（只是 accepted 作废）");
    assert_eq!(record["control"], "ignore");
    assert!(
        record["accepted"].as_array().unwrap().is_empty(),
        "ignore 模式 accepted 必须全作废"
    );
    let rej = record["rejected"].as_object().unwrap();
    assert_eq!(rej.len(), 1, "真实拒绝保留");
    assert!(rej.contains_key("low"));
    assert!(
        summary.contains("ignore") || summary.contains("宽容"),
        "摘要提及形态：{summary}"
    );
}

#[test]
fn aggregate_control_inconclusive_is_none() {
    assert!(
        aggregate_effort_probe(
            &all_accepted(),
            &EffortProbeVerdict::Inconclusive("connect error".into()),
        )
        .is_none(),
        "对照未定 = 整轮不可信 = None 不写盘"
    );
}

#[test]
fn aggregate_inconclusive_tier_omitted() {
    let results: Vec<(&'static str, EffortProbeVerdict)> = EFFORT_PROBE_CANDIDATES
        .iter()
        .map(|w| {
            if *w == "xhigh" {
                (*w, EffortProbeVerdict::Inconclusive("timeout".into()))
            } else {
                (*w, EffortProbeVerdict::Accepted)
            }
        })
        .collect();
    let (record, _) = aggregate_effort_probe(
        &results,
        &EffortProbeVerdict::Rejected("status 400: zzz".into()),
    )
    .unwrap();
    let acc = record["accepted"].as_array().unwrap();
    assert_eq!(acc.len(), 5, "未定档不进 accepted（UI 渲染 ?）");
    assert!(!acc.iter().any(|v| v == "xhigh"));
}

// write_effort_probe（记录 round-trip + 重探覆盖） -------------------------

const ROUNDTRIP_SEED: &str = r#"{
  "agents": { "defaults": { "llm": "m1" } },
  "model_list": [
    { "model_name": "m1", "model": "qwen/qwen3-30b", "api_key": "sk-x", "reasoning_effort": "low" }
  ]
}"#;

#[test]
fn write_effort_probe_roundtrip_and_reprobe_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), ROUNDTRIP_SEED);
    let h = home_str(&dir);

    // 首探写盘。
    let results = all_accepted();
    let (record, _) = aggregate_effort_probe(
        &results,
        &EffortProbeVerdict::Rejected("status 400: zzz".into()),
    )
    .unwrap();
    super::write_effort_probe(&h, "m1", &record).unwrap();
    let cfg = read_config_raw(dir.path());
    let written = &cfg["model_list"][0]["effort_probe"];
    assert_eq!(written["control"], "strict");
    assert_eq!(written["accepted"].as_array().unwrap().len(), 6);
    assert!(
        written["probed_at"].as_str().unwrap().contains('T'),
        "RFC3339 时间戳"
    );

    // 重探覆盖：拒绝形态的新记录整体替换（不残留旧键）。
    let mut results2 = all_accepted();
    results2[0].1 = EffortProbeVerdict::Rejected("status 400: minimal is unsupported".into());
    let (record2, _) = aggregate_effort_probe(
        &results2,
        &EffortProbeVerdict::Rejected("status 400: zzz".into()),
    )
    .unwrap();
    super::write_effort_probe(&h, "m1", &record2).unwrap();
    let cfg2 = read_config_raw(dir.path());
    let written2 = &cfg2["model_list"][0]["effort_probe"];
    assert_eq!(
        written2["rejected"]["minimal"].as_str().unwrap(),
        "status 400: minimal is unsupported",
        "重探记录整体覆盖"
    );
    assert!(
        !written2.as_object().unwrap().contains_key("stale"),
        "重探后 stale 标记不残留（覆盖写语义）"
    );
}

// effort_marks_and_notes 渲染矩阵 -------------------------------------------

#[test]
fn marks_fallback_identity_partial_acceptance() {
    // 通用端点（无方言）：off=NotSent 恒 ok；accepted {low,max} 两档 ✓；
    // rejected {medium} ✗ 带端点原文；其余 ?。
    let probe = serde_json::json!({
        "accepted": ["low", "max"],
        "rejected": { "medium": "status 400: unsupported value 'medium'" },
        "control": "strict",
        "probed_at": "2026-11-09T00:00:00Z"
    });
    let marks = effort_marks_for_entry(
        "vendor/model-x",
        "http://x/v1",
        &serde_json::Value::Null,
        &probe,
    );
    let notes = effort_notes_for_entry(
        "vendor/model-x",
        "http://x/v1",
        &serde_json::Value::Null,
        &probe,
    );
    assert_eq!(marks.len(), 7, "与 EFFORT_TIERS 平行");
    assert_eq!(marks[0], "ok", "off = NotSent（nothing to reject）");
    assert_eq!(marks[1], "unknown", "minimal 未探测");
    assert_eq!(marks[2], "ok", "low accepted");
    assert_eq!(marks[3], "bad", "medium rejected");
    assert_eq!(marks[4], "unknown", "high 未探测");
    assert_eq!(marks[5], "unknown", "xhigh 未探测");
    assert_eq!(marks[6], "ok", "max accepted");
    assert!(
        notes[3].contains("unsupported value"),
        "拒绝原文进注记（tooltip）：{}",
        notes[3]
    );
    assert_eq!(notes[2], "", "accepted 档无注记");
}

#[test]
fn marks_glm_ceil_mapping_all_ok_from_wire_semantics() {
    // GLM 实测（验证脚本 .tmp-effort-probe-validate）：raw minimal/medium/
    // xhigh 400，low/high/max 200。方言 ceil 后 minimal→low 等在 wire 面
    // 全部命中 accepted——用户可选档全部可用（✓ 是 wire 语义不是 raw 键）。
    let probe = serde_json::json!({
        "accepted": ["low", "high", "max"],
        "rejected": { "minimal": "400 code 1210", "medium": "400 code 1210", "xhigh": "400 code 1210" },
        "control": "strict",
        "probed_at": "2026-11-09T00:00:00Z"
    });
    let marks = effort_marks_for_entry(
        "glm-5.3-flash",
        "https://open.bigmodel.cn/api/paas/v4",
        &serde_json::Value::Null,
        &probe,
    );
    // minimal→low ok / low→low ok / medium→high ok / high→high ok / xhigh→max ok / max→max ok
    assert!(
        marks.iter().all(|m| *m == "ok"),
        "GLM ceil 后全部档位 wire 命中 accepted：{marks:?}"
    );
}

#[test]
fn marks_stale_resets_everything_to_unknown() {
    let probe = serde_json::json!({
        "accepted": ["low", "max"],
        "rejected": { "medium": "400" },
        "control": "strict",
        "probed_at": "2026-11-09T00:00:00Z",
        "stale": true
    });
    let marks = effort_marks_for_entry(
        "vendor/model-x",
        "http://x/v1",
        &serde_json::Value::Null,
        &probe,
    );
    assert!(
        marks[1..].iter().all(|m| *m == "unknown"),
        "stale = 判定记录整体作废（off 除外，恒 ok）：{marks:?}"
    );
}

#[test]
fn marks_no_probe_record_all_unknown() {
    let marks = effort_marks_for_entry(
        "qwen/qwen3-30b",
        "http://x/v1",
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    );
    assert!(
        marks[1..].iter().all(|m| *m == "unknown"),
        "无记录 = 全部 ?：{marks:?}"
    );
    assert_eq!(marks[0], "ok", "off 恒 NotSent");
}

#[test]
fn marks_private_key_dialect_not_probeable() {
    // qwen3 enable_thinking / glm thinking 私有键方言：静态探针测不到
    // ——全档 ?（诚实未定）。
    let probe = serde_json::json!({
        "accepted": ["low"], "rejected": {}, "control": "strict",
        "probed_at": "2026-11-09T00:00:00Z"
    });
    let marks = effort_marks_for_entry(
        "qwen/qwen3-30b",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        &serde_json::Value::Null,
        &probe,
    );
    assert!(
        marks[1..].iter().all(|m| *m == "unknown"),
        "私有键方言 NotProbeable：{marks:?}"
    );
}

#[test]
fn marks_deepseek_not_sent_all_ok() {
    let probe = serde_json::Value::Null;
    let marks = effort_marks_for_entry(
        "deepseek/deepseek-reasoner",
        "https://api.deepseek.com/v1",
        &serde_json::Value::Null,
        &probe,
    );
    assert!(
        marks.iter().all(|m| *m == "ok"),
        "零注入方言 NotSent 恒 ok：{marks:?}"
    );
}

#[test]
fn marks_extra_body_pin_maps_all_tiers_to_pinned_wire() {
    // extra_body 钉死 reasoning_effort=low：所有档位上线同一 wire 值 →
    // 全部按 low 的判定渲染（诚实：钉死档位的判定就是全部档位的判定）。
    let probe = serde_json::json!({
        "accepted": ["low"], "rejected": {}, "control": "strict",
        "probed_at": "2026-11-09T00:00:00Z"
    });
    let pin = serde_json::json!({ "reasoning_effort": "low" });
    let marks = effort_marks_for_entry("vendor/model-x", "http://x/v1", &pin, &probe);
    assert!(
        marks[1..].iter().all(|m| *m == "ok"),
        "钉死 low 且 low accepted → 全 ✓：{marks:?}"
    );
}

// effort_probe_stale_entries（启动 sweep 候选选取） -------------------------

const SWEEP_SEED: &str = r#"{
  "agents": { "defaults": { "llm": "fresh" } },
  "model_list": [
    { "model_name": "missing-probe", "model": "qwen/qwen3-30b", "api_key": "k", "reasoning_effort": "low" },
    { "model_name": "fresh", "model": "qwen/qwen3-30b", "api_key": "k", "reasoning_effort": "low",
      "effort_probe": { "accepted": ["low"], "rejected": {}, "control": "strict", "probed_at": "__FRESH_TS__" } },
    { "model_name": "aged", "model": "qwen/qwen3-30b", "api_key": "k", "reasoning_effort": "high",
      "effort_probe": { "accepted": ["high"], "rejected": {}, "control": "strict", "probed_at": "2026-01-01T00:00:00Z" } },
    { "model_name": "stale-flag", "model": "qwen/qwen3-30b", "api_key": "k", "reasoning_effort": "low",
      "effort_probe": { "accepted": ["low"], "rejected": {}, "control": "strict", "probed_at": "__FRESH_TS__", "stale": true } },
    { "model_name": "no-effort", "model": "qwen/qwen3-30b", "api_key": "k" },
    { "model_name": "effort-off", "model": "qwen/qwen3-30b", "api_key": "k", "reasoning_effort": "off" },
    { "model_name": "images", "model": "openai/img-x", "api_key": "k", "protocol": "images-openai", "reasoning_effort": "low" },
    { "model_name": "bad-ts", "model": "qwen/qwen3-30b", "api_key": "k", "reasoning_effort": "low",
      "effort_probe": { "accepted": [], "rejected": {}, "control": "strict", "probed_at": "not-a-date" } }
  ]
}"#;

#[test]
fn sweep_selects_missing_stale_aged_and_badts_only() {
    let dir = tempfile::tempdir().unwrap();
    let seed = SWEEP_SEED.replace("__FRESH_TS__", &chrono::Utc::now().to_rfc3339());
    write_config(dir.path(), &seed);
    let mut got = effort_probe_stale_entries(&home_str(&dir));
    got.sort();
    assert_eq!(
        got,
        vec![
            "aged".to_string(),
            "bad-ts".to_string(),
            "missing-probe".to_string(),
            "stale-flag".to_string()
        ],
        "候选 = [无记录, stale, 超龄, 时间戳解析失败]；fresh/no-effort/off/images 排除"
    );
}

#[test]
fn sweep_refresh_days_zero_disables() {
    let dir = tempfile::tempdir().unwrap();
    let seed = SWEEP_SEED.replace("__FRESH_TS__", &chrono::Utc::now().to_rfc3339());
    write_config(dir.path(), &seed);
    // 覆写 agents 段加 probe_effort_refresh_days=0。
    let mut cfg = read_config_raw(dir.path());
    cfg["agents"]["probe_effort_refresh_days"] = serde_json::json!(0);
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    assert!(
        effort_probe_stale_entries(&home_str(&dir)).is_empty(),
        "refresh_days=0 = 整闸关闭"
    );
}

#[test]
fn sweep_unreadable_config_is_empty_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    // 无 config.json（home 目录空）。
    assert!(effort_probe_stale_entries(&home_str(&dir)).is_empty());
}

// probe_effort_entry 回执形态 -----------------------------------------------

#[tokio::test]
async fn entry_missing_errors() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), ROUNDTRIP_SEED);
    let err = probe_effort_entry(&home_str(&dir), "ghost")
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[tokio::test]
async fn entry_images_protocol_unsupported_receipt() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
        "agents": { "defaults": { "llm": "img" } },
        "model_list": [
            { "model_name": "img", "model": "openai/img-x", "api_key": "k", "protocol": "images-openai" }
        ]
    }"#,
    );
    let r = probe_effort_entry(&home_str(&dir), "img").await.unwrap();
    assert_eq!(r["status"], "unsupported");
    assert_eq!(r["lane"], "images-openai");
}

#[tokio::test]
async fn entry_anthropic_lane_skipped_receipt() {
    // anthropic lane 不可静态探测（预算映射依赖运行时 max_tokens）——
    // 诚实 skip 回执，不写盘。
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
        "agents": { "defaults": { "llm": "cl" } },
        "model_list": [
            { "model_name": "cl", "model": "claude-x", "api_key": "k", "protocol": "anthropic",
              "api_base": "https://api.anthropic.com", "reasoning_effort": "low" }
        ]
    }"#,
    );
    let h = home_str(&dir);
    let r = probe_effort_entry(&h, "cl").await.unwrap();
    assert_eq!(r["status"], "skipped", "回执：{r}");
    assert!(
        !read_config_raw(dir.path())["model_list"][0]
            .as_object()
            .unwrap()
            .contains_key("effort_probe"),
        "skip 不写盘"
    );
}

#[tokio::test]
async fn entry_baseline_fail_receipt_unreachable_port() {
    // HttpCompat lane + 不可达端口：基准请求（不带 effort）先死 →
    // fail kind=baseline，不写盘、不发 7 探针。
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
        "agents": { "defaults": { "llm": "dead" } },
        "model_list": [
            { "model_name": "dead", "model": "qwen/qwen3-30b", "api_key": "k",
              "api_base": "http://127.0.0.1:1/v1", "reasoning_effort": "low" }
        ]
    }"#,
    );
    let h = home_str(&dir);
    let r = probe_effort_entry(&h, "dead").await.unwrap();
    assert_eq!(r["status"], "fail", "回执：{r}");
    assert_eq!(r["kind"], "baseline", "失败归因基准请求：{r}");
    assert!(
        !read_config_raw(dir.path())["model_list"][0]
            .as_object()
            .unwrap()
            .contains_key("effort_probe"),
        "基准失败不写盘"
    );
}

// list() 标记面 --------------------------------------------------------------

#[test]
fn list_rows_carry_effort_marks_and_notes() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
        "agents": { "defaults": { "llm": "m1" } },
        "model_list": [
            { "model_name": "m1", "model": "vendor/model-x", "api_key": "k", "reasoning_effort": "low",
              "effort_probe": { "accepted": ["low"], "rejected": { "medium": "status 400: no" },
                                "control": "strict", "probed_at": "2026-11-09T00:00:00Z" } }
        ]
    }"#,
    );
    let h = ModelsHandler::new();
    let out = h.list(&home_str(&dir)).unwrap().unwrap();
    let row = &out["models"][0];
    let marks = row["effort_marks"].as_array().unwrap();
    assert_eq!(marks.len(), 7);
    assert_eq!(marks[2], "ok", "low accepted");
    assert_eq!(marks[3], "bad", "medium rejected");
    let notes = row["effort_notes"].as_array().unwrap();
    assert_eq!(notes.len(), 7);
    assert!(notes[3].as_str().unwrap().contains("status 400"));
    assert!(
        row["effort_probe"]["control"] == "strict",
        "原始记录随行下发"
    );
}

// effort_auto_fix_target（C · 探测后自动修正）＋ apply_effort_autofix -------

fn accepted_of(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn autofix_identity_rejected_tier_nearest_low_on_tie() {
    // xhigh(5) 被拒，accepted=[low,high,max]（Sol 实测形态）：high(4)/max(6)
    // 同距 1 → 平距取低 = high。
    let got = effort_auto_fix_target(
        "sol/gpt-5.6-sol",
        "http://x/v1",
        &serde_json::Value::Null,
        "xhigh",
        &accepted_of(&["low", "high", "max"]),
    );
    assert_eq!(got, Some(("xhigh".to_string(), "high".to_string())));
}

#[test]
fn autofix_nearest_upper_only_candidate() {
    // medium(3) 被拒，只有 medium 上方可接受档存在时取上（唯一候选）。
    let got = effort_auto_fix_target(
        "vendor/model-x",
        "http://x/v1",
        &serde_json::Value::Null,
        "medium",
        &accepted_of(&["high"]),
    );
    assert_eq!(got, Some(("medium".to_string(), "high".to_string())));
}

#[test]
fn autofix_tie_prefers_lower_direction() {
    // medium(3) 被拒，low(2)/high(4) 同距 → 取低（保守方向）。
    let got = effort_auto_fix_target(
        "vendor/model-x",
        "http://x/v1",
        &serde_json::Value::Null,
        "medium",
        &accepted_of(&["low", "high"]),
    );
    assert_eq!(got, Some(("medium".to_string(), "low".to_string())));
}

#[test]
fn autofix_current_tier_legal_is_none() {
    // 当前档 wire 目标 ∈ accepted（合法）→ 不动。
    assert_eq!(
        effort_auto_fix_target(
            "vendor/model-x",
            "http://x/v1",
            &serde_json::Value::Null,
            "low",
            &accepted_of(&["low", "high"]),
        ),
        None
    );
}

#[test]
fn autofix_glm_mapped_legal_is_none() {
    // GLM 实证形态：配置 medium → wire=high ∈ accepted → 不动（方言
    // 映射已经把配置档落在端点接受的值上）。
    assert_eq!(
        effort_auto_fix_target(
            "zhipu/glm-5.3-flash",
            "https://open.bigmodel.cn/api/paas/v4",
            &serde_json::Value::Null,
            "medium",
            &accepted_of(&["low", "high", "max"]),
        ),
        None
    );
}

#[test]
fn autofix_glm_mapped_rejected_follows_wire_semantics() {
    // GLM 配 xhigh → wire=max 被拒；候选按 wire 判定（medium→high 可接受）
    // → 最近 = high（档距 1）。never 按配置名直判。
    let got = effort_auto_fix_target(
        "zhipu/glm-5.3-flash",
        "https://open.bigmodel.cn/api/paas/v4",
        &serde_json::Value::Null,
        "xhigh",
        &accepted_of(&["low", "high"]),
    );
    assert_eq!(got, Some(("xhigh".to_string(), "high".to_string())));
}

#[test]
fn autofix_off_and_empty_and_no_accepted_are_none() {
    let a = accepted_of(&["low"]);
    assert_eq!(
        effort_auto_fix_target(
            "vendor/m",
            "http://x/v1",
            &serde_json::Value::Null,
            "off",
            &a
        ),
        None,
        "off = 不发键，无修正对象"
    );
    assert_eq!(
        effort_auto_fix_target("vendor/m", "http://x/v1", &serde_json::Value::Null, "", &a),
        None,
        "空 effort = 未配置"
    );
    assert_eq!(
        effort_auto_fix_target(
            "vendor/m",
            "http://x/v1",
            &serde_json::Value::Null,
            "high",
            &accepted_of(&[]),
        ),
        None,
        "ignore 宽容端点 accepted 作废 = 无法判定不瞎修"
    );
}

#[test]
fn autofix_private_key_dialect_not_probeable_is_none() {
    // qwen3 私有键方言：wire 目标 NotProbeable → 探测数据不可判定。
    assert_eq!(
        effort_auto_fix_target(
            "dashscope/qwen3-30b",
            "https://x/v1",
            &serde_json::Value::Null,
            "high",
            &accepted_of(&["low"]),
        ),
        None
    );
}

#[test]
fn autofix_extra_body_pin_escape_hatch_is_honest_no_fix() {
    // 钉档被拒：所有档同发钉值 ∉ accepted → 候选集空 → None（钉档是
    // 用户逃生舱，自动改它越权）。钉档合法（= accepted）→ 当前合法 → None。
    let pin_rejected = serde_json::json!({ "reasoning_effort": "minimal" });
    assert_eq!(
        effort_auto_fix_target(
            "vendor/model-x",
            "http://x/v1",
            &pin_rejected,
            "high",
            &accepted_of(&["low", "high"]),
        ),
        None,
        "钉档被拒 → 候选集空，诚实不修"
    );
    let pin_legal = serde_json::json!({ "reasoning_effort": "low" });
    assert_eq!(
        effort_auto_fix_target(
            "vendor/model-x",
            "http://x/v1",
            &pin_legal,
            "high",
            &accepted_of(&["low"]),
        ),
        None,
        "钉档 wire=low ∈ accepted → 当前态合法，不动"
    );
}

#[test]
fn apply_effort_autofix_roundtrip_and_receipt_shape() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
        "agents": { "defaults": { "llm": "m1" } },
        "model_list": [
            { "model_name": "m1", "model": "sol/gpt-5.6-sol", "api_key": "k",
              "reasoning_effort": "xhigh",
              "effort_probe": { "accepted": ["low", "high", "max"], "rejected": {},
                                "control": "strict", "probed_at": "2026-11-09T00:00:00Z" } }
        ]
    }"#,
    );
    let h = home_str(&dir);
    apply_effort_autofix(&h, "m1", "xhigh", "high").expect("autofix 写盘");

    let cfg = read_config_raw(dir.path());
    let entry = &cfg["model_list"][0];
    assert_eq!(entry["reasoning_effort"], "high", "配置档位已自动改写");
    let fx = &entry["effort_probe"]["auto_fix"];
    assert_eq!(fx["from"], "xhigh");
    assert_eq!(fx["to"], "high");
    assert!(
        fx["at"].as_str().is_some_and(|s| !s.is_empty()),
        "注记带时间戳留痕"
    );
    // 兄弟 extras 不动。
    assert_eq!(entry["model"], "sol/gpt-5.6-sol");
    assert_eq!(entry["api_key"], "k");
    assert_eq!(cfg["agents"]["defaults"]["llm"], "m1");

    // 条目缺席 = 诚实报错（不造幻影写）。
    let err = apply_effort_autofix(&h, "ghost", "xhigh", "high");
    assert!(err.is_err());
}

// probe_effort_on_save_enabled（磁盘直读闸）--------------------------------

#[test]
fn gate_reads_disk_directly_conservative_default_on_miss() {
    // 键缺席（无 agents 段）= 默认开。
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{ "model_list": [ { "model_name": "m1", "model": "sol/gpt-5.6-sol" } ] }"#,
    );
    assert!(
        super::probe_effort_on_save_enabled(&home_str(&dir)),
        "无 agents 段 = 保守按开"
    );

    // 磁盘真相显式 false = 关（直读磁盘，不经 live-first 路径）。
    write_config(
        dir.path(),
        r#"{ "agents": { "probe_effort_on_save": false } }"#,
    );
    assert!(
        !super::probe_effort_on_save_enabled(&home_str(&dir)),
        "磁盘 false 必须生效——w5b2 全量红根因即此键被 live 槽遮蔽"
    );

    // true = 开；非布尔值 = as_bool 失败落保守开。
    write_config(
        dir.path(),
        r#"{ "agents": { "probe_effort_on_save": true } }"#,
    );
    assert!(super::probe_effort_on_save_enabled(&home_str(&dir)));

    write_config(
        dir.path(),
        r#"{ "agents": { "probe_effort_on_save": "false" } }"#,
    );
    assert!(
        super::probe_effort_on_save_enabled(&home_str(&dir)),
        "非布尔 = 保守按开"
    );

    // config 读失败（坏 JSON）= 保守按开。
    write_config(dir.path(), r#"{ broken"#);
    assert!(super::probe_effort_on_save_enabled(&home_str(&dir)));
}
