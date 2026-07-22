//! Command Guard - Layer 2
//! Blocks dangerous commands based on 45+ blocklist entries with metadata.

use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Command guard error.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("dangerous command blocked: {0}")]
    Blocked(String),
}

/// Severity of a blocklist entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/// Platform target for a blocklist entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    All,
    Linux,
    Windows,
    MacOS,
}

/// Category of dangerous command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandCategory {
    Destructive,
    Network,
    Privilege,
    Recon,
    Obfuscation,
    Persistence,
    Exfiltration,
}

/// Metadata for a blocklist entry.
#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub name: &'static str,
    pub category: CommandCategory,
    pub severity: Severity,
    pub platform: Platform,
    pub reason: &'static str,
}

/// Guard configuration.
#[derive(Debug, Clone)]
pub struct GuardConfig {
    pub enabled: bool,
    pub strict_mode: bool,
    pub extra_patterns: Vec<String>,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strict_mode: false,
            extra_patterns: vec![],
        }
    }
}

/// Command guard with dynamic entries.
pub struct Guard {
    config: GuardConfig,
    extra_entries: RwLock<Vec<(String, Regex)>>,
}

impl Guard {
    pub fn new(enabled: bool) -> Self {
        Self {
            config: GuardConfig {
                enabled,
                ..Default::default()
            },
            extra_entries: RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: GuardConfig) -> Self {
        let mut extra = Vec::new();
        for pattern in &config.extra_patterns {
            if let Ok(re) = Regex::new(pattern) {
                extra.push((pattern.clone(), re));
            }
        }
        Self {
            config,
            extra_entries: RwLock::new(extra),
        }
    }

    /// Check if a command is safe.
    pub fn check(&self, command: &str) -> Result<(), GuardError> {
        if !self.config.enabled {
            return Ok(());
        }

        let lower = command.to_lowercase();

        // Check static blocklist
        let patterns = get_blocklist();
        for (name, re) in patterns {
            if re.is_match(&lower) {
                return Err(GuardError::Blocked(format!(
                    "matches dangerous pattern: {}",
                    name
                )));
            }
        }

        // Check dynamic entries
        let extra = self.extra_entries.read();
        for (name, re) in extra.iter() {
            if re.is_match(&lower) {
                return Err(GuardError::Blocked(format!(
                    "matches dynamic pattern: {}",
                    name
                )));
            }
        }

        // Strict mode: check for partial matches
        if self.config.strict_mode {
            let strict_keywords = get_strict_keywords();
            for keyword in strict_keywords {
                if lower.contains(keyword) {
                    return Err(GuardError::Blocked(format!(
                        "strict mode: contains keyword '{}'",
                        keyword
                    )));
                }
            }
        }

        Ok(())
    }

    /// Add a dynamic entry.
    pub fn add_entry(&self, name: &str, pattern: &str) -> Result<(), String> {
        let re = Regex::new(pattern).map_err(|e| format!("invalid pattern: {}", e))?;
        self.extra_entries.write().push((name.to_string(), re));
        Ok(())
    }

    /// Remove a dynamic entry by name.
    pub fn remove_entry(&self, name: &str) -> bool {
        let mut entries = self.extra_entries.write();
        let before = entries.len();
        entries.retain(|(n, _)| n != name);
        entries.len() < before
    }

    /// Get all blocklist entries with metadata.
    pub fn list_entries(&self) -> Vec<BlockEntry> {
        get_blocklist_metadata().to_vec()
    }

    /// Get the category for a blocklist entry by name.
    pub fn get_category(name: &str) -> Option<CommandCategory> {
        get_blocklist_metadata()
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.category)
    }

    /// Get a specific blocklist entry by name.
    pub fn get_blocked_entry(name: &str) -> Option<&'static BlockEntry> {
        get_blocklist_metadata().iter().find(|e| e.name == name)
    }

    /// Simplify/normalize a command for better pattern matching.
    ///
    /// Strips extra whitespace, normalizes quotes, removes comments.
    pub fn simplify_command(command: &str) -> String {
        let mut result = command
            .trim()
            .replace('\t', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // Normalize quotes
        result = result.replace('"', "'").replace("''", "'");
        // Remove trailing comments
        if let Some(pos) = result.find(" #") {
            result = result[..pos].to_string();
        }
        if let Some(pos) = result.find(" //") {
            result = result[..pos].to_string();
        }
        result.to_lowercase()
    }

    /// Update the guard configuration at runtime.
    ///
    /// Equivalent to Go's `Guard.SetConfig()`. Compiles any new extra patterns
    /// from the config's `extra_patterns` field.
    pub fn set_config(&self, config: GuardConfig) -> Result<(), String> {
        // Compile new extra patterns
        let mut extra = Vec::new();
        for pattern in &config.extra_patterns {
            let re =
                Regex::new(pattern).map_err(|e| format!("invalid pattern {}: {}", pattern, e))?;
            extra.push((pattern.clone(), re));
        }
        *self.extra_entries.write() = extra;
        // Note: we can't mutate self.config because it's not behind a lock.
        // The enabled/strict_mode fields are read from self.config directly.
        // For full dynamic mutation, the caller should create a new Guard.
        Ok(())
    }

    /// Returns `true` if the command matches a blocked pattern.
    ///
    /// Equivalent to Go's `Guard.IsBlocked()`. Convenience wrapper around
    /// `check()` for boolean results.
    pub fn is_blocked(&self, command: &str) -> bool {
        self.check(command).is_err()
    }
}

