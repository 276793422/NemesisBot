//! Security command - manage security settings, rules, approvals.

use crate::common;
use anyhow::Result;

// ---------------------------------------------------------------------------
// CLI action enums
// ---------------------------------------------------------------------------

#[derive(clap::Subcommand)]
pub enum SecurityAction {
    /// Show security status
    Status,
    /// Enable security module
    Enable,
    /// Disable security module
    Disable,
    /// Show or manage security configuration
    Config {
        #[command(subcommand)]
        action: Option<SecurityConfigAction>,
    },
    /// Show audit log
    Audit {
        #[command(subcommand)]
        action: Option<AuditAction>,
    },
    /// Manage security scanner
    Scanner {
        #[command(subcommand)]
        action: ScannerAction,
    },
    /// Test a security check
    Test {
        /// Tool name to test
        #[arg(long)]
        tool: String,
        /// Arguments as JSON
        #[arg(long)]
        args: String,
    },
    /// Manage security rules
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },
    /// Manage saved approval rules (F3 always-allow memory)
    Approvals {
        #[command(subcommand)]
        action: ApprovalsAction,
    },
    /// Approve a pending operation
    Approve {
        /// Operation ID to approve
        id: String,
    },
    /// Deny a pending operation
    Deny {
        /// Operation ID to deny
        id: String,
        /// Reason for denial (optional)
        #[arg(trailing_var_arg = true)]
        reason: Vec<String>,
    },
    /// List pending approval requests
    Pending,
    /// Open security config in editor
    Edit,
    /// Reset security config to defaults
    #[command(name = "config-reset")]
    ConfigReset,
}

#[derive(clap::Subcommand)]
pub enum SecurityConfigAction {
    /// Show security configuration
    Show,
    /// Open security config in editor
    Edit,
    /// Reset security config to defaults
    Reset,
}

#[derive(clap::Subcommand)]
pub enum AuditAction {
    /// Show audit log entries
    Show {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Export audit log to file
    Export {
        /// Output file path
        output: String,
    },
    /// Show denied operations
    Denied,
}

/// Re-export ScannerAction from the standalone scanner module to avoid duplication.
pub use super::scanner::ScannerAction;

#[derive(clap::Subcommand)]
pub enum RulesAction {
    /// List security rules
    List {
        /// Filter by rule type (file/directory/process/network/hardware/registry)
        rule_type: Option<String>,
    },
    /// Add a security rule
    Add {
        /// Rule type: file, directory, process, network, hardware, registry
        rule_type: String,
        /// Operation to match
        operation: String,
        /// Pattern to match (supports * and ** wildcards)
        #[arg(long)]
        pattern: Option<String>,
        /// Action: allow, deny, or ask
        #[arg(long, default_value = "deny")]
        action: Option<String>,
    },
    /// Remove a security rule
    Remove {
        /// Rule type
        rule_type: String,
        /// Operation
        operation: String,
        /// Rule index
        index: usize,
    },
    /// Test a rule against a target path
    Test {
        /// Rule type
        rule_type: String,
        /// Operation
        operation: String,
        /// Target to test against rules
        target: String,
    },
}

/// F3: approvals 管理动作（always-allow 规则表）。
#[derive(clap::Subcommand)]
pub enum ApprovalsAction {
    /// List saved approval rules (always-allow memory)
    List,
    /// Clear all saved approval rules
    Clear,
}

// ---------------------------------------------------------------------------
// Rule types and helpers
// ---------------------------------------------------------------------------

const VALID_RULE_TYPES: &[&str] = &[
    "file",
    "directory",
    "process",
    "network",
    "hardware",
    "registry",
];

/// Valid operations per rule type.
fn valid_operations_for_type(rule_type: &str) -> &[&str] {
    match rule_type {
        "file" => &["read", "write", "delete"],
        "directory" => &["read", "create", "delete"],
        "process" => &["exec", "spawn", "kill", "suspend"],
        "network" => &["request", "download", "upload"],
        "hardware" => &["i2c", "spi", "gpio"],
        "registry" => &["read", "write", "delete"],
        _ => &[],
    }
}

/// CFG-02（2026-09-16 横扫存量加固）：rules 子命令曾读写死键 `rules.*`
/// （`{file:[],directory:[]...}` 平铺）——生产真相源是
/// `file_rules`/`dir_rules`/`process_rules`... 分节 + 每节按操作名分组的
/// `{pattern,action,comment}` 数组（见 security_setup::load_security_rules）。
/// 旧实现对死键的一切增删测试都不影响运行时行为（用户以为加白实际没加）。
/// 现全部对齐真相源；类型→分节名单一映射如下。
fn section_name_for_type(rule_type: &str) -> Option<&'static str> {
    match rule_type {
        "file" => Some("file_rules"),
        "directory" => Some("dir_rules"),
        "process" => Some("process_rules"),
        "network" => Some("network_rules"),
        "hardware" => Some("hardware_rules"),
        "registry" => Some("registry_rules"),
        _ => None,
    }
}

