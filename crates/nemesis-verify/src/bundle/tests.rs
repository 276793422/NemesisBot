//! bundle 覆盖率批次（2026-09-24，AGT）：分包形态分类 / 三类材料（sign / issuing /
//! root）装载-校验-序列化全臂 / 一致性校验 / split / mint-leaf / 锚提取。
//!
//! 夹具 = `keygen::generate_at`（确定性三级体系，可注时钟）。JSON 无 Clone derive，
//! 用 serde 往返复制（`dup`）。校验失败臂全部走 `err_of` + 消息关键词断言
//! （错误文案是 crate 契约面，供 CLI 呈现）。

use super::*;
use crate::GLOBAL_STATE_LOCK as TEST_LOCK;
use crate::cert::{TbsInput, ski_value};
use crate::hex_util::hex_encode;
use crate::keygen::{KEYS_JSON_VERSION, KeyHierarchy, generate_at};
use crate::verify::{VerifyOutcome, sign_content_v4, verify_bytes};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("post-1970 clock")
        .as_secs()
}

/// `unwrap_err` 要求 Ok 侧 `Debug`（SigningMaterial 等材料结构未实现），
/// 用泛型 helper 取 Err，不给生产类型加 derive。
fn err_of<T, E>(r: Result<T, E>) -> E {
    match r {
        Err(e) => e,
        Ok(_) => panic!("expected Err"),
    }
}

/// 确定性三级体系 + 其 JSON 形态。
fn fresh() -> (KeyHierarchy, KeyHierarchyJson) {
    let h = generate_at(now()).expect("generate_at");
    let j = h.to_json();
    (h, j)
}

/// KeyHierarchyJson 无 Clone——serde 往返复制。
fn dup(j: &KeyHierarchyJson) -> KeyHierarchyJson {
    serde_json::from_str(&serde_json::to_string(j).expect("serialize")).expect("deserialize")
}

fn write_json(dir: &std::path::Path, name: &str, j: &KeyHierarchyJson) -> String {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_vec_pretty(j).unwrap()).unwrap();
    p.to_string_lossy().to_string()
}

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ===== BundleKind：四合法形态 + 全部非法组合 =====

#[test]
fn bundle_kind_four_legal_shapes() {
    let (_, j) = fresh();
    assert_eq!(j.bundle_kind().unwrap(), BundleKind::Full);

    let mut s = dup(&j);
    s.root_sk.clear();
    s.issuing_sk.clear();
    assert_eq!(s.bundle_kind().unwrap(), BundleKind::SignOnly);

    let mut i = dup(&j);
    i.root_sk.clear();
    i.leaf_sk.clear();
    assert_eq!(i.bundle_kind().unwrap(), BundleKind::IssuingOnly);

    let mut r = dup(&j);
    r.issuing_sk.clear();
    r.leaf_sk.clear();
    assert_eq!(r.bundle_kind().unwrap(), BundleKind::RootOnly);
}

#[test]
fn bundle_kind_rejects_all_illegal_combos() {
    let (_, j) = fresh();
    // root+leaf（缺 issuing）
    let mut a = dup(&j);
    a.issuing_sk.clear();
    assert!(a.bundle_kind().is_err());
    // issuing+leaf（缺 root）
    let mut b = dup(&j);
    b.root_sk.clear();
    assert!(b.bundle_kind().is_err());
    // root+issuing（缺 leaf）
    let mut c = dup(&j);
    c.leaf_sk.clear();
    assert!(c.bundle_kind().is_err());
    // 全空
    let mut d = dup(&j);
    d.root_sk.clear();
    d.issuing_sk.clear();
    d.leaf_sk.clear();
    assert!(d.bundle_kind().is_err());
    // kind_str：非法组合诚实呈现，合法形态呈现形态名
    assert!(kind_str(&d).contains("非法形态"), "{}", kind_str(&d));
    let mut s = dup(&j);
    s.root_sk.clear();
    s.issuing_sk.clear();
    assert_eq!(kind_str(&s), "SignOnly");
}

