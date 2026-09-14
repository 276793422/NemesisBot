//! 项目目录 git 仓库化 + 三方合并（看板项目档案 goal P4/E1+H4+H5）。
//!
//! 档案目录既是工作集也是档案（goal B4）——本模块把它变成 git 仓库（master
//! 进程内 git2，静态编译，零外部 git 安装/零新进程/零新端口）：
//!
//! - [`ensure_repo`]：幂等 init（缺则建；对现有内容首 commit——用户选已有
//!   目录作初始基线是 B5 合法形态；`.gitignore` 照常生效，投影文件不进库）；
//! - [`commit_worktree`]：工作集现状落一笔 commit（tree 无变化 = no-op）；
//! - [`export_head_tree`]：HEAD 树全量导出到目录（E2 基线快照，天然只含
//!   跟踪树——投影文件已被 .gitignore 排除，不会下发历史执行记录）；
//! - [`merge_changeset`]：E4 三方合并——基线 commit 为 ancestor、worker 树
//!   为 theirs、当前 HEAD 为 ours（行级合并，git2 [`Repository::merge_trees`]）；
//!   无冲突 = 自动合入 + commit + 工作区同步；冲突 = 逐文件明细（内容级
//!   二进制判定，E5）原样返回，**不触碰仓库任何状态**。
//!
//! 并发模型：所有函数每次调用独立 open 仓库、用完即弃（`git2::Repository`
//! 是 Send + !Sync；短生命周期对象不做全局共享，天然免锁）。合并串行化由
//! 调用方保证（E4：每收一个变更集合一次 commit 一次）。

use std::path::{Path, PathBuf};

use git2::{IndexAddOption, IndexEntry, IndexTime, ObjectType, Oid, Repository, ResetType, Tree};

/// commit author/committer（固定身份；档案仓库不接受外部身份配置）。
const AUTHOR_NAME: &str = "nemesis-board";
const AUTHOR_EMAIL: &str = "nemesis-board@localhost";

/// git 内容级二进制判定（同 git/heuristic：前 8000 字节含 NUL）。
pub fn looks_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|&b| b == 0)
}

