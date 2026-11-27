// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// FT：文件工具路径解析统一入口——带界时过
/// [`validate_workspace_path`]（相对路径 join 工作区根 + restrict 边界闸，
/// 锚定与限制正交：restrict=false 时相对路径同样锚定到工作区根），无界时
/// 保持旧行为（按进程 cwd 解析，基线/测试形态）。读族（read/list/delete/
/// create_dir/delete_dir）此前不走本入口，exe 直启（cwd=部署目录）形态下
/// 相对路径全部 File not found，且 restrict=true 时无第二道闸。
/// 起 loop 写后钩子（format-on-save / 诊断回灌）同源
/// 消费本入口——执行器与钩子统一定义，故 pub(crate)。
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

/// 知识库重定向（天工系统 v1）完整链：围栏前 redirect（绝对路径命中
/// ——restrict=true 时围栏外的本地缺席三件套先改写成镜像路径才能进围
/// 栏）→ resolve_tool_path（相对路径锚定边界根 + 围栏闸）→ 围栏后
/// redirect（相对路径锚定后的二次命中）。redirect 纯函数幂等（改写
/// 结果不在 code_root 子树内恒原样），两段调用无害。knowledge 缺席
/// （基线/测试/exec_worker 形态）= 与旧 resolve_tool_path 行为逐字节
/// 一致。
pub(crate) fn resolve_tool_path_with_redirect(
    path: &str,
    boundary: Option<&WorkspaceBoundary>,
    family: FenceOpFamily,
    knowledge: Option<&crate::knowledge::KnowledgeHandle>,
) -> Result<PathBuf, String> {
    let pre = crate::knowledge::maybe_redirect_path(Path::new(path), knowledge);
    let resolved = resolve_tool_path(&pre.to_string_lossy(), boundary, family)?;
    Ok(crate::knowledge::maybe_redirect_path(&resolved, knowledge))
}

/// preview 侧路径解析（best-effort）：与 execute 同链（复用
/// [`resolve_tool_path_with_redirect`——单一真相源，写工具 preview 的
/// 声明路径必须等于 execute 的落盘路径，否则 checkpoint/rewind 对
/// 重定向写不设防：快照记原路径、镜像写永不回滚，第五轮横扫
/// 2026-11-27 修正）。失败（围栏拒绝等）= **None**（BUG-20261127-048，
/// 契约锚 `fs_preview_kind_tests::preview_write_outside_ws_returns_none`，
/// BUG-20261127-039 落的锁）：围栏必死的写不许预览基线——execute 届时
/// 同样失败不产生写入，但 Some 会令 checkpoint 捕获界外路径的幻影
/// 基线，rewind 还原臂反而**写工作区外的文件**（restore 的 written
/// 臂不设围栏）。曾短暂回落原路径（第五轮横扫中间态），判定被测试
/// 契约否决。knowledge=None 时仍须走边界解析（BUG-20261127-039：
/// preview 的存在性判定必须与 execute 同源——裸相对路径对照进程 CWD
/// 会把工作区已有文件系统性误报 Create → checkpoint 基线漏捕 →
/// rewind 整删；合并天工链时此语义并入本单一真相源）。preview 契约
/// （[`crate::loop::tools_trait::Tool::preview`]）：绝不 panic。
fn preview_resolve_path(
    path: &str,
    boundary: Option<&WorkspaceBoundary>,
    knowledge: Option<&crate::knowledge::KnowledgeHandle>,
) -> Option<PathBuf> {
    resolve_tool_path_with_redirect(path, boundary, FenceOpFamily::WriteDelete, knowledge).ok()
}

/// A tool that reads the contents of a file from disk.
///
/// FT：可携带工作区边界（`with_boundary`，生产形态）——
/// 相对路径锚定工作区根 + restrict 时工作区外拒绝（与写族
/// [`WriteFileTool`] 同型）。`default()` 无界（基线注册/测试形态，行为同
/// 边界引入前）。
/// read_file 单次全量加载上限（字节）。下游 prune（≤8192 内联）/spill
/// （≥65536 落盘）数字闸都作用在**已读进内存的结果串**上，救不了读侧：
/// `read_to_string` 对 GB 级文件会在 spill 判定前把整个文件拉进 RAM →
/// gateway OOM。帽值对齐家族口径 `search_code::MAX_GREP_SINGLE_FILE_BYTES`
/// （20MB，同款「全量入内存仍需有界」理由）——grep 超限文案指路
/// read_file 分段，两帽一致避免 16-20MB 死区；20MB ≫ exec 排水帽
/// （1MB，`PIPE_CAPTURE_CAP_BYTES`），正常工具输出与 spill 文件恒在帽内
/// 可回读。offset/limit 是字符口径全文定位，超帽文件分段同样需要全量
/// 加载，故一并拒绝（诚实暴露而非假装支持）。
const READ_FILE_MAX_BYTES: u64 = 20_000_000;