#[test]
fn json_load_reports_missing_file_and_parse_failure() {
    let tmp = tmpdir();
    let missing = tmp.path().join("nope.json");
    assert!(KeyHierarchyJson::load(&missing.to_string_lossy()).is_err());

    let bad = tmp.path().join("bad.json");
    std::fs::write(&bad, b"{not json").unwrap();
    let err = err_of(KeyHierarchyJson::load(&bad.to_string_lossy()));
    assert!(err.to_string().contains("解析失败"), "{err}");
}

// ===== SigningMaterial（sign 材料全臂）=====

#[test]
fn signing_material_full_lifecycle_signs_and_verifies() {
    let _g = TEST_LOCK.lock().unwrap(); // e2e 走 verify 第⑥步（吊销读 env）
    let tmp = tmpdir();
    let (h, j) = fresh();
    let p = write_json(tmp.path(), "sign.json", &j);

    let m = SigningMaterial::load(&p).unwrap();
    assert_eq!(m.root_anchor(), h.root_anchor_fingerprint());
    assert_eq!(m.chain().len(), 3);
    m.validate(now()).unwrap();

    // to_json → SignOnly 形态 → from_json → validate
    let j2 = m.to_json();
    assert_eq!(j2.bundle_kind().unwrap(), BundleKind::SignOnly);
    let m2 = SigningMaterial::from_json(&j2).unwrap();
    m2.validate(now()).unwrap();

    // save → load 闭环（0600 落盘在 Windows 走 std::fs::write 臂）
    let p2 = tmp.path().join("sign_only.json");
    m2.save(&p2.to_string_lossy()).unwrap();
    let m3 = SigningMaterial::load(&p2.to_string_lossy()).unwrap();
    m3.validate(now()).unwrap();

    // e2e：铸出的材料真能签出可验文件
    let signed = sign_content_v4(
        b"bundle e2e payload",
        &m3.leaf_sk,
        42_000,
        &m3.chain(),
        None,
    )
    .expect("sign_content_v4");
    match verify_bytes(&signed, &[m3.root_anchor()], now()) {
        VerifyOutcome::Valid { signed_at, .. } => assert_eq!(signed_at, 42_000),
        o => panic!("expected Valid, got {o:?}"),
    }
}

#[test]
fn signing_material_from_json_rejects_all_bad_shapes() {
    let (_, j) = fresh();
    // 缺 leaf_sk
    let mut a = dup(&j);
    a.leaf_sk.clear();
    let e = err_of(SigningMaterial::from_json(&a));
    assert!(e.to_string().contains("leaf_sk 在场"), "{e}");
    // 空 leaf_cert / issuing_cert / root_cert（逐字段构造，无 Index）
    for (field, jj) in [
        ("leaf_cert", {
            let mut x = dup(&j);
            x.leaf_cert.clear();
            x
        }),
        ("issuing_cert", {
            let mut x = dup(&j);
            x.issuing_cert.clear();
            x
        }),
        ("root_cert", {
            let mut x = dup(&j);
            x.root_cert.clear();
            x
        }),
    ] {
        let e = err_of(SigningMaterial::from_json(&jj));
        assert!(e.to_string().contains("字段为空"), "{field}: {e}");
        assert!(e.to_string().contains("包形态"), "{field}: {e}");
    }
    // 坏 hex：sk 与 cert 各一
    let mut b1 = dup(&j);
    b1.leaf_sk = "zz".into();
    assert!(SigningMaterial::from_json(&b1).is_err());
    let mut b2 = dup(&j);
    b2.leaf_cert = "zz".into();
    let e = err_of(SigningMaterial::from_json(&b2));
    assert!(e.to_string().contains("leaf_cert"), "{e}");
    // 合法 hex 但非 DER
    let mut b3 = dup(&j);
    b3.leaf_cert = hex_encode(b"not der");
    assert!(SigningMaterial::from_json(&b3).is_err());
    // 版本不支持
    let mut b4 = dup(&j);
    b4.version = KEYS_JSON_VERSION + 7;
    let e = err_of(SigningMaterial::from_json(&b4));
    assert!(e.to_string().contains("版本不支持"), "{e}");
}

