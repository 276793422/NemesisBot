//! 全自动流转 P2（B1）：确定性验收锚点——零 LLM 成本、零命令执行的客观
//! 检查层（单一真相源：本模块与 planner prompt 的 `[CHECK]` 指令段同步
//! 演化）。
//!
//! 验收标准（`acceptance_criteria` 自由文本）中以 `[CHECK]` 开头的行是
//! 客观锚点，其余行照旧走 LLM 语义项（`parse_anchors` 返回二元组）：
//!
//! ```text
//! [CHECK] file:src/lib.rs exists            — 文件存在
//! [CHECK] re:交付完成                        — 对 worker delivery 文本正则匹配
//! [CHECK] file:docs/api.md contains:鉴权     — 文件内容包含关键词
//! [CHECK] file:src/main.rs re:^fn main      — 文件内容正则匹配
//! ```
//!
//! 刻意不执行任何命令（保持零攻击面；「命令退出码」类检查归 P4 B2）。
//! 文件类锚点只读 `std::fs`，路径经 [`resolve_anchor_path`] 单点收紧：
//! 仅限 workspace 子树内相对路径，拒绝绝对路径 / `..` 穿越 / 8.3 短名 /
//! 符号链接越界（canonicalize 后前缀校验）。无法解析的 `[CHECK]` 行不
//! 拒绝——回落语义项，诚实降级不炸验收。

use regex::Regex;
use std::path::{Path, PathBuf};

/// 锚点检查上限（超过按失败处理，诚实降级；防锚点检查本身变成内存攻击面）。
const MAX_ANCHOR_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 锚点行前缀（trim 后判定；planner 指令段使用同一标记）。
pub const ANCHOR_PREFIX: &str = "[CHECK]";

/// 锚点种类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorKind {
    /// `file:<路径> exists` — 文件存在。
    FileExists,
    /// `file:<路径> re:<正则>` — 文件内容正则匹配。
    FileRegex,
    /// `file:<路径> contains:<关键词>` — 文件内容包含关键词。
    FileContains,
    /// `re:<正则>` — 对 worker delivery 文本（验收输入）正则匹配。
    ContentRegex,
}

impl AnchorKind {
    /// 派发/评审文案里的短名（`AnchorResult::detail` 拼接用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorKind::FileExists => "文件存在",
            AnchorKind::FileRegex => "文件正则",
            AnchorKind::FileContains => "文件包含",
            AnchorKind::ContentRegex => "交付文本正则",
        }
    }
}

/// 一条解析后的锚点（`raw` = 原始行，明细评论里回显）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorCheck {
    pub raw: String,
    pub kind: AnchorKind,
    /// 文件类 = workspace 相对路径；ContentRegex = 空串。
    pub target: String,
    /// 正则模式 / 关键词；FileExists = None。
    pub pattern: Option<String>,
}

/// 单条锚点执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorResult {
    pub anchor: AnchorCheck,
    pub passed: bool,
    pub detail: String,
}

/// 被拒绝的锚点行（路径形态不安全：绝对路径 / `..` 穿越 / 盘符 / UNC /
/// 8.3 短名）。拒绝 = 不产出锚点、行回落语义项 + 调用方落告警评论——
/// 恶意/不安全锚点是验收标准自身的毛病，重派惩罚不到执行者，也不得
/// 让验收炸掉或变成对工作区外文件系统的探测激励。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedAnchor {
    pub raw: String,
    pub reason: String,
}