#[derive(Default)]
pub struct ReadFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
    /// 知识库重定向（天工系统 v1）：Some = 命中 code_root 子树的本地缺席
    /// 三件套路径改写为知识库镜像（围栏前+围栏后双段，见
    /// [`resolve_tool_path_with_redirect`]）；None = 行为与引入前一致。
    knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
}

impl ReadFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
            knowledge: None,
        }
    }

    /// 知识库重定向接线（registry 生产形态注入；重复调用覆盖）。
    pub fn with_knowledge(
        mut self,
        knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
    ) -> Self {
        self.knowledge = knowledge;
        self
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn description(&self) -> String {
        "Read the contents of a file. For large files (e.g. spill locator files), pass offset/limit to read a character-based segment instead of the whole file. Files above 20MB are rejected (use exec with head/tail/grep for byte-level access instead)".to_string()
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
        let path = resolve_tool_path_with_redirect(
            &path,
            self.boundary.as_deref(),
            FenceOpFamily::Read,
            self.knowledge.as_deref(),
        )?;

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        // 读侧内存闸：超帽文件拒绝（含 offset/limit 分段请求——字符口径
        // 分段同样要全量加载，诚实报错并指引字节口径工具）。
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| format!("Failed to stat file: {}", e))?;
        if meta.len() > READ_FILE_MAX_BYTES {
            return Err(format!(
                "File too large for read_file: {} bytes (cap 20MB). Even offset/limit segmentation loads the whole file — use exec with byte-level tools instead (head/tail/sed/grep), e.g. `exec head -c 65536 \"{}\"`",
                meta.len(),
                path.display()
            ));
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            // （devtool-upgrade）：非 UTF-8 → magic byte 二进制
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

/// （devtool-upgrade）：read_file 对二进制文件的诚实摘要。
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

/// 文件工具的围栏操作族（editor 开关 × 围栏修复）：开关闸按族
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
    /// 附加放行根（项目 home）：目标落任一根内即放行。
    /// 项目会话 = `[agent_home]`（Bot 可自改的数据域，围栏第二根）；
    /// 其余形态 = 空（行为与单根版逐字节一致）。
    pub extra_roots: Vec<PathBuf>,
    /// editor 放行开关闸（两层不认账修复）：Some 时界外路径
    /// 拒绝前先问闸（`(归一后路径, 是否写删族) -> 放行`），判定与审批层
    /// `EditorAccessState::evaluate` 同源——审批层认开关，围栏同一裁决；
    /// 开关全关恒 false = 旧行为。None（主会话按暂不接线 /
    /// executor 子进程无开关通路）= 旧行为一概拒绝。闭包签名纯原始类型，
    /// 不引 security 类型（`--no-default-features` 裁剪编译不受影响）。
    pub editor_gate: Option<Arc<dyn Fn(&str, bool) -> bool + Send + Sync>>,
    /// 目标系统文件保护闸（D-Q3 创建者二分）：Some 时**写删族**路径在
    /// restrict 短路**之前**先过守卫（restrict=false 也保护人配目标）；
    /// 读族不设防。输入 workspace 相对路径（`/` 分隔归一），返回
    /// Some(拒绝理由) = 拒、None = 放行。裁决逻辑单一真相源在
    /// nemesis-goals `gate::fence_denial`（goals/ 下按创建者二分 +
    /// evals.jsonl/注册表恒拒）；主 agent / 集群 worker / executor 子
    /// 进程装守卫，项目 loop（spec.rs）不装（root=项目目录，主 ws
    /// goals/ 不可达）。None = 不设防（feature 裁剪形态）。
    pub goals_guard: Option<Arc<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    /// 审计面日志树保护闸：true 时**写删族**路径落在边界根的 `logs/` 子树
    /// 内 = 拒绝（restrict 短路之前检查，restrict=false 也不豁免）。
    ///
    /// 威胁模型：`<workspace>/logs/` 整树是系统观测面（security_logs 审计、
    /// cluster_logs 请求日志、session_logs 对话历史、boundary、request_logs、
    /// spill、checkpoints…），全部写手是系统内部 IO（不走工具闸）。消费面
    /// （Dashboard 安全审计 tab `logs.security` 盲 glob `security_audit_*.log`、
    /// 集群任务列表 glob 设备目录）对 agent 可创建的文件不做身份校验——
    /// write_file 写一个 `logs/security_audit_2099-01-01.log` 即可在安全审计
    /// 视图顶部注入伪造 allowed 行（timestamp 字符串降序排最顶），单方面
    /// 污染审计可观测面。审计的意义是「不信任 agent 的外部真相」，故本闸
    /// 不随 restrict 开关松动、不受 editor_gate 旁路。读族不设防（agent
    /// 读日志自查合法且有用）。
    ///
    /// 主 workspace / 集群 worker / executor 子进程 = true（root 是 bot
    /// workspace，logs/ 是审计面）；项目 loop（spec.rs）= false——root 是
    /// 用户项目目录，项目自身的 logs/（如 node 项目的运行日志）是项目内容，
    /// agent 开发期写它完全合法。装配声明制：保护范围由构造点显式定，
    /// 不按根名猜测。
    pub protect_log_tree: bool,
}

impl std::fmt::Debug for WorkspaceBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceBoundary")
            .field("root", &self.root)
            .field("restrict", &self.restrict)
            .field("extra_roots", &self.extra_roots)
            .field("editor_gate", &self.editor_gate.is_some())
            .field("goals_guard", &self.goals_guard.is_some())
            .field("protect_log_tree", &self.protect_log_tree)
            .finish()
    }
}

