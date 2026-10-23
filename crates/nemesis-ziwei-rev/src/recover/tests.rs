//! recover 重扫引擎单测（合成 PE32+ 镜像，夹具复用 crate 级 tests.rs 构造器）。

use super::{AnchorLite, load_image, parse_pattern, rescan};
use crate::tests::test_pe_file;

fn anchor(kind: &str, f: &str, patch: impl FnOnce(&mut AnchorLite)) -> AnchorLite {
    let mut a = AnchorLite {
        function: f.to_string(),
        kind: kind.to_string(),
        pattern: None,
        text: None,
        value: None,
        imports: None,
    };
    patch(&mut a);
    a
}

#[test]
fn pattern_parse_forms() {
    // 带空格 + 连续双形态同解
    let (b1, m1) = parse_pattern("90 90 B8 8D").unwrap();
    let (b2, m2) = parse_pattern("9090B88D").unwrap();
    assert_eq!(b1, vec![0x90, 0x90, 0xB8, 0x8D]);
    assert_eq!(m1, vec![0xFF; 4]);
    assert_eq!(b1, b2);
    assert_eq!(m1, m2);
    // 通配整字节
    let (b, m) = parse_pattern("B8 ?? ?? ?? 5A").unwrap();
    assert_eq!(b, vec![0xB8, 0, 0, 0, 0x5A]);
    assert_eq!(m, vec![0xFF, 0, 0, 0, 0xFF]);
    // 半通配 / 奇数长 / 空 loud 拒绝
    assert!(parse_pattern("B? 8D").is_err());
    assert!(parse_pattern("90 0").is_err());
    assert!(parse_pattern("").is_err());
    assert!(parse_pattern("ZZ 8D").is_err());
}

#[test]
fn load_image_from_synthetic_file() {
    let img = load_image(&test_pe_file()).expect("合成 PE 组装");
    assert_eq!(img.pe.sections.len(), 2);
    // .text 头 5 字节 = E8 03 00 00 00
    assert_eq!(
        img.read_rva(0x1000, 5),
        Some(&[0xE8, 0x03, 0x00, 0x00, 0x00][..])
    );
}

#[test]
fn byte_pattern_hit_and_wildcard() {
    let file = test_pe_file();
    // nopsled×8 + ret：0x1020 唯一命中
    let a = anchor("byte_pattern", "sled_fn", |a| {
        a.pattern = Some("90 90 90 90 90 90 90 90 C3".into());
    });
    let v = rescan(&file, &[a]).unwrap();
    assert_eq!(v.verdict, "recoverable");
    let r = &v.per_anchor[0];
    assert!(r.hit);
    assert_eq!(r.found_rva, Some(0x1020));
    assert_eq!(r.hit_count, 1);
    // 通配：B8 ?? ?? ?? 5A → 0x1008
    let a = anchor("byte_pattern", "imm_fn", |a| {
        a.pattern = Some("B8 ?? ?? ?? 5A".into());
    });
    let v = rescan(&file, &[a]).unwrap();
    assert_eq!(v.per_anchor[0].found_rva, Some(0x1008));
    // miss：全 FF 不在镜像
    let a = anchor("byte_pattern", "gone", |a| {
        a.pattern = Some("FF FF FF FF FF FF FF FF FF FF".into());
    });
    let v = rescan(&file, &[a]).unwrap();
    assert!(!v.per_anchor[0].hit);
    assert_eq!(v.verdict, "unrecoverable");
}

