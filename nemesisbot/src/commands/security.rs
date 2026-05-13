//! Security command - manage security settings, rules, approvals.

use anyhow::Result;
use crate::common;

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

// ---------------------------------------------------------------------------
// Rule types and helpers
// ---------------------------------------------------------------------------

const VALID_RULE_TYPES: &[&str] = &["file", "directory", "process", "network", "hardware", "registry"];

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

/// Read or create the security rules config.
fn read_rules_config(security_cfg: &std::path::Path) -> Result<serde_json::Value> {
    if security_cfg.exists() {
        let data = std::fs::read_to_string(security_cfg)?;
        let cfg: serde_json::Value = serde_json::from_str(&data)?;
        // Ensure rules section exists
        let mut cfg = cfg;
        if cfg.get("rules").is_none() {
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert("rules".to_string(), default_rules());
            }
        }
        Ok(cfg)
    } else {
        Ok(default_security_config())
    }
}

fn default_security_config() -> serde_json::Value {
    serde_json::json!({
        "default_action": "ask",
        "log_all_operations": false,
        "log_denials_only": true,
        "approval_timeout": 300,
        "max_pending_requests": 10,
        "audit_retention_days": 30,
        "audit_log_file_enabled": true,
        "synchronous_mode": false,
        "rules": default_rules(),
        "pending": []
    })
}

fn default_rules() -> serde_json::Value {
    serde_json::json!({
        "file": [],
        "directory": [],
        "process": [],
        "network": [],
        "hardware": [],
        "registry": []
    })
}

fn write_rules_config(security_cfg: &std::path::Path, cfg: &serde_json::Value) -> Result<()> {
    let dir = security_cfg.parent().unwrap();
    let _ = std::fs::create_dir_all(dir);
    std::fs::write(security_cfg, serde_json::to_string_pretty(cfg).unwrap_or_default())?;
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
            } else if pc == t_chars[ti] || (pc == '/' && t_chars[ti] == '\\') || (pc == '\\' && t_chars[ti] == '/') {
                dp[pi][ti] = dp[pi + 1][ti + 1];
            }
        }
    }

    dp[0][0]
}

// ---------------------------------------------------------------------------
// Rules sub-commands
// ---------------------------------------------------------------------------

fn cmd_rules_list(security_cfg: &std::path::Path, rule_type: Option<&str>) -> Result<()> {
    let cfg = read_rules_config(security_cfg)?;
    let rules = cfg.get("rules").cloned().unwrap_or_else(|| default_rules());

    println!("Security Rules");
    println!("==============");

    let types_to_show = if let Some(rt) = rule_type {
        if !VALID_RULE_TYPES.contains(&rt) {
            println!("Invalid rule type: {}. Valid types: {:?}", rt, VALID_RULE_TYPES);
            return Ok(());
        }
        vec![rt]
    } else {
        VALID_RULE_TYPES.to_vec()
    };

    let mut found_any = false;
    for rt in &types_to_show {
        if let Some(type_rules) = rules.get(*rt).and_then(|v| v.as_array()) {
            found_any = true;
            println!();
            println!("  [{}]", rt);

            // Group by operation
            let valid_ops = valid_operations_for_type(rt);
            for op in valid_ops {
                let op_rules: Vec<_> = type_rules.iter()
                    .enumerate()
                    .filter(|(_, e)| e.get("operation").and_then(|v| v.as_str()) == Some(*op))
                    .collect();

                if op_rules.is_empty() {
                    println!("    {}: (none)", op);
                } else {
                    for (i, entry) in &op_rules {
                        let pattern = entry.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
                        let action = entry.get("action").and_then(|v| v.as_str()).unwrap_or("deny");
                        println!("    [{}] {}: pattern: {:<30} action: {}", op, i, pattern, action);
                    }
                }
            }
        }
    }

    if !found_any && rule_type.is_none() {
        println!("  No rules defined.");
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
        println!("Error: Invalid rule type '{}'. Valid types: {:?}", rule_type, VALID_RULE_TYPES);
        return Ok(());
    }

    // Validate operation for this type
    let valid_ops = valid_operations_for_type(rule_type);
    if !valid_ops.contains(&operation) {
        println!("Error: Invalid {} operation '{}'. Valid: {}", rule_type, operation, valid_ops.join(", "));
        return Ok(());
    }

    let action_val = action.unwrap_or("deny");
    if action_val != "allow" && action_val != "deny" && action_val != "ask" {
        println!("Error: Invalid action '{}'. Must be 'allow', 'deny', or 'ask'.", action_val);
        return Ok(());
    }

    let mut cfg = read_rules_config(security_cfg)?;

    if let Some(rules) = cfg.get_mut("rules").and_then(|v| v.as_object_mut()) {
        if !rules.contains_key(rule_type) {
            rules.insert(rule_type.to_string(), serde_json::Value::Array(vec![]));
        }
        if let Some(arr) = rules.get_mut(rule_type).and_then(|v| v.as_array_mut()) {
            arr.push(serde_json::json!({
                "pattern": pattern.unwrap_or("*"),
                "operation": operation,
                "action": action_val,
                "comment": ""
            }));
        }
    }

    write_rules_config(security_cfg, &cfg)?;
    println!("Rule added: [{}] {} {} -> {}", rule_type, operation, pattern.unwrap_or("*"), action_val);
    if action_val == "ask" {
        println!("NOTE: The 'ask' action in rules is currently treated as 'deny' for security.");
    }
    Ok(())
}

