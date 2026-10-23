//! policy-sample 示例插件（guest 侧，world `plugin-loop-policy`）。
//!
//! 三方开发者照这里写循环策略插件：实现
//! `exports::nemesis::plugin::loop_policy::Guest` 四方法
//! （`get_policy_metadata` 自报接管面 + 三策略点各一），SDK 宏
//! [`nemesis_plugin_sdk::export_loop_policy!`] 收尾。
//!
//! 本插件是 AgentLoop 三策略点的**标记驱动行为夹具**，演示每点的
//! 「最小可观测差异 + builtin 回退」：
//!
//! - `validate-args`：args 含 `__REJECT__` 的 exec 调用拒绝；write_file
//!   的路径含 `__autofix_this__` 自动改写；其余一律 builtin（零干扰回退）。
//! - `judge-final`：content 含 `__FORCE_GIVEUP__` 终止轮次；含
//!   `__FORCE_RETRY__` 要求重答（`__LLM_NUDGE__` 时改走宿主
//!   llm-complete 合成提醒文案，失败回退静态文案）；否则 builtin。
//! - `compact-check`：trigger 已过软位（`soft>0`，软位 token 位置）一律
//!   skip（组件压制压缩时机）、硬位回宿主；exec 已有摘要（含
//!   `__NO_RECOMPACT__`）skip、否则产出确定性占位摘要。
//!
//! 设计要点（与 WIT 合同对应）：
//! - 出参统一支持 `{"verdict":"builtin"}` = 单次显式回退宿主内置逻辑——
//!   组件只接管自己有把握的分支，其余交还原路径（渐进定制的基础）。
//! - 入参解析用 serde_json 正经解析（夹具级的子串匹配见 translate）。
//! - 宿主硬闸在组件决策之上：空摘要拒绝 / 文本钳制 / fixed args 必须可
//!   解析——组件侧同样自律，不产出越界文本。
//!
//! 构建：`cargo build --target wasm32-wasip2 --release`
//! 产物：`target/wasm32-wasip2/release/policy_sample_plugin.wasm`

use serde::Deserialize;

use nemesis::plugin::host;

struct PolicySamplePlugin;

// ---------------------------------------------------------------------------
// 入参投影（WIT 合同形状；全部字段缺省容忍——畸形输入按 builtin 回退）
// ---------------------------------------------------------------------------

// 入参投影（WIT 合同形状；全部字段缺省容忍——畸形输入按 builtin 回退）。
// 字段按合同全形保留（结构性文档），未消费字段允许 dead_code。
// compact trigger 投影（`{"point":"trigger",...}`；数值全为宿主侧账本值，
// 含 `soft`——软位 token 位置，0 = 未过软位）。
#[allow(dead_code)]
#[derive(Deserialize, Default)]
struct TriggerInput {
    #[serde(default)]
    session_key: String,
    #[serde(default)]
    tail_tokens: u64,
    #[serde(default)]
    provider_signal: u64,
    #[serde(default)]
    signal: u64,
    #[serde(default)]
    threshold: u64,
    #[serde(default)]
    soft: u64,
    #[serde(default)]
    window: u64,
    #[serde(default)]
    covers_up_to: u64,
    #[serde(default)]
    new_boundary: u64,
}

/// compact prefix 条目（content 已被宿主截断到投影上限）。
#[allow(dead_code)]
#[derive(Deserialize, Default)]
struct PrefixTurn {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_name: Option<String>,
}

/// compact exec 投影（`{"point":"exec",...}`）。
#[allow(dead_code)]
#[derive(Deserialize, Default)]
struct ExecInput {
    #[serde(default)]
    session_key: String,
    #[serde(default)]
    prefix: Vec<PrefixTurn>,
    #[serde(default)]
    existing_summary: String,
    #[serde(default)]
    window: u64,
    #[serde(default)]
    model: String,
}

/// judge-final 投影（content 已被宿主截断到投影上限）。
#[allow(dead_code)]
#[derive(Deserialize, Default)]
struct JudgeInput {
    #[serde(default)]
    session_key: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    turns_used: u32,
    #[serde(default)]
    validation_failures: u32,
}

/// validate-args 投影（args = 原始 JSON **串**，未解析——与内置 autofix
/// 消费形态一致；schema 可为 null = 工具未声明）。
#[allow(dead_code)]
#[derive(Deserialize, Default)]
struct ValidateInput {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    schema: serde_json::Value,
    #[serde(default)]
    args: String,
}

