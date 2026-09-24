//! exe-sign-tool 单测：分包拆分 / 铸叶 / 锚定等价性 / 诚实拒绝 / 正负例闭环。
//!
//! 全部用临时目录 + 现生成密钥（不碰任何真实密钥材料；CI Secrets 形态用空串
//! 字段模拟，Secrets 值从不出现在测试里）。

use super::*;
use nemesis_verify::bundle::{self, BundleKind, IssuingMaterial, RootMaterial, SigningMaterial};
use nemesis_verify::cert::Certificate;
use nemesis_verify::keygen::{KeyHierarchy, KeyHierarchyJson};
use nemesis_verify::verify::{VerifyOutcome, sign_content_v4, verify_bytes};

/// 生成全量包落盘，返回（内存句柄, 文件路径）。
fn gen_full(dir: &tempfile::TempDir) -> (KeyHierarchy, String) {
    let h = nemesis_verify::keygen::generate().unwrap();
    let p = dir.path().join("keys.json").to_str().unwrap().to_string();
    h.save(&p).unwrap();
    (h, p)
}

fn path_in(dir: &tempfile::TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_string()
}

#[test]
fn split_keys_roundtrip_and_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let (h, full_path) = gen_full(&dir);
    let root_p = path_in(&dir, "root.offline.json");
    let issuing_p = path_in(&dir, "issuing.ci.json");
    let cert_p = path_in(&dir, "root_cert.der");

    cmd_split_keys(&full_path, &root_p, &issuing_p, Some(&cert_p)).unwrap();

    // 根材料严格形态回载 + 锚一致
    let root_mat = RootMaterial::load(&root_p).unwrap();
    assert_eq!(root_mat.root_anchor(), h.root_anchor_fingerprint());
    // 中间 CA 材料回载 + 锚一致
    let iss_mat = IssuingMaterial::load(&issuing_p).unwrap();
    assert_eq!(iss_mat.root_anchor(), h.root_anchor_fingerprint());
    // --root-cert-out 产出的 DER 锚一致
    let c = Certificate::from_der(&std::fs::read(&cert_p).unwrap()).unwrap();
    assert_eq!(c.sha256_fingerprint(), h.root_anchor_fingerprint());
    // 形态分类正确
    let rj = KeyHierarchyJson::load(&root_p).unwrap();
    assert_eq!(rj.bundle_kind().unwrap(), BundleKind::RootOnly);
    let ij = KeyHierarchyJson::load(&issuing_p).unwrap();
    assert_eq!(ij.bundle_kind().unwrap(), BundleKind::IssuingOnly);
    // 拆出的两份密钥与原包一致
    assert_eq!(root_mat.root_sk.to_bytes(), h.root_sk.to_bytes());
    assert_eq!(iss_mat.issuing_sk.to_bytes(), h.issuing_sk.to_bytes());
}

#[test]
fn mint_leaf_sign_only_signs_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = gen_full(&dir);
    let issuing_p = path_in(&dir, "issuing.ci.json");
    let (_, iss_mat) = bundle::split_keys(&h, now_secs()).unwrap();
    iss_mat.save(&issuing_p).unwrap();

    // CI 链路：IssuingOnly 材料装载 → 铸叶 → sign-only 包
    let mat = bundle::mint_leaf(
        &IssuingMaterial::load(&issuing_p).unwrap(),
        365,
        "NemesisBot CI test",
        now_secs(),
    )
    .unwrap();
    assert_eq!(mat.root_anchor(), h.root_anchor_fingerprint());
    let minted_p = path_in(&dir, "ci-keys.json");
    mat.save(&minted_p).unwrap();

    // sign-only 包形态分类
    let mj = KeyHierarchyJson::load(&minted_p).unwrap();
    assert_eq!(mj.bundle_kind().unwrap(), BundleKind::SignOnly);

    // serde 缺省字段：删掉 root_sk 字段的 json 仍可反序列化（空串补位）
    let raw = std::fs::read_to_string(&minted_p).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    v.as_object_mut().unwrap().remove("root_sk");
    let j2: KeyHierarchyJson = serde_json::from_value(v).unwrap();
    assert!(j2.root_sk.is_empty());

    // sign-only 包装载 → sign → 同锚 verify = Valid
    let sign_mat = SigningMaterial::load(&minted_p).unwrap();
    let signed = sign_content_v4(
        b"payload",
        &sign_mat.leaf_sk,
        now_secs(),
        &sign_mat.chain(),
        None,
    )
    .unwrap();
    let signed_p = path_in(&dir, "app.signed.bin");
    std::fs::write(&signed_p, &signed).unwrap();
    let outcome = verify_bytes(
        &std::fs::read(&signed_p).unwrap(),
        &[h.root_anchor_fingerprint()],
        now_secs(),
    );
    assert!(
        matches!(outcome, VerifyOutcome::Valid { .. }),
        "{outcome:?}"
    );

    // 全量装载对 sign-only 包诚实拒绝（分包不能冒充全量——防占位误用）
    assert!(KeyHierarchy::from_json(&mj).is_err());
}

