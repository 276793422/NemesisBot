use super::*;
use nemesis_ziwei_profile::{DataStructure, FieldDef};

fn ft(spec: &str) -> FieldType {
    FieldType::parse(spec).unwrap()
}

#[test]
fn json_helpers() {
    let s = r#"{"proto":1,"token":"abc123","arch":"x64","pid":42}"#;
    assert_eq!(json_get_u64(s, "proto"), Some(1));
    assert_eq!(json_get_str(s, "token"), Some("abc123"));
    assert_eq!(json_get_str(s, "arch"), Some("x64"));
    assert_eq!(json_get_u64(s, "pid"), Some(42));
    assert_eq!(json_get_u64(s, "missing"), None);
    let hex = r#"{"addr":"0x7ff6a000","len":16}"#;
    // 字符串值不走 u64 解析
    assert_eq!(json_get_u64(hex, "addr"), None);
    let hex2 = r#"{"addr":0x7ff6a000,"len":16}"#;
    assert_eq!(json_get_u64(hex2, "addr"), Some(0x7ff6_a000));
    assert_eq!(json_get_u64(hex2, "len"), Some(16));
}

#[test]
fn base64_roundtrip() {
    assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(base64_decode("YQ==").unwrap(), b"a");
    assert_eq!(base64_decode("YWI=").unwrap(), b"ab");
    assert_eq!(base64_decode("YWJj").unwrap(), b"abc");
    assert!(base64_decode("!!!").is_err());
}

