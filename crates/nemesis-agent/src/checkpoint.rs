//! Checkpoint store — snapshot-based edit safety net.
//!
//! Before a writer tool (`write_file`/`edit_file`/`append_file`/`delete_file`)
//! changes a file, the agent records the file's pre-edit content here, keyed to
//! the current user turn. A rewind can then restore the workspace to an earlier
//! turn — restoring code, or (caller-side) the conversation, or both.
//!
//! D2（devtool-upgrade 阶段 5，2026-09-06）：**双后端**。
//!
//! - **git 影子库**（`{workspace}/.git` 存在且为目录时启用）：独立 git-dir
//!   `{workspace}/logs/checkpoints.git`（bare init + `set_workdir` attach，
//!   绝不触碰用户真实 `.git`），`objects/info/alternates` 指向真实仓库
//!   objects → 已提交 blob 零拷贝复用，只有未提交内容落影子库。track =
//!   turn 边界（`begin`）对整个工作区 `index.add_all` + `write_tree`，记
//!   `(turn, tree_oid)`——**shell/exec 副作用也被快照**（工具级 preview
//!   声明只用于 picker 展示与落盘纪律）。restore =
//!   `diff_tree_to_tree(target..current)` 分类 → `checkout_tree` 按路径恢复
//!   加显式删除多余文件——diff 集是真实工作区差异，天然覆盖 shell 产物。
//!   v1 不做 gc：影子库对象随 turn JSON 的 truncate 清索引，对象本体留待
//!   后续 TTL/gc。
//! - **JSON 回落**（D4 约束）：无 `.git`、`.git` 是文件（linked worktree/
//!   submodule）、或影子库初始化失败 → 原有 git-free JSON 快照（每个 turn
//!   一个 JSON 文件，`FileSnap` 存全文）原样保留，行为逐字节不变。
//!   两形态回归测试齐备。
//!
//! 快照落盘：`{workspace}/logs/checkpoints/`（2026-08-30 统一收编进 logs
//! 家族）。git 模式沿用同一 JSON 索引——`Checkpoint.tree` 存 tree hash、
//! 工作区内文件的 `files[].content` 恒 None（内容在 git 对象里）、`paths`
//! 存工具声明路径供 picker 展示（JSON 模式 `paths` 同步填充，老 JSON 无此
//! 字段由 `list_meta` 回退 `files`）。**跨模式安全**：会话丢失 `.git` 后以
//! JSON 形态加载 git 时代索引时，restore 按 `tree.is_some()` 识别 git 时代
//! 记录、对 in-root None 条目诚实 no-op（绝不按 JSON 语义误删）。
//!
//! 落盘纪律（2026-08-30）：**只有产生了文件快照的 turn 才落盘**。内部请求
//! （history 翻页/watchdog）的空 turn 只存在于内存、不写文件。git 模式
//! `begin` 时若工作区 tree 与上一 turn 不同（shell 副作用已发生）也落盘，
//! 否则空 turn 同样跳过——翻页请求不留壳。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::r#loop::{FileChange, FileChangeKind};

/// One file's pre-edit state at the moment it was first touched in a turn.
/// `content == None` means the file did not exist then, so a restore deletes it.
///
/// git 模式下工作区内文件的 content 恒为 `None`（内容在影子库 tree 里，不
/// 双份存储）；工作区外的声明路径（absolute path 写出工作区、restrict 关闭
/// 时合法）仍按 JSON 语义读全文——影子树罩不住工作区外。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnap {
    pub path: String,
    pub content: Option<String>,
}

/// Anchors the pre-edit state of every distinct file touched during one user turn.
///
/// `paths`（D2 新增，serde default 兼容老 JSON）：本 turn 工具声明的变更路径
/// （preview_all 集合），供 picker/meta 展示。`tree`（D2 新增）：git 模式下
/// `begin` 时刻的影子库 tree hex；JSON 模式恒 None。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub turn: usize,
    pub time: String, // RFC3339
    pub prompt: String,
    pub files: Vec<FileSnap>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub tree: Option<String>,
}

/// Picker-facing summary of a checkpoint (no file contents).
#[derive(Debug, Clone)]
pub struct CheckpointMeta {
    pub turn: usize,
    pub time: String,
    pub prompt: String,
    pub paths: Vec<String>,
}