/// 校验 `path` 是否落在边界内，返回归一后的路径供后续 IO 使用。
///
/// 相对路径先 join 到边界根；双侧 `canonicalize_for_compare` 归一（防
/// symlink escape + Windows 8.3 短名/大小写失配误拒——RUNNER~1 家族），
/// 目标不存在时按最长存在祖先解析（write 新文件路径合法）。
///
/// 界外且 restrict 时，先问 editor 开关闸（`family` 决定读
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
    // 审计面日志树保护：写删族落 `logs/` 子树 = 拒（restrict 短路之前，
    // protect_log_tree=true 的装配不随边界开关松动）。归一后按**首组件**
    // 精确比对（`logsfoo/` 不误伤；`logs/../x` 经 canonicalize_for_compare
    // 出栈成 `x` 天然放行——实际落点不在审计面；悬空 symlink 逃逸标记的
    // 前导 `..` 使 strip_prefix 失败、不命中本闸，由既有界外语义处理）。
    // `logs` 本身（delete_dir logs / create_dir logs\x）同拒。Windows 下
    // 首组件大小写折叠比对——NTFS 大小写不敏感（保留创建形态但同名同
    // 目录），`LOGS/security_audit_2099.log` 与 logs/ 是同一审计面，精确
    // 比对会被大写形态绕过（fs_watcher 忽略表 eq_ignore_ascii_case 同域
    // 先例）；Linux/macOS 大小写敏感是两个真不同目录，精确比对不误伤。
    if family.is_write_delete() && boundary.protect_log_tree {
        let resolved_guard = canonicalize_for_compare(&canonical);
        let ws_guard = canonicalize_for_compare(&boundary.root);
        if let Ok(rel) = resolved_guard.strip_prefix(&ws_guard) {
            let hits_log_tree = match rel.components().next() {
                Some(std::path::Component::Normal(name)) => {
                    #[cfg(windows)]
                    {
                        name.to_string_lossy().eq_ignore_ascii_case("logs")
                    }
                    #[cfg(not(windows))]
                    {
                        name == std::ffi::OsStr::new("logs")
                    }
                }
                _ => false,
            };
            if hits_log_tree {
                return Err(format!(
                    "access denied: '{}' is inside the protected log tree (workspace/logs). \
                     Audit and log files are system-owned and must not be created, modified \
                     or deleted via tools; write your artifacts elsewhere.",
                    path
                ));
            }
        }
    }
    // 目标系统文件保护（D-Q3 创建者二分）：写删族在 restrict 短路**之前**
    // 检查——restrict=false 也保护人配目标（goal.md 是评估的单一真相源，
    // 不随边界开关松动）；读族放行。守卫裁决单一真相源在 nemesis-goals
    // gate::fence_denial；界外路径不归本守卫管（由 restrict/editor_gate
    // 既有语义处理）。
    if family.is_write_delete()
        && let Some(guard) = &boundary.goals_guard
    {
        let resolved_guard = canonicalize_for_compare(&canonical);
        let ws_guard = canonicalize_for_compare(&boundary.root);
        if let Ok(rel) = resolved_guard.strip_prefix(&ws_guard)
            && let Some(reason) = guard(&rel.to_string_lossy().replace('\\', "/"))
        {
            return Err(reason);
        }
    }
    if !boundary.restrict {
        return Ok(canonical);
    }
    let resolved = canonicalize_for_compare(&canonical);
    let ws = canonicalize_for_compare(&boundary.root);
    if resolved.starts_with(&ws) {
        return Ok(canonical);
    }
    // 附加放行根：与主根同一套 canonicalize 防护（symlink escape /
    // 8.3 短名 / 最长存在祖先），任一根命中即放行。
    for extra in &boundary.extra_roots {
        let extra_canon = canonicalize_for_compare(extra);
        if resolved.starts_with(&extra_canon) {
            return Ok(canonical);
        }
    }
    // editor 放行开关闸：传**归一后**路径（逃逸/8.3 形态
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
/// 可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct WriteFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
    /// 知识库重定向（天工系统 v1）：同 [`ReadFileTool::knowledge`]。
    knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
}