#[test]
fn signing_material_validate_failure_arms() {
    let now = now();
    let (h, j) = fresh();
    // sk ↔ cert 错配（leaf_sk 换成另一体系的）
    let other = generate_at(now).unwrap();
    let mut bad = dup(&j);
    bad.leaf_sk = hex_encode(other.leaf_sk.to_bytes().as_ref());
    let m = SigningMaterial::from_json(&bad).unwrap();
    let e = err_of(m.validate(now));
    assert!(e.to_string().contains("不匹配"), "{e}");
    // 有效期越窗（leaf +3y；+10y 必然过期 → 链校验失败）
    let good = SigningMaterial::from_json(&j).unwrap();
    let e = err_of(good.validate(now + 10 * 365 * 86400));
    assert!(e.to_string().contains("链校验失败"), "{e}");
    // load 把 from_json 错误带路径包装
    let tmp = tmpdir();
    let mut root_only = dup(&j);
    root_only.issuing_sk.clear();
    root_only.leaf_sk.clear();
    root_only.leaf_cert.clear();
    let p = write_json(tmp.path(), "root_only.json", &root_only);
    let e = err_of(SigningMaterial::load(&p));
    assert!(e.to_string().contains("root_only.json"), "{e}");
    let _ = h;
}

// ===== IssuingMaterial（中间 CA 材料全臂）=====

#[test]
fn issuing_material_lifecycle_and_full_form_tolerance() {
    let tmp = tmpdir();
    let (h, j) = fresh();
    // IssuingOnly 形态
    let mut ij = dup(&j);
    ij.root_sk.clear();
    ij.leaf_sk.clear();
    ij.leaf_cert.clear();
    assert_eq!(ij.bundle_kind().unwrap(), BundleKind::IssuingOnly);
    let im = IssuingMaterial::from_json(&ij).unwrap();
    im.validate(now()).unwrap();
    assert_eq!(im.root_anchor(), h.root_anchor_fingerprint());

    // to_json roundtrip + save/load
    let j2 = im.to_json();
    assert_eq!(j2.bundle_kind().unwrap(), BundleKind::IssuingOnly);
    let im2 = IssuingMaterial::from_json(&j2).unwrap();
    im2.validate(now()).unwrap();
    let p = write_json(tmp.path(), "issuing.json", &j2);
    let im3 = IssuingMaterial::load(&p).unwrap();
    im3.validate(now()).unwrap();

    // Full 形态也容忍（多余 leaf 字段忽略）
    let imf = IssuingMaterial::from_json(&j).unwrap();
    imf.validate(now()).unwrap();
}

#[test]
fn issuing_material_from_json_rejects_all_bad_shapes() {
    let (_, j) = fresh();
    // 缺 issuing_sk
    let mut a = dup(&j);
    a.issuing_sk.clear();
    let e = err_of(IssuingMaterial::from_json(&a));
    assert!(e.to_string().contains("issuing_sk 在场"), "{e}");
    // 缺 issuing_cert
    let mut b = dup(&j);
    b.issuing_cert.clear();
    let e = err_of(IssuingMaterial::from_json(&b));
    assert!(e.to_string().contains("issuing_cert + root_cert"), "{e}");
    // 缺 root_cert
    let mut c = dup(&j);
    c.root_cert.clear();
    assert!(IssuingMaterial::from_json(&c).is_err());
    // 坏 hex
    let mut d = dup(&j);
    d.issuing_cert = "zz".into();
    assert!(IssuingMaterial::from_json(&d).is_err());
    let mut d2 = dup(&j);
    d2.root_cert = "zz".into();
    assert!(IssuingMaterial::from_json(&d2).is_err());
    let mut d3 = dup(&j);
    d3.issuing_sk = "zz".into();
    assert!(IssuingMaterial::from_json(&d3).is_err());
    // 合法 hex 非 DER
    let mut e1 = dup(&j);
    e1.root_cert = hex_encode(b"junk");
    assert!(IssuingMaterial::from_json(&e1).is_err());
    // 版本
    let mut f = dup(&j);
    f.version = 1;
    assert!(IssuingMaterial::from_json(&f).is_err());
}