/// 取 `<分节>.<操作>` 规则数组的可变引用（缺节/缺操作时创建空数组；
/// 已存在的 null 归一为空容器——typed 保存会为空分节写出 `null`，
/// `or_insert_with` 不替换已有键，不归一则 push 静默落空）。
fn op_rules_mut<'a>(
    cfg: &'a mut serde_json::Value,
    section: &str,
    operation: &str,
) -> Option<&'a mut Vec<serde_json::Value>> {
    cfg.as_object_mut()?
        .entry(section.to_string())
        .and_modify(|v| {
            if v.is_null() {
                *v = serde_json::Value::Object(serde_json::Map::new());
            }
        })
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()?
        .entry(operation.to_string())
        .and_modify(|v| {
            if v.is_null() {
                *v = serde_json::Value::Array(vec![]);
            }
        })
        .or_insert_with(|| serde_json::Value::Array(vec![]))
        .as_array_mut()
}

/// 取 `<分节>.<操作>` 规则数组的只读引用（缺失 = None）。
fn op_rules<'a>(
    cfg: &'a serde_json::Value,
    section: &str,
    operation: &str,
) -> Option<&'a Vec<serde_json::Value>> {
    cfg.get(section)?.get(operation)?.as_array()
}

/// Read the security config raw（真相源即磁盘文件；缺失时给最小骨架）。
fn read_rules_config(security_cfg: &std::path::Path) -> Result<serde_json::Value> {
    if security_cfg.exists() {
        let data = std::fs::read_to_string(security_cfg)?;
        Ok(serde_json::from_str(&data)?)
    } else {
        Ok(default_security_config())
    }
}

/// 最小出厂骨架。注意（B-F7 复核 2026-09-16）：这是「出厂模板形态」的
/// 近似（D1/D2 用户裁决值），**不是**「缺文件时 runtime 的默认」——
/// config.security.json 缺失时 load_security_rules 提前返回，auditor 保持
/// 构造默认 `default_action: "deny"`；而本骨架写盘后下次启动生效的是
/// `allow`。即：对一个缺文件系统执行 reset/写盘类命令 = 从 deny 放宽到
/// allow，属显式的出厂姿态选择（与出厂模板 default_action=allow 一致），
/// 非静默降级。
fn default_security_config() -> serde_json::Value {
    serde_json::json!({
        "default_action": "allow",
        "exec_unknown_policy": "allow",
        "guardian_failure_policy": "ask"
    })
}

fn write_rules_config(security_cfg: &std::path::Path, cfg: &serde_json::Value) -> Result<()> {
    let dir = security_cfg.parent().unwrap();
    let _ = std::fs::create_dir_all(dir);
    // REL-002：统一原子写入（security 规则配置）。
    nemesis_utils::write_file_atomic(
        &security_cfg.to_string_lossy(),
        serde_json::to_string_pretty(cfg)
            .unwrap_or_default()
            .as_bytes(),
        0o600,
    )
    .map_err(anyhow::Error::msg)?;
    Ok(())
}

/// Wildcard pattern matching.
///
/// - `*` matches any characters except path separators (`/` and `\`)
/// - `**` matches any characters including path separators
pub fn match_pattern(pattern: &str, target: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let target = target.replace('\\', "/");
    match_pattern_inner(&pattern, &target)
}

fn match_pattern_inner(pattern: &str, target: &str) -> bool {
    // Normalize pattern: **/ and /** both become ** (standard glob behavior)
    let normalized_pattern = pattern
        .replace("/**/", "/**")
        .replace("/**", "**")
        .replace("**/", "**");

    let p_chars: Vec<char> = normalized_pattern.chars().collect();
    let t_chars: Vec<char> = target.chars().collect();
    let p_len = p_chars.len();
    let t_len = t_chars.len();

    // Use DP approach: dp[pi][ti] = can pattern[pi..] match target[ti..]
    let mut dp = vec![vec![false; t_len + 1]; p_len + 1];
    dp[p_len][t_len] = true;

    // Fill trailing stars
    for pi in (0..p_len).rev() {
        if p_chars[pi] == '*' {
            dp[pi][t_len] = dp[pi + 1][t_len];
        } else {
            break;
        }
    }

    for pi in (0..p_len).rev() {
        for ti in (0..t_len).rev() {
            let pc = p_chars[pi];
            if pc == '?' {
                dp[pi][ti] = dp[pi + 1][ti + 1];
            } else if pc == '*' {
                // Count consecutive stars
                let mut star_end = pi + 1;
                while star_end < p_len && p_chars[star_end] == '*' {
                    star_end += 1;
                }

                if star_end - pi >= 2 {
                    // ** matches zero or more chars (including separators)
                    dp[pi][ti] = dp[star_end][ti] || dp[pi][ti + 1];
                } else {
                    // Single * matches any chars except separators
                    if t_chars[ti] == '/' || t_chars[ti] == '\\' {
                        dp[pi][ti] = dp[pi + 1][ti];
                    } else {
                        dp[pi][ti] = dp[pi + 1][ti] || dp[pi][ti + 1];
                    }
                }
            } else if pc == t_chars[ti]
                || (pc == '/' && t_chars[ti] == '\\')
                || (pc == '\\' && t_chars[ti] == '/')
            {
                dp[pi][ti] = dp[pi + 1][ti + 1];
            }
        }
    }

    dp[0][0]
}

