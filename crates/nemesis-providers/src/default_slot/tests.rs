//! default_slot 单测：委派/钉扎路由 + 事故剧本回归（2026-09-22）。
//!
//! 槽是进程级单例，本文件全部断言收进**单个** #[test]（同一测试二进制内
//! 并行测试会互踩全局槽）。事故剧本：无 key 启动 → wrapper 捕获
//! NullProvider → chat 诚实报「装配失败」→ set_default 热切（swap 真实
//! provider）→ **同一 wrapper 实例** chat 立即恢复，全程不重建 loop。

use super::*;
use crate::null_provider::null_provider;
use crate::types::LLMResponse;

/// 记录收到的 model 名并回显 `{tag}:{model}` 的哑 provider。
struct StubProvider {
    tag: &'static str,
}

#[async_trait::async_trait]
impl LLMProvider for StubProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Ok(LLMResponse {
            content: format!("{}:{}", self.tag, model),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn default_model(&self) -> &str {
        self.tag
    }

    fn name(&self) -> &str {
        self.tag
    }
}

fn stub(tag: &'static str) -> Arc<dyn LLMProvider> {
    Arc::new(StubProvider { tag })
}

async fn chat_model(p: &dyn LLMProvider, model: &str) -> String {
    p.chat(&[], &[], model, &ChatOptions::default())
        .await
        .unwrap()
        .content
}

/// 全场景单函数串行：钉扎/委派/空名/多次切换/Null 恢复。
#[tokio::test]
async fn default_slot_routing_matrix_and_incident_recovery() {
    // 基线：槽未安装（本测试是该测试二进制内唯一写点）→ wrapper 全走捕获。
    assert!(current().is_none(), "前置：槽应尚未被本二进制触碰");
    let captured = stub("cap");
    let w = default_following(Arc::clone(&captured), "glm-flash", "zhipu/glm-flash");
    // 钉扎名透传捕获 provider，模型名不劫持。
    assert_eq!(
        chat_model(w.as_ref(), "other/model").await,
        "cap:other/model"
    );
    // 槽未装时空名兜底捕获名。
    assert_eq!(chat_model(w.as_ref(), "").await, "cap:glm-flash");

    // 事故剧本：捕获 NullProvider（启动无 key）→ chat 报「装配失败」。
    let broken = default_following(
        null_provider("no API key configured for provider: zhipu"),
        "glm-flash",
        "zhipu/glm-flash",
    );
    let err = broken
        .chat(&[], &[], "glm-flash", &ChatOptions::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("启动时装配失败"), "got: {err}");

    // set_default 热切（swap）→ **同一 wrapper 实例**立即恢复，且两种名字
    // 形态（捕获名/当前名、去前缀/带前缀）与空名都委派到新 provider +
    // 新模型名。
    swap(stub("sw1"), "glm-flash", "zhipu/glm-flash");
    assert_eq!(
        chat_model(broken.as_ref(), "glm-flash").await,
        "sw1:glm-flash"
    );
    assert_eq!(
        chat_model(broken.as_ref(), "zhipu/glm-flash").await,
        "sw1:glm-flash"
    );

    // 换默认模型（用户切到别的厂商）→ 捕获旧名的消费者继续跟随新默认。
    swap(stub("sw2"), "deepseek-v3", "deepseek/deepseek-v3");
    assert_eq!(
        chat_model(broken.as_ref(), "glm-flash").await,
        "sw2:deepseek-v3"
    );
    assert_eq!(
        chat_model(broken.as_ref(), "deepseek-v3").await,
        "sw2:deepseek-v3"
    );
    assert_eq!(
        chat_model(broken.as_ref(), "deepseek/deepseek-v3").await,
        "sw2:deepseek-v3"
    );
    // SSE 路径：传入的就是当前默认名（活槽文本）→ 委派，不误判钉扎。
    assert_eq!(
        chat_model(broken.as_ref(), "deepseek-v3").await,
        "sw2:deepseek-v3"
    );
    // 真钉扎不受热切影响：非默认名走捕获 provider、模型名原样。
    assert_eq!(
        chat_model(w.as_ref(), "other/model").await,
        "cap:other/model"
    );

    // chat_stream 同一判定：委派臂拿到新 provider 的流，钉扎臂走捕获。
    let mut rx = broken.chat_stream(&[], &[], "glm-flash", &ChatOptions::default());
    // stub 未实现 chat_stream → 委派到 trait 默认实现，报「not implemented」
    // 且 provider 名是 sw2（证明路由到了新 provider 而非捕获）。
    let chunk = rx.recv().await.unwrap().unwrap_err().to_string();
    assert!(
        chunk.contains("sw2"),
        "chat_stream 应委派到槽内 provider: {chunk}"
    );
}
