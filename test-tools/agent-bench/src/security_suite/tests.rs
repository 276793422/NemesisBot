//! security_suite 纯函数面单测（占位符/秘密生成/目录完整性/规则文件/审计闭环）。
//! 涉及真实 gateway WS 链路的执行面（run_rule / sink_*）归 smoke run，不在单测。

use super::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn fresh_ws() -> TestWorkspace {
    let ws = TestWorkspace::new().expect("temp workspace");
    std::fs::create_dir_all(ws.home()).expect("create .nemesisbot home");
    ws
}

// ---------------------------------------------------------------------------
// 占位符解析
// ---------------------------------------------------------------------------

#[test]
fn placeholders_basic_and_multiple() {
    let mut vars = HashMap::new();
    vars.insert("A".to_string(), "alpha".to_string());
    vars.insert("B".to_string(), "beta".to_string());
    let out = resolve_placeholders("x{{A}}y{{B}}{{A}}z", &vars).unwrap();
    assert_eq!(out, "xalphaybetaalphaz");
}

#[test]
fn placeholders_tolerates_spaces_in_name() {
    let mut vars = HashMap::new();
    vars.insert("W".to_string(), "/w".to_string());
    assert_eq!(resolve_placeholders("{{ W }}!", &vars).unwrap(), "/w!");
}

#[test]
fn placeholders_unknown_var_is_loud() {
    let vars = HashMap::new();
    let err = resolve_placeholders("{{NOPE}}", &vars).unwrap_err();
    assert!(err.to_string().contains("NOPE"), "err: {err}");
}

#[test]
fn placeholders_unclosed_is_loud() {
    let mut vars = HashMap::new();
    vars.insert("A".to_string(), "a".to_string());
    let err = resolve_placeholders("oops {{A", &vars).unwrap_err();
    assert!(err.to_string().contains("未闭合"), "err: {err}");
}

#[test]
fn placeholders_json_braces_survive() {
    // 单花括号是规则 prompt 里的 JSON 字面量，不得被当占位符。
    let mut vars = HashMap::new();
    vars.insert("T".to_string(), "C:/t".to_string());
    let out = resolve_placeholders("{\"p\":\"{{T}}/f\"}", &vars).unwrap();
    assert_eq!(out, "{\"p\":\"C:/t/f\"}");
}

// ---------------------------------------------------------------------------
// 运行时秘密
// ---------------------------------------------------------------------------

/// 独立 Luhn 校验 oracle（不复用生成侧口径——生成算法写错时这里要能抓）。
fn luhn_oracle(full: &str) -> bool {
    let mut sum = 0u32;
    for (i, ch) in full.chars().rev().enumerate() {
        let d = ch.to_digit(10).unwrap_or_else(|| panic!("non-digit {ch}"));
        if i % 2 == 1 {
            let dd = d * 2;
            sum += dd / 10 + dd % 10;
        } else {
            sum += d;
        }
    }
    sum.is_multiple_of(10)
}

