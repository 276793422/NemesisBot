//! 必检计划（P2 外循环，2026-10-25）：「计划内检查没跑 = 交付不完整」。
//!
//! 任务正文/验收标准里反引号包裹的多词命令（`` `cargo test` `` 等）= 本单
//! 的**必检计划**——评审收口时对账 worker 交付证据里的 `checks[]`（管线性
//! 确定性记账，`checks_capture`），计划内没跑的检查 → 交付不完整（PASS 降
//! 级 FAIL），对账表同时注入评审 prompt（评审 agent 读对账结果，不再凭
//! worker 自述「已测试」放行）。标题即纪律：**SKIP≠PASS**。
//!
//! 设计取舍：
//! - **纯函数零存储**：计划是 issue 文本的确定性派生（创建后文本即计划），
//!   评审时现算——无 schema 迁移、无漂移面。闸的启用由全局
//!   `board.review.require_check_plan` 控制（默认 false 烧入期，对齐
//!   `require_delivery_evidence` 的推广路径）。
//! - **保守派生**：只认「反引号 + 多词 + 命令形首 token」的片段（`[CHECK]`
//!   行已归锚点双检管，不重复计）；不猜语言/扩展名（把 `.rs 文件改成 x`
//!   当成必检是造计划，不是计划）。
//! - **对账按 cmd 归一子串匹配**：计划 `cargo test` 被执行 `cargo test
//!   --lib` 覆盖（跑得更细 = 覆盖）；多次运行任一失败 = 该项失败（诚实
//!   AND）。匹配不到任何执行记录 = 未跑（含 checks 块整体缺席——没跑过
//!   检查的诚实缺席形态，本闸正是为它存在）。

/// 单条必检计划项的对账结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckPlanStatus {
    /// 计划项原文（反引号内的命令）。
    pub check: String,
    /// 是否有执行证据覆盖（checks[] 里存在 cmd 匹配项）。
    pub ran: bool,
    /// 执行证据的成败（`ran=false` 时恒 None；多次运行任一失败 = false）。
    pub ok: Option<bool>,
}

/// 从单据文本派生必检计划（title/description/AC 逐段喂入；顺序保持，
/// 去重保首个）。`[CHECK]` 行剔除——锚点声明行里的命令归锚点双检管，
/// 不重复计计划（同一命令两条闸双报 = 噪声）。
pub fn derive_check_plan(texts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in texts {
        for line in text.lines() {
            if line.contains("[CHECK]") {
                continue;
            }
            for span in backtick_spans(line) {
                if is_plausible_command(&span) && !out.contains(&span) {
                    out.push(span);
                }
            }
        }
    }
    out
}

/// 单行内反引号片段提取（成对配对；奇数个反引号的尾部残片忽略——无闭合
/// 反引号的「片段」是行文残尾，不是代码片段）。
fn backtick_spans(line: &str) -> Vec<String> {
    let mut parts = line.split('`');
    let mut out = Vec::new();
    parts.next(); // 丢弃首片段前的文本
    while let Some(span) = parts.next() {
        match parts.next() {
            // 有闭合反引号（后随文本段在场）= 真片段；无 = 奇数尾部残片。
            Some(_) => out.push(span.to_string()),
            None => break,
        }
    }
    out
}

/// 片段是否像一条可执行命令（保守判定）：
/// - 多词（含空白）——单词片段（`TODO`、文件名）不是命令；
/// - 无控制字符、长度有界（≤200 chars）；
/// - 首 token 命令形：ASCII 字母/数字开头，字符集 `[A-Za-z0-9_@./:+-]`，
///   ≥2 字符，不以 `.` / `:` 结尾（`e.g.` 之类的行文缩写不是命令）。
fn is_plausible_command(span: &str) -> bool {
    let s = span.trim();
    if s.chars().count() > 200 || s.chars().any(char::is_control) {
        return false;
    }
    let mut tokens = s.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if tokens.next().is_none() {
        return false; // 单词 = 不是命令
    }
    let mut chars = first.chars();
    let Some(c0) = chars.next() else {
        return false;
    };
    if !c0.is_ascii_alphanumeric() {
        return false;
    }
    if first.len() < 2 || first.len() > 64 || first.ends_with('.') || first.ends_with(':') {
        return false;
    }
    first
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '/' | ':' | '+' | '-'))
}

/// 命令归一（连续空白折叠为单空格）——匹配在归一形态上进行。
fn normalize_cmd(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 必检计划 × 执行证据对账（纯函数）：计划项按「归一子串」匹配 checks[]
/// 的 cmd（计划 `cargo test` ⊂ 执行 `cargo test --lib` = 覆盖）；多条匹配
/// 任一失败 = 该项失败（诚实 AND）。`checks` 条目缺 cmd / 缺 ok = 形态非
/// 法条目，跳过不匹配（形态闸在评审门另有单点，这里只做覆盖判定）。
pub fn reconcile_check_plan(plan: &[String], checks: &[serde_json::Value]) -> Vec<CheckPlanStatus> {
    plan.iter()
        .map(|want| {
            let want_n = normalize_cmd(want);
            let mut ok: Option<bool> = None;
            for c in checks {
                let Some(cmd) = c.get("cmd").and_then(|v| v.as_str()) else {
                    continue;
                };
                if normalize_cmd(cmd).contains(&want_n) {
                    let passed = c.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    ok = Some(match ok {
                        None => passed,
                        Some(prev) => prev && passed,
                    });
                }
            }
            CheckPlanStatus {
                check: want.clone(),
                ran: ok.is_some(),
                ok,
            }
        })
        .collect()
}

/// 对账表渲染（评审 prompt 注入 / 留痕评论共用；逐条可读）。
pub fn render_check_plan_reconciliation(statuses: &[CheckPlanStatus]) -> String {
    let mut out = String::new();
    for s in statuses {
        match (s.ran, s.ok) {
            (true, Some(true)) => out.push_str(&format!("- ✅ `{}`：已执行，通过\n", s.check)),
            (true, _) => out.push_str(&format!(
                "- ❌ `{}`：已执行，**失败**（失败也是证据，按失败处置）\n",
                s.check
            )),
            (false, _) => out.push_str(&format!(
                "- ⏭ `{}`：**未执行**——计划内必检项没跑，交付按不完整处置（SKIP≠PASS）\n",
                s.check
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests;
