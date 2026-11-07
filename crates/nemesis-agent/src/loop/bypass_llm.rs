//! 杂务旁路 LLM 调用（prompt-pack pro M4）。
//!
//! 标题生成（E7）、前情摘要（G1/T4/multipart）、Forge 评审与反思等辅助调用
//! 共用的统一护栏：此前这些调用点各自裸调 `provider.chat`——无 token 上限
//! （走 provider 默认 8192）、无墙钟超时（慢模型/挂死连接会拖住后台任务）、
//! 空输出各自解读。本模块把「限 token + 超时 + 输出校验 + 遥测」收敛为单一
//! 真相源，消费方只保留各自的失败回退语义（标题→None 跳过；摘要→None 保持
//! 历史不折叠；Forge 评审→启发式/默认分）。
//!
//! 安全语义不在此处：guardian LLM 审计是安全判定点（fail-closed），其无上下
//! 文宪法、只升格消费与预筛词表独立演化，不迁入本模块。
use std::time::Duration;

/// 标题生成的输出 token 上限。
///
/// 2026-09-28 从 64 上调至 8192（用户裁决：预算是防失控上限不是支出，全仓
/// 杂务文本任务统一 8192）：64 是非思考时代遗产，thinking 系模型（GLM
/// anthropic 兼容端点默认开思考）把思考 token 一并计入 max_tokens，思考先
/// 烧穿 64 → 正文为空 → 真机 5/5 空输出（标题功能等于关闭）。8192 对非思考
/// 模型封顶失控长输出、对思考模型留足正文余量；标题清洗仍钳 24 字，超量
/// 零成本。`AUX_TITLE_TIMEOUT` 与重试护栏保持独立兜底。
pub const AUX_TITLE_MAX_TOKENS: u32 = 8192;

/// 前情摘要的输出 token 上限（结构化摘要；2026-09-28 真模型验证从 2048 上调
/// 至 8192：thinking 系模型（GLM anthropic 兼容端点默认开思考）会把思考 token
/// 一并计入 max_tokens，2048 被思考烧完后 text 为空 → 摘要「无有效内容」静默
/// 失败。8192 对非思考模型封顶失控长输出、对思考模型留足正文余量）。
pub const AUX_SUMMARY_MAX_TOKENS: u32 = 8192;

/// 标题生成墙钟上限（小模型短输出，60s 足够；超时 = 本轮放弃，下轮再试）。
pub const AUX_TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 前情摘要墙钟上限（大历史 + 慢模型的兜底上限；再往上就该让 compact 失败
/// 保持历史不折叠，而不是挂住后台任务）。
pub const AUX_SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);

/// Forge 评审/反思/草稿生成墙钟上限（LLMCaller 桥共用；评审与反思都是
/// 千 token 级短输出）。
pub const AUX_FORGE_TIMEOUT: Duration = Duration::from_secs(180);

/// 杂务调用的 ChatOptions 组装单点：钳 `max_tokens` + **显式禁思考**
/// （`reasoning_effort: Some("off")`，anthropic lane 映射为
/// `thinking:{type:"disabled"}`；openai-compat lane 过滤不透传）——杂务
/// 调用（标题/摘要/分支摘要/Forge 桥）不需要思考，思考只会烧穿预算。仅
/// 作用于按此函数组装 options 的杂务请求，主循环 effort（low/medium/high/
/// None）不受影响。采样参数不设值（留 adapter 默认，与既有线上行为一致）。
pub fn aux_chat_options(max_tokens: u32) -> crate::types::ChatOptions {
    crate::types::ChatOptions {
        max_tokens: Some(max_tokens),
        reasoning_effort: Some("off".to_string()),
        ..Default::default()
    }
}

/// 杂务旁路调用的统一护栏：墙钟超时 + 空输出校验 + 时长遥测。
///
/// 对任意「最终产出文本」的 LLM 调用未来生效（`Result<String, String>`）；
/// 返回 `Err` 的情形：上游报错原样透传、超时、空/纯空白输出（按失败处理，
/// 不让空串流进下游当成功结果）。成功路径打 debug 级时长日志，失败路径
/// warn（带标签，便于按消费点归因）。
pub async fn guarded_llm_call<F>(
    label: &str,
    timeout: Duration,
    llm_call: F,
) -> Result<String, String>
where
    F: std::future::Future<Output = Result<String, String>>,
{
    let start = std::time::Instant::now();
    match tokio::time::timeout(timeout, llm_call).await {
        Ok(Ok(content)) if !content.trim().is_empty() => {
            tracing::debug!(
                "[bypass:{}] 完成（{}ms，{} 字符）",
                label,
                start.elapsed().as_millis(),
                content.len()
            );
            Ok(content)
        }
        Ok(Ok(_)) => {
            tracing::warn!("[bypass:{}] 模型返回空输出，按失败处理", label);
            Err(format!("[bypass:{}] 空输出，按失败处理", label))
        }
        Ok(Err(e)) => {
            tracing::warn!(
                "[bypass:{}] LLM 调用失败（{}ms）：{}",
                label,
                start.elapsed().as_millis(),
                e
            );
            Err(e)
        }
        Err(_) => {
            tracing::warn!(
                "[bypass:{}] LLM 调用超时（上限 {}s）",
                label,
                timeout.as_secs()
            );
            Err(format!(
                "[bypass:{}] 超时（上限 {}s）",
                label,
                timeout.as_secs()
            ))
        }
    }
}

