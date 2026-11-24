//! vault_domain 测试：load_at 四态 / evaluate 判定矩阵 / slot 层级匹配 /
//! 优先级（OutOfDomain > NoBindings > AllInDomain 反推）。

use super::*;
use std::path::PathBuf;

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "vault-domain-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// refs 构造便捷（字段路径, 别名）。
fn r(field: &str, alias: &str) -> (String, String) {
    (field.to_string(), alias.to_string())
}

// load_at 四态 -----------------------------------------------------------

#[test]
fn load_at_absent_file_returns_none() {
    let dir = tempdir();
    let cfg = VaultDomainConfig::load_at(&dir.join("config.vault.json")).unwrap();
    assert!(cfg.is_none(), "文件缺席 = 无绑定");
}

#[test]
fn load_at_parses_valid_config() {
    let dir = tempdir();
    let path = dir.join("config.vault.json");
    std::fs::write(
        &path,
        r#"{"bindings":{"corp-key":[{"tool":"web_fetch","slot":"headers"}]}}"#,
    )
    .unwrap();
    let cfg = VaultDomainConfig::load_at(&path).unwrap().unwrap();
    assert_eq!(cfg.bindings["corp-key"][0].tool, "web_fetch");
    assert_eq!(cfg.bindings["corp-key"][0].slot, "headers");
}

#[test]
fn load_at_rejects_malformed() {
    let dir = tempdir();
    let path = dir.join("config.vault.json");
    std::fs::write(&path, "not json at all {{{").unwrap();
    assert!(
        VaultDomainConfig::load_at(&path).is_err(),
        "损坏必须 Err（fail-secure 素材）"
    );
    // 类型错（bindings 不是映射）同面。
    std::fs::write(&path, r#"{"bindings": [1,2]}"#).unwrap();
    assert!(VaultDomainConfig::load_at(&path).is_err());
}

#[test]
fn load_at_empty_object_yields_empty_bindings() {
    let dir = tempdir();
    let path = dir.join("config.vault.json");
    std::fs::write(&path, "{}").unwrap();
    let cfg = VaultDomainConfig::load_at(&path).unwrap().unwrap();
    assert!(cfg.bindings.is_empty());
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers", "any")]),
        DomainVerdict::NoBindings
    );
}

// BUG-20261125-008：`[]` 文档曾从 serde visit_seq 零元素臂溜过 = 静默
// Ok(Some(空))——绕开文档化「损坏 → Err → 调用方 WARN + fail-secure」路径
// （配置的免卡松弛全部静默失效，零诊断）。ensure_object 前置后数组/标量
// 文档 loud Err；空 object `{}` 仍合法（上一测试钉住，不误伤）。
#[test]
fn load_at_rejects_array_document_loud() {
    let dir = tempdir();
    let path = dir.join("config.vault.json");
    std::fs::write(&path, "[]").unwrap();
    let err = VaultDomainConfig::load_at(&path).expect_err("`[]` 必须 loud Err（fail-secure）");
    assert!(err.contains("must be a JSON object"), "err={err}");
    assert!(err.contains("array"), "形状必须可见：err={err}");
    // 标量文档同闸。
    std::fs::write(&path, "42").unwrap();
    let err = VaultDomainConfig::load_at(&path).expect_err("标量必须 loud Err");
    assert!(err.contains("must be a JSON object"), "err={err}");
}

// BUG-20261120-005：Windows 记事本 utf8BOM 保存形态（用户可手编资产 +
// 记事本是 Windows 现实编辑器）。裸 read_to_string 不剥 BOM → serde_json
// 解析失败 → Err → dispatch 闸调用方按无绑定兜底 → 用户配置的域绑定全部
// 失效（免卡直过退化 v1 审批卡 + 持续 WARN）。同 crate 同款 BUG 已修过
// 两处（plugin_wasm runtime_enabled、eval_rules load_rules
// BUG-20261109-036），本位点漏网。回归锁：BOM 前缀文件照常解析。
#[test]
fn load_at_tolerates_utf8_bom() {
    let dir = tempdir();
    let path = dir.join("config.vault.json");
    let body = r#"{"bindings":{"corp-key":[{"tool":"web_fetch","slot":"headers"}]}}"#;
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend_from_slice(body.as_bytes());
    std::fs::write(&path, &with_bom).unwrap();
    let cfg = VaultDomainConfig::load_at(&path)
        .expect("utf8BOM 保存形态必须照常解析（记事本手编兼容），不得 Err 使绑定全部失效")
        .expect("配置应在场");
    assert_eq!(cfg.bindings["corp-key"][0].tool, "web_fetch");
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers", "corp-key")]),
        DomainVerdict::AllInDomain,
        "BOM 文件的绑定必须真实生效"
    );
}