/// 解析验收标准：`[CHECK]` 行 → 锚点，其余行（含无法解析的 `[CHECK]` 行）
/// → 语义项回落。非法正则同样回落语义项（诚实降级，不拒绝整段验收标准）。
/// 路径形态不安全的锚点行进入第三元组 `Vec<RejectedAnchor>`（同样回落语
///义项，供调用方落系统告警评论）。
pub fn parse_anchors(
    acceptance_criteria: &str,
) -> (Vec<AnchorCheck>, Vec<String>, Vec<RejectedAnchor>) {
    let mut anchors = Vec::new();
    let mut semantic = Vec::new();
    let mut rejected = Vec::new();
    for line in acceptance_criteria.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(ANCHOR_PREFIX) {
            match parse_anchor_line(rest.trim()) {
                Ok(Some(anchor)) => anchors.push(anchor),
                Ok(None) => semantic.push(line.to_string()),
                Err(reason) => {
                    semantic.push(line.to_string());
                    rejected.push(RejectedAnchor {
                        raw: trimmed.to_string(),
                        reason,
                    });
                }
            }
        } else if !trimmed.is_empty() {
            semantic.push(line.to_string());
        }
    }
    (anchors, semantic, rejected)
}

/// 解析 `[CHECK]` 之后的剩余文本。
/// - `Ok(Some(anchor))` = 合法锚点；
/// - `Ok(None)` = 无法解析（回落语义项）；
/// - `Err(reason)` = 路径形态不安全（回落语义项 + 告警）。
fn parse_anchor_line(rest: &str) -> Result<Option<AnchorCheck>, String> {
    let raw = format!("{ANCHOR_PREFIX} {rest}");
    if rest.is_empty() {
        return Ok(None);
    }
    if let Some(target) = rest.strip_prefix("file:") {
        // 目标路径到首个空白为止；谓词可含空格（正则/关键词）。
        let (target, predicate) = match target.find(char::is_whitespace) {
            Some(idx) => (&target[..idx], target[idx..].trim()),
            None => (target, ""),
        };
        if target.is_empty() {
            return Ok(None);
        }
        // 路径安全形态闸（解析期，FS 无关）：绝对路径 / 盘符 / UNC /
        // `..` 穿越 / 8.3 短名直接拒绝——不安全锚点不得成为重派理由，
        // 更不能诱导执行者向工作区外写文件来「满足锚点」。
        validate_anchor_path_shape(target)?;
        let (kind, pattern) = if predicate == "exists" {
            (AnchorKind::FileExists, None)
        } else if let Some(pattern) = predicate.strip_prefix("re:") {
            if pattern.trim().is_empty() || Regex::new(pattern.trim()).is_err() {
                return Ok(None); // 空模式 / 非法正则 → 语义项回落
            }
            (AnchorKind::FileRegex, Some(pattern.trim().to_string()))
        } else if let Some(keyword) = predicate.strip_prefix("contains:") {
            let keyword = keyword.trim();
            if keyword.is_empty() {
                return Ok(None);
            }
            (AnchorKind::FileContains, Some(keyword.to_string()))
        } else {
            return Ok(None); // 未知谓词 → 语义项回落
        };
        return Ok(Some(AnchorCheck {
            raw,
            kind,
            target: target.to_string(),
            pattern,
        }));
    }
    if let Some(pattern) = rest.strip_prefix("re:") {
        let pattern = pattern.trim();
        if pattern.is_empty() || Regex::new(pattern).is_err() {
            return Ok(None);
        }
        return Ok(Some(AnchorCheck {
            raw,
            kind: AnchorKind::ContentRegex,
            target: String::new(),
            pattern: Some(pattern.to_string()),
        }));
    }
    Ok(None)
}

/// 执行锚点组：文件类只读 `std::fs`，正则走 regex crate（线性时间，无
/// ReDoS 面）。全部判定保守——任何执行层意外（路径不安全 / 读取失败 /
/// 超限）都算未通过并带诚实明细，不静默放行。
pub fn run_anchors(
    anchors: &[AnchorCheck],
    workspace_root: &Path,
    delivery_text: &str,
) -> Vec<AnchorResult> {
    anchors
        .iter()
        .map(|anchor| {
            let (passed, detail) = run_one(anchor, workspace_root, delivery_text);
            AnchorResult {
                anchor: anchor.clone(),
                passed,
                detail,
            }
        })
        .collect()
}