/// 单次重试原语（类型无关）：工厂闭包 `make_call` 每次调用产出一个新
/// future——重试必须重建请求（future 不可重复 poll），因此收闭包不收
/// future。首个 `Ok` 直接返回；`Err` 则 warn 后重建再试一次，两次全败
/// 返回第二次的 `Err`。采样抖动型失败（空输出/瞬态上游错误）由这一层
/// 兜底；外层护栏的墙钟预算不变（重试在超时窗口内进行，不翻倍最坏延迟）。
pub async fn with_one_retry<T, F, Fut>(label: &str, mut make_call: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    match make_call().await {
        Ok(v) => Ok(v),
        Err(first_err) => {
            tracing::warn!("[bypass:{}] 首次调用失败，重试一次：{}", label, first_err);
            make_call().await
        }
    }
}

/// [`guarded_llm_call`] 的重试版：空输出/瞬态错误重试一次，仍失败才上抛。
///
/// 关键：每次尝试**先**过「空输出=失败」归一再进 [`with_one_retry`]——
/// 否则空输出以 `Ok("")` 形态穿过重试原语（它只对 `Err` 重试），护栏只在
/// 最终结果上校验，重试对空输出（thinking 系模型烧穿的主要失败形态）永远
/// 不触发。超时仍包在重试外层（总墙钟预算 = timeout，不随重试放大）；
/// 最终结果再过一次 [`guarded_llm_call`] 的完整校验（幂等，兜住归一外的
/// 形态）。
///
/// S8：无账本载荷的旧签名——委托 [`guarded_llm_call_retrying_ledgered`]（
/// [`AuxLedger::disabled`] 不落账），护栏/重试/文案行为逐字节零变化
/// （bypass_llm_tests 回归钉死）。
pub async fn guarded_llm_call_retrying<F, Fut>(
    label: &str,
    timeout: Duration,
    mut make_call: F,
) -> Result<String, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    guarded_llm_call_retrying_ledgered(label, timeout, AuxLedger::disabled(), move || {
        let r = make_call();
        async move {
            r.await.map(|content| AuxOutcome {
                content,
                usage: None,
            })
        }
    })
    .await
}

// ---------------------------------------------------------------------------
// S8（2026-11-06 B29/B30 设计 §4）：杂务调用账本落账
// ---------------------------------------------------------------------------

/// 杂务调用的产出形态：文本 + 可选实测 usage。provider 回传 usage 时原样
/// 携带（实测口径入账/计价），缺失时 `None`（账本行按 estimated 注记口径
/// 诚实降级，不造数）。
pub struct AuxOutcome {
    pub content: String,
    pub usage: Option<crate::loop_executor::ObserverUsageInfo>,
}

/// 杂务调用的账本归因载荷（设计 §4.2：复用 `insert_request_log` 同一真相
/// 源，不新增 observer 事件类型）。`aux_kind` 四族：`title` / `summary` /
/// `branch_summary` / `forge_review`（调用点传入）；同一值写 RequestLog 的
/// `provider_type` 列（organ-5 主链路恒空串——非空即杂务，单一判别列，
/// 用量统计据此分家）。[`AuxLedger::disabled`] = 不落账：旧签名
/// [`guarded_llm_call_retrying`] 以此形态委托 ledgered 版，行为零变化。
pub struct AuxLedger<'a> {
    pub aux_kind: &'a str,
    pub model: &'a str,
    pub session_key: Option<&'a str>,
    pub data_store: Option<&'a std::sync::Arc<nemesis_data::DataStore>>,
}

impl AuxLedger<'_> {
    pub fn disabled() -> Self {
        Self {
            aux_kind: "",
            model: "",
            session_key: None,
            data_store: None,
        }
    }
}