fn cmd_rules_remove(security_cfg: &std::path::Path, rule_type: &str, operation: &str, index: usize) -> Result<()> {
    if !VALID_RULE_TYPES.contains(&rule_type) {
        println!("Invalid rule type: {}. Valid types: {:?}", rule_type, VALID_RULE_TYPES);
        return Ok(());
    }

    let mut cfg = read_rules_config(security_cfg)?;
    let mut found = false;

    if let Some(rules) = cfg.get_mut("rules").and_then(|v| v.as_object_mut()) {
        if let Some(arr) = rules.get_mut(rule_type).and_then(|v| v.as_array_mut()) {
            // Find entries matching operation at the given index
            let matching: Vec<usize> = arr.iter().enumerate()
                .filter(|(_, e)| e.get("operation").and_then(|v| v.as_str()) == Some(operation))
                .map(|(i, _)| i)
                .collect();

            if index < matching.len() {
                let actual_idx = matching[index];
                arr.remove(actual_idx);
                found = true;
            }
        }
    }

    if found {
        write_rules_config(security_cfg, &cfg)?;
        println!("Rule removed: [{}] {} #{}", rule_type, operation, index);
    } else {
        println!("Rule not found: [{}] {} #{}", rule_type, operation, index);
    }
    Ok(())
}

fn cmd_rules_test(security_cfg: &std::path::Path, rule_type: &str, operation: &str, target: &str) -> Result<()> {
    if !VALID_RULE_TYPES.contains(&rule_type) {
        println!("Invalid rule type: {}. Valid types: {:?}", rule_type, VALID_RULE_TYPES);
        return Ok(());
    }

    // Validate rule type
    let cfg = read_rules_config(security_cfg)?;
    let rules = cfg.get("rules");

    println!("Rule Test Result");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Type:       {}", rule_type);
    println!("  Operation:  {}", operation);
    println!("  Target:     {}", target);
    println!();

    let mut matched = false;
    let mut final_action = "deny"; // default deny
    let mut matched_rule_idx: Option<usize> = None;
    let mut matched_pattern = String::new();

    if let Some(rules) = rules.and_then(|r| r.get(rule_type)).and_then(|r| r.as_array()) {
        for (i, rule) in rules.iter().enumerate() {
            let pattern = rule.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
            let rule_op = rule.get("operation").and_then(|v| v.as_str()).unwrap_or("*");
            let action = rule.get("action").and_then(|v| v.as_str()).unwrap_or("deny");

            if rule_op != "*" && rule_op != operation {
                continue;
            }

            if match_pattern(pattern, target) {
                matched = true;
                matched_rule_idx = Some(i);
                matched_pattern = pattern.to_string();
                // "ask" is treated as deny at runtime
                final_action = if action == "ask" { "deny" } else { action };
                println!("  Matched rule [{}]: {} -> {}", i, pattern, action);
            }
        }
    }

    if matched {
        let (icon, _label) = if final_action == "allow" { ("ALLOWED", "allowed") } else { ("DENIED", "denied") };
        let reason = if final_action == "deny" && cfg.get("rules")
            .and_then(|r| r.get(rule_type))
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.get(matched_rule_idx.unwrap()))
            .and_then(|r| r.get("action"))
            .and_then(|v| v.as_str()) == Some("ask")
        {
            "Matched rule requires approval (treated as deny)".to_string()
        } else {
            format!("Matched rule [{}]: {} -> {}", matched_rule_idx.unwrap(), matched_pattern, final_action)
        };
        println!();
        println!("  Result:  {}", icon);
        println!("  Reason:  {}", reason);
    } else {
        println!();
        println!("  Result:  DENIED");
        println!("  Reason:  No matching rule found; default deny applies");
    }

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    Ok(())
}

