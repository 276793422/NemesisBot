//! Workspace 文件路径补全（I2，devtool-upgrade 阶段 3）+ 目录树（M4，阶段 5）。
//!
//! Dashboard 聊天输入框 `@` 引用的补全后端：`fs.complete_path {prefix}` 在
//! workspace 内做前缀匹配（路径前缀或 basename 前缀，大小写不敏感），
//! 返回 ≤[`MAX_COMPLETIONS`] 条工作区相对路径（`/` 分隔）+ truncated 标记。
//!
//! M4 目录树：`fs.tree {path?, depth?=3}` 列出 workspace 内目录树（条目
//! 上限 [`MAX_TREE_ENTRIES`]，忽略表同补全），`children: null` 的目录项
//! 表示「深度边界未加载」——前端点击目录再查子层（懒展开）。
//!
//! 噪声纪律与 fs_watcher 的 watch 过滤**共用同一张表**
//! （`nemesis_path::is_workspace_ignored`——I2 收敛：两消费方不再各写一份）：
//! `node_modules`/`target`/`logs` 等运行时目录与 `*.jsonl`/`*.db` 等
//! 运行时产物永远不会出现在补全/树列表里。
//!
//! 有界性：补全扫描 ≤[`MAX_SCAN_ENTRIES`] 项、树 ≤[`MAX_TREE_ENTRIES`]
//! 条目，均 1s 截止（先到为准），到限即 `truncated: true` 诚实返回——
//! 这是 UX 加速器，不是全量索引，宁可少不可卡。含空白的文件名跳过
//! （`@` token 语义是连续非空白，这类路径用户敲不出来，列表里出现只会
//! 制造选不中的死项）。

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 补全列表上限（计划 I2：≤20 条）。
pub const MAX_COMPLETIONS: usize = 20;
/// 扫描项上限（目录爆炸兜底；与 deadline 先到为准）。
pub const MAX_SCAN_ENTRIES: usize = 20_000;
/// 单次补全的时间预算。
pub const SCAN_DEADLINE: Duration = Duration::from_secs(1);

/// 目录树条目上限（计划 M4：500；所有层级合计）。
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

