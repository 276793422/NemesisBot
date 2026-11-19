//! DLP Engine - Layer 5
//! Data Loss Prevention with 30+ configurable rules for PII and sensitive data.

use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use nemesis_types::utils;

use crate::credential::exemptions;

/// DLP scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpResult {
    pub has_matches: bool,
    pub matches: Vec<DlpMatch>,
    pub action: String,
    pub summary: String,
    /// Whether the pipeline should *block* the operation.
    ///
    /// True iff at least one match resolved to a "block" effective action
    /// (i.e. a High/Medium-confidence match under the default policy).
    /// Low-confidence matches (phone/ip/email on arbitrary web content) set
    /// this to false — they are recorded for audit but do not break the flow.
    #[serde(default)]
    pub should_block: bool,
}

/// Severity of a DLP match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DlpSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

/// Confidence that a match is a *genuine* secret/PII, not a false positive.
///
/// Orthogonal to [`DlpSeverity`] (which measures *how sensitive* the data is).
/// Confidence drives the effective action: `High`→block, `Medium`→block,
/// `Low`→log-only by default (see `DlpConfig::low_confidence_action`).
///
/// Low-confidence rules are pattern-only and prone to false positives on
/// arbitrary web content (e.g. `phone_international` matching a Chinese ICP
/// filing number, `ip_address_public` matching a software version like `2.4.1.8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DlpConfidence {
    /// High certainty: fixed prefix + checksum/structure (private keys, AWS keys,
    /// JWT, GitHub/Slack tokens). Almost never a false positive.
    High,
    /// Medium certainty: keyed assignment patterns (`password=`, `token=`) or
    /// structured identifiers (credit cards w/o Luhn yet, IBAN).
    #[default]
    Medium,
    /// Low certainty: bare pattern, easily matches ordinary numbers/text
    /// (phone, public IP, email, national IDs without checksum).
    Low,
}

/// A single DLP match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpMatch {
    pub rule_name: String,
    pub category: String,
    pub count: usize,
    /// Severity of the match.
    #[serde(default)]
    pub severity: DlpSeverity,
    /// Confidence that this is a genuine match (not a false positive).
    #[serde(default)]
    pub confidence: DlpConfidence,
    /// Effective action decided for this match ("block" or "log").
    /// Driven by confidence + config policy; see [`DlpConfig::action_for`].
    #[serde(default)]
    pub effective_action: String,
    /// Masked/redacted representation of the match.
    #[serde(default)]
    pub masked_value: String,
    /// Byte offset of the first match within the input.
    #[serde(default)]
    pub start_position: usize,
}

/// A DLP rule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpRule {
    pub name: String,
    pub category: String,
    pub pattern: String,
    pub enabled: bool,
    pub action: String,
    /// Confidence of this rule's matches (default Medium).
    #[serde(default)]
    pub confidence: DlpConfidence,
}

/// DLP engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlpConfig {
    pub enabled: bool,
    pub action: String,
    pub custom_rules: Vec<DlpRule>,
    /// Only run rules whose names are in this list (empty = all rules).
    #[serde(default)]
    pub enabled_rules: Vec<String>,
    /// Maximum content length to scan (0 = no limit).
    #[serde(default)]
    pub max_content_length: usize,
    /// Action taken for **low-confidence** matches (default "log" — detect but
    /// don't block). High/Medium-confidence matches always use `action`.
    /// Set to "block" to restore the old hard-block-everything behavior.
    #[serde(default = "default_low_confidence_action")]
    pub low_confidence_action: String,
}

fn default_low_confidence_action() -> String {
    "log".to_string()
}

impl Default for DlpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            action: "block".to_string(),
            custom_rules: vec![],
            enabled_rules: vec![],
            max_content_length: 0,
            low_confidence_action: default_low_confidence_action(),
        }
    }
}

impl DlpConfig {
    /// Decide the effective action for a match of the given confidence.
    ///
    /// - High/Medium → `action` (block by default)
    /// - Low → `low_confidence_action` (log by default)
    pub fn action_for(&self, confidence: DlpConfidence) -> String {
        match confidence {
            DlpConfidence::High | DlpConfidence::Medium => self.action.clone(),
            DlpConfidence::Low => self.low_confidence_action.clone(),
        }
    }
}

/// DLP engine with dynamic rules and redaction.
pub struct DlpEngine {
    config: DlpConfig,
    dynamic_rules: RwLock<Vec<DlpRule>>,
}

impl DlpEngine {
    pub fn new(enabled: bool, action: &str) -> Self {
        Self {
            config: DlpConfig {
                enabled,
                action: action.to_string(),
                custom_rules: vec![],
                enabled_rules: vec![],
                max_content_length: 0,
                low_confidence_action: default_low_confidence_action(),
            },
            dynamic_rules: RwLock::new(Vec::new()),
        }
    }

    pub fn with_config(config: DlpConfig) -> Self {
        Self {
            config,
            dynamic_rules: RwLock::new(Vec::new()),
        }
    }

    /// Scan tool input for sensitive data.
    pub fn scan_tool_input(&self, _tool_name: &str, args: &serde_json::Value) -> DlpResult {
        let text = extract_text(args);
        self.scan_text(&text)
    }

