//! Injection Classifier - heuristic-based injection scoring.
//!
//! Uses keyword density, entropy analysis, structural indicators,
//! repetition detection, and instruction structure scoring.

/// A single factor that contributed to the classification score.
#[derive(Debug, Clone)]
pub struct ScoreFactor {
    pub name: String,
    pub value: f64,
    pub desc: String,
}

/// Output of the classifier.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// Overall score 0.0-1.0.
    pub score: f64,
    /// "clean", "suspicious", or "malicious".
    pub level: String,
    /// Individual factors.
    pub factors: Vec<ScoreFactor>,
}

/// Heuristic injection classifier.
pub struct Classifier {
    keywords: Vec<(String, f64)>,
}

impl Classifier {
    /// Create a classifier with the default keyword set.
    pub fn new() -> Self {
        Self {
            keywords: default_keywords(),
        }
    }

    /// Classify input text and return a result.
    pub fn classify(&self, input: &str) -> ClassificationResult {
        let mut factors = Vec::new();

        // Factor 1: keyword density
        let (kw_score, kw_desc) = self.keyword_density(input);
        factors.push(ScoreFactor {
            name: "keyword_density".to_string(),
            value: kw_score,
            desc: kw_desc,
        });

        // Factor 2: entropy
        let (ent_score, ent_desc) = entropy_score(input);
        factors.push(ScoreFactor {
            name: "entropy".to_string(),
            value: ent_score,
            desc: ent_desc,
        });

        // Factor 3: structural
        let (struct_score, struct_desc) = structural_score(input);
        factors.push(ScoreFactor {
            name: "structural".to_string(),
            value: struct_score,
            desc: struct_desc,
        });

        // Factor 4: repetition
        let (rep_score, rep_desc) = repetition_score(input);
        factors.push(ScoreFactor {
            name: "repetition".to_string(),
            value: rep_score,
            desc: rep_desc,
        });

        // Factor 5: instruction structure
        let (instr_score, instr_desc) = instruction_structure_score(input);
        factors.push(ScoreFactor {
            name: "instruction_structure".to_string(),
            value: instr_score,
            desc: instr_desc,
        });

        let total: f64 = (0.30 * kw_score
            + 0.15 * ent_score
            + 0.20 * struct_score
            + 0.15 * rep_score
            + 0.20 * instr_score)
            .min(1.0_f64);

        let level = if total >= 0.7 {
            "malicious"
        } else if total >= 0.4 {
            "suspicious"
        } else {
            "clean"
        };

        ClassificationResult {
            score: round_score(total),
            level: level.to_string(),
            factors,
        }
    }
}

impl Default for Classifier {
    fn default() -> Self {
        Self::new()
    }
}

fn default_keywords() -> Vec<(String, f64)> {
    vec![
        // Jailbreak
        ("ignore".to_string(), 0.15),
        ("previous".to_string(), 0.10),
        ("instructions".to_string(), 0.12),
        ("disregard".to_string(), 0.14),
        ("override".to_string(), 0.15),
        ("bypass".to_string(), 0.14),
        ("restrictions".to_string(), 0.12),
        ("safety".to_string(), 0.08),
        ("constraints".to_string(), 0.10),
        ("forget".to_string(), 0.12),
        ("jailbreak".to_string(), 0.18),
        ("unrestricted".to_string(), 0.14),
        ("uncensored".to_string(), 0.15),
        ("unfiltered".to_string(), 0.13),
        // Role escape
        ("pretend".to_string(), 0.12),
        ("roleplay".to_string(), 0.10),
        ("simulate".to_string(), 0.08),
        ("persona".to_string(), 0.10),
        ("character".to_string(), 0.06),
        ("impersonate".to_string(), 0.13),
        ("act".to_string(), 0.05),
        ("imagine".to_string(), 0.06),
        // Data extraction
        ("prompt".to_string(), 0.10),
        ("system".to_string(), 0.06),
        ("reveal".to_string(), 0.12),
        ("confidential".to_string(), 0.10),
        ("secret".to_string(), 0.10),
        ("hidden".to_string(), 0.08),
        ("internal".to_string(), 0.07),
        ("private".to_string(), 0.08),
        ("config".to_string(), 0.06),
        ("dump".to_string(), 0.10),
        ("expose".to_string(), 0.12),
        // Command injection
        ("exec".to_string(), 0.10),
        ("eval".to_string(), 0.10),
        ("script".to_string(), 0.07),
        ("inject".to_string(), 0.12),
        ("payload".to_string(), 0.12),
        ("exploit".to_string(), 0.13),
        ("vulnerability".to_string(), 0.08),
        ("root".to_string(), 0.08),
        ("admin".to_string(), 0.07),
        ("privilege".to_string(), 0.09),
        ("escalation".to_string(), 0.10),
        // Obfuscation
        ("base64".to_string(), 0.10),
        ("encode".to_string(), 0.06),
        ("decode".to_string(), 0.06),
        ("obfuscate".to_string(), 0.12),
        ("encrypt".to_string(), 0.05),
        ("cipher".to_string(), 0.07),
    ]
}

