//! P31 记忆 dreaming —— gateway 侧 cron 挂载 + sweep 执行器。
//!
//! 职责：
//! - [`sync_dreaming_job`]：启动时把 `memory.dreaming` 配置节同步到
//!   CronService（store=配置真相源，幂等：补登记 / patch 跟随 / 删孤儿），
//!   必须在 `cron.start()` 之前调用（同 board autopilot sync 先例）。
//! - [`fire_dreaming_sweep`]：on_job 触发入口。on_job 回调是同步
//!   `Fn(&CronJob) -> Result<String, String>`，LLM sweep 是 30s+ 的异步长活
//!   ——同步返回「已启动」，实际工作 spawn 到后台（不阻塞 cron tick 循环）。
//! - sweep 本体在 [`run_and_report`]：盘上 config 为准（运行中改配置下次
//!   sweep 生效）→ 小模型通道（`agents.small_model`，未配回落主模型）→
//!   `run_sweep` 闭环 → 报告落 `workspace/logs/dreaming/sweep_<ts>.json`。
//!
//! 安全边界：sweep 只动记忆条目（metadata 标记 / tier / 归档标记 / 新建
//! merge 条目），不执行任何工具调用、不经 agent loop——无审批面。

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use nemesis_memory::dreaming::{DreamingLlm, DreamingWeights, SweepReport, run_sweep};
use nemesis_memory::manager::MemoryManager;

/// dreaming sweep 的 cron job 名（on_job 分支按 `memory-dreaming:` 前缀路由）。
pub const DREAMING_JOB_NAME: &str = "memory-dreaming:sweep";

/// cron 表达式非法/缺省时的回落值（与 nemesis-config `default_dreaming_cron`
/// 一致——每日 03:30 本地时间）。
const FALLBACK_CRON: &str = "30 3 * * *";

// ---------------------------------------------------------------------------
// 启动同步（配置真相源 → cron store，幂等）
// ---------------------------------------------------------------------------

/// 把 `memory.dreaming` 配置同步到 cron 服务。幂等：无 job 且开启 → 登记；
/// 已有 job → schedule/enabled 跟随配置；关闭 → 删 job（含孤儿清理）。
/// 表达式非法 → warn + 回落默认（诚实可见，不静默不崩）。
pub fn sync_dreaming_job(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    cfg: &nemesis_config::Config,
) {
    let dreaming_cfg = cfg.memory.as_ref().and_then(|m| m.dreaming.as_ref());
    let enabled = dreaming_cfg.map(|d| d.enabled).unwrap_or(false);
    let configured_cron = dreaming_cfg.map(|d| d.cron.clone()).unwrap_or_default();

    let svc = match cron.lock() {
        Ok(s) => s,
        Err(_) => {
            warn!("[Dreaming] cron 服务锁中毒，启动同步跳过");
            return;
        }
    };

    if !enabled {
        // 关闭态：清理残留 job（幂等——无 job 时静默返回）
        for job in svc.list_jobs(true) {
            if job.name == DREAMING_JOB_NAME {
                svc.remove_job(&job.id);
                info!(
                    "[Dreaming] memory.dreaming.enabled=false — 已移除 cron job {}",
                    job.id
                );
            }
        }
        return;
    }

    // 表达式合法性闸：非法 → 回落默认（warn 留痕）。
    let expr = if !configured_cron.is_empty()
        && nemesis_cron::service::CronService::validate_schedule(&configured_cron).is_ok()
    {
        configured_cron
    } else {
        if !configured_cron.is_empty() {
            warn!("[Dreaming] dreaming.cron `{configured_cron}` 非法 — 回落默认 `{FALLBACK_CRON}`");
        }
        FALLBACK_CRON.to_string()
    };

    let schedule = nemesis_cron::service::CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(expr.clone()),
        tz: None,
    };

    let existing = svc
        .list_jobs(true)
        .into_iter()
        .find(|j| j.name == DREAMING_JOB_NAME);
    match existing {
        Some(job) => {
            let current_expr = job.schedule.expr.clone().unwrap_or_default();
            if current_expr != expr || !job.enabled {
                match svc.patch_job(
                    &job.id,
                    &nemesis_cron::service::CronJobPatch {
                        schedule: Some(schedule),
                        enabled: Some(true),
                        ..Default::default()
                    },
                ) {
                    Ok(_) => info!(
                        "[Dreaming] cron job {} 已跟随配置更新 (cron={expr})",
                        job.id
                    ),
                    Err(e) => warn!("[Dreaming] cron job 跟随配置更新失败: {e}"),
                }
            } else {
                info!("[Dreaming] cron job 已登记且与配置一致 (cron={expr})");
            }
        }
        None => {
            // message 留空——dreaming job 不走消息总线（on_job 分支在 message
            // 判空前拦截）；deliver=false（结果落报告文件，不投递通道）。
            match svc.add_job_ext(
                DREAMING_JOB_NAME,
                schedule,
                "",
                false,
                None,
                None,
                None,
                None,
                true,
            ) {
                Ok(job) => info!("[Dreaming] cron job 已登记: id={} (cron={expr})", job.id),
                Err(e) => warn!("[Dreaming] cron job 登记失败: {e}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 触发入口（同步 on_job → 异步后台 sweep）
// ---------------------------------------------------------------------------

/// on_job 触发入口：同步快速返回「已启动」，sweep spawn 到后台。
/// （on_job 是同步回调；LLM 决策 30s+ 不能阻塞 cron tick 循环。）
pub fn fire_dreaming_sweep(mgr: Arc<MemoryManager>, home: PathBuf) -> Result<String, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "记忆 dreaming：当前无 tokio 运行时上下文，本次触发跳过".to_string())?;
    handle.spawn(async move {
        match run_and_report(mgr, home).await {
            Ok(report) => info!(
                "[Dreaming] sweep 完成: candidates={} applied={} rejected={} merged={} promoted={} expired={} kept={}",
                report.candidates,
                report.decisions_applied,
                report.decisions_rejected,
                report.merged,
                report.promoted,
                report.expired,
                report.kept
            ),
            Err(e) => warn!("[Dreaming] sweep 失败: {e}"),
        }
    });
    Ok("记忆 dreaming sweep 已在后台启动（结果见 workspace/logs/dreaming/）".to_string())
}

/// sweep 闭环 + 报告落盘。盘上 config 为真相源（enabled 关 = 拒绝执行——
/// 说明 cron sync 尚未跟上，诚实报错不静默跑）。
async fn run_and_report(mgr: Arc<MemoryManager>, home: PathBuf) -> Result<SweepReport, String> {
    let config_path = home.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| format!("config.json 读取失败: {e}"))?;
    let dreaming_cfg = cfg.memory.as_ref().and_then(|m| m.dreaming.clone());
    if !dreaming_cfg.as_ref().map(|d| d.enabled).unwrap_or(false) {
        return Err(
            "memory.dreaming.enabled=false（配置已关；cron job 应由启动同步移除）".to_string(),
        );
    }
    let weights: DreamingWeights = dreaming_cfg
        .as_ref()
        .and_then(|d| d.weights.as_ref())
        .map(weights_from_config)
        .unwrap_or_default();
    let top_k = dreaming_cfg.as_ref().map(|d| d.top_k).unwrap_or(8).max(1);

    let llm = build_dream_llm(&cfg)?;
    let report = run_sweep(&mgr, &llm, &weights, top_k, chrono::Local::now()).await?;

    // 报告落盘：workspace/logs/dreaming/sweep_<ts>.json（诚实留痕——每次
    // sweep 的决策/拒绝计数与原因可回查）
    let dir = home.join("workspace").join("logs").join("dreaming");
    std::fs::create_dir_all(&dir).map_err(|e| format!("报告目录创建失败: {e}"))?;
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = dir.join(format!("sweep_{ts}.json"));
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("报告写入失败: {e}"))?;
    info!("[Dreaming] 报告已写入 {}", path.display());
    Ok(report)
}