fn run_one(anchor: &AnchorCheck, workspace_root: &Path, delivery_text: &str) -> (bool, String) {
    match &anchor.kind {
        AnchorKind::ContentRegex => {
            let Some(pattern) = &anchor.pattern else {
                return (false, "锚点缺少正则模式".to_string());
            };
            match Regex::new(pattern) {
                Ok(re) => {
                    // 自指防御（2026-09-12 UAT T30③ 实证）：交付文本逐字
                    // 引用锚点行本身（重派 prompt 回显、复述验收标准的汇报）
                    // 不构成满足锚点的证据——剥离全部锚点行原文后再匹配。
                    // 否则 worker 只要把任务卡原样抄回来，re: 锚点就在引文
                    // 里自命中 → 假 PASS → 自动收货。
                    let cleaned = delivery_text.replace(&anchor.raw, "");
                    if re.is_match(&cleaned) {
                        (true, format!("交付文本命中 /{pattern}/"))
                    } else {
                        (false, format!("交付文本未命中 /{pattern}/"))
                    }
                }
                Err(e) => (false, format!("正则编译失败（{e}）")),
            }
        }
        AnchorKind::FileExists => match resolve_anchor_path(workspace_root, &anchor.target) {
            // resolve 内部 canonicalize 成功 = 存在。
            Ok(path) => (true, format!("文件存在（{}）", path.display())),
            Err(e) => (false, e),
        },
        AnchorKind::FileRegex | AnchorKind::FileContains => {
            // 文件内容类：先解析路径（安全闸），再读内容按谓词判定。
            let path = match resolve_anchor_path(workspace_root, &anchor.target) {
                Ok(p) => p,
                Err(e) => return (false, e),
            };
            let content = match read_limited(&path) {
                Ok(c) => c,
                Err(e) => return (false, e),
            };
            match &anchor.kind {
                AnchorKind::FileRegex => {
                    let Some(pattern) = &anchor.pattern else {
                        return (false, "锚点缺少正则模式".to_string());
                    };
                    match Regex::new(pattern) {
                        Ok(re) => {
                            if re.is_match(&content) {
                                (true, format!("文件命中 /{pattern}/"))
                            } else {
                                (false, format!("文件未命中 /{pattern}/"))
                            }
                        }
                        Err(e) => (false, format!("正则编译失败（{e}）")),
                    }
                }
                AnchorKind::FileContains => {
                    let Some(keyword) = &anchor.pattern else {
                        return (false, "锚点缺少关键词".to_string());
                    };
                    if content.contains(keyword.as_str()) {
                        (true, format!("文件包含「{keyword}」"))
                    } else {
                        (false, format!("文件不包含「{keyword}」"))
                    }
                }
                _ => unreachable!("上方 match 已收窄到文件内容类"),
            }
        }
    }
}

