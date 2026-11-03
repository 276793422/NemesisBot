//! L13-b mandate 模块测试（数据形态/签验全链/四拒路径/密钥面/前置闸）。

use std::path::PathBuf;

use super::*;
use crate::config_loader::{MandateConfig, MandateMode};
use crate::roaming::trust::{mandate_pin_forced, mandate_revoke};

/// 临时 workspace（trust.toml / mandate_key.json 隔离）。
fn temp_workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nb_l13b_mandate_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

/// 生成一对密钥（KeyFile 形态）。
fn make_key_file(tag: &str) -> MandateKeyFile {
    let sk = SigningKey::random(&mut OsRng);
    MandateKeyFile {
        private_key: hex::encode(sk.to_bytes().as_slice()),
        public_key: hex::encode(public_key_bytes(sk.verifying_key())),
        created_at: format!("test-{tag}"),
    }
}

fn default_body(task_id: &str, signed_at: u64) -> MandateBody {
    build_default_mandate(
        task_id,
        Some(vec!["exec".into(), "read_file".into()]),
        3600,
        100_000,
        50,
        signed_at,
    )
}

// ---------------------------------------------------------------------------
// 规范化 / 摘要 / roundtrip
// ---------------------------------------------------------------------------

#[test]
fn canonical_bytes_deterministic_and_order_stable() {
    let now = now_unix();
    let b1 = default_body("t-1", now);
    let b2 = default_body("t-1", now);
    assert_eq!(canonical_bytes(&b1).unwrap(), canonical_bytes(&b2).unwrap());
    // 声明序：v 打头、signed_at 收尾（防字段重排悄悄破坏 wire 契约）。
    let text = String::from_utf8(canonical_bytes(&b1).unwrap()).unwrap();
    assert!(text.starts_with(r#"{"v":1,"task_id":"t-1""#), "got: {text}");
    assert!(text.contains(r#","signed_at":"#), "got: {text}");
}

#[test]
fn envelope_roundtrip_preserves_canonical_bytes() {
    let now = now_unix();
    let body = default_body("t-1", now);
    let sk = signing_key_from_hex(&make_key_file("rt").private_key).unwrap();
    let env = sign_mandate(&body, &sk).unwrap();
    // wire 字节形态：sig 收尾（struct 序列化保声明序；serde_json::to_value
    // 产物是排序 BTreeMap，键序断言必须落在 to_string 字节上）。
    let wire = serde_json::to_string(&env).unwrap();
    assert!(
        wire.ends_with(&format!(r#""sig":"{}"}}"#, env.sig)),
        "sig 字段应收尾: {wire}"
    );
    // 反序列化后重规范化 == 原规范化字节（canonical roundtrip 前提）。
    let value = serde_json::to_value(&env).unwrap();
    let parsed = parse_envelope(&value).unwrap();
    assert_eq!(
        canonical_bytes(&parsed.body).unwrap(),
        canonical_bytes(&body).unwrap()
    );
    // null allowlist 保留 null（不限语义 roundtrip）。
    let mut b2 = default_body("t-1", now);
    b2.tool_allowlist = None;
    let v2 = serde_json::to_value(sign_mandate(&b2, &sk).unwrap()).unwrap();
    assert!(v2.get("tool_allowlist").unwrap().is_null());
}

#[test]
fn parse_envelope_rejects_malformed() {
    // 非对象
    assert!(matches!(
        parse_envelope(&serde_json::json!("not-an-object")),
        Err(MandateViolation::Malformed(_))
    ));
    // 缺字段（strict：不给缺省猜测，canonical roundtrip 前提）
    let now = now_unix();
    let kf = make_key_file("mf");
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &sk).unwrap();
    let mut v = serde_json::to_value(&env).unwrap();
    v.as_object_mut().unwrap().remove("budget");
    assert!(matches!(
        parse_envelope(&v),
        Err(MandateViolation::Malformed(_))
    ));
    // 未知版本
    let mut v = serde_json::to_value(&env).unwrap();
    v["v"] = serde_json::json!(2);
    assert!(matches!(
        parse_envelope(&v),
        Err(MandateViolation::Malformed(_))
    ));
    // sig 非 hex → verify 阶段 Malformed（parse_envelope 只查形态与版本）
    let mut v = serde_json::to_value(&env).unwrap();
    v["sig"] = serde_json::json!("zzzz");
    let parsed = parse_envelope(&v).unwrap();
    assert!(matches!(
        verify_mandate(&parsed, "t-1", now, &kf.public_key),
        Err(MandateViolation::Malformed(_))
    ));
}

// ---------------------------------------------------------------------------
// 签验全链（钉存 + 四拒）
// ---------------------------------------------------------------------------

#[test]
fn sign_verify_happy_path_full_chain() {
    let ws = temp_workspace("happy");
    let kf = make_key_file("happy");
    // A 端签名
    let now = now_unix();
    let body = default_body("board_dispatch:x1", now);
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&body, &sk).unwrap();
    let value = serde_json::to_value(&env).unwrap();
    // B 端钉存 + 全链验证
    mandate_pin_forced(&ws, "node-a", &kf.public_key).unwrap();
    let verified = verify_mandate_full(&ws, "node-a", &value, "board_dispatch:x1", now).unwrap();
    assert_eq!(verified.task_id, "board_dispatch:x1");
    assert_eq!(
        verified.tool_allowlist,
        Some(vec!["exec".into(), "read_file".into()])
    );
    assert_eq!(verified.time_window.expires_at, now + 3600);
    // not_before 带时钟容差回拨（B 端时钟落后不假拒）
    assert_eq!(
        verified.time_window.not_before,
        now.saturating_sub(CLOCK_SKEW_TOLERANCE_SECS)
    );
    // enforcement 投影
    let enf = enforcement_from_body(&verified);
    assert_eq!(enf.task_id, "board_dispatch:x1");
    assert_eq!(enf.expires_at, now + 3600);
    assert_eq!(enf.max_tokens, 100_000);
    assert_eq!(enf.max_tool_calls, 50);
    cleanup(&ws);
}

#[test]
fn tampered_body_rejected() {
    let ws = temp_workspace("tamper");
    let kf = make_key_file("tamper");
    let now = now_unix();
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &sk).unwrap();
    let mut v = serde_json::to_value(&env).unwrap();
    // 篡改 allowlist（扩权攻击形态）——canonical 摘要不匹配
    v["tool_allowlist"] = serde_json::json!(["exec", "read_file", "shell"]);
    mandate_pin_forced(&ws, "node-a", &kf.public_key).unwrap();
    assert_eq!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now),
        Err(MandateViolation::BadSignature)
    );
    cleanup(&ws);
}

#[test]
fn forged_signature_rejected() {
    let ws = temp_workspace("forge");
    // 攻击者用自己的密钥签，但钉存的是 A 的公钥
    let attacker = make_key_file("forge-attacker");
    let victim = make_key_file("forge-victim");
    let now = now_unix();
    let atk_sk = signing_key_from_hex(&attacker.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &atk_sk).unwrap();
    let v = serde_json::to_value(&env).unwrap();
    mandate_pin_forced(&ws, "node-a", &victim.public_key).unwrap();
    assert_eq!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now),
        Err(MandateViolation::BadSignature)
    );
    cleanup(&ws);
}

