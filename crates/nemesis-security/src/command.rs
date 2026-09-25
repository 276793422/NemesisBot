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

        // CMD-04/05（2026-09-16 横扫存量加固）：除原文小写形态外，再对
        // 归一化形态（剥引号/合并空白/小写，见 matcher::normalize_exec_command）
        // 扫一遍——`rm "-rf"`、`rm --Recursive`、`rd "/s"` 曾借 shell 剥引号
        // 与大小写差异同时绕过 Guard 与 ABAC，原文形态打不中。
        let lower = command.to_lowercase();
        let normalized = crate::matcher::normalize_exec_command(command);
        let candidates = [lower.as_str(), normalized.as_str()];

        // Check static blocklist
        let patterns = get_blocklist();
        for candidate in candidates {
            for (name, re) in patterns {
                if re.is_match(candidate) {
                    return Err(GuardError::Blocked(format!(
                        "matches dangerous pattern: {}",
                        name
                    )));
                }
            }
        }

        // Named fork bomb（CMD-10：classic `:(){ :|:& };:` 形态由
        // catastrophic_pipe 正则覆盖；自递归命名函数形态
        // `bomb(){ bomb|bomb & };bomb` 需要自引用判定，正则无后向引用，
        // 代码级扫描。
        if let Some(name) = detect_self_recursive_function(&normalized) {
            return Err(GuardError::Blocked(format!(
                "fork bomb pattern: self-recursive function `{}`",
                name
            )));
        }

        // Check dynamic entries
        let extra = self.extra_entries.read();
        for candidate in candidates {
            for (name, re) in extra.iter() {
                if re.is_match(candidate) {
                    return Err(GuardError::Blocked(format!(
                        "matches dynamic pattern: {}",
                        name
                    )));
                }
            }
        }
        drop(extra);

        // Strict mode: check for partial matches
        if self.config.strict_mode {
            let strict_keywords = get_strict_keywords();
            for candidate in candidates {
                for keyword in strict_keywords {
                    if candidate.contains(keyword) {
                        return Err(GuardError::Blocked(format!(
                            "strict mode: contains keyword '{}'",
                            keyword
                        )));
                    }
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
    /// CMD-11①：归一化单一真相源 = `matcher::normalize_exec_command`
    /// （剥引号/合并空白/小写）；本函数只在其上保留注释剥离（`#`/`//`
    /// 尾注释会让 blocklist 看见注释里的危险词，如 `echo hi # rm -rf`）。
    pub fn simplify_command(command: &str) -> String {
        let trimmed = command.trim();
        // Remove trailing comments
        let stripped = if let Some(pos) = trimmed.find(" #") {
            &trimmed[..pos]
        } else if let Some(pos) = trimmed.find(" //") {
            &trimmed[..pos]
        } else {
            trimmed
        };
        crate::matcher::normalize_exec_command(stripped)
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
        // CMD-02：解释器包装形态（strict 档整拒包装；默认档由 ABAC
        // 内层载荷扫描接管，避免 `powershell -c "Get-Date"` 之类正常
        // Windows 运维被硬拦）。
        "powershell -c",
        "powershell -command",
        "powershell -e",
        "pwsh -c",
        "pwsh -command",
        "bash -c",
        "sh -c",
        "zsh -c",
        "python -c",
        "python3 -c",
        "node -e",
        "perl -e",
        "ruby -e",
        "eval ",
        "exec ",
    ];
    KEYWORDS
}

/// CMD-10：自递归命名函数 fork bomb 检测（`bomb(){ bomb|bomb & };bomb`）。
/// 正则无后向引用做不了「体内引用自身名」的判定，代码级扫描：找 `(){`
/// 函数定义，取其名，看首个 `}` 前的体内是否出现同名 token。误伤面：
/// 单行函数体内字符串恰好含自身名（agent exec 场景极罕见），可接受。
fn detect_self_recursive_function(cmd: &str) -> Option<String> {
    let mut search_from = 0usize;
    while let Some(rel) = cmd[search_from..].find("(){") {
        let pos = search_from + rel;
        let before = &cmd[..pos];
        let name: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if !name.is_empty()
            && let Some(close_rel) = cmd[pos..].find('}')
        {
            let body = &cmd[pos..pos + close_rel];
            if body
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|token| token == name)
            {
                return Some(name);
            }
        }
        search_from = pos + 3;
    }
    None
}

type BlockList = Vec<(&'static str, Regex)>;

fn get_blocklist() -> &'static BlockList {
    static LIST: OnceLock<BlockList> = OnceLock::new();
    LIST.get_or_init(|| {
        let raw: Vec<(&str, &str)> = vec![
            // Destructive (CMD-09 扩充，2026-09-16 横扫存量加固)
            // rm_rf：任意 flag 形态——`-rf`/`-fr`/`-r`/`-rfv`/flag 后置
            // （`rm dir -rf`）/长参（`--recursive`），含 r 或 f 即命中；
            // `rm -i file`、`rm file` 不误伤。
            // 前缀用 `\b`（词边界）：`;rm`/`/bin/rm`/`$(rm` 等非空白分隔
            // 形态必须命中（复核 2026-09-16：`(^|\s)` 前缀把旧版 `\b` 能拦
            // 的 `cd /tmp;rm -rf ~` 族全部放跑——拦截回归）。
            ("rm_rf", r"(?i)\brm\s+(\S+\s+)*-{1,2}[a-z]*[rf][a-z]*(\s|$)"),
            // del_force：/s（递归）/f（强制）/q（安静）与组合（/sf）。
            ("del_force", r"(?i)\bdel(\.exe)?\s+/[a-z]*[sfq]"),
            // remove_item_recurse：PowerShell 的 rm -rf 等价物——
            // `Remove-Item -Recurse`（可 `-Force` 组合）。
            ("remove_item_recurse", r"(?i)\bremove-item\s+.*-recurse"),
            ("rd_s", r"(?i)\brd(\.exe)?\s+/[a-z]*[sfq]"),
            ("erase_recursive", r"(?i)\berase(\.exe)?\s+/[a-z]*[sfq]"),
            // format：只拦带盘符/设备目标的形态——裸 `\bformat\b` 会误伤
            // `git format-patch`、`npm run format`、`dotnet format`。
            ("format", r"(?i)(\bformat\s+[a-z]:|\bmkfs)"),
            ("dd", r"(?i)\bdd\s.*(if|of)="),
            ("shutdown", r"(?i)\b(shutdown|reboot|poweroff|halt)\b"),
            ("wipefs", r"(?i)\bwipefs\b"),
            ("shred", r"(?i)\bshred\b"),
            ("truncate", r"(?i)\btruncate\s+-s\s+0\b"),
            ("srm", r"(?i)\bsrm\b"),
            // catastrophic_pipe：classic 冒号炸弹（`:(){ :|:& };:`）——
            // 旧正则要求 `;` 在 `}` 前而 canonical 形态 `;` 在后，恒打不中
            // （CMD-10）；自递归命名函数形态由 check() 的代码级扫描覆盖。
            ("catastrophic_pipe", r"(?i):\(\)\s*\{[^}]*\}\s*;?\s*:"),
            // Privilege escalation (8)
            ("sudo", r"(?i)\bsudo\b"),
            ("chmod", r"(?i)\bchmod\s+[0-7]{3,4}\b"),
            // chmod_recursive：旧 `\bchmod\s+[0-7]{3,4}\b` 打不中
            // `chmod -R 777 /`（旗标隔在中间）。
            ("chmod_recursive", r"(?i)\bchmod\s+-[a-z]*r"),
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
            // Windows specific (CMD-03/04 修复 + CMD-09 缺组补齐)
            // powershell_encoded：pwsh 同样认 -enc/-EncodedCommand/-ec。
            (
                "powershell_encoded",
                r"(?i)\b(powershell|pwsh)(\.exe)?\b.*\s-(enc\w*|ec\b)",
            ),
            // cmd_c：`cmd.exe /c`（.exe 后缀）与 /k（保持窗口）同样拦截。
            ("cmd_c", r"(?i)\bcmd(\.exe)?\s+/[ck]\b"),
            ("reg_delete", r"(?i)\breg(\.exe)?\s+(delete|add)\b"),
            ("net_user", r"(?i)\bnet(\.exe)?\s+(user|localgroup)\b"),
            ("wmic", r"(?i)\bwmic\b"),
            ("bitsadmin", r"(?i)\bbitsadmin\b"),
            ("robocopy_mir", r"(?i)\brobocopy(\.exe)?\b.*\s/mir\b"),
            (
                "vssadmin_delete",
                r"(?i)\bvssadmin(\.exe)?\s+delete\s+shadows",
            ),
            (
                "wevtutil_cl",
                r"(?i)\bwevtutil(\.exe)?\s+(cl\b|clear-log\b)",
            ),
            ("diskpart", r"(?i)\bdiskpart(\.exe)?\b"),
            ("cipher_wipe", r"(?i)\bcipher(\.exe)?\s+/w\b"),
            ("schtasks_delete", r"(?i)\bschtasks(\.exe)?\s+/delete\b"),
            ("sc_delete", r"(?i)\bsc(\.exe)?\s+delete\b"),
            ("set_executionpolicy", r"(?i)\bset-executionpolicy\b"),
            ("clear_content", r"(?i)\bclear-content\b"),
            // Package manipulation (4)
            ("apt_remove", r"(?i)\bapt(-get)?\s+(remove|purge)\b"),
            ("yum_remove", r"(?i)\byum\s+remove\b"),
            ("dnf_remove", r"(?i)\bdnf\s+remove\b"),
            ("pip_uninstall", r"(?i)\bpip\s+uninstall\b"),
            // Disk/filesystem (3)
            ("mount_remount", r"(?i)\bmount\s+-o\s+.*\bremount\b"),
            ("fdisk", r"(?i)\bfdisk\b"),
            ("parted", r"(?i)\bparted\b"),
            // find 递归删除形态（CMD-08）
            ("find_delete", r"(?i)\bfind\s+.*\s-delete(\s|$)"),
            ("find_exec_rm", r"(?i)\bfind\s+.*\s-exec\s+(rm|del)\b"),
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
                name: "remove_item_recurse",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "PowerShell recursive delete",
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
            // 2026-09-16 横扫存量加固新增条目的 metadata
            BlockEntry {
                name: "rd_s",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Recursive directory delete (rd /s)",
            },
            BlockEntry {
                name: "erase_recursive",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Recursive delete via erase alias",
            },
            BlockEntry {
                name: "chmod_recursive",
                category: CommandCategory::Privilege,
                severity: Severity::High,
                platform: Platform::Linux,
                reason: "Recursive permission change",
            },
            BlockEntry {
                name: "robocopy_mir",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Mirror copy (deletes destination extras)",
            },
            BlockEntry {
                name: "vssadmin_delete",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Shadow copy deletion (ransomware staple)",
            },
            BlockEntry {
                name: "wevtutil_cl",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Event log clearing",
            },
            BlockEntry {
                name: "diskpart",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Disk partition manipulation",
            },
            BlockEntry {
                name: "cipher_wipe",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Windows,
                reason: "Secure wipe free space (/w)",
            },
            BlockEntry {
                name: "schtasks_delete",
                category: CommandCategory::Persistence,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Scheduled task deletion",
            },
            BlockEntry {
                name: "sc_delete",
                category: CommandCategory::Persistence,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Service deletion",
            },
            BlockEntry {
                name: "set_executionpolicy",
                category: CommandCategory::Obfuscation,
                severity: Severity::High,
                platform: Platform::Windows,
                reason: "Script execution policy change",
            },
            BlockEntry {
                name: "clear_content",
                category: CommandCategory::Destructive,
                severity: Severity::Medium,
                platform: Platform::Windows,
                reason: "File content truncation",
            },
            BlockEntry {
                name: "find_delete",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "Recursive find -delete",
            },
            BlockEntry {
                name: "find_exec_rm",
                category: CommandCategory::Destructive,
                severity: Severity::Critical,
                platform: Platform::Linux,
                reason: "find -exec rm batch delete",
            },
            BlockEntry {
                name: "pip_uninstall",
                category: CommandCategory::Destructive,
                severity: Severity::High,
                platform: Platform::All,
                reason: "Package removal",
            },
        ]
    })
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
