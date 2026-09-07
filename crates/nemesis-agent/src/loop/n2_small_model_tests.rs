//! N2 (devtool-upgrade 阶段 4)：`agents.small_model` 小模型杂务通道测试。
//! - `resolve_summary_provider`：未配置 → 主模型；已配置 → 小模型
//!   （provider 与模型名绑在同一槽位换，杜绝两者失配）。
//! - `compact_session` 端到端：摘要 LLM 调用落到小模型 provider，主模型零调用。
//! - `force_compression`（自动压缩路径）与 `force_compression_opts(false)`：
//!   即使小模型已配置也仍走主模型（质量敏感）。

use super::*;

// ---------- 录制型 mock：主 provider 走共享 CallLog（所有权进 AgentLoop
// 后测试仍可观察），小 provider 由测试持有 Arc 句柄直接观察 ----------

#[derive(Default)]
struct CallLog {
    models: std::sync::Mutex<Vec<String>>,
}

impl CallLog {
    fn models(&self) -> Vec<String> {
        self.models.lock().unwrap().clone()
    }
}

struct RecordingProvider {
    log: std::sync::Arc<CallLog>,
    response: String,
}

impl RecordingProvider {
    fn new(log: std::sync::Arc<CallLog>, response: &str) -> Self {
        Self {
            log,
            response: response.to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn chat(
        &self,
        model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.log.models.lock().unwrap().push(model.to_string());
        Ok(LlmResponse {
            content: self.response.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 把具体 provider Arc 收窄成 trait object 槽位（显式绑定，杜绝推断歧义）。
fn small_slot(p: Arc<RecordingProvider>, name: &str) -> (Arc<dyn LlmProvider>, String) {
    (p, name.to_string())
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "main-model".to_string(),
        // None：不带隐式 system turn，历史算术透明可断言。
        system_prompt: None,
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

// ---------- resolve_summary_provider ----------

#[tokio::test]
async fn resolve_summary_provider_defaults_to_main_without_small() {
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log.clone(), "MAIN")),
        test_config(),
    );
    // prefer_small 两态在未配置时都回退主模型（槽位名 = config.model）。
    let (_, main_via_small_pref) = al.resolve_summary_provider(true);
    let (_, main_via_auto) = al.resolve_summary_provider(false);
    assert_eq!(main_via_small_pref, "main-model");
    assert_eq!(main_via_auto, "main-model");
    assert!(main_log.models().is_empty());
}

#[tokio::test]
async fn resolve_summary_provider_prefers_configured_small() {
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log.clone(), "MAIN")),
        test_config(),
    );
    let small = Arc::new(RecordingProvider::new(
        std::sync::Arc::new(CallLog::default()),
        "SMALL",
    ));
    al.set_small_model(Some(small_slot(small, "cheap-mini")));

    let (_, via_small_pref) = al.resolve_summary_provider(true);
    assert_eq!(
        via_small_pref, "cheap-mini",
        "配置了 small_model 应路由到小模型"
    );
    let (_, via_auto) = al.resolve_summary_provider(false);
    assert_eq!(via_auto, "main-model", "prefer_small=false 永远主模型");
}

// ---------- compact_session（唯一生产消费点） ----------

#[tokio::test]
async fn compact_session_routes_summary_to_small_model() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let main_log = std::sync::Arc::new(CallLog::default());
    let mut al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log.clone(), "MAIN SUMMARY")),
        test_config(),
    );
    al.set_session_store(store.clone());
    let small_log = std::sync::Arc::new(CallLog::default());
    let small = Arc::new(RecordingProvider::new(
        small_log.clone(),
        "N2 SMALL SUMMARY",
    ));
    al.set_small_model(Some(small_slot(small, "cheap-mini")));

    store.get_or_create("agent:main:session:n2_route");
    for i in 0..5 {
        store.add_message(
            "agent:main:session:n2_route",
            "user",
            &format!("n2 msg {i}"),
        );
    }

    let receipt = al
        .compact_session("agent:main:session:n2_route")
        .await
        .unwrap();
    assert!(receipt.contains("已压缩"), "unexpected receipt: {receipt}");
    // 摘要来自小模型；主模型零调用（省钱契约的硬证据）。
    assert_eq!(
        store.get_summary("agent:main:session:n2_route"),
        "N2 SMALL SUMMARY"
    );
    assert_eq!(small_log.models(), vec!["cheap-mini".to_string()]);
    assert!(
        main_log.models().is_empty(),
        "main provider must not be called"
    );
}

// ---------- 自动压缩路径不受 small_model 影响 ----------

fn instance_with(n: usize) -> AgentInstance {
    let instance = AgentInstance::new(test_config());
    for i in 0..n {
        instance.add_user_message(&format!("n2 msg {i}"));
    }
    instance
}

#[tokio::test]
async fn force_compression_auto_path_stays_on_main() {
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log.clone(), "MAIN SUMMARY")),
        test_config(),
    );
    let small_log = std::sync::Arc::new(CallLog::default());
    let small = Arc::new(RecordingProvider::new(small_log.clone(), "SMALL SUMMARY"));
    al.set_small_model(Some(small_slot(small, "cheap-mini")));

    let instance = instance_with(10);
    al.force_compression(&instance).await;
    assert_eq!(
        instance.get_summary_cache().expect("cache set").text,
        "MAIN SUMMARY"
    );
    assert_eq!(main_log.models(), vec!["main-model".to_string()]);
    assert!(
        small_log.models().is_empty(),
        "auto path must not use small model"
    );
}

#[tokio::test]
async fn force_compression_opts_false_explicitly_uses_main() {
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log.clone(), "MAIN SUMMARY")),
        test_config(),
    );
    let small_log = std::sync::Arc::new(CallLog::default());
    let small = Arc::new(RecordingProvider::new(small_log.clone(), "SMALL SUMMARY"));
    al.set_small_model(Some(small_slot(small, "cheap-mini")));

    let instance = instance_with(10);
    al.force_compression_opts(&instance, false).await;
    assert_eq!(
        instance.get_summary_cache().expect("cache set").text,
        "MAIN SUMMARY"
    );
    assert!(small_log.models().is_empty());
}