impl WriteFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
            knowledge: None,
        }
    }

    /// 知识库重定向接线（registry 生产形态注入；重复调用覆盖）。
    pub fn with_knowledge(
        mut self,
        knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
    ) -> Self {
        self.knowledge = knowledge;
        self
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

        // 工作区边界（纵深防御，不单靠安全 8 层管线）+ 知识库重定向
        // （围栏前改写绝对路径 → 围栏 → 围栏后二次改写相对路径）。
        let path = resolve_tool_path_with_redirect(
            &path,
            self.boundary.as_deref(),
            FenceOpFamily::WriteDelete,
            self.knowledge.as_deref(),
        )?;

        // 覆盖保护——已存在文件先读旧内容（读取失败按新建处理，不挡
        // 写入），覆盖成功后附 unified diff。超大旧文件跳过 diff 读
        // （读侧内存闸同族，BUG-20261110-041：diff 只是展示面，不值得
        // 为它全量加载 GB 级旧文件——写入本身不受影响，与「读取失败按
        // 新建处理」语义一致）。
        let old_content = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.len() <= READ_FILE_MAX_BYTES => {
                tokio::fs::read_to_string(&path).await.ok()
            }
            _ => None,
        };

        // Create parent directories if needed.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        // 原子写（BUG-20261116-010）：tmp+fsync+rename 替代裸直写——写中段
        // 进程死亡不再撕裂源文件；已存在文件保留权限位与 symlink 链。
        nemesis_utils::write_file_atomic_async(&path, content.as_bytes(), 0o644)
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
        // 重定向感知（第五轮横扫 2026-11-27）：存在性判定与声明路径都用
        // 解析后路径（镜像命中时 kind 如实反映镜像侧现状）；围栏拒 → None
        // （无基线可预览，BUG-20261127-048）。
        let path =
            preview_resolve_path(&path, self.boundary.as_deref(), self.knowledge.as_deref())?;
        let kind = if path.exists() {
            FileChangeKind::Modify
        } else {
            FileChangeKind::Create
        };
        Some(FileChange {
            path: path.to_string_lossy().to_string(),
            kind,
        })
    }
}

/// A tool that lists the contents of a directory.
///
/// FT：可携带工作区边界（同 [`ReadFileTool`] 注记）。
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
            // 确定性输出序：listing 是给 LLM 的目录清单（模型
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

/// 单条编辑替换决策的**单一真相源**（纯函数）——
/// `edit_file`与 `multiedit`共用同一套匹配语义： 多命中
/// 歧义提示 → 全量替换 → 五级模糊级联。错误文案与历史 edit_file
/// 行为逐字一致（测试锁死）。
pub(crate) struct EditOutcome {
    pub new_content: String,
    /// old_text 在原文中的出现次数（`replace_all` 计数文案用；级联命中
    /// 路径恒 0——级联的命中语义由 level 名承载）。
    pub occurrences: usize,
    /// 级联命中级名（exact 命中为 None）。
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
        // exact 未命中且非 replace_all → 五级模糊替换级联。replace_all 不进
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

