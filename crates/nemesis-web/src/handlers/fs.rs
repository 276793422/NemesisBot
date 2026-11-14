//! Workspace 文件路径补全（devtool-upgrade）+ 目录树。
//!
//! Dashboard 聊天输入框 `@` 引用的补全后端：`fs.complete_path {prefix}` 在
//! workspace 内做前缀匹配（路径前缀或 basename 前缀，大小写不敏感），
//! 返回 ≤[`MAX_COMPLETIONS`] 条工作区相对路径（`/` 分隔）+ truncated 标记。
//!
//! 目录树：`fs.tree {path?, depth?=3}` 列出 workspace 内目录树（条目
//! 上限 [`MAX_TREE_ENTRIES`]，忽略表同补全），`children: null` 的目录项
//! 表示「深度边界未加载」——前端点击目录再查子层（懒展开）。
//!
//! 噪声纪律与 fs_watcher 的 watch 过滤**共用同一张表**
//! （`nemesis_path::is_workspace_ignored`—— 收敛：两消费方不再各写一份）：
//! `node_modules`/`target`/`logs` 等运行时目录与 `*.jsonl`/`*.db` 等
//! 运行时产物永远不会出现在补全/树列表里。
//!
//! 有界性：补全扫描 ≤[`MAX_SCAN_ENTRIES`] 项、树 ≤[`MAX_TREE_ENTRIES`]
//! 条目，均 1s 截止（先到为准），到限即 `truncated: true` 诚实返回——
//! 这是 UX 加速器，不是全量索引，宁可少不可卡。含空白的文件名跳过
//! （`@` token 语义是连续非空白，这类路径用户敲不出来，列表里出现只会
//! 制造选不中的死项）。

use crate::handlers::require_workspace;
use crate::handlers::shell_open;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 补全列表上限（计划：≤20 条）。
pub const MAX_COMPLETIONS: usize = 20;
/// 扫描项上限（目录爆炸兜底；与 deadline 先到为准）。
pub const MAX_SCAN_ENTRIES: usize = 20_000;
/// 单次补全的时间预算。
pub const SCAN_DEADLINE: Duration = Duration::from_secs(1);

/// 目录树条目上限（计划：500；所有层级合计）。
pub const MAX_TREE_ENTRIES: usize = 500;
/// 目录树深度钳制上限（请求 depth 再大也不超过；默认 3）。
pub const MAX_TREE_DEPTH: usize = 8;

pub struct FsHandler;

/// 目录树扫描共享预算（条目数 + deadline，跨递归层级累计）。
struct TreeScan {
    entries: usize,
    truncated: bool,
    deadline: Instant,
}

impl Default for FsHandler {
    fn default() -> Self {
        Self::new()
    }
}

// fs.open：聊天文件链接打开（用户报告问题 2——模型输出 markdown
// 里的文件路径在 UI 上不可点击，用户得手动翻目录找文件）

/// 允许「默认应用直接打开」的扩展名白名单（小写带点）。
///
/// 安全动机：防「模型写 evil.bat → 聊天链接 → 用户点击 → 执行」链。
/// 白名单只收被动打开类型（文本文档/图片/PDF）；可执行/脚本/宏形态
/// （bat/cmd/exe/ps1/vbs/js/hta/lnk/...）一律降级为「文件管理器定位」
/// （reveal）——用户看得见、找得到，但不会因一次点击而执行。
const OPENABLE_EXTENSIONS: &[&str] = &[
    ".md", ".txt", ".log", ".json", ".csv", ".pdf", //
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp",
];

/// 扩展名白名单判定（大小写不敏感——`REPORT.MD` 同 `.md` 放行）。
fn is_openable_extension(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    OPENABLE_EXTENSIONS.contains(&ext.as_str())
}