#[test]
fn read_mem_resp_parse() {
    let resp = r#"{"ok":true,"addr":4281396224,"read":4,"data":"BQIDAA=="}"#;
    let data = parse_read_mem_resp(resp).unwrap();
    assert_eq!(data, vec![5u8, 2, 3, 0]);
    let err = parse_read_mem_resp(r#"{"ok":false,"error":"ReadProcessMemory 失败"}"#).unwrap_err();
    assert!(err.to_string().contains("ReadProcessMemory"));
}

#[test]
fn decode_v1_record() {
    // 与 dummy-draw V1 记录同构：id u32 @0, value i64 @8, label char[16] @16, flags u32 @32
    let ds = DataStructure {
        name: "DataRecord".into(),
        size: 40,
        fields: vec![
            FieldDef {
                offset: 0,
                name: "id".into(),
                type_: ft("u32"),
                decode: None,
            },
            FieldDef {
                offset: 8,
                name: "value".into(),
                type_: ft("i64"),
                decode: None,
            },
            FieldDef {
                offset: 16,
                name: "label".into(),
                type_: ft("char[16]"),
                decode: Some(FieldDecode::Utf8),
            },
            FieldDef {
                offset: 32,
                name: "flags".into(),
                type_: ft("u32"),
                decode: Some(FieldDecode::Hex),
            },
        ],
    };
    let mut buf = vec![0u8; 40];
    buf[0..4].copy_from_slice(&3u32.to_le_bytes());
    buf[8..16].copy_from_slice(&1005i64.to_le_bytes());
    buf[16..16 + 5].copy_from_slice(b"REC_3");
    buf[32..36].copy_from_slice(&0x0301u32.to_le_bytes());
    let v = decode_struct(&buf, &ds).unwrap();
    assert_eq!(v["id"], 3);
    assert_eq!(v["value"], 1005);
    assert_eq!(v["label"], "REC_3");
    assert_eq!(v["flags"], "0x301");
}

#[test]
fn decode_wchar_and_unix() {
    // MonitorState 布局（与 dummy-monitor 同构）：magic@0 status@4 tick@8 message(wchar[32])@16 → 80 字节
    let ds = DataStructure {
        name: "MonitorState".into(),
        size: 80,
        fields: vec![
            FieldDef {
                offset: 0,
                name: "magic".into(),
                type_: ft("u32"),
                decode: Some(FieldDecode::Hex),
            },
            FieldDef {
                offset: 4,
                name: "status".into(),
                type_: ft("u32"),
                decode: None,
            },
            FieldDef {
                offset: 8,
                name: "tick".into(),
                type_: ft("u64"),
                decode: None,
            },
            FieldDef {
                offset: 16,
                name: "message".into(),
                type_: ft("wchar[32]"),
                decode: Some(FieldDecode::Utf16),
            },
        ],
    };
    let mut buf = vec![0u8; 80];
    buf[0..4].copy_from_slice(&0x4D4F_4E31u32.to_le_bytes());
    buf[4..8].copy_from_slice(&2u32.to_le_bytes());
    buf[8..16].copy_from_slice(&7u64.to_le_bytes());
    let msg: Vec<u16> = "MONITOR_ALERT".encode_utf16().collect();
    for (i, u16v) in msg.iter().enumerate() {
        buf[16 + i * 2..16 + i * 2 + 2].copy_from_slice(&u16v.to_le_bytes());
    }
    let v = decode_struct(&buf, &ds).unwrap();
    assert_eq!(v["magic"], "0x4d4f4e31");
    assert_eq!(v["status"], 2);
    assert_eq!(v["tick"], 7);
    assert_eq!(v["message"], "MONITOR_ALERT");

    // UnixMs 解码单独小结构
    let ts = DataStructure {
        name: "Ts".into(),
        size: 8,
        fields: vec![FieldDef {
            offset: 0,
            name: "at_ms".into(),
            type_: ft("u64"),
            decode: Some(FieldDecode::UnixMs),
        }],
    };
    let mut tb = vec![0u8; 8];
    tb.copy_from_slice(&1_700_000_000_000u64.to_le_bytes());
    let v = decode_struct(&tb, &ts).unwrap();
    assert_eq!(v["at_ms"], "2023-11-14T22:13:20+00:00");
}

#[test]
fn decode_out_of_bounds_rejected() {
    let ds = DataStructure {
        name: "Bad".into(),
        size: 8,
        fields: vec![FieldDef {
            offset: 4,
            name: "v".into(),
            type_: ft("u64"),
            decode: None,
        }],
    };
    let buf = vec![0u8; 8];
    assert!(decode_struct(&buf, &ds).is_err());
}

#[test]
fn decode_revalidates_field_bounds_u64_truth() {
    // BUG-20261028-123 红证：u32 乘法回绕曾把 8×536870913 缩成 8 < 96
    // 溜过界检，随后 0 步长 + 巨量 with_capacity 可打爆进程。decode
    // 自含重验（u64 真值，不依赖调用方先跑 validate）必须 loud 拒绝。
    let ds = DataStructure {
        name: "Boom".into(),
        size: 96,
        fields: vec![FieldDef {
            offset: 0,
            name: "boom".into(),
            type_: ft("u64[536870913]"),
            decode: None,
        }],
    };
    let buf = vec![0u8; 96];
    let err = decode_struct(&buf, &ds).unwrap_err();
    assert!(err.contains("越界"), "{err}");
}

// ── M4：call_fn 三态 / hook 事件拉取解析面 ─────────────────────────────

#[test]
fn callfn_resp_three_states() {
    // Ok 态
    let o = parse_call_fn_resp(r#"{"ok":true,"ret":100,"elapsed_ms":12}"#).unwrap();
    assert_eq!(
        o,
        CallFnOutcome::Ok {
            ret: 100,
            elapsed_ms: 12
        }
    );
    // SEH 态
    let o = parse_call_fn_resp(r#"{"ok":false,"seh":"0xC0000005","elapsed_ms":3}"#).unwrap();
    assert_eq!(
        o,
        CallFnOutcome::Seh {
            code: 0xC000_0005,
            elapsed_ms: 3
        }
    );
    // Timeout 态
    let o = parse_call_fn_resp(r#"{"timeout":true,"waited_ms":1500}"#).unwrap();
    assert_eq!(o, CallFnOutcome::Timeout { waited_ms: 1500 });
    // payload 报错
    assert!(parse_call_fn_resp(r#"{"ok":false,"error":"addr 缺失"}"#).is_err());
    // 不可解析
    assert!(parse_call_fn_resp("garbage").is_err());
}

#[test]
fn callfn_outcome_json_notes() {
    let ok = CallFnOutcome::Ok {
        ret: 7,
        elapsed_ms: 1,
    }
    .to_json(0x1000);
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["ret"], 7);
    assert_eq!(ok["addr"], "0x1000");
    let seh = CallFnOutcome::Seh {
        code: 0xC000_0094,
        elapsed_ms: 2,
    }
    .to_json(0x1000);
    assert_eq!(seh["ok"], false);
    assert_eq!(seh["seh"], "0xC0000094");
    assert!(seh["note"].as_str().unwrap().contains("VEH"));
    let to = CallFnOutcome::Timeout { waited_ms: 99 }.to_json(0x1000);
    assert_eq!(to["timeout"], true);
    assert!(to["note"].as_str().unwrap().contains("单飞"));
}

#[test]
fn poll_events_resp_parse() {
    let resp = r#"{"ok":true,"count":2,"dropped":0,"events":[{"id":1,"seq":0,"tid":4012,"regs":[16,1005,0,0]},{"id":2,"seq":1,"tid":4012,"regs":[32,7,0,0]}]}"#;
    let (events, dropped) = parse_poll_events_resp(resp).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, 1);
    assert_eq!(events[0].seq, 0);
    assert_eq!(events[0].tid, 4012);
    assert_eq!(events[0].regs, [16, 1005, 0, 0]);
    assert_eq!(events[1].id, 2);
    assert_eq!(dropped, 0);
    // 空事件批 + 丢弃计数
    let (events, dropped) =
        parse_poll_events_resp(r#"{"ok":true,"count":0,"dropped":37,"events":[]}"#).unwrap();
    assert!(events.is_empty());
    assert_eq!(dropped, 37);
    // 报错面
    assert!(parse_poll_events_resp(r#"{"ok":false,"error":"poll 失败"}"#).is_err());
}

#[test]
fn json_u64_array_parse() {
    assert_eq!(
        json_u64_array(r#"{"regs":[1,2,3,4]}"#, "regs"),
        Some([1, 2, 3, 4])
    );
    // 不足 4 个余位补零；hex 形态
    assert_eq!(
        json_u64_array(r#"{"regs":[0x10,7]}"#, "regs"),
        Some([0x10, 7, 0, 0])
    );
    // 非整数元素 → None（v1 loud 拒绝面）
    assert_eq!(json_u64_array(r#"{"regs":[1.5,2]}"#, "regs"), None);
    assert_eq!(json_u64_array(r#"{"regs":["a"]}"#, "regs"), None);
    assert_eq!(json_u64_array(r#"{"regs":[]}"#, "regs"), Some([0, 0, 0, 0]));
    assert_eq!(json_u64_array(r#"{}"#, "regs"), None);
}
