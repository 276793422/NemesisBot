//! config_watch 纯逻辑测试：`current_max_tokens_override` 的 u32 值域防护。
//!
//! per-model `max_output_tokens` 是 JSON 直读 i64（resolver 家族契约：值
//! 原样透传），消费侧 `ChatOptions.max_tokens` 是 `Option<u32>`——裸 `as`
//! 会把 -1 回绕成 4294967291，每轮请求带巨值 max_tokens 打爆 provider
//! 400。修复后：值域外的坏值按缺席处理（None → 调用方回落 8192 默认）。
//!
//! 独立测试文件（生产文件只保留声明行，仓库 2026-08-25 纪律）；provider
//! 桩自建（loop/tests.rs 的辅助模块私有，与 length_escalation_tests 同先例）。

use super::*;
use async_trait::async_trait;

fn cw_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

struct NoopProvider;

#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("not used".to_string())
    }
}

/// 带 config.json 的 loop（TempDir 须与 loop 同寿命，返回持有）。
fn loop_with_config(cfg: serde_json::Value) -> (tempfile::TempDir, Arc<AgentLoop>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    std::fs::write(&path, serde_json::to_string(&cfg).unwrap()).unwrap();
    let loop_ = Arc::new(AgentLoop::new(Box::new(NoopProvider), cw_config()));
    loop_.set_config_path(path);
    (dir, loop_)
}

fn override_of(loop_: &AgentLoop, alias: &str) -> Option<u32> {
    loop_.current_max_tokens_override(Some(alias))
}

#[test]
fn valid_value_passthrough() {
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/ok", "model_name": "m-ok", "max_output_tokens": 1234}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-ok"), Some(1234));
}

#[test]
fn negative_value_treated_as_absent_not_wrapped() {
    // 修复前：-1i64 as u32 = 4294967291 → 每轮请求带巨值 max_tokens。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/neg", "model_name": "m-neg", "max_output_tokens": -1}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-neg"), None);
}

#[test]
fn zero_value_treated_as_absent_reaches_default_fallback() {
    // resolver 层补 `.filter(>0)` 前穿透——try_from(0)=Ok(0)
    // 是值域内合法值，unwrap_or(DEFAULT) 不触发 → 请求体 max_tokens: 0 →
    // provider 400（要求 ≥1）。修复后 0 按缺席处理，调用方回落 64K 默认。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/zero", "model_name": "m-zero", "max_output_tokens": 0}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-zero"), None);
}

#[test]
fn over_u32_value_treated_as_absent_not_truncated() {
    // 修复前：99_999_999_999i64 as u32 高位截断成无意义小值。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/big", "model_name": "m-big",
             "max_output_tokens": 99_999_999_999i64}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-big"), None);
}

#[test]
fn absent_key_still_none() {
    // 对照：键缺席 = None（resolver 家族语义不受影响）。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/plain", "model_name": "m-plain"}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-plain"), None);
}

// ---- G3（2026-11-09）：会话级强度 override（chat.set_effort 链路后端）----

#[test]
fn session_effort_set_get_clear() {
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [{"model": "m/plain", "model_name": "m-plain"}]
    }));
    // 未设置 = None（前端下拉显示「默认」）。
    assert_eq!(loop_.session_effort("web:s1"), None);
    // 设置 → 读回。
    loop_.set_session_effort("web:s1", Some("high".to_string()));
    assert_eq!(loop_.session_effort("web:s1"), Some("high".to_string()));
    // 清除（null 语义）→ 回落 None。
    loop_.set_session_effort("web:s1", None);
    assert_eq!(loop_.session_effort("web:s1"), None);
}

#[test]
fn session_effort_isolated_by_key() {
    // 会话隔离（用户拍板语义）：s1 设 off 不牵连 s2。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [{"model": "m/plain", "model_name": "m-plain"}]
    }));
    loop_.set_session_effort("web:s1", Some("off".to_string()));
    loop_.set_session_effort("web:s2", Some("high".to_string()));
    assert_eq!(loop_.session_effort("web:s1"), Some("off".to_string()));
    assert_eq!(loop_.session_effort("web:s2"), Some("high".to_string()));
    assert_eq!(loop_.session_effort("web:s3"), None);
}

