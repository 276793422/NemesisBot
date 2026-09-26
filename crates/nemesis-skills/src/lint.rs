//! Skill linter - checks skill content for dangerous patterns.
//!
//! Performs line-by-line scanning with pattern IDs, severity levels, line
//! tracking, and matched text capture. Includes Windows PowerShell-specific
//! patterns alongside Unix patterns.

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Category of a lint warning.
///
/// 12 categories aligned with legacy scanner (P15 供应链扩面)：
/// 既有 5 类语义并入（Destructive=危险执行 / Exfiltration=数据外传 /
/// Privilege=提权 / Obfuscation=混淆编码 / Recon=网络扫描），新增 7 类
/// 规则来自嵌入 JSON 规则表（`security_rules.json`，便于热更）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LintCategory {
    /// Dangerous execution (rm -rf, format, shutdown, etc.) - 危险执行.
    Destructive,
    /// Data exfiltration (curl to external, wget, scp, etc.) - 数据外传.
    Exfiltration,
    /// Privilege escalation (sudo, su, chmod 777, etc.) - 提权.
    Privilege,
    /// Obfuscation techniques (base64 decode, eval, hidden files, etc.) - 混淆编码.
    Obfuscation,
    /// Reconnaissance / network scanning (nmap, whoami, env vars, etc.) - 网络扫描.
    Recon,
    /// Credential theft (SSH keys, cloud credentials, keychain dump) - 凭证窃取.
    CredentialTheft,
    /// Persistence mechanisms (cron, startup, service install) - 持久化.
    Persistence,
    /// Download-then-execute chains (curl | sh, certutil urlcache) - 下载执行链.
    DownloadExecuteChain,
    /// Sensitive path access (sudoers, SAM, browser cookies) - 敏感路径触达.
    SensitivePathAccess,
    /// Environment variable probing of secrets (env sniffing) - 环境变量嗅探.
    EnvironmentProbing,
    /// Dynamically constructed execution (echo|sh, new Function, python -c) - 动态构造执行.
    DynamicConstructionExec,
    /// Supply-chain traces (install scripts, git config hijack) - 供应链痕迹.
    SupplyChainTrace,
}

impl LintCategory {
    /// Parse a category from its snake_case JSON name (rule-table loader).
    ///
    /// Unknown names return `None` so bad rule entries can be skipped loudly.
    pub fn from_rule_name(name: &str) -> Option<Self> {
        Some(match name {
            "destructive" | "dangerous_execution" => Self::Destructive,
            "exfiltration" | "data_exfiltration" => Self::Exfiltration,
            "privilege" | "privilege_escalation" => Self::Privilege,
            "obfuscation" => Self::Obfuscation,
            "recon" | "network_scanning" => Self::Recon,
            "credential_theft" => Self::CredentialTheft,
            "persistence" => Self::Persistence,
            "download_execute_chain" => Self::DownloadExecuteChain,
            "sensitive_path_access" => Self::SensitivePathAccess,
            "environment_probing" => Self::EnvironmentProbing,
            "dynamic_construction_exec" => Self::DynamicConstructionExec,
            "supply_chain_trace" => Self::SupplyChainTrace,
            _ => return None,
        })
    }

    /// Per-warning score penalty weight for this category.
    pub fn score_weight(&self) -> f64 {
        match self {
            Self::Destructive => 0.20,
            Self::Exfiltration => 0.15,
            Self::Privilege => 0.12,
            Self::Obfuscation => 0.10,
            Self::Recon => 0.05,
            Self::CredentialTheft => 0.20,
            Self::Persistence => 0.15,
            Self::DownloadExecuteChain => 0.15,
            Self::SensitivePathAccess => 0.10,
            Self::EnvironmentProbing => 0.08,
            Self::DynamicConstructionExec => 0.10,
            Self::SupplyChainTrace => 0.08,
        }
    }
}

impl std::fmt::Display for LintCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintCategory::Destructive => write!(f, "destructive"),
            LintCategory::Exfiltration => write!(f, "exfiltration"),
            LintCategory::Privilege => write!(f, "privilege"),
            LintCategory::Obfuscation => write!(f, "obfuscation"),
            LintCategory::Recon => write!(f, "recon"),
            LintCategory::CredentialTheft => write!(f, "credential-theft"),
            LintCategory::Persistence => write!(f, "persistence"),
            LintCategory::DownloadExecuteChain => write!(f, "download-execute-chain"),
            LintCategory::SensitivePathAccess => write!(f, "sensitive-path-access"),
            LintCategory::EnvironmentProbing => write!(f, "environment-probing"),
            LintCategory::DynamicConstructionExec => write!(f, "dynamic-construction-exec"),
            LintCategory::SupplyChainTrace => write!(f, "supply-chain-trace"),
        }
    }
}

