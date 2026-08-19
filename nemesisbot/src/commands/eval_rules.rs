//! `nemesisbot eval rules ...` —— 规则管理命令组（plan Step 3）。
//!
//! 纯文件操作（读改写 `workspace/config/eval_rules.json`），无平台门控、
//! 无沙盒依赖——run() 里 Rules 分支前置独立处理（A1 修订）。
//!
//! add/edit 校验 JSON 结构 + 正则合法性 + id 规则（add 冲突拒绝、
//! edit 要求存在、remove/enable/disable 不存在报错）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::common;
use crate::eval_assessor;

#[derive(Subcommand)]
pub enum RulesAction {
    /// List all rules (disabled ones marked).
    List {
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Show one rule's full definition.
    Show {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Interactive rule wizard — answer a few questions, no JSON needed.
    /// (For programmatic/batch use, keep `add --file`.)
    New {
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Add rule(s) from a JSON file (single rule object or {"rules":[...]}).
    Add {
        /// Path to the rule JSON file.
        #[arg(long)]
        file: PathBuf,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Replace an existing rule from a JSON file (single rule object; id in file wins).
    Edit {
        id: String,
        /// Path to the new rule JSON file.
        #[arg(long)]
        file: PathBuf,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Remove a rule.
    Remove {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Enable a rule.
    Enable {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Disable a rule.
    Disable {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Reset to the built-in default rule set (confirmation prompt; --force skips).
    Reset {
        /// Skip the confirmation prompt.
        #[arg(long)]
        force: bool,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
}

pub async fn run(action: RulesAction, cli_local: bool) -> Result<()> {
    use RulesAction as A;
    // Z1 修复：每个子命令带 --local（与 eval prompt/skill 的 EvalCommon.local
    // 对齐——全局 --local 也仍然有效，两者取或）。缺它时用户按 eval 其他
    // 子命令的肌肉记忆写 `eval rules list --local` 会 clap 报错。
    let (local, act, writes): (bool, A, bool) = match action {
        A::List { local } => (local, A::List { local }, false),
        A::Show { id, local } => (local, A::Show { id, local }, false),
        A::New { local } => (local, A::New { local }, true),
        A::Add { file, local } => (local, A::Add { file, local }, true),
        A::Edit { id, file, local } => (local, A::Edit { id, file, local }, true),
        A::Remove { id, local } => (local, A::Remove { id, local }, true),
        A::Enable { id, local } => (local, A::Enable { id, local }, true),
        A::Disable { id, local } => (local, A::Disable { id, local }, true),
        A::Reset { force, local } => (local, A::Reset { force, local }, true),
    };
    let home = common::resolve_home(cli_local || local);

    // BB4（第十轮）防静默建家：--local 指向的 {cwd}/.nemesisbot 不存在时，
    // resolve_home 仍返回该路径，load_rules 会在那里**静默创建**完整 home
    //（实测：在仓库根跑 `eval rules add --local` 就地长出一棵 .nemesisbot/）。
    // 写命令拒绝；只读命令直接展示内置默认集（不落盘、不建目录）。
    let local_missing = (cli_local || local) && !home.exists();
    if local_missing {
        if writes {
            bail!(
                "--local home 不存在：{}（写命令拒绝静默创建；先在目标目录 onboard，或去掉 --local 用默认 home）",
                home.display()
            );
        }
        // 只读命令（list/show）：从内置默认集展示，不落盘不建目录。
        println!("--local home 不存在：{}（以下为内置默认规则，未读写任何文件）", home.display());
        let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON)?;
        match &act {
            A::Show { id, .. } => {
                let r = defaults
                    .rules
                    .iter()
                    .find(|r| r.id == *id)
                    .with_context(|| format!("rule '{id}' not found in built-in defaults"))?;
                println!("{}", serde_json::to_string_pretty(r)?);
            }
            _ => print_rule_table(&defaults),
        }
        return Ok(());
    }

    let path = eval_assessor::rules_file_path(&home);

    match act {
        A::List { .. } => cmd_list(&path),
        A::Show { id, .. } => cmd_show(&path, &id),
        A::New { .. } => cmd_new(&path),
        A::Add { file, .. } => cmd_add(&path, &file),
        A::Edit { id, file, .. } => cmd_edit(&path, &id, &file),
        A::Remove { id, .. } => cmd_remove(&path, &id),
        A::Enable { id, .. } => cmd_toggle(&path, &id, true),
        A::Disable { id, .. } => cmd_toggle(&path, &id, false),
        A::Reset { force, .. } => cmd_reset(&path, force),
    }
}

/// 单条规则的匹配条件摘要（list 里直接可见"匹配什么"，不用再 show）：
/// `arguments.command ~ (?i)(\.aws|...) 且 min 1 条`。
fn condition_summary(r: &eval_assessor::Rule) -> String {
    let parts: Vec<String> = r
        .conditions
        .iter()
        .map(|c| {
            let val = match &c.value {
                serde_json::Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            // 值太长截断显示（完整内容用 show <id>）
            let val_display = if val.len() > 60 {
                format!("{}…", &val[..crate::eval_assessor::truncation_point(&val, 60)])
            } else {
                val
            };
            match c.op.as_str() {
                "exists" => format!("{} 存在", c.field),
                "equals" => format!("{} == {}", c.field, val_display),
                "contains" => format!("{} 含 '{}'", c.field, val_display),
                "regex" => format!("{} 匹配 /{}/", c.field, val_display),
                "gt" => format!("{} > {}", c.field, val_display),
                _ => format!("{} {} {}", c.field, c.op, val_display),
            }
        })
        .collect();
    format!(
        "{}{}",
        parts.join(" 且 "),
        if r.min_count > 1 { format!("（≥{} 条记录）", r.min_count) } else { String::new() }
    )
}

fn cmd_list(path: &std::path::Path) -> Result<()> {
    let file = eval_assessor::load_rules(path)?;
    println!("Rules file: {}", path.display());
    print_rule_table(&file);
    Ok(())
}

/// 规则表打印（cmd_list 与 --local-home-missing 的降级展示共用）。
/// 每行带 MATCH 列（匹配条件摘要）——用户在列表里直接看到"匹配什么"，
/// 不用再 show <id> 才知道规则内容。
fn print_rule_table(file: &eval_assessor::RulesFile) {
    if file.rules.is_empty() {
        println!("(no rules)");
        return;
    }
    println!(
        "{:<3} {:<28} {:<8} {:<7} {:<14} {}",
        "#", "ID", "LEVEL", "ON", "SOURCE", "DESCRIPTION"
    );
    for (i, r) in file.rules.iter().enumerate() {
        let on = if r.enabled { "yes" } else { "no" };
        println!(
            "{:<3} {:<28} {:<8} {:<7} {:<14} {}",
            i + 1,
            r.id,
            r.level,
            on,
            r.source,
            r.description
        );
        // 匹配条件摘要（缩进第二行——表格列已宽，正则单独一行更可读）
        println!("     ↳ 匹配: {}", condition_summary(r));
    }
    let enabled = file.rules.iter().filter(|r| r.enabled).count();
    println!("\n{} rule(s), {} enabled", file.rules.len(), enabled);
}

fn cmd_show(path: &std::path::Path, id: &str) -> Result<()> {
    let file = eval_assessor::load_rules(path)?;
    let r = file
        .rules
        .iter()
        .find(|r| r.id == id)
        .with_context(|| format!("rule '{id}' not found (see: eval rules list)"))?;
    println!("{}", serde_json::to_string_pretty(r)?);
    Ok(())
}

fn cmd_add(path: &std::path::Path, file_in: &std::path::Path) -> Result<()> {
    let content = std::fs::read_to_string(file_in)
        .with_context(|| format!("read rules file {}", file_in.display()))?;
    let incoming = eval_assessor::parse_rules_lenient(&content)
        .with_context(|| format!("invalid rules file {}", file_in.display()))?;
    if incoming.rules.is_empty() {
        bail!("input file contains no rules");
    }

    let mut file = eval_assessor::load_rules(path)?;
    for r in &incoming.rules {
        if file.rules.iter().any(|x| x.id == r.id) {
            bail!("rule '{}' already exists (use: eval rules edit {} --file ...)", r.id, r.id);
        }
    }
    file.rules.extend(incoming.rules.clone());
    eval_assessor::save_rules(path, &file)?;
    for r in &incoming.rules {
        println!("added rule '{}' ({}, {})", r.id, r.level, r.source);
    }
    Ok(())
}

fn cmd_edit(path: &std::path::Path, id: &str, file_in: &std::path::Path) -> Result<()> {
    let content = std::fs::read_to_string(file_in)
        .with_context(|| format!("read rules file {}", file_in.display()))?;
    let incoming = eval_assessor::parse_rules_lenient(&content)
        .with_context(|| format!("invalid rules file {}", file_in.display()))?;
    // edit 是单规则语义：多条规则静默取第一条丢其余是坑——明确拒绝，
    // 想批量加用 add（add 天然支持 {"rules":[...]})。
    if incoming.rules.len() != 1 {
        bail!(
            "edit expects exactly one rule, got {} (use `eval rules add --file` for batch)",
            incoming.rules.len()
        );
    }
    let new_rule = incoming.rules.into_iter().next().unwrap();
    if new_rule.id != id {
        bail!("rule id mismatch: edit target is '{id}' but the file defines '{}'", new_rule.id);
    }

    let mut file = eval_assessor::load_rules(path)?;
    let idx = file
        .rules
        .iter()
        .position(|r| r.id == id)
        .with_context(|| format!("rule '{id}' not found (see: eval rules list)"))?;
    file.rules[idx] = new_rule;
    eval_assessor::save_rules(path, &file)?;
    println!("updated rule '{id}'");
    Ok(())
}

fn cmd_remove(path: &std::path::Path, id: &str) -> Result<()> {
    let mut file = eval_assessor::load_rules(path)?;
    let before = file.rules.len();
    file.rules.retain(|r| r.id != id);
    if file.rules.len() == before {
        bail!("rule '{id}' not found (see: eval rules list)");
    }
    eval_assessor::save_rules(path, &file)?;
    println!("removed rule '{id}'");
    Ok(())
}

fn cmd_toggle(path: &std::path::Path, id: &str, enable: bool) -> Result<()> {
    let mut file = eval_assessor::load_rules(path)?;
    let r = file
        .rules
        .iter_mut()
        .find(|r| r.id == id)
        .with_context(|| format!("rule '{id}' not found (see: eval rules list)"))?;
    r.enabled = enable;
    let state = if enable { "enabled" } else { "disabled" };
    eval_assessor::save_rules(path, &file)?;
    println!("{state} rule '{id}'");
    Ok(())
}

fn cmd_reset(path: &std::path::Path, force: bool) -> Result<()> {
    let existing = eval_assessor::load_rules(path)?; // also seeds if missing
    let custom_count = existing.rules.len();

    if !force {
        print!(
            "Reset will replace {custom_count} rule(s) in {} with the built-in defaults. Continue? (y/N): ",
            path.display()
        );
        use std::io::{self as std_io, Write as StdWrite};
        std_io::stdout().flush().ok();
        let mut answer = String::new();
        std_io::stdin().read_line(&mut answer).ok();
        if answer.trim().to_lowercase() != "y" {
            println!("Aborted (use --force to skip confirmation).");
            return Ok(());
        }
    }

    let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON)?;
    eval_assessor::save_rules(path, &defaults)?;
    println!("reset {custom_count} → {} rule(s) from built-in defaults", defaults.rules.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// 交互式规则向导（2026-08-19 用户决定：手写 JSON 对用户不可接受——
// 转义/AND-OR 拆分/字段名全是工程师知识。向导把这一切包掉：用户只回答
// "想抓什么"，程序生成正则、拆 OR、起 id、校验、预览、确认后写入。）
// ---------------------------------------------------------------------------

/// 读取一行用户输入（trim）。
fn ask(prompt: &str) -> String {
    use std::io::{self as std_io, Write as StdWrite};
    print!("{prompt}");
    std_io::stdout().flush().ok();
    let mut line = String::new();
    std_io::stdin().read_line(&mut line).ok();
    line.trim().to_string()
}

/// 读取一行并解析为选项序号（1..=max）。空输入 = 默认值。
fn ask_choice(prompt: &str, max: usize, default: usize) -> usize {
    loop {
        let raw = ask(prompt);
        if raw.is_empty() {
            return default;
        }
        if let Ok(n) = raw.parse::<usize>() {
            if (1..=max).contains(&n) {
                return n;
            }
        }
        println!("  请输入 1-{max}（回车 = 默认）");
    }
}

/// 把用户输入的"要抓的内容"（路径/文件名/关键词，任意形式）编译成安全正则：
/// - regex 元字符全部转义（用户输入按字面量处理）
/// - 前缀 (?i) 大小写不敏感
/// - 路径分隔符 / \ 互相兼容（用户写哪种都行）
/// 用户永远不需要懂正则。
fn keyword_to_pattern(kw: &str) -> String {
    let escaped: String = kw
        .chars()
        .map(|c| match c {
            // 路径分隔符：两种斜杠等价（必须先于元字符分支——'\' 也是元字符）
            '/' | '\\' => r"[\\/]".to_string(),
            // 正则元字符转义（按字面量处理）
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' => format!("\\{c}"),
            _ => c.to_string(),
        })
        .collect();
    format!("(?i){escaped}")
}

fn cmd_new(path: &std::path::Path) -> Result<()> {
    println!("=== 新建评估规则向导 ===");
    println!("回答几个问题即可，无需写 JSON。（程序化批量添加仍可用 add --file）");
    println!();

    // ── 1. 抓什么类别的行为 ──
    println!("要抓什么行为？");
    println!("  1. 探测/读取敏感路径（文件或目录，如 hosts、某凭据文件、项目外的目录）");
    println!("  2. 执行了特定命令或命令含特定关键词（如 curl 上传、注册表操作）");
    println!("  3. 提示词本身含可疑文字（如越狱指令、特定话术）");
    let kind = ask_choice("选择 [1-3，回车=1]: ", 3, 1);

    let mut rules: Vec<eval_assessor::Rule> = Vec::new();

    match kind {
        3 => {
            // subject：静态文本层
            let kw = ask("\n要抓的文字/关键词（提示词里出现即命中，可含空格）: ");
            if kw.is_empty() {
                bail!("关键词为空，已取消");
            }
            let desc = ask("规则描述（回车自动生成）: ");
            let desc = if desc.is_empty() { format!("提示词含关键词“{kw}”") } else { desc };
            rules.push(eval_assessor::Rule {
                id: format!("subject-{}", slugify(&kw)),
                description: desc,
                level: "high".into(),
                enabled: true,
                source: "subject".into(),
                conditions: vec![eval_assessor::Condition {
                    field: "text".into(),
                    op: "contains".into(),
                    value: serde_json::json!(kw),
                }],
                min_count: 1,
            });
        }
        2 => {
            // 命令层：tool_trace 的 arguments.command
            let kw = ask("\n命令里的关键词（命令中包含该词即命中，如 curl、reg add、可疑脚本名）: ");
            if kw.is_empty() {
                bail!("关键词为空，已取消");
            }
            let level = ask_level();
            let desc = ask("规则描述（回车自动生成）: ");
            let desc = if desc.is_empty() { format!("执行了含“{kw}”的命令") } else { desc };
            rules.push(eval_assessor::Rule {
                id: format!("cmd-{}", slugify(&kw)),
                description: desc,
                level,
                enabled: true,
                source: "tool_trace".into(),
                conditions: vec![eval_assessor::Condition {
                    field: "arguments.command".into(),
                    op: "regex".into(),
                    value: serde_json::json!(keyword_to_pattern(&kw)),
                }],
                min_count: 1,
            });
        }
        _ => {
            // 路径层：tool_trace 的 arguments.command + arguments.path 两条（OR 拆分）
            let kw = ask("\n要保护的路径或文件名（写一部分即可，如 hosts、id_ed25519、.kube、D:\\secret）: ");
            if kw.is_empty() {
                bail!("路径为空，已取消");
            }
            let level = ask_level();
            let desc = ask("规则描述（回车自动生成）: ");
            let desc = if desc.is_empty() { format!("工具调用中探测路径“{kw}”") } else { desc };
            let pattern = keyword_to_pattern(&kw);
            let base_id = format!("probe-{}", slugify(&kw));
            // OR 语义拆两条（引擎只支持记录内 AND）：
            rules.push(eval_assessor::Rule {
                id: base_id.clone(),
                description: desc.clone(),
                level: level.clone(),
                enabled: true,
                source: "tool_trace".into(),
                conditions: vec![eval_assessor::Condition {
                    field: "arguments.command".into(),
                    op: "regex".into(),
                    value: serde_json::json!(pattern.clone()),
                }],
                min_count: 1,
            });
            rules.push(eval_assessor::Rule {
                id: format!("{base_id}-path"),
                description: desc,
                level,
                enabled: true,
                source: "tool_trace".into(),
                conditions: vec![eval_assessor::Condition {
                    field: "arguments.path".into(),
                    op: "regex".into(),
                    value: serde_json::json!(pattern),
                }],
                min_count: 1,
            });
        }
    }

    // ── 校验 + id 去重 ──
    let mut file = eval_assessor::load_rules(path)?;
    for r in &mut rules {
        // id 冲突时自动加后缀（-2、-3…）
        let mut n = 1;
        let base = r.id.clone();
        while file.rules.iter().any(|x| x.id == r.id) {
            n += 1;
            r.id = format!("{base}-{n}");
        }
        eval_assessor::validate_rule(r).with_context(|| format!("生成的规则不合法: {}", r.id))?;
    }

    // ── 预览 + 确认 ──
    println!();
    println!("将新增 {} 条规则：", rules.len());
    for r in &rules {
        println!("  [{}] {} — {}", r.level, r.id, r.description);
        println!("       匹配: {}", condition_summary(r));
    }
    println!();
    let confirm = ask("确认保存？(Y/n): ");
    if confirm.eq_ignore_ascii_case("n") {
        println!("已取消，未写入。");
        return Ok(());
    }

    file.rules.extend(rules.clone());
    eval_assessor::save_rules(path, &file)?;
    println!("已保存 {} 条规则。用 `eval rules list` 查看；下次 eval 自动生效。", rules.len());
    Ok(())
}

/// 等级询问（回车=high）。
fn ask_level() -> String {
    println!();
    println!("严重程度：");
    println!("  1. critical（确认的攻击行为）");
    println!("  2. high（强烈恶意信号）");
    println!("  3. medium（可疑，需人工判读）");
    println!("  4. low（弱信号/噪音倾向）");
    match ask_choice("选择 [1-4，回车=2]: ", 4, 2) {
        1 => "critical".into(),
        3 => "medium".into(),
        4 => "low".into(),
        _ => "high".into(),
    }
}

/// 关键词 → kebab-case id 片段（去非法字符、压缩分隔）。
fn slugify(kw: &str) -> String {
    let s: String = kw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut out = String::new();
    let mut prev_dash = true; // 开头不出现 -
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        }
    }
    // 结尾去 -、防空、限长
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "custom".into()
    } else {
        out.chars().take(40).collect()
    }
}

#[cfg(test)]
mod tests;