    /// Scan text for DLP patterns.
    pub fn scan_text(&self, text: &str) -> DlpResult {
        if !self.config.enabled || text.is_empty() {
            return DlpResult {
                has_matches: false,
                matches: vec![],
                action: self.config.action.clone(),
                summary: String::new(),
                should_block: false,
            };
        }

        // Truncate oversized content instead of skipping.
        // floor_char_boundary avoids slicing mid-codepoint (which panics on
        // multibyte text like Chinese) — same approach as scan_tool_output.
        let scan_text: std::borrow::Cow<str> =
            if self.config.max_content_length > 0 && text.len() > self.config.max_content_length {
                let end = utils::floor_char_boundary(text, self.config.max_content_length);
                std::borrow::Cow::Borrowed(&text[..end])
            } else {
                std::borrow::Cow::Borrowed(text)
            };

        let rules = get_dlp_rules();
        let mut matches = Vec::new();

        for rule in rules {
            if !self.is_rule_enabled(rule.name) {
                continue;
            }
            for mat in rule.re.find_iter(&scan_text) {
                let matched_text = mat.as_str();
                // 引用豁免后滤（2026-11-10 B4 会话凭据误拦修复 + 裸形态批，
                // 用户拍板；与 credential.rs Layer 4 同源同语义）：匹配文本
                // 剔除全部 vault 引用形态（完整模板 `{{vault:<alias>}}` +
                // 裸引用 `vault:<alias>`，含无引号形态）后对同一规则重扫
                // ——仍命中才算真命中。`password = vault:zope/main` 是
                // 配置文件合法写法不是 DLP 命中；明文/混拼/并存秘密照旧。
                // ⚠ 勿删豁免 = 误报回归；根因/防回退/未来出路（数据式凭据
                // 通道）全文见 nemesis_config::vault_ref::
                // strip_reference_forms 文档。
                // `VAULT_PREFIX`（`vault:`）是两类引用形态的公共子串——
                // 守卫只查它即可全覆盖（裸引用不含 `{{vault:`，查
                // TEMPLATE_HEAD 会漏掉无引号形态）；纯明文零成本短路。
                if matched_text.contains(nemesis_config::vault_ref::VAULT_PREFIX)
                    && !rule
                        .re
                        .is_match(&nemesis_config::vault_ref::strip_reference_forms(
                            matched_text,
                        ))
                {
                    continue;
                }
                // Post-match validator (Luhn / ID checksum): demote to Low if it fails,
                // killing "right format, not a real number" false positives.
                let confidence = match rule.validator {
                    Some(v) if !v(&scan_text, mat.start(), mat.end()) => DlpConfidence::Low,
                    _ => rule.confidence,
                };
                let effective_action = self.config.action_for(confidence);
                matches.push(DlpMatch {
                    rule_name: rule.name.to_string(),
                    category: rule.category.to_string(),
                    count: 1,
                    severity: category_to_severity(rule.category),
                    confidence,
                    effective_action: effective_action.clone(),
                    masked_value: partial_mask(matched_text),
                    start_position: mat.start(),
                });
            }
        }

        // Check dynamic rules
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            if !rule.enabled {
                continue;
            }
            if !self.is_rule_enabled(&rule.name) {
                continue;
            }
            if let Ok(re) = Regex::new(&rule.pattern) {
                for mat in re.find_iter(&scan_text) {
                    let matched_text = mat.as_str();
                    // 引用豁免后滤（同上方静态规则臂，裸形态批切换）。
                    if matched_text.contains(nemesis_config::vault_ref::VAULT_PREFIX)
                        && !re.is_match(&nemesis_config::vault_ref::strip_reference_forms(
                            matched_text,
                        ))
                    {
                        continue;
                    }
                    let confidence = rule.confidence;
                    let effective_action = self.config.action_for(confidence);
                    matches.push(DlpMatch {
                        rule_name: rule.name.clone(),
                        category: rule.category.clone(),
                        count: 1,
                        severity: DlpSeverity::Medium,
                        confidence,
                        effective_action: effective_action.clone(),
                        masked_value: partial_mask(matched_text),
                        start_position: mat.start(),
                    });
                }
            }
        }

        // Deduplicate by (rule_name, start_position) — same pattern at
        // different positions produces separate entries.
        let mut seen = std::collections::HashSet::new();
        matches.retain(|m| seen.insert((m.rule_name.clone(), m.start_position)));

        // Aggregate count per (rule_name, start_position) is 1 by construction,
        // but we may have multiple positions for the same rule. Sum total.
        let has_matches = !matches.is_empty();
        // should_block: any match whose effective action is "block"
        // (i.e. High/Medium-confidence under the default policy). Low-confidence
        // matches (phone/ip/email on arbitrary content) do not block.
        let should_block = matches.iter().any(|m| m.effective_action == "block");
        let summary = if has_matches {
            let total: usize = matches.iter().map(|m| m.count).sum();
            let blocking = matches
                .iter()
                .filter(|m| m.effective_action == "block")
                .count();
            // BUG-20261110-046 伴生修复：按**去重规则数**计数——旧实现把
            // match 条目数（rule,position 对）当 rule 数，同一规则多处命中
            // 时虚报（2026-11-10 实录：3× secret_password_assignment +
            // 1× ip_address_public 报成「across 4 rule(s)」，实为 2 条）。
            let rule_count = matches
                .iter()
                .map(|m| m.rule_name.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len();
            format!(
                "{} sensitive data pattern(s) detected across {} rule(s) ({} blocking)",
                total, rule_count, blocking
            )
        } else {
            String::new()
        };

        DlpResult {
            has_matches,
            matches,
            action: self.config.action.clone(),
            summary,
            should_block,
        }
    }