/// 路径包含判定：分隔符统一 `/` 后做前缀比较。Windows 大小写不敏感
/// （聊天文本里的盘符/目录大小写不可控）；前缀必须命中边界（相等或
/// 后随 `/`），防 `C:\foo2` 命中 `C:\foo`。纯词法（不 canonicalize）——
/// 这是点击链接的 UX 便利闸，不是安全 8 层的替代；主防线是「只放行
/// workspace/已注册项目目录」+ 扩展名白名单。
fn path_contains(root: &Path, target: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string()
    };
    let (r, t) = (norm(root), norm(target));
    if r.is_empty() {
        return true; // 根盘形态（实践中不出现；防御性放行避免误拒）
    }
    let eq = if cfg!(windows) {
        r.eq_ignore_ascii_case(&t)
    } else {
        r == t
    };
    if eq {
        return true;
    }
    if t.len() <= r.len() {
        return false;
    }
    // `t[..r.len()]` 裸切片在 r.len() 落于 t 的多字节
    // 字符中间时 panic（`fs.open` 带模型输出的 CJK 绝对路径即可触发）。
    // `str::get` 边界安全：非边界 = None = 前缀必不等，诚实 false。
    let prefix = match t.get(..r.len()) {
        Some(p) if cfg!(windows) => p.eq_ignore_ascii_case(&r),
        Some(p) => p == r,
        None => false,
    };
    prefix && t.as_bytes()[r.len()] == b'/'
}

/// `..` 段拒绝 + 分隔符归一（点击链接语义不需要逃逸形态；合法嵌套路径
/// 用绝对或 workspace 相对形态表达）。
fn normalize_click_path(raw: &str) -> Result<String, String> {
    let norm = raw.trim().replace('\\', "/");
    if norm.is_empty() {
        return Err("path 缺失".to_string());
    }
    if norm.split('/').any(|seg| seg == "..") {
        return Err(format!("path 越出 workspace: {raw}"));
    }
    Ok(norm)
}

/// 允许打开的根集合：主 workspace + 全部已注册项目目录（绝对路径只放行
/// 这两类——防诱导打开任意系统文件；相对路径按 session root 解析后再过
/// 同一包含闸）。
fn allowed_open_roots(main_workspace: &str) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(main_workspace)];
    if let Some(bridge) = crate::handlers::projects::projects_bridge() {
        for p in bridge.list() {
            roots.push(PathBuf::from(p.path));
        }
    }
    roots
}

/// 解析产物：绝对路径 + 类型 + 存在性。`exists=false` = 文件未命中、
/// 回落到最近存在祖先目录的产物（响应面诚实注记「文件未找到」）。
#[derive(Debug, PartialEq)]
pub(crate) struct ResolvedTarget {
    pub path: PathBuf,
    pub is_dir: bool,
    pub exists: bool,
}

/// 锚链单次解析的结果分类：命中 / 未命中（带祖先候选——CJK 剥前缀回退
/// 可继续，全部候选未命中才落祖先回退）/ 越界（短路——回退候选同样
/// 不放松任何闸）。
enum ChainOutcome {
    /// 存在目标命中（exists=true）。
    Hit(ResolvedTarget),
    /// 全链无存在目标：`ancestor` = 最近存在祖先目录（若有）；`err` =
    /// 「文件不存在」错误面。**祖先回退不在这里落地**——CJK 剥前缀
    /// 优先（候选命中真实文件远优于开一个祖先目录），祖先只在全部
    /// 候选未命中后由 [`resolve_click_target`] 兜底消费。
    Miss(Option<ResolvedTarget>, String),
    /// 越界（containment 拒绝——短路，不参与回退）。
    OutOfBounds(String),
}