#[test]
fn path_helper_joins_config_dir() {
    let p = vault_domain_config_path_in(Path::new("/ws/config"));
    assert_eq!(p, PathBuf::from("/ws/config").join("config.vault.json"));
}

// evaluate 判定矩阵 ------------------------------------------------------

fn sample_cfg() -> VaultDomainConfig {
    let mut b = BTreeMap::new();
    b.insert(
        "corp-key".to_string(),
        vec![DomainBinding {
            tool: "web_fetch".to_string(),
            slot: "headers".to_string(),
        }],
    );
    b.insert(
        "leaf-key".to_string(),
        vec![DomainBinding {
            tool: "web_fetch".to_string(),
            slot: "headers.Authorization".to_string(),
        }],
    );
    VaultDomainConfig { bindings: b }
}

#[test]
fn evaluate_no_bindings_when_refs_unbound() {
    let cfg = sample_cfg();
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers", "other-alias")]),
        DomainVerdict::NoBindings
    );
    assert_eq!(
        cfg.evaluate("web_fetch", &[]),
        DomainVerdict::NoBindings,
        "空 refs 直落"
    );
}

#[test]
fn evaluate_all_in_domain_top_slot_covers_leaves() {
    let cfg = sample_cfg();
    // 顶槽绑定盖本体与任意叶。
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers", "corp-key")]),
        DomainVerdict::AllInDomain
    );
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers.Authorization", "corp-key")]),
        DomainVerdict::AllInDomain
    );
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers.X-Custom", "corp-key")]),
        DomainVerdict::AllInDomain
    );
}

#[test]
fn evaluate_exact_leaf_binding_is_narrow() {
    let cfg = sample_cfg();
    // 精确叶绑定只覆盖该叶；同槽其它叶出域。
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers.Authorization", "leaf-key")]),
        DomainVerdict::AllInDomain
    );
    let v = cfg
        .evaluate("web_fetch", &[r("headers.X-Custom", "leaf-key")])
        .expect_out_of_domain();
    assert_eq!(v[0].alias, "leaf-key");
    assert_eq!(v[0].field, "headers.X-Custom");
    assert_eq!(
        v[0].allowed,
        vec!["web_fetch:headers.Authorization".to_string()]
    );
}

#[test]
fn evaluate_out_of_domain_on_tool_mismatch() {
    let cfg = sample_cfg();
    let v = cfg
        .evaluate("exec", &[r("headers", "corp-key")])
        .expect_out_of_domain();
    assert_eq!(v.len(), 1);
}

#[test]
fn evaluate_priority_out_of_domain_over_unbound() {
    let cfg = sample_cfg();
    // 出域 + 无绑定混合：硬策略优先（OutOfDomain），不落卡路径。
    let mixed = vec![r("headers", "corp-key"), r("headers", "ghost-alias")];
    // corp-key 域内 + ghost 无绑定 → NoBindings（卡路径罩全调用）。
    assert_eq!(cfg.evaluate("web_fetch", &mixed), DomainVerdict::NoBindings);
    // corp-key 换出域槽（精确叶键只认 Authorization，用 tool 错配制造出域）：
    let mixed_bad = vec![r("params.headers", "corp-key"), r("headers", "ghost-alias")];
    let v = cfg.evaluate("web_fetch", &mixed_bad).expect_out_of_domain();
    assert_eq!(v.len(), 1, "只报出域条目（corp-key 的 params.headers）");
    assert_eq!(v[0].alias, "corp-key");
}

#[test]
fn evaluate_empty_binding_entries_treated_unbound() {
    let mut b = BTreeMap::new();
    b.insert("hollow".to_string(), vec![]);
    let cfg = VaultDomainConfig { bindings: b };
    assert_eq!(
        cfg.evaluate("web_fetch", &[r("headers", "hollow")]),
        DomainVerdict::NoBindings,
        "空条目数组 = 无有效域（配置面手编容错，不误判出域拒绝）"
    );
}

/// DomainVerdict 便捷解构（测试可读性）。
trait ExpectOutOfDomain {
    fn expect_out_of_domain(&self) -> Vec<DomainViolation>;
}
impl ExpectOutOfDomain for DomainVerdict {
    fn expect_out_of_domain(&self) -> Vec<DomainViolation> {
        match self {
            DomainVerdict::OutOfDomain(v) => v.clone(),
            other => panic!("expected OutOfDomain, got {other:?}"),
        }
    }
}
