//! Injection Detection - Layer 1
//! Detects prompt injection, jailbreak, and role escape patterns.
//! 50+ patterns across 5 categories with configurable classifier.

use crate::classifier::Classifier;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Injection detection result with detailed analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub is_injection: bool,
    pub score: f64,
    pub level: String,
    pub matched_patterns: Vec<PatternMatch>,
    pub recommendation: String,
    pub summary: String,
    pub strict_violations: Vec<String>,
}

/// A single pattern match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternMatch {
    /// Name/identifier of the pattern that matched.
    pub pattern_name: String,
    /// Category of the pattern (e.g., "jailbreak", "role_escape").
    pub category: String,
    /// The text that was matched by the pattern.
    pub matched_text: String,
    /// The weight/score contribution of this pattern.
    pub weight: f64,
    /// Byte offset of the match within the input.
    pub position: usize,
}

/// Legacy injection result (backward compat).
#[derive(Debug, Clone)]
pub struct InjectionResult {
    pub is_injection: bool,
    pub score: f64,
    pub level: String,
    pub matched_patterns: Vec<String>,
}

/// Injection detector configuration.
#[derive(Debug, Clone)]
pub struct InjectionConfig {
    pub enabled: bool,
    pub threshold: f64,
    pub max_input_length: usize,
    pub strict_mode: bool,
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.7,
            max_input_length: 100_000,
            strict_mode: false,
        }
    }
}

/// Injection pattern categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionCategory {
    Jailbreak,
    RoleEscape,
    DataExtraction,
    CommandInjection,
    Encoding,
}

impl std::fmt::Display for InjectionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jailbreak => write!(f, "jailbreak"),
            Self::RoleEscape => write!(f, "role_escape"),
            Self::DataExtraction => write!(f, "data_extraction"),
            Self::CommandInjection => write!(f, "command_injection"),
            Self::Encoding => write!(f, "encoding"),
        }
    }
}

/// Scoring factors for the classifier.
#[derive(Debug, Clone, Default)]
pub struct ScoringFactors {
    pub pattern_count: usize,
    pub max_single_weight: f64,
    pub category_diversity: usize,
    pub input_length_factor: f64,
    pub repetition_factor: f64,
}

/// A compiled custom pattern.
struct CompiledPattern {
    name: String,
    regex: Regex,
    category: InjectionCategory,
    weight: f64,
}

/// Injection detector with classifier.
pub struct Detector {
    config: parking_lot::RwLock<InjectionConfig>,
    /// Optional custom patterns (when created with `with_patterns`).
    custom_patterns: Option<Vec<CompiledPattern>>,
    /// Heuristic classifier for combined scoring (mirrors Go's `Detector.classifier`).
    classifier: Classifier,
}

impl Detector {
    pub fn new(config: InjectionConfig) -> Self {
        Self {
            config: parking_lot::RwLock::new(config),
            custom_patterns: None,
            classifier: Classifier::new(),
        }
    }