/// 解析点击路径 → [`ResolvedTarget`]。
///
/// - **相对**（`reports/notes.md` / `notes.md`）：锚链逐根试存在性——
///   session root（项目会话语义不变）在先，未命中再试其余允许根（主
///   workspace + 全部注册项目），首个存在者胜（确定性优先）。
/// - **绝对**（模型常输出绝对路径）：仅放行 [`allowed_open_roots`] 集合内；
///   集合外诚实报错（含探测面收敛：报错只说越界，不回显集合内容）。
/// - 全链未命中 → **CJK 引导语剥前缀回退**（见 [`cjk_boundary_candidates`]，
///   只收存在命中）→ 仍无命中 → **最近存在祖先目录兜底**（文件未找到时
///   打开其所在的真实目录，`exists=false` 诚实注记；祖先逐级过
///   containment 绝不逃逸）；连祖先都不存在才诚实报错（前端 toast 提示，
///   不留死链接静默）。
///
/// 纯决策不 spawn——测试面；实际打开在 [`FsHandler::handle_cmd`] 的
/// `open` / `resolve` 臂。
fn resolve_click_target(
    session_root: &str,
    main_workspace: &str,
    raw: &str,
) -> Result<ResolvedTarget, String> {
    let norm = normalize_click_path(raw)?;
    match try_resolve_chain(session_root, main_workspace, &norm) {
        ChainOutcome::Hit(t) => Ok(t),
        ChainOutcome::OutOfBounds(e) => Err(e),
        ChainOutcome::Miss(ancestor, e) => {
            // 存在性回退（用户设计：候选不存在不应直接死，先剥
            // 粘连的中文引导语找真实文件——「见docs/总结.md」→ docs/总结.md）。
            // 每个候选先过同一套 normalize 闸（`..` 段拒绝必须重跑——raw
            // 「见../x」因首段是「见..」可通过整体检查，切出的候选「../x」
            // 若不过闸，path_contains 字符串前缀检查拦不住 OS 层.. 解析，
            // 即成逃逸面），再走同一条锚链。**只收 Hit**——候选再 miss
            // 也不抢原始路径的祖先回退权（候选的祖先与原始路径的祖先
            // 高度重叠，抢了会让「见docs/不存在.md」开错目录）。
            for cand in cjk_boundary_candidates(&norm) {
                if normalize_click_path(&cand).is_err() {
                    continue;
                }
                if let ChainOutcome::Hit(t) = try_resolve_chain(session_root, main_workspace, &cand)
                {
                    return Ok(t);
                }
            }
            // 祖先回退（最后）：全链无存在目标 → 最近存在祖先目录
            // （文件所在的真实目录，exists=false）；剥到根仍不存在
            // （注册项目目录被删等）→ 诚实报错。
            match ancestor {
                Some(t) => Ok(t),
                None => Err(e),
            }
        }
    }
}

/// 锚链解析（归一后的路径）：绝对臂 / 相对臂共用存在性判定 + 祖先回退 +
/// 分隔符统一。
fn try_resolve_chain(session_root: &str, main_workspace: &str, norm: &str) -> ChainOutcome {
    let p = Path::new(norm);
    let roots = allowed_open_roots(main_workspace);
    if p.is_absolute() || p.has_root() {
        if !roots.iter().any(|root| path_contains(root, p)) {
            return ChainOutcome::OutOfBounds(format!(
                "path 越出 workspace（仅允许工作区/项目目录内）: {norm}"
            ));
        }
        let target = unify_separators(PathBuf::from(norm));
        return match std::fs::metadata(&target) {
            Ok(meta) => ChainOutcome::Hit(ResolvedTarget {
                path: target,
                is_dir: meta.is_dir(),
                exists: true,
            }),
            Err(_) => match nearest_existing_ancestor(&target, &roots) {
                Some(dir) => ChainOutcome::Miss(
                    Some(ResolvedTarget {
                        path: unify_separators(dir),
                        is_dir: true,
                        exists: false,
                    }),
                    format!("文件不存在: {norm}"),
                ),
                None => ChainOutcome::Miss(None, format!("文件不存在: {norm}")),
            },
        };
    }
    // 相对：锚链 = session root 在先 + 其余允许根（去重——主会话形态
    // session root == 主 workspace，锚链退化为单根，行为与旧版一致）。
    let mut chain = vec![PathBuf::from(session_root)];
    for r in &roots {
        if !chain.iter().any(|c| same_path(c, r)) {
            chain.push(r.clone());
        }
    }
    let mut ancestor: Option<ResolvedTarget> = None;
    for root in &chain {
        let joined = unify_separators(root.join(norm));
        if !path_contains(root, &joined) {
            continue; // 纵深闸（合法 join 恒过）
        }
        if let Ok(meta) = std::fs::metadata(&joined) {
            return ChainOutcome::Hit(ResolvedTarget {
                path: joined,
                is_dir: meta.is_dir(),
                exists: true,
            });
        }
        if ancestor.is_none()
            && let Some(a) = nearest_existing_ancestor(&joined, std::slice::from_ref(root))
        {
            ancestor = Some(ResolvedTarget {
                path: unify_separators(a),
                is_dir: true,
                exists: false,
            });
        }
    }
    ChainOutcome::Miss(ancestor, format!("文件不存在: {norm}"))
}

