//! 评审取证自取（evidence-on-demand，2026-10-15）：`board_evidence_read`
//! 工具——评审 agent 在项目档案根围栏内只读交付文件。
//!
//! 背景（NB-102/NB-103 真机实证）：变更集注入层是「任意硬编码上限」——
//! 单文件节选、总预算、文件数名额，超限即「未附内容」，评审员对内容无法
//! 核对只能 UNSURE 转人工。上限再宽也是猜的（「随便写点小作文不就超了」
//! ——用户 2026-10-15）。正确性不该由注入预算决定：清单与节选只是省轮次
//! 的快路径，穷尽证据的通路是让评审员自己读——档案已由系统合并到 master
//! 本机（`board-projects/<项目>/`），「远端 worker 工件不在本机」的张冠李
//! 戴顾虑对档案围栏工具不成立（[`ReviewToolMode`] 顶注）。
//!
//! 围栏（安全 8 层之前的结构性第一道闸）：
//! - issue 参数解析本单单号 → project_id → 档案根（`project_archive_root`
//!   单一真相源，目录缺失诚实报错）；
//! - path 参数过 [`nemesis_board::resolve_subtree_path`]（与锚点实核同一
//!   逻辑单点：拒绝对路径 / `..` / UNC / 盘符 / 8.3 短名，canonicalize 后
//!   强制仍在档案根子树内——符号链接越界同拦）；
//! - 非 UTF-8 = 诚实报错不落评审上下文（二进制内容没有文本评审意义）；
//! - 操作分类 FileRead（LOW）：注入检测/凭据扫描/DLP/审计链全跑。
//!
//! 注册面：仅主 agent（`agent_factory.rs` 8b 块，store 未装配 = 不注册）。
//! 主对话（big 档全量供给）也会看到本工具——它对主 agent 同样无害且有用
//! （「读一下 NB-103 交付的 PRD」）；mini/normal 档走 `tier_allowed_tools`
//! 白名单天然不含本工具。
//!
//! 评审消费：`board_review.rs` 的 `ReviewToolMode::EvidenceOnly`（远端
//! worker 评审）/ `ReadOnly`（本机取证）把本工具放进 `DetachedOpts.allowed_tools`。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;

/// 工具注册名。
pub const TOOL_NAME: &str = "board_evidence_read";

/// board_evidence_read 工具：按单号解析项目档案根，围栏内只读取文件。
pub struct BoardEvidenceTool {
    store: Arc<nemesis_board::BoardStore>,
}

impl BoardEvidenceTool {
    pub fn new(store: Arc<nemesis_board::BoardStore>) -> Self {
        Self { store }
    }

    /// issue 引用 → Issue：纯数字 = DB id；`NB-<n>` = 单号（与
    /// board_issue_tool 同形——两处都是薄解析，语义单点在 store）。
    fn resolve_issue(&self, issue_ref: &str) -> Result<nemesis_board::Issue, String> {
        let r = issue_ref.trim();
        if let Some(n) = r.strip_prefix("NB-").or_else(|| r.strip_prefix("nb-")) {
            return self.store.get_issue_by_number(&format!("NB-{n}"));
        }
        r.parse::<i64>()
            .map_err(|_| format!("issue 引用 {r:?} 无法识别（应为数字 id 或 NB-<n> 单号）"))
            .and_then(|id| self.store.get_issue(id))
    }
}

/// 解析后的入参（execute 内部第一步；独立成纯结构便于单测）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EvidenceReadArgs {
    pub issue: String,
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// args JSON → 结构化入参（issue/path 必填诚实报错；offset/limit 非负整数
/// 可选——args_validator 兜 schema 层，语义层这里自己守）。
pub(crate) fn parse_evidence_args(args: &str) -> Result<EvidenceReadArgs, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    let issue = v
        .get("issue")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if issue.is_empty() {
        return Err("missing field: issue (NB-<n> or numeric id)".to_string());
    }
    let path = v
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if path.is_empty() {
        return Err(
            "missing field: path (archive-relative path from the merged-changeset listing)"
                .to_string(),
        );
    }
    let offset = match v.get("offset") {
        None | Some(serde_json::Value::Null) => None,
        Some(n) => Some(
            usize::try_from(n.as_u64().ok_or("offset must be a non-negative integer")?)
                .map_err(|_| "offset 超出平台指针宽度")?,
        ),
    };
    let limit = match v.get("limit") {
        None | Some(serde_json::Value::Null) => None,
        Some(n) => Some(
            usize::try_from(n.as_u64().ok_or("limit must be a positive integer")?)
                .map_err(|_| "limit 超出平台指针宽度")?,
        ),
    };
    Ok(EvidenceReadArgs {
        issue,
        path,
        offset,
        limit,
    })
}

/// 档案围栏读（纯函数，单测钉死）：路径安全单点 → 文本判定 → 字符分段。
/// 无 offset/limit 时全文返回（超长输出的溢出保护由 loop 的 spill 机制承
/// 担，与 read_file 同一层）；分段形态与 read_file 的 `[read_file 分段]`
/// 头同构，评审员对两种格式零适配成本。
pub(crate) fn read_archive_file(
    root: &std::path::Path,
    raw_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let path = nemesis_board::resolve_subtree_path(root, raw_path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("读取失败（{raw_path}）：{e}"))?;
    let content = String::from_utf8(bytes).map_err(|_| {
        format!("档案文件不是文本（{raw_path}）：二进制内容不进评审上下文，请以清单/锚点结论为准")
    })?;
    let total = content.chars().count();
    let (offset, limit) = match (offset, limit) {
        (None, None) => (0, total),
        _ => {
            let off = offset.unwrap_or(0);
            if total > 0 && off >= total {
                return Err(format!("offset {off} 超出文件长度（total_chars={total}）"));
            }
            (off, limit.unwrap_or(total.saturating_sub(off)))
        }
    };
    let slice: String = content.chars().skip(offset).take(limit).collect();
    let returned = slice.chars().count();
    Ok(format!(
        "[board_evidence_read] path={raw_path} total_chars={total} offset={offset} limit={limit} chars_returned={returned}\n{slice}"
    ))
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for BoardEvidenceTool {
    fn description(&self) -> String {
        "Read a delivered file from a board issue's project archive (read-only, path-fenced). \
         Use it during review when the injected merged-changeset listing/snippets don't cover \
         the content you need to verify. Pass the issue number (NB-<n>) and the archive-relative \
         path exactly as shown in the listing; supports offset/limit character paging for large \
         files. Rejects absolute paths / `..` / Windows 8.3 short names; binary (non-UTF-8) \
         files are reported honestly instead of dumped."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "issue": {
                    "type": "string",
                    "description": "Board issue number (NB-<n>) or numeric id; resolves to the owning project's archive directory."
                },
                "path": {
                    "type": "string",
                    "description": "Archive-relative file path as shown in the merged-changeset listing (e.g. \"docs/prd-variant-A.md\")."
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional 0-based character offset to start reading from."
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional max characters to return starting at offset."
                }
            },
            "required": ["issue", "path"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed = parse_evidence_args(args)?;
        let issue = self.resolve_issue(&parsed.issue)?;
        let project_id = issue
            .project_id
            .ok_or("该单未挂项目（无档案目录），没有可读取的交付档案")?;
        let project = self
            .store
            .get_project(project_id)
            .map_err(|e| format!("项目查询失败: {e}"))?;
        let root = crate::board_review::project_archive_root(project.directory.as_deref())?;
        read_archive_file(&root, &parsed.path, parsed.offset, parsed.limit)
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
