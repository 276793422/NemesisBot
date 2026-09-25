//! relay/server.rs AGT 覆盖率批次（2026-09-25）。与 relay/tests.rs 互补，
//! 聚焦仍缺的确定性臂：
//! - cluster_frame_sink 槽的注入/取用（二期上行出口）
//! - authenticate_device 的「接入门未配置」「中继开关已关闭」「空 name
//!   回落 node_id」臂
//! - send_to_device 的队列满 / 通道关闭臂
//! - access_check 的下行投递失败臂（队列满 → 摘 waiter 诚实报错）+ 超时臂
//!   （5s 实等一次）
//! - maintenance_tick 的关闭早退 + 超龄 conn 清理
//! - handle_control_frame 的数据面帧防御吞掉臂 + decode_or_warn 失败臂
//!
//! 结构性豁免（见报告）：access_check 的 `Ok(Err(_))` 臂（oneshot 发送端
//! 只在 deliver 时被消费，无「drop 而不发」路径）；encode_or_none 的失败
//! 臂（serde_json 序列化 BridgeFrame 实际不可失败，防御性 ERROR 分支）。

use super::{RelayServer, decode_or_warn, encode_or_none, handle_control_frame};
use crate::relay::cluster_frame::ClusterFrameSink;
use crate::relay::protocol::{ACCESS_CHECK_TIMEOUT_SECS, BridgeFrame};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// cluster_frame_sink 槽
// ---------------------------------------------------------------------------

struct AgtSink;
impl ClusterFrameSink for AgtSink {
    fn on_cluster_frame(
        &self,
        _from_device: &str,
        _payload: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send + '_>>
    {
        Box::pin(async { None })
    }
}

#[test]
fn agt_cluster_frame_sink_slot_roundtrip() {
    let relay = RelayServer::new("tok".to_string(), true);
    assert!(relay.cluster_frame_sink().is_none(), "未注入 = None");
    relay.set_cluster_frame_sink(Arc::new(AgtSink));
    assert!(relay.cluster_frame_sink().is_some(), "注入后取用命中");
}

// ---------------------------------------------------------------------------
// authenticate_device：门未配置 / 开关关闭 / 空 name 回落
// ---------------------------------------------------------------------------

#[test]
fn agt_authenticate_gate_disabled_and_empty_name() {
    // ① 空 token = 接入门未配置（fail-closed，鉴权最前置）。
    let relay = RelayServer::new(String::new(), true);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let err = relay
        .authenticate_device("tok", "n1", "名", "v", None, tx)
        .expect_err("门未配置必须拒");
    assert!(err.contains("接入门未配置"), "{err}");

    // ② 开关关闭（token 对、门开着也不行）。
    let relay = RelayServer::new("tok".to_string(), true);
    relay.set_enabled(false);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let err = relay
        .authenticate_device("tok", "n1", "名", "v", None, tx)
        .expect_err("开关关闭必须拒");
    assert!(err.contains("中继开关已关闭"), "{err}");

    // ③ 空 name：登记成功，显示名回落 node_id。
    let relay = RelayServer::new("tok".to_string(), true);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    relay
        .authenticate_device("tok", "bare-node", "", "v", None, tx)
        .expect("注册应成功");
    let devices = relay.list_devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "bare-node", "空 name 回落 node_id");
}

// ---------------------------------------------------------------------------
// send_to_device：队列满 / 通道关闭
// ---------------------------------------------------------------------------

#[test]
fn agt_send_to_device_full_and_closed() {
    let relay = RelayServer::new("tok".to_string(), true);
    // 容量 2：authenticate 触发 broadcast_member_sync 先占 1 格
    // （设备上线 = 摘要变化 → 立即广播），再留 1 格给测试帧。
    let (tx, rx) = tokio::sync::mpsc::channel::<BridgeFrame>(2);
    relay
        .authenticate_device("tok", "n1", "名", "v", None, tx)
        .expect("注册应成功");

    // 注册帧入队，第二帧入队，第三帧队列满 → false（丢弃 + WARN）。
    assert!(relay.send_to_device("n1", BridgeFrame::Pong, 10));
    assert!(
        !relay.send_to_device("n1", BridgeFrame::Pong, 10),
        "满队列必须丢弃"
    );

    // 接收端 drop → 通道关闭 → false。
    drop(rx);
    assert!(
        !relay.send_to_device("n1", BridgeFrame::Pong, 10),
        "关闭通道必须失败"
    );

    // 表外设备 → false。
    assert!(!relay.send_to_device("ghost", BridgeFrame::Pong, 10));
}