/// 最近存在祖先目录（文件不存在回退）：从 target 向根方向逐级剥段，
/// 每级过 containment（锚 root 或绝对臂的全集），首个存在的目录即胜；
/// 剥到根本身仍不存在（项目目录被删等）= None（诚实报错）。
fn nearest_existing_ancestor(target: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let mut cur = target.parent();
    while let Some(dir) = cur {
        if !roots.iter().any(|r| path_contains(r, dir)) {
            return None;
        }
        if dir.is_dir() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// 路径等值（分隔符归一 + Windows 大小写不敏感）——锚链去重用。
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string()
    };
    let (a, b) = (norm(a), norm(b));
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

/// 分隔符统一：解析产物在 Windows 上把 join 保留的 `/` 归一成 `\`
/// （混合分隔符让 explorer /select 选中行为不稳，也让响应 path 形态
/// 不一致）。非 Windows 原样。
fn unify_separators(p: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(p.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        p
    }
}

/// CJK 统一表意区判定（含扩展 A）——与前端 linkify SEG 字符类口径一致
/// （web/src/utils/markdown.ts）。
fn is_cjk_char(c: char) -> bool {
    ('\u{3400}'..='\u{4DBF}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// CJK 引导语剥前缀候选：模型输出「见docs/总结.md」「文件在C:/ws/a.md」时
/// 中英文直接粘连（无空格），整段路径不存在；在 CJK→ASCII 边界切开后取
/// 后缀作为候选路径重试。只在「后一字符是路径合法 ASCII 字符」处切，
/// 全角标点（。）后不切。候选按边界从左到右（长后缀优先——第一个
/// 边界剥掉的通常就是引导语本身）。
fn cjk_boundary_candidates(norm: &str) -> Vec<String> {
    let chars: Vec<char> = norm.chars().collect();
    let mut out = Vec::new();
    for i in 1..chars.len() {
        // norm 已把 `\` 归一成 `/`，此处无需处理反斜杠。
        if is_cjk_char(chars[i - 1])
            && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '/' | '.' | '_' | '-' | '@'))
        {
            let cand: String = chars[i..].iter().collect();
            // 「总结.md」的「结→.」边界会切出 ".md" 这类扩展名碎片（垃圾
            // 候选）；"./x" 是合法相对形态不受影响。
            if cand.starts_with('.') && !cand.starts_with("./") {
                continue;
            }
            if !out.contains(&cand) {
                out.push(cand);
            }
        }
    }
    out
}

/// open/reveal 两臂共用的 path 参数提取：缺 data / 缺键 / 非字符串 →
/// 空串（归一化统一报「path 缺失」——错误面单一真相源在
/// [`resolve_click_target`]）。
fn path_arg(data: Option<&serde_json::Value>) -> &str {
    data.and_then(|d| d.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

impl FsHandler {
    pub fn new() -> Self {
        Self
    }

    /// 在 workspace 内做 `@` 前缀补全（同步有界遍历；deadline 兜底）。
    fn complete_path_in(&self, workspace: &str, prefix: &str) -> Result<serde_json::Value, String> {
        let root = PathBuf::from(workspace);
        if !root.is_dir() {
            return Err(format!("workspace not found: {}", workspace));
        }
        let deadline = Instant::now() + SCAN_DEADLINE;
        let prefix_lc = prefix.replace('\\', "/").to_lowercase();
        // basename 前缀段：`src/ma` 的 `ma`；裸 `main.rs` 的整个前缀。
        let name_part = prefix_lc.rsplit('/').next().unwrap_or("");

        let mut queue: Vec<PathBuf> = vec![root.clone()];
        let mut matches: Vec<String> = Vec::new();
        let mut truncated = false;
        let mut scanned = 0usize;

        'outer: while let Some(dir) = queue.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                scanned += 1;
                if scanned > MAX_SCAN_ENTRIES || Instant::now() > deadline {
                    truncated = true;
                    break 'outer;
                }
                let Ok(name) = entry.file_name().into_string() else {
                    continue; // 非 UTF-8 名敲不出来
                };
                if name.contains(char::is_whitespace) {
                    continue;
                }
                let path = entry.path();
                let Ok(rel) = path.strip_prefix(&root) else {
                    continue;
                };
                if nemesis_path::is_workspace_ignored(rel) {
                    continue; // 运行时目录/产物与 watch 过滤同表
                }
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                let is_dir = path.is_dir();
                let rel_lc = rel_str.to_lowercase();
                let path_hit = rel_lc.starts_with(&prefix_lc);
                let name_hit = !name_part.is_empty() && name.to_lowercase().starts_with(name_part);
                if path_hit || name_hit {
                    // 目录补全带尾斜杠（`@src/` 直接续打下一层）。
                    let display = if is_dir {
                        format!("{}/", rel_str)
                    } else {
                        rel_str
                    };
                    matches.push(display);
                    if matches.len() > MAX_COMPLETIONS {
                        truncated = true;
                        break 'outer;
                    }
                }
                if is_dir {
                    queue.push(path);
                }
            }
        }

        // 截断前可能超收 1 条；排序保确定性（read_dir 顺序不保证）。
        matches.sort_by_key(|p| p.to_lowercase());
        matches.truncate(MAX_COMPLETIONS);
        Ok(serde_json::json!({ "paths": matches, "truncated": truncated }))
    }

    /// 列出 workspace 内目录树。`path` 为工作区相对路径（空=根），
    /// 禁绝对路径与 `..` 逃逸；`depth` 钳到 [`MAX_TREE_DEPTH`]；条目总数
    /// 钳到 [`MAX_TREE_ENTRIES`]（含子层），到限 `truncated: true`。
    fn tree_in(
        &self,
        workspace: &str,
        path: &str,
        depth: usize,
    ) -> Result<serde_json::Value, String> {
        let root = PathBuf::from(workspace);
        if !root.is_dir() {
            return Err(format!("workspace not found: {}", workspace));
        }
        // 归一化：统一 `/`、剥首尾斜杠；`..` 逐段拒绝；绝对/根路径形态
        // 显式拒绝（只读列表也不给越权探测面）。
        let norm = path.replace('\\', "/");
        let norm = norm.trim_matches('/');
        if norm.split('/').any(|seg| seg == "..") {
            return Err(format!("path 越出 workspace: {}", path));
        }
        // 根修：`PathBuf::join` 遇绝对路径会**替换整个
        // root**——Windows 盘符形态（`C:/Windows`）归一化后仍是绝对路径，
        // 此前只拒 `..` 时漏挡 = 任意目录列举。`has_root` 覆盖盘符与根
        // 形态（Unix 绝对路径已被 trim 剥成相对，此闸在 Windows 生效）。
        let norm_path = Path::new(&norm);
        if norm_path.is_absolute() || norm_path.has_root() {
            return Err(format!("path 越出 workspace: {}", path));
        }
        let dir = if norm.is_empty() {
            root.clone()
        } else {
            let joined = root.join(norm);
            // 纵深闸：join 后仍在 root 下才放行（防前两道闸漏网的替换
            // 形态；相对路径 join 恒在 root 下，合法输入不受影响）。
            if !joined.starts_with(&root) {
                return Err(format!("path 越出 workspace: {}", path));
            }
            joined
        };
        if !dir.is_dir() {
            return Err(format!("目录不存在: {}", norm));
        }
        let depth = depth.clamp(1, MAX_TREE_DEPTH);
        let mut scan = TreeScan {
            entries: 0,
            truncated: false,
            deadline: Instant::now() + SCAN_DEADLINE,
        };
        let entries = Self::list_level(&root, norm, depth, &mut scan);
        Ok(serde_json::json!({
            "path": norm,
            "entries": entries,
            "truncated": scan.truncated,
        }))
    }

    /// 单层列出（递归到 depth 边界）。目录在前、文件在后，各自按名排序
    /// （read_dir 顺序不保证）。子目录在深度边界处 `children: null`
    /// （= 未加载，前端懒展开再查）；空目录是 `children: []`。
    fn list_level(
        root: &Path,
        rel: &str,
        depth: usize,
        scan: &mut TreeScan,
    ) -> Vec<serde_json::Value> {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Vec::new(); // 单目录读失败不拖垮整棵树
        };
        let mut dirs: Vec<(String, String)> = Vec::new();
        let mut files: Vec<(String, String)> = Vec::new();
        for entry in read.flatten() {
            scan.entries += 1;
            if scan.entries > MAX_TREE_ENTRIES || Instant::now() > scan.deadline {
                scan.truncated = true;
                break;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue; // 非 UTF-8 名敲不出来
            };
            if name.contains(char::is_whitespace) {
                continue; // @ 引用语义敲不出来，与补全同纪律
            }
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", rel, name)
            };
            if nemesis_path::is_workspace_ignored(Path::new(&child_rel)) {
                continue; // 运行时目录/产物与 watch 过滤同表
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs.push((name, child_rel));
            } else {
                files.push((name, child_rel));
            }
        }
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        files.sort_by_key(|(name, _)| name.to_lowercase());
        let mut out = Vec::with_capacity(dirs.len() + files.len());
        for (name, child_rel) in dirs {
            let children = if depth > 1 {
                serde_json::Value::Array(Self::list_level(root, &child_rel, depth - 1, scan))
            } else {
                serde_json::Value::Null
            };
            out.push(serde_json::json!({
                "name": name, "path": child_rel, "type": "dir", "children": children,
            }));
        }
        for (name, child_rel) in files {
            out.push(serde_json::json!({ "name": name, "path": child_rel, "type": "file" }));
        }
        out
    }
}

