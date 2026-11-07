// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// FT（2026-09-17）：文件工具路径解析统一入口——带界时过
/// [`validate_workspace_path`]（相对路径 join 工作区根 + restrict 边界闸，
/// 锚定与限制正交：restrict=false 时相对路径同样锚定到工作区根），无界时
/// 保持旧行为（按进程 cwd 解析，基线/测试形态）。读族（read/list/delete/
/// create_dir/delete_dir）此前不走本入口，exe 直启（cwd=部署目录）形态下
/// 相对路径全部 File not found，且 restrict=true 时无第二道闸。
/// M5（2026-10-27 整改）起 loop 写后钩子（format-on-save / 诊断回灌）同源
/// 消费本入口——执行器与钩子单一真相源，故 pub(crate)。
pub(crate) fn resolve_tool_path(
    path: &str,
    boundary: Option<&WorkspaceBoundary>,
    family: FenceOpFamily,
) -> Result<PathBuf, String> {
    match boundary {
        Some(b) => validate_workspace_path(path, b, family),
        None => Ok(PathBuf::from(path)),
    }
}

/// A tool that reads the contents of a file from disk.
///
/// FT（2026-09-17）：可携带工作区边界（`with_boundary`，生产形态）——
/// 相对路径锚定工作区根 + restrict 时工作区外拒绝（与写族
/// [`WriteFileTool`] 同型）。`default()` 无界（基线注册/测试形态，行为同
/// 边界引入前）。
#[derive(Default)]
pub struct ReadFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl ReadFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn description(&self) -> String {
        "Read the contents of a file. For large files (e.g. spill locator files), pass offset/limit to read a character-based segment instead of the whole file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional start offset in characters (0-based). Use with limit to read large files in segments"
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional max characters to return starting at offset (positive)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = resolve_tool_path(&path, self.boundary.as_deref(), FenceOpFamily::Read)?;

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            // A8（2026-09-04 devtool-upgrade）：非 UTF-8 → magic byte 二进制
            // 分支（图片提示走 vision 附加正道 / PDF 诚实说明 / 其他报字节
            // 数），不再一律报错让模型反复重试。
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Ok(binary_file_summary(&path));
            }
            Err(e) => return Err(format!("Failed to read file: {}", e)),
        };

        // Optional char-based segmentation (offset/limit). This backs the
        // spill marker's promise ("可用 read_file 工具按 offset/limit 分段
        // 读取"): without it, re-reading a >64KB spill file would itself
        // spill again (a locator pointing at another locator). Char-based
        // slicing is multibyte-safe. When neither param is present the raw
        // full content is returned byte-identically (legacy path).
        let (offset, limit) = extract_offset_limit(args)?;
        if offset.is_none() && limit.is_none() {
            return Ok(content);
        }
        let offset = offset.unwrap_or(0);
        let total = content.chars().count();
        // Effective limit defaults to "to end of file" when only offset is given.
        let eff_limit = limit.unwrap_or(total.saturating_sub(offset));
        let slice: String = content.chars().skip(offset).take(eff_limit).collect();
        let returned = slice.chars().count();
        Ok(format!(
            "[read_file 分段] path={} total_chars={} offset={} limit={} chars_returned={}\n{}",
            path.display(),
            total,
            offset,
            eff_limit,
            returned,
            slice
        ))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// A8（2026-09-04 devtool-upgrade）：read_file 对二进制文件的诚实摘要。
///
/// 非 UTF-8 文件不再一律报错：按 magic byte 分类——图片 → 提示走 vision
/// 附加正道（多模态管线已有 base64 上行，不塞 base64 进文本）；PDF →
/// 诚实说明文本提取是远期项；其他二进制 → 报字节数。返回 Ok 说明文本
/// （工具执行成功但内容不可文本化），模型读后自行决策，不再盲目重试。
pub(super) fn binary_file_summary(path: &Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let head = std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            let mut buf = [0u8; 12];
            let mut read = 0;
            while read < buf.len() {
                let n = f.read(&mut buf[read..])?;
                if n == 0 {
                    break;
                }
                read += n;
            }
            Ok(buf[..read].to_vec())
        })
        .unwrap_or_default();
    // PDF：头 `%PDF`（规范允许头 1024 字节内，实际恒在文件最前）。
    if head.starts_with(b"%PDF") {
        return format!(
            "binary PDF file ({} bytes); text extraction is not supported yet - ask the user for the content or process it with a dedicated tool",
            size
        );
    }
    // 图片格式（与多模态附加管线同一张 magic 签名表）。
    if let Some(ext) = crate::image_path_detector::ext_from_magic(&head) {
        return format!(
            "binary {} image file ({} bytes); use vision by attaching the image instead of reading it as text",
            ext, size
        );
    }
    format!("{} bytes binary (type unknown)", size)
}