// ---------------------------------------------------------------------------
// access_check：下行投递失败 + 超时
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_access_check_send_failure_and_timeout() {
    // ① 设备在表但下行队列满 → 投递失败：摘 waiter + 诚实报错。
    let relay = RelayServer::new("tok".to_string(), true);
    // 容量 2：注册广播 MemberSync 占 1 格，手工帧占 1 格 → 满企。
    let (tx, _rx) = tokio::sync::mpsc::channel::<BridgeFrame>(2);
    relay
        .authenticate_device("tok", "n1", "名", "v", None, tx)
        .expect("注册应成功");
    // 填满下行队列（无接收端消费）。
    assert!(relay.send_to_device("n1", BridgeFrame::Pong, 0));
    assert!(
        !relay.send_to_device("n1", BridgeFrame::Pong, 0),
        "队列此刻必须已满"
    );
    let err = relay.access_check("n1", "abcd").await.expect_err("");
    assert!(err.contains("设备不在线"), "{err}");

    // ② 设备在表、通道健康但无回执 → 超时 Ok(false)（真等 5s）。
    let (tx, rx) = tokio::sync::mpsc::channel::<BridgeFrame>(16);
    relay
        .authenticate_device("tok", "n2", "名", "v", None, tx)
        .expect("注册应成功");
    let started = Instant::now();
    let ok = relay.access_check("n2", "abcd").await.expect("超时非 Err");
    assert!(!ok, "无回执 = 校验不通过");
    assert!(
        started.elapsed() >= Duration::from_secs(ACCESS_CHECK_TIMEOUT_SECS),
        "必须等满超时窗"
    );
    drop(rx);
}

// ---------------------------------------------------------------------------
// maintenance_tick：关闭早退 + 超龄 conn 清理
// ---------------------------------------------------------------------------

#[test]
fn agt_maintenance_tick_disabled_and_conn_expiry() {
    // 关闭态：单行早退（设备表已被 set_enabled(false) 清空）。
    let relay = RelayServer::new("tok".to_string(), true);
    relay.set_enabled(false);
    relay.maintenance_tick();

    // 超龄 conn：CONN_MAX_AGE_SECS(600s) 前登记的泄漏 conn 被清。
    let relay = Arc::new(RelayServer::new("tok".to_string(), true));
    let _rx = relay.register_conn(7, "n1", 1);
    assert!(relay.conns.contains_key(&7), "登记生效");
    // 直接拨旧 created_at（子模块可见私有字段——同 backdate 家族语义）。
    if let Some(mut c) = relay.conns.get_mut(&7) {
        c.created_at = Instant::now() - Duration::from_secs(601);
    }
    relay.maintenance_tick();
    assert!(!relay.conns.contains_key(&7), "超龄 conn 必须被清");

    // 新鲜 conn 存活。
    let _rx2 = relay.register_conn(8, "n1", 1);
    relay.maintenance_tick();
    assert!(relay.conns.contains_key(&8), "新鲜 conn 不清");
}

// ---------------------------------------------------------------------------
// handle_control_frame 数据面防御臂 + decode_or_warn 失败臂
// ---------------------------------------------------------------------------

#[test]
fn agt_control_frame_dataplane_defense_and_decode_warn() {
    let relay = RelayServer::new("tok".to_string(), true);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    relay
        .authenticate_device("tok", "n1", "名", "v", None, tx)
        .expect("注册应成功");

    // 数据面帧不进控制分发——防御性吞掉，不炸不误路由。
    handle_control_frame(
        BridgeFrame::ConnData {
            conn_id: 7,
            seq: 1,
            data_b64: "aGk=".to_string(),
            fin: false,
        },
        &relay,
        "n1",
    );
    handle_control_frame(
        BridgeFrame::ConnOpen {
            conn_id: 7,
            target: "/api/x".to_string(),
        },
        &relay,
        "n1",
    );

    // 解码失败 → WARN + None。
    assert!(decode_or_warn("this is not json").is_none());
    assert!(decode_or_warn("").is_none());
    // 编码正常帧恒 Some（失败臂不可达，见头注豁免）。
    assert!(encode_or_none(&BridgeFrame::Pong).is_some());
}