/// Severity level of a lint warning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LintSeverity {
    /// Critical: immediate blocking recommended.
    Critical,
    /// High: strong concern, should block by default.
    High,
    /// Medium: moderate concern, review recommended.
    Medium,
    /// Low: minor concern, informational.
    Low,
}

impl std::fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintSeverity::Critical => write!(f, "critical"),
            LintSeverity::High => write!(f, "high"),
            LintSeverity::Medium => write!(f, "medium"),
            LintSeverity::Low => write!(f, "low"),
        }
    }
}

/// A single lint warning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintWarning {
    /// Category of the warning.
    pub category: LintCategory,
    /// Human-readable description of the detected pattern.
    pub message: String,
    /// The regex pattern that was matched.
    pub pattern: String,
    /// Unique pattern identifier (e.g., "DEST-001").
    pub pattern_id: String,
    /// 1-based line number where the pattern was found (None if whole-content match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The actual text that was matched.
    pub matched_text: String,
    /// Severity level of the warning.
    #[serde(default = "default_severity")]
    pub severity: LintSeverity,
}

fn default_severity() -> LintSeverity {
    LintSeverity::Medium
}

/// Result of linting a skill's content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    /// Name of the skill that was linted (may be empty).
    #[serde(default)]
    pub skill_name: String,
    /// Whether the skill passed the lint check (score >= 0.6 and no critical/high warnings).
    #[serde(default)]
    pub passed: bool,
    /// Overall safety score (0.0-1.0, where 1.0 is safest).
    pub score: f64,
    /// All warnings found during linting.
    pub warnings: Vec<LintWarning>,
}

impl LintResult {
    /// Count warnings per category (P15 分类计数，供审批卡摘要与评分展示).
    pub fn category_counts(&self) -> std::collections::BTreeMap<LintCategory, usize> {
        let mut counts = std::collections::BTreeMap::new();
        for w in &self.warnings {
            *counts.entry(w.category.clone()).or_insert(0) += 1;
        }
        counts
    }
}

/// Internal representation of a compiled pattern with metadata.
struct PatternEntry {
    category: LintCategory,
    regex: Regex,
    description: String,
    id: String,
    severity: LintSeverity,
}

/// Linter that checks skill content for dangerous patterns.
pub struct SkillLinter {
    patterns: Vec<PatternEntry>,
}

impl SkillLinter {
    /// Create a new linter with all built-in dangerous patterns.
    pub fn new() -> Self {
        let patterns = Self::build_patterns();
        Self { patterns }
    }