/// [`guarded_llm_call_retrying`] 的落账版：护栏/重试/超时/归一语义与旧
/// 签名**逐字节相同**（S8 纪律：不改变 aux 通道任何既有行为——重试/超时/
/// 模型回落链零变化，只在成功返回处补一次账本写入）；差异仅两处——尝试
/// 闭包产出 [`AuxOutcome`]（带 usage），成功路径落 RequestLog（失败/超时/
/// 空输出不落账——成功专用轻量可见性，与 organ-5 主链路及 S16 终局行
/// 互补）。
pub async fn guarded_llm_call_retrying_ledgered<F, Fut>(
    label: &str,
    timeout: Duration,
    ledger: AuxLedger<'_>,
    mut make_call: F,
) -> Result<String, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<AuxOutcome, String>>,
{
    // 账本计时从首次尝试起（覆盖重试窗口的总墙钟，与护栏超时语义一致）。
    let started = std::time::Instant::now();
    // usage 槽位：成功尝试把实测 usage 带出重试链（失败尝试不写、重试成功
    // 覆盖——槽位与最终 Ok 恒一致）；文本归一成 String 形态复用旧护栏原样
    // （超时/空输出归一/文案单一真相源 [`guarded_llm_call`]，不因落账分叉
    // 第二套臂）。锁不跨 await（语句内即释放），async 语境安全。
    let usage_slot = std::sync::Mutex::new(None::<crate::loop_executor::ObserverUsageInfo>);
    // 归一在闭包体内完成（future 只持有已产出的 Fut，不借环境——保住
    // FnMut 的可重复调用性）；文案与旧签名逐字节一致（bypass_llm_tests 钉）。
    let attempt = || {
        let r = make_call();
        // 槽位按引用进 future（async move 搬所有权会把外层闭包降级 FnOnce）。
        let slot = &usage_slot;
        async move {
            match r.await {
                Ok(out) if out.content.trim().is_empty() => {
                    tracing::warn!("[bypass:{label}] 模型返回空输出，按失败处理");
                    Err(format!("[bypass:{label}] 空输出，按失败处理"))
                }
                Ok(out) => {
                    *slot.lock().expect("aux usage slot poisoned") = out.usage;
                    Ok(out.content)
                }
                Err(e) => Err(e),
            }
        }
    };
    let result = guarded_llm_call(label, timeout, with_one_retry(label, attempt)).await;
    if let Ok(content) = &result {
        // guard 先绑定（guard 临时值活到语句尾，链式 .as_ref() 会悬垂）。
        let usage_guard = usage_slot.lock().expect("aux usage slot poisoned");
        write_aux_ledger_row(&ledger, usage_guard.as_ref(), content, started.elapsed());
    }
    result
}

/// 杂务成功调用落一条账本行（`insert_request_log` 同一真相源）。usage
/// 在场 = 实测口径（token 列真实值 + `compute_cost_breakdown` 计价，与
/// organ-5 `record_round_usage` 同一套公式）；usage 缺失 = estimated 注记
/// 行（字符数 + `ceil/4` 估算、token/计价列全 0——估算值绝不冒充实测入
/// 计价，§7 口径：estimated 行只进用量可见性不进计价总额）。空输出永远
/// 到不了这里（护栏已按失败处理）→ 无需 usage_missing 臂。
fn write_aux_ledger_row(
    ledger: &AuxLedger<'_>,
    usage: Option<&crate::loop_executor::ObserverUsageInfo>,
    content: &str,
    latency: std::time::Duration,
) {
    let Some(ds) = ledger.data_store else {
        return;
    };
    let error_message = match usage {
        Some(_) => None,
        None => {
            let n = content.chars().count();
            let est = (n as u64).div_ceil(4);
            Some(format!(
                "[estimated] {}: 输出 {n} 字符 ≈ {est} output tokens（估算值不入计价；provider 未回传实测 usage）",
                ledger.aux_kind
            ))
        }
    };
    let (input_tokens, output_tokens, cache_creation, cache_read, pricing) = if let Some(u) = usage
    {
        let cache_creation = u.cache_creation_tokens.unwrap_or(0);
        let cache_read = u.cache_read_tokens.or(u.cached_tokens).unwrap_or(0);
        let breakdown = ds.compute_cost_breakdown(
            ledger.model,
            u.prompt_tokens,
            u.completion_tokens,
            cache_creation,
            cache_read,
        );
        let empty = nemesis_data::CostBreakdown::default();
        let bd = breakdown.as_ref().unwrap_or(&empty);
        (
            u.prompt_tokens,
            u.completion_tokens,
            cache_creation,
            cache_read,
            (
                bd.total_cost_usd,
                bd.pricing_model.clone(),
                bd.input_cost_usd,
                bd.output_cost_usd,
                bd.cache_creation_cost_usd,
                bd.cache_read_cost_usd,
            ),
        )
    } else {
        (0, 0, 0, 0, (0.0, String::new(), 0.0, 0.0, 0.0, 0.0))
    };
    let (
        total_cost_usd,
        pricing_model,
        input_cost_usd,
        output_cost_usd,
        cache_creation_cost_usd,
        cache_read_cost_usd,
    ) = pricing;
    let log = nemesis_data::RequestLog {
        id: 0,
        trace_id: format!(
            "aux-{}-{}",
            ledger.aux_kind,
            chrono::Local::now().timestamp_millis()
        ),
        model: ledger.model.to_string(),
        provider_type: ledger.aux_kind.to_string(),
        input_tokens,
        output_tokens,
        cache_creation_tokens: cache_creation,
        cache_read_tokens: cache_read,
        total_cost_usd,
        latency_ms: latency.as_millis() as i64,
        status_code: 200,
        error_message,
        is_streaming: false,
        created_at: chrono::Local::now().timestamp(),
        pricing_model,
        input_cost_usd,
        output_cost_usd,
        cache_creation_cost_usd,
        cache_read_cost_usd,
        first_token_ms: None,
        session_key: ledger.session_key.unwrap_or("").to_string(),
    };
    if let Err(e) = ds.insert_request_log(&log) {
        tracing::warn!(
            "[bypass:{}] Failed to record aux ledger row: {e}",
            ledger.aux_kind
        );
    }
}