#[test]
fn task_id_mismatch_rejected() {
    let ws = temp_workspace("bind");
    let kf = make_key_file("bind");
    let now = now_unix();
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    // mandate 签给任务 1，贴到任务 2（移植攻击）
    let env = sign_mandate(&default_body("task-1", now), &sk).unwrap();
    let v = serde_json::to_value(&env).unwrap();
    mandate_pin_forced(&ws, "node-a", &kf.public_key).unwrap();
    assert_eq!(
        verify_mandate_full(&ws, "node-a", &v, "task-2", now),
        Err(MandateViolation::TaskIdMismatch)
    );
    cleanup(&ws);
}

#[test]
fn expired_and_not_yet_valid_rejected() {
    let ws = temp_workspace("time");
    let kf = make_key_file("time");
    let now = now_unix();
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &sk).unwrap();
    let v = serde_json::to_value(&env).unwrap();
    mandate_pin_forced(&ws, "node-a", &kf.public_key).unwrap();
    // 过期重放（expires_at = signed_at + 3600；+3700s 后验）
    assert!(matches!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now + 3700),
        Err(MandateViolation::Expired { .. })
    ));
    // 未生效（not_before = signed_at - 60s 容差；提前 61s 验仍落在窗外）
    assert!(matches!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now.saturating_sub(61)),
        Err(MandateViolation::NotYetValid { .. })
    ));
    // 容差窗内（落后 30s）不假拒——e2e 双机时钟差实测形态
    assert!(verify_mandate_full(&ws, "node-a", &v, "t-1", now.saturating_sub(30)).is_ok());
    cleanup(&ws);
}

