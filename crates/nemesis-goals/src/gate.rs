//! Goals 纯逻辑（零 IO，全部可单测）。
//!
//! 状态转移校验 / 预算判定 / 评估提示词构建 / 判定解析 / goal 轮注入消息
//! 构建 / 文件保护裁决。评估闸驱动器（gateway 侧 goals_engine）的全部
//! 裁决都收在此处，宿主只管 IO 与 LLM 调用（nemesis-loop wake.rs 同款
//! 分层）。

use crate::state::{EvalVerdict, GoalCreatedBy, GoalEntry, GoalStatus};
use crate::store::{GOALS_DIR_NAME, GOALS_REGISTRY_FILE};

/// outbond 帧是否构成「turn 结束」评估触发：带会话键的终局回复（回执卡
/// `receipt=true` 与瞬态帧 `occupies=Some(false)` 均非回合产物，跳过）。
pub fn should_eval_on_outbound(
    session_key: Option<&str>,
    receipt: bool,
    occupies: Option<bool>,
) -> bool {
    let Some(key) = session_key else {
        return false;
    };
    !key.is_empty() && !receipt && occupies != Some(false)
}

/// 状态转移校验（全表；非法转移调用方 loud 拒绝）。
///
/// - `active → paused`：人 disable；`active → blocked`：引擎停车（预算
///   耗尽 / 评估失败）；`active → done`：评估闸判定；
/// - `paused → active` / `blocked → active`：人 enable（blocked 解锁后
///   下一 turn 评估闸照常，预算/失败面不变）；
/// - `done` 终态：不转出（替换 = 新建 goal，D-Q8）。
pub fn transition_allowed(from: GoalStatus, to: GoalStatus) -> bool {
    use GoalStatus::*;
    matches!(
        (from, to),
        (Active, Paused)
            | (Active, Blocked)
            | (Active, Done)
            | (Paused, Active)
            | (Blocked, Active)
    )
}

/// 预算判定：耗尽返回 Some((code, message))——本轮**跳过评估**直接停车
/// （不猜 CONTINUE，D-Q10）。None = 继续评估。
pub fn budget_block_reason(entry: &GoalEntry) -> Option<(String, String)> {
    if entry.max_rounds > 0 && entry.rounds_used >= entry.max_rounds {
        return Some((
            "round-limit".to_string(),
            format!(
                "轮数预算耗尽（{}/{} 轮），评估闸停车；人工评估后 enable 解锁或调整预算",
                entry.rounds_used, entry.max_rounds
            ),
        ));
    }
    None
}

/// 单次工具调用的评估材料（引擎从 ToolStarted/ToolFinished 事件按 call_id
/// 关联装配）。args/结果原样透传（事件层 args 200 字符 / result 1KB 预览
/// 是既有 infra 契约，此处不追加截断）。
#[derive(Debug, Clone)]
pub struct ToolCallMaterial {
    pub tool: String,
    pub args_preview: String,
    /// None = 未观测到返回（工具悬挂 / 评估窗口错过 ToolFinished）。
    pub ok: Option<bool>,
    pub result_preview: String,
}

/// 构建评估提示词（材料：goal.md 全文 + 本轮 user 消息 + 最终回复 + 本轮
/// 工具调用记录（含实际返回）+ 轮数上下文——本轮材料面，不带全量会话历史）。
pub fn build_eval_prompt(
    goal_md: &str,
    user_message: &str,
    final_reply: &str,
    tool_calls: &[ToolCallMaterial],
    round: u64,
    max_rounds: u64,
) -> String {
    let tools = if tool_calls.is_empty() {
        "（本轮未调用工具）".to_string()
    } else {
        let mut lines = Vec::with_capacity(tool_calls.len());
        for (i, c) in tool_calls.iter().enumerate() {
            let outcome = match c.ok {
                Some(true) => "成功",
                Some(false) => "失败",
                None => "未观测到返回",
            };
            let args = c.args_preview.trim();
            let args = if args.is_empty() { "-" } else { args };
            let result = c.result_preview.trim();
            let result = if result.is_empty() {
                "（空）"
            } else {
                result
            };
            lines.push(format!(
                "{}. {} — 参数: {} → {}，结果: {}",
                i + 1,
                c.tool,
                args,
                outcome,
                result
            ));
        }
        lines.join("\n")
    };
    format!(
        "你是目标评估闸（独立评审，不采信执行方的自我申报）。判断下面的执行回合是否已完整达成长期目标。\n\n\
         # 长期目标（goal.md 全文）\n{goal_md}\n\n\
         # 本轮用户消息\n{user_message}\n\n\
         # 本轮 agent 最终回复\n{final_reply}\n\n\
         # 本轮工具调用记录（含实际返回）\n{tools}\n\n\
         # 轮数进度\n第 {round} 轮 / 上限 {max}（{unlimited}）\n\n\
         评估纪律：\n\
         - 独立核验：只根据目标与验收标准判断，agent 在回复里自称「完成」不构成完成；\
         效果主张以工具调用记录中的实际返回为证据。\n\
         - 严格标准：目标里的每条验收标准都必须满足；存在缺口就 CONTINUE。\n\
         - CONTINUE 时 gap 必须具体指出还差什么、下一步该做什么。\n\n\
         只输出一行 JSON，不要输出其他内容：\n\
         {{\"verdict\":\"DONE\"}}\n或\n\
         {{\"verdict\":\"CONTINUE\",\"gap\":\"<具体缺口>\"}}",
        goal_md = goal_md.trim(),
        user_message = truncate_str(user_message, 4000),
        final_reply = truncate_str(final_reply, 4000),
        tools = tools,
        round = round,
        max = max_rounds,
        unlimited = if max_rounds == 0 { "0=不限" } else { "达到上限将停车" },
    )
}