fn get_strict_keywords() -> &'static [&'static str] {
    static KEYWORDS: &[&str] = &[
        "rm -",
        "del /",
        "format ",
        "mkfs.",
        "dd if=",
        "shutdown",
        "reboot",
        "poweroff",
        "halt",
        "sudo ",
        "su -",
        "runas ",
        "curl |",
        "wget |",
        "cmd /c",
        "powershell -enc",
        "eval ",
        "exec ",
    ];
    KEYWORDS
}

type BlockList = Vec<(&'static str, Regex)>;

fn get_blocklist() -> &'static BlockList {
    static LIST: OnceLock<BlockList> = OnceLock::new();
    LIST.get_or_init(|| {
        let raw: Vec<(&str, &str)> = vec![
            // Destructive (10)
            ("rm_rf", r"(?i)\brm\s+-[rf]{1,2}\b"),
            ("del_force", r"(?i)\bdel\s+/[fq]\b"),
            ("format", r"(?i)\b(format|mkfs)\b"),
            ("dd", r"(?i)\bdd\s+if="),
            ("shutdown", r"(?i)\b(shutdown|reboot|poweroff|halt)\b"),
            ("wipefs", r"(?i)\bwipefs\b"),
            ("shred", r"(?i)\bshred\b"),
            ("truncate", r"(?i)\btruncate\s+-s\s+0\b"),
            ("srm", r"(?i)\bsrm\b"),
            ("catastrophic_pipe", r"(?i):\(\)\{[^}]*;\}\s*;"),
            // Privilege escalation (8)
            ("sudo", r"(?i)\bsudo\b"),
            ("chmod", r"(?i)\bchmod\s+[0-7]{3,4}\b"),
            ("chown", r"(?i)\bchown\b"),
            ("runas", r"(?i)\brunas\b"),
            ("su_switch", r"(?i)\bsu\s+[-\w]*\b"),
            ("pkexec", r"(?i)\bpkexec\b"),
            ("doas", r"(?i)\bdoas\b"),
            ("gosu", r"(?i)\bgosu\b"),
            // Process killing (4)
            ("pkill", r"(?i)\bpkill\b"),
            ("killall", r"(?i)\bkillall\b"),
            ("kill_9", r"(?i)\bkill\s+-9\b"),
            ("taskkill", r"(?i)\btaskkill\b"),
            // Network recon (4)
            ("nmap", r"(?i)\bnmap\b"),
            ("netcat_bind", r"(?i)\bnc\s+.*-\b[l]\b"),
            ("tcpdump", r"(?i)\btcpdump\b"),
            ("wireshark", r"(?i)\b(tshark|wireshark)\b"),
            // Remote execution (4)
            ("curl_pipe_sh", r"(?i)\bcurl\b.*\|\s*(sh|bash)"),
            ("wget_pipe_sh", r"(?i)\bwget\b.*\|\s*(sh|bash)"),
            ("eval", r"(?i)\beval\b"),
            ("socat_shell", r"(?i)\bsocat\b.*exec"),
            // Windows specific (6)
            ("powershell_encoded", r"(?i)powershell.*-enc"),
            ("cmd_c", r"(?i)\bcmd\s+/c\b"),
            ("reg_delete", r"(?i)\breg\s+(delete|add)\b"),
            ("net_user", r"(?i)\bnet\s+(user|localgroup)\b"),
            ("wmic", r"(?i)\bwmic\b"),
            ("bitsadmin", r"(?i)\bbitsadmin\b"),
            // Package manipulation (4)
            ("apt_remove", r"(?i)\bapt(-get)?\s+(remove|purge)\b"),
            ("yum_remove", r"(?i)\byum\s+remove\b"),
            ("dnf_remove", r"(?i)\bdnf\s+remove\b"),
            ("pip_uninstall", r"(?i)\bpip\s+uninstall\b"),
            // Disk/filesystem (3)
            ("mount_remount", r"(?i)\bmount\s+-o\s+.*\bremount\b"),
            ("fdisk", r"(?i)\bfdisk\b"),
            ("parted", r"(?i)\bparted\b"),
            // Obfuscation (4)
            ("base64_pipe", r"(?i)base64\s+-d\s*\|\s*(sh|bash)"),
            ("xxd_reverse", r"(?i)xxd\s+-r\s*\|"),
            (
                "hex_decode_exec",
                r"(?i)echo\s+\\x[0-9a-f]+\s*\|\s*(sh|bash)",
            ),
            (
                "python_eval",
                r"(?i)python[23]?\s+-c\s+(import|exec|eval|os\.system)",
            ),
        ];

        raw.into_iter()
            .filter_map(|(name, pattern)| Regex::new(pattern).ok().map(|re| (name, re)))
            .collect()
    })
}