#[test]
fn revoked_key_rejected_before_signature() {
    let ws = temp_workspace("revoke");
    let kf = make_key_file("revoke");
    let now = now_unix();
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &sk).unwrap();
    let v = serde_json::to_value(&env).unwrap();
    mandate_pin_forced(&ws, "node-a", &kf.public_key).unwrap();
    mandate_revoke(&ws, "node-a").unwrap();
    // 吊销一票否决（优先于验签——即使签名合法也拒）
    assert_eq!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now),
        Err(MandateViolation::Revoked)
    );
    cleanup(&ws);
}

#[test]
fn no_pinned_key_rejected() {
    let ws = temp_workspace("nopin");
    let kf = make_key_file("nopin");
    let now = now_unix();
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &sk).unwrap();
    let v = serde_json::to_value(&env).unwrap();
    // TOFU 纪律：任务时到达 mandate 但无钉存钥 = 拒（不自动钉）
    assert_eq!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now),
        Err(MandateViolation::NoPinnedKey)
    );
    cleanup(&ws);
}

#[test]
fn key_substitution_attack_structurally_immune() {
    let ws = temp_workspace("subst");
    let victim = make_key_file("subst-victim");
    let attacker = make_key_file("subst-attacker");
    let now = now_unix();
    // 攻击者用自己的钥签 mandate，并在信封里塞自己的公钥（密钥替换攻击
    // 形态）——信封无公钥槽，验证只读本地钉存钥（victim）
    let atk_sk = signing_key_from_hex(&attacker.private_key).unwrap();
    let env = sign_mandate(&default_body("t-1", now), &atk_sk).unwrap();
    let mut v = serde_json::to_value(&env).unwrap();
    v["pubkey"] = serde_json::json!(attacker.public_key);
    mandate_pin_forced(&ws, "node-a", &victim.public_key).unwrap();
    // 信封公钥被无视：按 victim 钉存钥验签 → BadSignature（非攻击者钥）
    assert_eq!(
        verify_mandate_full(&ws, "node-a", &v, "t-1", now),
        Err(MandateViolation::BadSignature)
    );
    cleanup(&ws);
}

// ---------------------------------------------------------------------------
// 密钥面（load-or-generate）
// ---------------------------------------------------------------------------

#[test]
fn load_or_generate_creates_and_reuses() {
    let ws = temp_workspace("keygen");
    let kf1 = load_or_generate_mandate_key(&ws).unwrap();
    let kf2 = load_or_generate_mandate_key(&ws).unwrap();
    assert_eq!(kf1.private_key, kf2.private_key, "复用同钥");
    assert!(mandate_key_path(&ws).exists());
    // 形态钉子：私钥 64 hex / 公钥 130 hex 04 头
    assert_eq!(kf1.private_key.len(), 64);
    assert_eq!(kf1.public_key.len(), 130);
    assert!(kf1.public_key.starts_with("04"));
    cleanup(&ws);
}

#[test]
fn load_or_generate_rejects_corrupt_and_mismatched() {
    let ws = temp_workspace("keycorrupt");
    let path = mandate_key_path(&ws);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // 损坏 JSON → Err
    std::fs::write(&path, "not json").unwrap();
    assert!(load_or_generate_mandate_key(&ws).is_err());
    // 公私不一致（手改残留形态）→ Err
    let kf = make_key_file("mismatch");
    let other = make_key_file("mismatch-other");
    let bad = MandateKeyFile {
        private_key: kf.private_key.clone(),
        public_key: other.public_key.clone(),
        created_at: kf.created_at.clone(),
    };
    std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
    let err = load_or_generate_mandate_key(&ws).unwrap_err();
    assert!(err.contains("公钥与私钥派生不一致"), "got: {err}");
    cleanup(&ws);
}

// ---------------------------------------------------------------------------
// A 端签名器
// ---------------------------------------------------------------------------