// ---------------------------------------------------------------------------
// fs.open：聊天文件链接打开（2026-10-19 用户报告问题 2——模型输出 markdown
// 里的文件路径在 UI 上不可点击，用户得手动翻目录找文件）
// ---------------------------------------------------------------------------

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
    let prefix = if cfg!(windows) {
        t[..r.len()].eq_ignore_ascii_case(&r)
    } else {
        t.starts_with(&r)
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

/// 解析点击路径 → (绝对路径, 是否目录)。
///
/// - **相对**（`reports/notes.md` / `notes.md`）：拼 session root（调用方
///   已按会话切换），再过包含闸（纵深：join 相对恒在 root 下，闸兜异常形态）。
/// - **绝对**（模型常输出绝对路径）：仅放行 [`allowed_open_roots`] 集合内；
///   集合外诚实报错（含探测面收敛：报错只说越界，不回显集合内容）。
/// - 目标不存在 → **CJK 引导语剥前缀回退**（见 [`cjk_boundary_candidates`]）
///   后仍不存在才诚实报错（前端 toast 提示，不留死链接静默）。
///
/// 纯决策不 spawn——测试面；实际打开在 [`FsHandler::handle_cmd`] 的 `open` 臂。
fn resolve_click_target(
    session_root: &str,
    main_workspace: &str,
    raw: &str,
) -> Result<(PathBuf, bool), String> {
    let norm = normalize_click_path(raw)?;
    match try_resolve_once(session_root, main_workspace, &norm) {
        Ok(hit) => Ok(hit),
        Err(e) if e.contains("文件不存在") => {
            // 存在性回退（2026-10-19 用户设计：候选不存在不应直接死，先剥
            // 粘连的中文引导语找真实文件——「见docs/总结.md」→ docs/总结.md）。
            // 每个候选先过同一套 normalize 闸（`..` 段拒绝必须重跑——raw
            // 「见../x」因首段是「见..」可通过整体检查，切出的候选「../x」
            // 若不过闸，path_contains 字符串前缀检查拦不住 OS 层 .. 解析，
            // 即成逃逸面），再走同一条 try_resolve_once。
            for cand in cjk_boundary_candidates(&norm) {
                if normalize_click_path(&cand).is_err() {
                    continue;
                }
                if let Ok(hit) = try_resolve_once(session_root, main_workspace, &cand) {
                    return Ok(hit);
                }
            }
            Err(e)
        }
        Err(e) => Err(e),
    }
}

/// 单次解析（归一后的路径）：containment + 存在性，供回退循环复用。
fn try_resolve_once(
    session_root: &str,
    main_workspace: &str,
    norm: &str,
) -> Result<(PathBuf, bool), String> {
    let p = Path::new(norm);
    let target = if p.is_absolute() || p.has_root() {
        if !allowed_open_roots(main_workspace)
            .iter()
            .any(|root| path_contains(root, p))
        {
            return Err(format!(
                "path 越出 workspace（仅允许工作区/项目目录内）: {norm}"
            ));
        }
        PathBuf::from(norm)
    } else {
        let joined = PathBuf::from(session_root).join(norm);
        if !path_contains(Path::new(session_root), &joined) {
            return Err(format!("path 越出 workspace: {norm}"));
        }
        joined
    };
    let meta =
        std::fs::metadata(&target).map_err(|_| format!("文件不存在: {}", target.display()))?;
    Ok((target, meta.is_dir()))
}

/// CJK 统一表意区判定（含扩展 A）——与前端 linkify SEG 字符类口径一致
/// （web/src/utils/markdown.ts）。
fn is_cjk_char(c: char) -> bool {
    ('\u{3400}'..='\u{4DBF}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// CJK 引导语剥前缀候选：模型输出「见docs/总结.md」「文件在C:/ws/a.md」时
/// 中英文直接粘连（无空格），整段路径不存在；在 CJK→ASCII 边界切开后取
/// 后缀作为候选路径重试。只在「后一字符是路径合法 ASCII 字符」处切，
/// 全角标点（：、。）后不切。候选按边界从左到右（长后缀优先——第一个
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

/// 默认应用打开文件（Windows=`explorer <file>`——资源管理器对文件路径
/// 走默认关联程序，绕开被禁的 `start` 系命令；macOS=`open`；
/// Linux=`xdg-open`）。spawn 不等待；CREATE_NO_WINDOW 同后台进程纪律。
fn open_file_with_default(path: &Path) -> Result<(), String> {
    let spawn = || -> std::io::Result<std::process::Child> {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            std::process::Command::new("explorer")
                .arg(path)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(path).spawn()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open").arg(path).spawn()
        }
    };
    spawn()
        .map(|_| ())
        .map_err(|e| format!("打开文件失败: {e}"))
}

/// 文件管理器定位文件（不打开、不高亮执行——非白名单扩展名的降级动作）。
/// Windows=`explorer /select,<path>`（单参数形态——explorer 自行解析命令
/// 行，拆参会在含空格路径上断裂）；macOS=`open -R`；Linux 无标准 reveal，
/// 打开父目录（诚实降级）。
fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    let spawn = || -> std::io::Result<std::process::Child> {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let parent = path.parent().unwrap_or(path);
            std::process::Command::new("xdg-open").arg(parent).spawn()
        }
    };
    spawn()
        .map(|_| ())
        .map_err(|e| format!("定位文件失败: {e}"))
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

    /// 列出 workspace 内目录树（M4）。`path` 为工作区相对路径（空=根），
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
        // F7（2026-10-14）根修：`PathBuf::join` 遇绝对路径会**替换整个
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
        &["complete_path", "tree", "open"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // L6++ G4（2026-09-08）：可选 session_id 项目判别——项目会话把遍历
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
            // M4：目录树（懒展开 = 前端点目录再查子层，depth=1 即一层）。
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
            // + 全部项目；session root 已含于其中）。
            "open" => {
                let raw = data
                    .as_ref()
                    .and_then(|d| d.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let main_workspace = require_workspace(ctx)?.to_string();
                let (target, is_dir) = resolve_click_target(&workspace, &main_workspace, &raw)?;
                let out = if is_dir {
                    crate::handlers::projects::open_in_file_manager(&target)?;
                    serde_json::json!({
                        "kind": "dir", "action": "open", "path": target.display().to_string(),
                    })
                } else if is_openable_extension(&target) {
                    open_file_with_default(&target)?;
                    serde_json::json!({
                        "kind": "file", "action": "open", "path": target.display().to_string(),
                    })
                } else {
                    reveal_in_file_manager(&target)?;
                    serde_json::json!({
                        "kind": "file", "action": "reveal", "path": target.display().to_string(),
                    })
                };
                Ok(Some(out))
            }
            _ => Err(format!("unknown command: fs.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