// ---------------------------------------------------------------------------
// Rules sub-commands
// ---------------------------------------------------------------------------

/// F3: 列出「总是允许」规则表（`<workspace>/config/approval_rules.json`，
/// 路径真相源 nemesis-path；文件缺失/为空都诚实展示空表）。
fn cmd_approvals_list(home: &std::path::Path) -> Result<()> {
    let path =
        nemesis_path::resolve_approval_rules_path_in_workspace(&common::workspace_path(home));
    println!("Approval Rules (always-allow memory)");
    println!("=====================================");
    println!("File: {}", path.display());
    if !path.exists() {
        println!("(none — no always-allow rules saved)");
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)?;
    let rules: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
    if rules.is_empty() {
        println!("(empty)");
        return Ok(());
    }
    for (i, r) in rules.iter().enumerate() {
        let op = r.get("op").and_then(|v| v.as_str()).unwrap_or("?");
        let pattern = r.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
        let action = r.get("action").and_then(|v| v.as_str()).unwrap_or("?");
        let created = r.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        println!(
            "  [{}] {} pattern='{}' action={} created={}",
            i + 1,
            op,
            pattern,
            action,
            created
        );
    }
    Ok(())
}

/// F3: 清空「总是允许」规则表（写回空表，文件保留；运行中 gateway 的
/// HotReloader 经 mtime 感知）。
fn cmd_approvals_clear(home: &std::path::Path) -> Result<()> {
    let path =
        nemesis_path::resolve_approval_rules_path_in_workspace(&common::workspace_path(home));
    let removed = if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        serde_json::from_str::<Vec<serde_json::Value>>(&raw)
            .map(|v| v.len())
            .unwrap_or(0)
    } else {
        0
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // REL-002：统一原子写入（approval 规则表清空）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), b"[]\n", 0o600)
        .map_err(anyhow::Error::msg)?;
    println!("Cleared {} approval rule(s): {}", removed, path.display());
    Ok(())
}

fn cmd_rules_list(security_cfg: &std::path::Path, rule_type: Option<&str>) -> Result<()> {
    let cfg = read_rules_config(security_cfg)?;

    println!("Security Rules");
    println!("==============");

    let types_to_show = if let Some(rt) = rule_type {
        if !VALID_RULE_TYPES.contains(&rt) {
            println!(
                "Invalid rule type: {}. Valid types: {:?}",
                rt, VALID_RULE_TYPES
            );
            return Ok(());
        }
        vec![rt]
    } else {
        VALID_RULE_TYPES.to_vec()
    };

    let mut found_any = false;
    for rt in &types_to_show {
        let Some(section) = section_name_for_type(rt) else {
            continue;
        };
        let valid_ops = valid_operations_for_type(rt);
        let mut type_has_rules = false;
        let mut type_output = String::new();
        for op in valid_ops {
            if let Some(arr) = op_rules(&cfg, section, op) {
                if arr.is_empty() {
                    continue;
                }
                if !type_has_rules {
                    type_has_rules = true;
                    found_any = true;
                    type_output.push_str(&format!("\n  [{}] ({})\n", rt, section));
                }
                for (i, entry) in arr.iter().enumerate() {
                    let pattern = entry.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
                    let action = entry
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("deny");
                    type_output.push_str(&format!(
                        "    {} [{}]: pattern: {:<30} action: {}\n",
                        op, i, pattern, action
                    ));
                }
            }
        }
        print!("{}", type_output);
    }

    if !found_any {
        if rule_type.is_some() {
            println!("  No rules defined for this type.");
        } else {
            println!("  No rules defined.");
        }
    }
    Ok(())
}