/// 文件工具的围栏操作族（2026-11-01 editor 开关 × 围栏修复）：开关闸按族
/// 分判——读族只受开关一（Full Access）管辖，写删族受开关一+二管辖，口径
/// 与 nemesis-security `editor_access::WRITE_DELETE_OPS` 一致。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FenceOpFamily {
    /// 读族：read_file / list_dir（及只读消费方：LSP、诊断回灌钩子）。
    Read,
    /// 写删族：write_file / edit_file（含 action=append）/ multiedit /
    /// delete_file / create_dir / delete_dir（及 format-on-save 钩子）。
    /// 2026-11-06：append_file 并入 edit_file（action=append），append 分支
    /// 同走本族闸。
    WriteDelete,
}

impl FenceOpFamily {
    /// 闸门布尔契约（`EditorAccessState::fence_gate`：true = 写删族）。
    fn is_write_delete(self) -> bool {
        matches!(self, Self::WriteDelete)
    }
}

/// write_file / edit_file（含 action=append）的工作区边界。
///
/// 安全 8 层管线之外的第二道闸（纵深防御，不单靠管线）：管线按操作语义
/// 拦截，本边界保证写路径落点不出工作区。`restrict=false` 时形同未设界
/// （用户在 config 显式关掉 `agents.defaults.restrict_to_workspace`）。
#[derive(Clone)]
pub struct WorkspaceBoundary {
    /// 工作区根（gateway 侧 = `<home>/workspace`；executor 子进程侧 =
    /// `NEMESISBOT_EXECUTOR_WORKSPACE`）。
    pub root: PathBuf,
    /// 是否启用限制。
    pub restrict: bool,
    /// 附加放行根（B1 项目 home，2026-10-31）：目标落任一根内即放行。
    /// 项目会话 = `[agent_home]`（Bot 可自改的数据域，围栏第二根）；
    /// 其余形态 = 空（行为与单根版逐字节一致）。
    pub extra_roots: Vec<PathBuf>,
    /// editor 放行开关闸（2026-11-01 两层不认账修复）：Some 时界外路径
    /// 拒绝前先问闸（`(归一后路径, 是否写删族) -> 放行`），判定与审批层
    /// `EditorAccessState::evaluate` 同源——审批层认开关，围栏同一裁决；
    /// 开关全关恒 false = 旧行为。None（主会话按用户拍板暂不接线 /
    /// executor 子进程无开关通路）= 旧行为一概拒绝。闭包签名纯原始类型，
    /// 不引 security 类型（`--no-default-features` 裁剪编译不受影响）。
    pub editor_gate: Option<Arc<dyn Fn(&str, bool) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for WorkspaceBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceBoundary")
            .field("root", &self.root)
            .field("restrict", &self.restrict)
            .field("extra_roots", &self.extra_roots)
            .field("editor_gate", &self.editor_gate.is_some())
            .finish()
    }
}

