//! eval 结果评估器（assessor）——规则驱动三分类。
//!
//! Plan: docs/PLAN/2026-08-17_eval-result-assessor.md
//!
//! 纯函数评估：报告目录是唯一输入（可回放、可单测），不依赖运行时状态。
//! 读 `workspace/config/eval_rules.json`（不存在 → 种子内置默认集再加载；
//! 默认集唯一定义在 `config/eval_rules.default.json`，经 include_str 引用）。
//!
//! 三分类（优先级从上到下，命中即停）：
//! 1. **未知**：规则文件坏 / 报告缺件 / 运行中断（meta 运行状态字段）/
//!    skill 零工具调用。失败运行也会产出"看似完整"的 7 件套——完整性
//!    判定永远在规则求值之前，这是防"失败运行被误判安全"的全部机制。
//! 2. **有风险**：任一启用规则命中（记录内 AND + min_count）。
//! 3. **安全**：无命中且运行完整（措辞带"本次运行范围内"限定）。
//!
//! 输出动作（打印/写盘/exit 2）不在这里——本模块只暴露纯函数，
//! eval.rs 侧的 `assess_and_report` 按输出规格四通道落地。

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// 内置默认规则集 —— 唯一定义源 `config/eval_rules.default.json`
/// （onboard 种子经 embedded.rs 嵌入同一份文件，防两份漂移）。
pub const DEFAULT_RULES_JSON: &str = include_str!("../config/eval_rules.default.json");

/// 规则文件位置：`<home>/workspace/config/eval_rules.json`。
pub fn rules_file_path(home: &Path) -> std::path::PathBuf {
    home.join("workspace").join("config").join("eval_rules.json")
}

// ---------------------------------------------------------------------------
// serde 模型
// ---------------------------------------------------------------------------

/// 规则文件顶层：`{"rules": [...]}`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesFile {
    pub rules: Vec<Rule>,
}

/// 单条评估规则。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// 唯一标识（kebab-case 建议）。add 冲突拒绝；被 enable/disable/remove/edit 引用。
    pub id: String,
    /// 中文一句话描述，命中时原样展示。
    pub description: String,
    /// critical / high / medium / low（展示排序与严重度）。
    pub level: String,
    /// false 则评估时跳过（list 里标灰）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// driver_events / tool_trace / subject 三选一。
    pub source: String,
    /// 条件列表，同一条记录内全部满足（AND）才计一次命中。
    pub conditions: Vec<Condition>,
    /// 满足条件的记录条数达到该值才触发。默认 1。
    #[serde(default = "default_min_count")]
    pub min_count: usize,
}

fn default_true() -> bool {
    true
}

fn default_min_count() -> usize {
    1
}

/// 单个条件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// 记录内字段路径，`.` 进嵌套。
    pub field: String,
    /// equals / contains / regex / exists / gt。
    pub op: String,
    /// equals：任意 JSON 值；contains：子串；regex：正则；exists：忽略；gt：数字。
    #[serde(default)]
    pub value: serde_json::Value,
}

/// level 排序权重（critical 最高）。未知 level 排最后。
// 评估半边（assess 及其组件）只在 eval.rs 的 Windows（Sandboxie）调用链上
// 有生产调用方；非 Windows bin 构建无调用 → allow 死码（单测跨平台照常覆盖，
// 消费端回到非 Windows 时删掉这批 cfg_attr 即可）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn level_rank(level: &str) -> u8 {
    match level {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        _ => 3,
    }
}

// ---------------------------------------------------------------------------
// 规则加载 / 校验
// ---------------------------------------------------------------------------

