// handlers/llm.rs 覆盖率补充测试（provider 成功臂：Ok 内容 → 成功
// HandleResult）。
//
// 豁免：无。

use super::*;
use std::sync::Arc;

/// 恒成功的 provider：驱动 Ok(content) 臂。
struct OkProvider;
impl LlmProvider for OkProvider {
    fn chat_completion(
        &self,
        model: &str,
        _messages: &[serde_json::Value],
        _options: &serde_json::Value,
    ) -> Result<String, String> {
        Ok(format!("cov reply via {model}"))
    }
}

/// provider 成功路径：Ok → success=true + content 回填（116/127）。
#[test]
fn provider_ok_returns_success_handle_result() {
    let handler = LlmProxyHandler::with_provider("node-cov".into(), Arc::new(OkProvider));

    let result = handler.handle(serde_json::json!({
        "messages": [{"role": "user", "content": "hi"}],
    }));

    assert!(result.success, "{result:?}");
    assert_eq!(result.response["content"], "cov reply via default");
    assert!(result.error.is_none());
}