    /// Redact content by replacing all DLP matches with overlap detection.
    ///
    /// Sorts all match positions, resolves overlaps by preferring the longest
    /// match, and replaces only in non-overlapping regions.
    pub fn redact_content(&self, text: &str) -> String {
        if !self.config.enabled || text.is_empty() {
            return text.to_string();
        }

        // Collect all match spans (start, end) from all rules
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let rules = get_dlp_rules();
        let _byte_text = text.as_bytes();

        for rule in rules.iter() {
            for m in rule.re.find_iter(text) {
                // 引用豁免后滤（2026-11-10 A 批 redact 面补齐 + 裸形态批，
                // 与 scan_text 同源同语义）：纯引用跨度不进替换集——出站面
                //（gateway outbound DLP）与输出复扫不再打码引用原文（模板
                // 与裸引用形态）。仍命中（明文/混拼）照旧替换。
                let matched = m.as_str();
                if matched.contains(nemesis_config::vault_ref::VAULT_PREFIX)
                    && !rule
                        .re
                        .is_match(&nemesis_config::vault_ref::strip_reference_forms(matched))
                {
                    continue;
                }
                // db_connection 占位符豁免与 scan_text 同源同语义（2026-11-11
                // 横扫批）：教程/模板连接串不构成泄漏，照常保留——不打码
                // 损毁文档/配置可读性。判定单一真相源
                // crate::credential::exemptions（豁免表单一真相源模块）。
                if rule.name == "database_connection_string"
                    && crate::credential::exemptions::is_placeholder_db_credential(matched)
                {
                    continue;
                }
                // authorization_header 占位符豁免与 scan_text 同源同语义
                // （2026-11-11 二次横扫批）：Basic 散文/教程形态照常保留，
                // 不打码损毁文档可读性。判定单一真相源
                // `credential/exemptions.rs`（validator 臂
                // authorization_not_placeholder 同源）。
                if rule.name == "authorization_header"
                    && !authorization_not_placeholder(text, m.start(), m.end())
                {
                    continue;
                }
                // 三次横扫 validator 的 redact 面对称（2026-11-11 三次横扫
                // 批自查补齐；aws 两臂 2026-11-11 机制化重构批拆分）：validator
                // 判 false（AWS 文档 canonical / 值段散文 / 教程引号复合词 /
                // PEM 格式示意 / jwt.io demo / stripe 文档测试 key / slack
                // 文档占位——不构成泄漏）的跨度不进替换集，不打码损毁文档
                // 可读性。判定单一真相源 = validator 本体（scan_text 阻断面
                // 同函数，两口径永不分裂）。
                if rule.name == "aws_access_key"
                    && !aws_access_not_doc_example(text, m.start(), m.end())
                {
                    continue;
                }
                if rule.name == "aws_secret_key"
                    && !aws_secret_not_doc_example(text, m.start(), m.end())
                {
                    continue;
                }
                if rule.name == "github_token" && !github_token_not_prose(text, m.start(), m.end())
                {
                    continue;
                }
                if matches!(
                    rule.name,
                    "azure_api_key" | "generic_hex_key" | "generic_base64_key"
                ) && !token_value_not_prose(text, m.start(), m.end())
                {
                    continue;
                }
                if rule.name == "jwt_token" && !jwt_not_known_demo(text, m.start(), m.end()) {
                    continue;
                }
                if rule.name == "stripe_key"
                    && !stripe_not_docs_or_placeholder(text, m.start(), m.end())
                {
                    continue;
                }
                if rule.name == "slack_token"
                    && !slack_not_docs_placeholder(text, m.start(), m.end())
                {
                    continue;
                }
                if matches!(
                    rule.name,
                    "secret_password_assignment"
                        | "secret_token_assignment"
                        | "secret_key_assignment"
                ) && value_is_quoted(text, m.start(), m.end())
                    && !quoted_secret_not_tutorial(text, m.start(), m.end())
                {
                    // 仅「引号 + 教程复合词」形态保留原文；裸值（validator
                    // 同判 false）不豁免——裸秘密在出站仍是泄漏面，照常
                    // 打码（test_bare_secret_redacted_but_not_blocking 钉死
                    // 的既有语义）。
                    continue;
                }
                if matches!(
                    rule.name,
                    "private_key_rsa"
                        | "private_key_generic"
                        | "private_key_openssh"
                        | "private_key_pkcs8"
                ) && !pem_not_illustration(text, m.start(), m.end())
                {
                    continue;
                }
                spans.push((m.start(), m.end()));
            }
        }

        // Dynamic rules
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            if !rule.enabled {
                continue;
            }
            if let Ok(re) = Regex::new(&rule.pattern) {
                for m in re.find_iter(text) {
                    // 引用豁免后滤（同上方静态规则臂，裸形态批切换）。
                    let matched = m.as_str();
                    if matched.contains(nemesis_config::vault_ref::VAULT_PREFIX)
                        && !re.is_match(&nemesis_config::vault_ref::strip_reference_forms(matched))
                    {
                        continue;
                    }
                    spans.push((m.start(), m.end()));
                }
            }
        }

        if spans.is_empty() {
            return text.to_string();
        }