/// M3：某文件在本库的最早基线锚（首个声明过它的 checkpoint 条目）。
///
/// `tree`：git 形态的影子 tree（内容从 tree 读 blob）；`content`：JSON
/// 形态的 pre-edit 快照内容。二者互补恒有一个可用（同 Checkpoint 语义）。
#[derive(Debug, Clone)]
pub struct CheckpointBase {
    pub turn: usize,
    pub tree: Option<String>,
    pub content: Option<String>,
}

/// Which persistence backend the store selected at construction (D4：
/// 诊断/测试用——`backend()` 返回 "git" 或 "json"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointBackend {
    /// git 影子库（`{workspace}/.git` 存在且为目录）。
    Git,
    /// JSON 快照回落（无 .git / .git 文件形态 / 影子库初始化失败）。
    Json,
}

struct Inner {
    done: Vec<Checkpoint>,
    cur: Option<Checkpoint>,
    seen: HashSet<String>, // paths already snapshotted in the current turn
}

/// git2 `Repository` is Send but !Sync — Mutex 使其可跨 await 共享。
/// 锁纪律：repo 锁与 inner 锁绝不互相嵌套持有（先 repo 操作、放锁、再
/// inner，反之亦然），杜绝双锁排序死锁。
struct GitBackend {
    repo: Mutex<git2::Repository>,
}

/// Holds a session's checkpoints in memory and, when `dir` is set, persists one
/// JSON file per turn under it. All methods are safe for concurrent use.
pub struct CheckpointStore {
    dir: Option<PathBuf>,
    root: PathBuf,
    git: Option<GitBackend>,
    inner: Mutex<Inner>,
}

impl CheckpointStore {
    /// Create a store for the given checkpoint dir and workspace root, loading
    /// any checkpoints already persisted under `dir`. `dir = None` disables
    /// persistence (in-memory only for the session).
    ///
    /// Backend selection (D4)：`{root}/.git` 存在且为**目录** → git 影子库；
    /// 其余（无 .git / .git 文件 / 初始化失败）→ JSON 回落。模式在构造期
    /// 固定，运行中新建 .git 不热切换（诚实边界，见模块文档）。
    pub fn new(dir: Option<PathBuf>, root: PathBuf) -> Self {
        let git = Self::try_git_backend(&root);
        let store = Self {
            dir,
            root,
            git,
            inner: Mutex::new(Inner {
                done: Vec::new(),
                cur: None,
                seen: HashSet::new(),
            }),
        };
        store.load();
        store
    }

    /// D4 诊断口：本 store 实际启用的后端。
    pub fn backend(&self) -> CheckpointBackend {
        if self.git.is_some() {
            CheckpointBackend::Git
        } else {
            CheckpointBackend::Json
        }
    }

    // ------------------------------------------------------------------
    // git 影子库初始化 / 打开
    // ------------------------------------------------------------------

    fn try_git_backend(root: &Path) -> Option<GitBackend> {
        let real_git = root.join(".git");
        // 只认目录形态的 .git；文件形态（linked worktree / submodule）→ JSON 回落。
        if !real_git.is_dir() {
            return None;
        }
        let shadow = nemesis_path::logs_dir_in_workspace(root).join("checkpoints.git");
        let result = if shadow.exists() {
            Self::open_shadow(&shadow, root)
        } else {
            Self::init_shadow(&shadow, root)
        };
        match result {
            Ok(repo) => {
                if let Err(e) = Self::ensure_alternates(&shadow, root) {
                    warn!("[checkpoint] alternates 写失败（blob 复用降级，不影响正确性）: {e}");
                }
                Some(GitBackend {
                    repo: Mutex::new(repo),
                })
            }
            Err(e) => {
                warn!("[checkpoint] git 影子库不可用，回落 JSON 快照（D4）: {e}");
                None
            }
        }
    }