// ---------------------------------------------------------------------------
// LLM 通道（小模型优先；ForgeProviderBridge 同型）
// ---------------------------------------------------------------------------

/// `agents.small_model` 优先（杂务通道语义——同 /compact / 标题生成），
/// 未配置回落主模型（`agents.defaults.llm`）。解析/构造失败 = Err（sweep
/// 本轮放弃，诚实报错）。
fn build_dream_llm(cfg: &nemesis_config::Config) -> Result<GatewayDreamLlm, String> {
    let small_ref = cfg
        .agents
        .small_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (model_ref, channel) = match small_ref {
        Some(s) => (s.to_string(), "agents.small_model"),
        None => (nemesis_config::get_effective_llm(Some(cfg)), "default"),
    };
    let resolution = nemesis_config::resolve_model_config(cfg, &model_ref)
        .map_err(|e| format!("[Dreaming] 模型 '{model_ref}'（{channel}）解析失败: {e}"))?;
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        proxy: resolution.proxy.clone(),
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        // provider 不消费 workspace（消费方是工具层）；dreaming 无工具调用。
        workspace: String::new(),
        connect_mode: resolution.connect_mode.clone(),
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    let provider = nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| format!("[Dreaming] provider 构造失败: {e}"))?;
    Ok(GatewayDreamLlm {
        provider,
        model: resolution.model_name.clone(),
    })
}

/// gateway 的 dreaming LLM 通道（`DreamingLlm` trait 的 provider 实现）。
/// 低温度 + 小输出预算——决策 JSON 不需要长文。
struct GatewayDreamLlm {
    provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

#[async_trait::async_trait]
impl DreamingLlm for GatewayDreamLlm {
    async fn decide(&self, system_prompt: &str, user_prompt: &str) -> Result<String, String> {
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".to_string(),
                content: system_prompt.to_string().into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".to_string(),
                content: user_prompt.to_string().into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
        ];
        let options = nemesis_providers::types::ChatOptions {
            temperature: Some(0.2),
            max_tokens: Some(2048),
            top_p: None,
            stop: None,
            reasoning_effort: None,
            extra: std::collections::HashMap::new(),
        };
        let response = self
            .provider
            .chat(&messages, &[], &self.model, &options)
            .await
            .map_err(|e| format!("{e:?}"))?;
        if response.content.trim().is_empty() {
            Err("LLM returned no content".to_string())
        } else {
            Ok(response.content)
        }
    }
}

// ---------------------------------------------------------------------------
// 权重配置转换（DreamingWeightsConfig → DreamingWeights，只覆盖显式项）
// ---------------------------------------------------------------------------

/// 孤儿规则：不能在 nemesisbot 侧给两个外部类型实现 From——用普通转换 fn
///（语义相同：只覆盖用户显式给出的项，其余用内置默认）。
fn weights_from_config(w: &nemesis_config::DreamingWeightsConfig) -> DreamingWeights {
    let d = DreamingWeights::default();
    DreamingWeights {
        recall: w.recall.unwrap_or(d.recall),
        decay: w.decay.unwrap_or(d.decay),
        conflict: w.conflict.unwrap_or(d.conflict),
        redundancy: w.redundancy.unwrap_or(d.redundancy),
        age: w.age.unwrap_or(d.age),
        source: w.source.unwrap_or(d.source),
    }
}
