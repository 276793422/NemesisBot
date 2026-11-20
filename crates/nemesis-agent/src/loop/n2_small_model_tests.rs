//! (devtool-upgrade)：`agents.small_model` 小模型杂务通道测试。
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
            finish_reason: None,
            escalated_to: None,
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
        system_prompt_split: None,
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
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
    al.force_compression(&instance, "test-session").await;
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
    al.force_compression_opts(&instance, false, "test-session")
        .await;
    assert_eq!(
        instance.get_summary_cache().expect("cache set").text,
        "MAIN SUMMARY"
    );
    assert!(small_log.models().is_empty());
}

// ---------- chore_completion（杂务文本补全单一入口） ----------

#[tokio::test]
async fn chore_completion_routes_to_small_model_when_configured() {
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log.clone(), "MAIN")),
        test_config(),
    );
    let small_log = std::sync::Arc::new(CallLog::default());
    let small = Arc::new(RecordingProvider::new(
        small_log.clone(),
        "SMALL CHORE OUTPUT",
    ));
    al.set_small_model(Some(small_slot(small, "cheap-mini")));

    let out = al
        .chore_completion("monitor-summary", "总结这批变化".to_string(), 256)
        .await
        .expect("chore_completion ok");
    assert_eq!(out, "SMALL CHORE OUTPUT");
    // 摘要落小 provider（模型名随槽位），主模型零调用。
    assert_eq!(small_log.models(), vec!["cheap-mini".to_string()]);
    assert!(
        main_log.models().is_empty(),
        "main provider must not be called when small_model configured"
    );
}

#[tokio::test]
async fn chore_completion_falls_back_to_main_without_small() {
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(
            main_log.clone(),
            "MAIN CHORE OUTPUT",
        )),
        test_config(),
    );

    let out = al
        .chore_completion("monitor-summary", "总结这批变化".to_string(), 256)
        .await
        .expect("chore_completion ok");
    assert_eq!(out, "MAIN CHORE OUTPUT");
    assert_eq!(main_log.models(), vec!["main-model".to_string()]);
}

// ---------- C2 热生效（2026-11-20）：small_model 槽热换 ----------
//
// 假 resolver（不造真 provider）：按键原文返回带标记的 RecordingProvider，
// 观察面 = 槽的 name 与 resolver 调用计数。

/// 热换测试夹具：tempdir config.json + set_config_path（绑定即强制发布代）。
fn hot_swap_fixture(dir: &std::path::Path, small_ref: &str) -> AgentLoop {
    let cfg_path = dir.join("config.json");
    let body = format!(r#"{{"agents":{{"small_model":"{small_ref}"}}}}"#);
    std::fs::write(&cfg_path, body).unwrap();
    let main_log = std::sync::Arc::new(CallLog::default());
    let al = AgentLoop::new(
        Box::new(RecordingProvider::new(main_log, "MAIN")),
        test_config(),
    );
    al.set_config_path(cfg_path);
    al
}

/// 按键原文造新 provider 的假 resolver（name = `name-<ref>`，观察换槽）。
fn tagged_resolver(
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::sync::Arc<
    dyn Fn(&nemesis_config::Config, &str) -> Option<(Arc<dyn LlmProvider>, String)> + Send + Sync,
> {
    std::sync::Arc::new(move |_cfg: &nemesis_config::Config, r: &str| {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let small = Arc::new(RecordingProvider::new(
            std::sync::Arc::new(CallLog::default()),
            &format!("RESP-{r}"),
        ));
        Some(small_slot(small, &format!("name-{r}")))
    })
}

/// 未注入 resolver = 装配期形态冻结（旧语义）：改键 + 新代发布不动槽。
#[tokio::test]
async fn hot_swap_resolver_not_injected_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let al = hot_swap_fixture(dir.path(), "ref-a");
    let small = Arc::new(RecordingProvider::new(
        std::sync::Arc::new(CallLog::default()),
        "SMALL",
    ));
    al.set_small_model_with_ref(Some(small_slot(small, "name-ref-a")), "ref-a");

    // 改盘换键 → 新代发布 → sync 无 resolver 直接返回。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"agents":{"small_model":"ref-b"}}"#,
    )
    .unwrap();
    al.check_config_reload();
    let (p, name) = al.small_model_slot().expect("槽应保持");
    assert_eq!(name, "name-ref-a", "未注入 resolver = 形态冻结");
    assert!(std::sync::Arc::strong_count(&p) >= 1, "provider 仍被槽持有");
}

