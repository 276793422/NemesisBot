//! Credential Scanner - Layer 4
//! Detects leaked credentials in tool arguments (AWS, GCP, GitHub, etc.).
//! 30+ patterns with configurable mask functions.
//!
//! Additional methods:
//! - `scan_tool_output()` - scan tool output with tool-name logging
//! - `set_action()` - dynamically update action at runtime
//! - `is_enabled()` / `get_action()` - query methods

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Instant;

use nemesis_types::utils;

pub(crate) mod exemptions;

/// Credential scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialResult {
    pub has_matches: bool,
    pub matches: Vec<CredentialMatch>,
    pub action: String,
    pub summary: String,
}

/// A single credential match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialMatch {
    pub pattern_name: String,
    pub redacted: String,
    pub full_match_start: String,
    pub full_match_end: String,
    /// 命中行号（1 起，2026-11-11 二次横扫批）：拦截反馈此前只有规则名
    /// 聚合，agent/用户仍无法定位具体行——目标设备二次实录里 agent 明确
    /// 报告「工具没有给出具体字符位置，尚不能断定命中的确切片段」。行号
    /// 是元数据不是内容，无泄漏面。serde(default)：字段晚于既有序列化
    /// 产物，旧 JSON 无此键时反序列化容忍缺失（回退 0 = 未知行）。
    #[serde(default)]
    pub line: usize,
}

/// Mask function configuration.
#[derive(Debug, Clone, Copy, Default)]
pub enum MaskMode {
    /// Keep first 4 and last 4 chars
    #[default]
    KeepPrefix,
    /// Replace with fixed [REDACTED]
    Fixed,
    /// Show key name but redact value
    KeyValue,
}

/// Credential scanner with configurable masking.
pub struct Scanner {
    enabled: bool,
    action: String,
    mask_mode: MaskMode,
}

impl Scanner {
    pub fn new(enabled: bool, action: &str) -> Self {
        Self {
            enabled,
            action: action.to_string(),
            mask_mode: MaskMode::KeepPrefix,
        }
    }

    pub fn with_mask_mode(enabled: bool, action: &str, mask_mode: MaskMode) -> Self {
        Self {
            enabled,
            action: action.to_string(),
            mask_mode,
        }
    }