#[async_trait::async_trait]
impl ModuleHandler for FsHandler {
    fn module_name(&self) -> &str {
        "fs"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["complete_path", "tree", "open", "reveal", "resolve"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 可选 session_id 项目判别——项目会话把遍历
        // 根切到项目目录（仍只读有界遍历）；无归属/未装配 = 主 workspace。
        let mut workspace = require_workspace(ctx)?.to_string();
        if let Some(sid) = data
            .as_ref()
            .and_then(|d| d.get("session_id"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let session_key = format!(
                "agent:main:session:{}",
                nemesis_agent::session::SessionStore::sanitize_session_id(sid)
            );
            if let Some(root) = crate::handlers::projects::project_root_for_session(&session_key) {
                workspace = root;
            }
        }
        match cmd {
            // 路由按 module_name 分发后 cmd 是裸名（模块前缀已被剥掉）。
            "complete_path" => {
                let prefix = data
                    .as_ref()
                    .and_then(|d| d.get("prefix"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(Some(self.complete_path_in(&workspace, &prefix)?))
            }
            // 目录树（懒展开 = 前端点目录再查子层，depth=1 即一层）。
            "tree" => {
                let path = data
                    .as_ref()
                    .and_then(|d| d.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let depth = data
                    .as_ref()
                    .and_then(|d| d.get("depth"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3) as usize;
                Ok(Some(self.tree_in(&workspace, &path, depth)?))
            }
            // 文件链接打开（问题 2）：目录→文件管理器；白名单扩展名→默认
            // 应用；其余→文件管理器定位（不点击执行）。main_workspace 用
            // 未切换的主 workspace（绝对路径 containment 集合 = 主 workspace
            // + 全部项目；session root 已含于其中）。响应带 exists 注记
            // （祖先目录回退产物 exists=false——前端 toast 语义分级用）。
            "open" => {
                let raw = path_arg(data.as_ref()).to_string();
                let main_workspace = require_workspace(ctx)?.to_string();
                let t = resolve_click_target(&workspace, &main_workspace, &raw)?;
                let out = if t.is_dir {
                    shell_open::open_dir(&t.path)?;
                    serde_json::json!({
                        "kind": "dir", "action": "open", "path": t.path.display().to_string(),
                        "exists": t.exists,
                    })
                } else if is_openable_extension(&t.path) {
                    shell_open::open_file_default(&t.path)?;
                    serde_json::json!({
                        "kind": "file", "action": "open", "path": t.path.display().to_string(),
                        "exists": t.exists,
                    })
                } else {
                    shell_open::reveal_file(&t.path)?;
                    serde_json::json!({
                        "kind": "file", "action": "reveal", "path": t.path.display().to_string(),
                        "exists": t.exists,
                    })
                };
                Ok(Some(out))
            }
            // 右键菜单「打开所在目录」（reveal 语义，2026-11-13）：与
            // `open` 同一套解析与 containment，只换最终动作——目录链接
            // 直接打开该目录（用户拍板：目录特判不做 reveal）；文件链接
            // 打开所在目录并选中该文件。文件未命中时打开最近存在祖先
            // 目录（解析层回退产物，exists=false 诚实注记）。解析/越界/
            // 缺参错误面与 open 完全同源。
            "reveal" => {
                let raw = path_arg(data.as_ref()).to_string();
                let main_workspace = require_workspace(ctx)?.to_string();
                let t = resolve_click_target(&workspace, &main_workspace, &raw)?;
                let out = if t.is_dir {
                    shell_open::open_dir(&t.path)?;
                    serde_json::json!({
                        "kind": "dir", "action": "open", "path": t.path.display().to_string(),
                        "exists": t.exists,
                    })
                } else {
                    shell_open::reveal_file(&t.path)?;
                    serde_json::json!({
                        "kind": "file", "action": "reveal", "path": t.path.display().to_string(),
                        "exists": t.exists,
                    })
                };
                Ok(Some(out))
            }
            // 纯决策面（不 spawn——reveal 会弹窗，复制路径等只需要
            // 「绝对路径 + 存在性」）：与 open/reveal 共用同一解析阶梯。
            // exists=false = 文件未命中、path 是最近存在祖先目录（前端
            // 复制路径后 toast 注记「文件尚未定位到」）。
            "resolve" => {
                let raw = path_arg(data.as_ref()).to_string();
                let main_workspace = require_workspace(ctx)?.to_string();
                let t = resolve_click_target(&workspace, &main_workspace, &raw)?;
                Ok(Some(serde_json::json!({
                    "path": t.path.display().to_string(),
                    "kind": if t.is_dir { "dir" } else { "file" },
                    "exists": t.exists,
                })))
            }
            _ => Err(format!("unknown command: fs.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