/// 校验 `path` 是否落在边界内，返回归一后的路径供后续 IO 使用。
///
/// 相对路径先 join 到边界根；双侧 `canonicalize_for_compare` 归一（防
/// symlink escape + Windows 8.3 短名/大小写失配误拒——RUNNER~1 家族），
/// 目标不存在时按最长存在祖先解析（write 新文件路径合法）。
///
/// 2026-11-01：界外且 restrict 时，先问 editor 开关闸（`family` 决定读
/// 族/写删族口径，见 [`WorkspaceBoundary::editor_gate`]），闸放行即放；
/// 闸缺席或说拒 = 旧行为拒绝。
pub fn validate_workspace_path(
    path: &str,
    boundary: &WorkspaceBoundary,
    family: FenceOpFamily,
) -> Result<PathBuf, String> {
    let target = Path::new(path);
    let canonical = if target.is_absolute() {
        target.to_path_buf()
    } else {
        boundary.root.join(target)
    };
    if !boundary.restrict {
        return Ok(canonical);
    }
    let resolved = canonicalize_for_compare(&canonical);
    let ws = canonicalize_for_compare(&boundary.root);
    if resolved.starts_with(&ws) {
        return Ok(canonical);
    }
    // 附加放行根（B1）：与主根同一套 canonicalize 防护（symlink escape /
    // 8.3 短名 / 最长存在祖先），任一根命中即放行。
    for extra in &boundary.extra_roots {
        let extra_canon = canonicalize_for_compare(extra);
        if resolved.starts_with(&extra_canon) {
            return Ok(canonical);
        }
    }
    // editor 放行开关闸（2026-11-01）：传**归一后**路径（逃逸/8.3 形态
    // 按 evaluate 既有语义处理），判定与审批层 evaluate 同源。
    if let Some(gate) = &boundary.editor_gate
        && gate(&resolved.to_string_lossy(), family.is_write_delete())
    {
        return Ok(canonical);
    }
    Err(format!(
        "access denied: path '{}' is outside the workspace",
        path
    ))
}

/// A tool that writes content to a file on disk.
///
/// A5（2026-09-04）：可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct WriteFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl WriteFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn description(&self) -> String {
        "Write content to a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let (path, content) = extract_path_and_content(args)?;

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）。
        let path = match &self.boundary {
            Some(b) => validate_workspace_path(&path, b, FenceOpFamily::WriteDelete)?,
            None => PathBuf::from(&path),
        };

        // A5：覆盖保护——已存在文件先读旧内容（读取失败按新建处理，不挡
        // 写入），覆盖成功后附 unified diff（A2）。
        let old_content = tokio::fs::read_to_string(&path).await.ok();

        // Create parent directories if needed.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        tokio::fs::write(&path, &content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        match old_content {
            Some(old) => {
                let diff = edit_hint::unified_diff(&path.to_string_lossy(), &old, &content);
                let mut out = format!(
                    "Successfully wrote {} bytes to {} (overwrote existing)",
                    content.len(),
                    path.display()
                );
                if !diff.is_empty() {
                    out.push_str(&format!("\n```diff\n{diff}\n```"));
                }
                Ok(out)
            }
            None => Ok(format!(
                "Successfully wrote {} bytes to {}",
                content.len(),
                path.display()
            )),
        }
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let (path, _content) = extract_path_and_content(args).ok()?;
        let kind = if Path::new(&path).exists() {
            FileChangeKind::Modify
        } else {
            FileChangeKind::Create
        };
        Some(FileChange { path, kind })
    }
}

/// A tool that lists the contents of a directory.
///
/// FT（2026-09-17）：可携带工作区边界（同 [`ReadFileTool`] 注记）。
#[derive(Default)]
pub struct ListDirectoryTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl ListDirectoryTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn description(&self) -> String {
        "List files and directories in a path".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to list"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = resolve_tool_path(&path, self.boundary.as_deref(), FenceOpFamily::Read)?;