        // Sort by start position, then by length descending (prefer longer matches)
        spans.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));

        // Resolve overlaps: keep non-overlapping spans
        let mut resolved: Vec<(usize, usize)> = Vec::new();
        for span in spans {
            if let Some(last) = resolved.last()
                && span.0 < last.1
            {
                // Overlapping - skip (already have a longer match at this position)
                continue;
            }
            resolved.push(span);
        }

        // Build the result by replacing matched regions
        let mut result = String::with_capacity(text.len());
        let mut last_end = 0;
        for (start, end) in &resolved {
            if *start > last_end {
                result.push_str(&text[last_end..*start]);
            }
            result.push_str("[REDACTED]");
            last_end = *end;
        }
        if last_end < text.len() {
            result.push_str(&text[last_end..]);
        }

        result
    }

    /// Add a dynamic rule.
    pub fn add_rule(&self, rule: DlpRule) -> Result<(), String> {
        // Validate pattern
        Regex::new(&rule.pattern).map_err(|e| format!("invalid pattern: {}", e))?;
        self.dynamic_rules.write().push(rule);
        Ok(())
    }

    /// Remove a dynamic rule by name.
    pub fn remove_rule(&self, name: &str) -> bool {
        let mut rules = self.dynamic_rules.write();
        let before = rules.len();
        rules.retain(|r| r.name != name);
        rules.len() < before
    }

    /// Get all rule names (static + dynamic).
    pub fn get_rule_names(&self) -> Vec<String> {
        let static_rules = get_dlp_rules();
        let mut names: Vec<String> = static_rules.iter().map(|r| r.name.to_string()).collect();
        let dynamic = self.dynamic_rules.read();
        for rule in dynamic.iter() {
            names.push(rule.name.clone());
        }
        names
    }

    /// Update configuration dynamically.
    pub fn update_config(&mut self, enabled: Option<bool>, action: Option<String>) {
        if let Some(e) = enabled {
            self.config.enabled = e;
        }
        if let Some(a) = action {
            self.config.action = a;
        }
    }

    /// Scan tool output for sensitive data leaks.
    pub fn scan_tool_output(&self, tool_name: &str, output: &str) -> DlpResult {
        let text = if output.len() > 5000 {
            let end = utils::floor_char_boundary(output, 5000);
            &output[..end]
        } else {
            output
        };
        let mut result = self.scan_text(text);
        // Mark output scans as lower severity
        for m in &mut result.matches {
            if m.severity == DlpSeverity::Critical {
                m.severity = DlpSeverity::High;
            }
        }
        let _ = tool_name; // used for logging in full impl
        result
    }

    /// Check if a rule is enabled based on the enabled_rules filter.
    fn is_rule_enabled(&self, rule_name: &str) -> bool {
        if self.config.enabled_rules.is_empty() {
            return true;
        }
        self.config.enabled_rules.iter().any(|r| r == rule_name)
    }

    /// Check if the engine is enabled.
    /// Mirrors Go DLP `IsEnabled()`.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Enable or disable the engine.
    /// Mirrors Go DLP `SetEnabled(bool)`.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
    }

    /// Get total rule count (static + dynamic), regardless of enabled_rules filter.
    /// Mirrors Go DLP `GetRuleCount()`.
    pub fn total_rule_count(&self) -> usize {
        get_dlp_rules().len() + self.dynamic_rules.read().len()
    }

    /// Get enabled rules count.
    pub fn enabled_rule_count(&self) -> usize {
        if self.config.enabled_rules.is_empty() {
            get_dlp_rules().len() + self.dynamic_rules.read().len()
        } else {
            self.config.enabled_rules.len()
        }
    }

    /// Scan content — convenience wrapper for scan_text.
    /// Mirrors Go DLP `ScanContent(ctx, content)`.
    pub fn scan_content(&self, content: &str) -> DlpResult {
        self.scan_text(content)
    }
}

fn extract_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            map.values().map(extract_text).collect::<Vec<_>>().join(" ")
        }
        serde_json::Value::Array(arr) => arr.iter().map(extract_text).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// Map a category string to severity.
fn category_to_severity(category: &str) -> DlpSeverity {
    match category {
        "credential" | "secret_key" | "api_key" => DlpSeverity::Critical,
        "pii" | "financial" | "credit_card" | "bank" => DlpSeverity::High,
        "network" | "contact" | "personal" | "location" | "medical" => DlpSeverity::Medium,
        "ssn" | "national_id" | "email" | "phone" | "ip" => DlpSeverity::Medium,
        _ => DlpSeverity::Low,
    }
}

/// Partially mask a matched string, showing the first 2 and last 2 characters.
///
/// For strings shorter than 4 characters, returns `[REDACTED]`.
/// Mirrors Go's partial masking behavior (e.g., `"42****1234"`).
fn partial_mask(s: &str) -> String {
    // Char-based (not byte-based) slicing so multibyte matched text — e.g. a
    // Chinese password caught by `secret_password_assignment`'s `[^\s'"]{8,}` —
    // doesn't slice mid-codepoint and panic. Shows first 2 + last 2 chars,
    // e.g. "中文密码符" -> "中文****码符".
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 5 {
        // Too short: prefix(2)+suffix(2) would cover the whole string, so
        // masking would leak everything — return fully redacted instead.
        return "[REDACTED]".to_string();
    }
    let n = chars.len();
    let prefix: String = chars[..2].iter().collect();
    let suffix: String = chars[n - 2..].iter().collect();
    format!("{}****{}", prefix, suffix)
}

type DlpRules = Vec<StaticRule>;