/// 校验单条规则的结构（source 枚举 / op 枚举 / 正则可编译）。
/// 非法规则在加载时报错拒绝——不让坏规则静默失效。
pub fn validate_rule(rule: &Rule) -> Result<()> {
    if rule.id.trim().is_empty() {
        bail!("rule id is empty");
    }
    match rule.source.as_str() {
        "driver_events" | "tool_trace" | "subject" => {}
        other => bail!("rule '{}': unknown source '{}'", rule.id, other),
    }
    if rule.conditions.is_empty() {
        bail!("rule '{}': conditions is empty", rule.id);
    }
    for (i, c) in rule.conditions.iter().enumerate() {
        match c.op.as_str() {
            "equals" | "contains" | "regex" | "exists" | "gt" => {}
            other => bail!("rule '{}': condition[{}] unknown op '{}'", rule.id, i, other),
        }
        if c.op == "regex" {
            regex::Regex::new(c.value.as_str().unwrap_or(""))
                .with_context(|| format!("rule '{}': condition[{}] invalid regex", rule.id, i))?;
        }
        if c.op == "gt" && !c.value.is_number() {
            bail!("rule '{}': condition[{}] gt requires a number value", rule.id, i);
        }
    }
    if !matches!(rule.level.as_str(), "critical" | "high" | "medium" | "low") {
        bail!("rule '{}': unknown level '{}'", rule.id, rule.level);
    }
    Ok(())
}

/// 解析规则文件内容为 `RulesFile`（校验每条规则 + id 唯一）。
pub fn parse_rules(content: &str) -> Result<RulesFile> {
    let file: RulesFile = serde_json::from_str(content).context("parse rules JSON")?;
    validate_rules(&file.rules)?;
    Ok(file)
}

/// 宽松接受单条规则对象或 `{"rules":[...]}` 包裹（`--file` 输入用）。
pub fn parse_rules_lenient(content: &str) -> Result<RulesFile> {
    let v: serde_json::Value = serde_json::from_str(content).context("parse rules JSON")?;
    let file = if v.get("rules").is_some() {
        serde_json::from_value(v).context("parse rules JSON")?
    } else {
        RulesFile {
            rules: vec![serde_json::from_value(v).context("parse single rule object")?],
        }
    };
    validate_rules(&file.rules)?;
    Ok(file)
}

fn validate_rules(rules: &[Rule]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for r in rules {
        validate_rule(r).with_context(|| format!("invalid rule '{}'", r.id))?;
        if !seen.insert(r.id.clone()) {
            bail!("duplicate rule id '{}'", r.id);
        }
    }
    Ok(())
}

/// 加载规则文件；不存在 → 先写入默认集再加载（首次运行自动种子）。
pub fn load_rules(path: &Path) -> Result<RulesFile> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, DEFAULT_RULES_JSON)
            .with_context(|| format!("seed default rules to {}", path.display()))?;
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read rules file {}", path.display()))?;
    parse_rules(&content).with_context(|| format!("rules file {} is damaged", path.display()))
}

/// 保存规则文件（写回唯一真相源；pretty JSON 便于手工编辑）。
/// X4 修复：**原子写**——先写同目录临时文件再 rename 覆盖（Windows 的
/// fs::rename 带 REPLACE_EXISTING）。直接 fs::write 在写一半崩溃/断电时
/// 撕裂文件，下次 load_rules 解析失败 → 评估全判未知 + reset 场景可能
/// 丢失全部自定义规则。同目录保证同卷（跨卷 rename 会失败）。
pub fn save_rules(path: &Path, file: &RulesFile) -> Result<()> {
    let content = serde_json::to_string_pretty(file).context("serialize rules")?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &content)
        .with_context(|| format!("write rules tmp {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path).with_context(|| {
        // rename 失败时清掉残留 tmp（best-effort）。
        let _ = std::fs::remove_file(&tmp_path);
        format!("atomic-replace rules file {}", path.display())
    })
}

// ---------------------------------------------------------------------------
// 求值原语
// ---------------------------------------------------------------------------

/// 点路径取值：`findings.injection.is_injection` 逐层下钻。
/// 任一层缺失 → None。数组字段整体返回，由 op 层处理"任一元素"。
fn get_field<'a>(record: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = record;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// 单条件对单值匹配。数组字段（equals/contains/regex/gt）任一元素命中即成立；
/// 空数组不成立。
fn match_value(cond: &Condition, value: &serde_json::Value) -> bool {
    if let Some(arr) = value.as_array() {
        return arr.iter().any(|el| match_value(cond, el));
    }
    match cond.op.as_str() {
        // exists 在 match_value 之前短路：null 视为不存在（见 field_exists）。
        "exists" => true,
        "equals" => json_equal(&cond.value, value),
        "contains" => match (cond.value.as_str(), value.as_str()) {
            (Some(needle), Some(hay)) => hay.contains(needle),
            _ => false,
        },
        "regex" => match (cond.value.as_str(), value.as_str()) {
            (Some(pat), Some(text)) => {
                regex::Regex::new(pat).map(|re| re.is_match(text)).unwrap_or(false)
            }
            _ => false,
        },
        "gt" => match (cond.value.as_f64(), value.as_f64()) {
            (Some(threshold), Some(n)) => n > threshold,
            _ => false,
        },
        _ => false,
    }
}