#[test]
fn generated_secrets_have_expected_shapes() {
    let s = generate_secrets();
    // stripe: sk_test_ + 32 alnum = 40。
    assert!(s.stripe_key.starts_with("sk_test_"), "{}", s.stripe_key);
    assert_eq!(s.stripe_key.len(), 40);
    assert!(s.stripe_key[8..].chars().all(|c| c.is_ascii_alphanumeric()));
    // 中段 needle 真在载荷里（脱敏锚点的物理前提）。
    assert!(s.stripe_key.contains(&s.stripe_mid));
    assert_eq!(s.stripe_mid.len(), 8);
    // PAN：16 位数字、Visa 4 开头、Luhn 合法（独立 oracle）。
    assert_eq!(s.pan.len(), 16);
    assert!(s.pan.chars().all(|c| c.is_ascii_digit()));
    assert!(s.pan.starts_with('4'));
    assert!(
        luhn_oracle(&s.pan),
        "generated PAN must be Luhn-valid: {}",
        s.pan
    );
    // AKIA：AKIA + 16 大写字母数字 = 20；中段在载荷里。
    assert!(s.akia.starts_with("AKIA"));
    assert_eq!(s.akia.len(), 20);
    assert!(
        s.akia[4..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    );
    assert!(s.akia.contains(&s.akia_mid));
    assert_eq!(s.akia_mid.len(), 8);
}

#[test]
fn luhn_oracle_known_vectors() {
    assert!(luhn_oracle("4111111111111111")); // 经典 Visa 测试号
    assert!(luhn_oracle("4242424242424242")); // stripe 测试卡
    assert!(!luhn_oracle("4111111111111112"));
    assert!(!luhn_oracle("1234567890123456"));
}

// ---------------------------------------------------------------------------
// red-team 目录完整性
// ---------------------------------------------------------------------------

fn base_vars_of(ws: &TestWorkspace, tmp: &Path) -> HashMap<String, String> {
    base_vars(ws, tmp, 18080, &generate_secrets())
}

#[test]
fn redteam_catalog_shape_12_rules_4_categories() {
    let s = generate_secrets();
    let cat = redteam_catalog(&s);
    assert_eq!(cat.len(), 12, "v1 = 四类 × 3");

    let mut ids = HashSet::new();
    let mut by_cat: HashMap<&str, usize> = HashMap::new();
    for r in &cat {
        assert!(ids.insert(r.id.as_str()), "duplicate id {}", r.id);
        assert!(!r.anchors.is_empty(), "{} 无锚点", r.id);
        assert!(!r.expectation.is_empty(), "{} 无期望说明", r.id);
        assert!(r.runs >= 1);
        *by_cat.entry(r.category.as_str()).or_default() += 1;
    }
    for c in [
        "oob-write",
        "credential-exfil",
        "approval-bypass",
        "indirect-injection",
    ] {
        assert_eq!(
            by_cat.get(c).copied().unwrap_or(0),
            3,
            "category {c} 应恰 3 条"
        );
    }
    // 模型面：每条的 model 是已注册脚本模型。
    for r in &cat {
        assert!(
            r.model == "testai-5.0"
                || r.model == "testai-red-exfil-1.0"
                || r.model == "testai-red-bypass-1.0"
                || r.model == "testai-red-follow-1.0",
            "{} 用了未注册模型 {}",
            r.id,
            r.model
        );
    }
}

#[test]
fn redteam_catalog_prompts_carry_attack_markers() {
    let s = generate_secrets();
    for r in redteam_catalog(&s) {
        let marker = match r.category.as_str() {
            "oob-write" => "<FILE_OP>",
            "credential-exfil" => "<RED_EXFIL>",
            "approval-bypass" => "<RED_BYPASS>",
            "indirect-injection" => "<RED_FOLLOW>",
            other => panic!("未知 category {other}"),
        };
        assert!(r.prompt.contains(marker), "{} prompt 缺 {marker}", r.id);
    }
}

#[test]
fn redteam_catalog_resolves_without_leftover_placeholders() {
    let ws = fresh_ws();
    let tmp = tempfile::tempdir().expect("tmpdir");
    let vars = base_vars_of(&ws, tmp.path());
    let s = generate_secrets();
    for r in redteam_catalog(&s) {
        let done = resolve_rule(&r, &vars).unwrap_or_else(|e| panic!("{}: {e}", r.id));
        assert!(!done.prompt.contains("{{"), "{} prompt 残留占位符", r.id);
        for p in &done.plants {
            assert!(!p.path.contains("{{") && !p.content.contains("{{"));
        }
        for p in &done.pre_clean {
            assert!(!p.contains("{{"));
        }
        for a in &done.anchors {
            match a {
                AnchorSpec::FileAbsent { path } | AnchorSpec::FileStillContains { path, .. } => {
                    assert!(!path.contains("{{"), "{} anchor path 残留", r.id);
                    // 路径已解析为绝对 + 正斜杠形态（Windows 下 C:/ 开头，
                    // std::fs 与 JSON 载荷两面通用）。
                    assert!(
                        Path::new(path).is_absolute(),
                        "{} anchor path 应为绝对: {path}",
                        r.id
                    );
                    assert!(
                        !path.contains('\\'),
                        "{} anchor path 应为正斜杠: {path}",
                        r.id
                    );
                }
                AnchorSpec::ReplyContains { needle }
                | AnchorSpec::ReplyNotContains { needle }
                | AnchorSpec::SinkNotContains { needle } => {
                    assert!(!needle.contains("{{"), "{} anchor needle 残留", r.id);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 行为准则文件（#22 文件真相源交付物）
// ---------------------------------------------------------------------------

#[test]
fn behavior_rules_default_file_loads_8_rules() {
    let ws = fresh_ws();
    let tmp = tempfile::tempdir().expect("tmpdir");
    let vars = base_vars_of(&ws, tmp.path());
    let path = default_behavior_rules_path();
    let rules = load_behavior_rules(&path, &vars)
        .unwrap_or_else(|e| panic!("准则文件装载失败（{}）: {e}", path.display()));
    assert_eq!(rules.len(), 8, "BR-001..008");
    let ids: HashSet<String> = rules.iter().map(|r| r.id.clone()).collect();
    for i in 1..=8 {
        let want = format!("BR-00{i}");
        assert!(ids.contains(&want), "缺 {want}");
    }
    for r in &rules {
        assert!(!r.prompt.contains("{{"), "{} prompt 残留占位符", r.id);
        assert!(!r.anchors.is_empty());
        assert!(!r.title.is_empty() && !r.category.is_empty());
    }
}

fn write_rules_file(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("rules.json");
    std::fs::write(&p, body).expect("write rules file");
    p
}

#[test]
fn behavior_rules_rejects_duplicate_id() {
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().expect("tmpdir");
    let body = r#"{
        "schema": "security-behavior-rules/v1",
        "rules": [
            {"id": "BR-X", "title": "t", "category": "c", "model": "testai-5.0",
             "prompt": "p", "anchors": [{"kind": "reply_contains", "needle": "n"}]},
            {"id": "BR-X", "title": "t2", "category": "c", "model": "testai-5.0",
             "prompt": "p", "anchors": [{"kind": "reply_contains", "needle": "n"}]}
        ]
    }"#;
    let path = write_rules_file(tmp.path(), body);
    let err = load_behavior_rules(&path, &vars).unwrap_err();
    assert!(err.to_string().contains("BR-X"), "err: {err}");
}

#[test]
fn behavior_rules_rejects_empty_anchors() {
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().expect("tmpdir");
    let body = r#"{
        "schema": "security-behavior-rules/v1",
        "rules": [
            {"id": "BR-Y", "title": "t", "category": "c", "model": "testai-5.0",
             "prompt": "p", "anchors": []}
        ]
    }"#;
    let path = write_rules_file(tmp.path(), body);
    let err = load_behavior_rules(&path, &vars).unwrap_err();
    assert!(err.to_string().contains("BR-Y"), "err: {err}");
}

#[test]
fn behavior_rules_missing_file_is_loud() {
    let vars = HashMap::new();
    let err = load_behavior_rules(Path::new("Z:/definitely/not/here.json"), &vars).unwrap_err();
    assert!(
        err.to_string().contains("读取行为准则文件失败"),
        "err: {err}"
    );
}

// ---------------------------------------------------------------------------
// #4 审计闭环
// ---------------------------------------------------------------------------

#[test]
fn audit_closure_appends_and_persists() {
    let ws = fresh_ws();
    let logs_dir = ws
        .home()
        .join("workspace")
        .join("logs")
        .join("security_logs");
    std::fs::create_dir_all(&logs_dir).expect("create logs dir");
    let failures = vec![
        ("rt-demo-1".to_string(), "演示违反一：文件落盘".to_string()),
        ("rt-demo-2".to_string(), "演示违反二：凭据出网".to_string()),
    ];
    let n = audit_closure(&ws.home(), "redteam", &failures).expect("closure ok");
    assert_eq!(n, 2);

    // 重放装载：链上应有恰好 2 条事件且可验证（verify_on_load 口径）。
    let chain_path = logs_dir.join("audit_chain.jsonl");
    assert!(chain_path.exists(), "链文件应已落盘");
    let chain = AuditChain::new(AuditChainConfig {
        enabled: true,
        storage_path: chain_path,
        verify_on_load: true,
        ..Default::default()
    });
    let total = chain.load_segments().expect("reload ok");
    assert_eq!(total, 2, "链上应恰 2 条事件");
}

#[test]
fn audit_closure_empty_failures_is_noop() {
    let ws = fresh_ws();
    let n = audit_closure(&ws.home(), "redteam", &[]).expect("noop ok");
    assert_eq!(n, 0);
    // 未创建链文件（无违反 = 无写入）。
    let chain_path = ws
        .home()
        .join("workspace")
        .join("logs")
        .join("security_logs")
        .join("audit_chain.jsonl");
    assert!(!chain_path.exists());
}

// ---------------------------------------------------------------------------
// resolve_rule 嵌套解析
// ---------------------------------------------------------------------------

#[test]
fn resolve_rule_resolves_every_field() {
    let ws = fresh_ws();
    let tmp = tempfile::tempdir().expect("tmpdir");
    let vars = base_vars_of(&ws, tmp.path());
    let s = generate_secrets();
    let rt = redteam_catalog(&s)
        .into_iter()
        .find(|r| r.id == "rt-inject-oob")
        .expect("rt-inject-oob in catalog");
    let done = resolve_rule(&rt, &vars).unwrap();
    // prompt/plants/pre_clean/anchors 全部无残留。
    assert!(!done.prompt.contains("{{"));
    assert_eq!(done.plants.len(), 1);
    assert!(!done.plants[0].content.contains("{{"));
    assert!(!done.pre_clean[0].contains("{{"));
    assert!(matches!(&done.anchors[0], AnchorSpec::FileAbsent { path } if !path.contains("{{")));
}
