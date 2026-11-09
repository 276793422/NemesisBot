use super::*;
use nemesis_verify::RevDim;
use nemesis_verify::verify::VerifyOutcome;

#[test]
fn grade_mapping_table_is_discipline() {
    // §4a 分级表逐态钉住：改映射必须显式过这里。
    assert_eq!(grade(&VerifyOutcome::NoSignature), SigGrade::Yellow);
    assert_eq!(grade(&VerifyOutcome::Untrusted), SigGrade::Red);
    assert_eq!(grade(&VerifyOutcome::SignatureInvalid), SigGrade::Red);
    assert_eq!(grade(&VerifyOutcome::Tampered("x".into())), SigGrade::Red);
    assert_eq!(grade(&VerifyOutcome::Expired("y".into())), SigGrade::Red);
    assert_eq!(grade(&VerifyOutcome::Malformed("z".into())), SigGrade::Red);
    assert_eq!(
        grade(&VerifyOutcome::UnsupportedVersion("w".into())),
        SigGrade::Red
    );
    assert_eq!(
        grade(&VerifyOutcome::Revoked {
            dim: RevDim::KeyFp,
            value: "fp".into(),
            revoked_at: 1,
            reason: "r".into()
        }),
        SigGrade::Red
    );
    // Valid（绿）：真实 keygen+verify 才能造出，归 crate 级 e2e（tests.rs
    // signed_package_green_with_runtime_anchor）覆盖。
}

#[test]
fn outcome_text_covers_all_nine_states() {
    // 每态文本非空（GUI 横幅/静默 WARN 的唯一文案源）。
    let cases: Vec<VerifyOutcome> = vec![
        VerifyOutcome::Valid {
            signed_at: 42,
            key_fp: [1; 32],
            pubkey: [2; 65],
        },
        VerifyOutcome::NoSignature,
        VerifyOutcome::Tampered("t".into()),
        VerifyOutcome::SignatureInvalid,
        VerifyOutcome::Untrusted,
        VerifyOutcome::Revoked {
            dim: RevDim::Publisher,
            value: "pub".into(),
            revoked_at: 9,
            reason: "why".into(),
        },
        VerifyOutcome::Expired("e".into()),
        VerifyOutcome::UnsupportedVersion("u".into()),
        VerifyOutcome::Malformed("m".into()),
    ];
    for c in &cases {
        let text = outcome_text(c);
        assert!(!text.is_empty(), "{c:?} 文本为空");
    }
    assert!(outcome_text(&VerifyOutcome::NoSignature).contains("无签名"));
}

#[test]
fn banner_text_matches_grade() {
    assert_eq!(
        SigGrade::Green.banner_text(&VerifyOutcome::NoSignature),
        "签名有效"
    );
    let yellow = SigGrade::Yellow.banner_text(&VerifyOutcome::NoSignature);
    assert!(yellow.contains("未签名"));
    let red = SigGrade::Red.banner_text(&VerifyOutcome::Tampered("t".into()));
    assert!(red.contains("签名异常"));
    assert!(red.contains("t"), "红级文本必须携带根因明细");
}

#[test]
fn anchors_default_empty_is_fail_closed() {
    // 无编译期注入 + 无运行时 env = 空锚集 = 一切签名 Untrusted。
    // （CI/开发机常态；锚注入属部署配置，不在单测里设。）
    // 注意：本用例若在注入了 NEMESIS_ROOT_ANCHOR 的环境跑会失真——
    // crate 级 e2e 的 ENV_LOCK 纪律只护它自己的用例；这里只在锚为空时断言。
    if std::env::var("NEMESIS_ROOT_ANCHOR").is_err() {
        let anchors = anchors();
        // 编译期也未注入（option_env! 编译本 crate 时无值）。
        assert!(
            anchors.is_empty(),
            "零配置锚集必须为空（fail-closed），实际 {} 条",
            anchors.len()
        );
    }
}

#[test]
fn shell_self_check_returns_valid_outcome_shape() {
    // 外壳自检（§4a）只要求「返回九态之一且文本非空」——测试二进制未签，
    // 常态 = NoSignature（黄）；若开发机环境带了锚 + 整包被签过则可能 Red，
    // 都属合法形态，断言不锚具体态（环境敏感清单纪律）。
    let display = shell_self_check();
    assert!(!display.text.is_empty(), "外壳自检文本为空");
    // 测试进程 exe 未走 pack sign → PE/ELF 无证书表/footer → 必非绿。
    assert_ne!(
        display.grade,
        SigGrade::Green,
        "未签测试二进制不应自检为绿: {}",
        display.text
    );
}
