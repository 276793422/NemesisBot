//! http-send 请求编组回归锁（2026-11-03 空 body 实锤修复）。
//!
//! 跨界契约（双侧 abi.rs 钉死）：http-request → JSON
//! `{"method","url","headers":[[k,v],…],"body":b64|null}`。body 的 WIT
//! 形态是 `option<list<u8>>`，组件边界到达壳时是
//! `Val::Option(Some(Val::List([Val::U8,…])))`——泛型 [`val_to_json_value`]
//! 会把它落成数字数组，宿主 `as_str()` 失配 → body 静默丢失（demo 实锤：
//! 回帖全部记空 + 201）。本文件钉住专项编组 [`http_request_to_json`] 的
//! 字节精确往返、null/空体形态与失配诚实报错。

use crate::store::{http_request_to_json, val_to_json_value};
use base64::Engine as _;
use wasmtime::component::Val;

/// 构造 http-request record Val（body 由调用方给）。
fn req_val(body: Val) -> Val {
    Val::Record(vec![
        ("method".into(), Val::String("POST".into())),
        (
            "url".into(),
            Val::String("http://127.0.0.1:46395/rest/api/2/issue/PROJ-1/comment".into()),
        ),
        (
            "headers".into(),
            Val::List(vec![Val::Tuple(vec![
                Val::String("Content-Type".into()),
                Val::String("application/json".into()),
            ])]),
        ),
        ("body".into(), body),
    ])
}

/// body 字节：非 ASCII UTF-8 JSON（demo 丢的就是中文回帖正文）。
fn body_bytes() -> Vec<u8> {
    r#"{"body":"NemesisBot 流水线完成「示例任务」（看板单 NB-2）"}"#
        .as_bytes()
        .to_vec()
}

/// 有 body：字节精确 base64 往返（含非 ASCII UTF-8），其余字段原样穿透。
#[test]
fn body_bytes_encode_as_base64_roundtrip() {
    let payload = body_bytes();
    let v = req_val(Val::Option(Some(Box::new(Val::List(
        payload.iter().map(|b| Val::U8(*b)).collect(),
    )))));
    let json = http_request_to_json(&v).expect("marshal must succeed");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let got = parsed
        .get("body")
        .and_then(|b| b.as_str())
        .expect("body must be a base64 string");
    assert_eq!(
        got,
        base64::engine::general_purpose::STANDARD.encode(&payload),
        "body must be the exact base64 of the request bytes"
    );
    // 契约形态断言：不是数字数组（泛型序列化的病灶形态），不是明文。
    assert!(
        parsed.get("body").unwrap().is_string(),
        "body must be string, not array"
    );
    assert!(
        !got.contains("流水线"),
        "non-ASCII must not leak as plaintext"
    );
    assert_eq!(parsed["method"], "POST");
    assert_eq!(
        parsed["url"],
        "http://127.0.0.1:46395/rest/api/2/issue/PROJ-1/comment"
    );
    assert_eq!(parsed["headers"][0][0], "Content-Type");
    assert_eq!(parsed["headers"][0][1], "application/json");
}

/// 无 body（WIT None）：body 落 JSON null（宿主缺省 = 不带 body 的 GET 形态）。
#[test]
fn body_none_is_json_null() {
    let v = req_val(Val::Option(None));
    let json = http_request_to_json(&v).expect("marshal must succeed");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        parsed.get("body").unwrap().is_null(),
        "None body must marshal to null"
    );
}

/// 空 body（Some 空表）：base64 空串（宿主解码回零字节，POST 带 Content-Length: 0）。
#[test]
fn body_empty_list_is_empty_base64() {
    let v = req_val(Val::Option(Some(Box::new(Val::List(vec![])))));
    let json = http_request_to_json(&v).expect("marshal must succeed");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["body"], "");
}

/// record 形态失配：诚实报错（实例化期类型匹配后不该发生，防御性钉住）。
#[test]
fn non_record_param_fails_honestly() {
    let err = http_request_to_json(&Val::String("not a record".into())).unwrap_err();
    assert!(err.to_string().contains("http-request"), "{err}");
}

/// body 字段类型失配（非 option）：诚实报错。
#[test]
fn body_wrong_shape_fails_honestly() {
    let v = req_val(Val::String("plain".into()));
    let err = http_request_to_json(&v).unwrap_err();
    assert!(err.to_string().contains("option<list<u8>>"), "{err}");
}

/// body 元素类型失配（list 里混非 U8）：诚实报错。
#[test]
fn body_wrong_element_type_fails_honestly() {
    let v = req_val(Val::Option(Some(Box::new(Val::List(vec![Val::U32(1)])))));
    let err = http_request_to_json(&v).unwrap_err();
    assert!(err.to_string().contains("expected u8"), "{err}");
}

/// 反向钉病灶形态：泛型 val_to_json_value 对同一 body 落数字数组（正是宿主
/// as_str() 失配的根因）——回归警示，任何人把 handler 改回泛型时这
/// 条对照仍成立。
#[test]
fn generic_serializer_still_produces_array_for_bytes() {
    let v = req_val(Val::Option(Some(Box::new(Val::List(
        body_bytes().iter().map(|b| Val::U8(*b)).collect(),
    )))));
    let parsed = val_to_json_value(&v);
    assert!(
        parsed["body"].is_array(),
        "generic serializer emits number array (the bug shape)"
    );
}
