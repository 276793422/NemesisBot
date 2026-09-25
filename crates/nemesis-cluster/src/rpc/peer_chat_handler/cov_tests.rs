// peer_chat_handler.rs 覆盖率补充测试（task_receive_hook 有附录臂 /
// send_callback 携带 usage 字段 + 重试耗尽臂）。
//
// 豁免：无（本文件两臂均可确定性驱动；重试 backoff 经 tokio 时间自动
// 推进瞬时完成）。

use super::*;
use std::sync::Arc;

/// 恒返附录的钩子：驱动 Some(appendix) 臂。
struct AppendHook;
impl TaskReceiveHook for AppendHook {
    fn on_task_received(&self, _task_id: &str, _payload: &serde_json::Value) -> Option<String> {
        Some("[cov appendix]".into())
    }
}

/// 永远解析不到对端的 resolver → 回调每次尝试都立即失败。
struct DeadResolver;
impl crate::rpc::client::PeerResolver for DeadResolver {
    fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        None
    }
    fn get_local_interfaces(&self) -> Vec<crate::rpc::client::LocalNetworkInterface> {
        Vec::new()
    }
    fn get_node_id(&self) -> String {
        "cov-self".into()
    }
}

/// task_receive_hook 命中：附录追加进任务内容（339）。
#[tokio::test]
async fn task_receive_hook_appendix_arm() {
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_task_receive_hook(Arc::new(AppendHook));

    let payload = serde_json::json!({
        "content": "hello",
        "task_id": "cov-hook-task",
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted", "{:?}", ack.status);
}

/// send_callback：usage 字段落 payload（708）+ 全部重试失败逐次 warn
/// （745）→ 最终 false（tokio paused time 令 backoff 瞬时推进）。
#[tokio::test(start_paused = true)]
async fn send_callback_usage_field_and_retry_exhaustion() {
    let client = RpcClient::with_resolver(Arc::new(DeadResolver));

    let ok = send_callback(
        Some(&client),
        "cov-src",
        "cov-self",
        "cov-task-1",
        "completed",
        "payload body",
        "",
        Some(serde_json::json!({"input": 1, "output": 2})),
        None,
    )
    .await;

    assert!(!ok, "解析不到对端 → 重试耗尽返回 false");
}