#[test]
fn issuing_material_validate_failure_arms() {
    let now = now();
    let (h, j) = fresh();
    let other = generate_at(now).unwrap();

    // ① sk ↔ cert 错配
    let bad_sk = IssuingMaterial {
        issuing_sk: other.issuing_sk.clone(),
        issuing_cert: h.issuing_cert.clone(),
        root_cert: h.root_cert.clone(),
    };
    let e = err_of(bad_sk.validate(now));
    assert!(e.to_string().contains("不匹配"), "{e}");

    // ② issuing AKI ≠ 根 SKI（两个体系的 issuing/root 混装）
    let not_linked = IssuingMaterial {
        issuing_sk: h.issuing_sk.clone(),
        issuing_cert: h.issuing_cert.clone(),
        root_cert: other.root_cert.clone(),
    };
    let e = err_of(not_linked.validate(now));
    assert!(e.to_string().contains("不链"), "{e}");

    // ③ issuing 签名非该根所签：铸一枚「AKI 指向 h 根、签名却出自 other 根」的伪 issuing
    let forged = crate::keygen::issue_x509(
        h.issuing_sk.verifying_key(),
        &other.root_sk,
        &ski_value(h.root_sk.verifying_key()).unwrap(),
        TbsInput {
            subject_cn: "Forged Issuing",
            subject_org: Some("X"),
            issuer_cn: "H Root",
            issuer_org: Some("X"),
            is_ca: true,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: true,
            ku_crl_sign: true,
            eku_code_signing: false,
            not_before_unix: now.saturating_sub(3600),
            not_after_unix: now + 30 * 86400,
        },
    )
    .unwrap();
    let wrong_signer = IssuingMaterial {
        issuing_sk: h.issuing_sk.clone(),
        issuing_cert: forged,
        root_cert: h.root_cert.clone(),
    };
    let e = err_of(wrong_signer.validate(now));
    assert!(e.to_string().contains("签名非该根所签"), "{e}");

    // ④ 根非自签形态：root = 真 issuing 证书（ issuing 的 AKI 指向其 SKI、
    //    签名可被其公钥验证——唯一败点是「根非自签」）
    let child = crate::keygen::issue_x509(
        h.leaf_sk.verifying_key(),
        &h.issuing_sk,
        &ski_value(h.issuing_sk.verifying_key()).unwrap(),
        TbsInput {
            subject_cn: "Child",
            subject_org: Some("X"),
            issuer_cn: "Issuing",
            issuer_org: Some("X"),
            is_ca: false,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: false,
            ku_crl_sign: false,
            eku_code_signing: true,
            not_before_unix: now.saturating_sub(3600),
            not_after_unix: now + 30 * 86400,
        },
    )
    .unwrap();
    let non_self_root = IssuingMaterial {
        issuing_sk: h.leaf_sk.clone(),
        issuing_cert: child,
        root_cert: h.issuing_cert.clone(),
    };
    let e = err_of(non_self_root.validate(now));
    assert!(e.to_string().contains("根证书非自签形态"), "{e}");

    // ⑤ 有效期：issuing 过窗（+10y > issuing 10y 临界——用 +40y 稳过 root 30y）
    let im = IssuingMaterial::from_json(&j).unwrap();
    let e = err_of(im.validate(now + 40 * 365 * 86400));
    assert!(e.to_string().contains("不在有效期"), "{e}");
    let _ = j;
}

// ===== RootMaterial（根材料全臂）=====

fn root_only_json(j: &KeyHierarchyJson) -> KeyHierarchyJson {
    let mut r = dup(j);
    r.issuing_sk.clear();
    r.issuing_cert.clear();
    r.leaf_sk.clear();
    r.leaf_cert.clear();
    r
}

#[test]
fn root_material_lifecycle() {
    let tmp = tmpdir();
    let (h, j) = fresh();
    let rj = root_only_json(&j);
    assert_eq!(rj.bundle_kind().unwrap(), BundleKind::RootOnly);

    let rm = RootMaterial::from_json(&rj).unwrap();
    rm.validate().unwrap();
    assert_eq!(rm.root_anchor(), h.root_anchor_fingerprint());

    // to_json roundtrip + save/load
    let j2 = rm.to_json();
    assert_eq!(j2.bundle_kind().unwrap(), BundleKind::RootOnly);
    let rm2 = RootMaterial::from_json(&j2).unwrap();
    rm2.validate().unwrap();
    let p = write_json(tmp.path(), "root.json", &j2);
    let rm3 = RootMaterial::load(&p).unwrap();
    rm3.validate().unwrap();
}

