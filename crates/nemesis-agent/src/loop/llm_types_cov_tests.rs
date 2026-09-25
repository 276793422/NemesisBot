//! llm_types 覆盖率补充（wave5）：observer 消息值提取的图片 base64 脱敏、
//! chat_call_bounded 的零超时直通臂、wait_estop_engaged 的 false→true 唤醒臂。

use async_trait::async_trait;

use super::AgentLoop;
use super::llm_types::{
    LlmMessage, LlmProvider, LlmResponse, OBSERVER_IMAGE_DATA_MARKER, observer_msg_values,
};

/// 恒定返回 Ok 的桩 provider。
struct InstantOkProvider;

#[async_trait]
impl LlmProvider for InstantOkProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn msg_with_image(data: &str) -> LlmMessage {
    LlmMessage {
        role: "user".to_string(),
        content: "看图".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: vec![crate::image_attach::LlmImage {
            path: "C:/x/a.png".to_string(),
            media_type: "image/png".to_string(),
            data: data.to_string(),
        }],
    }
}

/// 非空 data 在发射源被替换为省略标记；path/media_type 保留（46-56）。
#[test]
fn observer_msg_values_masks_nonempty_image_data() {
    let vals = observer_msg_values(&[msg_with_image("QUJD")]);
    let imgs = vals[0]
        .get("images")
        .and_then(|v| v.as_array())
        .expect("images 数组存在");
    assert_eq!(imgs.len(), 1);
    assert_eq!(
        imgs[0].get("data").and_then(|d| d.as_str()),
        Some(OBSERVER_IMAGE_DATA_MARKER),
        "base64 必须在发射源被脱敏"
    );
    assert_eq!(
        imgs[0].get("path").and_then(|p| p.as_str()),
        Some("C:/x/a.png")
    );
    assert_eq!(
        imgs[0].get("media_type").and_then(|m| m.as_str()),
        Some("image/png")
    );
}

/// 空 data 不替换（has_data 守卫 false 侧），无图消息连 images 键都不出现。
#[test]
fn observer_msg_values_leaves_empty_data_and_imageless_msgs_untouched() {
    let vals = observer_msg_values(&[
        msg_with_image(""),
        LlmMessage {
            role: "assistant".to_string(),
            content: "纯文本".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
    ]);
    assert_eq!(vals[0]["images"][0]["data"], "");
    assert!(vals[1].get("images").is_none(), "空图列表不序列化");
}

/// call_timeout = 0 → 关闭超时预算，直通 provider future（121）。
#[tokio::test]
async fn chat_call_bounded_zero_timeout_passes_through() {
    let provider: std::sync::Arc<dyn LlmProvider> = std::sync::Arc::new(InstantOkProvider);
    let resp = AgentLoop::chat_call_bounded(0, &provider, "m", Vec::new(), None, Vec::new())
        .await
        .unwrap();
    assert_eq!(resp.content, "ok");
}

/// 订阅时 false → changed() 环等待，翻转成 true 后 return（149）。
#[tokio::test]
async fn wait_estop_engaged_returns_when_flag_flips_true() {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let sender = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let _ = tx.send(true);
    });
    AgentLoop::wait_estop_engaged(Some(&mut rx)).await;
    assert!(*rx.borrow(), "唤醒时标志应为 true");
    sender.await.unwrap();
}