        if !path.exists() {
            return Err(format!("Directory not found: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }

        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        let mut listing = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Entry error: {}", e))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry
                .metadata()
                .await
                .map_err(|e| format!("Metadata error: {}", e))?;
            let type_tag = if metadata.is_dir() { "dir" } else { "file" };
            let size = metadata.len();
            listing.push(format!("{} [{}] ({} bytes)", name, type_tag, size));
        }

        if listing.is_empty() {
            Ok("(empty directory)".to_string())
        } else {
            // 确定性输出序（2026-11-01）：listing 是给 LLM 的目录清单（模型
            // 可能按位置取第一个条目跟进操作），而 read_dir 顺序是平台实现
            // 细节（NTFS 字典序 vs ext4 哈希序）——与 filesystem.rs /
            // guest/tools.rs / security middleware 的同语义实现保持一致，
            // 按名字排序。
            listing.sort();
            Ok(listing.join("\n"))
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Extract a file path from tool arguments (JSON).
///
/// Expects either a JSON object with a "path" field, or treats the entire
/// string as a path if JSON parsing fails.
pub(super) fn extract_path(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(path) = val.get("path").and_then(|v| v.as_str()) {
            return Ok(path.to_string());
        }
        return Err("Missing 'path' field in arguments".to_string());
    }
    // Fallback: treat raw args as path.
    Ok(args.trim().to_string())
}

/// Extract optional `offset`/`limit` (non-negative integers) from read_file
/// arguments. Returns `Ok((None, None))` when neither is present (legacy
/// full-read path) and when args are not JSON at all (raw-path fallback).
/// Validation errors are explicit so a malformed value is never silently
/// ignored (args_validator would catch type errors first, but this tool is
/// also callable via paths that bypass it).
pub(super) fn extract_offset_limit(args: &str) -> Result<(Option<usize>, Option<usize>), String> {
    let val: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(_) => return Ok((None, None)),
    };
    let parse = |name: &str| -> Result<Option<usize>, String> {
        match val.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| format!("'{name}' must be a non-negative integer, got: {v}"))?;
                Ok(Some(n as usize))
            }
        }
    };
    let offset = parse("offset")?;
    let limit = parse("limit")?;
    if limit == Some(0) {
        return Err("'limit' must be a positive integer (got 0)".to_string());
    }
    Ok((offset, limit))
}

/// Extract path and content from tool arguments (JSON).
///
/// Expects a JSON object with "path" and "content" fields.
pub(super) fn extract_path_and_content(args: &str) -> Result<(String, String), String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

    let path = val
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' field")?
        .to_string();

    let content = val
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content' field")?
        .to_string();

    Ok((path, content))
}

/// Extract path, old_text, and new_text from tool arguments (JSON).
///
/// Expects a JSON object with "path", "old_text", and "new_text" fields.
/// `replace_all` is optional and defaults to false.
pub(super) fn extract_edit_args(args: &str) -> Result<(String, String, String, bool), String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

    let path = val
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' field")?
        .to_string();

    let old_text = val
        .get("old_text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'old_text' field")?
        .to_string();

    let new_text = val
        .get("new_text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'new_text' field")?
        .to_string();

    let replace_all = val
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok((path, old_text, new_text, replace_all))
}

/// A7（2026-09-06）：单条编辑替换决策的**单一真相源**（纯函数）——
/// `edit_file`（A1-A4）与 `multiedit`（A7）共用同一套匹配语义：A1 多命中
/// 歧义提示 → A3 全量替换 → A4 五级模糊级联。错误文案与历史 edit_file
/// 行为逐字一致（A1-A4 测试锁死）。
pub(crate) struct EditOutcome {
    pub new_content: String,
    /// old_text 在原文中的出现次数（`replace_all` 计数文案用；级联命中
    /// 路径恒 0——级联的命中语义由 level 名承载）。
    pub occurrences: usize,
    /// A4 级联命中级名（exact 命中为 None）。
    pub cascade_level: Option<&'static str>,
}

/// 对 `content` 应用一条替换决策。`path_disp` 仅用于错误文案。
pub(crate) fn apply_edit_to_content(
    content: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
    path_disp: &str,
) -> Result<EditOutcome, String> {
    let count = content.matches(old_text).count();
    if count > 1 && !replace_all {
        let lines = edit_hint::match_line_numbers(content, old_text);
        return Err(format!(
            "old_text appears {} times in {} (at lines {:?}). Provide more \
             surrounding context to make it unique, or use replace_all=true to \
             replace every occurrence",
            count, path_disp, lines
        ));
    }

    if count == 0 {
        if replace_all {
            return Err(format!(
                "old_text not found in {}. {}",
                path_disp,
                edit_hint::build_not_found_hint(content, old_text)
            ));
        }
        // A4：exact 未命中且非 replace_all → 五级模糊替换级联。replace_all 不进
        // 级联：模糊命中位置内容各不相同，"全部替换"语义无法推广。
        return match edit_replacers::cascade_replace(content, old_text, new_text) {
            Ok(m) => Ok(EditOutcome {
                new_content: m.content,
                occurrences: 0,
                cascade_level: Some(m.level),
            }),
            Err(edit_replacers::CascadeError::Ambiguous(msg)) => Err(msg),
            Err(edit_replacers::CascadeError::NoMatch { span_note }) => {
                let mut msg = format!("old_text not found in {}. ", path_disp);
                if let Some(note) = span_note {
                    msg.push_str(&note);
                }
                msg.push_str(&edit_hint::build_not_found_hint(content, old_text));
                Err(msg)
            }
        };
    }

    // A3：replace_all=true → 全部替换。注意 Rust 语义：`replacen(…, 0)` 是
    // 「最多换 0 处」（什么都不换）而非全量——全量是 `str::replace`
    // （实施计划该处笔误，已按 §1.1 核实现修订）。
    let new_content = if replace_all {
        content.replace(old_text, new_text)
    } else {
        content.replacen(old_text, new_text, 1)
    };
    Ok(EditOutcome {
        new_content,
        occurrences: count,
        cascade_level: None,
    })
}

