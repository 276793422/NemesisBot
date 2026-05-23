//! Skill linter - checks skill content for dangerous patterns.
//!
//! Performs line-by-line scanning with pattern IDs, severity levels, line
//! tracking, and matched text capture. Includes Windows PowerShell-specific
//! patterns alongside Unix patterns.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Category of a lint warning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LintCategory {
    /// Destructive operations (rm -rf, format, drop, etc.).
    Destructive,
    /// Data exfiltration (curl to external, wget, scp, etc.).
    Exfiltration,
    /// Privilege escalation (sudo, su, chmod 777, etc.).
    Privilege,
    /// Obfuscation techniques (base64 decode, eval, hidden files, etc.).
    Obfuscation,
    /// Reconnaissance (nmap, whoami, /etc/passwd, env vars, etc.).
    Recon,
}

impl std::fmt::Display for LintCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintCategory::Destructive => write!(f, "destructive"),
            LintCategory::Exfiltration => write!(f, "exfiltration"),
            LintCategory::Privilege => write!(f, "privilege"),
            LintCategory::Obfuscation => write!(f, "obfuscation"),
            LintCategory::Recon => write!(f, "recon"),
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
    /// Returns 27 patterns across 5 categories, matching the Go implementation:
    /// - Destructive (DEST-001..DEST-006): file deletion, disk wipe, shutdown, etc.
    /// - Exfiltration (EXFL-001..EXFL-006): upload, base64 exfil, DNS tunnel, etc.
    /// - Privilege (PRIV-001..PRIV-005): sudo, permission change, user creation, etc.
    /// - Obfuscation (OBFS-001..OBFS-005): base64 decode exec, eval, compressed payload, etc.
    /// - Recon (RECN-001..RECN-005): network scan, process list, file search, etc.
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

        raw.into_iter()
            .filter_map(|(category, pat, description, id, severity)| {
                Regex::new(pat).ok().map(|regex| PatternEntry {
                    category,
                    regex,
                    description: description.to_string(),
                    id: id.to_string(),
                    severity,
                })
            })
            .collect()
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
        warnings.iter().any(|w| {
            w.severity == LintSeverity::Critical || w.severity == LintSeverity::High
        })
    }

    /// Calculate safety score based on warnings.
    ///
    /// Score calculation:
    /// - Start at 1.0 (perfectly safe)
    /// - Destructive: -0.20 each
    /// - Exfiltration: -0.15 each
    /// - Privilege: -0.12 each
    /// - Obfuscation: -0.10 each
    /// - Recon: -0.05 each
    ///
    /// Score is clamped to [0.0, 1.0].
    fn calculate_score(warnings: &[LintWarning]) -> f64 {
        let penalty: f64 = warnings
            .iter()
            .map(|w| match w.category {
                LintCategory::Destructive => 0.20,
                LintCategory::Exfiltration => 0.15,
                LintCategory::Privilege => 0.12,
                LintCategory::Obfuscation => 0.10,
                LintCategory::Recon => 0.05,
            })
            .sum();

        (1.0 - penalty).max(0.0).min(1.0)
    }
}

impl Default for SkillLinter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