fn cmd_rules_add(
    security_cfg: &std::path::Path,
    rule_type: &str,
    operation: &str,
    pattern: Option<&str>,
    action: Option<&str>,
) -> Result<()> {
    if !VALID_RULE_TYPES.contains(&rule_type) {
        println!(
            "Error: Invalid rule type '{}'. Valid types: {:?}",
            rule_type, VALID_RULE_TYPES
        );
        return Ok(());
    }

    // Validate operation for this type
    let valid_ops = valid_operations_for_type(rule_type);
    if !valid_ops.contains(&operation) {
        println!(
            "Error: Invalid {} operation '{}'. Valid: {}",
            rule_type,
            operation,
            valid_ops.join(", ")
        );
        return Ok(());
    }

    let action_val = action.unwrap_or("deny");
    if action_val != "allow" && action_val != "deny" && action_val != "ask" {
        println!(
            "Error: Invalid action '{}'. Must be 'allow', 'deny', or 'ask'.",
            action_val
        );
        return Ok(());
    }

    let Some(section) = section_name_for_type(rule_type) else {
        return Ok(());
    };
    let mut cfg = read_rules_config(security_cfg)?;

    match op_rules_mut(&mut cfg, section, operation) {
        Some(arr) => {
            arr.push(serde_json::json!({
                "pattern": pattern.unwrap_or("*"),
                "action": action_val,
                "comment": ""
            }));
        }
        None => {
            // 复核 2026-09-16：此前无条件打印成功——`op_rules_mut` 返回
            // None（分节/操作层是 null 或类型不符）时规则根本没写进去，
            // 「以为加白实际没加」的假姿态复发。诚实报错退出。
            println!(
                "Error: failed to add rule: section '{}.{}' exists but is not a rules array (malformed config?). Fix or remove the '{}' section manually.",
                section, operation, section
            );
            return Ok(());
        }
    }

    write_rules_config(security_cfg, &cfg)?;
    println!(
        "Rule added: [{} {}] {} -> {}",
        section,
        operation,
        pattern.unwrap_or("*"),
        action_val
    );
    if action_val == "ask" {
        println!("NOTE: 'ask' = requires interactive approval (an approval card is shown).");
    }
    println!("NOTE: Restart the gateway (or reload config) for the change to take effect.");
    Ok(())
}

fn cmd_rules_remove(
    security_cfg: &std::path::Path,
    rule_type: &str,
    operation: &str,
    index: usize,
) -> Result<()> {
    if !VALID_RULE_TYPES.contains(&rule_type) {
        println!(
            "Invalid rule type: {}. Valid types: {:?}",
            rule_type, VALID_RULE_TYPES
        );
        return Ok(());
    }

    let Some(section) = section_name_for_type(rule_type) else {
        return Ok(());
    };
    let mut cfg = read_rules_config(security_cfg)?;

    // index 与 `rules list` 输出的操作内序号一致（同源数组位置）。
    let removed = op_rules_mut(&mut cfg, section, operation).and_then(|arr| {
        if index < arr.len() {
            Some(arr.remove(index))
        } else {
            None
        }
    });

    match removed {
        Some(entry) => {
            write_rules_config(security_cfg, &cfg)?;
            println!(
                "Rule removed: [{} {}] #{} pattern={}",
                section,
                operation,
                index,
                entry.get("pattern").and_then(|v| v.as_str()).unwrap_or("?")
            );
            println!("NOTE: Restart the gateway (or reload config) for the change to take effect.");
        }
        None => {
            println!("Rule not found: [{} {}] #{}", section, operation, index);
        }
    }
    Ok(())
}

