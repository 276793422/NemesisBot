//! nemesis-ziwei-profile 单测：schema round-trip / loud 拒绝 / 锚判定 /
//! 存储读写 / 校验语义。

use crate::anchors::{
    Anchor, AnchorScanResult, AnchorSet, AnchorType, AttachVerdict, decide_verdict,
};
use crate::model::*;
use crate::store::{ProfileStore, new_profile_id};
use serde_json::json;

fn sample_profile() -> Profile {
    let mut vr = std::collections::BTreeMap::new();
    vr.insert("ProductName".to_string(), "DataMon".to_string());
    vr.insert("FileVersion".to_string(), "3.2.1".to_string());
    Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: "zw-pro-8f3a0001".to_string(),
        name: "XX数据台-导出报表".to_string(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: ProfileStatus::Active,
        target: TargetFingerprint {
            exe_name: "DataMon.exe".to_string(),
            exe_path_hint: Some("C:/Program Files/DataMon/DataMon.exe".to_string()),
            machine: Machine::X64,
            file_sha256: Some("a".repeat(64)),
            version_resource: vr,
            verify_policy: VerifyPolicy::HashOrVersion,
        },
        attach: AttachInfo::default(),
        functions: vec![FunctionDef {
            semantic: "数据推送（绘图前最后一次经过）".to_string(),
            locate: LocateSpec {
                kind: LocateKind::BytePattern,
                module: Some("DataMon.exe".to_string()),
                pattern: Some("48 89 5C 24 ?? 57 48 83 EC 30".to_string()),
                rva: None,
            },
            conv: Conv::MsX64,
            args: vec![ArgSpec {
                reg: "rcx".to_string(),
                ty: None,
                desc: Some("DataRecord 指针".to_string()),
            }],
            capture: Some(CaptureSpec {
                decode: Some("struct:DataRecord".to_string()),
                emit: Some(EmitSink::HookCapture),
                chain: vec![],
            }),
        }],
        data_structures: vec![DataStructure {
            name: "DataRecord".to_string(),
            size: 96,
            fields: vec![
                FieldDef {
                    offset: 0,
                    name: "timestamp".to_string(),
                    type_: FieldType::parse("u64").unwrap(),
                    decode: Some(FieldDecode::UnixMs),
                },
                FieldDef {
                    offset: 8,
                    name: "value".to_string(),
                    type_: FieldType::parse("f64").unwrap(),
                    decode: None,
                },
                FieldDef {
                    offset: 16,
                    name: "label".to_string(),
                    type_: FieldType::parse("char[32]").unwrap(),
                    decode: Some(FieldDecode::Utf8),
                },
            ],
        }],
        tools: vec![ToolDef {
            tool_name: "datamon_export".to_string(),
            description: "从 XX数据台 导出实时数据记录".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "count": { "type": "integer", "default": 100 } }
            }),
            action: ToolAction {
                primitive: Primitive::PollReport,
                addr: "struct:DataRecord".to_string(),
                mode: Some(PollMode::OnChange),
                interval_secs: Some(2),
                max_records: Some(1000),
                timeout_ms: None,
                notify: None,
            },
            output: Some(OutputSpec {
                dest: "workspace/reports/datamon_export.json".to_string(),
            }),
            run_policy: RunPolicy::Auto,
        }],
        verification: Verification {
            on_attach: vec!["file_sha256".to_string(), "anchors_vote>=2".to_string()],
            stale_action: Some("notify_user".to_string()),
        },
    }
}

fn scans(matched: &[bool]) -> Vec<AnchorScanResult> {
    matched
        .iter()
        .enumerate()
        .map(|(i, &m)| AnchorScanResult {
            function: format!("fn{i}"),
            kind: AnchorType::BytePattern,
            matched: m,
            hits: if m { 1 } else { 0 },
        })
        .collect()
}

#[test]
fn roundtrip_full_profile() {
    let p = sample_profile();
    let s = serde_json::to_string(&p).unwrap();
    let p2: Profile = serde_json::from_str(&s).unwrap();
    assert_eq!(p, p2);
    // 总控 §6.2 的 JSON 键形态："type" 字段与枚举 snake_case 都要还原。
    assert_eq!(
        p2.data_structures[0].fields[2].type_,
        FieldType::parse("char[32]").unwrap()
    );
    assert_eq!(p2.functions[0].conv, Conv::MsX64);
}