/// Post-match validator: given the full text and the match's byte span,
/// returns true if the match is genuine (passes checksum). false demotes the
/// match to Low confidence. Used to kill "right format, not a real number"
/// false positives (e.g. any 16-digit string masquerading as a credit card).
type Validator = fn(text: &str, start: usize, end: usize) -> bool;

/// A compiled static DLP rule with confidence and optional post-match validator.
struct StaticRule {
    category: &'static str,
    name: &'static str,
    re: Regex,
    confidence: DlpConfidence,
    validator: Option<Validator>,
}

/// Luhn checksum validation for credit-card-shaped matches.
fn luhn_valid(text: &str, start: usize, end: usize) -> bool {
    let s = match text.get(start..end) {
        Some(s) => s,
        None => return false,
    };
    let digits: Vec<u8> = s
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| d as u8)
        .collect();
    if digits.len() < 2 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut x = d as u32;
        if double {
            x *= 2;
            if x > 9 {
                x -= 9;
            }
        }
        sum += x;
        double = !double;
    }
    sum.is_multiple_of(10)
}

/// Checksum validation for China resident-ID-shaped matches (18 digits).
/// Last digit is a weighted checksum (GB 11643-1999).
fn china_id_valid(text: &str, start: usize, end: usize) -> bool {
    let s = match text.get(start..end) {
        Some(s) => s,
        None => return false,
    };
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 18 {
        return false;
    }
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    let mut sum = 0u32;
    for i in 0..17 {
        let d = match chars[i].to_digit(10) {
            Some(d) => d,
            None => return false,
        };
        sum += d * WEIGHTS[i];
    }
    let expected = CHECK[(sum % 11) as usize];
    chars[17].to_ascii_uppercase() == expected
}

/// db 连接串占位符豁免（2026-11-11 横扫批）：教程/模板形态
/// （`postgres://user:password@localhost`、compose 的 `${DB_USER}:${DB_PASSWORD}@db`、
/// 文档尖括号 `<user>:<password>@host`）不构成泄漏——判 false → demote Low
/// → 只 log 不 block。判定单一真相源在 `crate::credential::exemptions`
/// （豁免表单一真相源模块；Layer 4 四条 db_connection_* 规则同源豁免），
/// 两口径永不分裂。
fn connection_not_placeholder(text: &str, start: usize, end: usize) -> bool {
    match text.get(start..end) {
        Some(seg) => !crate::credential::exemptions::is_placeholder_db_credential(seg),
        None => false,
    }
}

/// authorization_header 占位符豁免（2026-11-11 二次横扫批；Bearer 臂散文
/// 闸为三次横扫批扩展）：Basic 臂复用 Layer 4 单一真相源
/// `is_placeholder_basic_auth`（散文长词 `Basic authentication`、教程示例
/// base64("user:password") 不构成泄漏 → demote Low 只 log）；Bearer 臂值
/// 段走 `is_prose_token_value`（无数字且非大小写混排 = 文档占位符
/// `your-bearer-token-here` 类 → demote）。从小写副本定位 scheme 词——
/// ASCII 小写化不改字节偏移，切片安全；Bearer token 内偶然含 "basic"/
/// "bearer" 子串时余段无空白、首词不再等于 scheme 词，对应判定返回
/// false → 判真占位不成立 → 恒过，无误降级。
fn authorization_not_placeholder(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    let lower = seg.to_ascii_lowercase();
    if let Some(pos) = lower.find("basic") {
        return !crate::credential::exemptions::is_placeholder_basic_auth(seg[pos..].trim_end());
    }
    if let Some(pos) = lower.find("bearer") {
        let after = &seg[pos + "bearer".len()..];
        return !after
            .split_whitespace()
            .next()
            .is_some_and(crate::credential::exemptions::is_prose_token_value);
    }
    true
}

/// keyed-assignment 值段形态闸（BUG-20261110-046）：值段必须是**引号
/// 字面量**（与 Layer 4 credential.rs secret_assignment 的 d405ff8b 修后
/// 形态对齐）。未加引号的值 = 代码表达式/标识符拷贝
///（`password = getpass.getpass(...)`、`password = value.get(...)`、
/// `pwd = home_dir`）而非密钥字面量——降级 Low（不 block；redact 面仍
/// 打码，裸值秘密出站掩码能力不回退）。2026-11-10 实录：只读 Jira
/// 脚本三处 `password = <函数调用>` 被 Medium→block 硬拦——Layer 4
/// 已修的同类误拦在 DLP 复发，因本族规则值段引号可选。
fn value_is_quoted(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    let Some(sep) = seg.find(['=', ':']) else {
        return false;
    };
    matches!(
        seg[sep + 1..].trim_start().chars().next(),
        Some('\'') | Some('"')
    )
}

/// 引号内值段提取（quoted_secret_not_tutorial 辅助）：首个引号与其配对
/// 引号之间的本体。赋值规则值段单行语义（同 Layer 4 secret_assignment
/// 的 d405ff8b 修后形态）保证配对成立；不成对返回 None。
fn extract_quoted_value(seg: &str) -> Option<&str> {
    let q = seg.chars().find(|c| *c == '"' || *c == '\'')?;
    let (_, after) = seg.split_once(q)?;
    after.split_once(q).map(|(value, _)| value)
}

