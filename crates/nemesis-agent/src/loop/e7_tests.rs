//! E7 (devtool-upgrade 阶段 5)：会话标题自动生成测试。
//!
//! - `sanitize_generated_title`：首行/剥引号/截 24 字（CJK 边界）/空 → None。
//! - `spawn_session_title_job` 端到端：小模型生成 → sidecar meta 落盘。
//! - 跳过守卫：未配置小模型 / 手动改名 / 已有真实标题 → 不 spawn；
//!   默认占位符 → 生成并覆盖。

use super::*;
use crate::chat_log::{self, write_session_meta_manual};

// ---------- 录制型 mock：返回预设标题文本，记录收到的模型名 ----------

/// 主 provider 占位（E7 路径不触碰它；断言它未被调用即 main 零烧）。
struct E7NoopProvider;

#[async_trait]
impl LlmProvider for E7NoopProvider {
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

struct TitleProvider {
    response: std::sync::Mutex<Option<String>>,
    models: std::sync::Mutex<Vec<String>>,
}

impl TitleProvider {
    fn new(response: &str) -> Self {
        Self {
            response: std::sync::Mutex::new(Some(response.to_string())),
            models: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn models(&self) -> Vec<String> {
        self.models.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for TitleProvider {
    async fn chat(
        &self,
        model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.models.lock().unwrap().push(model.to_string());
        let resp = self.response.lock().unwrap().take().unwrap_or_default();
        Ok(LlmResponse {
            content: resp,
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn e7_uniq_key(tag: &str) -> String {
    format!(
        "test:e7:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn e7_loop_with_small_model(response: &str) -> (AgentLoop, std::sync::Arc<TitleProvider>) {
    let al = AgentLoop::new(
        Box::new(E7NoopProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    );
    let provider = std::sync::Arc::new(TitleProvider::new(response));
    al.set_small_model(Some((
        provider.clone() as std::sync::Arc<dyn LlmProvider>,
        "test-small".to_string(),
    )));
    (al, provider)
}

/// 摆一条首条 user 消息（append_chat_log_meta 同生产写入形态）。
fn e7_write_user_row(key: &str, content: &str) {
    crate::chat_log::append_chat_log_meta(
        key,
        "user",
        content,
        &crate::chat_log::ChatLogMeta::default(),
    );
}

// ---------- sanitize_generated_title ----------

#[test]
fn e7_sanitize_strips_quotes_and_takes_first_line() {
    let out = sanitize_generated_title("  \"Rust 编译错误排查\"  \n第二行不该出现");
    assert_eq!(out.as_deref(), Some("Rust 编译错误排查"));
}

#[test]
fn e7_sanitize_truncates_at_char_boundary() {
    let long = "这是一个非常长的中文标题超过了二十四个字符的限制必须被截断";
    let out = sanitize_generated_title(long).unwrap();
    assert_eq!(out.chars().count(), E7_TITLE_MAX_CHARS);
    assert!(long.starts_with(&out), "截断必须是前缀（不劈 char）");
}

#[test]
fn e7_sanitize_empty_returns_none() {
    assert_eq!(sanitize_generated_title(""), None);
    assert_eq!(sanitize_generated_title("  \n\t "), None);
    assert_eq!(sanitize_generated_title("\"\""), None);
}

// ---------- spawn_session_title_job 端到端 ----------

#[tokio::test]
async fn e7_generates_and_persists_auto_title() {
    let (al, provider) = e7_loop_with_small_model("\"登录页面 500 报错排查\"\n多余行");
    let key = e7_uniq_key("ok");
    e7_write_user_row(&key, "帮我看看登录接口为什么报 500");
    // 占位符标题不挡自动生成。
    chat_log::write_session_meta(&key, chat_log::DEFAULT_SESSION_TITLE);

    let handle = al
        .spawn_session_title_job(&key)
        .expect("小模型已配置且无真实标题，应 spawn");
    handle.await.unwrap();

    assert_eq!(provider.models(), vec!["test-small".to_string()]);
    assert_eq!(
        chat_log::read_session_meta(&key).as_deref(),
        Some("登录页面 500 报错排查")
    );
    crate::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn e7_skips_without_small_model_or_real_title() {
    // 未配置 agents.small_model → 不 spawn（不烧主模型 token）。
    let al = AgentLoop::new(
        Box::new(E7NoopProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    );
    let key = e7_uniq_key("nosmall");
    e7_write_user_row(&key, "任意请求");
    assert!(al.spawn_session_title_job(&key).is_none());
    assert!(chat_log::read_session_meta(&key).is_none());

    // 手动改名 → 不 spawn。
    let (al, provider) = e7_loop_with_small_model("\"不该被调用\"");
    let key2 = e7_uniq_key("manual");
    e7_write_user_row(&key2, "任意请求");
    write_session_meta_manual(&key2, "我的会话");
    assert!(al.spawn_session_title_job(&key2).is_none());
    assert!(provider.models().is_empty());
    assert_eq!(
        chat_log::read_session_meta(&key2).as_deref(),
        Some("我的会话")
    );

    // 已有自动/真实标题（非占位符）→ 不 spawn。
    let (al, provider) = e7_loop_with_small_model("\"也不该被调用\"");
    let key3 = e7_uniq_key("titled");
    e7_write_user_row(&key3, "任意请求");
    chat_log::write_session_meta(&key3, "已有标题");
    assert!(al.spawn_session_title_job(&key3).is_none());
    assert!(provider.models().is_empty());

    // 首条 user 消息缺失 → 不 spawn。
    let (al, _p) = e7_loop_with_small_model("\"没人说话\"");
    let key4 = e7_uniq_key("nouser");
    assert!(al.spawn_session_title_job(&key4).is_none());

    for k in [&key, &key2, &key3, &key4] {
        crate::chat_log::delete_chat_log(k);
    }
}

#[tokio::test]
async fn e7_llm_failure_or_empty_output_leaves_title_absent() {
    // LLM 返回空白 → 清洗为 None → 不写 meta（下轮回复再试）。
    let (al, _provider) = e7_loop_with_small_model("   \n  ");
    let key = e7_uniq_key("empty");
    e7_write_user_row(&key, "任意请求");

    let handle = al
        .spawn_session_title_job(&key)
        .expect("应 spawn 再诚实丢弃");
    handle.await.unwrap();
    assert!(chat_log::read_session_meta(&key).is_none());
    crate::chat_log::delete_chat_log(&key);
}