/// 键变化 → 新代发布 → resolver 现构造换槽（provider+name 绑定换）。
#[tokio::test]
async fn hot_swap_picks_up_key_change() {
    let dir = tempfile::tempdir().unwrap();
    let al = hot_swap_fixture(dir.path(), "ref-a");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    al.set_small_model_resolver(tagged_resolver(calls.clone()));
    let small = Arc::new(RecordingProvider::new(
        std::sync::Arc::new(CallLog::default()),
        "SMALL",
    ));
    al.set_small_model_with_ref(Some(small_slot(small, "name-ref-a")), "ref-a");

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"agents":{"small_model":"ref-b"}}"#,
    )
    .unwrap();
    al.check_config_reload();
    let (_, name) = al.small_model_slot().expect("热换后槽应在场");
    assert_eq!(name, "name-ref-b", "键变化 → resolver 重建换槽");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "resolver 恰好调用一次"
    );
}

/// 键删除/清空 → 清槽（杂务回落主模型）。
#[tokio::test]
async fn hot_swap_clears_slot_when_key_removed() {
    let dir = tempfile::tempdir().unwrap();
    let al = hot_swap_fixture(dir.path(), "ref-a");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    al.set_small_model_resolver(tagged_resolver(calls.clone()));
    let small = Arc::new(RecordingProvider::new(
        std::sync::Arc::new(CallLog::default()),
        "SMALL",
    ));
    al.set_small_model_with_ref(Some(small_slot(small, "name-ref-a")), "ref-a");

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(dir.path().join("config.json"), r#"{"agents":{}}"#).unwrap();
    al.check_config_reload();
    assert!(
        al.small_model_slot().is_none(),
        "键删除 → 清槽（诚实回退主模型）"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// resolver 失败（解析/构造失败 → None）= 保留旧槽 + warn（坏配置不打掉
/// 好槽；下轮新代发布重试）。
#[tokio::test]
async fn hot_swap_failure_keeps_old_slot() {
    let dir = tempfile::tempdir().unwrap();
    let al = hot_swap_fixture(dir.path(), "ref-a");
    al.set_small_model_resolver(std::sync::Arc::new(
        |_cfg: &nemesis_config::Config, _r: &str| -> Option<(Arc<dyn LlmProvider>, String)> {
            None // 模拟 resolve/create 失败
        },
    ));
    let small = Arc::new(RecordingProvider::new(
        std::sync::Arc::new(CallLog::default()),
        "SMALL",
    ));
    al.set_small_model_with_ref(Some(small_slot(small.clone(), "name-ref-a")), "ref-a");

    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"agents":{"small_model":"ref-broken"}}"#,
    )
    .unwrap();
    al.check_config_reload();
    let (_, name) = al.small_model_slot().expect("失败保旧：槽仍在场");
    assert_eq!(name, "name-ref-a", "resolver 失败 → 保留旧槽");
    let _ = small; // 句柄存活证明旧 provider 未被清
}

/// 新代发布但键未变 → 零成本原样返回（resolver 不调、槽对象不动——
/// Arc ptr_eq 证同一实例）。
#[tokio::test]
async fn hot_swap_key_unchanged_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let al = hot_swap_fixture(dir.path(), "ref-a");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    al.set_small_model_resolver(tagged_resolver(calls.clone()));
    let small = Arc::new(RecordingProvider::new(
        std::sync::Arc::new(CallLog::default()),
        "SMALL",
    ));
    al.set_small_model_with_ref(Some(small_slot(small, "name-ref-a")), "ref-a");
    let (before, _) = al.small_model_slot().unwrap();

    // 无关键变化（新键）→ 新代发布 → 键比对相等 → 不动槽。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"agents":{"small_model":"ref-a"},"unrelated":1}"#,
    )
    .unwrap();
    al.check_config_reload();
    let (after, name) = al.small_model_slot().unwrap();
    assert_eq!(name, "name-ref-a");
    assert!(
        std::sync::Arc::ptr_eq(&before, &after),
        "键未变 → 槽对象原样（零成本）"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "键未变 → resolver 不调用"
    );
}
