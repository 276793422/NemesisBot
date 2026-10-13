//! L6 pre-exec 脚本内容扫描（2026-10-12 三方学习 T1）。
//!
//! 8 层管线全部只看 `invocation.args`——磁盘既有脚本（`exec bash ./setup.sh`）
//! 的**内容**在执行前零检查：脚本可含注入样载荷（jailbreak 指令、下载器、
//! 递归删除）而不触发任何一层。本模块在 exec/spawn 进 ABAC 前，从命令行
//! 提取脚本文件路径，读取内容过注入检测（复用 [`crate::injection`] 的
//! `Detector::analyze` 文本级 API）：
//!
//! - 命中 → 返回 hit，pipeline 把 `force_approval` 钉到 OperationRequest，
//!   ABAC 升格强制人工审批（审批卡）——**误报出路**，非纯硬拦；
//! - 文件缺失 → 放行（exec 自会报错，不是安全事件）；
//! - 二进制（首 8KB 含 NUL）→ 跳过（文本检测器对字节流无意义）；
//! - `bash -c "…"` 内联形态不受影响（args 本身对 Layer 1/2 可见）。
//!
//! 已知边界（诚实声明）：`curl … | bash` 管道形态**不在覆盖内**（内容不落
//! 盘不进上下文，由下载层 + 工具输出复扫间接覆盖）；相对路径只按注入的
//! workspace root 解析（exec 工具默认 cwd=workspace，覆盖主流形态）；
//! `~` 经环境变量展开（USERPROFILE/HOME），展开失败即跳过。

use crate::injection::{Detector as InjectionDetector, InjectionConfig};
use parking_lot::RwLock;

/// 单次命令最多扫描的脚本数（防超长命令链拖垮管线；超出部分诚实忽略）。
const MAX_SCRIPTS_PER_COMMAND: usize = 8;
/// shebang 探测读取上限（也用于二进制判定）。
const SNIFF_BYTES: usize = 8 * 1024;
/// 单脚本内容扫描上限（超过截断扫描——检测器对超长文本本就截断）。
const MAX_SCAN_BYTES: usize = 512 * 1024;

/// 脚本扩展名（点后小写比较）。覆盖 shell / PowerShell / Python / Ruby /
/// Perl / Node 家族；无扩展名候选走 shebang 探测（`#!/` 前缀）。
const SCRIPT_EXTENSIONS: &[&str] = &[
    "sh", "bash", "zsh", "ksh", "ash", "ps1", "psm1", "psd1", "py", "pyw", "rb", "pl", "pm", "js",
    "mjs", "cjs", "ts",
];

/// 扫描命中详情（pipeline 转成 force_approval 理由 + 审计事件）。
#[derive(Debug, Clone)]
pub struct PreExecHit {
    /// 命中的脚本路径（解析后的绝对/相对原样）。
    pub script: String,
    /// 注入分数（0.0-1.0）。
    pub score: f64,
    /// 等级（none/low/medium/high/critical）。
    pub level: String,
    /// 命中的模式（检测器原文，已截断展示）。
    pub matched: Vec<String>,
}

/// pre-exec 脚本内容扫描器。挂在 SecurityPlugin 上，构造期随
/// `preexec_scan_enabled` 开关装配（关 = 不构造 = 扫描短路，与
/// injection/command_guard 同款供给模式）。
pub struct PreExecScanner {
    detector: InjectionDetector,
    /// 相对脚本路径的解析根（workspace root）。空串 = 未注入 → 相对路径
    /// 不解析（诚实降级：绝对路径仍覆盖）。
    workspace_root: RwLock<String>,
}

impl PreExecScanner {
    pub fn new(threshold: f64) -> Self {
        Self {
            detector: InjectionDetector::new(InjectionConfig {
                enabled: true,
                threshold,
                max_input_length: MAX_SCAN_BYTES,
                strict_mode: false,
            }),
            workspace_root: RwLock::new(String::new()),
        }
    }

    /// 注入相对路径解析根（security_setup 构造期调用；与 D1 保护路径同源
    /// 的 workspace root）。
    pub fn set_workspace_root(&self, root: &str) {
        *self.workspace_root.write() = root.trim().to_string();
    }