#[test]
fn arg_ty_roundtrip_and_legacy_compat() {
    // B4：ArgSpec.ty round-trip 保真
    let mut p = sample_profile();
    p.functions[0].args[0].ty = Some("u64".to_string());
    let s = serde_json::to_string(&p).unwrap();
    let p2: Profile = serde_json::from_str(&s).unwrap();
    assert_eq!(p2.functions[0].args[0].ty.as_deref(), Some("u64"));
    // 合法形态过 validate
    p.validate().unwrap();
    // 旧文件兼容：无 ty 键照读（serde default = None）
    let mut v = serde_json::to_value(&p).unwrap();
    v["functions"][0]["args"][0]
        .as_object_mut()
        .unwrap()
        .remove("ty");
    let legacy: Profile = serde_json::from_value(v).unwrap();
    assert_eq!(legacy.functions[0].args[0].ty, None);
    legacy.validate().unwrap();
}

#[test]
fn reject_bad_arg_ty_and_empty_reg() {
    let mut p = sample_profile();
    p.functions[0].args[0].ty = Some("int*".to_string());
    let err = p.validate().unwrap_err();
    assert!(err.contains("类型") && err.contains("非法"), "{err}");
    let mut p2 = sample_profile();
    p2.functions[0].args[0].reg = "  ".to_string();
    let err2 = p2.validate().unwrap_err();
    assert!(err2.contains("reg 为空"), "{err2}");
}

#[test]
fn reject_unknown_field_loud() {
    let mut v = serde_json::to_value(sample_profile()).unwrap();
    v["extra_unknown_key"] = json!(1);
    let err = serde_json::from_value::<Profile>(v).unwrap_err();
    assert!(
        err.to_string().contains("extra_unknown_key"),
        "错误须点名未知字段: {err}"
    );
}

#[test]
fn reject_unknown_enum_value() {
    let mut v = serde_json::to_value(sample_profile()).unwrap();
    v["tools"][0]["action"]["primitive"] = json!("write_mem");
    let err = serde_json::from_value::<Profile>(v).unwrap_err();
    assert!(!err.to_string().is_empty());
    // 未知原语必须在反序列化期就拒——不允许带病加载。
}

#[test]
fn reject_unknown_schema_version() {
    let mut p = sample_profile();
    p.schema_version = 2;
    assert!(p.validate().is_err());
}

#[test]
fn reject_field_out_of_bounds() {
    let mut p = sample_profile();
    p.data_structures[0].fields[0].offset = 90; // u64 @90, 90+8=98 < 96? 否：90+8=98 > 96 越界
    let err = p.validate().unwrap_err();
    assert!(err.contains("越界"), "{err}");
}

#[test]
fn reject_chain_over_cap() {
    let mut p = sample_profile();
    let step = ChainStep {
        locate: LocateSpec {
            kind: LocateKind::BytePattern,
            module: None,
            pattern: Some("90 90".to_string()),
            rva: None,
        },
        decode: None,
        note: None,
    };
    p.functions[0].capture.as_mut().unwrap().chain = vec![step.clone(); CHAIN_CAP + 1];
    let err = p.validate().unwrap_err();
    assert!(err.contains("超上限"), "{err}");
    // 恰好 4 层合法（决议 5）。
    p.functions[0].capture.as_mut().unwrap().chain = vec![step; CHAIN_CAP];
    assert!(p.validate().is_ok());
}

#[test]
fn reject_bad_tool_name_and_dup() {
    let mut p = sample_profile();
    p.tools[0].tool_name = "Datamon-Export!".to_string();
    assert!(p.validate().is_err());
    let dup = p.tools[0].clone();
    p.tools.push(dup);
    // 注意先恢复合法名再测重复。
    p.tools[0].tool_name = "datamon_export".to_string();
    p.tools[1].tool_name = "datamon_export".to_string();
    assert!(p.validate().is_err());
}

#[test]
fn reject_call_fn_to_unknown_fn() {
    let mut p = sample_profile();
    p.functions.clear();
    p.tools[0].action = ToolAction {
        primitive: Primitive::CallFn,
        addr: "fn:数据推送（绘图前最后一次经过）".to_string(),
        mode: None,
        interval_secs: None,
        max_records: None,
        timeout_ms: Some(3000),
        notify: None,
    };
    let err = p.validate().unwrap_err();
    assert!(err.contains("不在 functions"), "{err}");
}

#[test]
fn reject_absolute_output_dest() {
    let mut p = sample_profile();
    p.tools[0].output.as_mut().unwrap().dest = "C:/Windows/temp/x.json".to_string();
    assert!(p.validate().is_err());
    p.tools[0].output.as_mut().unwrap().dest = "../escape.json".to_string();
    assert!(p.validate().is_err());
    p.tools[0].output.as_mut().unwrap().dest = "workspace/reports/ok.json".to_string();
    assert!(p.validate().is_ok());
}