fn get_blocklist_metadata() -> &'static Vec<BlockEntry> {
    static ENTRIES: OnceLock<Vec<BlockEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        vec![
            BlockEntry {
                name: "rm_rf",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Recursive force delete",
            },
            BlockEntry {
                name: "del_force",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Force delete files",
            },
            BlockEntry {
                name: "format",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::All,
                reason: "Disk format",
            },
            BlockEntry {
                name: "dd",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Disk dump/overwrite",
            },
            BlockEntry {
                name: "shutdown",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::All,
                reason: "System shutdown",
            },
            BlockEntry {
                name: "wipefs",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Wipe filesystem signature",
            },
            BlockEntry {
                name: "shred",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Secure file deletion",
            },
            BlockEntry {
                name: "sudo",
                category: CommandCategory::Privilege,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Privilege escalation",
            },
            BlockEntry {
                name: "chmod",
                category: CommandCategory::Privilege,
                severity: Severity::Medium,
                platform: Platform::Linux,
                reason: "Permission change",
            },
            BlockEntry {
                name: "chown",
                category: CommandCategory::Privilege,
                severity: Severity::Medium,
                platform: Platform::Linux,
                reason: "Ownership change",
            },
            BlockEntry {
                name: "runas",
                category: CommandCategory::Privilege,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Windows privilege escalation",
            },
            BlockEntry {
                name: "pkexec",
                category: CommandCategory::Privilege,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "PolicyKit escalation",
            },
            BlockEntry {
                name: "pkill",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Process kill by pattern",
            },
            BlockEntry {
                name: "killall",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Kill all by name",
            },
            BlockEntry {
                name: "kill_9",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::All,
                reason: "Force kill",
            },
            BlockEntry {
                name: "taskkill",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Windows process kill",
            },
            BlockEntry {
                name: "nmap",
                category: CommandCategory::Recon,
                severity: Severity::Medium,
                platform: Platform::All,
                reason: "Network scanning",
            },
            BlockEntry {
                name: "netcat_bind",
                category: CommandCategory::Network,
                severity: Severity::High,
                platform: Platform::All,
                reason: "Netcat bind shell",
            },
            BlockEntry {
                name: "tcpdump",
                category: CommandCategory::Recon,
                severity: Severity::Medium,
                platform: Platform::Linux,
                reason: "Packet capture",
            },
            BlockEntry {
                name: "curl_pipe_sh",
                category: CommandCategory::Network,
                severity: Severity::Critical,
                platform: Platform::All,
                reason: "Remote code execution",
            },
            BlockEntry {
                name: "wget_pipe_sh",
                category: CommandCategory::Network,
                severity: Severity::Critical,
                platform: Platform::All,
                reason: "Remote code execution",
            },
            BlockEntry {
                name: "eval",
                category: CommandCategory::Obfuscation,
                severity: Severity::High,
                platform: Platform::All,
                reason: "Dynamic code execution",
            },
            BlockEntry {
                name: "socat_shell",
                category: CommandCategory::Network,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Reverse shell",
            },
            BlockEntry {
                name: "powershell_encoded",
                category: CommandCategory::Obfuscation,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Encoded PowerShell",
            },
            BlockEntry {
                name: "cmd_c",
                category: CommandCategory::Obfuscation,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "CMD execution",
            },
            BlockEntry {
                name: "reg_delete",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Registry modification",
            },
            BlockEntry {
                name: "net_user",
                category: CommandCategory::Privilege,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "User management",
            },
            BlockEntry {
                name: "wmic",
                category: CommandCategory::Recon,
                severity: Severity::Medium,
                platform: Platform::Windows,
                reason: "WMI command",
            },
            BlockEntry {
                name: "bitsadmin",
                category: CommandCategory::Network,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Background file transfer",
            },
            BlockEntry {
                name: "apt_remove",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Package removal",
            },
            BlockEntry {
                name: "yum_remove",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Package removal",
            },
            BlockEntry {
                name: "dnf_remove",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Package removal",
            },
            BlockEntry {
                name: "mount_remount",
                category: CommandCategory::Privilege,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Remount filesystem",
            },
            BlockEntry {
                name: "fdisk",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Disk partitioning",
            },
            BlockEntry {
                name: "parted",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Disk partitioning",
            },
            BlockEntry {
                name: "base64_pipe",
                category: CommandCategory::Obfuscation,
                severity: Severity::High,
                platform: Platform::All,
                reason: "Encoded command execution",
            },
            BlockEntry {
                name: "python_eval",
                category: CommandCategory::Obfuscation,
                severity: Severity::High,
                platform: Platform::All,
                reason: "Python code execution",
            },
        ]
    })
}

#[cfg(test)]
mod tests;