/// 变更集单文件 upsert（新增/修改统一形态；内容全集随行）。
#[derive(Debug, Clone)]
pub struct ChangesetFile {
    /// `/` 分隔仓库相对路径。
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

/// E8：变更集归属校验 + 合并输入（worker 声明其基线 commit——祖先由此
/// 而来；「是否属于活跃 dispatch 轮次」由调用方按 dispatch 记录裁决）。
#[derive(Debug, Clone, Default)]
pub struct MergeInput {
    /// worker 声明的基线 commit hex（派发时 master 下发的 HEAD）。
    pub baseline_commit: String,
    pub upserts: Vec<ChangesetFile>,
    pub deletions: Vec<String>,
}

/// 合并结果。冲突时**仓库零改动**（树对象可能入 odb——不可达垃圾，无害）。
#[derive(Debug, Clone)]
pub enum MergeOutcome {
    /// 已合入并 commit（空变更集 = no-op，返回当前 HEAD，不产空 commit）。
    Merged { commit_oid: String },
    /// 真冲突：逐文件明细（`binary` = 内容级判定，E5 二进制不进行级合并）。
    Conflict { files: Vec<ConflictFile> },
}

#[derive(Debug, Clone)]
pub struct ConflictFile {
    pub path: String,
    pub binary: bool,
    /// 我方（当前 HEAD）阶段内容（该侧无条目 = None，如 worker 新增文件）。
    pub ours: Option<Vec<u8>>,
    /// 对方（worker 变更集）阶段内容。
    pub theirs: Option<Vec<u8>>,
    /// 共同祖先（worker 声明基线）阶段内容（双方新增 = None）。
    pub ancestor: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// 路径自防御（git 层第二道闸；wire 层另有 transfer::safe_relative_path）
// ---------------------------------------------------------------------------

/// 8.3 短名组件判定（transfer.rs `is_short_name` 逐字节同款：`NAME~<数字>`
/// 后跟组件结尾或 `.`；语义对齐防漂移）。
fn component_is_short_name(comp: &str) -> bool {
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

/// 变更集路径形状校验：`/` 分隔、非空、无 `.`/`..`/空组件、无反斜杠/盘符、
/// 无 8.3 短名（archive.rs 同款教训：canonicalize 前先词法拦截）。
fn validate_rel_path(rel: &str) -> Result<PathBuf, String> {
    let t = rel.trim();
    if t.is_empty() {
        return Err("变更集路径为空".into());
    }
    if t.contains('\\') {
        return Err(format!("变更集路径拒绝反斜杠：{t}"));
    }
    if t.contains(':') {
        return Err(format!("变更集路径拒绝盘符/冒号：{t}"));
    }
    let mut out = PathBuf::new();
    for comp in t.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(format!("变更集路径拒绝特殊组件：{t}"));
        }
        // 8.3 短名：`NAME~<数字>` 后跟组件结尾或 `.`（transfer.rs
        // is_short_name 逐字节同款——覆盖 BASE~1 与 BASE~1.EXT 双形态）。
        if component_is_short_name(comp) {
            return Err(format!("变更集路径拒绝 8.3 短名组件：{t}"));
        }
        out.push(comp);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// E1：仓库幂等 init + 工作集 commit
// ---------------------------------------------------------------------------

/// 幂等 ensure：目录内已有仓库（`.git` 在场）直接打开；否则 init + 对现有
/// 内容首 commit（`.gitignore` 生效——投影文件不进库）。返回是否新建仓库。
pub fn ensure_repo(root: &Path) -> Result<bool, String> {
    let fresh = !root.join(".git").exists();
    let repo = if fresh {
        Repository::init(root).map_err(|e| format!("init 项目仓库 {} 失败: {e}", root.display()))?
    } else {
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?
    };
    // 档案仓库钉死本地配置：基线/变更集契约是**字节精确**（sha256/size 双
    // 对账），而机器全局 gitconfig 的 `core.autocrlf` 会经 add/checkout 过滤
    // 器改写行尾（本机 autocrlf=true 实测把 LF 顶成 CRLF）——本地显式 false
    // 压住（幂等，每次 ensure 都写；local 层优先级压过 system/global）。
    let mut cfg = repo.config().map_err(|e| format!("读仓库配置失败: {e}"))?;
    cfg.set_bool("core.autocrlf", false)
        .map_err(|e| format!("钉 core.autocrlf=false 失败: {e}"))?;
    if fresh {
        commit_worktree_in(&repo, "init: 项目档案基线（nemesis-board 自动建档）")?;
        return Ok(true);
    }
    Ok(false)
}

/// 工作集现状落一笔 commit（tree 无变化 = no-op 返回 None）。
/// `.gitignore` 命中文件不入库（add_all 默认跳过）。返回 commit hex。
pub fn commit_worktree(root: &Path, message: &str) -> Result<Option<String>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    commit_worktree_in(&repo, message)
}

fn commit_worktree_in(repo: &Repository, message: &str) -> Result<Option<String>, String> {
    let mut index = repo.index().map_err(|e| e.to_string())?;
    // 见 checkpoint.rs 坑注：空 pathspec + 回调 = libgit2 空指针崩；
    // 必须传非空全匹配 spec（** 递归匹配含子目录）。回调传 None（无 shim）。
    index
        .add_all(["**"], IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("stage 工作集失败: {e}"))?;
    let tree_oid = index.write_tree().map_err(|e| e.to_string())?;
    index.write().map_err(|e| format!("写 index 失败: {e}"))?;

    let parent_commit = head_commit(repo);
    if let Some(parent) = &parent_commit
        && parent.tree_id() == tree_oid
    {
        return Ok(None); // tree 无变化 → 不产空 commit
    }

    let sig = signature()?;
    let tree = repo.find_tree(tree_oid).map_err(|e| e.to_string())?;
    let parents: Vec<&git2::Commit> = parent_commit.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("commit 失败: {e}"))?;
    Ok(Some(oid.to_string()))
}

fn signature() -> Result<git2::Signature<'static>, String> {
    git2::Signature::now(AUTHOR_NAME, AUTHOR_EMAIL).map_err(|e| e.to_string())
}

