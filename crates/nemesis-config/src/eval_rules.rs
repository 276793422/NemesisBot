//! eval 评估规则——规则驱动三分类 assessor 的**数据半边**（单一真相源）。
//!
//! 本模块收编：serde 模型（[`RulesFile`]/[`Rule`]/[`Condition`]）、结构校验
//! （[`validate_rule`]/[`validate_rules`]）、解析（[`parse_rules`]/
//! [`parse_rules_lenient`]）、文件 IO（[`load_rules`]/[`save_rules`]：首次
//! 自动种子 + 原子写 + 覆写前 `.json.bak` 备份）与内置默认集
//! [`DEFAULT_RULES_JSON`]（编译期内嵌，唯一定义源 `eval_rules_default.json`）。
//!
//! 求值原语（[`get_field`]/[`match_value`]/[`json_equal`]/[`field_exists`]/
//! [`scan_record`]+[`ScanHit`]）随批三收编本模块——单记录规则扫描（#14
//! MCP 装前扫描）与全报告评估（`nemesisbot::eval_assessor`）共用同一套
//! op 语义，杜绝两处实现漂移。assess 的预编译正则主路径仍在
//! `nemesisbot::eval_assessor`（后者 `use` 本模块原语 + `pub use` 数据
//! 模型，CLI（commands/eval_rules.rs）与评估链（commands/eval.rs）调用
//! 点零迁移）。WSAPI `security.eval_rules.*`（Dashboard 评估规则管理
//! TAB）直接消费本模块——handler 侧不存在第二份规则结构真相。
//!
//! 文件位置 `<home>/workspace/config/eval_rules.json`：路径归 nemesisbot
//! （workspace 布局是宿主的关注点），本模块只认调用方给的绝对路径。

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// 内置默认规则集 —— 唯一定义源 `eval_rules_default.json`（原
/// `nemesisbot/config/eval_rules.default.json`，随模型收编本 crate）。
pub const DEFAULT_RULES_JSON: &str = include_str!("eval_rules_default.json");

// ---------------------------------------------------------------------------
// serde 模型
// ---------------------------------------------------------------------------

/// 规则文件当前版本（漂移检测基准；字段演进时递增，旧 version 文件
/// 照常载入，高 version 文件拒载——降级读入会产生运行期字段缺失）。
pub const CURRENT_RULES_VERSION: u32 = 1;

fn default_rules_version() -> u32 {
    CURRENT_RULES_VERSION
}

/// 规则文件顶层：`{"version": 1, "rules": [...]}`（version 缺省 = 1，
/// 旧形态文件零迁移）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesFile {
    #[serde(default = "default_rules_version")]
    pub version: u32,
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
            other => bail!(
                "rule '{}': condition[{}] unknown op '{}'",
                rule.id,
                i,
                other
            ),
        }
        // regex/contains 的 value 必须是字符串：regex
        // 此前用 as_str().unwrap_or("") 按空模式校验——非字符串 value 永远
        // 过闸，求值侧却恒 false（规则静默失效，fail-open）。加载期 loud
        // 拒绝才与「不让坏规则静默失效」的立场一致。
        if c.op == "regex" {
            let Some(pat) = c.value.as_str() else {
                bail!(
                    "rule '{}': condition[{}] regex requires a string value",
                    rule.id,
                    i
                );
            };
            regex::Regex::new(pat)
                .with_context(|| format!("rule '{}': condition[{}] invalid regex", rule.id, i))?;
        }
        if c.op == "contains" && c.value.as_str().is_none() {
            bail!(
                "rule '{}': condition[{}] contains requires a string value",
                rule.id,
                i
            );
        }
        if c.op == "gt" && !c.value.is_number() {
            bail!(
                "rule '{}': condition[{}] gt requires a number value",
                rule.id,
                i
            );
        }
    }
    if !matches!(rule.level.as_str(), "critical" | "high" | "medium" | "low") {
        bail!("rule '{}': unknown level '{}'", rule.id, rule.level);
    }
    Ok(())
}

/// 解析规则文件内容为 `RulesFile`（校验每条规则 + id 唯一 + 版本漂移）。
pub fn parse_rules(content: &str) -> Result<RulesFile> {
    let file: RulesFile = serde_json::from_str(content).context("parse rules JSON")?;
    // 漂移检测：文件 version 高于程序支持 = 来自更新版本的程序，拒载
    // （与 sqlite_preflight::check_schema_drift 同语义：降级打开会产生
    // 运行期字段缺失，宁 loud 拒不静默）。
    if file.version > CURRENT_RULES_VERSION {
        bail!(
            "rules file version {} is newer than supported {} \
             (written by a newer build; upgrade this program before loading)",
            file.version,
            CURRENT_RULES_VERSION
        );
    }
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
            version: CURRENT_RULES_VERSION,
            rules: vec![serde_json::from_value(v).context("parse single rule object")?],
        }
    };
    validate_rules(&file.rules)?;
    Ok(file)
}

/// 校验规则集：逐条结构校验 + id 全局唯一。
pub fn validate_rules(rules: &[Rule]) -> Result<()> {
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
        // 统一原子写入（首次运行自动种子与 save_rules 同源）。
        nemesis_utils::write_file_atomic(
            &path.to_string_lossy(),
            DEFAULT_RULES_JSON.as_bytes(),
            0o600,
        )
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("seed default rules to {}", path.display()))?;
    }
    // BUG-20261109-036：用户可手工编辑（pretty JSON「便于手工编辑」是
    // 本文件自己的注释）——读侧走 BOM 剥离 helper，Windows 记事本
    // utf8BOM 保存形态不再误报「rules file is damaged」。
    let content = nemesis_utils::fsutil::read_config_string(path)
        .with_context(|| format!("read rules file {}", path.display()))?;
    parse_rules(&content).with_context(|| format!("rules file {} is damaged", path.display()))
}

