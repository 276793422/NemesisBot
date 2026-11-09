//! eval 评估规则——规则驱动三分类 assessor 的**数据半边**（单一真相源）。
//!
//! 本模块收编：serde 模型（[`RulesFile`]/[`Rule`]/[`Condition`]）、结构校验
//! （[`validate_rule`]/[`validate_rules`]）、解析（[`parse_rules`]/
//! [`parse_rules_lenient`]）、文件 IO（[`load_rules`]/[`save_rules`]：首次
//! 自动种子 + 原子写 + 覆写前 `.json.bak` 备份）与内置默认集
//! [`DEFAULT_RULES_JSON`]（编译期内嵌，唯一定义源 `eval_rules_default.json`）。
//!
//! 求值半边（assess/match 原语）与 home 路径布局（`rules_file_path`）留在
//! `nemesisbot::eval_assessor`——后者 `pub use` 本模块全部公开项，CLI
//! （commands/eval_rules.rs）与评估链（commands/eval.rs）调用点零迁移。
//! WSAPI `security.eval_rules.*`（Dashboard 评估规则管理 TAB）直接消费本
//! 模块——handler 侧不存在第二份规则结构真相。
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
    let content = std::fs::read_to_string(path)
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