#[test]
fn reject_bad_byte_pattern() {
    assert!(parse_byte_pattern("48 89 5C").is_ok());
    assert!(parse_byte_pattern("48 ?? 5C").is_ok());
    assert!(parse_byte_pattern("48 8? 5C").is_err());
    assert!(parse_byte_pattern("").is_err());
    assert!(parse_byte_pattern("ZZ").is_err());
    // 连续形态（candidates payload / LLM 应答的無空格 hex）
    let (b, m) = parse_byte_pattern("90909090E940000000").expect("compact");
    assert_eq!(b, vec![0x90, 0x90, 0x90, 0x90, 0xE9, 0x40, 0, 0, 0]);
    assert_eq!(m, vec![1, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert!(parse_byte_pattern("48895C247").is_err(), "奇数长连续串拒绝");
    assert!(
        parse_byte_pattern("488 95C").is_err(),
        "混合形态走 token 路径拒绝"
    );
}

#[test]
fn notify_expr_syntax() {
    assert!(validate_notify_expr("state==ALERT").is_ok());
    assert!(validate_notify_expr("value >= 90.5").is_ok());
    assert!(validate_notify_expr("state != WARN").is_ok());
    assert!(validate_notify_expr("no operator here").is_err());
    assert!(validate_notify_expr("1state==X").is_err());
    assert!(validate_notify_expr("state==!!").is_err());
}

#[test]
fn anchors_vote_threshold_parse() {
    let v = Verification {
        on_attach: vec!["file_sha256".to_string(), "anchors_vote>=3".to_string()],
        stale_action: None,
    };
    assert_eq!(v.anchors_vote_threshold(), 3);
    let v2 = Verification {
        on_attach: vec![],
        stale_action: None,
    };
    assert_eq!(v2.anchors_vote_threshold(), 2);
    let v3 = Verification {
        on_attach: vec!["anchors_vote>=0".to_string()],
        stale_action: None,
    };
    // 0 非法 → 落默认 2（投票阈值至少 1）。
    assert_eq!(v3.anchors_vote_threshold(), 2);
}

#[test]
fn verdict_decide() {
    // hash 一致 → 直接 active。
    assert_eq!(decide_verdict(true, &[], 2), AttachVerdict::Active);
    // hash 不一致 + 2 票过阈值 → 降级 active。
    assert_eq!(
        decide_verdict(false, &scans(&[true, true, false]), 2),
        AttachVerdict::ActiveDegraded
    );
    // 票不过 → stale。
    assert_eq!(
        decide_verdict(false, &scans(&[true, false, false]), 2),
        AttachVerdict::Stale
    );
    assert_eq!(
        decide_verdict(false, &scans(&[false, false, false]), 2),
        AttachVerdict::Stale
    );
}

#[test]
fn anchor_validate_forms() {
    let ok = Anchor {
        function: "数据推送".to_string(),
        kind: AnchorType::ImmConst,
        module: Some("DataMon.exe".to_string()),
        pattern: None,
        text: None,
        distance: None,
        value: Some(0xDE1C7A01),
        imports: None,
        bb_hash: None,
        note: None,
    };
    assert!(crate::anchors::validate_anchor(&ok).is_ok());
    let bad = Anchor { value: None, ..ok };
    assert!(crate::anchors::validate_anchor(&bad).is_err());
}

#[test]
fn store_save_load_roundtrip_and_reject() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(tmp.path());
    let p = sample_profile();
    let anchors = AnchorSet {
        profile_id: p.profile_id.clone(),
        anchors: vec![Anchor {
            function: "数据推送（绘图前最后一次经过）".to_string(),
            kind: AnchorType::BytePattern,
            module: Some("DataMon.exe".to_string()),
            pattern: Some("48 89 5C 24 ?? 57".to_string()),
            text: None,
            distance: None,
            value: None,
            imports: None,
            bb_hash: None,
            note: None,
        }],
    };
    store.save(&p, Some(&anchors)).unwrap();

    let loaded = store.load(&p.profile_id).unwrap();
    assert_eq!(loaded, p);
    let la = store.load_anchors(&p.profile_id).unwrap();
    assert_eq!(la, anchors);
    store
        .save_notes(&p.profile_id, "# 标注笔记\n- 数据推送在绘图前触发\n")
        .unwrap();
    assert!(
        store
            .load_notes(&p.profile_id)
            .unwrap()
            .contains("标注笔记")
    );

    let list = store.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].profile_id, p.profile_id);
    assert_eq!(list[0].tool_count, 1);

    // 锚关联不存在函数 → 拒。
    let mut bad_anchors = anchors.clone();
    bad_anchors.anchors[0].function = "不存在的函数".to_string();
    assert!(store.save(&p, Some(&bad_anchors)).is_err());
    // anchors.profile_id 不一致 → 拒。
    let mut wrong = anchors;
    wrong.profile_id = "zw-pro-other000".to_string();
    assert!(store.save(&p, Some(&wrong)).is_err());

    store.delete(&p.profile_id).unwrap();
    assert!(store.load(&p.profile_id).is_err());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn store_rejects_path_traversal_id() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(tmp.path());
    assert!(store.load("../evil").is_err());
    assert!(store.load("zw-pro-ok").is_err()); // 不存在但形态合法 → NotFound
    match store.load("zw-pro-ok") {
        Err(crate::store::StoreError::NotFound(_)) => {}
        other => panic!("期望 NotFound，得到 {other:?}"),
    }
}