    fn init_shadow(shadow: &Path, root: &Path) -> Result<git2::Repository, String> {
        // 以 bare 形态初始化（gitdir = shadow），再 attach 工作区。不能用
        // no_dotgit_dir + workdir_path 直接 init——工作区里已有真实 .git 时
        // libgit2 报 "cannot overwrite gitlink file"。set_workdir 第二参
        // false = 不往工作区写 .git gitlink（绝不碰用户真实 .git）。
        let mut opts = git2::RepositoryInitOptions::new();
        opts.bare(true).mkpath(true);
        let repo = git2::Repository::init_opts(shadow, &opts).map_err(|e| e.to_string())?;
        repo.set_workdir(root, false).map_err(|e| e.to_string())?;
        Self::ensure_workdir(&repo, root)?;
        Ok(repo)
    }

    fn open_shadow(shadow: &Path, root: &Path) -> Result<git2::Repository, String> {
        let repo = git2::Repository::open_ext(
            shadow,
            git2::RepositoryOpenFlags::NO_SEARCH,
            Vec::<&std::ffi::OsStr>::new(),
        )
        .map_err(|e| e.to_string())?;
        // 双保险：config 里的 core.worktree 可能漂（目录搬移），打开时钉回。
        Self::ensure_workdir(&repo, root)?;
        Ok(repo)
    }

