//! BUG②（2026-11-05）死房间补拉 hint：`broadcast_chat_dirty_hint` 扇出单测。
//!
//! 语义：目标连接房间消亡（页面刷新）导致投递失败时，向**全部**活跃房间
//! 扇出 `{type:"push", module:"chat", cmd:"chat_dirty", data:{session_id,
//! reason:"delivery_missed"}}`；帧未落环（hint 本身不带 seq，丢了无妨——
//! 它只是补拉触发器），走 droppable（lo）通道。

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::protocol::ProtocolMessage;
use crate::server::broadcast_chat_dirty_hint;
use crate::session::SessionManager;
use crate::websocket_handler::SendQueue;

/// 挂一个测试 SendQueue 到 session（真实 WS sink 的测试替身），返回 lo
/// 通道接收端——hint 扇出走 broadcast_droppable（低频小载荷尽力而为）。
fn attach_fake_queue(manager: &SessionManager, session_id: &str) -> mpsc::Receiver<Vec<u8>> {
    let (queue, _hi_rx, lo_rx, _done_tx) = SendQueue::test_channels(16);
    manager.set_send_queue(session_id, Arc::new(queue));
    lo_rx
}

#[tokio::test]
async fn hint_fans_out_to_live_rooms_with_correct_payload() {
    let manager = SessionManager::with_default_timeout();
    // 死房间：会话在场但无 send queue（连接已断——broadcast 失败的形态）。
    let _dead = manager.create_session();
    // 活房间：刷新后的新连接（正在看该会话的面板）。
    let live = manager.create_session();
    let mut lo_rx = attach_fake_queue(&manager, &live.id);

    broadcast_chat_dirty_hint(&manager, "agent:main:session:s1");

    let raw = lo_rx.recv().await.expect("live room must receive hint");
    let msg = ProtocolMessage::parse(&raw).expect("hint frame must be valid protocol message");
    assert_eq!(msg.msg_type, "push");
    assert_eq!(msg.module, "chat");
    assert_eq!(msg.cmd, "chat_dirty");
    let data = msg.data.expect("hint frame must carry payload");
    assert_eq!(data["session_id"], "s1");
    assert_eq!(data["reason"], "delivery_missed");
}

#[tokio::test]
async fn hint_strips_agent_prefix_and_passes_bare_key_through() {
    let manager = SessionManager::with_default_timeout();
    let live = manager.create_session();
    let mut lo_rx = attach_fake_queue(&manager, &live.id);

    // 前缀形态剥出 agent 会话 id。
    broadcast_chat_dirty_hint(&manager, "agent:main:session:s2");
    let raw = lo_rx.recv().await.expect("prefixed key hint must arrive");
    let msg = ProtocolMessage::parse(&raw).unwrap();
    assert_eq!(msg.data.unwrap()["session_id"], "s2");

    // 无前缀形态原样透传（调用方已解析过的形态）。
    broadcast_chat_dirty_hint(&manager, "s3");
    let raw = lo_rx.recv().await.expect("bare key hint must arrive");
    let msg = ProtocolMessage::parse(&raw).unwrap();
    assert_eq!(msg.data.unwrap()["session_id"], "s3");
}