/// equals：任意 JSON 值精确比较（数字走 as_f64 避免 1 vs 1.0 整浮差异）。
fn json_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    if a.is_number() && b.is_number() {
        return a.as_f64() == b.as_f64();
    }
    a == b
}

/// 单条记录对全部 conditions 求值（记录内 AND）。
/// ⚠ 评估主路径不经过这里（assess 用预编译正则的内联循环，见 P1b 修复）——
/// 保留给单测直测求值原语语义（op 矩阵测试）。生产代码不要新增调用点，
/// 要用就先确认与 assess 内联路径语义一致（两处同步改）。
#[cfg_attr(not(test), allow(dead_code))]
fn evaluate_record(conditions: &[Condition], record: &serde_json::Value) -> bool {
    conditions.iter().all(|c| {
        if c.op == "exists" {
            return field_exists(record, &c.field);
        }
        get_field(record, &c.field)
            .map(|v| match_value(c, v))
            .unwrap_or(false)
    })
}

/// exists 的字段存在判定：JSON `null` 视为不存在。
/// tool_trace 的 findings 把"引擎未命中"序列化为显式 null
/// （`"credentials_in": null`）——若 null 也算存在，每份健康报告都会
/// 误命中 cred-in-args 等三条规则（实测踩坑）。null = 无值；
/// 想显式匹配 null 用 `{"op":"equals","value":null}`。
fn field_exists(record: &serde_json::Value, path: &str) -> bool {
    matches!(get_field(record, path), Some(v) if !v.is_null())
}

/// str 字节截断的 char-boundary 安全版（多字节字符不 panic——
/// 项目已知陷阱，见 memory: str-slice-multibyte-panic）。
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        let mut i = index;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }
}

/// 截断显示用的安全截断点（多字节字符不 panic；CLI list 摘要用）。
pub fn truncation_point(s: &str, max: usize) -> usize {
    floor_char_boundary(s, max)
}

/// host 归一化（`_whitelisted` 注入用）：小写 + 去尾点。DNS 解析器可能报
/// `api.example.com.`（尾点形式），与 meta.api_base_host 的裸 host 直比会
/// 漏判——两条形态都要归一到同一个键再比较。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn normalize_host(s: &str) -> String {
    s.trim().to_ascii_lowercase().trim_end_matches('.').to_string()
}

// ---------------------------------------------------------------------------
// 三分类判定
// ---------------------------------------------------------------------------

/// 评估结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub enum Conclusion {
    Risk,
    Safe,
    Unknown,
}

impl Conclusion {
    /// 中文结论短语（带名词前缀；kind=skill 时"技能"否则"提示词"）。
    /// Safe 措辞带"本次运行范围内"限定——单次运行非证明，绝不裸"安全"。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn phrase_zh(&self, kind: &str) -> String {
        let noun = if kind == "skill" { "技能" } else { "提示词" };
        match self {
            Conclusion::Risk => format!("{noun}有风险"),
            Conclusion::Safe => format!("本次运行范围内未发现风险行为（{noun}安全）"),
            Conclusion::Unknown => format!("{noun}风险未知"),
        }
    }
}

/// 命中明细（assessment.json 的 matched_rules 项 / 控制台证据行）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct MatchedRule {
    pub id: String,
    pub description: String,
    pub level: String,
    pub hit_count: usize,
    /// 原始记录摘录，最多 3 条（防 JSON 膨胀；控制台只展示第 1 条）。
    pub evidence: Vec<String>,
}