    /// Create a detector with custom regex patterns.
    ///
    /// Each pattern string is compiled into a regex. Uncompilable patterns are
    /// silently skipped (matching Go's `NewDetectorWithPatterns` behavior).
    /// Custom patterns are assigned a default weight of 0.7 and category of
    /// `InjectionCategory::CommandInjection`.
    pub fn with_patterns(config: InjectionConfig, patterns: &[String]) -> Self {
        let compiled: Vec<CompiledPattern> = patterns
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                Regex::new(p).ok().map(|re| CompiledPattern {
                    name: format!("custom_{}", i),
                    regex: re,
                    category: InjectionCategory::CommandInjection,
                    weight: 0.7,
                })
            })
            .collect();

        Self {
            config: parking_lot::RwLock::new(config),
            custom_patterns: if compiled.is_empty() {
                None
            } else {
                Some(compiled)
            },
            classifier: Classifier::new(),
        }
    }

    /// Update the detector configuration dynamically.
    ///
    /// Thread-safe: acquires an internal lock to swap the config.
    /// Mirrors Go's `Detector.UpdateConfig()`.
    pub fn update_config(&self, config: InjectionConfig) {
        *self.config.write() = config;
    }

    /// Analyze tool input for injection patterns (legacy interface).
    ///
    /// Uses both pattern-based scoring (65%) AND the Classifier's heuristic
    /// analysis (35%) — matching Go's `AnalyzeToolInput` behavior.
    ///
    /// M18+M19: When strict mode is enabled AND the tool is high-risk
    /// (file_write, process_exec, shell_exec, exec), the detection threshold
    /// is lowered by 30% (minimum 0.3).
    pub fn analyze_tool_input(&self, tool_name: &str, args: &serde_json::Value) -> InjectionResult {
        let input = extract_all_text(args);
        if input.is_empty() {
            return InjectionResult {
                is_injection: false,
                score: 0.0,
                level: "none".to_string(),
                matched_patterns: vec![],
            };
        }

        let cfg = self.config.read();
        if input.len() > cfg.max_input_length {
            return InjectionResult {
                is_injection: false,
                score: 0.0,
                level: "none".to_string(),
                matched_patterns: vec![],
            };
        }

        // M18+M19: Lower threshold for high-risk tools in strict mode
        let effective_threshold = if cfg.strict_mode && Self::is_high_risk_tool(tool_name) {
            (cfg.threshold * 0.7).max(0.3)
        } else {
            cfg.threshold
        };

        // 1. Pattern-based scoring
        let patterns = get_injection_patterns();
        let mut matched = Vec::new();
        let mut pattern_raw_score = 0.0;

        for (category, re, weight) in patterns {
            if re.is_match(&input) {
                matched.push(format!("{}: {}", category, re.as_str()));
                pattern_raw_score += weight;
            }
        }

        // Also check custom patterns if present
        if let Some(ref custom) = self.custom_patterns {
            for cp in custom {
                if let Some(_mat) = cp.regex.find(&input) {
                    matched.push(format!("{}: {}", cp.category, cp.regex.as_str()));
                    pattern_raw_score += cp.weight;
                }
            }
        }

        // Normalize pattern score with sigmoid-like diminishing returns
        let pattern_score = if pattern_raw_score > 0.0 {
            pattern_raw_score / (pattern_raw_score + 1.0)
        } else {
            0.0
        };

        // 2. Classifier heuristic scoring
        let classifier_result = self.classifier.classify(&input);
        let classifier_score = classifier_result.score;

        // 3. Combine: 65% pattern + 35% classifier
        let score = (0.65 * pattern_score + 0.35 * classifier_score).min(1.0);
        let is_injection = score >= effective_threshold;
        let level = if score >= 0.9 {
            "critical"
        } else if score >= 0.7 {
            "high"
        } else if score >= 0.5 {
            "medium"
        } else {
            "low"
        };

        let _ = tool_name; // used above for high-risk check

        InjectionResult {
            is_injection,
            score,
            level: level.to_string(),
            matched_patterns: matched,
        }
    }

    /// Detailed analysis with scoring factors, recommendation, and strict mode.
    pub fn analyze_detailed(&self, _tool_name: &str, args: &serde_json::Value) -> AnalysisResult {
        let input = extract_all_text(args);

        let cfg = self.config.read();
        if input.is_empty() || input.len() > cfg.max_input_length {
            return AnalysisResult {
                is_injection: false,
                score: 0.0,
                level: "none".to_string(),
                matched_patterns: vec![],
                recommendation: "Input is empty or too long for analysis.".to_string(),
                summary: "No analysis needed.".to_string(),
                strict_violations: vec![],
            };
        }

        let patterns = get_injection_patterns();
        let mut matched: Vec<PatternMatch> = Vec::new();
        let mut total_score = 0.0;
        let mut categories: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (category, re, weight) in patterns {
            if let Some(mat) = re.find(&input) {
                let matched_text = mat.as_str();
                let position = mat.start();
                categories.insert(category.to_string());
                matched.push(PatternMatch {
                    pattern_name: format!("{}/{}", category, re.as_str()),
                    category: category.to_string(),
                    matched_text: matched_text.chars().take(120).collect(),
                    weight: *weight,
                    position,
                });
                total_score += weight;
            }
        }

        // Also check custom patterns if present
        if let Some(ref custom) = self.custom_patterns {
            for cp in custom {
                if let Some(mat) = cp.regex.find(&input) {
                    let matched_text = mat.as_str();
                    let position = mat.start();
                    categories.insert(cp.category.to_string());
                    matched.push(PatternMatch {
                        pattern_name: cp.name.clone(),
                        category: cp.category.to_string(),
                        matched_text: matched_text.chars().take(120).collect(),
                        weight: cp.weight,
                        position,
                    });
                    total_score += cp.weight;
                }
            }
        }

        // Calculate scoring factors
        let factors = ScoringFactors {
            pattern_count: matched.len(),
            max_single_weight: matched.iter().map(|m| m.weight).fold(0.0_f64, f64::max),
            category_diversity: categories.len(),
            input_length_factor: if input.len() < 50 { 0.9 } else { 1.0 },
            repetition_factor: 1.0, // Could be enhanced with repetition detection
        };

        // Boost score based on category diversity
        let diversity_boost = 1.0 + (factors.category_diversity as f64 - 1.0) * 0.05;
        let score = (total_score
            * factors.input_length_factor
            * factors.repetition_factor
            * diversity_boost)
            .min(1.0);

        let is_injection = score >= cfg.threshold;
        let level = if score >= 0.9 {
            "critical"
        } else if score >= 0.7 {
            "high"
        } else if score >= 0.5 {
            "medium"
        } else {
            "low"
        };

        // Strict mode violations
        let mut strict_violations = Vec::new();
        if cfg.strict_mode {
            // In strict mode, any single high-weight pattern is a violation
            for m in &matched {
                if m.weight >= 0.7 {
                    strict_violations.push(format!(
                        "{} pattern detected: {}",
                        m.category, m.pattern_name
                    ));
                }
            }
        }

        let recommendation = if is_injection {
            format!(
                "Block this input. {} injection pattern(s) detected across {} category/categories. Risk level: {}.",
                matched.len(),
                categories.len(),
                level
            )
        } else if score > 0.3 {
            "Input is suspicious but below threshold. Consider manual review.".to_string()
        } else {
            "Input appears safe.".to_string()
        };

        let summary = if matched.is_empty() {
            "No injection patterns detected.".to_string()
        } else {
            let cat_summary: Vec<String> = categories.into_iter().collect();
            format!(
                "Detected {} pattern(s) in {} category/categories: {}",
                matched.len(),
                cat_summary.len(),
                cat_summary.join(", ")
            )
        };

        AnalysisResult {
            is_injection,
            score,
            level: level.to_string(),
            matched_patterns: matched,
            recommendation,
            summary,
            strict_violations,
        }
    }

    /// Analyze free-form text for injection patterns.
    ///
    /// Equivalent to Go's `Analyze(text)`. Returns an InjectionResult for the given text.
    /// Uses both pattern-based scoring (65%) AND the Classifier's heuristic analysis (35%).
    pub fn analyze(&self, text: &str) -> InjectionResult {
        let cfg = self.config.read();
        if text.is_empty() || text.len() > cfg.max_input_length {
            return InjectionResult {
                is_injection: false,
                score: 0.0,
                level: "none".to_string(),
                matched_patterns: vec![],
            };
        }

        // 1. Pattern-based scoring
        let patterns = get_injection_patterns();
        let mut matched = Vec::new();
        let mut pattern_raw_score = 0.0;

        for (category, re, weight) in patterns {
            if re.is_match(text) {
                matched.push(format!("{}: {}", category, re.as_str()));
                pattern_raw_score += weight;
            }
        }

        // Also check custom patterns if present
        if let Some(ref custom) = self.custom_patterns {
            for cp in custom {
                if cp.regex.is_match(text) {
                    matched.push(format!("{}: {}", cp.category, cp.regex.as_str()));
                    pattern_raw_score += cp.weight;
                }
            }
        }

        // Normalize pattern score with sigmoid-like diminishing returns
        let pattern_score = if pattern_raw_score > 0.0 {
            pattern_raw_score / (pattern_raw_score + 1.0)
        } else {
            0.0
        };

        // 2. Classifier heuristic scoring
        let classifier_result = self.classifier.classify(text);
        let classifier_score = classifier_result.score;

        // 3. Combine: 65% pattern + 35% classifier
        let score = (0.65 * pattern_score + 0.35 * classifier_score).min(1.0);
        let is_injection = score >= cfg.threshold;
        let level = if score >= 0.9 {
            "critical"
        } else if score >= 0.7 {
            "high"
        } else if score >= 0.5 {
            "medium"
        } else {
            "low"
        };

        InjectionResult {
            is_injection,
            score,
            level: level.to_string(),
            matched_patterns: matched,
        }
    }

    /// Combine raw score with pattern count using sigmoid normalization.
    ///
    /// 65% weight on raw score + 35% weight on pattern count (normalized).
    pub fn combine_scores(raw_score: f64, pattern_count: usize) -> f64 {
        let normalized_raw = 1.0 / (1.0 + (-raw_score + 2.0).exp()); // sigmoid
        let count_factor = (pattern_count as f64 / 10.0).min(1.0);
        (0.65 * normalized_raw + 0.35 * count_factor).min(1.0)
    }

    /// Check if a tool is considered high-risk (lower threshold for injection detection).
    pub fn is_high_risk_tool(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "exec"
                | "shell_exec"
                | "process_exec"
                | "write_file"
                | "file_write"
                | "file_edit"
                | "file_append"
                | "shell"
                | "download"
                | "http_request"
        )
    }
}