#[test]
fn mandate_signer_signs_per_task() {
    let kf = make_key_file("signer");
    let signer =
        MandateSigner::from_key_file(&kf, Some(vec!["exec".into()]), 600, 5000, 10).unwrap();
    let m1 = signer.sign_for_task("chat-abc").unwrap();
    let m2 = signer.sign_for_task("board_dispatch:x2").unwrap();
    assert_ne!(
        m1.get("task_id").unwrap(),
        m2.get("task_id").unwrap(),
        "逐任务绑定"
    );
    // 自己钉存自己可验（A 端自验闭环）
    let ws = temp_workspace("signer-verify");
    mandate_pin_forced(&ws, "self", &kf.public_key).unwrap();
    let body = verify_mandate_full(&ws, "self", &m1, "chat-abc", now_unix()).unwrap();
    assert_eq!(body.tool_allowlist, Some(vec!["exec".into()]));
    assert_eq!(body.budget.max_tokens, 5000);
    assert_eq!(
        body.time_window.expires_at - body.time_window.not_before,
        600 + CLOCK_SKEW_TOLERANCE_SECS,
        "窗口 = window_secs + 容差回拨量"
    );
    cleanup(&ws);
}

#[test]
fn mandate_signer_key_fp_and_pubkey() {
    let kf = make_key_file("signer-fp");
    let signer = MandateSigner::from_key_file(&kf, None, 3600, 0, 0).unwrap();
    assert_eq!(signer.public_key, kf.public_key);
    assert_eq!(signer.key_fp.len(), 64);
}

// ---------------------------------------------------------------------------
// B 端前置闸（MandatePolicy）
// ---------------------------------------------------------------------------

fn policy_for(ws: &Path, mode: MandateMode) -> MandatePolicy {
    MandatePolicy {
        workspace: ws.to_path_buf(),
        mode,
    }
}

fn signed_payload(kf: &MandateKeyFile, task_id: &str, signed_at: u64) -> serde_json::Value {
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let env = sign_mandate(&default_body(task_id, signed_at), &sk).unwrap();
    let mut payload = serde_json::json!({
        "content": "do work",
        "task_id": task_id,
    });
    payload["mandate"] = serde_json::to_value(&env).unwrap();
    payload
}

#[test]
fn policy_off_ignores_even_garbage_mandate() {
    let ws = temp_workspace("policy-off");
    let policy = policy_for(&ws, MandateMode::Off);
    let payload = serde_json::json!({ "content": "x", "mandate": "garbage" });
    assert!(
        policy
            .check("node-a", &payload, "t-1", now_unix())
            .unwrap()
            .is_none()
    );
    cleanup(&ws);
}

#[test]
fn policy_verify_missing_mandate_passes_old_behavior() {
    let ws = temp_workspace("policy-verify-miss");
    let policy = policy_for(&ws, MandateMode::Verify);
    let payload = serde_json::json!({ "content": "x" });
    assert!(
        policy
            .check("node-a", &payload, "t-1", now_unix())
            .unwrap()
            .is_none()
    );
    // mandate: null 同缺失语义
    let payload = serde_json::json!({ "content": "x", "mandate": null });
    assert!(
        policy
            .check("node-a", &payload, "t-1", now_unix())
            .unwrap()
            .is_none()
    );
    cleanup(&ws);
}

#[test]
fn policy_require_missing_mandate_rejected() {
    let ws = temp_workspace("policy-req-miss");
    let policy = policy_for(&ws, MandateMode::Require);
    let payload = serde_json::json!({ "content": "x" });
    let err = policy
        .check("node-a", &payload, "t-1", now_unix())
        .unwrap_err();
    assert!(matches!(err, MandateViolation::Malformed(ref d) if d.contains("缺失")));
    cleanup(&ws);
}

#[test]
fn policy_verify_validates_present_mandate() {
    let ws = temp_workspace("policy-verify-present");
    let kf = make_key_file("pvp");
    mandate_pin_forced(&ws, "node-a", &kf.public_key).unwrap();
    let policy = policy_for(&ws, MandateMode::Verify);
    let now = now_unix();
    let payload = signed_payload(&kf, "t-9", now);
    let enf = policy
        .check("node-a", &payload, "t-9", now)
        .unwrap()
        .unwrap();
    assert_eq!(enf.task_id, "t-9");
    // 篡改 → 拒
    let mut bad = signed_payload(&kf, "t-9", now);
    bad["content"] = serde_json::json!("do MORE work"); // mandate 只签 body，content 不在内
    let _ = policy.check("node-a", &bad, "t-9", now).unwrap(); // content 不在签名面——payload 内容不受 mandate 保护
    // 但 mandate body 本身被篡改必拒
    let mut bad2 = signed_payload(&kf, "t-9", now);
    bad2["mandate"]["budget"]["max_tokens"] = serde_json::json!(999999);
    assert!(policy.check("node-a", &bad2, "t-9", now).is_err());
    cleanup(&ws);
}