/// 文件读取（带大小上限；非 UTF-8 / 超限 / IO 错误统一诚实报错）。
fn read_limited(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("文件读取失败：{e}"))?;
    if meta.len() > MAX_ANCHOR_FILE_BYTES {
        return Err(format!(
            "文件超过 {}MB，锚点检查拒绝读取",
            MAX_ANCHOR_FILE_BYTES / (1024 * 1024)
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("文件读取失败（非 UTF-8？）：{e}"))
}

/// 路径安全·形态闸（解析期，FS 无关）：仅接受 workspace 内相对路径的
/// 字面形态。拒绝：绝对路径 / Windows 盘符与 UNC 形态 / `..` 穿越 /
/// 8.3 短名组件（`RUNNER~1` 家族教训：canonicalize 前先拦，杜绝短名
/// 别名语义绕过）。解析期拒绝 = 不安全锚点回落语义项 + 告警，不产出生
/// 效锚点（T2-3 语义：不安全的验收标准是标准自身的毛病，不惩罚执行者）。
pub fn validate_anchor_path_shape(raw: &str) -> Result<(), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("锚点路径为空".to_string());
    }
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return Err(format!("锚点路径拒绝绝对路径：{raw}"));
    }
    // Windows 形态（UNC / 盘符 / 根相对 `\foo`）在**所有平台**拒绝——本闸
    // 契约是「解析期，FS 无关」（见函数头注释），集群是跨平台的：Linux
    // master 审 Windows worker 形态的锚点必须同样判不安全，不能因运行平台
    // 放空（`C:\evil` 在 Linux 上只是普通文件名字符串，is_absolute=false，
    // 此前 cfg(windows) 门控导致 Linux 全放行——2026-09-12 CI Linux 红根修，
    // remote_file_anchor_gate / parent_malicious_anchor 两测试实证）。
    if raw.starts_with("\\\\") {
        return Err(format!("锚点路径拒绝 UNC 形态：{raw}"));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!("锚点路径拒绝盘符形态：{raw}"));
    }
    if bytes.first() == Some(&b'\\') {
        // Windows 上由下方 components() 的 RootDir 臂兜住；非 Windows 平台
        // `\` 是普通字符，需在此显式拒绝根相对形态。
        return Err(format!("锚点路径拒绝根相对形态（\\foo）：{raw}"));
    }
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(c) => {
                if has_83_short_name(&c.to_string_lossy()) {
                    return Err(format!("锚点路径拒绝 8.3 短名组件：{raw}"));
                }
            }
            std::path::Component::CurDir => {}
            _ => {
                // ParentDir（`..`）/ RootDir（`\foo`）/ Prefix（`C:\`）全部拒绝。
                return Err(format!("锚点路径拒绝越界组件（`..` / 根 / 盘符）：{raw}"));
            }
        }
    }
    Ok(())
}

/// 路径安全·运行时闸（纵深防御后备闸）：形态闸之外再 canonicalize +
/// workspace 前缀校验（拦符号链接越界）。失败 = 锚点未通过（保守——
/// 运行期到达此处的都是形态合法的锚点，失败即真实环境意外）。
fn resolve_anchor_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    validate_anchor_path_shape(raw)?;
    let raw = raw.trim();
    let rel = Path::new(raw);
    let joined = root.join(rel);
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("锚点文件不存在或不可达（{raw}）：{e}"))?;
    let root_canonical = root
        .canonicalize()
        .map_err(|e| format!("workspace 根解析失败：{e}"))?;
    if !canonical.starts_with(&root_canonical) {
        return Err(format!("锚点路径越出 workspace 子树：{raw}"));
    }
    Ok(canonical)
}

/// 8.3 短名组件判定：`NAME~<数字>` 后跟组件结尾或 `.`（`RUNNER~1`、
/// `ABC~1.TXT`；`my~2notes.txt` 这类普通文件名不受影响）。
fn has_83_short_name(comp: &str) -> bool {
    let bytes = comp.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'~' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let after = &bytes[i + 2..];
            let digits = after.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits == after.len() || after[digits] == b'.' {
                return true;
            }
        }
    }
    false
}

/// 锚点组全过？（空组 = 过——无锚点的单完全走旧路径，零回归面。）
pub fn all_passed(results: &[AnchorResult]) -> bool {
    results.iter().all(|r| r.passed)
}

/// 通过摘要（拼进 LLM user prompt：`## 客观锚点检查（已通过）` 段落体）。
pub fn render_anchor_summary(results: &[AnchorResult]) -> String {
    let mut out = String::new();
    for r in results {
        out.push_str(&format!("- ✅ [{}] {}\n", r.anchor.kind.as_str(), r.detail));
    }
    out
}

/// 失败明细（FAIL 短路评论体；per-anchor 逐条可读）。
pub fn render_anchor_failures(results: &[AnchorResult]) -> String {
    let mut out = String::new();
    for r in results.iter().filter(|r| !r.passed) {
        out.push_str(&format!(
            "- ❌ `{}`（{}）：{}\n",
            r.anchor.raw,
            r.anchor.kind.as_str(),
            r.detail
        ));
    }
    out
}

#[cfg(test)]
mod tests;