#[test]
fn root_material_from_json_rejects_all_bad_shapes() {
    let (_, j) = fresh();
    // 多余字段（Full 形态严格拒收）
    let e = err_of(RootMaterial::from_json(&j));
    assert!(e.to_string().contains("多余字段"), "{e}");
    // 四个多余字段各自触发（单字段混入也拒）
    for mutator in [
        |j: &mut KeyHierarchyJson| j.issuing_sk = "aa".into(),
        |j: &mut KeyHierarchyJson| j.issuing_cert = "aa".into(),
        |j: &mut KeyHierarchyJson| j.leaf_sk = "aa".into(),
        |j: &mut KeyHierarchyJson| j.leaf_cert = "aa".into(),
    ] {
        let mut r = root_only_json(&j);
        mutator(&mut r);
        assert!(RootMaterial::from_json(&r).is_err());
    }
    // 缺 root_sk / root_cert
    let mut a = root_only_json(&j);
    a.root_sk.clear();
    let e = err_of(RootMaterial::from_json(&a));
    assert!(e.to_string().contains("root_sk 在场"), "{e}");
    let mut b = root_only_json(&j);
    b.root_cert.clear();
    let e = err_of(RootMaterial::from_json(&b));
    assert!(e.to_string().contains("root_cert 在场"), "{e}");
    // 坏 hex / 非 DER / 版本
    let mut c = root_only_json(&j);
    c.root_cert = "zz".into();
    assert!(RootMaterial::from_json(&c).is_err());
    let mut d = root_only_json(&j);
    d.root_cert = hex_encode(b"junk");
    assert!(RootMaterial::from_json(&d).is_err());
    let mut f = root_only_json(&j);
    f.version = KEYS_JSON_VERSION + 1;
    assert!(RootMaterial::from_json(&f).is_err());
}

#[test]
fn root_material_validate_failure_arms() {
    let now = now();
    let (h, _) = fresh();
    let other = generate_at(now).unwrap();
    // sk ↔ cert 错配
    let bad = RootMaterial {
        root_sk: other.root_sk.clone(),
        root_cert: h.root_cert.clone(),
    };
    let e = err_of(bad.validate());
    assert!(e.to_string().contains("不匹配"), "{e}");
    // 非自签形态（root_cert 换成 issuing 证书）
    let non_self = RootMaterial {
        root_sk: h.issuing_sk.clone(),
        root_cert: h.issuing_cert.clone(),
    };
    let e = err_of(non_self.validate());
    assert!(e.to_string().contains("根证书非自签形态"), "{e}");
}

// ===== 一致性校验 / 拆分 / 铸叶 =====

#[test]
fn validate_full_and_signing_consistency() {
    let now = now();
    let (h, _) = fresh();
    validate_full_consistency(&h, now).unwrap();
    validate_signing_consistency(&h, now).unwrap();

    // 篡改 leaf_sk → full 与 signing 双双失败（各自第一断言点不同）
    let other = generate_at(now).unwrap();
    let bad = KeyHierarchy {
        root_sk: h.root_sk.clone(),
        root_cert: h.root_cert.clone(),
        issuing_sk: h.issuing_sk.clone(),
        issuing_cert: h.issuing_cert.clone(),
        leaf_sk: other.leaf_sk.clone(),
        leaf_cert: h.leaf_cert.clone(),
    };
    assert!(validate_full_consistency(&bad, now).is_err());
    assert!(validate_signing_consistency(&bad, now).is_err());

    // 时钟越窗（+40y > root 30y）→ 链过期
    assert!(validate_full_consistency(&h, now + 40 * 365 * 86400).is_err());
    assert!(validate_signing_consistency(&h, now + 40 * 365 * 86400).is_err());
}

