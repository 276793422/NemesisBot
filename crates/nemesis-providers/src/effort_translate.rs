//! 强度翻译（U1，2026-11-09）：reasoning_effort 配置 → openai 兼容请求体补丁。
//!
//! 背景：强度配置（off/low/medium/high）在 anthropic/codex lane 已消费，
//! 但 HttpCompat（openai 兼容，覆盖面最广的默认 lane）此前零消费——配置
//! 静默无效，杂务省 token 闸（aux 强制 off）也落空：开思考模型在
//! /compact、标题生成、judge 等杂务上照样烧思考 token。
//!
//! 设计纪律：**保守识别、不猜**。只对识别得出的厂商发私有参数；识别不出
//! 的走 OpenAI 官方 `reasoning_effort` 字段（low/medium/high），`off` 非官方
//! 合法值 → 不发（防 400）。effort 未配置（空）→ 零注入，请求体与现网
//! 字节一致（存量用户零变化）。显式 `extra_body` > 自动翻译（用户意志最大）。
//!
//! 结构（2026-11-09 按用户拍板重构）：厂商知识 = [`REGISTRY`] 里的**数据
//! 条目**（`Dialect` 接口：识别谓词 + 补丁函数），派发器只查表调接口，
//! 不含任何厂商 if-else。**新增厂商 = 在 REGISTRY 追加一条表项 + 两个小
//! 函数，三步全在本文件表区，派发逻辑零改动**。表有序（先专后通），首条
//! 命中即用；兜底词条恒真恒在末位。

use serde_json::Value;

/// 请求体强度翻译 + extra_body 合并（HttpCompat lane 专用，body 组装末段调用）。
///
/// - `effort`：ChatOptions.reasoning_effort 解析值（"" = 未配置，零注入；
///   未知档位同样零注入——不为垃圾值发明厂商键，防御臂）；
/// - `streaming`：本次请求是否流式（Qwen3 混合思考的 enable_thinking:true
///   在非流式请求上会被端点 400 拒绝——仅流式放行，非流式诚实跳过）；
/// - `extra_body`：模型条目 extra 的显式逃生舱（Null = 无；对象 = 浅合并，
///   键级覆盖自动翻译结果）。
pub fn apply_effort_to_body(
    body: &mut Value,
    model: &str,
    api_base: &str,
    effort: &str,
    streaming: bool,
    extra_body: &Value,
) {
    let effort = effort.trim();
    if let Some(tier) = Effort::parse(effort) {
        translate_by_registry(body, model, api_base, tier, streaming);
    }
    merge_extra_body(body, extra_body);
}

// ---------------------------------------------------------------------------
// 接口层：档位 + 方言词条（厂商知识只住在本节与下方 REGISTRY 表）
// ---------------------------------------------------------------------------

/// 统一档位（解析一次，词条免重复拆串；大小写不敏感，未知值不进词条）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Effort {
    Off,
    Low,
    Medium,
    High,
}