#[test]
fn session_effort_accepts_extended_tiers() {
    // 7 档值域扩档（2026-11-09）：override 是字符串直存，新档（minimal/
    // xhigh/max）零 loop 改动即生效——值域校验收口在 WSAPI 入口层。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [{"model": "m/plain", "model_name": "m-plain"}]
    }));
    for tier in ["minimal", "xhigh", "max"] {
        loop_.set_session_effort("web:s1", Some(tier.to_string()));
        assert_eq!(loop_.session_effort("web:s1"), Some(tier.to_string()));
    }
}

#[test]
fn session_effort_priority_over_config() {
    // 取值优先级（run_loop chat_opts 组装同式）：会话 override > config
    // 模型级 reasoning_effort。s1 有 override → off；s2 无 → config 的 high；
    // 清除后回落 config 值。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/glm", "model_name": "test-model", "reasoning_effort": "high"}
        ]
    }));
    let effective = |lk: &AgentLoop, key: &str| {
        lk.session_effort(key)
            .or_else(|| lk.current_reasoning_effort_override(None))
    };
    loop_.set_session_effort("web:s1", Some("off".to_string()));
    assert_eq!(effective(&loop_, "web:s1"), Some("off".to_string()));
    assert_eq!(effective(&loop_, "web:s2"), Some("high".to_string()));
    loop_.set_session_effort("web:s1", None);
    assert_eq!(effective(&loop_, "web:s1"), Some("high".to_string()));
}

// --- BUG-20261108-030：外层包裹超时跟随 provider 请求超时（超时阶梯
// 纪律：外层必须大于内层）。显式配置直接采信（含 0=关）——用户预算
// 意志优先；缺省跟随活跃模型 per-model timeout + 30s 余量。 ---

#[test]
fn provider_call_timeout_follows_per_model_timeout_plus_margin() {
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "slow/big", "model_name": "slow-big", "timeout_secs": 900}
        ]
    }));
    loop_.set_active_model("slow-big");
    assert_eq!(loop_.current_provider_call_timeout_secs(), 930);
}

#[test]
fn provider_call_timeout_alias_timeout_key_same_semantics() {
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "slow/alias", "model_name": "slow-alias", "timeout": 700}
        ]
    }));
    loop_.set_active_model("slow-alias");
    assert_eq!(loop_.current_provider_call_timeout_secs(), 730);
}

#[test]
fn provider_call_timeout_default_follows_lane_default_plus_margin() {
    // 条目无 per-model timeout（= lane 默认 600）→ 外层 630。旧恒 480
    // 低于内层 600：480~600s 的诚实慢响应被外层误杀、per-model 放宽被
    // 静默截断。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "plain/x", "model_name": "plain-x"}
        ]
    }));
    loop_.set_active_model("plain-x");
    assert_eq!(loop_.current_provider_call_timeout_secs(), 630);
}

#[test]
fn provider_call_timeout_explicit_config_wins_over_following() {
    // 显式配置（含低于内层的值）直接采信——用户预算意志优先
    //（与 tier `model_tier != auto` 直接采信同哲学）。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "agents": {"defaults": {"provider_call_timeout_secs": 60}},
        "model_list": [
            {"model": "slow/big", "model_name": "slow-big", "timeout_secs": 900}
        ]
    }));
    loop_.set_active_model("slow-big");
    assert_eq!(loop_.current_provider_call_timeout_secs(), 60);
}

#[test]
fn provider_call_timeout_explicit_zero_disables() {
    // 0 = 关闭超时（旧行为），显式 0 优先于跟随。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "agents": {"defaults": {"provider_call_timeout_secs": 0}},
        "model_list": [
            {"model": "slow/big", "model_name": "slow-big", "timeout_secs": 900}
        ]
    }));
    loop_.set_active_model("slow-big");
    assert_eq!(loop_.current_provider_call_timeout_secs(), 0);
}

#[test]
fn provider_lane_default_double_write_is_pinned() {
    // PROVIDER_LANE_DEFAULT_TIMEOUT_SECS 与 nemesis_providers::factory::
    // DEFAULT_TIMEOUT_SECS（超时阶梯最内层真相源）是刻意双写——providers
    // 是 dev-dep，lib 代码不可引用。factory 值变化必须同步本侧，否则
    // 外层兜底与内层 lane 默认阶梯错位（外层 < 内层旧形态复发）。
    assert_eq!(
        crate::r#loop::rate_limit::PROVIDER_LANE_DEFAULT_TIMEOUT_SECS,
        nemesis_providers::factory::DEFAULT_TIMEOUT_SECS
    );
}