/// HEAD commit（unborn HEAD / 空仓库 = None）。
fn head_commit(repo: &Repository) -> Option<git2::Commit<'_>> {
    let head = repo.head().ok()?;
    head.peel_to_commit().ok()
}

/// HEAD commit hex（空仓库 = None；E2 派发里程碑基线标记用）。
pub fn head_commit_hex(root: &Path) -> Result<Option<String>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    Ok(head_commit(&repo).map(|c| c.id().to_string()))
}

// ---------------------------------------------------------------------------
// E2：HEAD 树导出（基线快照；只含跟踪树）
// ---------------------------------------------------------------------------

/// HEAD 树全量导出到 `dest`（已存在的 `dest` 先清空）。返回 (文件数, 字节数)。
/// 空仓库 = Ok((0, 0))（dest 建空目录）。子模块/gitlink 跳过。
pub fn export_head_tree(root: &Path, dest: &Path) -> Result<(usize, u64), String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| format!("清空导出目录失败: {e}"))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("建导出目录失败: {e}"))?;
    let Some(commit) = head_commit(&repo) else {
        return Ok((0, 0));
    };
    let tree = commit.tree().map_err(|e| e.to_string())?;
    let mut stats = (0usize, 0u64);
    walk_tree(&repo, &tree, "", dest, &mut stats)?;
    Ok(stats)
}

fn walk_tree(
    repo: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    dest: &Path,
    stats: &mut (usize, u64),
) -> Result<(), String> {
    for entry in tree.iter() {
        let name = String::from_utf8_lossy(entry.name_bytes()).to_string();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Tree) => {
                let sub = repo.find_tree(entry.id()).map_err(|e| e.to_string())?;
                walk_tree(repo, &sub, &rel, dest, stats)?;
            }
            Some(ObjectType::Blob) => {
                let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
                let dest_path = dest.join(validate_rel_path(&rel)?);
                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("建目录 {}: {e}", parent.display()))?;
                }
                let content = blob.content();
                std::fs::write(&dest_path, content)
                    .map_err(|e| format!("写 {}: {e}", dest_path.display()))?;
                stats.0 += 1;
                stats.1 += content.len() as u64;
            }
            _ => {} // gitlink/commit（子模块）跳过——档案树不含子模块语义
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// E4：三方合并
// ---------------------------------------------------------------------------

/// 空树 oid（theirs 全删/基线空时的边界）。
fn empty_tree(repo: &Repository) -> Result<Tree<'_>, String> {
    let oid = repo
        .treebuilder(None)
        .and_then(|tb| tb.write())
        .map_err(|e| e.to_string())?;
    repo.find_tree(oid).map_err(|e| e.to_string())
}