/// A tool that edits a file — action-discriminated（2026-11-06 二合一收编）：
///
/// - `action = "edit"`（缺省，兼容旧调用形态）：精确替换 old_text →
///   new_text。By default old_text must occur exactly once;
///   `replace_all=true` replaces every occurrence.
/// - `action = "append"`：把 content 追加到文件末尾（旧 `append_file`
///   工具收编，执行逻辑逐字节搬运——create 缺失父目录 + append 模式打开 +
///   flush）。
///
/// A5（2026-09-04）：可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct EditFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl EditFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn description(&self) -> String {
        "Edit a file on disk. action=\"edit\" (default): replace old_text with new_text — by default old_text must occur exactly once, set replace_all=true to replace every occurrence. action=\"append\": append content to the end of the file (creating it if missing).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the file to edit"},
                "action": {"type": "string", "enum": ["edit", "append"], "description": "Which edit mode to use. Default: edit."},
                "old_text": {"type": "string", "description": "(action=edit) Exact text to find and replace"},
                "new_text": {"type": "string", "description": "(action=edit) Replacement text"},
                "replace_all": {"type": "boolean", "description": "(action=edit) Replace every occurrence of old_text instead of requiring a unique match (default: false)"},
                "content": {"type": "string", "description": "(action=append) Content to append to the end of the file"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        // action 判别（缺省 = edit，旧调用形态 {path, old_text, new_text}
        // 逐字节兼容——旧 schema required 三字段，此处以显式校验补位）。
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;
        let action = val.get("action").and_then(|v| v.as_str()).unwrap_or("edit");
        match action {
            "edit" => self.execute_edit(args).await,
            "append" => self.execute_append(args).await,
            other => Err(format!(
                "unknown action '{other}' — expected 'edit' or 'append'"
            )),
        }
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let val: serde_json::Value = serde_json::from_str(args).ok()?;
        let path = val.get("path")?.as_str()?.to_string();
        let action = val.get("action").and_then(|v| v.as_str()).unwrap_or("edit");
        let kind = if action == "append" {
            // append 分支允许目标不存在（create=true），按存在性如实申报。
            if Path::new(&path).exists() {
                FileChangeKind::Modify
            } else {
                FileChangeKind::Create
            }
        } else {
            // edit 分支要求文件已存在（execute errors otherwise）。
            FileChangeKind::Modify
        };
        Some(FileChange { path, kind })
    }
}

