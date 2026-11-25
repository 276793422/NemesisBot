//! 项2 P2 召回质量闸的 gateway 侧装配：`memory.recall_gate` 配置 → 小模型
//! 判定通道 → `RecallGate`（挂 `MemoryManager`）。
//!
//! 模型链（对齐 spec「诚实跳过」语义，**无主模型臂**——主模型每轮判召回
//! 正是闸要省的成本）：`recall_gate.model` 显式别名 → `agents.small_model`
//! → 都未配 = WARN 一次 + None（直通现状，绝不因缺配置失败）。provider
//! 构造复用 dreaming 通道同款（`resolve_model_config` + `create_provider`）。
//!
//! 装配点在 `agent_factory` 的 set_memory_inject 块——gateway / run / acp
//! 三入口同源生效；每次 loop 构建执行一次（幂等替换，provider 构造廉价）。

use std::sync::Arc;

use nemesis_memory::recall_gate::{RecallGate, RecallJudgeLlm, judge_prompt, parse_judge_response};

/// gateway 的判定 LLM 通道（`RecallJudgeLlm` trait 的 provider 实现）。
/// 低温度 + 小输出预算——bool 数组不需要长文。
struct GatewayRecallJudge {
    provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

#[async_trait::async_trait]
impl RecallJudgeLlm for GatewayRecallJudge {
    async fn judge(&self, query: &str, candidates: &[String]) -> Result<Vec<bool>, String> {
        let (system, user) = judge_prompt(query, candidates);
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".to_string(),
                content: system.into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".to_string(),
                content: user.into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
        ];
        let options = nemesis_providers::types::ChatOptions {
            temperature: Some(0.2),
            max_tokens: Some(512),
            top_p: None,
            stop: None,
            reasoning_effort: None,
            per_request_extra_body: None,
            extra: std::collections::HashMap::new(),
        };
        let response = self
            .provider
            .chat(&messages, &[], &self.model, &options)
            .await
            .map_err(|e| format!("{e:?}"))?;
        if response.content.trim().is_empty() {
            return Err("LLM returned no content".to_string());
        }
        parse_judge_response(&response.content)
    }
}

/// 读 `memory.recall_gate` 装配闸。
///
/// 返回 `None` 的三种形态（全部直通现状，无注入面变化）：
/// - 配置缺席或 `enabled=false`（默认关，静默）；
/// - 模型链上无可用模型（WARN 一次——本函数只在装配期被调用，天然一次）；
/// - 模型解析/provider 构造失败（WARN 一次）。
pub fn build_recall_gate(cfg: &nemesis_config::Config) -> Option<Arc<RecallGate>> {
    let gate_cfg = cfg.memory.as_ref()?.recall_gate.as_ref()?;
    if !gate_cfg.enabled {
        return None;
    }

    // 模型链：显式别名 → agents.small_model → 诚实跳过。
    let explicit = gate_cfg.model.trim();
    let small = cfg
        .agents
        .small_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let model_ref = if !explicit.is_empty() {
        explicit.to_string()
    } else if let Some(s) = small {
        s.to_string()
    } else {
        tracing::warn!(
            "[RecallGate] memory.recall_gate.enabled=true 但判定模型未配置（recall_gate.model \
             与 agents.small_model 均空）——召回质量闸诚实跳过，注入直通现状"
        );
        return None;
    };

    let resolution = match nemesis_config::resolve_model_config(cfg, &model_ref) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[RecallGate] 判定模型 '{model_ref}' 解析失败: {e}——召回质量闸诚实跳过");
            return None;
        }
    };
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        proxy: resolution.proxy.clone(),
        stream_mode: resolution.stream_mode.clone(),
        cache_breakpoints: resolution.cache_breakpoints,
        extra_body: resolution.extra_body.clone(),
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        // 判定无工具调用（纯文本进出）；workspace 留空对 HTTP lane 无影响，
        // 但 CLI 型 lane（claude-cli/codex-cli）的子进程 cwd 会消费它
        // （BUG-20261125-018 横向排查定性：本函数签名只收 cfg 无 workspace
        // 可传，透传需改签名波及调用方——chore 通道挂账，见 BUG_FIX 横向排查节）。
        workspace: String::new(),
        connect_mode: resolution.connect_mode.clone(),
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    let provider = match nemesis_providers::factory::create_provider(&factory_cfg) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[RecallGate] provider 构造失败: {e}——召回质量闸诚实跳过");
            return None;
        }
    };

    let judge = GatewayRecallJudge {
        provider,
        model: resolution.model_name.clone(),
    };
    tracing::info!(
        model = %resolution.model_name,
        min_candidates = gate_cfg.min_candidates,
        max_items = gate_cfg.max_items,
        "[RecallGate] 召回质量闸已装配（判定模型 {model_ref}）"
    );
    Some(Arc::new(RecallGate::new(
        Arc::new(judge),
        gate_cfg.min_candidates,
        gate_cfg.max_items,
        gate_cfg.cache_secs,
    )))
}