/// 保存规则文件（写回唯一依据；pretty JSON 便于手工编辑）。
/// 起要求原子替换； 起统一走 `write_file_atomic`（唯一临时名
/// `.tmp-{pid}-{nanos}` + sync_all + 失败清理，替代此前的自制固定名
/// `.json.tmp` + rename——旧形态无 sync、并发互踩、失败清理只覆盖 rename 臂）。
pub fn save_rules(path: &Path, file: &RulesFile) -> Result<()> {
    // 覆写前备份：现存旧文件轮换为 `.json.bak`（单份，本次成功覆盖旧
    // bak）。用户可编辑资产（add/edit/reset 都走本单一写点）被直接覆盖前
    // 至少留一份可回。备份是兜底不是闸门——复制失败 warn 不阻断保存。
    if path.exists() {
        let bak = path.with_extension("json.bak");
        if let Err(e) = std::fs::copy(path, &bak) {
            tracing::warn!(
                error = %e,
                path = %bak.display(),
                "eval_rules 覆写前备份失败（继续保存，不阻断）"
            );
        }
    }
    let content = serde_json::to_string_pretty(file).context("serialize rules")?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(anyhow::Error::msg)
}

// ---------------------------------------------------------------------------
// 求值原语（批三收编：单记录扫描与全报告评估共用）
// ---------------------------------------------------------------------------

/// 点路径取值：`findings.injection.is_injection` 逐层下钻。
/// 任一层缺失 → None。数组字段整体返回，由 op 层处理"任一元素"。
pub fn get_field<'a>(record: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = record;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// 单条件对单值匹配。数组字段（equals/contains/regex/gt）任一元素命中即成立；
/// 空数组不成立。
pub fn match_value(cond: &Condition, value: &serde_json::Value) -> bool {
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
            (Some(pat), Some(text)) => regex::Regex::new(pat)
                .map(|re| re.is_match(text))
                .unwrap_or(false),
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
pub fn json_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    if a.is_number() && b.is_number() {
        return a.as_f64() == b.as_f64();
    }
    a == b
}

/// exists 的字段存在判定：JSON `null` 视为不存在。
/// tool_trace 的 findings 把"引擎未命中"序列化为显式 null
/// （`"credentials_in": null`）——若 null 也算存在，每份健康报告都会
/// 误命中 cred-in-args 等三条规则（实测踩坑）。null = 无值；
/// 想显式匹配 null 用 `{"op":"equals","value":null}`。
pub fn field_exists(record: &serde_json::Value, path: &str) -> bool {
    matches!(get_field(record, path), Some(v) if !v.is_null())
}

/// str 字节截断的 char-boundary 安全版（多字节字符不 panic——
/// 项目已知陷阱，见 memory: str-slice-multibyte-panic）。
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
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

/// 截断显示用的安全截断点（多字节字符不 panic；CLI list 摘要 / 扫描
/// 命中证据共用）。
pub fn truncation_point(s: &str, max: usize) -> usize {
    floor_char_boundary(s, max)
}

/// 单条规则命中（[`scan_record`] 的返回项）。
#[derive(Debug, Clone, Serialize)]
pub struct ScanHit {
    pub rule_id: String,
    pub description: String,
    pub level: String,
    /// 原始记录摘录（400 字符 char-boundary 安全截断）。
    pub evidence: String,
}

/// 单记录规则扫描（批三新增）：`source == "subject"` 且启用的规则逐条
/// 求值（记录内 AND），命中即产出 [`ScanHit`]。min_count 在单记录语境下
/// 按 1 处理（多条计数是 assess 全报告语义，单点扫描面没有"多条"可言）。
///
/// `rules = None`（规则文件缺失/损坏等）→ 空结果：规则面 fail-open，
/// 注入检测等其他信号面照常工作——是否因"无规则"而拒绝由调用方决定
/// （#14 装前扫描的 decide 层持有该裁决）。
///
/// 典型 record 形态：`{"text": <待扫文本>}`——默认 subject 规则
/// （`subject-static-inject`）的字段即 `text`。
pub fn scan_record(record: &serde_json::Value, rules: Option<&RulesFile>) -> Vec<ScanHit> {
    let Some(file) = rules else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for rule in &file.rules {
        if !rule.enabled || rule.source != "subject" {
            continue;
        }
        let matched = rule.conditions.iter().all(|c| {
            if c.op == "exists" {
                return field_exists(record, &c.field);
            }
            get_field(record, &c.field)
                .map(|v| match_value(c, v))
                .unwrap_or(false)
        });
        if matched {
            // 证据摘录优先取 record 的 `text` 字段（subject 扫描的规范
            // 形态，给人读原文）；无 text 字段回落整条记录的序列化。
            let evidence = get_field(record, "text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| serde_json::to_string(record).unwrap_or_default());
            let cut = truncation_point(&evidence, 400);
            hits.push(ScanHit {
                rule_id: rule.id.clone(),
                description: rule.description.clone(),
                level: rule.level.clone(),
                evidence: evidence[..cut].to_string(),
            });
        }
    }
    hits
}