/// Create a default injection config.
/// Mirrors Go's `DefaultConfig()`.
pub fn default_config() -> InjectionConfig {
    InjectionConfig::default()
}

/// Get the list of default injection pattern strings.
/// Mirrors Go's `DefaultPatterns()` — returns (category, pattern_string, weight) tuples.
pub fn default_patterns() -> Vec<(InjectionCategory, String, f64)> {
    let raw: Vec<(InjectionCategory, &str, f64)> = vec![
        // === Jailbreak patterns (15) ===
        (
            InjectionCategory::Jailbreak,
            r"(?i)ignore\s+(all\s+)?previous\s+instructions",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)forget\s+(all\s+)?previous",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)you\s+are\s+now\s+(DAN|jailbroken|unlocked)",
            0.9,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)bypass\s+(your|the)\s+(safety|security|filter)",
            0.9,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)pretend\s+you\s+(are|have)\s+no\s+(rules|restrictions)",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)disregard\s+(all\s+)?(your|the)\s+(rules|guidelines)",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)you\s+are\s+(no\s+longer|not)\s+(bound|limited)\s+by",
            0.7,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)enable\s+(developer|god|admin)\s+mode",
            0.9,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)jailbreak\s+(the\s+)?(bot|ai|model|system)",
            0.9,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)switch\s+to\s+(unrestricted|unfiltered)\s+mode",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)override\s+(safety|security|content)\s+(filter|policy|guidelines)",
            0.9,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)do\s+anything\s+now\b",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i) bypass\s+openai\s+",
            0.8,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i) no\s+restrictions\s+mode",
            0.7,
        ),
        (
            InjectionCategory::Jailbreak,
            r"(?i)answer\s+without\s+(any\s+)?(restriction|limit|filter)",
            0.7,
        ),
        // === Role escape patterns (12) ===
        (InjectionCategory::RoleEscape, r"(?i)system\s*:\s*", 0.7),
        (InjectionCategory::RoleEscape, r"(?i)\[system\]", 0.6),
        (InjectionCategory::RoleEscape, r"(?i)<\|im_start\|>", 0.9),
        (
            InjectionCategory::RoleEscape,
            r"(?i)act\s+as\s+(if\s+you\s+(are|were)|an?\s+(unrestricted|admin))",
            0.7,
        ),
        (
            InjectionCategory::RoleEscape,
            r"(?i)you\s+are\s+(not|no\s+longer)\s+an?\s+(AI|assistant|bot)",
            0.8,
        ),
        (
            InjectionCategory::RoleEscape,
            r"(?i)new\s+instructions?\s*:",
            0.7,
        ),
        (
            InjectionCategory::RoleEscape,
            r"(?i)from\s+now\s+on[,.]?\s+you\s+(will|are|shall)",
            0.6,
        ),
        (InjectionCategory::RoleEscape, r"(?i)<\|endoftext\|>", 0.9),
        (InjectionCategory::RoleEscape, r"(?i)\[INST\]", 0.7),
        (InjectionCategory::RoleEscape, r"(?i)###\s*system", 0.6),
        (
            InjectionCategory::RoleEscape,
            r"(?i)role\s*:\s*(system|admin|root|superuser)",
            0.8,
        ),
        (
            InjectionCategory::RoleEscape,
            r"(?i)ignore\s+role\s*(?:restrictions|assignment)",
            0.7,
        ),
        // === Data extraction patterns (10) ===
        (
            InjectionCategory::DataExtraction,
            r"(?i)reveal\s+(your|the)\s+(system\s+)?prompt",
            0.8,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)show\s+me\s+(your|the)\s+(original|initial)\s+(instructions|prompt)",
            0.8,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)what\s+(are|were)\s+your\s+original\s+instructions",
            0.7,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)print\s+(your|the)\s+(system|initial|hidden)\s+prompt",
            0.8,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)repeat\s+(?:the\s+)?(?:words|text)\s+above",
            0.6,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)(?:output|display|echo|print)\s+(?:your|the)\s+(?:initial|original|hidden)\s+(?:instructions|prompt)",
            0.8,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)what\s+is\s+your\s+(system|hidden|secret)\s+prompt",
            0.8,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)(?:dump|export|extract)\s+(?:your|the)\s+(?:system|config|prompt)",
            0.7,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)(?:tell|inform)\s+me\s+(?:about|of)\s+your\s+(?:training|rules|constraints)",
            0.6,
        ),
        (
            InjectionCategory::DataExtraction,
            r"(?i)copy\s+(?:all|the)\s+(?:text|instructions)\s+(?:above|before)",
            0.6,
        ),
        // === Command injection patterns (25) ===
        (
            InjectionCategory::CommandInjection,
            r"(?i);\s*(rm|del|format|shutdown|reboot)",
            0.9,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)\|\s*(sh|bash|powershell|cmd)",
            0.8,
        ),
        (InjectionCategory::CommandInjection, r"(?i)`[^`]*`", 0.5),
        (InjectionCategory::CommandInjection, r"(?i)\$\([^)]*\)", 0.6),
        (
            InjectionCategory::CommandInjection,
            r"(?i)\\x[0-9a-f]{2}",
            0.5,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)\\u[0-9a-f]{4}",
            0.4,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)\\x[0-9a-f]{2}\\x[0-9a-f]{2}",
            0.6,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)%[0-9a-f]{2}%[0-9a-f]{2}",
            0.5,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)<script[^>]*>",
            0.7,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)javascript\s*:",
            0.7,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)on(error|load|click|mouseover)\s*=",
            0.6,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)data\s*:\s*text/html",
            0.5,
        ),
        (
            InjectionCategory::CommandInjection,
            r"(?i)(?:curl|wget)\s+.*\|\s*(?:sh|bash|python|perl)",
            0.9,
        ),
        // SQL injection
        (
            InjectionCategory::CommandInjection,
            r"(?i);\s*(drop|alter|truncate|delete\s+from)\s+(table|database|index)",
            0.95,
        ),
        // Log4Shell / JNDI injection
        (
            InjectionCategory::CommandInjection,
            r"\$\{jndi:(ldap|rmi|dns|nds|corba|iiop):",
            0.95,
        ),
        // XXE injection
        (
            InjectionCategory::CommandInjection,
            r"(?i)<!entity\s+",
            0.88,
        ),
        // SSTI (server-side template injection)
        (
            InjectionCategory::CommandInjection,
            r"\{\{.*?\.(class|mro|subclasses|bases|init|globals)\b",
            0.90,
        ),
        // LDAP injection
        (
            InjectionCategory::CommandInjection,
            r"\)\s*\(\s*\|\s*\(",
            0.78,
        ),
        // Null byte injection
        (InjectionCategory::CommandInjection, r"\\x00|%00|\\0", 0.80),
        // Path traversal (deep)
        (InjectionCategory::CommandInjection, r"\.\./\.\./", 0.88),
        // Format string exploit
        (
            InjectionCategory::CommandInjection,
            r"%s%s%s%s%s|%n%n%n%n",
            0.82,
        ),
        // JS prototype pollution
        (
            InjectionCategory::CommandInjection,
            r"\{\{constructor",
            0.88,
        ),
        // Env var injection
        (InjectionCategory::CommandInjection, r"\$\{env[\s:]", 0.85),
    ];
    raw.into_iter()
        .map(|(cat, pattern, weight)| (cat, pattern.to_string(), weight))
        .collect()
}