impl EditFileTool {
    /// action=edit（旧 EditFileTool::execute 原样搬运，行为零变更）。
    async fn execute_edit(&self, args: &str) -> Result<String, String> {
        let (path, old_text, new_text, replace_all) = extract_edit_args(args)?;

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）。
        let path = match &self.boundary {
            Some(b) => validate_workspace_path(&path, b, FenceOpFamily::WriteDelete)?,
            None => PathBuf::from(&path),
        };
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // A1/A3/A4 匹配语义已抽出为 [`apply_edit_to_content`]（A7 起与
        // multiedit 共用单一真相源）；此处只做 IO 与回执组装。
        let outcome = apply_edit_to_content(
            &content,
            &old_text,
            &new_text,
            replace_all,
            &path.to_string_lossy(),
        )?;

        tokio::fs::write(path, &outcome.new_content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        // A2（2026-09-04）：成功回 unified diff（模型自检 + 前端渲染 +
        // 审计可读；超长回退统计摘要，全量语义由 loop 层 spill 兜底）。
        let diff = edit_hint::unified_diff(&path.to_string_lossy(), &content, &outcome.new_content);
        // A3：replace_all 在头部报替换计数（count>=1 恒成立——0 已被 not-found
        // 拦截；无差异时也报计数，诚实反映发生了什么）。
        let header = if let Some(level) = outcome.cascade_level {
            format!("File edited: {} (matched via {})", path.display(), level)
        } else if replace_all {
            format!(
                "File edited: {} ({} occurrences replaced)",
                path.display(),
                outcome.occurrences
            )
        } else {
            format!("File edited: {}", path.display())
        };
        if diff.is_empty() {
            return Ok(header);
        }
        Ok(format!("{}\n```diff\n{}```", header, diff))
    }

    /// action=append（旧 AppendFileTool::execute 原样搬运，行为零变更）。
    async fn execute_append(&self, args: &str) -> Result<String, String> {
        let (path, content) = extract_path_and_content(args)?;

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）。
        let path = match &self.boundary {
            Some(b) => validate_workspace_path(&path, b, FenceOpFamily::WriteDelete)?,
            None => PathBuf::from(&path),
        };
        let path = path.as_path();

        // Create parent directories if needed.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        // Use OpenOptions for append mode.
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|e| format!("Failed to open file: {}", e))?;

        file.write_all(content.as_bytes())
            .await
            .map_err(|e| format!("Failed to append to file: {}", e))?;
        // tokio fs::File 的 write_all 只提交到后台任务，不 flush 则数据可能
        // 尚未写入文件（drop 不等待未完成 IO）。
        file.flush()
            .await
            .map_err(|e| format!("Failed to flush file: {}", e))?;

        Ok(format!(
            "Appended {} bytes to {}",
            content.len(),
            path.display()
        ))
    }
}

/// A7（2026-09-06）：multiedit 批量编辑——`edits: [{path, old_text,
/// new_text, replace_all?}, ...]`，**原子性**：全部编辑先在内存完成（同
/// 文件多条编辑按序累积生效），任一失败则整批不落盘并回灌首个失败详情 +
/// 逐条状态清单；全部成功才统一写盘（只写内容实际变化的文件），逐文件
/// 回 unified diff。匹配语义与 `edit_file` 共用 [`apply_edit_to_content`]
/// 单一真相源。
///
/// A5（2026-09-04）同款边界形态：`with_boundary` 生产 / `default()` 基线。
#[derive(Default)]
pub struct MultiEditTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

/// 一条批量编辑条目（schema 校验后的形态）。
struct MultiEditEntry {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
}

