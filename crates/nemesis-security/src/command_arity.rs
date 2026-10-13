//! Shell 命令前缀归约（B5）。
//!
//! 把完整命令行归约成可读前缀（`cargo build --release --features foo` →
//! `cargo build *`），供审批 pattern 记忆（F3）与前缀粒度审批匹配（F4）
//! 生成/展示 pattern 用。纯函数、无状态，不参与 deny 管线判断。
//!
//! 归约规则：
//! 1. 分词（识别单双引号，引号剥除内容保留；不支持引号内反斜杠转义——
//!    Windows 路径反斜杠照字面保留，这正是本项目的常见形态）。
//! 2. 跳过头部 `VAR=value` 环境变量前缀（`FOO=1 cargo build` 与
//!    `cargo build` 归约出同一 pattern，pattern 更可复用）。
//! 3. 查静态 arity 表（最长匹配优先）：命中 → 保留对应 token 数；
//!    未命中 → 退首 token。
//! 4. 剩余 token 数 ≤ 保留数 → 原样输出整个命令（不加 ` *`，
//!    此时命令本身已是完整可枚举 pattern）；否则截断补 ` *`。
//!
//! 已知边界：大小写敏感（`Git` ≠ `git`）；`cmd /c`、`powershell -Command`
//! 等 shell 包装不展开（包装内命令按首 token 退化）。

/// 命令分词：按空白切分，识别单/双引号（引号剥除、内容保留）。
/// 引号内空白不切分；引号内反斜杠不转义（照字面保留）。
pub fn tokenize_command(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_content = false;

    for ch in cmd.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                has_content = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_content = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_content || !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                    has_content = false;
                }
            }
            c => {
                cur.push(c);
                has_content = true;
            }
        }
    }
    if has_content || !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// 顶层复合命令分段（CMD-12，2026-10-13）。
///
/// 在**原文**上按顶层操作符切分：`&&` / `||` / `;` / `|` / `&` / 换行。
/// 引号感知（同 [`tokenize_command`]：单双引号内不切分；不支持引号内
/// 反斜杠转义）。段各自 trim，空段（连续操作符）跳过；引号不闭合 →
/// 整条命令退单段（宁可不切，不可切错）。**不是 shell 解析器**——
/// 只认顶层操作符字面形态；`2>&1` 这类重定向中的 `&` 因前导字符为
/// `>` 不切分（`cargo build 2>&1` 保持单段）。
///
/// 消费语义（两端不对称是刻意的）：
/// - deny/ask（ABAC 规则）：**任一段命中即拦**——整串锚定打不中非首段
///   （`echo hi && rm -rf y` 曾逃过 `rm -rf *`）。
/// - allow（审批 pattern 记忆）：**全部段命中才放**——`cd *` 不得罩住
///   `cd x && rm -rf y` 的删除段。
/// - 归约（[`crate::approval_rules::pattern_for`]）：复合命令不构成
///   前缀规则（单一 pattern 表达不了全段语义，宁拒绝不放宽）。
pub fn command_segments(cmd: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_char: Option<char> = None;

    let mut chars = cmd.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(ch);
                prev_char = Some(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(ch);
                prev_char = Some(ch);
            }
            ';' | '\n' | '\r' if !in_single && !in_double => {
                segments.push(std::mem::take(&mut cur));
                prev_char = None;
            }
            '&' | '|' if !in_single && !in_double => {
                // `>&`（重定向，如 2>&1）不是操作符——`&` 随前导 `>` 归段。
                if ch == '&' && prev_char == Some('>') {
                    cur.push(ch);
                    prev_char = Some(ch);
                    continue;
                }
                segments.push(std::mem::take(&mut cur));
                if ch == chars.peek().copied().unwrap_or('\0') {
                    chars.next(); // `&&` / `||` 整体消费
                }
                prev_char = None;
            }
            c => {
                cur.push(c);
                prev_char = Some(c);
            }
        }
    }
    segments.push(cur);

    // 引号不闭合 → 整条命令退单段（原文原样，不冒险切一半）
    if in_single || in_double {
        let whole = cmd.trim().to_string();
        return if whole.is_empty() {
            Vec::new()
        } else {
            vec![whole]
        };
    }

    let mut out: Vec<String> = segments
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    out.dedup();
    out
}

/// 静态 arity 表：key（空格连接的前缀）→ 保留 token 数（含 key 自身）。
/// 匹配时最长 key 优先。未命中的命令退首 token。
static ARITY_TABLE: &[(&str, usize)] = &[
    // 多 token 键（最长匹配优先于下方单 token 键）
    ("npm run", 3),
    ("npm exec", 3),
    ("pnpm run", 3),
    ("yarn run", 3),
    ("docker compose", 3),
    // Rust
    ("cargo", 2),
    ("rustup", 2),
    // JS/TS 生态
    ("npm", 2),
    ("npx", 2),
    ("pnpm", 2),
    ("yarn", 2),
    ("bun", 2),
    ("bunx", 2),
    ("deno", 2),
    ("node", 2),
    // Python 生态
    ("python", 2),
    ("python3", 2),
    ("pip", 2),
    ("pip3", 2),
    ("uv", 2),
    ("poetry", 2),
    // 其他语言工具链
    ("go", 2),
    ("dotnet", 2),
    ("flutter", 2),
    ("gradle", 2),
    ("mvn", 2),
    ("make", 2),
    ("cmake", 2),
    // 容器 / 编排
    ("docker", 2),
    ("docker-compose", 2),
    ("podman", 2),
    ("kubectl", 2),
    ("helm", 2),
    // git / 平台 CLI
    ("git", 2),
    ("gh", 2),
    ("adb", 2),
    // 包管理 / 系统服务
    ("apt", 2),
    ("apt-get", 2),
    ("dnf", 2),
    ("yum", 2),
    ("pacman", 2),
    ("brew", 2),
    ("winget", 2),
    ("choco", 2),
    ("scoop", 2),
    ("systemctl", 2),
];

/// `VAR=value` 环境变量前缀判定：`IDENT=`（值任意，可为空）。
fn is_env_assignment(token: &str) -> bool {
    let Some(eq) = token.find('=') else {
        return false;
    };
    if eq == 0 {
        return false;
    }
    let name = &token[..eq];
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 把命令行归约成可读前缀 pattern（详见模块文档）。
///
/// 例：`cargo build --release` → `cargo build *`；`git status` → `git status`；
/// `unknown-tool --flag` → `unknown-tool *`；空串 → 空串。
pub fn reduce_command(cmd: &str) -> String {
    let tokens = tokenize_command(cmd);
    // 跳过头部 VAR=value 前缀（输出同样不含它们，pattern 更可复用）
    let start = tokens.iter().take_while(|t| is_env_assignment(t)).count();
    let rest = &tokens[start..];
    if rest.is_empty() {
        return String::new();
    }

    // 最长匹配：遍历表，取 key token 数最大且完全前缀匹配的条目
    let mut keep = 1usize;
    for (key, arity) in ARITY_TABLE {
        let key_tokens: Vec<&str> = key.split_whitespace().collect();
        if key_tokens.len() > rest.len() {
            continue;
        }
        if rest.iter().zip(key_tokens.iter()).all(|(t, k)| t == k) && *arity > keep {
            keep = *arity;
        }
    }

    if rest.len() <= keep {
        rest.join(" ")
    } else {
        format!("{} *", rest[..keep].join(" "))
    }
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
