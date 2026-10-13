//! Pattern matching utilities for security rules.
//!
//! Provides wildcard/glob matching for file paths, command patterns, and domain patterns.
//! Matches the Go implementation in `module/security/matcher.go`.

use regex::Regex;

/// Check if a target matches a pattern with wildcard support.
///
/// Supported wildcards:
/// - `*` matches any sequence within a single directory level (e.g., `*.key`, `D:/123/*.key`)
/// - `**` matches any sequence across multiple directory levels (e.g., `D:/123/**.key`)
/// - No wildcard: exact match (e.g., `/etc/passwd`)
///
/// Special case: patterns without a directory separator and containing wildcards
/// (e.g., `*.key`) match globally across all directories.
///
/// # Examples
/// ```
/// use nemesis_security::matcher::match_pattern;
/// assert!(match_pattern("*.key", "/home/user/test.key"));
/// assert!(match_pattern("D:/123/*.key", "D:/123/test.key"));
/// assert!(match_pattern("/etc/passwd", "/etc/passwd"));
/// ```
pub fn match_pattern(pattern: &str, target: &str) -> bool {
    // Normalize path separators to /
    let pattern = normalize_path(pattern);
    let target = normalize_path(target);

    // If no wildcards, do exact match
    if !pattern.contains('*') {
        return pattern == target;
    }

    // Special case: if pattern has no directory separator and has wildcards,
    // it's a global pattern - prepend ** to match across all directories.
    if !pattern.contains('/') {
        return do_match(&format!("**{}", pattern), &target);
    }

    do_match(&pattern, &target)
}

/// Normalize path separators to forward slash.
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// 文件/注册表路径规则匹配（auditor 路径桶单一入口）：Windows 文件系统
/// 大小写不敏感，规则与目标两侧同折小写——target 是模型敲的原始字符串
/// （无规范化直通），deny 规则 `D:/Secrets/*` 曾被任意大小写变体绕过
/// （D12-3，fail-open）；allow 规则反向不命中=重复弹卡。非 Windows 路径
/// 大小写敏感保持原样（fold 会让 allow 规则误放，fail-open 方向）。
/// 分隔符归一与通配语义仍由 [`match_pattern`] 承担。
pub fn match_path_rule(pattern: &str, target: &str) -> bool {
    if cfg!(windows) {
        match_pattern(&pattern.to_lowercase(), &target.to_lowercase())
    } else {
        match_pattern(pattern, target)
    }
}

/// Convert a wildcard pattern to a regex and match against the target.
fn do_match(pattern: &str, target: &str) -> bool {
    let regex_pattern = wildcard_to_regex(pattern);
    match Regex::new(&regex_pattern) {
        Ok(re) => re.is_match(target),
        Err(_) => false,
    }
}

/// Convert a wildcard pattern to a regex pattern.
///
/// Supports:
/// - `*` matches any sequence except `/` (single directory level)
/// - `**` matches any sequence including `/` (multiple directory levels)
fn wildcard_to_regex(pattern: &str) -> String {
    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');

    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Check for **
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            regex.push_str(".*");
            i += 2;
        } else if chars[i] == '*' {
            // Single * matches any sequence except /
            regex.push_str("[^/]*");
            i += 1;
        } else if matches!(
            chars[i],
            '^' | '$' | '+' | '.' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            // Escape special regex characters（D12-1：`.` 与 `?` 曾漏转义
            // ——字面点号被当正则任意字符（`*.key` 命中 `testXkey`），`?`
            // 被当可选量词（`file?.log` 漂移成「可选 e」）。本函数契约只
            // 认 `*`/`**` 通配，其余元字符一律字面。
            regex.push('\\');
            regex.push(chars[i]);
            i += 1;
        } else {
            regex.push(chars[i]);
            i += 1;
        }
    }

    regex.push('$');
    regex
}

/// Check if a command matches a pattern.
///
/// Supports `*` wildcard for command arguments. Unlike path matching,
/// `*` in command patterns matches any characters including spaces.
///
/// # Examples
/// ```
/// use nemesis_security::matcher::match_command_pattern;
/// assert!(match_command_pattern("git *", "git status"));
/// assert!(match_command_pattern("rm -rf *", "rm -rf /tmp/test"));
/// assert!(match_command_pattern("*sudo*", "sudo apt-get install"));
/// ```
pub fn match_command_pattern(pattern: &str, command: &str) -> bool {
    // For commands, * matches any characters including spaces.
    // Use a placeholder to preserve wildcards through quoting.
    const WILDCARD_PLACEHOLDER: &str = "\x00WILDCARD\x00";

    let escaped = pattern.replace('*', WILDCARD_PLACEHOLDER);
    let quoted = regex::escape(&escaped);
    let regex_body = quoted.replace(WILDCARD_PLACEHOLDER, ".*");
    let regex_pattern = format!("^{}$", regex_body);

    match Regex::new(&regex_pattern) {
        Ok(re) => re.is_match(command),
        Err(_) => false,
    }
}