#[test]
fn imm_const_and_str_ref() {
    let file = test_pe_file();
    // 0x5A6B7C8D → LE @0x1009（mov eax 立即数）
    let a = anchor("imm_const", "imm_fn", |a| a.value = Some(0x5A6B_7C8D));
    let v = rescan(&file, &[a]).unwrap();
    assert!(v.per_anchor[0].hit);
    assert_eq!(v.per_anchor[0].found_rva, Some(0x1009));
    // miss 常量
    let a = anchor("imm_const", "gone", |a| a.value = Some(0x1234_5678));
    let v = rescan(&file, &[a]).unwrap();
    assert!(!v.per_anchor[0].hit);
    // str_ref（.rdata @0x20F0）
    let a = anchor("str_ref", "str_fn", |a| {
        a.text = Some("ZIWEI_TEST_STRING".into());
    });
    let v = rescan(&file, &[a]).unwrap();
    assert!(v.per_anchor[0].hit);
    assert_eq!(v.per_anchor[0].found_rva, Some(0x20F0));
    // 超 imm32 域诚实不猜
    let a = anchor("imm_const", "big", |a| a.value = Some(0x1_0000_0000));
    let v = rescan(&file, &[a]).unwrap();
    assert!(!v.per_anchor[0].hit);
    assert!(v.per_anchor[0].note.contains("超 imm32"));
}

#[test]
fn import_call_seq_presence() {
    let file = test_pe_file();
    let a = anchor("import_call_seq", "import_fn", |a| {
        a.imports = Some("USER32.dll!MessageBoxA".into());
    });
    let v = rescan(&file, &[a]).unwrap();
    assert!(v.per_anchor[0].hit);
    // 部分缺失 → miss + 缺失清单注记
    let a = anchor("import_call_seq", "import_fn", |a| {
        a.imports = Some("USER32.dll!MessageBoxA,USER32.dll!Nope".into());
    });
    let v = rescan(&file, &[a]).unwrap();
    assert!(!v.per_anchor[0].hit);
    assert!(v.per_anchor[0].note.contains("Nope"));
}

#[test]
fn bb_hash_excluded_from_vote() {
    let file = test_pe_file();
    // 只有 bb_hash：支持集为空 → unrecoverable 且 hit 计 0
    let a = anchor("bb_hash", "bb_fn", |a| a.pattern = None);
    let v = rescan(&file, &[a]).unwrap();
    assert_eq!(v.anchors_total, 1);
    assert_eq!(v.anchors_supported, 0);
    assert_eq!(v.verdict, "unrecoverable");
    assert!(v.per_anchor[0].note.contains("v1 未实现"));
}

#[test]
fn mixed_verdicts() {
    let file = test_pe_file();
    let all_hit = vec![
        anchor("byte_pattern", "sled_fn", |a| {
            a.pattern = Some("90 90 90 90 90 90 90 90 C3".into());
        }),
        anchor("imm_const", "imm_fn", |a| a.value = Some(0x5A6B_7C8D)),
        anchor("str_ref", "str_fn", |a| {
            a.text = Some("ZIWEI_TEST_STRING".into())
        }),
        anchor("import_call_seq", "import_fn", |a| {
            a.imports = Some("USER32.dll!MessageBoxA".into());
        }),
        anchor("bb_hash", "bb_fn", |_| {}),
    ];
    let v = rescan(&file, &all_hit).unwrap();
    assert_eq!(v.anchors_supported, 4);
    assert_eq!(v.anchors_hit, 4);
    assert_eq!(v.verdict, "recoverable");

    // 一 miss（受支持锚）→ partially_recoverable
    let partial = vec![
        anchor("byte_pattern", "sled_fn", |a| {
            a.pattern = Some("90 90 90 90 90 90 90 90 C3".into());
        }),
        anchor("imm_const", "gone", |a| a.value = Some(0x1234_5678)),
    ];
    let v = rescan(&file, &partial).unwrap();
    assert_eq!(v.verdict, "partially_recoverable");

    // 全 miss（bb_hash 不算）→ unrecoverable
    let none = vec![
        anchor("imm_const", "gone", |a| a.value = Some(0x1234_5678)),
        anchor("bb_hash", "bb_fn", |_| {}),
    ];
    let v = rescan(&file, &none).unwrap();
    assert_eq!(v.verdict, "unrecoverable");
    assert_eq!(v.anchors_supported, 1);
}

#[test]
fn malformed_file_loud_reject() {
    assert!(rescan(b"MZ garbage", &[]).is_err());
    assert!(load_image(&[]).is_err());
}
