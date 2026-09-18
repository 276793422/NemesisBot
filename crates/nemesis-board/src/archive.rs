//! 项目档案目录（看板项目档案 goal P2/B1-B6）。
//!
//! 每个项目可绑定一个**档案目录**（既是工作集也是档案，P4 起 git 化）。
//! 本模块是目录的单一裁决点：
//! - [`sanitize_project_name`] / [`default_directory`]：项目名 → 文件名安全
//!   化 + 自动分配（纯函数，单测钉死）；
//! - [`resolve_project_directory`]：用户指定/自动分配的统一解析（绝对路径
//!   校验、与主 workspace/其他项目目录重叠拒绝、8.3 短名拒绝、创建时
//!   mkdir）——**绑定不可变**，解析只发生在 project.create 一次；
//! - [`ensure_scaffold`]：档案四件套初始化（project.json / docs/ /
//!   artifacts/ / records/ + timeline.jsonl + .gitignore）。
//!
//! 真相源原则（goal B4）：board.db 恒为权威；目录 = 投影 + worker 原件。
//! 投影部分（project.json/timeline/docs/records）可重建，`.gitignore` 排除
//! （B6）；`artifacts/` 是依赖任务产物流转通道，进跟踪树。

use std::path::{Path, PathBuf};

/// 档案根在工作区内的默认父目录（自动分配时）。
pub const BOARD_PROJECTS_DIR: &str = "board-projects";

/// 8.3 短名组件形态：`AAAAAA~1`（基础名 ≥1 字符 + `~` + 数字；与
/// anchor.rs 同款教训——canonicalize 前先词法拦截，杜绝短名绕过比较）。
static SHORT_NAME: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

fn short_name_re() -> &'static regex::Regex {
    SHORT_NAME.get_or_init(|| regex::Regex::new(r".+~\d+$").expect("short-name regex"))
}

/// 项目名 → 文件名安全段（B2 纯函数）：
/// - 空白折叠为 `_`；Windows 非法字符 `< > : " / \ | ? *` 与控制字符 → `_`；
/// - 连续 `_` 塌缩；首尾 `._ ` 修剪（Windows 尾点/尾空格陷阱）；
/// - 截断到 40 字符（多字节安全向下取整，防根级目录名超长）；
/// - 全空 → `project`。
pub fn sanitize_project_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for ch in name.trim().chars() {
        let c = if ch.is_whitespace()
            || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            || ch.is_control()
        {
            '_'
        } else {
            ch
        };
        if c == '_' && prev_underscore {
            continue;
        }
        prev_underscore = c == '_';
        out.push(c);
    }
    let trimmed = out
        .trim_matches(|c| matches!(c, '.' | '_' | ' '))
        .to_string();
    if trimmed.is_empty() {
        return "project".to_string();
    }
    // 多字节安全截断（floor char boundary）。
    let mut end = trimmed.len().min(40);
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end]
        .trim_matches(|c| matches!(c, '.' | '_' | ' '))
        .to_string()
}

/// 自动分配目录：`<workspace>/board-projects/<安全化项目名>/`，重名追加
/// 序号（`-2`、`-3`……上限 100，超出诚实报错——不静默换名）。`existing`
/// 是已绑定项目目录的 canonical 形态集合（去重比对在 canonical 域进行，
/// 词法不同的同一目录也会撞出序号）。
pub fn default_directory(
    workspace: &Path,
    project_name: &str,
    existing: &[PathBuf],
) -> Result<PathBuf, String> {
    let base = workspace
        .join(BOARD_PROJECTS_DIR)
        .join(sanitize_project_name(project_name));
    let canonical_base = nemesis_path::canonicalize_for_compare(&base);
    if !existing.iter().any(|e| e == &canonical_base) && !base.exists() {
        return Ok(base);
    }
    for n in 2..=100 {
        let cand = base.parent().unwrap_or(&base).join(format!(
            "{}-{n}",
            base.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
        ));
        let canonical = nemesis_path::canonicalize_for_compare(&cand);
        if !existing.iter().any(|e| e == &canonical) && !cand.exists() {
            return Ok(cand);
        }
    }
    Err(format!(
        "自动分配目录失败：{} 下 100 个候选名全部被占用",
        base.parent().unwrap_or(Path::new(".")).display()
    ))
}