fn extract_all_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map
            .values()
            .map(extract_all_text)
            .collect::<Vec<_>>()
            .join(" "),
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(extract_all_text)
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

type PatternList = Vec<(InjectionCategory, Regex, f64)>;

fn get_injection_patterns() -> &'static PatternList {
    static PATTERNS: OnceLock<PatternList> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw: Vec<(InjectionCategory, &str, f64)> = vec![
            // === Jailbreak patterns (15) ===
            (InjectionCategory::Jailbreak, r"(?i)ignore\s+(all\s+)?previous\s+instructions", 0.8),
            (InjectionCategory::Jailbreak, r"(?i)forget\s+(all\s+)?previous", 0.8),
            (InjectionCategory::Jailbreak, r"(?i)you\s+are\s+now\s+(DAN|jailbroken|unlocked)", 0.9),
            (InjectionCategory::Jailbreak, r"(?i)bypass\s+(your|the)\s+(safety|security|filter)", 0.9),
            (InjectionCategory::Jailbreak, r"(?i)pretend\s+you\s+(are|have)\s+no\s+(rules|restrictions)", 0.8),
            (InjectionCategory::Jailbreak, r"(?i)disregard\s+(all\s+)?(your|the)\s+(rules|guidelines)", 0.8),
            (InjectionCategory::Jailbreak, r"(?i)you\s+are\s+(no\s+longer|not)\s+(bound|limited)\s+by", 0.7),
            (InjectionCategory::Jailbreak, r"(?i)enable\s+(developer|god|admin)\s+mode", 0.9),
            (InjectionCategory::Jailbreak, r"(?i)jailbreak\s+(the\s+)?(bot|ai|model|system)", 0.9),
            (InjectionCategory::Jailbreak, r"(?i)switch\s+to\s+(unrestricted|unfiltered)\s+mode", 0.8),
            (InjectionCategory::Jailbreak, r"(?i)override\s+(safety|security|content)\s+(filter|policy|guidelines)", 0.9),
            (InjectionCategory::Jailbreak, r"(?i)do\s+anything\s+now\b", 0.8),
            (InjectionCategory::Jailbreak, r"(?i) bypass\s+openai\s+", 0.8),
            (InjectionCategory::Jailbreak, r"(?i) no\s+restrictions\s+mode", 0.7),
            (InjectionCategory::Jailbreak, r"(?i)answer\s+without\s+(any\s+)?(restriction|limit|filter)", 0.7),

            // === Role escape patterns (12) ===
            (InjectionCategory::RoleEscape, r"(?i)system\s*:\s*", 0.7),
            (InjectionCategory::RoleEscape, r"(?i)\[system\]", 0.6),
            (InjectionCategory::RoleEscape, r"(?i)<\|im_start\|>", 0.9),
            (InjectionCategory::RoleEscape, r"(?i)act\s+as\s+(if\s+you\s+(are|were)|an?\s+(unrestricted|admin))", 0.7),
            (InjectionCategory::RoleEscape, r"(?i)you\s+are\s+(not|no\s+longer)\s+an?\s+(AI|assistant|bot)", 0.8),
            (InjectionCategory::RoleEscape, r"(?i)new\s+instructions?\s*:", 0.7),
            (InjectionCategory::RoleEscape, r"(?i)from\s+now\s+on[,.]?\s+you\s+(will|are|shall)", 0.6),
            (InjectionCategory::RoleEscape, r"(?i)<\|endoftext\|>", 0.9),
            (InjectionCategory::RoleEscape, r"(?i)\[INST\]", 0.7),
            (InjectionCategory::RoleEscape, r"(?i)###\s*system", 0.6),
            (InjectionCategory::RoleEscape, r"(?i)role\s*:\s*(system|admin|root|superuser)", 0.8),
            (InjectionCategory::RoleEscape, r"(?i)ignore\s+role\s*(?:restrictions|assignment)", 0.7),

            // === Data extraction patterns (10) ===
            (InjectionCategory::DataExtraction, r"(?i)reveal\s+(your|the)\s+(system\s+)?prompt", 0.8),
            (InjectionCategory::DataExtraction, r"(?i)show\s+me\s+(your|the)\s+(original|initial)\s+(instructions|prompt)", 0.8),
            (InjectionCategory::DataExtraction, r"(?i)what\s+(are|were)\s+your\s+original\s+instructions", 0.7),
            (InjectionCategory::DataExtraction, r"(?i)print\s+(your|the)\s+(system|initial|hidden)\s+prompt", 0.8),
            (InjectionCategory::DataExtraction, r"(?i)repeat\s+(?:the\s+)?(?:words|text)\s+above", 0.6),
            (InjectionCategory::DataExtraction, r"(?i)(?:output|display|echo|print)\s+(?:your|the)\s+(?:initial|original|hidden)\s+(?:instructions|prompt)", 0.8),
            (InjectionCategory::DataExtraction, r"(?i)what\s+is\s+your\s+(system|hidden|secret)\s+prompt", 0.8),
            (InjectionCategory::DataExtraction, r"(?i)(?:dump|export|extract)\s+(?:your|the)\s+(?:system|config|prompt)", 0.7),
            (InjectionCategory::DataExtraction, r"(?i)(?:tell|inform)\s+me\s+(?:about|of)\s+your\s+(?:training|rules|constraints)", 0.6),
            (InjectionCategory::DataExtraction, r"(?i)copy\s+(?:all|the)\s+(?:text|instructions)\s+(?:above|before)", 0.6),

            // === Command injection patterns (25) ===
            (InjectionCategory::CommandInjection, r"(?i);\s*(rm|del|format|shutdown|reboot)", 0.9),
            (InjectionCategory::CommandInjection, r"(?i)\|\s*(sh|bash|powershell|cmd)", 0.8),
            (InjectionCategory::CommandInjection, r"(?i)`[^`]*`", 0.5),
            (InjectionCategory::CommandInjection, r"(?i)\$\([^)]*\)", 0.6),
            (InjectionCategory::CommandInjection, r"(?i)\\x[0-9a-f]{2}", 0.5),
            (InjectionCategory::CommandInjection, r"(?i)\\u[0-9a-f]{4}", 0.4),
            (InjectionCategory::CommandInjection, r"(?i)\\x[0-9a-f]{2}\\x[0-9a-f]{2}", 0.6),
            (InjectionCategory::CommandInjection, r"(?i)%[0-9a-f]{2}%[0-9a-f]{2}", 0.5),
            (InjectionCategory::CommandInjection, r"(?i)<script[^>]*>", 0.7),
            (InjectionCategory::CommandInjection, r"(?i)javascript\s*:", 0.7),
            (InjectionCategory::CommandInjection, r"(?i)on(error|load|click|mouseover)\s*=", 0.6),
            (InjectionCategory::CommandInjection, r"(?i)data\s*:\s*text/html", 0.5),
            (InjectionCategory::CommandInjection, r"(?i)(?:curl|wget)\s+.*\|\s*(?:sh|bash|python|perl)", 0.9),
            // SQL injection
            (InjectionCategory::CommandInjection, r"(?i);\s*(drop|alter|truncate|delete\s+from)\s+(table|database|index)", 0.95),
            // Log4Shell / JNDI injection
            (InjectionCategory::CommandInjection, r"\$\{jndi:(ldap|rmi|dns|nds|corba|iiop):", 0.95),
            // XXE injection
            (InjectionCategory::CommandInjection, r"(?i)<!entity\s+", 0.88),
            // SSTI (server-side template injection)
            (InjectionCategory::CommandInjection, r"\{\{.*?\.(class|mro|subclasses|bases|init|globals)\b", 0.90),
            // LDAP injection
            (InjectionCategory::CommandInjection, r"\)\s*\(\s*\|\s*\(", 0.78),
            // Null byte injection
            (InjectionCategory::CommandInjection, r"\\x00|%00|\\0", 0.80),
            // Path traversal (deep)
            (InjectionCategory::CommandInjection, r"\.\./\.\./", 0.88),
            // Format string exploit
            (InjectionCategory::CommandInjection, r"%s%s%s%s%s|%n%n%n%n", 0.82),
            // JS prototype pollution
            (InjectionCategory::CommandInjection, r"\{\{constructor", 0.88),
            // Env var injection
            (InjectionCategory::CommandInjection, r"\$\{env[\s:]", 0.85),
        ];

        raw.into_iter()
            .filter_map(|(cat, pattern, weight)| {
                Regex::new(pattern).ok().map(|re| (cat, re, weight))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;