    // replace_all=true → 全部替换。注意 Rust 语义：`replacen(…, 0)` 是
    // 「最多换 0 处」（什么都不换）而非全量——全量是 `str::replace`
    // （实施计划该处笔误，已按 核实现修订）。
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
/// 可携带工作区边界（`with_boundary`，生产形态）；
/// `default()` 无界（基线注册 / 测试形态，行为同边界引入前）。
#[derive(Default)]
pub struct EditFileTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
    /// 知识库重定向（天工系统 v1）：同 [`ReadFileTool::knowledge`]；
    /// edit/append 两分支均接线。
    knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
}

impl EditFileTool {
    /// 带工作区边界的生产形态。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
            knowledge: None,
        }
    }

    /// 知识库重定向接线（registry 生产形态注入；重复调用覆盖）。
    pub fn with_knowledge(
        mut self,
        knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
    ) -> Self {
        self.knowledge = knowledge;
        self
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
        // 重定向感知（第五轮横扫 2026-11-27）：同 write_file——声明路径
        // 必须是 execute 的落盘路径（镜像）；围栏拒 → None（同
        // BUG-20261127-048）。
        let path =
            preview_resolve_path(&path, self.boundary.as_deref(), self.knowledge.as_deref())?;
        let action = val.get("action").and_then(|v| v.as_str()).unwrap_or("edit");
        let kind = if action == "append" {
            // append 分支允许目标不存在（create=true），按存在性如实申报。
            if path.exists() {
                FileChangeKind::Modify
            } else {
                FileChangeKind::Create
            }
        } else {
            // edit 分支要求文件已存在（execute errors otherwise）。
            FileChangeKind::Modify
        };
        Some(FileChange {
            path: path.to_string_lossy().to_string(),
            kind,
        })
    }
}

impl EditFileTool {
    /// action=edit（旧 EditFileTool::execute 原样搬运，行为零变更）。
    async fn execute_edit(&self, args: &str) -> Result<String, String> {
        let (path, old_text, new_text, replace_all) = extract_edit_args(args)?;

        // 工作区边界（纵深防御，不单靠安全 8 层管线）+ 知识库重定向
        // （同 write_file 双段链）。
        let path = resolve_tool_path_with_redirect(
            &path,
            self.boundary.as_deref(),
            FenceOpFamily::WriteDelete,
            self.knowledge.as_deref(),
        )?;
        let path = Path::new(&path);

        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }

        // 读侧内存闸（同 read_file，BUG-20261110-041）：编辑语义必须
        // 全量加载后做字符串替换，超帽文件直接拒绝并指引字节口径工具。
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("Failed to stat file: {}", e))?;
        if meta.len() > READ_FILE_MAX_BYTES {
            return Err(format!(
                "File too large for edit_file: {} bytes (cap 20MB). Editing requires loading the whole file — use exec with byte-level tools instead (sed/perl), e.g. `exec sed -i 's/old/new/g' \"{}\"`",
                meta.len(),
                path.display()
            ));
        }

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // // 匹配语义已抽出为 [`apply_edit_to_content`]（起与
        // multiedit 共用统一定义）；此处只做 IO 与回执组装。
        let outcome = apply_edit_to_content(
            &content,
            &old_text,
            &new_text,
            replace_all,
            &path.to_string_lossy(),
        )?;

        // 原子写（BUG-20261116-010）：同 write_file——编辑结果经 tmp+rename 落盘。
        nemesis_utils::write_file_atomic_async(path, outcome.new_content.as_bytes(), 0o644)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        // 成功回 unified diff（模型自检 + 前端渲染 +
        // 审计可读；超长回退统计摘要，全量语义由 loop 层 spill 兜底）。
        let diff = edit_hint::unified_diff(&path.to_string_lossy(), &content, &outcome.new_content);
        // replace_all 在头部报替换计数（count>=1 恒成立——0 已被 not-found
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

        // A5（2026-09-04）：工作区边界（纵深防御，不单靠安全 8 层管线）
        // + 知识库重定向（同 write_file 双段链）。
        let path = resolve_tool_path_with_redirect(
            &path,
            self.boundary.as_deref(),
            FenceOpFamily::WriteDelete,
            self.knowledge.as_deref(),
        )?;
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