/// 目录解析统一入口（B2；**绑定不可变**——只在 project.create 调用一次）。
///
/// - `requested = Some(abs)`：绝对路径校验 + 8.3 短名拒绝（先展开盘上
///   存在的祖先链，展开后仍带 `~N` 组件才拒）+ 与主 workspace/其他项目
///   目录重叠拒绝（双向 contains，canonical 域比较）；
/// - `requested = None`：[`default_directory`] 自动分配。
///
/// 两条路都**创建时 mkdir**（存在性不要求；已存在同名目录直接使用——
/// 用户选已有目录作初始基线是 B5 的合法形态）。返回 canonical 显示路径。
pub fn resolve_project_directory(
    requested: Option<&str>,
    workspace: &Path,
    project_name: &str,
    existing: &[PathBuf],
) -> Result<PathBuf, String> {
    let path = match requested.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let p = PathBuf::from(raw);
            if !p.is_absolute() {
                return Err(format!(
                    "项目目录必须是绝对路径：{raw}（不填则自动分配 {BOARD_PROJECTS_DIR}/ 下目录）"
                ));
            }
            // 8.3 短名拦截 = 先展开、后词法：canonicalize_for_compare 把盘上
            // **存在**的祖先链全部展开（`C:\Users\RUNNER~1\...` → 真名），
            // 展开后仍带 `~N` 尾的组件才拒绝。纯词法先行会误杀合法输入——
            // GitHub Actions Windows runner 的 %TEMP% 用户组件本身就是
            // RUNNER~1 短名形态（2026-09-18 CI integration 实录：project.create
            // 对 %TEMP% 子目录全灭）。不存在的短名（SOMEDI~1 等探测形态）
            // 展不开、原样保留 → 词法兜底照旧拒绝。
            let expanded = nemesis_path::canonicalize_for_compare(&p);
            if expanded.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .is_some_and(|s| short_name_re().is_match(s))
            }) {
                return Err(format!("项目目录拒绝 8.3 短名组件：{raw}"));
            }
            let canonical = nemesis_path::canonicalize_for_compare(&p);
            let ws = nemesis_path::canonicalize_for_compare(workspace);
            if canonical == ws || canonical.starts_with(&ws) || ws.starts_with(&canonical) {
                return Err(format!(
                    "项目目录与主 workspace 重叠（拒绝）：{} ↔ {}",
                    p.display(),
                    workspace.display()
                ));
            }
            for e in existing {
                if canonical == *e || canonical.starts_with(e) || e.starts_with(&canonical) {
                    return Err(format!(
                        "项目目录与其他项目目录重叠（拒绝）：{} ↔ {}",
                        p.display(),
                        e.display()
                    ));
                }
            }
            p
        }
        None => default_directory(workspace, project_name, existing)?,
    };
    if path.exists() && !path.is_dir() {
        return Err(format!(
            "项目目录已存在且是文件（拒绝）：{}",
            path.display()
        ));
    }
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("创建项目目录失败 {}: {e}", path.display()))?;
    Ok(nemesis_path::canonicalize_for_compare(&path))
}

// ---------------------------------------------------------------------------
// 四件套脚手架 + project.json（B3/B4）
// ---------------------------------------------------------------------------

/// project.json（档案索引/清单；投影件，可从 board.db 重建）。`missing_blocks`
/// 非空 = P3/D6 回传核验发现的缺失块（⚠ 对前端可见）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectManifest {
    pub project_id: i64,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub integrity: String,
    #[serde(default)]
    pub missing_blocks: Vec<String>,
    pub created_at: String,
}

/// `.gitignore` 内容（B6）：投影文件不进版本库；跟踪树 = 工作集 + artifacts/
/// （依赖任务产物流转通道）。P2 落盘、P4 git 化时生效。
pub const GITIGNORE_CONTENT: &str = concat!(
    "# nemesis-board 项目档案：投影文件可从 board.db 重建，不进版本库（goal B6）\n",
    "# git 跟踪树 = 工作集（代码 + artifacts/ 交付物）\n",
    "/project.json\n",
    "/timeline.jsonl\n",
    "/docs/\n",
    "/records/\n",
);

/// 幂等初始化四件套（已存在的文件不覆盖——用户选已有目录时其现有文件
/// 作为初始基线保留，goal B5）。返回是否新建了脚手架（首次 = true）。
pub fn ensure_scaffold(
    root: &Path,
    project_id: i64,
    name: &str,
    status: &str,
) -> Result<bool, String> {
    let fresh = !root.join("project.json").exists();
    for sub in ["docs/review", "artifacts", "records"] {
        std::fs::create_dir_all(root.join(sub))
            .map_err(|e| format!("创建档案子目录 {sub} 失败: {e}"))?;
    }
    if fresh {
        let manifest = ProjectManifest {
            project_id,
            name: name.to_string(),
            status: status.to_string(),
            integrity: "ok".to_string(),
            missing_blocks: Vec::new(),
            created_at: chrono::Local::now().to_rfc3339(),
        };
        write_manifest(root, &manifest)?;
    }
    let timeline = root.join("timeline.jsonl");
    if !timeline.exists() {
        std::fs::write(&timeline, b"").map_err(|e| format!("创建 timeline.jsonl 失败: {e}"))?;
    }
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, GITIGNORE_CONTENT)
            .map_err(|e| format!("创建 .gitignore 失败: {e}"))?;
    }
    Ok(fresh)
}

/// 读 project.json（缺失/损坏 = None——投影可重建，不炸）。
pub fn read_manifest(root: &Path) -> Option<ProjectManifest> {
    let raw = std::fs::read_to_string(root.join("project.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 写 project.json（先临时文件再 rename，防半行 JSON）。
pub fn write_manifest(root: &Path, manifest: &ProjectManifest) -> Result<(), String> {
    let tmp = root.join("project.json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(manifest).map_err(|e| format!("manifest 序列化失败: {e}"))?,
    )
    .map_err(|e| format!("写 project.json 失败: {e}"))?;
    std::fs::rename(&tmp, root.join("project.json"))
        .map_err(|e| format!("project.json rename 失败: {e}"))
}

/// timeline 事件（追加写 jsonl；每行一个 JSON 对象）。
pub fn append_timeline(
    root: &Path,
    kind: &str,
    issue: Option<&str>,
    actor: &str,
    summary: &str,
) -> Result<(), String> {
    let event = serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "kind": kind,
        "issue": issue,
        "actor": actor,
        "summary": summary,
    });
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("timeline.jsonl"))
        .map_err(|e| format!("打开 timeline.jsonl 失败: {e}"))?;
    writeln!(f, "{event}").map_err(|e| format!("追加 timeline 事件失败: {e}"))
}

#[cfg(test)]
mod tests;