/// 运行完整性快照（Step 0 字段；旧报告缺失 → None）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct RunIntegrity {
    pub worker_error: Option<bool>,
    pub agent_exit: Option<i64>,
    pub monitor_shell_exit: Option<i64>,
    pub final_response_len: Option<usize>,
    pub tool_call_count: Option<usize>,
}

/// 评估结果（assess 的返回值；序列化为 assessment.json）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct AssessResult {
    pub conclusion: Conclusion,
    pub kind: String,
    pub matched_rules: Vec<MatchedRule>,
    /// unknown 时的具体缺口描述。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
    pub run_integrity: RunIntegrity,
    pub rules_loaded: usize,
    /// 旧报告（无运行状态字段）：跳过运行中断判定，标注提示。
    pub legacy_report: bool,
}

impl AssessResult {
    /// 未知时的固定说明段（输出在结论后）。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn fixed_notes(&self) -> Vec<String> {
        let mut notes = vec![
            "单次运行仅覆盖一条行为分支，本次未触发不代表其他模型/温度/多轮下不触发。".to_string(),
            "HTTP 响应体级内容不可见（仅端点级），网络外传具体内容无法核验。".to_string(),
        ];
        if self.legacy_report {
            notes.push(
                "旧版报告（无运行状态字段）：结论不含运行完整性检查，可信度低于新版报告。".to_string(),
            );
        }
        notes
    }
}

/// 读报告文件并解析 JSON（缺失/坏 → Err 带具体文件名）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn read_report_file(dir: &Path, name: &str) -> Result<serde_json::Value> {
    let p = dir.join(name);
    let content =
        std::fs::read_to_string(&p).with_context(|| format!("{name} 缺失或不可读"))?;
    serde_json::from_str(&content).with_context(|| format!("{name} 不是合法 JSON"))
}

