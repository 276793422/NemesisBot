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
        // 归一化：统一 `/`、剥首尾斜杠；`..` 逐段拒绝（只读列表也不给
        // 越权探测面）。
        let norm = path.replace('\\', "/");
        let norm = norm.trim_matches('/');
        if norm.split('/').any(|seg| seg == "..") {
            return Err(format!("path 越出 workspace: {}", path));
        }
        let dir = if norm.is_empty() {
            root.clone()
        } else {
            root.join(norm)
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
        &["complete_path", "tree"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?.to_string();
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
            _ => Err(format!("unknown command: fs.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