    /// 扫描一条 exec/spawn 命令行。返回第一个命中（命中即升级，多脚本
    /// 逐个扫、先命中先返回）。
    pub fn scan(&self, command: &str) -> Option<PreExecHit> {
        let mut scanned = 0usize;
        for token in tokenize(command) {
            if scanned >= MAX_SCRIPTS_PER_COMMAND {
                break;
            }
            if !is_script_candidate(&token) {
                continue;
            }
            let Some(path) = self.resolve(&token) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                // 文件缺失/不可读 → 放行：exec 自会报错，不是安全事件。
                continue;
            };
            if bytes.is_empty() {
                continue;
            }
            let sniff = &bytes[..bytes.len().min(SNIFF_BYTES)];
            if sniff.contains(&0u8) {
                continue; // 二进制 → 文本检测无意义，跳过。
            }
            // 无扩展名候选必须以 shebang 开头才算脚本（避免读任意数据文件）。
            if !has_script_extension(&token) && !sniff.starts_with(b"#!") {
                continue;
            }
            scanned += 1;
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_SCAN_BYTES)]);
            let result = self.detector.analyze(&text);
            if result.is_injection {
                return Some(PreExecHit {
                    script: path,
                    score: result.score,
                    level: result.level,
                    matched: result.matched_patterns,
                });
            }
        }
        None
    }

    /// 候选 token → 可读文件路径。绝对路径原样；`~` 经环境变量展开；
    /// 相对路径按 workspace root 解析（未注入 = None，诚实降级）。
    fn resolve(&self, token: &str) -> Option<String> {
        let t = token.replace('\\', "/");
        if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .ok()?;
            if home.is_empty() {
                return None;
            }
            return Some(format!("{}/{}", home.trim_end_matches(['/', '\\']), rest));
        }
        if std::path::Path::new(&t).is_absolute() {
            return Some(t);
        }
        let root = self.workspace_root.read().clone();
        if root.is_empty() {
            return None;
        }
        Some(format!(
            "{}/{}",
            root.trim_end_matches(['/', '\\']),
            t.trim_start_matches("./")
        ))
    }
}

/// 命令行分词：按空白切 + 剥成对引号。内联载荷（`-c "code"`）的碎片
/// 由 [`is_plausible_path`] / 扩展名检查自然过滤（带 `;|&<>()$` 等结构
/// 字符的 token 不是可信路径）。
fn tokenize(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|t| t.trim_matches(|c| c == '"' || c == '\'').to_string())
        .collect()
}

/// 候选资格：非旗标、非 URL、无结构字符、无通配符（glob 不是文件）、
/// 且「有脚本扩展名」或「形似路径（含分隔符，走 shebang 探测）」。
fn is_script_candidate(token: &str) -> bool {
    if token.is_empty()
        || token.starts_with('-')
        || token.contains("://")
        || token.contains([
            ';', '|', '&', '<', '>', '(', ')', '{', '}', '$', '`', '*', '?',
        ])
    {
        return false;
    }
    if has_script_extension(token) {
        return true;
    }
    // 无扩展名：必须形似路径（`./configure`、`tools/setup`），裸词不读。
    (token.starts_with("./") || token.starts_with("../") || token.contains('/'))
        && !has_any_extension(token)
}

/// token 是否带任意扩展名（最后一个路径段含非首位 `.`）。
fn has_any_extension(token: &str) -> bool {
    let seg = last_segment(token);
    match seg.rfind('.') {
        Some(0) | None => false,
        Some(_) => true,
    }
}

/// token 是否带脚本扩展名（最后一个路径段的点后缀，大小写不敏感）。
fn has_script_extension(token: &str) -> bool {
    let seg = last_segment(token);
    let Some(dot) = seg.rfind('.') else {
        return false;
    };
    let ext = seg[dot + 1..].to_ascii_lowercase();
    SCRIPT_EXTENSIONS.contains(&ext.as_str())
}

fn last_segment(token: &str) -> &str {
    token.rsplit(['/', '\\']).next().unwrap_or(token)
}

#[cfg(test)]
mod tests;