#[test]
fn verify_anchor_equivalence_and_exclusivity() {
    let dir = tempfile::tempdir().unwrap();
    let (h, full_path) = gen_full(&dir);
    let cert_p = path_in(&dir, "root_cert.der");
    std::fs::write(&cert_p, h.root_cert.to_der()).unwrap();

    // --keys 与 --root-cert 同锚同结论
    let a1 = resolve_verify_anchor(Some(&full_path), None).unwrap();
    let a2 = resolve_verify_anchor(None, Some(&cert_p)).unwrap();
    assert_eq!(a1, a2);
    assert_eq!(a1, h.root_anchor_fingerprint());

    // 互斥：同时给 / 都不给 → 诚实拒绝
    assert!(resolve_verify_anchor(Some(&full_path), Some(&cert_p)).is_err());
    assert!(resolve_verify_anchor(None, None).is_err());
}

#[test]
fn sign_verify_positive_and_negative_cases() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = gen_full(&dir);
    let mat = SigningMaterial::from_json(&h.to_json()).unwrap();
    let signed =
        sign_content_v4(b"hello v4", &mat.leaf_sk, now_secs(), &mat.chain(), None).unwrap();

    // 正例：原锚 Valid
    assert!(matches!(
        verify_bytes(&signed, &[h.root_anchor_fingerprint()], now_secs()),
        VerifyOutcome::Valid { .. }
    ));
    // 负例1：篡改内容区一字节 → Tampered（内容仅 8B，翻内容区而非 len/2——
    // 后者是 CMS footer 区，翻那里报 SignatureInvalid/Malformed 不是 Tampered）
    let mut bad = signed.clone();
    bad[2] ^= 0xFF;
    assert!(
        matches!(
            verify_bytes(&bad, &[h.root_anchor_fingerprint()], now_secs()),
            VerifyOutcome::Tampered(_)
        ),
        "内容区篡改应报 Tampered"
    );
    // 负例2：换一根 → Untrusted
    let other = nemesis_verify::keygen::generate().unwrap();
    assert!(matches!(
        verify_bytes(&signed, &[other.root_anchor_fingerprint()], now_secs()),
        VerifyOutcome::Untrusted
    ));
    // 负例3：未签名材料 → NoSignature
    assert!(matches!(
        verify_bytes(
            b"unsigned bytes",
            &[h.root_anchor_fingerprint()],
            now_secs()
        ),
        VerifyOutcome::NoSignature
    ));
}

#[test]
fn root_material_strict_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = gen_full(&dir);
    let (root_mat, _) = bundle::split_keys(&h, now_secs()).unwrap();

    // 纯 RootOnly 形态 OK
    assert_eq!(
        root_mat.to_json().bundle_kind().unwrap(),
        BundleKind::RootOnly
    );
    RootMaterial::from_json(&root_mat.to_json()).unwrap();

    // 混入多余私钥字段 → 诚实拒绝（根材料保管纪律：包内零多余密钥面）
    let mut j = root_mat.to_json();
    j.leaf_sk = hex_encode(&[0u8; 32]);
    assert!(RootMaterial::from_json(&j).is_err());

    // 签名材料不是根材料
    let mat = SigningMaterial::from_json(&h.to_json()).unwrap();
    assert!(RootMaterial::from_json(&mat.to_json()).is_err());
}

#[test]
fn issuing_material_from_ci_secret_shape() {
    // ci-sign-artifacts.sh 组装形态：root_sk 空 + issuing 真（Secrets 值）+ leaf 空
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = gen_full(&dir);
    let mut j = h.to_json();
    j.root_sk = String::new();
    j.leaf_sk = String::new();
    j.leaf_cert = String::new();
    assert_eq!(j.bundle_kind().unwrap(), BundleKind::IssuingOnly);

    let iss = IssuingMaterial::from_json(&j).unwrap();
    assert_eq!(iss.root_anchor(), h.root_anchor_fingerprint());
    // Secrets → mint → sign 全链即刻可用
    let mat = bundle::mint_leaf(&iss, 365, "CI", now_secs()).unwrap();
    assert_eq!(mat.root_anchor(), h.root_anchor_fingerprint());
}

#[test]
fn full_consistency_checks() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _) = gen_full(&dir);
    // 生成即自检通过
    bundle::validate_full_consistency(&h, now_secs()).unwrap();
    bundle::validate_signing_consistency(&h, now_secs()).unwrap();

    // 换掉 leaf 私钥 → 匹配失败诚实拒绝
    let rogue = nemesis_verify::crypto::signing_key_from_hex(
        &nemesis_verify::crypto::generate_key_pair().private_key,
    )
    .unwrap();
    let bad = KeyHierarchy {
        leaf_sk: rogue,
        ..h
    };
    assert!(bundle::validate_full_consistency(&bad, now_secs()).is_err());
    assert!(bundle::validate_signing_consistency(&bad, now_secs()).is_err());
}