    /// 确保 repo 的 workdir 解析到工作区（init 的 workdir_path / 老库的
    /// config 都只是尽力而为，这里统一钉死）。
    fn ensure_workdir(repo: &git2::Repository, root: &Path) -> Result<(), String> {
        let ok = repo.workdir().is_some_and(|w| w.starts_with(root));
        if !ok {
            repo.set_workdir(root, false).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// `objects/info/alternates` 指向真实仓库 objects → 已提交 blob 零拷贝
    /// 复用。幂等：内容一致不重写。
    fn ensure_alternates(shadow: &Path, root: &Path) -> std::io::Result<()> {
        let info_dir = shadow.join("objects").join("info");
        let file = info_dir.join("alternates");
        let want = root
            .join(".git")
            .join("objects")
            .to_string_lossy()
            .replace('\\', "/");
        if std::fs::read_to_string(&file).is_ok_and(|c| c.lines().any(|l| l.trim() == want)) {
            return Ok(());
        }
        std::fs::create_dir_all(&info_dir)?;
        std::fs::write(&file, format!("{want}\n"))
    }

    // ------------------------------------------------------------------
    // git 操作（全部短锁：拿 repo 锁做完即放，绝不嵌套 inner 锁）
    // ------------------------------------------------------------------

    /// 工作区现状 → 影子库 tree hex。add_all 默认跳过 .gitignore 命中文件；
    /// matcher 再跳过运行时属主目录（`is_workspace_ignored` 单一真相源——
    /// logs/cluster/board/... 与 fs_watcher 同表）。
    ///
    /// ⚠️ libgit2 坑（2026-09-06 实测）：**空 pathspec + 回调**组合下，
    /// `git_pathspec__match` 对空 spec 直接 `return true` 但不填
    /// `matched_pathspec`（pathspec.c:209-211）→ 回调收到 NULL → git2-rs
    /// shim `CStr::from_ptr(NULL)` 段错误（STATUS_ACCESS_VIOLATION）。必须传
    /// 非空全匹配 spec（`**` 递归匹配含子目录），不能用空数组表示「全部」。
    fn write_current_tree(repo: &mut git2::Repository) -> Result<String, String> {
        let mut index = repo.index().map_err(|e| e.to_string())?;
        index
            .add_all(
                ["**"], // 见下方坑注：空 pathspec + 回调 = libgit2 空指针崩
                git2::IndexAddOption::DEFAULT,
                Some(&mut |path: &Path, _spec: &[u8]| {
                    if nemesis_path::is_workspace_ignored(path) {
                        1 // skip
                    } else {
                        0 // add
                    }
                }),
            )
            .map_err(|e| e.to_string())?;
        // 先 write（index 落盘 = stat 缓存跨 turn 复用，重复 add_all 只重哈
        // 希 stat 变化的文件），再 write_tree。
        index.write().map_err(|e| e.to_string())?;
        Ok(index.write_tree().map_err(|e| e.to_string())?.to_string())
    }

    /// restore（git 模式）核心：diff target..current 分类出要恢复/要删除的
    /// 路径集，然后 checkout_tree（按路径）+ 逐文件删除。
    fn git_restore(
        root: &Path,
        repo: &mut git2::Repository,
        target_hex: &str,
    ) -> (Vec<String>, Vec<String>) {
        // 现状 tree（同样跳过运行时属主目录）。先做可变操作（index 写入），
        // 再拿 immutable 的 Tree 句柄——git2 的 Tree 借用 repo，顺序不能反。
        let cur_hex = match Self::write_current_tree(repo) {
            Ok(h) => h,
            Err(e) => {
                warn!("[checkpoint] 现状 tree 写入失败，放弃 git restore: {e}");
                return (Vec::new(), Vec::new());
            }
        };
        let target_oid = match git2::Oid::from_str(target_hex) {
            Ok(o) => o,
            Err(e) => {
                warn!("[checkpoint] tree oid 解析失败 {target_hex}: {e}");
                return (Vec::new(), Vec::new());
            }
        };
        let target_tree = match repo.find_tree(target_oid) {
            Ok(t) => t,
            Err(e) => {
                // 影子库对象缺失（如真实仓库 gc 掉了 alternates 引用的对象）。
                warn!("[checkpoint] 影子 tree 不可达 {target_hex}: {e}");
                return (Vec::new(), Vec::new());
            }
        };
        let cur_oid = match git2::Oid::from_str(&cur_hex) {
            Ok(o) => o,
            Err(_) => return (Vec::new(), Vec::new()),
        };
        let cur_tree = match repo.find_tree(cur_oid) {
            Ok(t) => t,
            Err(_) => return (Vec::new(), Vec::new()),
        };

        let diff = match repo.diff_tree_to_tree(Some(&target_tree), Some(&cur_tree), None) {
            Ok(d) => d,
            Err(e) => {
                warn!("[checkpoint] tree diff 失败: {e}");
                return (Vec::new(), Vec::new());
            }
        };

        // Added（现状有、目标无）→ 删除；Deleted/Modified/Typechange → 恢复。
        let mut to_checkout: Vec<String> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();
        for delta in diff.deltas() {
            match delta.status() {
                git2::Delta::Added => {
                    if let Some(p) = delta.new_file().path() {
                        to_remove.push(p.to_string_lossy().to_string());
                    }
                }
                git2::Delta::Deleted => {
                    if let Some(p) = delta.old_file().path() {
                        to_checkout.push(p.to_string_lossy().to_string());
                    }
                }
                git2::Delta::Modified | git2::Delta::Typechange | git2::Delta::Conflicted => {
                    if let Some(p) = delta.new_file().path() {
                        to_checkout.push(p.to_string_lossy().to_string());
                    }
                }
                _ => {}
            }
        }

        // checkout_tree 一次调用带全部路径（force 覆写本地改动=restore 的本
        // 意；update_index(false) 不动影子 index——下次 add_all 靠 stat 失配
        // 重哈希，正确性不受影响）。
        let mut written = Vec::new();
        if !to_checkout.is_empty() {
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.force().update_index(false);
            for p in &to_checkout {
                cb.path(p.as_str());
            }
            match repo.checkout_tree(target_tree.as_object(), Some(&mut cb)) {
                Ok(()) => written = to_checkout,
                Err(e) => warn!("[checkpoint] checkout_tree 失败（工作区保持现状）: {e}"),
            }
        }

        // 删除多余文件（target 没有而现状有的——含 shell 产物）。经 safe_path
        // 语义防逃逸（git 相对路径理论上不含 ..，防御性保留）。
        let mut deleted = Vec::new();
        for rel in to_remove {
            if rel.split(['/', '\\']).any(|seg| seg == "..") {
                continue;
            }
            let abs = root.join(&rel);
            match std::fs::remove_file(&abs) {
                Ok(()) => deleted.push(rel),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!("[checkpoint] 删除 {rel} 失败: {e}"),
            }
        }
        (written, deleted)
    }

    fn load(&self) {
        let Some(dir) = &self.dir else { return };
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut guard = self.inner.lock();
        for ent in entries.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(cp) = serde_json::from_slice::<Checkpoint>(&bytes) {
                guard.done.push(cp);
            }
        }
        guard.done.sort_by_key(|c| c.turn);
    }

    /// Open a checkpoint for a new user turn, finalizing the previous one. The
    /// prompt labels it in the picker.
    ///
    /// git 模式：begin 时即写工作区现状 tree（turn 边界 track——exec 副作用
    /// 也被快照的关键），tree 与上一 turn 不同才落盘 JSON（翻页空 turn 不
    /// 留壳，落盘纪律同 JSON 模式）。
    pub fn begin(&self, turn: usize, prompt: impl Into<String>) {
        let prompt = prompt.into();

        // git track 先行（repo 锁独立短区间，不与 inner 锁嵌套）。
        let tree_hex = match self.git.as_ref() {
            Some(gb) => {
                let mut repo = gb.repo.lock();
                match Self::write_current_tree(&mut repo) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        warn!(
                            "[checkpoint] turn {turn} 影子 tree 写入失败（本 turn 不可 git 回滚）: {e}"
                        );
                        None
                    }
                }
            }
            None => None,
        };

