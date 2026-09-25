//! ws_codec.rs AGT 覆盖率批次（2026-09-25）。与 relay/tests 内联 ws_codec_tests
//! 互补，聚焦仍缺的确定性臂：
//! - encode：Binary 小载荷臂（0x2 opcode）+ 扩展 16 位长度编码臂（126，
//!   109-110——既有测试只走过 64 位 127 臂）
//! - parse：扩展 16 位长度的截断 None（45）/ 完整读取（44/47/48）；
//!   64 位长度的截断 None（51）；FIN=0 中间分片占位 Text（78）；
//!   text opcode 载荷非法 UTF-8 → 空 Binary 兜底（88）；Pong（92）；
//!   保留 opcode → Binary 载荷兜底（94）
//!
//! 结构性豁免（见报告）：
//! - 126-128（build_frame 的 server_role 分支）：全仓唯一调用点
//!   encode_client_frame 恒传 client_role=true，该分支无任何调用方可达，
//!   属死臂（观测记录在案，不改生产代码）。

use super::{encode_client_frame, parse_server_frame};
use axum::extract::ws::Message;

/// 拼一个 unmasked server 帧（FIN + opcode + len + 载荷）。
fn server_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![if fin { 0x80 | opcode } else { opcode }];
    f.push(payload.len() as u8);
    f.extend_from_slice(payload);
    f
}

// ---------------------------------------------------------------------------
// encode：Binary 臂 + 扩展 16 位长度编码
// ---------------------------------------------------------------------------

#[test]
fn agt_encode_binary_small_and_extended16() {
    // 小 Binary：opcode 0x2 + mask 位 + 裸长度。
    let frame = encode_client_frame(&Message::Binary(vec![1u8, 2, 3].into()));
    assert_eq!(frame[0], 0x82, "FIN + binary opcode");
    assert_eq!(frame[1] & 0x80, 0x80, "client 帧必须带 mask 位");
    assert_eq!(frame[1] & 0x7F, 3, "裸长度 = 3");

    // 126..=u16::MAX 载荷：126 编码 + 2 字节 BE 长度（109-110）。
    let payload = vec![9u8; 200];
    let frame = encode_client_frame(&Message::Binary(payload.clone().into()));
    assert_eq!(frame[1] & 0x7F, 126, "200 字节必须走 126 编码");
    assert_eq!(&frame[2..4], &200u16.to_be_bytes(), "BE 16 位长度");
    // 掩码往返不损载荷。
    let (msg, consumed) = parse_server_frame(&frame).unwrap();
    assert_eq!(consumed, frame.len());
    match msg {
        Message::Binary(b) => assert_eq!(b.to_vec(), payload),
        other => panic!("期望 Binary，得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// parse：扩展长度截断 / 64 位截断 / FIN=0 占位
// ---------------------------------------------------------------------------

#[test]
fn agt_parse_extended16_truncated_and_full() {
    // 126 编码但长度字节不齐（< 4 字节头）→ None。
    assert!(parse_server_frame(&[0x81, 126, 0x00]).is_none());
    // 完整 16 位长度帧：读长度 + 推进 pos（44/47/48）。
    let mut f = vec![0x81u8, 126];
    f.extend_from_slice(&4u16.to_be_bytes());
    f.extend_from_slice(b"abcd");
    let (msg, consumed) = parse_server_frame(&f).unwrap();
    assert_eq!(consumed, f.len());
    match msg {
        Message::Text(t) => assert_eq!(t.as_str(), "abcd"),
        other => panic!("期望 Text，得到 {other:?}"),
    }
}

#[test]
fn agt_parse_len127_truncated_returns_none() {
    // 127 编码但不足 8 字节长度 → None（51）。
    let f = [0x82u8, 127, 0x00, 0x00, 0x00];
    assert!(parse_server_frame(&f).is_none());
}

#[test]
fn agt_parse_fin0_fragment_yields_placeholder_text() {
    // 中间分片（FIN=0）：占位空 Text + 完整消耗（78，流不卡死）。
    let f = server_frame(0x1, false, b"hello");
    let (msg, consumed) = parse_server_frame(&f).unwrap();
    assert_eq!(consumed, f.len());
    match msg {
        Message::Text(t) => assert_eq!(t.as_str(), "", "占位帧必须为空 Text"),
        other => panic!("期望占位 Text，得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// parse：opcode 变体兜底
// ---------------------------------------------------------------------------

#[test]
fn agt_parse_text_invalid_utf8_falls_back_to_empty_binary() {
    // text opcode + 非法 UTF-8 载荷 → 空 Binary 兜底（88）。
    let f = server_frame(0x1, true, &[0xFF, 0xFE]);
    let (msg, consumed) = parse_server_frame(&f).unwrap();
    assert_eq!(consumed, f.len());
    match msg {
        Message::Binary(b) => assert!(b.is_empty(), "兜底必须为空 Binary"),
        other => panic!("期望空 Binary，得到 {other:?}"),
    }
}

#[test]
fn agt_parse_pong_and_reserved_opcode() {
    // Pong（92）。
    let f = server_frame(0xA, true, &[0xAA, 0xBB]);
    let (msg, _) = parse_server_frame(&f).unwrap();
    match msg {
        Message::Pong(p) => assert_eq!(p.to_vec(), vec![0xAA, 0xBB]),
        other => panic!("期望 Pong，得到 {other:?}"),
    }

    // 保留 opcode（0x3）→ Binary 载荷兜底（94）。
    let f = server_frame(0x3, true, &[7]);
    let (msg, _) = parse_server_frame(&f).unwrap();
    match msg {
        Message::Binary(b) => assert_eq!(b.to_vec(), vec![7]),
        other => panic!("期望 Binary 兜底，得到 {other:?}"),
    }
}