/// E4 三方合并（调用方保证串行）。冲突 = 仓库零改动原样返回明细。
///
/// 流程：ancestor = worker 声明的基线 commit（E8：是否属于活跃 dispatch
/// 轮次由调用方先裁决，本函数只管 git 语义）→ theirs = 基线树 + 变更集
/// （内存 index 组装，不触碰盘上 index/workdir）→ `merge_trees` → 干净 =
/// commit + hard reset 同步工作区；冲突 = 收集逐文件明细（内容级二进制判定）。
pub fn merge_changeset(root: &Path, input: &MergeInput) -> Result<MergeOutcome, String> {
    // 变更集路径先全量过闸（任何一条畸形 = 拒绝整个变更集，不合入）。
    for f in &input.upserts {
        validate_rel_path(&f.path)?;
    }
    for d in &input.deletions {
        validate_rel_path(d)?;
    }

    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;

    // ancestor：worker 声明的基线 commit（master 派发时 commit 过，对象必在）。
    let base_oid =
        Oid::from_str(&input.baseline_commit).map_err(|e| format!("基线 commit 非法: {e}"))?;
    let base_commit = repo.find_commit(base_oid).map_err(|e| {
        format!("基线 commit {base_oid} 不在本仓库（worker 申报与仓库历史失配）: {e}")
    })?;
    let ancestor_tree = base_commit.tree().map_err(|e| e.to_string())?;

    // ours：当前 HEAD（空仓库 = 空树）。
    let ours_tree = match head_commit(&repo) {
        Some(c) => c.tree().map_err(|e| e.to_string())?,
        None => empty_tree(&repo)?,
    };

    // theirs：repo 背书的内存 index = 基线树 + upsert + 删除（不触碰盘上
    // index——read_tree 整体替换内容、全程不 write()；独立 Index::new() 无
    // repo 背书，add_frombuffer 会被 libgit2 以「not backed up」拒绝）。
    let mut theirs_index = repo.index().map_err(|e| e.to_string())?;
    theirs_index
        .read_tree(&ancestor_tree)
        .map_err(|e| format!("读基线树进内存 index 失败: {e}"))?;
    for f in &input.upserts {
        let entry = IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: if f.executable { 0o100755 } else { 0o100644 },
            uid: 0,
            gid: 0,
            file_size: 0,
            id: Oid::ZERO_SHA1,
            flags: 0,
            flags_extended: 0,
            path: f.path.as_bytes().to_vec(),
        };
        theirs_index
            .add_frombuffer(&entry, &f.content)
            .map_err(|e| format!("变更集 upsert {} 进 index 失败: {e}", f.path))?;
    }
    for d in &input.deletions {
        let p = validate_rel_path(d)?;
        theirs_index
            .remove(&p, 0)
            .map_err(|e| format!("变更集删除 {d} 不在基线树中: {e}"))?;
    }
    let theirs_oid = theirs_index
        .write_tree_to(&repo)
        .map_err(|e| format!("组装 worker 树失败: {e}"))?;
    let theirs_tree = repo.find_tree(theirs_oid).map_err(|e| e.to_string())?;

    // 三方合并（行级；不做 rename 检测——看板子单语义下 rename 罕见，
    // 检测反而增加误合面）。
    let mut opts = git2::MergeOptions::new();
    opts.find_renames(false);
    let mut merged = repo
        .merge_trees(&ancestor_tree, &ours_tree, &theirs_tree, Some(&opts))
        .map_err(|e| format!("三方合并失败: {e}"))?;

    if merged.has_conflicts() {
        // 迭代中途的 Err 当致命（不静默跳过——漏一个冲突文件就是假绿）。
        let conflicts: Vec<git2::IndexConflict> = merged
            .conflicts()
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        for conflict in conflicts {
            // 冲突条目：ours/theirs/ancestor 三阶段，任一在场的 path 都算。
            let (path, our_data, their_data) = match (&conflict.our, &conflict.their) {
                (Some(o), Some(t)) => (
                    o.path.clone(),
                    Some(blob_content(&repo, o.id)),
                    Some(blob_content(&repo, t.id)),
                ),
                (Some(o), None) => (o.path.clone(), Some(blob_content(&repo, o.id)), None),
                (None, Some(t)) => (t.path.clone(), None, Some(blob_content(&repo, t.id))),
                (None, None) => match &conflict.ancestor {
                    Some(a) => (a.path.clone(), None, None),
                    None => continue,
                },
            };
            let path_str = String::from_utf8_lossy(&path).to_string();
            // P5/F5：三阶段内容随行带出（AI 硬解/审计明细卡的数据源；
            // blob 缺失降级 None，不致命——明细卡少一段展示而已）。
            let ancestor_data = conflict
                .ancestor
                .as_ref()
                .map(|a| blob_content(&repo, a.id));
            // E5：内容级二进制判定（任一侧二进制 = 不进行级合并，择边语义）。
            let binary = [our_data.clone(), their_data.clone()]
                .into_iter()
                .flatten()
                .any(|d| looks_binary(&d));
            files.push(ConflictFile {
                path: path_str,
                binary,
                ours: our_data,
                theirs: their_data,
                ancestor: ancestor_data,
            });
        }
        return Ok(MergeOutcome::Conflict { files });
    }

    // 干净：tree 无实际变化 = 空变更集 no-op（goal「空集宽容」，不产空 commit）。
    let merged_oid = merged.write_tree_to(&repo).map_err(|e| e.to_string())?;
    let parent = head_commit(&repo);
    if let Some(p) = &parent
        && p.tree_id() == merged_oid
    {
        return Ok(MergeOutcome::Merged {
            commit_oid: p.id().to_string(),
        });
    }

    // commit（parent = 当前 HEAD）+ hard reset 同步工作区（调用方已在合并前
    // commit_worktree 保证工作区干净——reset 只是把 index/workdir 推到新树）。
    let tree = repo.find_tree(merged_oid).map_err(|e| e.to_string())?;
    let sig = signature()?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let message = format!("merge: 变更集合入（基线 {base_oid}）");
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)
        .map_err(|e| format!("合并 commit 失败: {e}"))?;
    let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
    repo.reset(commit.as_object(), ResetType::Hard, None)
        .map_err(|e| format!("合并后同步工作区失败: {e}"))?;
    // libgit2 reset --hard 只写不删：index 先行重置后，checkout 把「目标树
    // 没有的旧跟踪文件」当 untracked 保留（本盘实测变更集删除 src/other.h
    // 后残留成 ??）。checkpoint.rs 恢复工作区同款教训——删除按 diff 显式补刀。
    if let Some(p) = &parent {
        let old_tree = p.tree().map_err(|e| e.to_string())?;
        let diff = repo
            .diff_tree_to_tree(Some(&old_tree), Some(&tree), None)
            .map_err(|e| e.to_string())?;
        for delta in diff.deltas() {
            if delta.status() != git2::Delta::Deleted {
                continue;
            }
            let Some(path) = delta.old_file().path() else {
                continue;
            };
            // 防逃逸（git 相对路径理论不含 ..，checkpoint.rs 同款防御保留）。
            let rel = path.to_string_lossy();
            if rel.split(['/', '\\']).any(|seg| seg == "..") {
                continue;
            }
            let abs = root.join(&*rel);
            std::fs::remove_file(&abs)
                .map_err(|e| format!("合并后删除工作区文件 {rel} 失败: {e}"))?;
        }
    }
    Ok(MergeOutcome::Merged {
        commit_oid: oid.to_string(),
    })
}