fn cmd_rules_test(
    security_cfg: &std::path::Path,
    rule_type: &str,
    operation: &str,
    target: &str,
) -> Result<()> {
    if !VALID_RULE_TYPES.contains(&rule_type) {
        println!(
            "Invalid rule type: {}. Valid types: {:?}",
            rule_type, VALID_RULE_TYPES
        );
        return Ok(());
    }
    let valid_ops = valid_operations_for_type(rule_type);
    if !valid_ops.contains(&operation) {
        println!(
            "Error: Invalid {} operation '{}'. Valid: {}",
            rule_type,
            operation,
            valid_ops.join(", ")
        );
        return Ok(());
    }

    let Some(section) = section_name_for_type(rule_type) else {
        return Ok(());
    };
    let cfg = read_rules_config(security_cfg)?;
    let rules = op_rules(&cfg, section, operation);

    println!("Rule Test Result");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Section:    {} ({})", section, rule_type);
    println!("  Operation:  {}", operation);
    println!("  Target:     {}", target);
    println!();

    // CMD-01 对齐：与运行时 auditor 相同的 deny→ask→allow 三遍扫 +
    // 匹配语义（process 类走命令归一化 + 锚定命令匹配；其余走路径
    // 通配匹配）。无命中落 default_action。
    let is_command_op = rule_type == "process";
    let match_target = if is_command_op {
        nemesis_security::matcher::normalize_exec_command(target)
    } else {
        target.to_string()
    };
    let pattern_matches = |pattern: &str| -> bool {
        if is_command_op {
            nemesis_security::matcher::match_command_pattern(&pattern.to_lowercase(), &match_target)
        } else {
            match_pattern(pattern, target)
        }
    };

    let mut matched: Option<(usize, String, String)> = None;
    if let Some(arr) = rules {
        for want in ["deny", "ask", "allow"] {
            for (i, rule) in arr.iter().enumerate() {
                let action = rule
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deny");
                let bucket = match action {
                    "deny" | "denied" => "deny",
                    "ask" | "require_approval" | "approval" | "pending" => "ask",
                    "allow" | "allowed" => "allow",
                    _ => "deny",
                };
                if bucket != want {
                    continue;
                }
                let pattern = rule.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
                if pattern_matches(pattern) {
                    matched = Some((i, pattern.to_string(), action.to_string()));
                    break;
                }
            }
            if matched.is_some() {
                break;
            }
        }
    }

    match matched {
        Some((i, pattern, action)) => {
            let (icon, label) = match action.as_str() {
                "allow" | "allowed" => ("ALLOWED", "allowed"),
                "ask" | "require_approval" => ("ASK", "requires approval"),
                _ => ("DENIED", "denied"),
            };
            println!("  Matched rule [{}]: {} -> {}", i, pattern, action);
            println!();
            println!("  Result:  {}", icon);
            println!("  Reason:  Matched rule ({})", label);
        }
        None => {
            let default_action = cfg
                .get("default_action")
                .and_then(|v| v.as_str())
                .unwrap_or("deny");
            let (icon, label) = match default_action {
                "allow" | "allowed" => ("ALLOWED", "allowed"),
                "ask" | "require_approval" => ("ASK", "requires approval"),
                _ => ("DENIED", "denied"),
            };
            println!("  Result:  {}", icon);
            println!(
                "  Reason:  No rule matched; default_action={} ({}) applies",
                default_action, label
            );
        }
    }

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    Ok(())
}

// ---------------------------------------------------------------------------
// Approvals
// ---------------------------------------------------------------------------

fn cmd_approve(security_cfg: &std::path::Path, id: &str) -> Result<()> {
    let pending_path = security_cfg
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workspace")
        .join("security")
        .join("pending.json");

    if !pending_path.exists() {
        println!("No pending operations found.");
        return Ok(());
    }

    let data = std::fs::read_to_string(&pending_path)?;
    let mut pending: Vec<serde_json::Value> = serde_json::from_str(&data)?;
    let before = pending.len();
    pending.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(id));

    if pending.len() < before {
        // REL-002：统一原子写入（security 待审批清单）。
        nemesis_utils::write_file_atomic(
            &pending_path.to_string_lossy(),
            serde_json::to_string_pretty(&pending)
                .unwrap_or_default()
                .as_bytes(),
            0o600,
        )
        .map_err(anyhow::Error::msg)?;
        println!("Operation {} approved.", id);
    } else {
        println!("Operation {} not found.", id);
    }
    Ok(())
}

fn cmd_deny(security_cfg: &std::path::Path, id: &str, reason: Option<&str>) -> Result<()> {
    let pending_path = security_cfg
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workspace")
        .join("security")
        .join("pending.json");

    if !pending_path.exists() {
        println!("No pending operations found.");
        return Ok(());
    }

    let data = std::fs::read_to_string(&pending_path)?;
    let mut pending: Vec<serde_json::Value> = serde_json::from_str(&data)?;
    let before = pending.len();
    pending.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(id));

    if pending.len() < before {
        // REL-002：统一原子写入（security 待审批清单）。
        nemesis_utils::write_file_atomic(
            &pending_path.to_string_lossy(),
            serde_json::to_string_pretty(&pending)
                .unwrap_or_default()
                .as_bytes(),
            0o600,
        )
        .map_err(anyhow::Error::msg)?;
        println!(
            "Operation {} denied.{}",
            id,
            reason
                .map(|r| format!(" Reason: {}", r))
                .unwrap_or_default()
        );
    } else {
        println!("Operation {} not found.", id);
    }
    Ok(())
}

fn cmd_pending(security_cfg: &std::path::Path) -> Result<()> {
    let pending_path = security_cfg
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workspace")
        .join("security")
        .join("pending.json");

    println!("Pending Approvals");
    println!("==================");

    if !pending_path.exists() {
        println!("  No pending operations.");
        return Ok(());
    }

    let data = std::fs::read_to_string(&pending_path)?;
    let pending: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();

    if pending.is_empty() {
        println!("  No pending operations.");
    } else {
        for p in &pending {
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let op = p.get("operation").and_then(|v| v.as_str()).unwrap_or("?");
            let tool = p.get("tool_name").and_then(|v| v.as_str()).unwrap_or("?");
            let ts = p.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?");
            println!("  {} | {} / {} ({})", id, tool, op, ts);
        }
        println!();
        println!("  Total: {} pending", pending.len());
    }
    Ok(())
}