/// 解析评估输出（宽松：剥 markdown 围栏后取首个 `{` 到末个 `}`；verdict
/// 大小写不敏感；gap 缺省 None）。解析失败 = Err（调用方重试后停车）。
pub fn parse_verdict(raw: &str) -> Result<(EvalVerdict, Option<String>), String> {
    let body = raw.trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .unwrap_or(body)
        .strip_suffix("```")
        .unwrap_or(body)
        .trim();
    let start = body.find('{').ok_or("no JSON object found")?;
    let end = body.rfind('}').ok_or("no JSON object closing")?;
    if end < start {
        return Err("malformed JSON object".to_string());
    }
    let json: serde_json::Value =
        serde_json::from_str(&body[start..=end]).map_err(|e| format!("JSON parse failed: {e}"))?;
    let verdict_raw = json
        .get("verdict")
        .and_then(|v| v.as_str())
        .ok_or("missing \"verdict\" string field")?;
    let verdict = match verdict_raw.to_ascii_lowercase().as_str() {
        "done" => EvalVerdict::Done,
        "continue" => EvalVerdict::Continue,
        other => return Err(format!("unknown verdict: {other}")),
    };
    let gap = json
        .get("gap")
        .and_then(|g| g.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok((verdict, gap))
}

/// 构建 goal 轮注入消息（`<goal_round>` 块惯例；经 bus 入站注入开启下一轮）。
pub fn build_round_message(id: &str, round: u64, max_rounds: u64, gap: Option<&str>) -> String {
    let gap_line = match gap {
        Some(g) if !g.trim().is_empty() => g.trim().to_string(),
        _ => "（评估器未给出缺口注记，请对照目标自查进度）".to_string(),
    };
    format!(
        "<goal_round id=\"{id}\" round=\"{round}\" max=\"{max}\">\n\
         目标未达成，评估闸判定继续。缺口：\n{gap_line}\n\n\
         请继续推进目标（目标全文见 goals/{id}.goal.md，每次动手前重读）；\
         本轮结束时评估闸会再次核验，完成前请勿自行宣告完成。\n\
         </goal_round>",
        max = max_rounds
    )
}

/// 文件保护裁决（创建者二分）：`rel` 为相对 workspace 根的路径字符串
/// （`/` 或 `\` 分隔皆可，内部归一），`created_by_of` 按目标 id 回查创建
/// 者（未知 id = None）。返回 Some(理由) = 拒绝写/删；None = 放行。
///
/// 保护面（仅 WriteDelete family 消费，读放行）：
/// - `config/goals.json` 注册表恒拒（agent 无状态写面）；
/// - `goals/{id}.goal.md` 人建拒（只读，改走 Dashboard）、agent 自建放行；
/// - `goals/{id}.evals.jsonl` 评估台账恒拒；
/// - `goals/` 下其余任何路径（杂散新文件/子目录）与 `goals` 目录本身
///   （整目录删除）恒拒。
pub fn fence_denial(
    rel: &str,
    created_by_of: &dyn Fn(&str) -> Option<GoalCreatedBy>,
) -> Option<String> {
    let norm = rel.replace('\\', "/");
    let parts: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        // 目录本身或其下任何内容
        [head, rest @ ..] if head.eq_ignore_ascii_case(GOALS_DIR_NAME) => {
            match rest {
                [name] => {
                    if let Some(id) = name.strip_suffix(".goal.md") {
                        match created_by_of(id) {
                            Some(GoalCreatedBy::Agent) => None, // 自有，放行
                            Some(GoalCreatedBy::Human) => {
                                Some("人工创建的目标文件对 agent 只读；如需调整请通过 Dashboard 编辑或与用户确认".to_string())
                            }
                            None => Some(format!(
                                "goals/ 下出现未注册文件 {name}；goal 文件须由 goal 工具或 Dashboard 创建"
                            )),
                        }
                    } else if name.to_ascii_lowercase().ends_with(".evals.jsonl") {
                        Some("评估台账（evals.jsonl）由评估闸维护，agent 不可写".to_string())
                    } else {
                        Some(format!(
                            "goals/ 目录只接受 goal 文件（{{id}}.goal.md），拒绝其他写入：{name}"
                        ))
                    }
                }
                _ => Some("goals/ 目录内不允许子目录或批量写入".to_string()),
            }
        }
        ["config", name] if name.eq_ignore_ascii_case(GOALS_REGISTRY_FILE) => {
            Some("目标注册表（config/goals.json）由系统维护，agent 不可写".to_string())
        }
        _ => None,
    }
}

/// 从 chat_log 窗口行提取本轮材料：最后一条 user 行 + 最后一条 assistant
/// 行（各自独立取——多轮窗口内 user 在前 assistant 在后；boundary 行由
/// read_chat_log 过滤不入窗口）。content 非字符串形态（历史兼容）按缺省。
pub fn extract_turn_material(rows: &[serde_json::Value]) -> (Option<String>, Option<String>) {
    let mut user = None;
    let mut reply = None;
    for row in rows {
        match row.get("role").and_then(|v| v.as_str()) {
            Some("user") => {
                user = row
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(String::from)
            }
            Some("assistant") => {
                reply = row
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(String::from)
            }
            _ => {}
        }
    }
    (user, reply)
}

/// 提示词材料截断封顶（评估材料防超长烧 token；尾部保留——结论多在末尾）。
fn truncate_str(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let skip = s.chars().count() - cap;
    let tail: String = s.chars().skip(skip).collect();
    format!("…（前文截断）{tail}")
}