/// 归一化 exec 目标用于 ABAC 匹配（CMD-05，2026-09-16 横扫存量加固）：
/// 剥引号（`"`/`'`/`` ` ``——shell 会剥，留下的引号曾打断两侧匹配，
/// `rm "-rf"` 绕过）、合并空白、统一小写（`--Recursive` 大小写绕过）。
/// 审计日志保留原文；规则匹配只看归一化形态。`command.rs` 的
/// `simplify_command` 委托本函数（CMD-11①：写了没人调的归一化基础接线）。
pub fn normalize_exec_command(command: &str) -> String {
    let collapsed: String = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let no_quotes: String = collapsed
        .chars()
        .filter(|c| *c != '"' && *c != '\'' && *c != '`')
        .collect();
    no_quotes.to_lowercase()
}

/// 解释器包装拆段（CMD-02/CMD-06②，2026-09-16 横扫存量加固）。
///
/// 输入必须是 `normalize_exec_command` 的产物（已剥引号/合并空白/小写）。
/// 从 `powershell -c <payload>`、`cmd /c <payload>`、`bash -c <payload>`、
/// `python -c <payload>`、`node -e <payload>` 等包装形态提取内层载荷，
/// 交由调用方做危险规则/结构扫描——外层 allow 规则（`python *`、
/// `powershell *`）不得屏蔽内层 `rm -rf`/`Remove-Item -Recurse` 的视线。
///
/// 一层拆段 + 载荷尾部并入（`cmd /c foo bar` 载荷 = `foo bar`）；链式包装
/// （`bash -c '... && python -c ...'`）由调用方对载荷重复调用实现递归。
/// 无旗标的解释器调用（`python script.py`）返回空——脚本文件内容不可知，
/// 由 exec_unknown_policy（D1）治理。
pub fn extract_interpreter_payloads(normalized_command: &str) -> Vec<String> {
    fn bin_key(token: &str) -> &str {
        // 取 basename（复检批：`/bin/sh`、`C:\Windows\System32\cmd.exe`
        // 这类带路径形态此前打不中包装表，内层载荷逃过 deny/ask 扫描；
        // 提取只增加扫描面，方向 fail-closed）。再剥 `.exe` 后缀。
        let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
        base.strip_suffix(".exe").unwrap_or(base)
    }
    // (解释器 token, 该解释器认的载荷旗标集)
    const WRAPPERS: &[(&str, &[&str])] = &[
        ("powershell", &["-c", "-command", "-e", "-enc"]),
        ("pwsh", &["-c", "-command", "-e", "-enc"]),
        ("cmd", &["/c", "/k"]),
        ("sh", &["-c"]),
        ("bash", &["-c"]),
        ("zsh", &["-c"]),
        ("ksh", &["-c"]),
        ("fish", &["-c"]),
        ("python", &["-c"]),
        ("python3", &["-c"]),
        ("python2", &["-c"]),
        ("node", &["-e", "--eval"]),
        ("perl", &["-e"]),
        ("ruby", &["-e"]),
    ];

    // normalized 已合并空白（单空格分隔），token 偏移可精确累计。
    let tokens: Vec<&str> = normalized_command
        .split(' ')
        .filter(|t| !t.is_empty())
        .collect();
    let mut payloads = Vec::new();
    let mut offset = 0usize;
    let mut offsets = Vec::with_capacity(tokens.len());
    for t in &tokens {
        offsets.push(offset);
        offset += t.len() + 1;
    }

    for (i, token) in tokens.iter().enumerate() {
        let Some((_, flags)) = WRAPPERS.iter().find(|(bin, _)| *bin == bin_key(token)) else {
            continue;
        };
        // 向后找第一个载荷旗标（跳过该解释器自己的中间选项，如 -Recurse
        // 之类不属于旗标集的 token 不影响——旗标集是精确匹配）。
        if let Some(j) = (i + 1..tokens.len()).find(|&j| flags.contains(&tokens[j])) {
            let payload_start = offsets[j] + tokens[j].len();
            if let Some(payload) = normalized_command.get(payload_start..) {
                let payload = payload.trim();
                if !payload.is_empty() {
                    payloads.push(payload.to_string());
                }
            }
        }
    }
    payloads
}

/// 内层载荷扫描用匹配（CMD-06②）：把 glob 形状的规则 pattern 当「按序
/// 子串序列」匹配——`rm -r*` → 文本须含 `rm -r`；`find*-delete*` → 依次
/// 含 `find`、`-delete`。与 `match_command_pattern` 的整串锚定语义刻意
/// 不同：载荷是代码/命令片段不是完整命令行，锚定打不中
/// `import os; os.system('rm -rf /data')` 里的 `rm -r`。
pub fn glob_contains(pattern: &str, text: &str) -> bool {
    let mut rest = text;
    for seg in pattern.split('*').filter(|s| !s.is_empty()) {
        match rest.find(seg) {
            Some(pos) => rest = &rest[pos + seg.len()..],
            None => return false,
        }
    }
    true
}