// ---------------------------------------------------------------------------
// Approvals
// ---------------------------------------------------------------------------

fn cmd_approve(security_cfg: &std::path::Path, id: &str) -> Result<()> {
    let pending_path = security_cfg.parent().unwrap()
        .parent().unwrap()
        .join("workspace").join("security").join("pending.json");

    if !pending_path.exists() {
        println!("No pending operations found.");
        return Ok(());
    }

    let data = std::fs::read_to_string(&pending_path)?;
    let mut pending: Vec<serde_json::Value> = serde_json::from_str(&data)?;
    let before = pending.len();
    pending.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(id));

    if pending.len() < before {
        std::fs::write(&pending_path, serde_json::to_string_pretty(&pending).unwrap_or_default())?;
        println!("Operation {} approved.", id);
    } else {
        println!("Operation {} not found.", id);
    }
    Ok(())
}

fn cmd_deny(security_cfg: &std::path::Path, id: &str, reason: Option<&str>) -> Result<()> {
    let pending_path = security_cfg.parent().unwrap()
        .parent().unwrap()
        .join("workspace").join("security").join("pending.json");

    if !pending_path.exists() {
        println!("No pending operations found.");
        return Ok(());
    }

    let data = std::fs::read_to_string(&pending_path)?;
    let mut pending: Vec<serde_json::Value> = serde_json::from_str(&data)?;
    let before = pending.len();
    pending.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(id));

    if pending.len() < before {
        std::fs::write(&pending_path, serde_json::to_string_pretty(&pending).unwrap_or_default())?;
        println!("Operation {} denied.{}", id, reason.map(|r| format!(" Reason: {}", r)).unwrap_or_default());
    } else {
        println!("Operation {} not found.", id);
    }
    Ok(())
}