        let cp = Checkpoint {
            turn,
            time: chrono::Local::now().to_rfc3339(),
            prompt,
            files: Vec::new(),
            paths: Vec::new(),
            tree: tree_hex,
        };

        let mut guard = self.inner.lock();
        let prev = guard.cur.take();
        if let Some(prev) = &prev {
            guard.done.push(prev.clone());
        }
        // 落盘纪律（git 模式）：tree 与上一 turn 一致 → 内容无变化，不落盘。
        let prev_tree = prev
            .as_ref()
            .and_then(|p| p.tree.clone())
            .or_else(|| guard.done.last().and_then(|c| c.tree.clone()));
        let changed_since_prev = cp.tree.is_some() && (prev_tree.is_none() || prev_tree != cp.tree);
        guard.cur = Some(cp.clone());
        guard.seen.clear();
        drop(guard);

        if changed_since_prev {
            self.persist(&cp);
        }
    }

    /// Snapshot the pre-edit state of the file a writer is about to change.
    /// Async — reads the file's current content. Only the first touch of a path
    /// in the current turn is kept (its turn-start content). A no-op before the
    /// first `begin`, and for paths that escape the workspace root.
    ///
    /// git 模式：工作区内文件不再读内容（begin 的 tree 已覆盖——省一次全文
    /// 读）；工作区外的声明路径仍读全文（影子树罩不住，见 FileSnap doc）。
    pub async fn snapshot(&self, change: &FileChange) {
        if change.path.is_empty() {
            return;
        }
        let in_git_mode = self.git.is_some();
        // Read current content (resolved against root) BEFORE taking the lock.
        let content = if in_git_mode && self.path_inside_root(&change.path) {
            None // tree 覆盖，不双份存
        } else {
            match change.kind {
                FileChangeKind::Create => None, // did not exist (by definition)
                FileChangeKind::Modify | FileChangeKind::Delete => {
                    match self.safe_path(&change.path) {
                        Some(abs) => tokio::fs::read_to_string(&abs).await.ok(),
                        None => None,
                    }
                }
            }
        };

        let mut guard = self.inner.lock();
        if guard.cur.is_none() {
            return;
        }
        if !guard.seen.insert(change.path.clone()) {
            return; // already snapshotted this turn
        }
        let cur = guard.cur.as_mut().expect("checked non-none above");
        cur.files.push(FileSnap {
            path: change.path.clone(),
            content,
        });
        cur.paths.push(change.path.clone());
        let cp = cur.clone();
        drop(guard);
        self.persist(&cp);
    }

    /// 声明路径是否落在工作区内（git 影子树的覆盖范围）。相对路径恒在内；
    /// 绝对路径按词法前缀判（与 fs_watcher 同语义，不解析 symlink）。
    fn path_inside_root(&self, p: &str) -> bool {
        if p.contains("..") {
            return false; // 保守：视为界外走 JSON 内容路径
        }
        let abs = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            return true;
        };
        abs.starts_with(&self.root)
    }

    /// Restore the workspace to its state at the start of turn `from_turn`.
    /// Returns (written, deleted) paths.
    ///
    /// git 模式：target = 最新 `turn <= from_turn` 的影子 tree；对 target..现状
    /// 的真实 diff 逐文件恢复/删除——覆盖 shell 副作用（preview 声明之外的
    /// 文件）。工作区外的 hybrid 内容快照（files 里 content Some 的）在 tree
    /// 恢复后按 JSON 语义补写。
    pub async fn restore_code(&self, from_turn: usize) -> (Vec<String>, Vec<String>) {
        // ---- 第一段：inner 锁内定 target tree + hybrid 集合，随即放锁 ----
        let (target_hex, hybrid) = {
            let guard = self.inner.lock();
            let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
            if let Some(cur) = guard.cur.as_ref() {
                all.push(cur);
            }
            all.sort_by_key(|c| c.turn);

            // 与 JSON 模式同款语义守卫：from_turn 之后没有任何 checkpoint → no-op。
            if !all.iter().any(|c| c.turn >= from_turn) {
                (None, Vec::new())
            } else {
                let target = all
                    .iter()
                    .filter(|c| c.turn <= from_turn)
                    .filter_map(|c| c.tree.clone())
                    .next_back();
                // hybrid：工作区外的内容快照（git 模式才可能产生；earliest wins
                // 同 JSON 模式）。in-root None = 「tree 负责」的跳过条件有两个
                // 触发面：**当前是 git 模式**（tree 恢复接手），或**该记录本身
                // 是 git 时代**（c.tree.is_some()——跨模式安全：.git 消失后以
                // JSON 形态加载 git 时代的索引，绝不能按 JSON 语义误删）。
                // 纯 JSON 时代记录（tree=None）维持原语义：None = 不存在须删除。
                let git_mode = self.git.is_some();
                let mut hybrid: Vec<(String, Option<String>)> = Vec::new();
                for c in all.iter().filter(|c| c.turn >= from_turn) {
                    for f in &c.files {
                        if (git_mode || c.tree.is_some())
                            && f.content.is_none()
                            && self.path_inside_root(&f.path)
                        {
                            continue; // 工作区内 None → tree 负责（或诚实 no-op）
                        }
                        if hybrid.iter().any(|(p, _)| p == &f.path) {
                            continue;
                        }
                        hybrid.push((f.path.clone(), f.content.clone()));
                    }
                }
                (target, hybrid)
            }
        };

        let Some(target_hex) = target_hex else {
            // 无 tree 可退（纯 JSON 老会话 / from_turn 之前无 git turn）——只做
            // hybrid 部分（等价 JSON 行为），tree 段跳过。
            return self.hybrid_restore(hybrid).await;
        };

        // ---- 第二段：repo 锁内做 tree diff + checkout，随即放锁 ----
        let mut result = match self.git.as_ref() {
            Some(gb) => {
                let mut repo = gb.repo.lock();
                Self::git_restore(&self.root, &mut repo, &target_hex)
            }
            None => {
                // 跨模式（会话曾以 git 模式记录，现 .git 消失回落 JSON）：tree
                // 段跳过，工作区内的恢复是诚实 no-op（绝无误删）。
                warn!("[checkpoint] 目标 tree {target_hex} 存在但影子库不可用，跳过工作区内恢复");
                (Vec::new(), Vec::new())
            }
        };

        // ---- 第三段：hybrid 工作区外内容恢复 ----
        let (hw, hd) = self.hybrid_restore(hybrid).await;
        result.0.extend(hw);
        result.1.extend(hd);
        result
    }

    /// JSON 语义的内容恢复（git 模式下仅工作区外路径走这里）。
    async fn hybrid_restore(
        &self,
        hybrid: Vec<(String, Option<String>)>,
    ) -> (Vec<String>, Vec<String>) {
        let mut written = Vec::new();
        let mut deleted = Vec::new();
        for (path, content) in hybrid {
            let Some(abs) = self.safe_path(&path) else {
                continue;
            };
            match content {
                None => {
                    if tokio::fs::remove_file(&abs).await.is_ok() {
                        deleted.push(path);
                    }
                }
                Some(body) => {
                    if let Some(parent) = abs.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    if tokio::fs::write(&abs, body).await.is_ok() {
                        written.push(path);
                    }
                }
            }
        }
        (written, deleted)
    }

    /// Metadata for all checkpoints (oldest turn first), for the rewind picker.
    pub fn list_meta(&self) -> Vec<CheckpointMeta> {
        let guard = self.inner.lock();
        let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
        if let Some(cur) = guard.cur.as_ref() {
            all.push(cur);
        }
        all.sort_by_key(|c| c.turn);
        all.into_iter()
            .map(|c| {
                // 老 JSON 无 paths 字段 → 回退 files 的路径（serde default 空 vec）。
                let paths = if c.paths.is_empty() {
                    c.files.iter().map(|f| f.path.clone()).collect()
                } else {
                    c.paths.clone()
                };
                CheckpointMeta {
                    turn: c.turn,
                    time: c.time.clone(),
                    prompt: c.prompt.clone(),
                    paths,
                }
            })
            .collect()
    }

    /// Discard checkpoints at or after `from_turn` (a conversation rewind removes
    /// those future turns, so their snapshots must not remain/collide).
    ///
    /// git 模式：只清 JSON 索引（v1 不做影子库对象 gc——模块文档诚实边界）。
    pub fn truncate_from(&self, from_turn: usize) {
        {
            let mut guard = self.inner.lock();
            guard.done.retain(|c| c.turn < from_turn);
            if guard.cur.as_ref().is_some_and(|c| c.turn >= from_turn) {
                guard.cur = None;
                guard.seen.clear();
            }
        }
        if let Some(dir) = &self.dir
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for ent in entries.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if let Some(rest) = name
                    .strip_prefix("turn-")
                    .and_then(|s| s.strip_suffix(".json"))
                    && let Ok(t) = rest.parse::<usize>()
                    && t >= from_turn
                {
                    let _ = std::fs::remove_file(ent.path());
                }
            }
        }
    }

    /// E3：当前最新影子 tree（done+cur 按 turn 排序，最后一个带 tree 的）。
    /// 消息级回退在截断 checkpoint 索引**前**记录 redo 前向恢复目标用。
    pub fn latest_tree_hex(&self) -> Option<String> {
        self.tree_hex_at_or_before(usize::MAX)
    }

    /// E3：`turn < from_turn` 里最后一个带 tree 的 checkpoint 的影子 tree。
    /// 即 `truncate_from(from_turn)` 之后索引里剩下的最新 tree——redo 的
    /// 陈旧性守卫基线（期间任何新 turn 都会改变它）。
    pub fn tree_hex_before(&self, from_turn: usize) -> Option<String> {
        self.tree_hex_at_or_before(from_turn.saturating_sub(1))
    }

    /// E3 redo：rewind 时刻的**实时**影子 tree。与 `latest_tree_hex` 的差
    /// 别：begin 树是各 turn 开始时写的——最后一个 turn 的变更（含 shell
    /// 副作用）不在任何 begin 树里，redo 的前向恢复目标必须是 rewind 时刻
    /// 的实况，否则 redo 后文件停在「最后 turn 开始前」，与已回填的对话行
    /// 不一致。JSON 形态返回 None（redo 文件步诚实跳过，语义不变）。
    pub fn current_tree_hex(&self) -> Option<String> {
        let gb = self.git.as_ref()?;
        let mut repo = gb.repo.lock();
        match Self::write_current_tree(&mut repo) {
            Ok(h) => Some(h),
            Err(e) => {
                warn!("[checkpoint] 实时影子 tree 写入失败（redo 前向恢复降级到 begin 树）: {e}");
                None
            }
        }
    }

    /// 共享实现：`turn <= bound` 里最后一个带 tree 的。
    fn tree_hex_at_or_before(&self, bound: usize) -> Option<String> {
        let guard = self.inner.lock();
        let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
        if let Some(cur) = guard.cur.as_ref() {
            all.push(cur);
        }
        all.sort_by_key(|c| c.turn);
        all.into_iter()
            .filter(|c| c.turn <= bound)
            .filter_map(|c| c.tree.clone())
            .next_back()
    }

    /// E3：把工作区文件恢复到**指定影子 tree**（绕过 JSON 索引——
    /// `truncate_from` 可能已把对应 turn 从索引清掉，但影子库对象 v1 不
    /// gc，tree 仍然有效）。仅 git 形态；JSON 形态诚实报错（无 tree 可
    /// 言）。只覆盖工作区内（tree 覆盖范围）；工作区外的 hybrid 内容快照
    /// 不参与（redo 诚实边界）。
    pub fn restore_to_tree(&self, tree_hex: &str) -> Result<(Vec<String>, Vec<String>), String> {
        let Some(gb) = self.git.as_ref() else {
            return Err("影子库不可用（JSON 形态），无法按 tree 恢复文件".to_string());
        };
        let mut repo = gb.repo.lock();
        Ok(Self::git_restore(&self.root, &mut repo, tree_hex))
    }

    /// M3：工作区根（`session_file_diff` 读现盘文件用——checkpoint 的
    /// root 即 AgentLoop 装配时的工作区根）。
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// M3：某文件的最早基线锚——索引里（done 按 turn 序 + cur）第一个
    /// 声明过该文件的条目（`paths` 命中或 `files` 含该路径；后者兼容
    /// D2 之前没有 `paths` 字段的老 JSON 条目）。
    ///
    /// 诚实边界：索引是 store 全局的（跨会话共享）——多会话共改同一文件
    /// 时基线可能取自更早会话的 turn（消费方 session_file_diff 以此为
    /// 「最早已知 pre-edit 态」语义）。
    pub fn base_for_path(&self, path: &str) -> Option<CheckpointBase> {
        let guard = self.inner.lock();
        let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
        if let Some(cur) = guard.cur.as_ref() {
            all.push(cur);
        }
        all.sort_by_key(|c| c.turn);
        all.into_iter()
            .filter(|c| c.paths.iter().any(|p| p == path) || c.files.iter().any(|f| f.path == path))
            .map(|c| CheckpointBase {
                turn: c.turn,
                tree: c.tree.clone(),
                content: c
                    .files
                    .iter()
                    .find(|f| f.path == path)
                    .and_then(|f| f.content.clone()),
            })
            .next()
    }

    /// M3：从影子 tree 读单文件内容（git 形态专用）。文件不在 tree 里 →
    /// `Ok(None)`（基线时刻尚不存在=新增文件语义）；真实 git 错误 → Err。
    pub fn read_file_from_tree(
        &self,
        tree_hex: &str,
        path: &str,
    ) -> Result<Option<String>, String> {
        let Some(gb) = self.git.as_ref() else {
            return Err("影子库不可用（JSON 形态），无法按 tree 读文件".to_string());
        };
        let repo = gb.repo.lock();
        let oid = git2::Oid::from_str(tree_hex).map_err(|e| format!("tree hex 解析失败: {e}"))?;
        let tree = repo
            .find_tree(oid)
            .map_err(|e| format!("tree 查找失败: {e}"))?;
        match tree.get_path(std::path::Path::new(path)) {
            Ok(entry) => {
                let blob = repo
                    .find_blob(entry.id())
                    .map_err(|e| format!("blob 读取失败: {e}"))?;
                Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(format!("tree 内路径查找失败: {e}")),
        }
    }

    fn persist(&self, cp: &Checkpoint) {
        let Some(dir) = &self.dir else { return };
        let bytes = match serde_json::to_vec_pretty(cp) {
            Ok(b) => b,
            Err(_) => return,
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let path = dir.join(format!("turn-{}.json", cp.turn));
        let _ = std::fs::write(path, bytes);
    }

    /// Resolve `p` against the workspace root, rejecting traversal escapes.
    /// Restore must never write outside the workspace, even if a snapshot path is
    /// hostile or the project moved since the snapshot was taken.
    fn safe_path(&self, p: &str) -> Option<PathBuf> {
        if p.contains("..") {
            return None;
        }
        let abs = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            self.root.join(p)
        };
        Some(abs)
    }
}

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

// D2 (devtool-upgrade 阶段 5): git 影子库双形态回归（D4 验收）。
#[cfg(test)]
mod d2_tests;

// E3 (devtool-upgrade 阶段 5): tree 查询 + restore_to_tree（消息级回退
// redo 的文件原语）测试。
#[cfg(test)]
mod e3_tests;

// M3 (devtool-upgrade 阶段 5): base_for_path + read_file_from_tree（会话级
// diff 查看器的基线原语）测试。
#[cfg(test)]
mod m3_tests;