/// secret_*_assignment 引号 + 占位符值复合双重闸（2026-11-11 三次横扫批
/// 立闸；机制化重构批换装 Layer 4 复合真相源）：值段无引号（既有
/// value_is_quoted 语义）**或**引号内是占位符值（`password = "your-password"`
/// / `"YOUR_PASSWORD_HERE"` / `"${DB_PASSWORD}"` / PEM 示意——
/// is_placeholder_secret_value 同源判定，五臂复合）→ demote Low 只 log。
/// 大小写混排/含数字的引号值保持 Medium 照拦。
fn quoted_secret_not_tutorial(text: &str, start: usize, end: usize) -> bool {
    if !value_is_quoted(text, start, end) {
        return false;
    }
    let Some(seg) = text.get(start..end) else {
        return true;
    };
    match extract_quoted_value(seg) {
        Some(value) => !crate::credential::exemptions::is_placeholder_secret_value(value),
        None => true,
    }
}

/// aws_access_key 文档 canonical + 占位符豁免（2026-11-11 三次横扫批立闸；
/// 机制化重构批扩占位符臂；2026-11-12 占位臂词表收窄）：匹配段含
/// "example"（AWS 官方文档 `AKIAIOSFODNN7EXAMPLE`）或 16 字符值段全同/
/// 占位词大写段（`AKIA` + `YOURACCESSKEY…` 教程形态）→ demote。判定
/// 单一真相源 `crate::credential::exemptions`（Layer 4 同源判据；词表
/// 收窄理由见 `is_screaming_vocab_placeholder`——base36 真实键全字母形态
/// 裸 SCREAMING 漏检 ≈0.66%）。
fn aws_access_not_doc_example(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    if exemptions::is_canonical_doc_example(seg) {
        return false; // 文档 canonical → 豁免（validator false = demote）
    }
    // 值段 = 4 字符前缀（AKIA/ASIA…）后的 16 字符。
    let value = seg.get(4..).unwrap_or(seg);
    !exemptions::is_uniform_char_value(value) && !exemptions::is_screaming_vocab_placeholder(value)
}

/// aws_secret_key 文档 canonical + 占位符豁免（同上批；机制化重构批扩占位
/// 符臂）：匹配段含 "example"（`wJalrXUtnFEMI/…EXAMPLEKEY`）或 `=`/`:` 后
/// 40 字符值段全同/SCREAMING 占位 → demote。
fn aws_secret_not_doc_example(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    if exemptions::is_canonical_doc_example(seg) {
        return false; // 文档 canonical → 豁免（validator false = demote）
    }
    // `key = VALUE` 分隔符后空格 trim 后再判（与 L4 TailAfter 同语义）。
    let Some(value) = seg.rsplit(['=', ':']).next().map(str::trim) else {
        return true;
    };
    !exemptions::is_uniform_char_value(value) && !exemptions::is_screaming_placeholder(value)
}

/// jwt_token 已知 demo 常量豁免（2026-11-11 机制化重构批）：jwt.io 首页
/// demo token（header/payload 通用 + 签名段 `SflKxwRJSMeKKF2QT4fwpMeJ…`）
/// 出现在每篇 JWT 教程——签名段精确等于知名常量 → demote（真实凭据与
/// 常量逐字节相等的概率为零）。L4 豁免表同源（KnownConstant 行）。
fn jwt_not_known_demo(text: &str, start: usize, end: usize) -> bool {
    !text
        .get(start..end)
        .is_some_and(|seg| seg.contains(exemptions::JWT_IO_DEMO_SIGNATURE))
}

/// stripe_key 文档测试 key + 占位符豁免（2026-11-11 机制化重构批，§7.3
/// 挂账「不可分辨」类的机制化精确豁免）：Stripe 官方文档测试 key
/// `4eC39HqLyjWDarjtT1zdp7dc` 出现在每篇 Stripe 教程——精确子串豁免；
/// `_` 后值段全同/SCREAMING 占位（`sk_test_YOURSTRIPEKEY…`）同豁免。
/// 随机形假钥仍拦（不可分辨，记录边界）。
fn stripe_not_docs_or_placeholder(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    if seg.contains(exemptions::STRIPE_DOCS_TEST_KEY) {
        return false; // 文档测试 key → 豁免（validator false = demote）
    }
    let Some(value) = seg.rsplit('_').next() else {
        return true;
    };
    !exemptions::is_uniform_char_value(value) && !exemptions::is_screaming_placeholder(value)
}

/// slack_token 文档占位符豁免（2026-11-11 机制化重构批）：末段（秘密承载
/// 段，`-` 分隔最后一段）全同或 SCREAMING 占位
///（`…-XXXX…` / `…-YOURSLACKTOKEN…` 教程形态）→ demote（返回 false）。
/// 中间数字段无判别力（真实团队 ID 与教程递增数字在「非全同非大写」上
/// 同形）不参与判定——占位信号承载在末段。真实混排末段恒过。
fn slack_not_docs_placeholder(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    let Some(value) = seg.rsplit('-').next().map(str::trim) else {
        return true;
    };
    !exemptions::is_uniform_char_value(value) && !exemptions::is_screaming_placeholder(value)
}

/// keyed 赋值规则（azure_api_key / generic_hex_key / generic_base64_key）
/// 值段散文豁免（2026-11-11 三次横扫批）：`azure_api_key =
/// YOUR_AZURE_SUBSCRIPTION_KEY...`、纯小写字母 hex 占位
///（`api_key = abcdef...` 32 字符无数字，hex 类内合法）类文档占位符 →
/// demote。值段取首个 `=`/`:` 分隔符尾段（与 value_is_quoted 同款提取；
/// 值类内无空白），判定单一真相源 `is_prose_token_value`（Layer 4 同源）。
fn token_value_not_prose(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    let Some(pos) = seg.find(['=', ':']) else {
        return true;
    };
    let value = seg[pos + 1..].trim_start();
    !exemptions::is_prose_token_value(value)
}