/// 解析失败一律 builtin（组件不猜，宿主走内置逻辑——与宿主侧畸形裁决
/// 同语义的反向形态）。
fn parse_or_builtin<T: for<'de> Deserialize<'de> + Default>(input: &str) -> T {
    serde_json::from_str(input).unwrap_or_default()
}

fn builtin() -> Result<String, host::HostError> {
    Ok(r#"{"verdict":"builtin"}"#.to_string())
}

/// retry nudge 合成：`__LLM_NUDGE__` 时走宿主 llm-complete（small_model
/// 杂务槽 + 护栏同源：estop fail-closed / token 钳制 / 墙钟超时）；宿主
/// 无 invoker 接线（单测/离线形态）时诚实回退静态文案。
fn synthesize_nudge() -> String {
    if let Ok(text) = host::llm_complete("用一句不超过30字的话，提醒模型修正它刚才的回答。", 64) {
        let t = text.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    "policy-sample: 回答包含 __FORCE_RETRY__ 标记，请修正后重新作答".to_string()
}

impl exports::nemesis::plugin::loop_policy::Guest for PolicySamplePlugin {
    /// 安装期对账 + 运行期路由的唯一事实源：本样例三点全接管（每点内部
    /// 再按分支回退 builtin，接管 ≠ 全程接管）。
    fn get_policy_metadata(
    ) -> Result<exports::nemesis::plugin::loop_policy::PolicyMetadata, host::HostError> {
        Ok(exports::nemesis::plugin::loop_policy::PolicyMetadata {
            compact: true,
            final_judge: true,
            validation: true,
        })
    }

    fn compact_check(input: String) -> Result<String, host::HostError> {
        let v: serde_json::Value = match serde_json::from_str(&input) {
            Ok(v) => v,
            Err(_) => return builtin(),
        };
        match v.get("point").and_then(serde_json::Value::as_str) {
            Some("trigger") => {
                let t: TriggerInput = parse_or_builtin(&input);
                if t.soft > 0 {
                    // 组件观察：已过软位（soft = 软位 token 位置，非零）不
                    // 触发压缩——可观测差异（宿主内置会压，这里压制给
                    // skip）；硬位裁决交宿主默认阈值逻辑（builtin 回退）。
                    Ok(r#"{"verdict":"skip"}"#.to_string())
                } else {
                    builtin()
                }
            }
            Some("exec") => {
                let e: ExecInput = parse_or_builtin(&input);
                if e.existing_summary.contains("__NO_RECOMPACT__") {
                    // 已有组件摘要时不再重压。
                    return Ok(r#"{"verdict":"skip"}"#.to_string());
                }
                // 确定性占位摘要：内容可预测（契约测试断言用），省幅由宿主
                // B2 无进展守卫复检——组件不越权保证省幅。
                let summary = format!(
                    "[policy-sample] 已折叠 {} 条历史（window={}，model={}）",
                    e.prefix.len(),
                    e.window,
                    e.model
                );
                Ok(serde_json::json!({ "verdict": "summary", "summary": summary }).to_string())
            }
            _ => builtin(),
        }
    }

    fn judge_final(input: String) -> Result<String, host::HostError> {
        let j: JudgeInput = parse_or_builtin(&input);
        if j.content.contains("__FORCE_GIVEUP__") {
            return Ok(serde_json::json!({
                "verdict": "giveup",
                "notice": "policy-sample: 回答包含 __FORCE_GIVEUP__ 标记，本轮终止并转人工说明",
            })
            .to_string());
        }
        if j.content.contains("__FORCE_RETRY__") {
            return Ok(
                serde_json::json!({ "verdict": "retry", "nudge": synthesize_nudge() }).to_string()
            );
        }
        builtin()
    }

    fn validate_args(input: String) -> Result<String, host::HostError> {
        let w: ValidateInput = parse_or_builtin(&input);
        if w.tool == "exec" && w.args.contains("__REJECT__") {
            return Ok(serde_json::json!({
                "verdict": "reject",
                "message": "policy-sample: 命令包含 __REJECT__ 标记，已被策略组件拒绝",
            })
            .to_string());
        }
        if w.tool == "write_file" && w.args.contains("__autofix_this__") {
            // fixed args 宿主硬闸：必须可解析为 JSON——标记替换不引入任何
            // 需转义字符，合法性不变。
            let fixed = w.args.replace("__autofix_this__", "autofixed-by-policy-sample");
            return Ok(serde_json::json!({ "verdict": "fixed", "args": fixed }).to_string());
        }
        // 其余工具调用零干扰（高频路径 builtin 直通宿主内置校验）。
        builtin()
    }
}

nemesis_plugin_sdk::export_loop_policy!(PolicySamplePlugin);