#[test]
fn policy_unpinned_node_rejected_when_mandate_present() {
    let ws = temp_workspace("policy-unpinned");
    let kf = make_key_file("pup");
    // 钉存的是别人的钥（TOFU 首见污染形态）
    let other = make_key_file("pup-other");
    mandate_pin_forced(&ws, "node-a", &other.public_key).unwrap();
    let policy = policy_for(&ws, MandateMode::Verify);
    let now = now_unix();
    let payload = signed_payload(&kf, "t-1", now);
    assert!(policy.check("node-a", &payload, "t-1", now).is_err());
    cleanup(&ws);
}

// ---------------------------------------------------------------------------
// 配置段
// ---------------------------------------------------------------------------

#[test]
fn mandate_config_defaults_and_resolution() {
    let cfg = MandateConfig::default();
    assert_eq!(cfg.mode, "verify");
    assert!(cfg.sign_enabled);
    assert!(cfg.tool_allowlist.is_empty());
    assert_eq!(cfg.window_secs, 7200);
    assert_eq!(cfg.max_tokens, 0);
    assert_eq!(cfg.max_tool_calls, 0);
    assert_eq!(cfg.resolve_mode(), MandateMode::Verify);
    assert_eq!(cfg.allowlist_or_unlimited(), None, "空数组 = 不限");
    // 词表解析
    let mut c = cfg.clone();
    c.mode = "require".into();
    assert_eq!(c.resolve_mode(), MandateMode::Require);
    c.mode = "off".into();
    assert_eq!(c.resolve_mode(), MandateMode::Off);
    c.mode = "whatever".into();
    assert_eq!(
        c.resolve_mode(),
        MandateMode::Verify,
        "未知值保守回落 verify"
    );
    // 显式白名单投影
    c.tool_allowlist = vec!["exec".into()];
    assert_eq!(c.allowlist_or_unlimited(), Some(vec!["exec".into()]));
}

#[test]
fn app_config_roundtrip_preserves_mandate_section() {
    let json = r#"{"enabled":true,"port":13000,"rpc_port":23000,
        "mandate":{"mode":"require","sign_enabled":false,"tool_allowlist":["exec"],
        "window_secs":600,"max_tokens":1000,"max_tool_calls":5}}"#;
    let cfg: crate::config_loader::AppConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.mandate.mode, "require");
    assert!(!cfg.mandate.sign_enabled);
    assert_eq!(cfg.mandate.tool_allowlist, vec!["exec"]);
    assert_eq!(cfg.mandate.window_secs, 600);
    assert_eq!(cfg.mandate.max_tokens, 1000);
    assert_eq!(cfg.mandate.max_tool_calls, 5);
    // 旧形态（无 mandate 键）零迁移
    let old: crate::config_loader::AppConfig =
        serde_json::from_str(r#"{"enabled":true,"port":13000}"#).unwrap();
    assert_eq!(old.mandate, MandateConfig::default());
    // roundtrip 保留
    let re = serde_json::to_string(&cfg).unwrap();
    assert!(re.contains(r#""mandate""#));
}

// ---------------------------------------------------------------------------
// parity（digest 与 v4_content_digest 字节等价；dev-dep 直引真函数钉死）
// ---------------------------------------------------------------------------

#[test]
fn digest_parity_with_v4_content_digest() {
    // L13-b 规格「A 端 v4_content_digest+签名」语义单源不漂移：mandate
    // 摘要与 v4 非 PE（raw codec 全文件哈希）内容摘要**字节等价**。
    let canonical = canonical_bytes(&default_body("t-1", 12345)).unwrap();
    assert_eq!(
        mandate_digest(&canonical),
        nemesis_verify::verify::v4_content_digest(&canonical).unwrap(),
        "mandate 摘要 vs v4_content_digest（canonical mandate body）"
    );
    // 任意字节同样等价（raw codec 路径 = SHA-256 全量，无格式分支）
    let junk = b"arbitrary bytes for digest parity \xf0\x9f\x8c\x80";
    assert_eq!(
        mandate_digest(junk),
        nemesis_verify::verify::v4_content_digest(junk).unwrap(),
        "mandate 摘要 vs v4_content_digest（任意字节）"
    );
}