/// multiedit 批量编辑——`edits: [{path, old_text,
/// new_text, replace_all?},...]`，**原子性**：全部编辑先在内存完成（同
/// 文件多条编辑按序累积生效），任一失败则整批不落盘并回灌首个失败详情 +
/// 逐条状态清单；全部成功才统一写盘（只写内容实际变化的文件），逐文件
/// 回 unified diff。匹配语义与 `edit_file` 共用 [`apply_edit_to_content`]
/// 统一定义。
///
/// 同款边界形态：`with_boundary` 生产 / `default()` 基线。
#[derive(Default)]
pub struct MultiEditTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
    /// 知识库重定向（天工系统 v1）：同 [`ReadFileTool::knowledge`]；
    /// 逐条 edit 接线（原子性不变——重定向发生在内存工作区收集前）。
    knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
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
            knowledge: None,
        }
    }

    /// 知识库重定向接线（registry 生产形态注入；重复调用覆盖）。
    pub fn with_knowledge(
        mut self,
        knowledge: Option<Arc<crate::knowledge::KnowledgeHandle>>,
    ) -> Self {
        self.knowledge = knowledge;
        self
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
            // 工作区边界（纵深防御，与 edit_file 同源校验）+ 知识库
            // 重定向（同 edit_file 双段链）。
            let vpath = match resolve_tool_path_with_redirect(
                &e.path,
                self.boundary.as_deref(),
                FenceOpFamily::WriteDelete,
                self.knowledge.as_deref(),
            ) {
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
                    // 读侧内存闸（同 read_file，BUG-20261110-041）：编辑
                    // 语义要求全量加载，超帽文件拒绝整批（原子性不变——
                    // 与 File not found 同款 FAILED 状态 + abort）。
                    let mlen = tokio::fs::metadata(&vpath).await.map(|m| m.len());
                    if let Ok(len) = mlen
                        && len > READ_FILE_MAX_BYTES
                    {
                        let m = format!(
                            "File too large for multiedit: {} bytes (cap 20MB) — use exec with byte-level tools (sed/perl)",
                            len
                        );
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
            // 原子写（BUG-20261116-010）：multiedit 统一写盘逐文件 tmp+rename。
            if let Err(err) =
                nemesis_utils::write_file_atomic_async(&f.path, f.current.as_bytes(), 0o644).await
            {
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
        // 重定向感知（第五轮横扫 2026-11-27）：与 execute 的内存工作区
        // 同键——去重按解析后路径（同一文件经不同形态路径两次声明只
        // 快照一次）。
        let mut seen = std::collections::HashSet::new();
        edits
            .into_iter()
            .filter_map(|e| {
                // 围栏拒 → None → 该 edit 丢弃不快照（BUG-20261127-048，
                // 同 write/edit：必死的写无基线）。
                let resolved = preview_resolve_path(
                    &e.path,
                    self.boundary.as_deref(),
                    self.knowledge.as_deref(),
                )?;
                seen.insert(resolved.clone()).then_some(FileChange {
                    path: resolved.to_string_lossy().to_string(),
                    kind: FileChangeKind::Modify,
                })
            })
            .collect()
    }
}

/// A tool that deletes a file from disk.
///
/// FT：可携带工作区边界（同 [`ReadFileTool`] 注记）——删除是
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

        // 韧性删除（同 DeleteDirTool；3789f815 只补了目录侧，文件侧漏网）：
        // Defender/索引器瞬时句柄是常态，单发失败 = 对模型假报错；
        // spawn_blocking 装同步重试内核（最坏 ~4.4s，不占 async worker）。
        // 重试变体把 NotFound 吞成成功——「删不存在文件要报错」的语义
        // 由前置存在性检查保留（nemesis-tools 同款注记）。
        let retry_path = path.clone();
        tokio::task::spawn_blocking(move || nemesis_utils::fsutil::remove_file_retry(&retry_path))
            .await
            .map_err(|e| format!("Failed to delete file: {e}"))?
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
/// FT：可携带工作区边界（同 [`ReadFileTool`] 注记）。
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
/// FT：可携带工作区边界（同 [`ReadFileTool`] 注记）——删除是
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