/// github_token 值段散文豁免（2026-11-11 三次横扫批）：`ghp_` 前缀后段
/// 单一大小写且无数字（`ghp_YOUR_GITHUB_PAT_TOKEN_HERE...` 类文档占位
/// 符）→ demote。前缀段大小写形态固定，判定必须落在值本体（splitn(2)
/// 使余段下划线完整保留在值段内）。
fn github_token_not_prose(text: &str, start: usize, end: usize) -> bool {
    let Some(seg) = text.get(start..end) else {
        return false;
    };
    seg.split_once('_')
        .is_none_or(|(_, value)| !exemptions::is_prose_token_value(value))
}

/// private_key 族 PEM 格式示意豁免（2026-11-11 三次横扫批）：marker 后
/// 300 字符窗口无 ≥48 字符连续 base64 段 = 格式示意文档（真实 PEM marker
/// 后紧跟 64 字符 base64 行）→ demote Low 只 log。判定单一真相源
/// `is_pem_illustration`（豁免表模块同源；validator 的 (text, start, end)
/// 签名天然支持窗口判定）。
fn pem_not_illustration(text: &str, _start: usize, end: usize) -> bool {
    !exemptions::is_pem_illustration(text, end)
}

fn get_dlp_rules() -> &'static DlpRules {
    static RULES: OnceLock<DlpRules> = OnceLock::new();
    RULES.get_or_init(|| {
        // (category, name, pattern, confidence, validator)
        let raw: Vec<(&'static str, &'static str, &'static str, DlpConfidence, Option<Validator>)> = vec![
            // Credit cards (6) — Medium, Luhn-validated (kills "any 16 digits" false positives)
            ("credit_card", "visa", r"\b4[0-9]{12}(?:[0-9]{3})?\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "mastercard", r"\b(?:5[1-5][0-9]{2}|222[1-9]|22[3-9][0-9]|2[3-6][0-9]{2}|27[01][0-9]|2720)[0-9]{12}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "amex", r"\b3[47][0-9]{13}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "discover", r"\b(?:6011|65[0-9]{2}|64[4-9][0-9]|622(?:12[6-9]|1[3-9][0-9]|[2-8][0-9]{2}|9[01][0-9]|92[0-5]))[0-9]{12}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "jcb", r"\b(?:352[89]|35[3-8][0-9])[0-9]{12}\b", DlpConfidence::Medium, Some(luhn_valid)),
            ("credit_card", "diners", r"\b(?:3(?:0[0-5]|[68][0-9]))[0-9]{11,13}\b", DlpConfidence::Medium, Some(luhn_valid)),

            // API keys and tokens (7) — High (fixed prefix + length, near-zero FP)
            // aws 文档 canonical + 占位符豁免（2026-11-11 三次横扫批立闸；
            // 机制化重构批拆两臂扩 uniform/screaming 判据）；keyed 值段散文
            // 豁免同批。
            ("credential", "aws_access_key", r"(?:A3T[A-Z0-9]|AKIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASIA)[A-Z0-9]{16}", DlpConfidence::High, Some(aws_access_not_doc_example)),
            ("credential", "aws_secret_key", r"(?i)aws[_\-]?secret[_\-]?access[_\-]?key\s*[=:]\s*[A-Za-z0-9/+=]{40}", DlpConfidence::High, Some(aws_secret_not_doc_example)),
            ("credential", "google_api_key", r"AIza[0-9A-Za-z\-_]{35}", DlpConfidence::High, None),
            ("credential", "google_oauth_token", r"ya29\.[0-9A-Za-z\-_]+", DlpConfidence::High, None),
            ("credential", "azure_api_key", r"(?i)azure[_\-]?(?:api|subscription)[_\-]?key\s*[=:]\s*[A-Za-z0-9\-_]{32,}", DlpConfidence::High, Some(token_value_not_prose)),
            ("credential", "generic_hex_key", r"(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[0-9a-f]{32,}", DlpConfidence::High, Some(token_value_not_prose)),
            ("credential", "generic_base64_key", r"(?i)(?:api[_\-]?key|apikey|secret|token|password|auth[_\-]?key)\s*[=:]\s*[A-Za-z0-9+/=]{40,}", DlpConfidence::High, Some(token_value_not_prose)),

            // Private keys (4) — High (fixed PEM header)
            // PEM 格式示意豁免（2026-11-11 三次横扫批）：marker 后无真实
            // base64 体 = 文档示意 → demote Low 只 log。
            ("credential", "private_key_rsa", r"-----BEGIN RSA PRIVATE KEY-----", DlpConfidence::High, Some(pem_not_illustration)),
            ("credential", "private_key_generic", r"-----BEGIN PRIVATE KEY-----", DlpConfidence::High, Some(pem_not_illustration)),
            ("credential", "private_key_openssh", r"-----BEGIN OPENSSH PRIVATE KEY-----", DlpConfidence::High, Some(pem_not_illustration)),
            ("credential", "private_key_pkcs8", r"-----BEGIN ENCRYPTED PRIVATE KEY-----", DlpConfidence::High, Some(pem_not_illustration)),

            // Personal IDs (4)
            ("pii", "us_ssn", r"\b[0-9]{3}-[0-9]{2}-[0-9]{4}\b", DlpConfidence::Medium, None),
            ("pii", "china_id", r"\b[1-9][0-9]{5}(?:19|20)[0-9]{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12][0-9]|3[01])[0-9]{3}[0-9Xx]\b", DlpConfidence::Medium, Some(china_id_valid)),
            ("pii", "email", r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b", DlpConfidence::Low, None),
            ("pii", "phone_international", r"(?:\+?\d{1,3}[\s\-.]?)?\(?\d{2,4}\)?[\s\-.]?\d{3,4}[\s\-.]?\d{3,4}", DlpConfidence::Low, None),

            // Network identifiers (3) — Low (false positives on version numbers, ICP numbers, etc.)
            ("network", "ip_address_private", r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3})\b", DlpConfidence::Low, None),
            ("network", "ip_address_public", r"\b(?:[1-9]\d?|1\d\d|2[01]\d|22[0-3])(?:\.\d{1,3}){3}\b", DlpConfidence::Low, None),
            ("network", "ip_address_ipv6", r"(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}", DlpConfidence::Low, None),

            // Financial: Bank accounts (2) — Medium (keyed)
            ("financial", "bank_account_number", r"\b(?:account[_\s\-]?number|acct|iban|swift|bic)\s*[=:]\s*[A-Z0-9]{8,17}\b", DlpConfidence::Medium, None),
            ("financial", "iban", r"\b[A-Z]{2}[0-9]{2}[A-Z0-9]{4}[0-9]{7}(?:[A-Z0-9]?){0,16}\b", DlpConfidence::Medium, None),

            // Tokens and connection strings (5) — High (fixed prefix/structure)
            // jwt/stripe/slack 占位与知名常量豁免（2026-11-11 机制化重构
            // 批，§7.3 挂账「不可分辨」类的机制化精确豁免 + 教程占位判据）。
            ("credential", "jwt_token", r"\beyJ[A-Za-z0-9\-_]+\.eyJ[A-Za-z0-9\-_]+\.[A-Za-z0-9\-_]+\b", DlpConfidence::High, Some(jwt_not_known_demo)),
            // 凭据段无界（教程 canonical user:password@ 形态误拦面）——
            // 占位符豁免走 connection_not_placeholder（2026-11-11 横扫批，
            // 与 Layer 4 db_connection_* 同源）。
            ("credential", "database_connection_string", r#"(?i)(?:mysql|postgres|postgresql|mongodb|redis|mssql|sqlserver|oracle)://[^\s'"]+:[^\s'"]+@[^\s'"]+"#, DlpConfidence::High, Some(connection_not_placeholder)),
            // 值段散文豁免（2026-11-11 三次横扫批，Layer 4 github_token
            // 同源判据）：ghp_ 前缀后段单一大小写无数字 = 文档占位符。
            ("credential", "github_token", r"gh[ps]_[A-Za-z0-9_]{36,}", DlpConfidence::High, Some(github_token_not_prose)),
            ("credential", "slack_token", r"xox[bopsa]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,34}", DlpConfidence::High, Some(slack_not_docs_placeholder)),
            ("credential", "stripe_key", r"(?:sk|pk)_(?:test_|live_)[A-Za-z0-9]{24,}", DlpConfidence::High, Some(stripe_not_docs_or_placeholder)),

            // Generic secrets patterns (4) — Medium (keyed assignment).
            // 三条赋值规则挂 quoted_secret_not_tutorial 双重闸（BUG-20261110-046
            // 引号闸 + 2026-11-11 三次横扫批教程复合词闸，Layer 4
            // secret_assignment 同源）：值段无引号（代码表达式/裸标识符）
            // 或引号内教程复合词（"your-password" 类）降级 Low 不 block；
            // 其余引号字面量保持 Medium 照拦。
            ("credential", "secret_password_assignment", r#"(?i)(?:password|passwd|pwd)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#, DlpConfidence::Medium, Some(quoted_secret_not_tutorial)),
            ("credential", "secret_token_assignment", r#"(?i)(?:token|bearer|access[_\-]?token|auth[_\-]?token|refresh[_\-]?token)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#, DlpConfidence::Medium, Some(quoted_secret_not_tutorial)),
            ("credential", "secret_key_assignment", r#"(?i)(?:secret[_\-]?key|client[_\-]?secret|shared[_\-]?secret|encryption[_\-]?key)\s*[=:]\s*['"]?[^\s'"]{8,}['"]?"#, DlpConfidence::Medium, Some(quoted_secret_not_tutorial)),
            // authorization 值段长度下限与 Layer 4 bearer_token/basic_auth
            // 同源同语义（2026-11-11 实录同族）：无下限时「Authorization:
            // Basic mode,」散文/文档形态整行命中 Medium→block。真实 Bearer
            // token ≥20；Basic base64(user:password) ≥12——二合一规则取宽
            // 闸 20（Basic 12-19 短样本漏检面由 Layer 4 basic_auth 覆盖）。
            ("credential", "authorization_header", r"(?i)authorization\s*:\s*(?:bearer|basic)\s+[A-Za-z0-9\-_.~+/]{20,}=*", DlpConfidence::Medium, Some(authorization_not_placeholder)),
        ];

        raw.into_iter()
            .filter_map(|(cat, name, pattern, confidence, validator)| {
                Regex::new(pattern).ok().map(|re| StaticRule {
                    category: cat,
                    name,
                    re,
                    confidence,
                    validator,
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
