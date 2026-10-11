//! S2①：exec 前脚本内容扫描（shell_bleed，2026-10-09 高优差距批次一）。
//!
//! 8 层管线全部只见工具**入参**——`write_file` 写脚本时内容在 args 里会
//! 过第④层，但**预先存在于磁盘的脚本**（用户创建 / web_fetch 下载 /
//! git clone）被 exec 执行时内容零检查。注入链只需让 agent
//! `bash setup.sh` 就能带着脚本里的任意逻辑绕过入参审查。
//!
//! 两级判定（openfang shell_bleed 同防御面，判定收紧一档）：
//! - **字面凭据**（第④层同源模式表命中）= **执行前拦截**——脚本文件里
//!   出现明文密钥，执行只会放大泄漏面；拒绝文案明示改用环境变量 / vault
//!   引用。
//! - **敏感环境变量引用**（`$OPENAI_API_KEY` / `os.environ["..."]` /
//!   `%GITHUB_TOKEN%` 等形态，经 `env_sanitize::is_sensitive_env_name`
//!   认定敏感）= **放行 + 事后提示**——读环境变量是脚本的正当模式
//!   （也恰是推荐的凭据供给方式），不拦；但提示模型/用户该脚本会读取
//!   宿主秘钥环境变量，来源不明时值得警惕（openfang 告警语义）。
//!
//! 剥离次序防误报：字面量扫描前先摘除环境变量引用 span（替换为 `\0` 防
//! 相邻字符意外拼合）——`API_TOKEN="$MY_API_TOKEN"` 是推荐形态，不能因
//! 泛化赋值模式被误判成字面凭据。
//!
//! 已知边界（诚实声明）：相对路径解析 base 取 `args.cwd`（与 exec 工具
//! 同字段）否则进程 cwd——解析失败的候选落入 `skipped`（诚实降级），
//! 拦截面以可读脚本为界；单脚本 1MB / 单命令 8 个候选上限；扩展名白名单
//! 之外的执行形态（`bash -c` 内联、解释器 stdin）入参本身已被管线扫描。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::credential::Scanner;
use nemesis_utils::env_sanitize::is_sensitive_env_name;

/// 单脚本内容上限（字节）：超过不读不扫（诚实记 skipped）。
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;
/// 单命令最多扫描的脚本数：防超长命令拖垮派发路径。
const MAX_SCRIPTS_PER_COMMAND: usize = 8;

/// 视作「脚本内容盲区」的扩展名白名单（大小写不敏感）。
const SCRIPT_EXTS: &[&str] = &[
    "py", "sh", "js", "mjs", "cjs", "ts", "ps1", "bat", "cmd", "rb", "pl", "lua",
];

fn env_ref_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?x)
\$\{([A-Za-z_][A-Za-z0-9_]*)\}
| \$ENV\{['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?\}
| process\.env\.([A-Za-z_][A-Za-z0-9_]*)
| process\.env\s*\[\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]
| environ(?:\.get)?\s*\(\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]
| getenv\s*\(\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]
| (?:environ|ENV)\s*\[\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]\s*\]
| %([A-Za-z_][A-Za-z0-9_]*)%
| \$([A-Za-z_][A-Za-z0-9_]*)
"#,
        )
        .expect("shell_bleed env-ref regex must compile")
    })
}

/// 字面凭据命中（= 执行前拦截依据）。
#[derive(Debug, Clone)]
pub struct LiteralHit {
    pub script: String,
    pub pattern: String,
    /// 掩码形态（首尾保留 4 字符），用于拒绝文案不回显原文。
    pub masked: String,
}

/// 敏感环境变量引用（= 放行 + 事后提示依据）。
#[derive(Debug, Clone)]
pub struct EnvRefHit {
    pub script: String,
    pub var: String,
}

/// 一次 exec 前脚本扫描的完整产出。
#[derive(Debug, Default)]
pub struct ScriptScanOutcome {
    pub literals: Vec<LiteralHit>,
    pub env_refs: Vec<EnvRefHit>,
    /// 实际读取并扫描的脚本（展示形态 = 命令中的原始 token）。
    pub scanned: Vec<String>,
    /// 跳过的候选及原因（诚实降级：不存在 / 非文件 / 超限 / 读取失败）。
    pub skipped: Vec<String>,
}

impl ScriptScanOutcome {
    pub fn has_literals(&self) -> bool {
        !self.literals.is_empty()
    }