    /// Build the complete list of dangerous patterns.
    ///
    /// 27 hardcoded legacy patterns across 5 categories (matching the Go
    /// implementation), plus the embedded JSON rule table
    /// (`security_rules.json`) covering the 7 extended categories:
    /// - Destructive (DEST-001..DEST-006): file deletion, disk wipe, shutdown, etc.
    /// - Exfiltration (EXFL-001..EXFL-006): upload, base64 exfil, DNS tunnel, etc.
    /// - Privilege (PRIV-001..PRIV-005): sudo, permission change, user creation, etc.
    /// - Obfuscation (OBFS-001..OBFS-005): base64 decode exec, eval, compressed payload, etc.
    /// - Recon (RECN-001..RECN-005): network scan, process list, file search, etc.
    /// - CredentialTheft (CRED-xxx) / Persistence (PERS-xxx) /
    ///   DownloadExecuteChain (DNXL-xxx) / SensitivePathAccess (SNST-xxx) /
    ///   EnvironmentProbing (ENVP-xxx) / DynamicConstructionExec (DYNE-xxx) /
    ///   SupplyChainTrace (SUPC-xxx): from the embedded JSON table.
    fn build_patterns() -> Vec<PatternEntry> {
        let raw: Vec<(LintCategory, &str, &str, &str, LintSeverity)> = vec![
            // ---- Destructive (6) ----
            (
                LintCategory::Destructive,
                r"(?i)rm\s+-rf\s+/|Remove-Item.*-Recurse.*-Force",
                "Recursive/forced file deletion detected",
                "DEST-001",
                LintSeverity::Critical,
            ),
            (
                LintCategory::Destructive,
                r"(?i)dd\s+if=|format\s+[A-Za-z]:|mkfs\.",
                "Disk wipe or format command detected",
                "DEST-002",
                LintSeverity::Critical,
            ),
            (
                LintCategory::Destructive,
                r"(?i)(?:^|\W)shutdown(?:\s|$)|(?:^|\W)halt(?:\s|$)|(?:^|\W)poweroff(?:\s|$)|Stop-Computer|Restart-Computer",
                "System shutdown or power-off command detected",
                "DEST-003",
                LintSeverity::Critical,
            ),
            (
                LintCategory::Destructive,
                r"(?i)kill\s+-9.*1|taskkill.*//F.*//IM",
                "Force kill all processes detected",
                "DEST-004",
                LintSeverity::High,
            ),
            (
                LintCategory::Destructive,
                r"(?i)reg\s+delete.*//f|Remove-Item.*HKLM:",
                "Registry deletion command detected",
                "DEST-005",
                LintSeverity::Critical,
            ),
            (
                LintCategory::Destructive,
                r"(?i)sc\s+delete|net\s+stop",
                "Service deletion or stop command detected",
                "DEST-006",
                LintSeverity::High,
            ),
            // ---- Exfiltration (6) ----
            (
                LintCategory::Exfiltration,
                r"(?i)curl.*--upload|Invoke-WebRequest.*-Method\s+PUT|scp.*@",
                "Network file upload detected",
                "EXFL-001",
                LintSeverity::High,
            ),
            (
                LintCategory::Exfiltration,
                r"(?i)base64.*\||Out-File.*-Encoding.*Base64|xxd.*-p",
                "Base64 encoding to pipe/file detected",
                "EXFL-002",
                LintSeverity::Medium,
            ),
            (
                LintCategory::Exfiltration,
                r"(?i)nslookup.*\|",
                "DNS exfiltration via pipe detected",
                "EXFL-003",
                LintSeverity::High,
            ),
            (
                LintCategory::Exfiltration,
                r"(?i)cat\s+/etc/passwd|cat\s+/etc/shadow|Get-Credential|net\s+user",
                "Credential or password file access detected",
                "EXFL-004",
                LintSeverity::Critical,
            ),
            (
                LintCategory::Exfiltration,
                r"(?i)(?:^|\W)env(?:\s|$)|(?:^|\W)printenv(?:\s|$)|Get-ChildItem\s+env:|set\s+>",
                "Environment variable dump detected",
                "EXFL-005",
                LintSeverity::High,
            ),
            (
                LintCategory::Exfiltration,
                r"(?i)keylog|Get-Keystroke|Register-Keys",
                "Keylogger or keystroke capture detected",
                "EXFL-006",
                LintSeverity::Critical,
            ),
            // ---- Privilege (5) ----
            (
                LintCategory::Privilege,
                r"(?i)sudo\s+su|sudo\s+-i|runas\s+/user:admin",
                "Privilege escalation via sudo or runas detected",
                "PRIV-001",
                LintSeverity::High,
            ),
            (
                LintCategory::Privilege,
                r"(?i)chmod\s+777|chmod\s+u\+s|icacls.*grant.*:F",
                "Dangerous permission change detected",
                "PRIV-002",
                LintSeverity::High,
            ),
            (
                LintCategory::Privilege,
                r"(?i)useradd|net\s+user\s+.*/add|New-LocalUser",
                "User creation command detected",
                "PRIV-003",
                LintSeverity::High,
            ),
            (
                LintCategory::Privilege,
                r"(?i)find.*-perm\s+-4000|find.*-perm\s+-2000",
                "SUID/SGID binary search detected",
                "PRIV-004",
                LintSeverity::Medium,
            ),
            (
                LintCategory::Privilege,
                r"(?i)setcap|getcap",
                "Linux capabilities manipulation detected",
                "PRIV-005",
                LintSeverity::Medium,
            ),
            // ---- Obfuscation (5) ----
            (
                LintCategory::Obfuscation,
                r"(?i)FromBase64String|base64\s+-d|xxd\s+-r",
                "Base64 decoding for execution detected",
                "OBFS-001",
                LintSeverity::High,
            ),
            (
                LintCategory::Obfuscation,
                r"(?i)iex\s*\(|Invoke-Expression|eval\s*\(",
                "Dynamic code execution via eval/iex detected",
                "OBFS-002",
                LintSeverity::High,
            ),
            (
                LintCategory::Obfuscation,
                r"(?i)Decompress|gunzip|Expand-Archive.*-Force",
                "Decompression of compressed payload detected",
                "OBFS-003",
                LintSeverity::Medium,
            ),
            (
                LintCategory::Obfuscation,
                r"(?i)-WindowStyle\s+Hidden|-EncodedCommand|/c\s+start",
                "Hidden or encoded command execution detected",
                "OBFS-004",
                LintSeverity::High,
            ),
            (
                LintCategory::Obfuscation,
                r"(?i)/tmp/|Temp\\|AppData\\.*\\.*\.exe",
                "Execution from temporary directory detected",
                "OBFS-005",
                LintSeverity::Medium,
            ),
            // ---- Recon (5) ----
            (
                LintCategory::Recon,
                r"(?i)(?:^|\W)nmap(?:\s|$)|netstat\s+-an|Get-NetTCPConnection",
                "Network scanning tool detected",
                "RECN-001",
                LintSeverity::High,
            ),
            (
                LintCategory::Recon,
                r"(?i)ps\s+aux|tasklist|Get-Process.*-",
                "Process enumeration detected",
                "RECN-002",
                LintSeverity::Medium,
            ),
            (
                LintCategory::Recon,
                r"(?i)find\s+/|-Recurse.*-Filter|Get-ChildItem.*-Recurse",
                "Recursive file system search detected",
                "RECN-003",
                LintSeverity::Medium,
            ),
            (
                LintCategory::Recon,
                r"(?i)uname\s+-a|systeminfo|Get-ComputerInfo",
                "System information gathering detected",
                "RECN-004",
                LintSeverity::Low,
            ),
            (
                LintCategory::Recon,
                r"(?i)lsof\s+-i|netstat\s+-tlnp|Get-NetTCPConnection.*State\s+Listen",
                "Listening port enumeration detected",
                "RECN-005",
                LintSeverity::Medium,
            ),
        ];

        let mut patterns: Vec<PatternEntry> = raw
            .into_iter()
            .filter_map(|(category, pat, description, id, severity)| {
                Regex::new(pat).ok().map(|regex| PatternEntry {
                    category,
                    regex,
                    description: description.to_string(),
                    id: id.to_string(),
                    severity,
                })
            })
            .collect();

        // P15: append the embedded JSON rule table (7 extended categories).
        // Invalid regexes / unknown categories are skipped with a warn so a
        // bad rule entry can never break installation entirely.
        for rule in crate::security_rules::embedded_rule_file().rules() {
            let category = match LintCategory::from_rule_name(&rule.category) {
                Some(c) => c,
                None => {
                    warn!(
                        "lint rule {}: unknown category '{}'",
                        rule.id, rule.category
                    );
                    continue;
                }
            };
            let regex = match Regex::new(&rule.pattern) {
                Ok(r) => r,
                Err(e) => {
                    warn!("lint rule {}: invalid regex: {}", rule.id, e);
                    continue;
                }
            };
            patterns.push(PatternEntry {
                category,
                regex,
                description: rule.message.clone(),
                id: rule.id.clone(),
                severity: rule.severity(),
            });
        }

        patterns
    }