fn tokenize(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| c.is_ascii_punctuation() || !c.is_ascii())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

impl Classifier {
    fn keyword_density(&self, input: &str) -> (f64, String) {
        let lower = input.to_lowercase();
        let words = tokenize(&lower);

        if words.is_empty() {
            return (0.0, "no tokens".to_string());
        }

        let mut total_weight = 0.0;
        let mut matched = 0usize;
        for w in &words {
            if let Some((_, weight)) = self.keywords.iter().find(|(kw, _)| kw == w) {
                total_weight += weight;
                matched += 1;
            }
        }

        let density = total_weight / words.len() as f64;
        let score: f64 = (density * 5.0_f64).min(1.0_f64);

        (
            round_score(score),
            format!("{}/{} keywords matched", matched, words.len()),
        )
    }
}

fn entropy_score(input: &str) -> (f64, String) {
    if input.is_empty() {
        return (0.0, "empty input".to_string());
    }

    let mut freq = std::collections::HashMap::new();
    for ch in input.chars() {
        *freq.entry(ch).or_insert(0usize) += 1;
    }

    let total = input.len() as f64;
    let mut entropy = 0.0;
    for &count in freq.values() {
        let p = count as f64 / total;
        if p > 0.0 {
            entropy -= p * p.log2();
        }
    }

    let max_entropy = (freq.len() as f64).log2();
    if max_entropy == 0.0 {
        return (0.0, "single character".to_string());
    }

    let normalised = entropy / max_entropy;

    let score = if normalised > 0.95 {
        0.6
    } else if normalised < 0.3 {
        0.5
    } else if normalised > 0.85 {
        0.3
    } else {
        0.0
    };

    (
        round_score(score),
        format!("entropy={:.2} bits (normalised={:.2})", entropy, normalised),
    )
}

fn structural_score(input: &str) -> (f64, String) {
    if input.is_empty() {
        return (0.0, "empty input".to_string());
    }

    let mut control_chars = 0usize;
    let mut script_types = std::collections::HashMap::new();
    let mut punctuation = 0usize;
    let mut has_unusual_quote = false;
    let total = input.chars().count();

    for ch in input.chars() {
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            control_chars += 1;
        }
        if ch.is_ascii_punctuation() {
            punctuation += 1;
        }

        let script = script_category(ch);
        if !script.is_empty() {
            *script_types.entry(script).or_insert(0usize) += 1;
        }

        if ch == '\u{2018}'
            || ch == '\u{2019}'
            || ch == '\u{201C}'
            || ch == '\u{201D}'
            || ch == '`'
            || ch == '\u{00B4}'
        {
            has_unusual_quote = true;
        }
    }

    let mixed_scripts = script_types.len() > 2;
    let mut score: f64 = 0.0;
    let mut indicators = Vec::new();

    let ctrl_ratio = control_chars as f64 / total as f64;
    if ctrl_ratio > 0.05 {
        score += 0.4;
        indicators.push("control_chars");
    } else if ctrl_ratio > 0.01 {
        score += 0.2;
        indicators.push("control_chars_low");
    }

    if mixed_scripts {
        score += 0.3;
        indicators.push("mixed_scripts");
    }

    let punct_ratio = punctuation as f64 / total as f64;
    if punct_ratio > 0.3 {
        score += 0.2;
        indicators.push("excessive_punct");
    }

    if has_unusual_quote {
        score += 0.1;
        indicators.push("unusual_quotes");
    }

    let score: f64 = score.min(1.0_f64);
    let desc = if indicators.is_empty() {
        "none".to_string()
    } else {
        indicators.join(", ")
    };

    (round_score(score), desc)
}