// mark_effort_probe_stale_at ------------------------------------------------
// effort 探针保鲜层③（2026-11-09）：真实使用 400 → effort_probe.stale
// 标记。自由函数核直测（不必造 AgentLoop）。

fn stale_write_config(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let p = dir.join("config.json");
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn stale_marks_target_entry_with_probe() {
    let dir = tempfile::tempdir().unwrap();
    let path = stale_write_config(
        dir.path(),
        r#"{
        "model_list": [
            { "model_name": "m1", "model": "x/y", "effort_probe": { "accepted": ["low"], "control": "strict" } },
            { "model_name": "m2", "model": "x/y", "effort_probe": { "accepted": ["high"], "control": "strict" } }
        ]
    }"#,
    );
    crate::r#loop::config_watch::mark_effort_probe_stale_at(&path, "m1");
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        cfg["model_list"][0]["effort_probe"]["stale"], true,
        "目标条目被标"
    );
    assert_eq!(
        cfg["model_list"][0]["effort_probe"]["accepted"],
        serde_json::json!(["low"]),
        "其余字段保留"
    );
    assert!(
        cfg["model_list"][1]["effort_probe"]
            .as_object()
            .unwrap()
            .get("stale")
            .is_none(),
        "兄弟条目不动"
    );
}

#[test]
fn stale_mark_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = stale_write_config(
        dir.path(),
        r#"{ "model_list": [ { "model_name": "m1", "effort_probe": { "accepted": [], "stale": true } } ] }"#,
    );
    let before = std::fs::read_to_string(&path).unwrap();
    crate::r#loop::config_watch::mark_effort_probe_stale_at(&path, "m1");
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before, after, "已 stale = 不重写盘（幂等）");
}

#[test]
fn stale_no_probe_record_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = stale_write_config(
        dir.path(),
        r#"{ "model_list": [ { "model_name": "m1", "model": "x/y" } ] }"#,
    );
    let before = std::fs::read_to_string(&path).unwrap();
    crate::r#loop::config_watch::mark_effort_probe_stale_at(&path, "m1");
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before, after, "无 effort_probe = 无可失效判定，不写盘");
}

#[test]
fn stale_missing_file_and_missing_name_no_panic() {
    let dir = tempfile::tempdir().unwrap();
    let ghost = dir.path().join("nope.json");
    crate::r#loop::config_watch::mark_effort_probe_stale_at(&ghost, "m1"); // 文件缺 → 静默
    let path = stale_write_config(dir.path(), r#"{ "model_list": [] }"#);
    crate::r#loop::config_watch::mark_effort_probe_stale_at(&path, "ghost"); // 名缺 → 静默
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains(r#""model_list": []"#), "内容不变");
}

// 2026-11-10 live 冒烟抓出的时序缝回归锁 -------------------------------

#[test]
fn effort_reader_sees_autofix_after_republish() {
    // C 自动修正写盘后，effort 读者必须经 check_config_reload 见到新值。
    // 修复前 turn 开始的 chat_opts 读的是旧代——探测修了 config、请求仍
    // 发旧档位（多花一轮 400+剥键，B 还会在已修对的配置上误标 stale）。
    let (dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "test-model", "model_name": "test-model", "reasoning_effort": "minimal"}
        ]
    }));
    // 绑定即发布（set_config_path force）——现行代 = minimal。
    assert_eq!(
        loop_.current_reasoning_effort_override(None),
        Some("minimal".to_string()),
        "绑定后现行代承载初始档位"
    );

    // C 自动修正：盘上 raw 改档（模拟 apply_effort_autofix 的写盘面）。
    let path = dir.path().join("config.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    raw["model_list"][0]["reasoning_effort"] = serde_json::json!("low");
    std::fs::write(&path, serde_json::to_string(&raw).unwrap()).unwrap();

    // 修复点：turn 开始的对账发布（run_loop TurnState 构造前新增）。
    loop_.check_config_reload();
    assert_eq!(
        loop_.current_reasoning_effort_override(None),
        Some("low".to_string()),
        "写盘 + 对账发布后 effort 读者必须见新档位"
    );
}