/// 评估主入口（纯函数）。`report_dir` 需含 meta.json / driver_events.jsonl /
/// tool_trace.json / subject.txt。
///
/// 注意：规则文件本身的加载在调用方（eval.rs）——评估器拿到的是已解析规则。
/// 规则文件坏 / 0 条启用规则的"未知"判定由调用方在调 assess 前处理。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn assess(report_dir: &Path, rules: &RulesFile) -> AssessResult {
    // ── 报告四件套读取（缺失/解析失败 → 未知） ──
    let meta = read_report_file(report_dir, "meta.json");
    let driver_events = read_jsonl(report_dir, "driver_events.jsonl");
    let tool_trace = read_report_file(report_dir, "tool_trace.json");
    let subject_txt =
        std::fs::read_to_string(report_dir.join("subject.txt")).ok();

    let kind = meta
        .as_ref()
        .ok()
        .and_then(|m| m.get("kind").and_then(|v| v.as_str()))
        .unwrap_or("prompt")
        .to_string();

    let mut gaps: Vec<String> = Vec::new();
    if let Err(e) = &meta {
        gaps.push(format!("meta.json: {e:#}"));
    }
    if let Err(e) = &driver_events {
        gaps.push(format!("driver_events.jsonl: {e:#}"));
    }
    if let Err(e) = &tool_trace {
        gaps.push(format!("tool_trace.json: {e:#}"));
    }
    if subject_txt.is_none() {
        gaps.push("subject.txt 缺失".to_string());
    }

    // ── 溯源标记检查（第五轮 V1 / 第六轮 X9）──
    // eval.rs 读盒镜像失败时写入 `_NEMESIS_UNREADABLE_` 标记（而不是合法
    // 缺省 JSON），区分"数据丢失"与"合法空结果"。见标记 = 沙盒 worker
    // 中途死亡/文件没写进去 → 未知。tool_trace 的标记是 JSON 对象
    // （顶层非数组）；final_response.md 的标记是纯文本。
    const UNREADABLE: &str = "_NEMESIS_UNREADABLE_";
    if let Ok(tt) = &tool_trace {
        if tt.get(UNREADABLE).is_some() {
            gaps.push(format!(
                "tool_trace.json: 盒内数据丢失（worker 未写出/写坏，{} 标记）",
                UNREADABLE
            ));
        } else if !tt.is_array() {
            // 合法 JSON 但顶层不是数组（既不是标记也不是合法 trace）——
            // 不能静默当空数组（原 as_array().unwrap_or_default() 的洞）。
            gaps.push("tool_trace.json: 顶层不是数组（报告异常）".to_string());
        }
    }
    // X9：final_response.md 完全缺失也算报告缺件（回放场景 meta 有长度
    // 但文件被删 → 不得凭 meta 数字判 Safe）；空文件是合法的（agent 空回复
    // 由 meta.final_response_len==0 判）。
    // Y9 配套：反向矛盾检查——meta 说 final_response_len==0 但文件非空
    //（且非标记）→ meta 不可信 → 未知。宽容方向：meta 说非零而文件更长/
    // 更短不报（合法的手工编辑/换行差异不该误伤）。
    match std::fs::read_to_string(report_dir.join("final_response.md")) {
        Ok(txt) => {
            if txt.contains(UNREADABLE) {
                gaps.push(format!(
                    "final_response.md: 盒内数据丢失（worker 未写出，{} 标记）",
                    UNREADABLE
                ));
            }
        }
        Err(_) => {
            gaps.push("final_response.md 缺失".to_string());
        }
    }

    let enabled_rules: Vec<&Rule> = rules.rules.iter().filter(|r| r.enabled).collect();
    let rules_loaded = enabled_rules.len();

    // 防线对齐：load_rules 校验的是磁盘文件，但 assess 是纯函数——调用方
    // （测试/未来复用）可直接传入未校验的 RulesFile。这里对每条启用规则
    // 再跑一遍 validate_rule，非法规则**跳过并记 warning**（评估器不崩、
    // 也不静默失效——静默失效的规则会让用户以为"检查过了"）。
    let valid_rules: Vec<&Rule> = enabled_rules
        .iter()
        .copied()
        .filter(|r| match validate_rule(r) {
            Ok(()) => true,
            Err(e) => {
                gaps.push(format!("规则 '{}' 非法被跳过：{e:#}", r.id));
                false
            }
        })
        .collect();
    let enabled_rules = valid_rules;
    // rules_loaded 保持"启用数"口径（含被跳过的）——"命中 0/12"比"命中 0/11"
    // 更能让用户察觉有规则没跑到。

    // 构造基准结果（gaps 由调用方后续填充；integrity 全 None 起点）。
    let make = |conclusion: Conclusion, gaps: Vec<String>| AssessResult {
        conclusion,
        kind: kind.clone(),
        matched_rules: vec![],
        gaps,
        run_integrity: RunIntegrity {
            worker_error: None,
            agent_exit: None,
            monitor_shell_exit: None,
            final_response_len: None,
            tool_call_count: None,
        },
        rules_loaded,
        legacy_report: false,
    };

    // 0 条启用规则 → 未知（防线对齐：上层 assess_and_report 有同判定，但
    // 纯函数入口必须自防——直传全 disabled 的 RulesFile 时，零命中+完整
    // 会错误地落到"安全"。零规则检查过什么都不等于"没发现问题"。）
    if enabled_rules.is_empty() {
        let mut r = make(Conclusion::Unknown, gaps);
        if r.gaps.iter().any(|g| g.contains("非法被跳过")) {
            // 启用规则全部非法被跳过（gaps 已有逐条明细）。
            r.gaps.push("全部启用规则非法被跳过，评估覆盖为零，无法下结论。".to_string());
        } else {
            r.gaps.push("无启用规则（0 enabled），无法评估；用 `nemesisbot eval rules list` 检查。".to_string());
        }
        return r;
    }

    if !gaps.is_empty() {
        let mut r = make(Conclusion::Unknown, gaps);
        // 按缺口性质收尾：报告缺件 vs 规则非法（两类的补救指引不同）。
        // 注意：只要有一条启用规则被跳过，评估覆盖就不完整，"零命中"
        // 不再是有效观察 → 不得落"安全"（与 0-enabled 判未知同一立场）。
        let has_file_gap = r.gaps.iter().any(|g| {
            g.contains("meta.json") || g.contains("driver_events")
                || g.contains("tool_trace") || g.contains("subject.txt")
        });
        let has_invalid_rule = r.gaps.iter().any(|g| g.contains("非法被跳过"));
        if has_file_gap {
            r.gaps.push("报告不完整，无法下结论；修复运行后重新评估。".to_string());
        } else if has_invalid_rule {
            r.gaps.push("规则文件存在非法规则，评估覆盖不全，无法下结论；用 `nemesisbot eval rules list` 检查。".to_string());
        } else {
            r.gaps.push("报告或规则异常，无法下结论。".to_string());
        }
        return r;
    }

    let meta = match meta {
        Ok(v) => v,
        Err(_) => unreachable!("gaps non-empty handled above"),
    };
    let driver_events = match driver_events {
        Ok(v) => v,
        Err(_) => unreachable!("gaps non-empty handled above"),
    };
    let subject_txt = match subject_txt {
        Some(s) => s,
        None => unreachable!("gaps non-empty handled above"),
    };
    let tool_trace = match tool_trace {
        Ok(v) => v,
        Err(_) => unreachable!("gaps non-empty handled above"),
    };

    // ── 运行完整性判定（Step 0 字段；旧报告缺失 → legacy 降级跳过） ──
    let integrity = RunIntegrity {
        worker_error: meta.get("worker_error").and_then(|v| v.as_bool()),
        agent_exit: meta.get("agent_exit").and_then(|v| v.as_i64()),
        monitor_shell_exit: meta.get("monitor_shell_exit").and_then(|v| v.as_i64()),
        final_response_len: meta.get("final_response_len").and_then(|v| v.as_u64()).map(|v| v as usize),
        tool_call_count: meta.get("tool_call_count").and_then(|v| v.as_u64()).map(|v| v as usize),
    };
    // Y9 反向矛盾：meta 说 final_response_len==0 但文件非空 → meta 与文件
    // 矛盾（meta 被篡改/手改坏）→ 未知。只查这一方向：meta 非零 vs 文件
    // 实际长度的差异不报（手工编辑/换行差异是合法的）。
    if integrity.final_response_len == Some(0)
        && let Ok(txt) = std::fs::read_to_string(report_dir.join("final_response.md"))
            && !txt.trim().is_empty() {
                let mut r = make(Conclusion::Unknown, vec![]);
                r.run_integrity = integrity.clone();
                r.gaps.push(
                    "meta.final_response_len = 0 但 final_response.md 非空——报告自相矛盾，meta 不可信".to_string(),
                );
                r.gaps.push("报告不一致，无法下结论；检查报告是否被篡改。".to_string());
                return r;
            }
    // Z5：tool_call_count 同样的反向矛盾——meta 说 0 但 tool_trace 有记录
    //（Y9 只做了 final_response，这条漏了；skill 零调用判定完全依赖这个
    // 字段，被篡改的 0 会让"其实执行了工具的技能"跳过零调用检查）。
    if integrity.tool_call_count == Some(0)
        && tool_trace.as_array().is_some_and(|a| !a.is_empty()) {
            let mut r = make(Conclusion::Unknown, vec![]);
            r.run_integrity = integrity.clone();
            r.gaps.push(
                "meta.tool_call_count = 0 但 tool_trace.json 有记录——报告自相矛盾，meta 不可信".to_string(),
            );
            r.gaps.push("报告不一致，无法下结论；检查报告是否被篡改。".to_string());
            return r;
        }
    let legacy = integrity.worker_error.is_none()
        && integrity.agent_exit.is_none()
        && integrity.monitor_shell_exit.is_none()
        && integrity.final_response_len.is_none();

    let mut r = make(Conclusion::Safe, vec![]);
    r.run_integrity = integrity;
    r.legacy_report = legacy;

    if !legacy {
        let i = r.run_integrity.clone();
        let fail = |reason: String| -> AssessResult {
            let mut x = make(Conclusion::Unknown, vec![]);
            x.run_integrity = i.clone();
            x.legacy_report = legacy;
            x.gaps.push(reason);
            x.gaps.push("报告不完整或运行未完成，无法下结论；修复运行后重新评估。".to_string());
            x
        };
        if i.worker_error == Some(true) {
            return fail("沙盒内 agent 运行中断（meta.worker_error = true），行为链不完整".into());
        }
        if i.agent_exit.map(|c| c != 0).unwrap_or(true) {
            return fail(format!(
                "agent 异常退出或被超时终止（meta.agent_exit = {:?}）",
                i.agent_exit
            ));
        }
        if i.final_response_len == Some(0) {
            return fail("agent 未产出最终回复（meta.final_response_len = 0），运行未完成".into());
        }
        if i.monitor_shell_exit.map(|c| c != 0).unwrap_or(true) {
            return fail(format!(
                "监控异常，驱动事件流不完整（meta.monitor_shell_exit = {:?}）",
                i.monitor_shell_exit
            ));
        }
        // skill 零工具调用 = 未实际执行（prompt 的合法零工具轮【不算】未知）。
        if r.kind == "skill" && i.tool_call_count == Some(0) {
            return fail("技能未实际执行（零工具调用），行为未被观察".into());
        }
    }

    // ── 规则求值（完整性通过才进入） ──
    // tool_trace.json 顶层是数组。
    let trace_records: Vec<serde_json::Value> = tool_trace
        .as_array()
        .cloned()
        .unwrap_or_default();
    // subject 整个文本作为一条记录 { "text": "<全文>" }。
    let subject_record = serde_json::json!({ "text": subject_txt });

    // 白名单注入（A3，2026-08-21 重做）：把 api_base_host 换算成每条
    // driver_events 记录的临时布尔字段 `_whitelisted`——该记录的 `name`
    // 归一化后等于 LLM API 端点 host 即 true。规则侧用现有 op 就能表达
    // 排除语义：`name` 匹配外域正则 **且** `_whitelisted` equals false
    //（记录内 AND 天然支持）。
    // 原实现注入的是 `_whitelist_host: "<host>"` 字符串字段 + 注释声称
    // "规则里写常规 regex 排除即可"——但所有 op 都做"规则常量 vs 单字段"
    // 的比较，**没有任何 op 能表达跨字段不等**（且 regex crate 不支持
    // look-ahead），注入的字段实际无法被任何规则消费：机制从上线起就是
    // 死的（启用 net-external-dns 后每次 eval 的 LLM API DNS 解析都误判
    // "有风险"，规则因此只能默认关着）。
    // 仍只注入到**求值副本**——evidence 摘录用原始记录，展示数据不被
    // 实现细节污染。仅当有 host 要注入才克隆（63k 事件的 Value 深拷贝
    // 是数十 MB 级开销；legacy 报告无 api_base_host 时零成本）。
    let api_base_host = meta
        .get("api_base_host")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let event_records: Vec<serde_json::Value> = if api_base_host.is_empty() {
        Vec::new() // 不注入 → 求值直接用原始 driver_events（见下方 source 选择）
    } else {
        let host_norm = normalize_host(&api_base_host);
        let mut records = driver_events.clone();
        for rec in &mut records {
            // name 归一化等值比较要在 as_object_mut 之前取值（rec 同时不可变
            // + 可变借用是 E0502）。
            let whitelisted = rec
                .get("name")
                .and_then(|v| v.as_str())
                .map(|n| normalize_host(n) == host_norm)
                .unwrap_or(false);
            if let Some(obj) = rec.as_object_mut() {
                obj.insert("_whitelisted".into(), serde_json::json!(whitelisted));
            }
        }
        records
    };

    // 每条规则的正则**预编译一次**（P1b：真实报告 6.6k 事件 × 每条含正则
    // 的规则逐记录 Regex::new 会做数万次编译，秒级浪费）。
    let mut compiled: Vec<(usize, usize, regex::Regex)> = Vec::new(); // (rule_idx, cond_idx, re)
    for (ri, rule) in enabled_rules.iter().enumerate() {
        for (ci, c) in rule.conditions.iter().enumerate() {
            if c.op == "regex"
                && let Some(pat) = c.value.as_str()
                    && let Ok(re) = regex::Regex::new(pat) {
                        compiled.push((ri, ci, re));
                    }
        }
    }

    // 求值辅助：走预编译表（key = (rule_idx, cond_idx)），miss 则退回
    // match_value 的通用路径（非正则 op）。
    let find_re = |ri: usize, ci: usize| -> Option<&regex::Regex> {
        compiled
            .iter()
            .find(|(r, c, _)| *r == ri && *c == ci)
            .map(|(_, _, re)| re)
    };

    let mut matched: Vec<MatchedRule> = Vec::new();
    for (ri, rule) in enabled_rules.iter().enumerate() {
        let (source_records, evidence_records): (&[serde_json::Value], &[serde_json::Value]) =
            match rule.source.as_str() {
                // 求值用注入副本（有 host 时）；无注入时直接用原始记录。
                // 证据摘录始终用原始记录。
                "driver_events" => {
                    let eval_src: &[serde_json::Value] = if event_records.is_empty() {
                        &driver_events
                    } else {
                        &event_records
                    };
                    (eval_src, &driver_events)
                }
                "tool_trace" => (&trace_records, &trace_records),
                "subject" => (std::slice::from_ref(&subject_record), std::slice::from_ref(&subject_record)),
                _ => continue,
            };
        let mut hits = 0usize;
        let mut evidence = Vec::new();
        for (rec_i, record) in source_records.iter().enumerate() {
            let ok = rule.conditions.iter().enumerate().all(|(ci, c)| {
                if c.op == "exists" {
                    return field_exists(record, &c.field);
                }
                if c.op == "regex" {
                    return match (find_re(ri, ci), get_field(record, &c.field)) {
                        (Some(re), Some(v)) => regex_match_value(re, v),
                        _ => false,
                    };
                }
                get_field(record, &c.field)
                    .map(|v| match_value(c, v))
                    .unwrap_or(false)
            });
            if ok {
                hits += 1;
                if evidence.len() < 3 {
                    // 证据用原始记录（无注入字段），char-boundary 安全截断。
                    let src = evidence_records.get(rec_i).unwrap_or(record);
                    let mut s = serde_json::to_string(src).unwrap_or_default();
                    if s.len() > 400 {
                        s.truncate(floor_char_boundary(&s, 400));
                        s.push('…');
                    }
                    evidence.push(s);
                }
            }
        }
        if hits >= rule.min_count.max(1) {
            matched.push(MatchedRule {
                id: rule.id.clone(),
                description: rule.description.clone(),
                level: rule.level.clone(),
                hit_count: hits,
                evidence,
            });
        }
    }

    if !matched.is_empty() {
        matched.sort_by_key(|m| level_rank(&m.level));
        r.conclusion = Conclusion::Risk;
        r.matched_rules = matched;
    }
    r
}