fn repetition_score(input: &str) -> (f64, String) {
    if input.len() < 4 {
        return (0.0, "input too short".to_string());
    }

    let lower = input.to_lowercase();
    let words = tokenize(&lower);
    if words.len() < 2 {
        return (0.0, "insufficient tokens".to_string());
    }

    let mut word_freq = std::collections::HashMap::new();
    for w in &words {
        *word_freq.entry(w.as_str()).or_insert(0usize) += 1;
    }

    let (most_common, max_freq) = word_freq
        .iter()
        .max_by_key(|(_, cnt)| *cnt)
        .map(|(&w, &c)| (w.to_string(), c))
        .unwrap_or(("".to_string(), 0));

    let word_rep_ratio = max_freq as f64 / words.len() as f64;

    let mut bigram_freq = std::collections::HashMap::new();
    let chars: Vec<char> = lower.chars().collect();
    for i in 0..chars.len().saturating_sub(1) {
        let bg = format!("{}{}", chars[i], chars[i + 1]);
        *bigram_freq.entry(bg).or_insert(0usize) += 1;
    }
    let max_bigram = bigram_freq.values().copied().max().unwrap_or(0);
    let bigram_rep_ratio = if !bigram_freq.is_empty() {
        max_bigram as f64 / bigram_freq.len() as f64
    } else {
        0.0
    };

    let mut score: f64 = if word_rep_ratio > 0.6 {
        0.8
    } else if word_rep_ratio > 0.4 {
        0.5
    } else if word_rep_ratio > 0.3 {
        0.3
    } else {
        0.0
    };

    if bigram_rep_ratio > 0.3 {
        score += 0.2;
    }

    let score: f64 = score.min(1.0_f64);

    if most_common.is_empty() {
        (round_score(score), "no repetition".to_string())
    } else {
        (
            round_score(score),
            format!(
                "most common word {:?} appears {}/{} times",
                most_common,
                max_freq,
                words.len()
            ),
        )
    }
}

fn instruction_structure_score(input: &str) -> (f64, String) {
    if input.is_empty() {
        return (0.0, "empty input".to_string());
    }

    let imperative_starters = [
        "do ",
        "don't ",
        "never ",
        "always ",
        "must ",
        "should ",
        "shall ",
        "ensure ",
        "make sure ",
        "remember ",
        "note ",
        "important ",
        "warning ",
        "caution ",
        "requirement ",
        "rule ",
        "policy ",
    ];

    let lines: Vec<&str> = input.lines().collect();
    let mut imperative_lines = 0usize;
    let mut numbered_lines = 0usize;
    let mut total_lines = 0usize;

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        total_lines += 1;
        let lower = trimmed.to_lowercase();

        for starter in &imperative_starters {
            if lower.starts_with(starter) {
                imperative_lines += 1;
                break;
            }
        }

        if is_numbered_line(&lower) {
            numbered_lines += 1;
        }
    }

    if total_lines == 0 {
        return (0.0, "no content lines".to_string());
    }

    let mut score: f64 = 0.0;
    let imperative_ratio = imperative_lines as f64 / total_lines as f64;
    let numbered_ratio = numbered_lines as f64 / total_lines as f64;

    if imperative_ratio > 0.4 {
        score += 0.4;
    } else if imperative_ratio > 0.2 {
        score += 0.2;
    }

    if numbered_ratio > 0.4 {
        score += 0.3;
    } else if numbered_ratio > 0.2 {
        score += 0.15;
    }

    if imperative_ratio > 0.2 && numbered_ratio > 0.2 {
        score += 0.2;
    }

    let score: f64 = score.min(1.0_f64);

    (
        round_score(score),
        format!(
            "imperative={}/{} numbered={}/{}",
            imperative_lines, total_lines, numbered_lines, total_lines
        ),
    )
}

fn is_numbered_line(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    if first.is_ascii_digit() {
        return true;
    }
    let prefixes = ["step ", "phase ", "stage ", "part ", "rule ", "#"];
    prefixes.iter().any(|p| s.starts_with(p))
}

fn script_category(ch: char) -> &'static str {
    if ch.is_ascii() {
        "latin"
    } else if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
        "cjk"
    } else if ('\u{0400}'..='\u{04FF}').contains(&ch) {
        "cyrillic"
    } else if ('\u{0600}'..='\u{06FF}').contains(&ch) {
        "arabic"
    } else if ('\u{AC00}'..='\u{D7AF}').contains(&ch) {
        "hangul"
    } else if ('\u{0900}'..='\u{097F}').contains(&ch) {
        "devanagari"
    } else if ('\u{0590}'..='\u{05FF}').contains(&ch) {
        "hebrew"
    } else if ('\u{0370}'..='\u{03FF}').contains(&ch) {
        "greek"
    } else if ('\u{0E00}'..='\u{0E7F}').contains(&ch) {
        "thai"
    } else {
        ""
    }
}

fn round_score(score: f64) -> f64 {
    (score * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests;