#[test]
fn split_keys_produces_valid_submaterials() {
    let now = now();
    let (h, _) = fresh();
    let (root_m, iss_m) = split_keys(&h, now).unwrap();
    root_m.validate().unwrap();
    iss_m.validate(now).unwrap();
    assert_eq!(root_m.root_anchor(), h.root_anchor_fingerprint());
    assert_eq!(iss_m.root_anchor(), h.root_anchor_fingerprint());
    assert_eq!(
        root_m.to_json().bundle_kind().unwrap(),
        BundleKind::RootOnly
    );
    assert_eq!(
        iss_m.to_json().bundle_kind().unwrap(),
        BundleKind::IssuingOnly
    );
}

#[test]
fn mint_leaf_produces_working_signing_material() {
    let _g = TEST_LOCK.lock().unwrap(); // e2e 走 verify 第⑥步（吊销读 env）
    let now = now();
    let (h, _) = fresh();
    let (root_m, iss_m) = split_keys(&h, now).unwrap();

    let mat = mint_leaf(&iss_m, 365, "ci-run-42", now).unwrap();
    mat.validate(now).unwrap();
    // 回拨 1h 生效：not_before ≤ now
    let nb = mat
        .leaf_cert
        .parsed()
        .unwrap()
        .tbs_certificate
        .validity
        .not_before
        .to_unix_duration()
        .as_secs();
    assert!(nb <= now, "铸叶须回拨 1h 容时钟偏差");
    // 铸出的叶真能签出可验文件
    let signed = sign_content_v4(b"minted payload", &mat.leaf_sk, 43_000, &mat.chain(), None)
        .expect("sign_content_v4");
    match verify_bytes(&signed, &[root_m.root_anchor()], now) {
        VerifyOutcome::Valid { signed_at, .. } => assert_eq!(signed_at, 43_000),
        o => panic!("expected Valid, got {o:?}"),
    }

    // 坏 issuing 材料（sk 错配）→ mint_leaf 防御性拒绝
    let other = generate_at(now).unwrap();
    let bad_iss = IssuingMaterial {
        issuing_sk: other.issuing_sk.clone(),
        issuing_cert: h.issuing_cert.clone(),
        root_cert: h.root_cert.clone(),
    };
    assert!(mint_leaf(&bad_iss, 365, "x", now).is_err());
}

// ===== 锚提取 =====

#[test]
fn load_root_anchor_variants() {
    let tmp = tmpdir();
    let (h, j) = fresh();
    let p = write_json(tmp.path(), "k.json", &j);
    assert_eq!(load_root_anchor(&p).unwrap(), h.root_anchor_fingerprint());

    // root_cert 空
    let mut e = dup(&j);
    e.root_cert.clear();
    let p2 = write_json(tmp.path(), "nocert.json", &e);
    let err = err_of(load_root_anchor(&p2));
    assert!(err.to_string().contains("字段为空"), "{err}");
    // 坏 hex / 非 DER / 文件缺失
    let mut b = dup(&j);
    b.root_cert = "zz".into();
    let p3 = write_json(tmp.path(), "badhex.json", &b);
    assert!(load_root_anchor(&p3).is_err());
    let mut d = dup(&j);
    d.root_cert = hex_encode(b"junk");
    let p4 = write_json(tmp.path(), "badder.json", &d);
    assert!(load_root_anchor(&p4).is_err());
    assert!(load_root_anchor(&tmp.path().join("missing.json").to_string_lossy()).is_err());
}

#[test]
fn root_anchor_from_der_file_variants() {
    let tmp = tmpdir();
    let (h, _) = fresh();
    let der_p = tmp.path().join("root.der");
    std::fs::write(&der_p, h.root_cert.to_der()).unwrap();
    assert_eq!(
        root_anchor_from_der_file(&der_p.to_string_lossy()).unwrap(),
        h.root_anchor_fingerprint()
    );
    // 缺文件 / 垃圾内容
    let missing = tmp.path().join("missing.der");
    let err = err_of(root_anchor_from_der_file(&missing.to_string_lossy()));
    assert!(err.to_string().contains("读取失败"), "{err}");
    let garbage = tmp.path().join("garbage.der");
    std::fs::write(&garbage, b"junk").unwrap();
    let err = err_of(root_anchor_from_der_file(&garbage.to_string_lossy()));
    assert!(err.to_string().contains("解析失败"), "{err}");
}