fn cmd_pending(security_cfg: &std::path::Path) -> Result<()> {
    let pending_path = security_cfg.parent().unwrap()
        .parent().unwrap()
        .join("workspace").join("security").join("pending.json");

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
        .unwrap_or_else(|_| if cfg!(target_os = "windows") { "notepad".to_string() } else { "vi".to_string() });

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

pub fn run(action: SecurityAction, local: bool) -> Result<()> {
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
                cfg.get("security").and_then(|s| s.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true)
            } else {
                true
            };

            println!("  Security module: {}", if enabled { "enabled" } else { "disabled" });

            // Show policy settings from security config
            let rules_cfg = read_rules_config(&security_cfg)?;
            let default_action = rules_cfg.get("default_action")
                .and_then(|v| v.as_str())
                .unwrap_or("allow")
                .to_uppercase();
            let log_ops = rules_cfg.get("log_all_operations")
                .and_then(|v| v.as_bool())
                .map(|v| if v { "yes" } else { "no" })
                .unwrap_or("no");
            let file_log = rules_cfg.get("audit_log_file_enabled")
                .and_then(|v| v.as_bool())
                .map(|v| if v { "yes" } else { "no" })
                .unwrap_or("no");
            let approval_timeout = rules_cfg.get("approval_timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let audit_retention = rules_cfg.get("audit_retention_days")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            println!("  Default action: {}", default_action);
            println!("  Log operations: {}", log_ops);
            println!("  File log: {}", file_log);
            if approval_timeout > 0 {
                println!("  Approval timeout: {}s", approval_timeout);
            }
            if audit_retention > 0 {
                println!("  Audit retention: {} days", audit_retention);
            }

            // Show policy settings from main config
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let cfg: serde_json::Value = serde_json::from_str(&data)?;
                let restrict = cfg.get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get("restrict_to_workspace"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                println!("  Workspace restricted: {}", restrict);
            }

            // Show scanner status
            if security_cfg.exists() {
                if let Ok(data) = std::fs::read_to_string(&security_cfg) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        if let Some(engines) = cfg.get("enabled").and_then(|v| v.as_array()) {
                            println!("  Scanner engines: {} configured", engines.len());
                        }
                        let restrict = cfg.get("restrict_to_workspace").and_then(|v| v.as_bool()).unwrap_or(true);
                        println!("  Restrict to workspace: {}", restrict);
                    }
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
                            let count = arr.iter()
                                .filter(|e| e.get("operation").and_then(|v| v.as_str()) == Some(*op))
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
                        if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                            defaults.insert("restrict_to_workspace".to_string(), serde_json::Value::Bool(false));
                        } else {
                            agents.insert("defaults".to_string(), serde_json::json!({"restrict_to_workspace": false}));
                        }
                    }
                    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
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
                        obj.insert("security".to_string(), serde_json::json!({"enabled": false}));
                    }
                    // Set RestrictToWorkspace = true when security is disabled
                    if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                        if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                            defaults.insert("restrict_to_workspace".to_string(), serde_json::Value::Bool(true));
                        } else {
                            agents.insert("defaults".to_string(), serde_json::json!({"restrict_to_workspace": true}));
                        }
                    }
                    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                }
            }
            println!("🔓 Security module disabled");
            println!("  Workspace restriction: enabled (all operations restricted to workspace)");
            println!();
            println!("  Restart agent/gateway to apply changes");
        }
        SecurityAction::Config { action } => {
            match action {
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
            }
        }
        SecurityAction::Audit { action } => {
            match action {
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
                                    println!("  [{}] {} / {} -> {} ({})",
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
                        let entries: Vec<serde_json::Value> = data.lines()
                            .filter(|l| !l.trim().is_empty())
                            .filter_map(|l| serde_json::from_str(l).ok())
                            .collect();
                        let export = serde_json::json!({
                            "exported_at": chrono::Utc::now().to_rfc3339(),
                            "total_entries": entries.len(),
                            "entries": entries,
                        });
                        std::fs::write(&output, serde_json::to_string_pretty(&export).unwrap_or_default())?;
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
                            let denied: Vec<_> = data.lines()
                                .filter(|l| !l.trim().is_empty())
                                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                                .filter(|e| e.get("decision").and_then(|v| v.as_str()) == Some("denied"))
                                .collect();
                            if denied.is_empty() {
                                println!("  No denied operations found.");
                            } else {
                                for evt in denied.iter().rev().take(50) {
                                    println!("  [{}] {} / {} ({})",
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
            }
        }
        SecurityAction::Scanner { action } => {
            // Delegate to the standalone scanner module
            super::scanner::run(action, local)?;
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
                        if let Some(e) = err {
                            println!("  Reason: {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("  Error: Invalid JSON args: {}", e);
                }
            }
        }
        SecurityAction::Rules { action } => {
            match action {
                RulesAction::List { rule_type } => {
                    cmd_rules_list(&security_cfg, rule_type.as_deref())?
                }
                RulesAction::Add { rule_type, operation, pattern, action } => {
                    cmd_rules_add(&security_cfg, &rule_type, &operation, pattern.as_deref(), action.as_deref())?
                }
                RulesAction::Remove { rule_type, operation, index } => {
                    cmd_rules_remove(&security_cfg, &rule_type, &operation, index)?
                }
                RulesAction::Test { rule_type, operation, target } => {
                    cmd_rules_test(&security_cfg, &rule_type, &operation, &target)?
                }
            }
        }
        SecurityAction::Approve { id } => cmd_approve(&security_cfg, &id)?,
        SecurityAction::Deny { id, reason } => {
            let reason_str = if reason.is_empty() { None } else { Some(reason.join(" ")) };
            cmd_deny(&security_cfg, &id, reason_str.as_deref())?;
        }
        SecurityAction::Pending => cmd_pending(&security_cfg)?,
        SecurityAction::Edit => cmd_edit(&security_cfg)?,
        SecurityAction::ConfigReset => cmd_config_reset(&security_cfg)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_pattern_exact() {
        assert!(match_pattern("test.exe", "test.exe"));
        assert!(!match_pattern("test.exe", "other.exe"));
    }

    #[test]
    fn test_match_pattern_single_star() {
        assert!(match_pattern("*.exe", "test.exe"));
        assert!(!match_pattern("*.exe", "dir/test.exe"));
        assert!(match_pattern("test*", "testFile"));
    }

    #[test]
    fn test_match_pattern_double_star() {
        assert!(match_pattern("**/*.exe", "test.exe"));
        assert!(match_pattern("**/*.exe", "dir/test.exe"));
        assert!(match_pattern("**/*.exe", "a/b/c/test.exe"));
        assert!(match_pattern("**", "anything"));
    }

    #[test]
    fn test_match_pattern_mixed() {
        assert!(match_pattern("dir/*.log", "dir/test.log"));
        assert!(!match_pattern("dir/*.log", "dir/sub/test.log"));
        assert!(match_pattern("dir/**/*.log", "dir/sub/test.log"));
    }
}