fn blob_content(repo: &Repository, id: Oid) -> Vec<u8> {
    repo.find_blob(id)
        .map(|b| b.content().to_vec())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// P5/F5：冲突硬解落盘
// ---------------------------------------------------------------------------

/// 冲突硬解产出落一笔 commit：HEAD 树起步 + override 条目替换/新增 →
/// commit（自定义 message，审计要求 auto-resolve 产物可辨认）→ hard reset
/// 同步工作区。与 merge_changeset 干净路径同构，但不走工作集 staging——
/// 解题内容来自 resolver 内存产物，与盘上工作区现状无关。
/// override 相对 HEAD 只有替换/新增（无删除语义），树无实际变化 = no-op
/// 返回当前 HEAD（同 merge 空集语义，不产空 commit）。返回 commit hex。
pub fn commit_resolution(
    root: &Path,
    overrides: Vec<(String, Vec<u8>)>,
    message: &str,
) -> Result<String, String> {
    for (path, _) in &overrides {
        validate_rel_path(path)?;
    }
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;

    let mut index = repo.index().map_err(|e| e.to_string())?;
    match head_commit(&repo) {
        Some(c) => {
            let tree = c.tree().map_err(|e| e.to_string())?;
            index
                .read_tree(&tree)
                .map_err(|e| format!("读 HEAD 树进 index 失败: {e}"))?;
        }
        None => index.clear().map_err(|e| e.to_string())?,
    }
    for (path, content) in &overrides {
        let entry = IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 0,
            id: Oid::ZERO_SHA1,
            flags: 0,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        };
        index
            .add_frombuffer(&entry, content)
            .map_err(|e| format!("硬解 override {path} 进 index 失败: {e}"))?;
    }
    let tree_oid = index
        .write_tree_to(&repo)
        .map_err(|e| format!("组装硬解树失败: {e}"))?;

    let parent = head_commit(&repo);
    if let Some(p) = &parent
        && p.tree_id() == tree_oid
    {
        return Ok(p.id().to_string()); // 树无实际变化 → no-op
    }

    let tree = repo.find_tree(tree_oid).map_err(|e| e.to_string())?;
    let sig = signature()?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("硬解 commit 失败: {e}"))?;
    let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
    repo.reset(commit.as_object(), ResetType::Hard, None)
        .map_err(|e| format!("硬解后同步工作区失败: {e}"))?;
    Ok(oid.to_string())
}

#[cfg(test)]
mod tests;