    /// Scan content for credentials.
    pub fn scan_content(&self, content: &str) -> CredentialResult {
        if !self.enabled || content.len() <= 10 {
            return CredentialResult {
                has_matches: false,
                matches: vec![],
                action: self.action.clone(),
                summary: String::new(),
            };
        }

        let patterns = get_credential_patterns();
        let mut matches = Vec::new();

        for (name, re) in patterns {
            for cap in re.captures_iter(content) {
                let m0 = cap.get(0);
                let full = m0.map(|m| m.as_str()).unwrap_or("");
                // 命中行号（1 起）：反馈定位面（2026-11-11 二次横扫批）。
                let line = m0
                    .map(|m| content[..m.start()].bytes().filter(|&b| b == b'\n').count() + 1)
                    .unwrap_or(1);
                // 豁免表统一求值（2026-11-11 机制化重构批，用户拍板「一个
                // 位置定义 + 机制消费」）：全部误报豁免 = 数据表单一定义
                // 点（credential/exemptions.rs），本面只剩这一行调用——
                // 新增豁免改表，不再在本 if 链贴钩子。语义与旧 12 组钩子
                // 链逐条 continue 等价（行间独立，任一判 true 即豁免）。
                if exemptions::is_exempt(&exemptions::ExemptionCtx {
                    rule: name,
                    full,
                    window: content,
                    whole: content,
                    match_end: m0.map(|m| m.end()).unwrap_or(0),
                    re,
                }) {
                    continue;
                }
                let redacted = self.mask_value(full);
                matches.push(CredentialMatch {
                    pattern_name: name.to_string(),
                    redacted: redacted.clone(),
                    line,
                    full_match_start: if full.len() > 4 {
                        // floor/ceil to a char boundary — multibyte matched values
                        // (e.g. a Chinese password) would panic on a naive byte slice.
                        let end = utils::floor_char_boundary(full, 4);
                        full[..end].to_string()
                    } else {
                        full.to_string()
                    },
                    full_match_end: if full.len() > 4 {
                        let start = utils::ceil_char_boundary(full, full.len() - 4);
                        full[start..].to_string()
                    } else {
                        String::new()
                    },
                });
            }
        }

        let has_matches = !matches.is_empty();
        let summary = if has_matches {
            // 规则名 + 行号聚合（只给规则名/行号与计数，不给匹配值）：拦截
            // 反馈面第一版只有总数，agent 与用户无从定位命中面；第二版加了
            // 规则名仍缺行号——目标设备二次实录里 agent 报告「没有给出具体
            // 字符位置，尚不能断定命中的确切片段」。行号定位后下一轮自检
            // 不再是盲改重试。
            let mut by_rule: Vec<(String, Vec<usize>)> = Vec::new();
            for m in &matches {
                match by_rule.iter_mut().find(|(n, _)| *n == m.pattern_name) {
                    Some((_, lines)) => lines.push(m.line),
                    None => by_rule.push((m.pattern_name.clone(), vec![m.line])),
                }
            }
            let rules = by_rule
                .iter()
                .map(|(n, lines)| {
                    let mut sorted = lines.clone();
                    sorted.sort_unstable();
                    sorted.dedup();
                    let locs = sorted
                        .iter()
                        .map(|l| format!("L{}", l))
                        .collect::<Vec<_>>()
                        .join(",");
                    if lines.len() > 1 {
                        format!("{}@{} x{}", n, locs, lines.len())
                    } else {
                        format!("{}@{}", n, locs)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} credential(s) detected [{}]", matches.len(), rules)
        } else {
            String::new()
        };

        CredentialResult {
            has_matches,
            matches,
            action: self.action.clone(),
            summary,
        }
    }

    /// Redact content by replacing all credential matches with masked versions.
    pub fn redact_content(&self, content: &str) -> String {
        if !self.enabled || content.len() <= 10 {
            return content.to_string();
        }

        let patterns = get_credential_patterns();
        let mut result = content.to_string();

        for (name, re) in patterns {
            // 豁免表统一求值（2026-11-11 机制化重构批）与 scan_content
            // 同源同语义：豁免 = 原样保留不打码（损毁教程/文档可读性）。
            // 本面只剩这一行调用，表见 credential/exemptions.rs。gcp 上下
            // 文闸用原始 content（整体真相，不随前序规则打码漂移）；PEM
            // 窗口闸用当前 result（caps 偏移与被扫串对齐）——两路文本经
            // ExemptionCtx 的 whole/window 分槽透传。
            result = re
                .replace_all(&result, |caps: &regex::Captures| {
                    let full = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                    if exemptions::is_exempt(&exemptions::ExemptionCtx {
                        rule: name,
                        full,
                        window: &result,
                        whole: content,
                        match_end: caps.get(0).map(|m| m.end()).unwrap_or(0),
                        re,
                    }) {
                        return full.to_string();
                    }
                    "[REDACTED_CREDENTIAL]".to_string()
                })
                .to_string();
        }

        result
    }

    fn mask_value(&self, value: &str) -> String {
        match self.mask_mode {
            MaskMode::KeepPrefix => mask_keep_prefix(value),
            MaskMode::Fixed => "[REDACTED]".to_string(),
            MaskMode::KeyValue => {
                if value.len() > 8 {
                    let end = utils::floor_char_boundary(value, 4);
                    let start = utils::ceil_char_boundary(value, value.len() - 4);
                    format!("{}...{}", &value[..end], &value[start..])
                } else {
                    "[REDACTED]".to_string()
                }
            }
        }
    }

    /// Scan tool output for leaked credentials.
    ///
    /// Equivalent to Go's `Scanner.ScanToolOutput()`. This is a convenience
    /// wrapper around `scan_content()` that logs timing and tool name info.
    pub fn scan_tool_output(&self, tool_name: &str, output: &str) -> CredentialResult {
        let start = Instant::now();
        let result = self.scan_content(output);
        let elapsed = start.elapsed();

        if result.has_matches {
            tracing::warn!(
                tool = tool_name,
                count = result.matches.len(),
                elapsed_ms = elapsed.as_millis() as u64,
                "[Security] Tool output contains potential credentials"
            );
        } else {
            tracing::debug!(
                tool = tool_name,
                elapsed_ms = elapsed.as_millis() as u64,
                "[Security] Tool output clean"
            );
        }

        result
    }

    /// Dynamically update the action at runtime.
    ///
    /// Equivalent to Go's `Scanner.SetAction()`. Valid actions are
    /// "block", "redact", and "warn". Returns an error for invalid actions.
    pub fn set_action(&mut self, action: &str) -> Result<(), String> {
        match action {
            "block" | "redact" | "warn" => {
                self.action = action.to_string();
                Ok(())
            }
            _ => Err(format!(
                "invalid action {:?}: must be block, redact, or warn",
                action
            )),
        }
    }

    /// Returns whether the credential scanner is enabled.
    ///
    /// Equivalent to Go's `Scanner.IsEnabled()`.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the configured action.
    ///
    /// Equivalent to Go's `Scanner.GetAction()`.
    pub fn get_action(&self) -> &str {
        &self.action
    }
}

fn mask_keep_prefix(value: &str) -> String {
    if value.len() > 8 {
        let end = utils::floor_char_boundary(value, 4);
        let start = utils::ceil_char_boundary(value, value.len() - 4);
        format!("{}...{}", &value[..end], &value[start..])
    } else {
        "[REDACTED]".to_string()
    }
}

type CredentialPatterns = Vec<(&'static str, Regex)>;

/// 内置凭据规则名只读快照（Dashboard 第④层页 `security.credential_rules`
/// 消费）。§2.7 核实结论（2026-11-10）：规则表纯硬编码（32 条 OnceLock，
/// nemesis-web overview_tests 有 drift 锁），无任何 config 扩展面——层页
/// 只读呈现，类别分组由前端按名字映射（Rust 侧无类别元数据）。
pub fn builtin_credential_rules() -> Vec<&'static str> {
    get_credential_patterns()
        .iter()
        .map(|(name, _)| *name)
        .collect()
}

fn get_credential_patterns() -> &'static CredentialPatterns {
    static PATTERNS: OnceLock<CredentialPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw: Vec<(&str, &str)> = vec![
            // AWS
            ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
            ("aws_secret_key", r"(?i)aws_secret_access_key\s*[=:]\s*[A-Za-z0-9/+=]{40}"),
            ("aws_session_token", r"(?i)aws_session_token\s*[=:]\s*[A-Za-z0-9/+=]{100,}"),
            // GCP
            ("gcp_service_account", r#"(?i)"type"\s*:\s*"service_account""#),
            // private_key_id 真实形态是 32 hex；{16,} 挡住文档 JSON 模板的
            // 短占位符（"abc123" 类，2026-11-11 横扫批：无下限时 1 个 hex
            // 字符即命中）。真实 32 ≥ 16，零真实漏检面。
            ("gcp_private_key_id", r#"(?i)"private_key_id"\s*:\s*"[a-f0-9]{16,}""#),
            // Azure
            // AccountKey 真实形态是 88 base64；{64,} 挡住连接串模板的短
            // 占位符（AccountKey=mykey; 类，2026-11-11 横扫批）。Azure 官方
            // 文档自身的 88 字符示例 key 仍命中——同长度不可分辨，属结构
            // 规则固有取舍（见修复报告横扫节）。
            ("azure_connection_string", r"(?i)AccountName=[A-Za-z0-9]+;AccountKey=[A-Za-z0-9+/=]{64,}"),
            ("azure_tenant_id", r"(?i)azure\s*tenant\s*(?:id)?\s*[=:]\s*[a-f0-9-]{36}"),
            // GitHub
            ("github_token", r"gh[pousr]_[A-Za-z0-9_]{36,}"),
            ("github_oauth", r"(?i)github[_-]?oauth\s*[=:]\s*[a-f0-9]{40}"),
            // Slack
            ("slack_token", r"xox[baprs]-[0-9]{10,}-[A-Za-z0-9]{24,}"),
            ("slack_webhook", r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+"),
            // Stripe
            ("stripe_key", r"(?:sk|pk)_(?:test|live)_[A-Za-z0-9]{24,}"),
            // SendGrid
            ("sendgrid_key", r"SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}"),
            // Twilio
            ("twilio_sid", r"AC[a-z0-9]{32}"),
            // Mailgun
            ("mailgun_key", r"key-[a-z0-9]{32}"),
            // Private keys
            ("private_key", r"-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----"),
            ("ec_private_key", r"-----BEGIN\s+EC\s+PRIVATE\s+KEY-----"),
            ("dsa_private_key", r"-----BEGIN\s+DSA\s+PRIVATE\s+KEY-----"),
            ("pgp_private_key", r"-----BEGIN\s+PGP\s+PRIVATE\s+KEY\s+BLOCK-----"),
            // JWT
            ("jwt_token", r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"),
            // Database connection strings
            ("db_connection_mysql", r"(?i)mysql://[^\s]+:[^\s]+@[^\s]+"),
            ("db_connection_postgres", r"(?i)postgres(ql)?://[^\s]+:[^\s]+@[^\s]+"),
            ("db_connection_mongodb", r"(?i)mongodb(\+srv)?://[^\s]+:[^\s]+@[^\s]+"),
            ("db_connection_redis", r"(?i)redis://:[^\s]+@[^\s]+"),
            // Generic API key patterns
            ("api_key_in_url", r"(?i)[?&](?:api[_-]?key|token|secret|password)=([A-Za-z0-9_\-]{20,})"),
            // scheme 词后的 token 段必须有长度下限：无下限时 docstring 散文
            // 「for Bearer mode.」「for Basic mode,」和 CLI 用法行「--auth bearer
            // --issue」里的普通英文词/参数名（mode、--issue，字符类内的字面
            // `-`/`.`）被当成 token 误拦（2026-11-11 实录：通用化 jira skill 脚本
            // 写入被 credential_scanner 以 2-3 处凭据硬拦，两台设备同族复现）。
            // 真实 Bearer token（RFC 6750，JWT 典型百字符）≥20；Basic 凭据是
            // base64(user:password)，user:password 最短 9 字符 → base64 ≥12。
            ("bearer_token", r"(?i)Bearer\s+[A-Za-z0-9\-._~+/]{20,}=*"),
            ("basic_auth", r"(?i)Basic\s+[A-Za-z0-9+/]{12,}=*"),
            // Generic secret assignment
            // 值段禁跨行（[^'"\n]）：[^'"] 会连 \n 一起吞，getpass("Jira password: ")
            // 的文案闭引号与下一行 JiraError(" 的开引号被跨行配对，中间两行正常代码
            // 被当成"密钥值"误拦（2026-11-10 实录：只读 Jira 脚本写入被 credential_scanner
            // 硬拦，复现 match=pass...or("）。密钥值单行内出现，禁 \n 无真实漏检面。
            ("secret_assignment", r#"(?i)(?:password|secret|token|api_key|apikey|private_key|access_key)\s*[=:]\s*['"][^'"\n]{8,}['"]"#),
            // Heroku
            ("heroku_key", r"(?i)heroku\s*[=:]\s*[a-f0-9-]{36}"),
            // Netlify
            ("netlify_token", r"(?i)netlify[_-]?token\s*[=:]\s*[A-Za-z0-9_-]{40,}"),
            // Discord
            ("discord_bot_token", r"(?i)discord[_-]?bot[_-]?token\s*[=:]\s*[A-Za-z0-9._-]{50,}"),
            // NPM
            // authToken 真实 token ≥36 字符（npm_ 前缀 + 36 [A-Za-z0-9]）；
            // 类含下划线（真实前缀 npm_ 必含 `_`——旧类漏下划线时真实
            // token 在类内只剩 "npm" 3 字符，根本匹配不上完整 token），
            // {20,} 挡住 .npmrc 文档里的短占位符（YOUR_AUTH_TOKEN 类 15
            // 字符，2026-11-11 横扫批；canonical `${NPM_TOKEN}` 模板因
            // `$` 在字符类外本就不命中）。
            ("npm_token", r"//registry\.npmjs\.org/:_authToken=[A-Za-z0-9_-]{20,}"),
        ];

        raw.into_iter()
            .filter_map(|(name, pattern)| {
                Regex::new(pattern).ok().map(|re| (name, re))
            })
            .collect()
    })
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
