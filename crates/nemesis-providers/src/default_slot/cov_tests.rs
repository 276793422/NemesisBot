// default_slot.rs 覆盖率补充测试（DefaultFollowingProvider 的委派面：
// chat_stream / default_model / name 146-152）。
//
// 纪律：不动全局槽（swap 是 tests.rs 专有的生命周期用例），本文件全部走
// 「显式钉扎模型名」路径 → 委派到 captured，结果与槽状态无关，避免与
// tests.rs 的槽读写并发竞争。

use super::*;
use crate::types::{ChatOptions, Message};

fn wrapper() -> Arc<dyn LLMProvider> {
    // captured 用 CodexProvider（default_model/name 有确定值；NullProvider
    // 的 default_model 是空串，不适合断言委派）。
    let captured = crate::codex::CodexProvider::new(crate::codex::CodexConfig {
        default_model: "captured-model".to_string(),
        ..Default::default()
    });
    default_following(Arc::new(captured), "captured-model", "prov/captured-model")
}

/// default_model / name 委派到 captured（150-152）。
#[test]
fn wrapper_delegates_default_model_and_name() {
    let w = wrapper();
    assert_eq!(w.default_model(), "captured-model");
    assert_eq!(w.name(), "codex");
}

/// 显式钉扎模型名经 chat 委派到 captured（codex lane 对死端口连接失败
/// → Timeout 诚实上报，证明请求确实走到了 captured）。
#[tokio::test]
async fn wrapper_chat_pinned_model_uses_captured() {
    let w = wrapper();
    let messages = vec![Message::text("user", "hi")];
    let err = w
        .chat(&messages, &[], "other-model", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, FailoverError::Timeout { ref provider, .. } if provider == "codex"),
        "预期 codex lane 的 Timeout，得到 {err:?}"
    );
}

/// chat_stream 委派（146-148）：captured 的默认实现回一条 Err 后收流
/// 关闭。
#[tokio::test]
async fn wrapper_chat_stream_pinned_model_delegates() {
    let w = wrapper();
    let messages = vec![Message::text("user", "hi")];
    let mut rx = w.chat_stream(&messages, &[], "other-model", &ChatOptions::default());
    let first = rx.recv().await;
    assert!(first.is_some(), "流必须先给一条 Err");
    assert!(first.unwrap().is_err());
}