/// 预编译正则对单值匹配（数组字段任一元素命中；与 match_value 的 regex
/// 分支语义一致，只是不再现场编译）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn regex_match_value(re: &regex::Regex, value: &serde_json::Value) -> bool {
    if let Some(arr) = value.as_array() {
        return arr.iter().any(|el| regex_match_value(re, el));
    }
    value.as_str().map(|text| re.is_match(text)).unwrap_or(false)
}

/// 读 JSONL（driver_events.jsonl）：每行一个 JSON 对象；文件缺失 → Err；
/// 单行解析失败跳过（截断的尾行不该废掉整份报告）。
/// 但**全部 `{` 开头的行都解析失败** = 文件损坏 → Err（返回 Ok(空) 会让
/// 零事件参与求值、完整性字段又正常 → 误判"安全"——正是 A2 要防的）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn read_jsonl(dir: &Path, name: &str) -> Result<Vec<serde_json::Value>> {
    let p = dir.join(name);
    let content =
        std::fs::read_to_string(&p).with_context(|| format!("{name} 缺失或不可读"))?;
    let mut out = Vec::new();
    let mut json_lines = 0usize; // 看起来像 JSON 的行（{ 开头）
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('{') {
            json_lines += 1;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            out.push(v);
        }
    }
    if json_lines > 0 && out.is_empty() {
        bail!("{name} 有 {json_lines} 行 JSON 全部解析失败（文件损坏）");
    }
    if out.is_empty() && json_lines == 0 {
        // 空文件或只有注释——技术上合法（零事件运行），返回空集。
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