#[test]
fn unknown_fields_in_nested_structs_rejected() {
    let mut v = serde_json::to_value(sample_profile()).unwrap();
    v["target"]["mystery"] = json!(true);
    assert!(serde_json::from_value::<Profile>(v).is_err());
    let mut v = serde_json::to_value(sample_profile()).unwrap();
    v["tools"][0]["action"]["mystery2"] = json!(true);
    assert!(serde_json::from_value::<Profile>(v).is_err());
}

#[test]
fn field_type_parse_forms() {
    assert_eq!(FieldType::parse("u64").unwrap().size_of(), 8);
    assert_eq!(FieldType::parse("char[32]").unwrap().size_of(), 32);
    assert_eq!(FieldType::parse("wchar[64]").unwrap().size_of(), 128);
    assert_eq!(FieldType::parse("bool").unwrap().size_of(), 1);
    assert!(FieldType::parse("u64[0]").is_none());
    assert!(FieldType::parse("int").is_none());
    assert!(FieldType::parse("u32[16").is_none());
    assert!(FieldType::parse("u64]8[").is_none());
    // round-trip
    let ft = FieldType::parse("f32[4]").unwrap();
    let s = serde_json::to_string(&ft).unwrap();
    assert_eq!(s, "\"f32[4]\"");
}

#[test]
fn profile_id_shape() {
    let id = new_profile_id("XX数据台");
    assert!(id.starts_with("zw-pro-") && id.len() == "zw-pro-".len() + 8);
    // 不同种子（名字 + 纳秒时间）产出不同 id。
    assert_ne!(new_profile_id("另一个软件"), id);
}

// ── BUG-20261028-123/124/125 回归锁 ─────────────────────────────────────

#[test]
fn size_of_returns_u64_truth_no_wraparound() {
    // BUG-20261028-123：u32 乘法在 `u64[536870913]` 上回绕——8×(2^29+1)
    // mod 2^32 = 8。size_of 必须回 u64 真值（修前 debug panic / release
    // 静默变小使 validate 越界闸失效）。
    let big = FieldType::parse("u64[536870913]").unwrap();
    assert_eq!(big.size_of(), 4_294_967_304u64);
    // 常规数组真值不受影响。
    assert_eq!(FieldType::parse("u32[16]").unwrap().size_of(), 64);
    assert_eq!(FieldType::parse("char[32]").unwrap().size_of(), 32);
}

#[test]
fn reject_overflow_array_field_out_of_bounds() {
    // 同上：回绕后 8 < 96 曾溜过越界闸（release 误过、debug panic）。
    // 修复后按 u64 真值 loud 拒绝。
    let mut p = sample_profile();
    p.data_structures[0].fields[0].type_ = FieldType::parse("u64[536870913]").unwrap();
    let err = p.validate().unwrap_err();
    assert!(err.contains("越界"), "{err}");
}

#[test]
fn reject_windows_reserved_names_in_dest() {
    // BUG-20261028-124：22 个 Windows 保留设备名（按「首个点之前」的
    // stem 判定、大小写不敏感）——修前只拦冒号，con.txt 等可溜过。
    for dest in ["con.txt", "reports/nul", "COM1.dat", "lpt9"] {
        let mut p = sample_profile();
        p.tools[0].output.as_mut().unwrap().dest = dest.to_string();
        let err = p.validate().unwrap_err();
        assert!(err.contains("保留"), "dest={dest}: {err}");
    }
    // 不误伤面：stem 不在保留集照常通过。
    for dest in [
        "workspace/reports/config.json",
        "console.log",
        "con-x/con_x.json",
        "auxiliary/x.json",
    ] {
        let mut p = sample_profile();
        p.tools[0].output.as_mut().unwrap().dest = dest.to_string();
        assert!(p.validate().is_ok(), "dest={dest} 不应被拦");
    }
}

#[test]
fn reject_duplicate_struct_and_fn_names() {
    // BUG-20261028-125：结构/函数语义名均按名引用（struct:/fn:/锚/
    // recover_confirm 接受集），重名即引用歧义。
    let mut p = sample_profile();
    let dup_ds = p.data_structures[0].clone();
    p.data_structures.push(dup_ds);
    let err = p.validate().unwrap_err();
    assert!(err.contains("重名"), "{err}");

    let mut p = sample_profile();
    let dup_fn = p.functions[0].clone();
    p.functions.push(dup_fn);
    let err = p.validate().unwrap_err();
    assert!(err.contains("重名"), "{err}");
}