    /// Lint the given content and return a result with score and warnings.
    ///
    /// Scans line by line, recording the line number and matched text for
    /// each pattern match. A single pattern can produce multiple warnings
    /// if it matches on different lines.
    pub fn lint(&self, content: &str) -> LintResult {
        self.lint_with_name(content, "")
    }

    /// Lint the given content with an optional skill name.
    ///
    /// Mirrors Go's `Linter.Lint(content, skillName)` signature.
    pub fn lint_with_name(&self, content: &str, skill_name: &str) -> LintResult {
        let mut warnings = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        for (line_idx, line) in lines.iter().enumerate() {
            let line_num = line_idx + 1; // 1-based

            for entry in &self.patterns {
                for mat in entry.regex.find_iter(line) {
                    warnings.push(LintWarning {
                        category: entry.category.clone(),
                        message: entry.description.clone(),
                        pattern: entry.regex.to_string(),
                        pattern_id: entry.id.clone(),
                        line: Some(line_num),
                        matched_text: mat.as_str().to_string(),
                        severity: entry.severity.clone(),
                    });
                }
            }
        }

        let score = Self::calculate_score(&warnings);
        let passed = score >= 0.6 && !Self::has_critical_or_high(&warnings);

        LintResult {
            skill_name: skill_name.to_string(),
            passed,
            score,
            warnings,
        }
    }

    /// Check if any warning has critical or high severity.
    ///
    /// Mirrors Go's `hasCriticalOrHigh(issues)`.
    pub fn has_critical_or_high(warnings: &[LintWarning]) -> bool {
        warnings
            .iter()
            .any(|w| w.severity == LintSeverity::Critical || w.severity == LintSeverity::High)
    }

    /// Calculate safety score based on warnings.
    ///
    /// Score calculation: start at 1.0, subtract each warning's per-category
    /// weight (see `LintCategory::score_weight`). Weights are flat per-warning
    /// (no cap): 9 recon warnings cost 0.45, matching the LINT_FAIL fixture
    /// expectation (score 0.55). Clamped to [0.0, 1.0].
    fn calculate_score(warnings: &[LintWarning]) -> f64 {
        let penalty: f64 = warnings.iter().map(|w| w.category.score_weight()).sum();

        // penalty is a sum of finite constants (never NaN), so clamp is equivalent.
        (1.0 - penalty).clamp(0.0, 1.0)
    }
}

impl Default for SkillLinter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