fn cmd_edit(security_cfg: &std::path::Path) -> Result<()> {
    // Ensure config exists
    if !security_cfg.exists() {
        let cfg = default_security_config();
        write_rules_config(security_cfg, &cfg)?;
    }

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });

    println!("Opening security config in {}...", editor);
    println!("  Path: {}", security_cfg.display());

    // Block until editor closes
    let status = std::process::Command::new(&editor)
        .arg(security_cfg)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Configuration saved.");
            println!("Restart gateway to apply changes.");
        }
        Ok(s) => println!("Editor exited with status: {}", s),
        Err(e) => println!("Failed to open editor: {}", e),
    }

    Ok(())
}

fn cmd_config_reset(security_cfg: &std::path::Path) -> Result<()> {
    print!("This will reset security configuration to defaults. Continue? (y/n): ");
    use std::io::{self, Write};
    io::stdout().flush().ok();

    let mut response = String::new();
    io::stdin().read_line(&mut response).ok();
    let answer = response.trim().to_lowercase();

    if answer != "y" {
        println!("Aborted.");
        return Ok(());
    }

    let cfg = default_security_config();
    write_rules_config(security_cfg, &cfg)?;
    println!("Security configuration reset to defaults.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub async fn run(action: SecurityAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);
    let security_cfg = common::security_config_path(&home);

    match action {
        SecurityAction::Status => {
            println!("🛡️ Security Status");
            println!("===============");

            let enabled = if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let cfg: serde_json::Value = serde_json::from_str(&data)?;
                cfg.get("security")
                    .and_then(|s| s.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
            } else {
                true
            };

            println!(
                "  Security module: {}",
                if enabled { "enabled" } else { "disabled" }
            );

            // Show policy settings from security config
            let rules_cfg = read_rules_config(&security_cfg)?;
            let default_action = rules_cfg
                .get("default_action")
                .and_then(|v| v.as_str())
                .unwrap_or("allow")
                .to_uppercase();
            let log_ops = rules_cfg
                .get("log_all_operations")
                .and_then(|v| v.as_bool())
                .map(|v| if v { "yes" } else { "no" })
                .unwrap_or("no");
            let file_log = rules_cfg
                .get("audit_log_file_enabled")
                .and_then(|v| v.as_bool())
                .map(|v| if v { "yes" } else { "no" })
                .unwrap_or("no");
            // CFG-05（2026-09-16）：键名对齐磁盘真相源。此前读
            // `approval_timeout`/`audit_retention_days`——模板键实为
            // `approval_timeout_seconds`/（已删）`audit_log_retention_days`，
            // 显示恒空，属假姿态。retention 已删（无清扫器），不再显示。
            let approval_timeout = rules_cfg
                .get("approval_timeout_seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            println!("  Default action: {}", default_action);
            println!("  Log operations: {}", log_ops);
            println!("  File log: {}", file_log);
            if approval_timeout > 0 {
                println!("  Approval timeout: {}s", approval_timeout);
            }

            // Show policy settings from main config
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let cfg: serde_json::Value = serde_json::from_str(&data)?;
                let restrict = cfg
                    .get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get("restrict_to_workspace"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                println!("  Workspace restricted: {}", restrict);
            }

            // Show scanner status
            if security_cfg.exists() {
                if let Ok(data) = std::fs::read_to_string(&security_cfg)
                    && let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data)
                {
                    if let Some(engines) = cfg.get("enabled").and_then(|v| v.as_array()) {
                        println!("  Scanner engines: {} configured", engines.len());
                    }
                    let restrict = cfg
                        .get("restrict_to_workspace")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    println!("  Restrict to workspace: {}", restrict);
                }
            } else {
                println!("  Scanner: not configured");
            }

            // Show rule counts per operation per type
            if let Some(rules) = rules_cfg.get("rules") {
                println!();
                println!("  Rules by type:");
                let mut total = 0;
                for rt in VALID_RULE_TYPES {
                    if let Some(arr) = rules.get(rt).and_then(|v| v.as_array()) {
                        if arr.is_empty() {
                            continue;
                        }
                        total += arr.len();

                        // Count by operation
                        let valid_ops = valid_operations_for_type(rt);
                        let mut op_counts = Vec::new();
                        for op in valid_ops {
                            let count = arr
                                .iter()
                                .filter(|e| {
                                    e.get("operation").and_then(|v| v.as_str()) == Some(*op)
                                })
                                .count();
                            if count > 0 {
                                op_counts.push(format!("{}={}", op, count));
                            }
                        }
                        if op_counts.is_empty() {
                            println!("    {}: {} rule(s)", rt, arr.len());
                        } else {
                            println!("    {}: {} ({})", rt, arr.len(), op_counts.join(", "));
                        }
                    }
                }
                println!("  Total rules: {}", total);
            }

            println!();
            println!("  Security layers:");
            let layers = [
                "Injection Detection",
                "Command Guard",
                "ABAC Auditor",
                "Credential Scanner",
                "DLP Engine",
                "SSRF Guard",
                "Virus Scanner",
                "Audit Chain",
            ];
            for (i, layer) in layers.iter().enumerate() {
                println!("    {}. {}", i + 1, layer);
            }
        }
        SecurityAction::Enable => {
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(obj) = cfg.as_object_mut() {
                    if let Some(sec) = obj.get_mut("security").and_then(|v| v.as_object_mut()) {
                        sec.insert("enabled".to_string(), serde_json::Value::Bool(true));
                    } else {
                        obj.insert("security".to_string(), serde_json::json!({"enabled": true}));
                    }
                    // Set RestrictToWorkspace = false when security is enabled
                    if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                        if let Some(defaults) =
                            agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                        {
                            defaults.insert(
                                "restrict_to_workspace".to_string(),
                                serde_json::Value::Bool(false),
                            );
                        } else {
                            agents.insert(
                                "defaults".to_string(),
                                serde_json::json!({"restrict_to_workspace": false}),
                            );
                        }
                    }
                    // REL-002：统一原子写入（security enable/disable 改主配置）。
                    nemesis_utils::write_file_atomic(
                        &cfg_path.to_string_lossy(),
                        serde_json::to_string_pretty(&cfg)
                            .unwrap_or_default()
                            .as_bytes(),
                        0o600,
                    )
                    .map_err(anyhow::Error::msg)?;
                }
            }

            // Ensure security config file exists with proper defaults (do not overwrite if it exists)
            if !security_cfg.exists() {
                let default_cfg = default_security_config();
                write_rules_config(&security_cfg, &default_cfg)?;
            }

            println!("Security module enabled");
            println!("  Configuration: {}", security_cfg.display());
            println!("  Workspace restriction: disabled (security module enforces rules instead)");
            println!();
            println!("  Restart agent/gateway to apply changes");
        }
        SecurityAction::Disable => {
            println!("WARNING: Disabling security removes all safety checks.");
            println!("This allows the bot to access the entire system.");
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(obj) = cfg.as_object_mut() {
                    if let Some(sec) = obj.get_mut("security").and_then(|v| v.as_object_mut()) {
                        sec.insert("enabled".to_string(), serde_json::Value::Bool(false));
                    } else {
                        obj.insert(
                            "security".to_string(),
                            serde_json::json!({"enabled": false}),
                        );
                    }
                    // Set RestrictToWorkspace = true when security is disabled
                    if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                        if let Some(defaults) =
                            agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                        {
                            defaults.insert(
                                "restrict_to_workspace".to_string(),
                                serde_json::Value::Bool(true),
                            );
                        } else {
                            agents.insert(
                                "defaults".to_string(),
                                serde_json::json!({"restrict_to_workspace": true}),
                            );
                        }
                    }
                    // REL-002：统一原子写入（security enable/disable 改主配置）。
                    nemesis_utils::write_file_atomic(
                        &cfg_path.to_string_lossy(),
                        serde_json::to_string_pretty(&cfg)
                            .unwrap_or_default()
                            .as_bytes(),
                        0o600,
                    )
                    .map_err(anyhow::Error::msg)?;
                }
            }
            println!("🔓 Security module disabled");
            println!("  Workspace restriction: enabled (all operations restricted to workspace)");
            println!();
            println!("  Restart agent/gateway to apply changes");
        }
        SecurityAction::Config { action } => match action {
            None | Some(SecurityConfigAction::Show) => {
                println!("Security Configuration");
                println!("======================");
                if security_cfg.exists() {
                    println!("{}", std::fs::read_to_string(&security_cfg)?);
                } else {
                    println!("  Using default configuration.");
                }
            }
            Some(SecurityConfigAction::Edit) => cmd_edit(&security_cfg)?,
            Some(SecurityConfigAction::Reset) => cmd_config_reset(&security_cfg)?,
        },
        SecurityAction::Audit { action } => match action {
            None | Some(AuditAction::Show { limit: _ }) => {
                let limit = match &action {
                    Some(AuditAction::Show { limit }) => *limit,
                    _ => 20,
                };
                println!("Security Audit Log (last {} entries)", limit);
                println!("======================================");
                let audit_path = common::workspace_path(&home).join("audit_chain.jsonl");
                if audit_path.exists() {
                    if let Ok(data) = std::fs::read_to_string(&audit_path) {
                        let lines: Vec<&str> = data.lines().collect();
                        for line in lines.iter().rev().take(limit) {
                            if let Ok(evt) = serde_json::from_str::<serde_json::Value>(line) {
                                println!(
                                    "  [{}] {} / {} -> {} ({})",
                                    evt.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("operation").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("tool_name").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("decision").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
                                );
                            }
                        }
                    }
                } else {
                    println!("  No audit log found.");
                }
            }
            Some(AuditAction::Export { output }) => {
                println!("Exporting audit log to: {}", output);
                let audit_path = common::workspace_path(&home).join("audit_chain.jsonl");
                if audit_path.exists() {
                    let data = std::fs::read_to_string(&audit_path)?;
                    let entries: Vec<serde_json::Value> = data
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .filter_map(|l| serde_json::from_str(l).ok())
                        .collect();
                    let export = serde_json::json!({
                        "exported_at": chrono::Local::now().to_rfc3339(),
                        "total_entries": entries.len(),
                        "entries": entries,
                    });
                    std::fs::write(
                        &output,
                        serde_json::to_string_pretty(&export).unwrap_or_default(),
                    )?;
                    println!("  Exported {} entries.", entries.len());
                } else {
                    println!("  No audit log found to export.");
                }
            }
            Some(AuditAction::Denied) => {
                println!("Denied Operations");
                println!("=================");
                let audit_path = common::workspace_path(&home).join("audit_chain.jsonl");
                if audit_path.exists() {
                    if let Ok(data) = std::fs::read_to_string(&audit_path) {
                        let denied: Vec<_> = data
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                            .filter(|e| {
                                e.get("decision").and_then(|v| v.as_str()) == Some("denied")
                            })
                            .collect();
                        if denied.is_empty() {
                            println!("  No denied operations found.");
                        } else {
                            for evt in denied.iter().rev().take(50) {
                                println!(
                                    "  [{}] {} / {} ({})",
                                    evt.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("tool_name").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("operation").and_then(|v| v.as_str()).unwrap_or("?"),
                                    evt.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
                                );
                            }
                            println!("  Total denied: {}", denied.len());
                        }
                    }
                } else {
                    println!("  No audit log found.");
                }
            }
        },
        SecurityAction::Scanner { action } => {
            // Delegate to the standalone scanner module
            super::scanner::run(action, local).await?;
        }
        SecurityAction::Test { tool, args } => {
            println!("Security test for tool '{}':", tool);
            match serde_json::from_str::<serde_json::Value>(&args) {
                Ok(json_args) => {
                    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
                    let plugin = SecurityPlugin::new(SecurityPluginConfig {
                        enabled: true,
                        default_action: "allow".to_string(),
                        ..Default::default()
                    });
                    let invocation = nemesis_security::ToolInvocation {
                        tool_name: tool.clone(),
                        args: json_args,
                        user: "test".to_string(),
                        source: "cli".to_string(),
                        metadata: Default::default(),
                    };
                    let (allowed, err) = plugin.execute(&invocation);
                    if allowed {
                        println!("  Result: ALLOWED");
                    } else {
                        println!("  Result: BLOCKED");
                        if let Some(info) = err {
                            // F5: DenyInfo 结构化反馈（layer/policy 与审计 JSONL 同源）。
                            println!("  Layer: {} | Policy: {}", info.layer, info.policy);
                            println!("  Reason: {}", info.summary);
                            if let Some(sug) = &info.suggestion {
                                println!("  Suggestion: {sug}");
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("  Error: Invalid JSON args: {}", e);
                }
            }
        }
        SecurityAction::Approvals { action } => match action {
            ApprovalsAction::List => cmd_approvals_list(&home)?,
            ApprovalsAction::Clear => cmd_approvals_clear(&home)?,
        },
        SecurityAction::Rules { action } => match action {
            RulesAction::List { rule_type } => cmd_rules_list(&security_cfg, rule_type.as_deref())?,
            RulesAction::Add {
                rule_type,
                operation,
                pattern,
                action,
            } => cmd_rules_add(
                &security_cfg,
                &rule_type,
                &operation,
                pattern.as_deref(),
                action.as_deref(),
            )?,
            RulesAction::Remove {
                rule_type,
                operation,
                index,
            } => cmd_rules_remove(&security_cfg, &rule_type, &operation, index)?,
            RulesAction::Test {
                rule_type,
                operation,
                target,
            } => cmd_rules_test(&security_cfg, &rule_type, &operation, &target)?,
        },
        SecurityAction::Approve { id } => cmd_approve(&security_cfg, &id)?,
        SecurityAction::Deny { id, reason } => {
            let reason_str = if reason.is_empty() {
                None
            } else {
                Some(reason.join(" "))
            };
            cmd_deny(&security_cfg, &id, reason_str.as_deref())?;
        }
        SecurityAction::Pending => cmd_pending(&security_cfg)?,
        SecurityAction::Edit => cmd_edit(&security_cfg)?,
        SecurityAction::ConfigReset => cmd_config_reset(&security_cfg)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests;