    /// 拒绝文案用摘要：按脚本归组（首次出现序去重）、模式去重计数（不回
    /// 显原文）。
    pub fn literal_summary(&self) -> String {
        let mut scripts: Vec<&str> = Vec::new();
        for l in &self.literals {
            if !scripts.contains(&l.script.as_str()) {
                scripts.push(&l.script);
            }
        }
        let mut parts: Vec<String> = Vec::new();
        for script in scripts {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for hit in self.literals.iter().filter(|l| l.script == script) {
                match counts.iter_mut().find(|(p, _)| *p == hit.pattern) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((hit.pattern.clone(), 1)),
                }
            }
            let detail = counts
                .iter()
                .map(|(p, n)| format!("{p}×{n}"))
                .collect::<Vec<_>>()
                .join("、");
            parts.push(format!("{script}: {detail}"));
        }
        parts.join("；")
    }

    /// 放行后追加给模型的事后提示；无敏感环境变量引用时 None。
    pub fn advisory_note(&self) -> Option<String> {
        if self.env_refs.is_empty() {
            return None;
        }
        let mut seen: Vec<&str> = Vec::new();
        let mut scripts: Vec<&str> = Vec::new();
        for e in &self.env_refs {
            if !seen.contains(&e.var.as_str()) {
                seen.push(&e.var);
            }
            if !scripts.contains(&e.script.as_str()) {
                scripts.push(&e.script);
            }
        }
        Some(format!(
            "[脚本提示: {} 引用了敏感环境变量（{}）；已放行执行。请确认脚本来源可信，勿将其读取的环境变量值发送到外部。",
            scripts.join("、"),
            seen.join("、")
        ))
    }
}

/// 入口：从 exec 命令行提取脚本候选 → 读内容 → 双路扫描。
/// `credential` 传 `None`（凭据层关闭）时只做环境变量引用告警。
pub fn scan_scripts_for_exec(
    command: &str,
    base: &Path,
    credential: Option<&Scanner>,
) -> ScriptScanOutcome {
    let mut out = ScriptScanOutcome::default();
    let mut seen_paths: HashSet<String> = HashSet::new();

    for token in extract_script_tokens(command) {
        if out.scanned.len() >= MAX_SCRIPTS_PER_COMMAND {
            out.skipped.push(format!(
                "（候选超过 {MAX_SCRIPTS_PER_COMMAND} 个，其余未扫描）"
            ));
            break;
        }
        let path = resolve_candidate(&token, base);
        let key = path
            .canonicalize()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        if !seen_paths.insert(key) {
            continue; // 同一脚本被多次引用只扫一次
        }
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                out.skipped.push(format!("{token}: 不可访问（{e}）"));
                continue;
            }
        };
        if !meta.is_file() {
            out.skipped.push(format!("{token}: 非常规文件"));
            continue;
        }
        if meta.len() > MAX_SCRIPT_BYTES {
            out.skipped
                .push(format!("{token}: 超过 {} 字节上限", MAX_SCRIPT_BYTES));
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                out.skipped.push(format!("{token}: 读取失败（{e}）"));
                continue;
            }
        };
        let content = String::from_utf8_lossy(&bytes);
        scan_one_script(&token, &content, credential, &mut out);
        out.scanned.push(token);
    }
    out
}

/// 单脚本双路扫描：环境变量引用（原文上找）+ 字面凭据（摘除引用后扫，
/// 防推荐形态 `TOKEN="$MY_TOKEN"` 被泛化赋值模式误判为字面凭据）。
fn scan_one_script(
    script: &str,
    content: &str,
    credential: Option<&Scanner>,
    out: &mut ScriptScanOutcome,
) {
    for cap in env_ref_regex().captures_iter(content) {
        for m in cap.iter().skip(1).flatten() {
            let name = m.as_str();
            if !is_sensitive_env_name(name) {
                continue;
            }
            if out
                .env_refs
                .iter()
                .any(|e| e.script == script && e.var == name)
            {
                continue;
            }
            out.env_refs.push(EnvRefHit {
                script: script.to_string(),
                var: name.to_string(),
            });
        }
    }
    if let Some(scanner) = credential {
        // 替换为 \0 而非删除/空格：防相邻字符意外拼合成新模式。
        let scrubbed = env_ref_regex().replace_all(content, "\0");
        let r = scanner.scan_content(&scrubbed);
        for m in r.matches {
            out.literals.push(LiteralHit {
                script: script.to_string(),
                pattern: m.pattern_name,
                masked: m.redacted,
            });
        }
    }
}

/// 从命令行提取脚本候选 token（引号感知分词 + 分隔符切分 + 扩展名过滤）。
fn extract_script_tokens(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    // && / || / 单个 & / ; / | / 换行皆视作命令边界（引号内的边界符会被
    // 误切——代价只是该候选解析失败落入 skipped，best-effort）。
    let normalized = command.replace("&&", ";").replace("||", ";");
    for segment in normalized.split([';', '|', '&', '\n']) {
        for tok in tokenize_segment(segment) {
            if is_script_candidate(&tok) {
                out.push(tok);
            }
        }
    }
    out
}

/// 引号感知分词：引号内空白不切分（"my script.py" 保持单 token）。
fn tokenize_segment(segment: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in segment.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

/// 候选判定：非 flag（`-` 开头跳过——`--config=x.py` 类 flag 值不扫）、
/// 扩展名命中白名单。
fn is_script_candidate(token: &str) -> bool {
    if token.starts_with('-') {
        return false;
    }
    let t = token.strip_prefix("./").unwrap_or(token);
    Path::new(t)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SCRIPT_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// 相对候选按 base（args.cwd / 进程 cwd）解析；绝对路径原样。
fn resolve_candidate(token: &str, base: &Path) -> PathBuf {
    let t = token.strip_prefix("./").unwrap_or(token);
    let p = Path::new(t);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

#[cfg(test)]
mod tests;