impl Effort {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// 厂商方言接口：识别 + 补丁。派发器（`translate_by_registry`）只消费此
/// 接口——新增厂商不碰派发逻辑。
///
/// - `matches`：保守识别谓词（入参为小写后的模型名与 api_base）。认不出
///   就别抢——兜底词条恒真恒在末位；
/// - `apply`：把档位写进请求体。某档位表达不了（恒思考族的 off、非流式
///   的 qwen3 true）就不写任何键 = 诚实跳过，派发器不追问原因。
struct Dialect {
    /// 条目名（诊断与测试定位用，不进 wire）
    name: &'static str,
    matches: fn(model: &str, base: &str) -> bool,
    apply: fn(body: &mut Value, effort: Effort, streaming: bool),
}

/// 厂商方言注册表（**有序数据表，先专后通，首条命中即用**）。
///
/// 新增厂商三步（全在本表区，零派发改动）：
/// 1. 写识别谓词 `fn is_xxx(model: &str, base: &str) -> bool`（保守：只认
///    明确名字前缀或自家域名，认不出不抢）；
/// 2. 写补丁函数 `fn apply_xxx(body, effort, streaming)`（表达不了的档位
///    不写键 = 诚实跳过）；
/// 3. 在本表对应位置插入一条 `Dialect`（专有词条必须排在兜底之前）。
static REGISTRY: &[Dialect] = &[
    // DeepSeek 推理系（deepseek-reasoner / deepseek-r1）：无思考开关、
    // 恒思考——任何档位都不发参数（诚实跳过；发 thinking 键会被拒）。
    Dialect {
        name: "deepseek-reasoner",
        matches: is_deepseek_reasoner,
        apply: apply_silent,
    },
    // GLM-5.x 恒思考族（2026-11-09 实机探测，bigmodel v4 端点）：glm-5
    // 前缀（glm-5.3 / glm-5.3-flash 实测）不支持关闭思考——`thinking:
    // {type:"disabled"}` 报 1210「该模型始终思考，不支持关闭思考；请使用
    // low、high 或 max」。原生档位控制 = `thinking:{type:"enabled",level}`：
    // vendor 无 medium，保序满射 low→low、medium→high、high→max。off =
    // 关不掉，诚实跳过不发键（DeepSeek-R1 同款纪律；代价：杂务省 token 闸
    // 在这族模型上无强制力——vendor 限制，非本层 bug）。
    Dialect {
        name: "glm-5-always-thinking",
        matches: is_glm5_always_thinking,
        apply: apply_glm5_level,
    },
    // GLM（zhipu 开放平台 openai 兼容端点，glm-4.x 等可开关族）：
    // thinking:{type} 单开关，无细档——off 关、其余档开（诚实不细分）。
    Dialect {
        name: "glm-switchable",
        matches: is_glm,
        apply: apply_glm_switch,
    },
    // Qwen3 混合思考（dashscope）：enable_thinking 布尔。非流式请求带
    // true 会被端点 400（incremental_output 限制）——仅流式放行 true，
    // 非流式诚实跳过（维持端点默认）。
    Dialect {
        name: "qwen3-hybrid-thinking",
        matches: is_qwen3,
        apply: apply_qwen3_enable_thinking,
    },
    // 其余/未知厂商（兜底恒真，恒在末位）：low/medium/high 走 OpenAI 官方
    // 字段直传（兼容端点普遍忽略未知字段）；off 非官方合法值，不发（防 400）。
    Dialect {
        name: "openai-official-fallback",
        matches: match_any,
        apply: apply_openai_effort,
    },
];

// ---------------------------------------------------------------------------
// 识别谓词（保守：只认明确名字前缀或自家域名）
// ---------------------------------------------------------------------------

fn is_deepseek_reasoner(model: &str, _base: &str) -> bool {
    model.contains("deepseek-reasoner") || model.contains("deepseek-r1")
}

fn is_glm5_always_thinking(model: &str, _base: &str) -> bool {
    model.contains("glm-5") || model.contains("glm5")
}

fn is_glm(model: &str, base: &str) -> bool {
    model.contains("glm") || base.contains("bigmodel.cn") || base.contains("z.ai")
}

fn is_qwen3(model: &str, base: &str) -> bool {
    model.contains("qwen3") || base.contains("dashscope")
}

fn match_any(_model: &str, _base: &str) -> bool {
    true
}

// ---------------------------------------------------------------------------
// 补丁函数（表达不了的档位不写键 = 诚实跳过）
// ---------------------------------------------------------------------------

/// 恒思考无开关族：任何档位零注入。
fn apply_silent(_body: &mut Value, _effort: Effort, _streaming: bool) {}

/// GLM-5.x 原生 level 档位（off 关不掉 → 诚实跳过）。
fn apply_glm5_level(body: &mut Value, effort: Effort, _streaming: bool) {
    if effort == Effort::Off {
        return;
    }
    let level = match effort {
        Effort::Low => "low",
        Effort::Medium => "high",
        // vendor 无 medium：medium 归 high；Off 臂不可达（上方已返回）。
        _ => "max",
    };
    body["thinking"] = serde_json::json!({ "type": "enabled", "level": level });
}

/// GLM 可开关族单开关。
fn apply_glm_switch(body: &mut Value, effort: Effort, _streaming: bool) {
    let t = if effort == Effort::Off {
        "disabled"
    } else {
        "enabled"
    };
    body["thinking"] = serde_json::json!({ "type": t });
}

/// Qwen3 混合思考布尔。
fn apply_qwen3_enable_thinking(body: &mut Value, effort: Effort, streaming: bool) {
    match effort {
        Effort::Off => body["enable_thinking"] = serde_json::json!(false),
        _ if streaming => body["enable_thinking"] = serde_json::json!(true),
        // 非流式 true 会被端点 400 —— 诚实跳过（维持端点默认）。
        _ => {}
    }
}

/// OpenAI 官方字段兜底。
fn apply_openai_effort(body: &mut Value, effort: Effort, _streaming: bool) {
    if effort != Effort::Off {
        let wire = match effort {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Off => "off", // 不可达（上方已排除）。
        };
        body["reasoning_effort"] = serde_json::json!(wire);
    }
}

// ---------------------------------------------------------------------------
// 派发器（不含任何厂商知识——厂商增删零改此处）
// ---------------------------------------------------------------------------

/// 查表派发：首条命中即用（先专后通；兜底恒真恒在末位）。
fn translate_by_registry(
    body: &mut Value,
    model: &str,
    api_base: &str,
    effort: Effort,
    streaming: bool,
) {
    let model_l = model.to_lowercase();
    let base_l = api_base.to_lowercase();
    if let Some(dialect) = REGISTRY.iter().find(|d| (d.matches)(&model_l, &base_l)) {
        // 命中词条名落 debug 日志——实机排查「档位到底走了哪条方言」时
        // 直接可查（gateway log 开 debug 即见）。
        tracing::debug!(
            target: "provider::effort",
            dialect = dialect.name,
            model,
            tier = ?effort,
            "reasoning effort translated by dialect"
        );
        (dialect.apply)(body, effort, streaming);
    }
}

/// 显式 extra_body 浅合并：对象键级覆盖自动翻译（用户意志最大）；
/// Null/非对象 = 无逃生舱，零合并。
fn merge_extra_body(body: &mut Value, extra_body: &Value) {
    if let Some(obj) = extra_body.as_object() {
        for (k, v) in obj {
            body[k.as_str()] = v.clone();
        }
    }
}

#[cfg(test)]
mod tests;