/// exec 规则匹配的段准备（闭环批 2026-10-13 收口：auditor 执行管线与
/// `security.rs` CLI 预览的字节级双拷贝闭包上移为单一真相源——两处语义
/// 曾靠人肉同步，漂移即审批预览与真实判定分叉）。
///
/// 在原文上切顶层复合段（`command_segments`：引号感知，引号内的 `&&`
/// 不切；引号不平衡退 quote-blind 盲切——单段 fallback 是 fail-open，一
/// 个撇号就能整体击穿），逐段归一化 + 剥 env 前缀（与归约端同语义）。
pub fn exec_rule_segments(target: &str) -> Vec<String> {
    crate::command_arity::command_segments(target)
        .iter()
        .map(|seg| {
            crate::command_arity::strip_env_assignments(&normalize_exec_command(seg)).to_string()
        })
        .collect()
}

/// exec 规则桶匹配（闭环批 2026-10-13 与 [`exec_rule_segments`] 一并上移）。
///
/// 两端不对称：**allow = 全部段命中才放**——任一段命中会把
/// `echo hi && cargo publish` 被 `cargo *` allow 规则整体放行（default
/// deny 配置下的镜像洞）；空段显式不命中（`.all()` 空迭代器恒真，空命令
/// 不得自动放行）。deny/ask = 任一段命中，或整串锚定命中——整串兜底是
/// **双形态联合**：剥 env 整串（跨段通配 `*a*b*` 与空命令 `*` 规则的旧
/// 拦截面）+ **未剥整串**（显式 env 形 deny 规则 `deny FOO=1 curl *` 的
/// 字面形态——target 侧剥 env 后，pattern 侧的 env 头只有靠未剥整串才能
/// 命中本尊输入；§九 D1 F4 声称的三臂联合，上移单一真相源时曾丢失）。
///
/// - `bucket`：`"allow"` / `"deny"` / `"ask"`（其余按 deny 桶处理）
/// - `pattern`：规则 pattern 原文（桶匹配内部过 [`normalize_exec_command`]
///   ——与 target 同管道字节级同源。曾只做小写：pattern 侧保留引号/多余
///   空白而 target 侧已剥已并，手写 shell 拟形 deny 规则（`rm "-rf" *`）
///   对归一化 target 永不命中 = 静默死规则 fail-open（D12-2））
/// - `target_normalized`：整串归一化（[`normalize_exec_command`] 产物）
/// - `segs`：[`exec_rule_segments`] 的产物
pub fn exec_rule_bucket_match(
    bucket: &str,
    pattern: &str,
    target_normalized: &str,
    segs: &[String],
) -> bool {
    let pattern_lower = &normalize_exec_command(pattern);
    let hit = |seg: &str| match_command_pattern(pattern_lower, seg);
    match bucket {
        "allow" => !segs.is_empty() && segs.iter().all(|seg| hit(seg)),
        // deny/ask：任一段命中，或整串锚定兜底（剥 env + 未剥双形态）
        _ => {
            segs.iter().any(|seg| hit(seg))
                || hit(crate::command_arity::strip_env_assignments(
                    target_normalized,
                ))
                || hit(target_normalized)
        }
    }
}

/// Check if a domain matches a pattern.
///
/// # Examples
/// ```
/// use nemesis_security::matcher::match_domain_pattern;
/// assert!(match_domain_pattern("*.github.com", "api.github.com"));
/// assert!(match_domain_pattern("github.com", "github.com"));
/// assert!(!match_domain_pattern("*.github.com", "github.com"));
/// ```
pub fn match_domain_pattern(pattern: &str, domain: &str) -> bool {
    let domain = domain.to_lowercase();
    let pattern = pattern.to_lowercase();

    // No wildcard - exact match
    if !pattern.contains('*') {
        return domain == pattern;
    }

    // Use placeholders to preserve wildcards and dots through escaping
    const WILDCARD_PLACEHOLDER: &str = "\x00WILDCARD\x00";
    const DOT_PLACEHOLDER: &str = "\x00LITERALDOT\x00";

    // Step 1: Replace wildcards with placeholder
    let p = pattern.replace('*', WILDCARD_PLACEHOLDER);
    // Step 2: Replace literal dots with placeholder
    let p = p.replace('.', DOT_PLACEHOLDER);
    // Step 3: Escape remaining special characters
    let p = regex::escape(&p);
    // Step 4: Replace placeholders with actual regex patterns
    // For domains, * should match only a single subdomain level (anything except dot)
    let p = p.replace(WILDCARD_PLACEHOLDER, "[^.]*");
    let p = p.replace(DOT_PLACEHOLDER, "\\.");

    let regex_pattern = format!("^{}$", p);
    match Regex::new(&regex_pattern) {
        Ok(re) => re.is_match(&domain),
        Err(_) => false,
    }
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