/// 解析 multiedit args：`edits` 必须是非空数组，每条必须带字符串
/// `path`/`old_text`/`new_text`（`replace_all` 可选布尔）。
fn extract_multiedit_args(args: &str) -> Result<Vec<MultiEditEntry>, String> {
    let val: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("Invalid JSON args: {}", e))?;
    let arr = val
        .get("edits")
        .and_then(|v| v.as_array())
        .ok_or("Missing 'edits' field (must be a non-empty array)")?;
    if arr.is_empty() {
        return Err("'edits' must be a non-empty array".to_string());
    }
    arr.iter()
        .enumerate()
        .map(|(i, e)| {
            let need = |k: &str| {
                e.get(k)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("edits[{}]: missing string field '{}'", i, k))
            };
            Ok(MultiEditEntry {
                path: need("path")?,
                old_text: need("old_text")?,
                new_text: need("new_text")?,
                replace_all: e
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

impl MultiEditTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for MultiEditTool {
    fn description(&self) -> String {
        "Apply multiple text replacements across one or more files in a single \
         atomic operation. All edits are applied in memory first; if any edit \
         fails, nothing is written to disk. Multiple edits to the same file are \
         applied sequentially (later edits see earlier results)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "description": "Edits to apply atomically",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string", "description": "Path to the file to edit"},
                            "old_text": {"type": "string", "description": "Exact text to find and replace"},
                            "new_text": {"type": "string", "description": "Replacement text"},
                            "replace_all": {"type": "boolean", "description": "Replace every occurrence of old_text (default: false)"}
                        },
                        "required": ["path", "old_text", "new_text"]
                    }
                }
            },
            "required": ["edits"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let edits = extract_multiedit_args(args)?;
        let total = edits.len();

        // 内存工作区：path → (原文, 当前内容, 已应用条数, 级联名)。
        // 同一文件的多条编辑按序累积（后者看到前者的结果）。
        struct MemFile {
            path: PathBuf,
            original: String,
            current: String,
            edits_applied: usize,
            levels: Vec<&'static str>,
        }
        let mut files: Vec<MemFile> = Vec::new();
        let mut statuses: Vec<String> = Vec::with_capacity(total);

        for (i, e) in edits.iter().enumerate() {
            let label = format!("edit {}/{}", i + 1, total);
            // A5：工作区边界（纵深防御，与 edit_file 同源校验）。
            let vpath = match &self.boundary {
                Some(b) => match validate_workspace_path(&e.path, b, FenceOpFamily::WriteDelete) {
                    Ok(p) => p,
                    Err(m) => {
                        statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                        return Err(format!(
                            "multiedit aborted at {}: {}\n\nNo files were \
                             modified. Edit status:\n{}",
                            label,
                            m,
                            statuses.join("\n")
                        ));
                    }
                },
                None => PathBuf::from(&e.path),
            };
            let idx = match files.iter().position(|f| f.path == vpath) {
                Some(i) => i,
                None => {
                    if !vpath.exists() {
                        let m = format!("File not found: {}", vpath.display());
                        statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                        return Err(format!(
                            "multiedit aborted at {}: {}\n\nNo files were \
                             modified. Edit status:\n{}",
                            label,
                            m,
                            statuses.join("\n")
                        ));
                    }
                    let content = match tokio::fs::read_to_string(&vpath).await {
                        Ok(c) => c,
                        Err(err) => {
                            let m = format!("Failed to read {}: {}", vpath.display(), err);
                            statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                            return Err(format!(
                                "multiedit aborted at {}: {}\n\nNo files were \
                                 modified. Edit status:\n{}",
                                label,
                                m,
                                statuses.join("\n")
                            ));
                        }
                    };
                    files.push(MemFile {
                        path: vpath,
                        original: content.clone(),
                        current: content,
                        edits_applied: 0,
                        levels: Vec::new(),
                    });
                    files.len() - 1
                }
            };
            // 匹配语义单一真相源（与 edit_file 同函数）；失败整批中止。
            match apply_edit_to_content(
                &files[idx].current,
                &e.old_text,
                &e.new_text,
                e.replace_all,
                &files[idx].path.to_string_lossy(),
            ) {
                Ok(outcome) => {
                    if let Some(level) = outcome.cascade_level {
                        files[idx].levels.push(level);
                    }
                    files[idx].current = outcome.new_content;
                    files[idx].edits_applied += 1;
                    statuses.push(format!("  {}. {} — ok", i + 1, e.path));
                }
                Err(m) => {
                    statuses.push(format!("  {}. {} — FAILED: {}", i + 1, e.path, m));
                    return Err(format!(
                        "multiedit aborted at {}: {}\n\nNo files were modified. \
                         Edit status:\n{}",
                        label,
                        m,
                        statuses.join("\n")
                    ));
                }
            }
        }

        // 全部成功 → 统一写盘（只写内容实际变化的文件，避免无谓 mtime）。
        let mut written: Vec<String> = Vec::new();
        for f in &files {
            if f.current == f.original {
                continue;
            }
            if let Err(err) = tokio::fs::write(&f.path, &f.current).await {
                // 磁盘写阶段失败无跨文件事务——诚实报告已写清单。
                return Err(format!(
                    "multiedit write failed on {}: {}. Files already written: {}",
                    f.path.display(),
                    err,
                    if written.is_empty() {
                        "(none)".to_string()
                    } else {
                        written.join(", ")
                    }
                ));
            }
            written.push(f.path.to_string_lossy().to_string());
        }

        // 回执：总数 + 逐文件 diff 汇总。
        let mut out = format!(
            "Multiedit complete: {} edit(s) across {} file(s).",
            total,
            files.len()
        );
        for f in &files {
            out.push_str(&format!(
                "\n\n── {} ({} edit(s)",
                f.path.display(),
                f.edits_applied
            ));
            if !f.levels.is_empty() {
                out.push_str(&format!(", matched via {}", f.levels.join(", ")));
            }
            out.push(')');
            let diff = edit_hint::unified_diff(&f.path.to_string_lossy(), &f.original, &f.current);
            if diff.is_empty() {
                out.push_str("\n(no change)");
            } else {
                out.push_str(&format!("\n```diff\n{}```", diff));
            }
        }
        Ok(out)
    }

    fn preview(&self, _args: &str) -> Option<FileChange> {
        // multiedit 的 checkpoint 预检走 [`Tool::preview_all`]（多文件）；
        // 单点 `preview` 无从选代表文件，诚实返回 None。
        None
    }

    fn preview_all(&self, args: &str) -> Vec<FileChange> {
        let Ok(edits) = extract_multiedit_args(args) else {
            return Vec::new();
        };
        // 去重保序（同文件多条编辑只快照一次——checkpoint seen 表本就幂等）。
        let mut seen = std::collections::HashSet::new();
        edits
            .into_iter()
            .filter(|e| seen.insert(e.path.clone()))
            .map(|e| FileChange {
                path: e.path,
                kind: FileChangeKind::Modify,
            })
            .collect()
    }
}

/// A tool that deletes a file from disk.
///
/// FT（2026-09-17）：可携带工作区边界（同 [`ReadFileTool`] 注记）——删除是
/// 破坏性操作，restrict=true 时工作区外路径必须有第二道闸（纵深防御）。
#[derive(Default)]
pub struct DeleteFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl DeleteFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn description(&self) -> String {
        "Delete a file".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to delete"}},"required":["path"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = resolve_tool_path(&path, self.boundary.as_deref(), FenceOpFamily::WriteDelete)?;

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        if path.is_dir() {
            return Err(format!(
                "Path is a directory, not a file: {}",
                path.display()
            ));
        }

        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| format!("Failed to delete file: {}", e))?;

        Ok(format!("Deleted file: {}", path.display()))
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        let path = extract_path(args).ok()?;
        Some(FileChange {
            path,
            kind: FileChangeKind::Delete,
        })
    }
}

/// A tool that creates a directory (and all parent directories).
///
/// FT（2026-09-17）：可携带工作区边界（同 [`ReadFileTool`] 注记）。
#[derive(Default)]
pub struct CreateDirTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl CreateDirTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for CreateDirTool {
    fn description(&self) -> String {
        "Create a directory".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the directory to create"}},"required":["path"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = resolve_tool_path(&path, self.boundary.as_deref(), FenceOpFamily::WriteDelete)?;

        if path.exists() {
            return Err(format!("Path already exists: {}", path.display()));
        }

        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| format!("Failed to create directory: {}", e))?;

        Ok("Directory created".to_string())
    }
}

/// A tool that removes a directory.
///
/// FT（2026-09-17）：可携带工作区边界（同 [`ReadFileTool`] 注记）——删除是
/// 破坏性操作，restrict=true 时工作区外路径必须有第二道闸（纵深防御）。
#[derive(Default)]
pub struct DeleteDirTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl DeleteDirTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for DeleteDirTool {
    fn description(&self) -> String {
        "Delete a directory and all its contents".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the directory to delete"}},"required":["path"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = resolve_tool_path(&path, self.boundary.as_deref(), FenceOpFamily::WriteDelete)?;

        if !path.exists() {
            return Err(format!("Directory not found: {}", path.display()));
        }

        if !path.is_dir() {
            return Err(format!("Path is not a directory: {}", path.display()));
        }

        // 韧性删除：Defender/索引器瞬时句柄是常态，单发失败 = 对模型假报错；
        // spawn_blocking 装同步重试内核（最坏 ~4.4s，不占 async worker）。
        let retry_path = path.clone();
        tokio::task::spawn_blocking(move || {
            nemesis_utils::fsutil::remove_dir_all_retry(&retry_path)
        })
        .await
        .map_err(|e| format!("Failed to remove directory: {e}"))?
        .map_err(|e| format!("Failed to remove directory: {}", e))?;

        Ok("Directory removed".to_string())
    }
}

// ---------------------------------------------------------------------------
// ExecTool - Shell command execution (mirrors Go's ExecTool)
// ---------------------------------------------------------------------------
