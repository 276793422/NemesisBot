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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

/// P20（2026-09-25 能力扩展 WS7）：单路径「变更后指纹」——turn 收尾封印
/// （`seal_turn`）时该路径的**现盘**内容 sha256 hex。`hash == None` = 封印
/// 时刻文件已不在盘上（删除语义）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub path: String,
    pub hash: Option<String>,
}

/// P20：单条冲突——rewind 目标之后声明过该路径的 checkpoint 封印指纹与
/// 当前盘上内容不一致（文件在 checkpoint 之后被外部修改）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConflict {
    pub path: String,
    pub expected_hash: String,
    pub current_hash: String,
}

/// P20：冲突预检报告。`unchecked_paths` = 最后声明该路径的 turn 尚未封印
/// （trailing turn 进行中 / 崩溃未走到封印点）——诚实跳过不比对，绝不回退
/// 到更旧的 seal（旧 seal 是更早状态，会把 agent 自己的后续变更误报成
/// 外部修改）。
#[derive(Debug, Clone, Default)]
pub struct CheckpointConflictReport {
    pub conflicts: Vec<CheckpointConflict>,
    pub unchecked_paths: Vec<String>,
    /// R31（2026-10-23 会话回退波及面收窄）：回退区间内**其他会话**声明过
    /// 变更的路径。restore 的全树恢复会把它们一并碾掉，但它们不属于本次回
    /// 退的会话意图——与 `conflicts` 同为默认拒绝（force 显式强过）。单列而
    /// 不并入 `conflicts`：它们大概率指纹干净（封印态=现盘态），语义是
    /// 「波及并发会话」而非「外部修改」，消费方能向用户说明差别。
    pub other_session_paths: Vec<String>,
}

/// Anchors the pre-edit state of every distinct file touched during one user turn.
///
/// `paths`（D2 新增，serde default 兼容老 JSON）：本 turn 工具声明的变更路径
/// （preview_all 集合），供 picker/meta 展示。`tree`（D2 新增）：git 模式下
/// `begin` 时刻的影子库 tree hex；JSON 模式恒 None。
///
/// `after` / `sealed`（P20 新增，serde default 兼容老 JSON）：turn 收尾封印
/// 的「变更后指纹」集与封印标记——rewind 冲突预检的比对基线。
///
/// `session`（R31 新增，serde default 兼容老 JSON）：begin 该 turn 的会话
/// 键（`begin_for_session` 穿针）。turn 序号是 loop 全局的
/// （`turn_counter.fetch_add`），而 `sessions.rewind_to_message` 是会话级
/// API——无归属字段时全局截断/全树恢复会把并发会话的变更一并碾掉。None =
/// 旧条目/测试直构造——scope 判定按「同会话」处理（不可归属不惩罚，单会
/// 话行为逐字节不变）。
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
    #[serde(default)]
    pub after: Vec<FileFingerprint>,
    #[serde(default)]
    pub sealed: bool,
    #[serde(default)]
    pub session: Option<String>,
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

/// R3（2026-10-24 外评）：恢复动作的结构化报告——成功与失败分明，失败项
/// 带原因。此前 restore 家族只回 `(written, deleted)` 两 vec，写失败/删失败
/// 被 `is_ok()` 过滤静默吞掉，rewind 侧无从感知、无条件盖「applied」戳并
/// 摧毁 checkpoint（失败后无法重试）。消费方（rewind 编排）以
/// [`RestoreReport::ok`] 决定是否推进截断。
#[derive(Debug, Default, Clone)]
pub struct RestoreReport {
    pub written: Vec<String>,
    pub deleted: Vec<String>,
    /// 恢复失败项 `(path, 原因)`。整体性失败（如影子 tree 不可达、diff
    /// 失败）以 path=`"*"` 单条承载。非空 = 工作区处于部分恢复态，调用方
    /// 不得推进任何截断。
    pub failures: Vec<(String, String)>,
}

impl RestoreReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    fn merge(&mut self, other: RestoreReport) {
        self.written.extend(other.written);
        self.deleted.extend(other.deleted);
        self.failures.extend(other.failures);
    }
}

/// P4（交付门）：一次交付分支捕获的结果——可审分支的寻址面。
///
/// 影子库本无 commit 流（只有 tree 流）：capture = 写当前工作区 tree →
/// 以其为根建单个 commit → `refs/heads/nemesis/<branch>` force 指向它。
/// diff 与统计对 `(base_tree, tree)` 二元组计算，与 commit 链无关——
/// base None 时 diff 以 canonical 空树为基线（全部文件计新增）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeliveryBranchCapture {
    /// 完整 ref 名（`refs/heads/nemesis/<branch>`）。
    pub ref_name: String,
    /// 捕获 commit 的 sha（可审交付点）。
    pub commit: String,
    /// 捕获时刻工作区 tree hex。
    pub tree: String,
    /// diff 基线 tree hex（调用方未指定时 = canonical 空树）。
    pub base_tree: String,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    /// unified diff（相对 `base_tree`）；超 `PATCH_MAX_BYTES` 截断。
    pub patch: String,
    pub patch_truncated: bool,
    /// 逐文件变更清单（typed 证据块 ②，2026-10-19）：从 `(base_tree, tree)`
    /// diff 确定性派生（**不依赖 agent 配合**），`sha256` = 交付 tree 中该
    /// blob 内容的摘要——与 commit/tree 同源定格，评审侧可对 patch 重放核
    /// 验。R5（2026-10-20 复评）：含全部变更类型（add/modify/delete）——
    /// 删除条目 `kind=delete` + `old_sha256`（变更前 blob 摘要），评审闸可
    /// 据此核验「删除确实发生」而非零证据放行。空 = 无变更或派生降级（**恒
    /// 序列化**——空清单与「字段缺席」语义不同：前者是派生事实，后者无法
    /// 区分旧 payload 与证据缺失）。
    pub files: Vec<DeliveryFileEntry>,
    /// files 清单截断位（超 [`FILES_MAX`] 诚实注记；只在 true 时序列化）。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub files_truncated: bool,
}

/// R5：交付清单条目变更类型（serde 小写序列化 `add`/`modify`/`delete`）。
/// rename 在 git delta 层呈现为 delete+add 对，不造第四值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryFileKind {
    Add,
    Modify,
    Delete,
}

/// 交付变更清单单条目（typed 证据块 ②）：相对路径 + 变更类型 + blob 内容
/// sha256 hex。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeliveryFileEntry {
    /// 交付根内的相对路径（forward slash 形态；派生自 git delta）。
    pub path: String,
    /// 变更类型（R5）。
    pub kind: DeliveryFileKind,
    /// 交付 tree 中该 blob 内容的 sha256 hex（64 字符）；delete 条目为空串
    /// 不序列化（无新内容可哈希——变更前内容见 `old_sha256`）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    /// 变更前 blob 内容 sha256 hex（`modify`/`delete` 携带；`add` 为空串
    /// 不序列化——新增无「变更前」）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub old_sha256: String,
}

/// 逐文件清单上限（防异常巨大交付撑爆 wire payload；超限诚实截断 +
/// `files_truncated` 注记，与 patch 截断同契约形态）。
pub const FILES_MAX: usize = 500;

/// 交付 patch 的字节上限（截断保护——异常巨大的交付 diff 不进账本
/// payload；完整 patch 由调用方落盘，账本只引用路径）。
pub const PATCH_MAX_BYTES: usize = 1_000_000;

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
    /// R4（2026-10-24 第四次外评）：per-session 进行中 turn 桶，键 = 会话键
    /// （None = 旧全局语义：`begin()`/`snapshot()` 兼容路径与存量测试）。
    /// 旧单 `Option<Checkpoint>` 槽被后 begin 的并发会话整体收编——前一
    /// 会话的 snapshot 全落进后一会话名下的 checkpoint（跨会话串档，
    /// rewind 波及面误判）。桶化后各会话各归各位。
    cur: HashMap<Option<String>, Checkpoint>,
    /// per-session 首 touch 去重（与 cur 桶同键同生命周期）。
    seen: HashMap<Option<String>, HashSet<String>>,
}

/// git2 `Repository` is Send but !Sync — Mutex 使其可跨 await 共享。
/// 锁纪律：repo 锁与 inner 锁绝不互相嵌套持有（先 repo 操作、放锁、再
/// inner，反之亦然），杜绝双锁排序死锁。
#[cfg(feature = "checkpoint-git")]
struct GitBackend {
    repo: Mutex<git2::Repository>,
}

/// checkpoint-git 关：git2（libgit2 ~1.5MB）不编译。空形态**刻意保留**
/// `git: Option<GitBackend>` 字段形与全部 `self.git` 读点的编译面；
/// `try_git_backend` 恒 None → JSON 快照回落（D4）自然接管，行为与
/// 「无 .git」完全一致（逐 turn JSON 文件、rewind/undo 照常）。
#[cfg(not(feature = "checkpoint-git"))]
struct GitBackend {}

/// Holds a session's checkpoints in memory and, when `dir` is set, persists one
/// JSON file per turn under it. All methods are safe for concurrent use.
pub struct CheckpointStore {
    dir: Option<PathBuf>,
    root: PathBuf,
    git: Option<GitBackend>,
    inner: Mutex<Inner>,
}

/// R31：scope 判定单一真相源——条目会话归属是否属于「其他会话」。
/// `rewind_session = None`（不收窄）恒 false；条目归属 None（旧条目/测试直
/// 构造，不可归属）恒 false——不可归属不惩罚，按同会话处理，单会话与存量
/// 数据行为逐字节不变。
fn is_other_session(entry_session: &Option<String>, rewind_session: Option<&str>) -> bool {
    match rewind_session {
        None => false,
        Some(rs) => entry_session.as_deref().is_some_and(|s| s != rs),
    }
}

/// R31：落盘 turn-N.json 是否属于其他会话（scoped 截断的磁盘镜像判定，
/// 与内存 retain 同源语义）。解析失败 = 不可归属 = false（按本会话删——
/// load 本也跳过解析失败文件，留着无法归属的壳没有消费面）。
fn persisted_checkpoint_belongs_to_other_session(
    path: &std::path::Path,
    rewind_session: Option<&str>,
) -> bool {
    let Some(rs) = rewind_session else {
        return false;
    };
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Checkpoint>(&bytes)
            .ok()
            .is_some_and(|c| c.session.as_deref().is_some_and(|s| s != rs)),
        Err(_) => false,
    }
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
        // 默认影子库位置：logs 家族（主 loop 布局，`{root}/logs/checkpoints.git`）。
        let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
        Self::build(dir, root, shadow)
    }

    /// L6++（2026-09-08）项目 loop 工厂用：影子库位置显式指定。项目 root =
    /// 用户项目目录时，默认派生 `{root}/logs/checkpoints.git` 会把影子库落
    /// 进用户项目目录（R7 禁止）；override 落主 workspace
    /// `logs/project_checkpoints/{pid}/repo.git`，项目目录零污染。其余行为
    /// 与 `new` 完全一致。
    pub fn new_with_shadow(dir: Option<PathBuf>, root: PathBuf, shadow: PathBuf) -> Self {
        Self::build(dir, root, shadow)
    }

    fn build(dir: Option<PathBuf>, root: PathBuf, shadow: PathBuf) -> Self {
        let git = Self::try_git_backend(&root, &shadow);
        let store = Self {
            dir,
            root,
            git,
            inner: Mutex::new(Inner {
                done: Vec::new(),
                cur: HashMap::new(),
                seen: HashMap::new(),
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

    #[cfg(feature = "checkpoint-git")]
    fn try_git_backend(root: &Path, shadow: &Path) -> Option<GitBackend> {
        let real_git = root.join(".git");
        // 只认目录形态的 .git；文件形态（linked worktree / submodule）→ JSON 回落。
        if !real_git.is_dir() {
            return None;
        }
        let result = if shadow.exists() {
            Self::open_shadow(shadow, root)
        } else {
            Self::init_shadow(shadow, root)
        };
        match result {
            Ok(repo) => {
                // P20（2026-09-25 能力扩展）：启动自愈——清扫崩溃残留的
                // temp 对象 / 半成品锁文件（幂等，无残留 = 0 清扫不吭声）。
                let healed = Self::heal_shadow_repo(shadow);
                if healed > 0 {
                    warn!("[checkpoint] 影子库自愈：清扫崩溃残留文件 {healed} 个");
                }
                if let Err(e) = Self::ensure_alternates(shadow, root) {
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

    /// checkpoint-git 关：恒 None（JSON 快照回落，D4）。
    #[cfg(not(feature = "checkpoint-git"))]
    fn try_git_backend(_root: &Path, _shadow: &Path) -> Option<GitBackend> {
        None
    }

    #[cfg(feature = "checkpoint-git")]
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

    #[cfg(feature = "checkpoint-git")]
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
    #[cfg(feature = "checkpoint-git")]
    fn ensure_workdir(repo: &git2::Repository, root: &Path) -> Result<(), String> {
        let ok = repo.workdir().is_some_and(|w| w.starts_with(root));
        if !ok {
            repo.set_workdir(root, false).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// `objects/info/alternates` 指向真实仓库 objects → 已提交 blob 零拷贝
    /// 复用。幂等：内容一致不重写。
    #[cfg(feature = "checkpoint-git")]
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

    /// P20（2026-09-25 能力扩展）：影子库自愈——清扫上次进程崩溃残留的
    /// temp 对象 / 半成品锁文件（git 对象写入走 tmp→rename 原子替换，残留
    /// 形态 = `tmp_obj_*` / `tmp_pack_*` / 各类 `*.lock`）。幂等：重复启动
    /// 或无残留都安全（返回清扫条数）。只删已知安全形态，绝不触碰
    /// refs / objects 本体 / packed-refs / alternates。
    #[cfg(feature = "checkpoint-git")]
    pub fn heal_shadow_repo(shadow: &Path) -> usize {
        let mut removed = 0usize;
        let mut stack = vec![shadow.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for ent in entries.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = ent.file_name().to_string_lossy().to_string();
                let stale = name.starts_with("tmp_obj_")
                    || name.starts_with("tmp_pack_")
                    || name.ends_with(".lock");
                // BUG-20261025-35 同族：stale 残留（尤其 .lock）清不掉会让
                // git 操作持续报锁——同样走韧性重试，瞬时句柄不致自愈失效
                //（装配期一次性清扫，重试窗口无碍）。
                if stale && nemesis_utils::fsutil::remove_file_retry(&p).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
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
    #[cfg(feature = "checkpoint-git")]
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
    /// 路径集，然后 checkout_tree（按路径）+ 逐文件删除。R3：整体性失败
    /// （tree 写入/解析/diff）与逐文件失败都进 `failures`（整体失败以
    /// path=`"*"` 承载），不再静默返空。
    #[cfg(feature = "checkpoint-git")]
    fn git_restore(root: &Path, repo: &mut git2::Repository, target_hex: &str) -> RestoreReport {
        let mut report = RestoreReport::default();
        // 现状 tree（同样跳过运行时属主目录）。先做可变操作（index 写入），
        // 再拿 immutable 的 Tree 句柄——git2 的 Tree 借用 repo，顺序不能反。
        let cur_hex = match Self::write_current_tree(repo) {
            Ok(h) => h,
            Err(e) => {
                warn!("[checkpoint] 现状 tree 写入失败，放弃 git restore: {e}");
                report
                    .failures
                    .push(("*".into(), format!("现状 tree 写入失败: {e}")));
                return report;
            }
        };
        let target_oid = match git2::Oid::from_str(target_hex) {
            Ok(o) => o,
            Err(e) => {
                warn!("[checkpoint] tree oid 解析失败 {target_hex}: {e}");
                report
                    .failures
                    .push(("*".into(), format!("tree oid 解析失败: {e}")));
                return report;
            }
        };
        let target_tree = match repo.find_tree(target_oid) {
            Ok(t) => t,
            Err(e) => {
                // 影子库对象缺失（如真实仓库 gc 掉了 alternates 引用的对象）。
                warn!("[checkpoint] 影子 tree 不可达 {target_hex}: {e}");
                report
                    .failures
                    .push(("*".into(), format!("影子 tree 不可达: {e}")));
                return report;
            }
        };
        let cur_oid = match git2::Oid::from_str(&cur_hex) {
            Ok(o) => o,
            Err(e) => {
                report
                    .failures
                    .push(("*".into(), format!("现状 oid 解析失败: {e}")));
                return report;
            }
        };
        let cur_tree = match repo.find_tree(cur_oid) {
            Ok(t) => t,
            Err(e) => {
                report
                    .failures
                    .push(("*".into(), format!("现状 tree 不可达: {e}")));
                return report;
            }
        };

        let diff = match repo.diff_tree_to_tree(Some(&target_tree), Some(&cur_tree), None) {
            Ok(d) => d,
            Err(e) => {
                warn!("[checkpoint] tree diff 失败: {e}");
                report
                    .failures
                    .push(("*".into(), format!("tree diff 失败: {e}")));
                return report;
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
        // 重哈希，正确性不受影响）。R3：checkout 失败 = 全部待恢复路径记
        // failures（一次调用整体失败，无部分成功可言）。
        if !to_checkout.is_empty() {
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.force().update_index(false);
            for p in &to_checkout {
                cb.path(p.as_str());
            }
            match repo.checkout_tree(target_tree.as_object(), Some(&mut cb)) {
                Ok(()) => report.written = to_checkout,
                Err(e) => {
                    warn!("[checkpoint] checkout_tree 失败（工作区保持现状）: {e}");
                    report.failures.extend(
                        to_checkout
                            .into_iter()
                            .map(|p| (p, format!("checkout 失败: {e}"))),
                    );
                }
            }
        }

        // 删除多余文件（target 没有而现状有的——含 shell 产物）。经 safe_path
        // 语义防逃逸（git 相对路径理论上不含 ..，防御性保留）。R3：删除失败
        // 进 failures（NotFound = 快照语义已达成，不计失败）。
        for rel in to_remove {
            if rel.split(['/', '\\']).any(|seg| seg == "..") {
                continue;
            }
            let abs = root.join(&rel);
            match std::fs::remove_file(&abs) {
                Ok(()) => report.deleted.push(rel),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    warn!("[checkpoint] 删除 {rel} 失败: {e}");
                    report.failures.push((rel, format!("删除失败: {e}")));
                }
            }
        }
        report
    }

    // ------------------------------------------------------------------
    // P4 交付门：交付分支捕获 / patch 重算 / 列举（git 形态专用）
    // 随 checkpoint-git 门整族裁剪——全族直接摸 git2（与上方 try_git_backend
    // 等逐函数门同纪律；漏门 = minimal-iot/desktop 形态 E0433/E0599 实录）。
    // ------------------------------------------------------------------

    /// 交付分支名校验：`[A-Za-z0-9._/-]`、无 `..`、无前导 `-`、≤80 字符。
    // 纯字符串校验不依赖 git2，但唯一消费方是交付门 git 形态——随门
    // 一起 cfg（避免 checkpoint-git 关时 dead_code 警告）。
    #[cfg(feature = "checkpoint-git")]
    fn validate_branch_name(branch: &str) -> Result<(), String> {
        if branch.is_empty() {
            return Err("交付分支名为空".to_string());
        }
        if branch.len() > 80 {
            return Err(format!(
                "交付分支名过长（{} 字符 > 80）：{branch}",
                branch.len()
            ));
        }
        if branch.starts_with('-') || branch.contains("..") {
            return Err(format!("交付分支名含危险前缀/段（- 或 ..）：{branch}"));
        }
        if !branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        {
            return Err(format!(
                "交付分支名含非法字符（只允许 A-Za-z0-9._/-）：{branch}"
            ));
        }
        Ok(())
    }

    #[cfg(feature = "checkpoint-git")]
    fn empty_tree_hex(repo: &git2::Repository) -> Result<String, String> {
        let tb = repo
            .treebuilder(None)
            .map_err(|e| format!("影子库空树构造失败: {e}"))?;
        Ok(tb
            .write()
            .map_err(|e| format!("影子库空树写入失败: {e}"))?
            .to_string())
    }

    /// `(base_tree, tree)` → unified diff + 统计（P4 capture 与 patch 重算
    /// 共用）。base None = canonical 空树（全部文件计新增）。
    #[cfg(feature = "checkpoint-git")]
    fn diff_patch(
        repo: &git2::Repository,
        base_hex: &str,
        tree_hex: &str,
    ) -> Result<(String, usize, usize, usize, bool), String> {
        let base_oid =
            git2::Oid::from_str(base_hex).map_err(|e| format!("base tree hex 解析失败: {e}"))?;
        let tree_oid =
            git2::Oid::from_str(tree_hex).map_err(|e| format!("tree hex 解析失败: {e}"))?;
        let base_tree = repo
            .find_tree(base_oid)
            .map_err(|e| format!("影子 base tree 不可达 {base_hex}: {e}"))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| format!("影子 tree 不可达 {tree_hex}: {e}"))?;
        let diff = repo
            .diff_tree_to_tree(Some(&base_tree), Some(&tree), None)
            .map_err(|e| format!("影子 tree diff 失败: {e}"))?;
        let stats = diff
            .stats()
            .map_err(|e| format!("影子 diff 统计失败: {e}"))?;
        let mut buf = Vec::new();
        // git2 0.21 无 Diff::format——patch 输出走 print 回调。⚠ line.content()
        // **不含** +/-/空格 前缀字节（context/add/del 三类须手动补 origin 字符，
        // FileHeader/HunkHeader 自带完整换行）——漏补产出的 patch 无法被
        // Diff::from_buffer 回读（"invalid patch hunk"），P3 发布链实证。
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            match line.origin() {
                '+' | '-' | ' ' => buf.push(line.origin() as u8),
                _ => {}
            }
            buf.extend_from_slice(line.content());
            true
        })
        .map_err(|e| format!("影子 diff 格式化失败: {e}"))?;
        let truncated = buf.len() > PATCH_MAX_BYTES;
        if truncated {
            buf.truncate(PATCH_MAX_BYTES);
        }
        let patch = String::from_utf8_lossy(&buf).into_owned();
        Ok((
            patch,
            stats.files_changed(),
            stats.insertions(),
            stats.deletions(),
            truncated,
        ))
    }

    /// 逐文件变更清单派生（typed 证据块 ②）：同一 `(base, tree)` diff 的
    /// delta 枚举——新增/修改取 `new_file` 的 blob 内容 sha256；修改/删除
    /// 附 `old_file` 摘要（R5：删除条目 `old_sha256` 承载证据，不再整类
    /// 跳过——删除也是交付事实）。rename 在 delta 层呈 delete+add 对，
    /// typechange 按 modify 记（同路径内容形态变更）。超 [`FILES_MAX`]
    /// 截断（诚实注记位）。与 [`Self::diff_patch`] 分开各跑一次 diff
    /// （影子库 task 收口一次性成本，换来两函数单一职责）。
    #[cfg(feature = "checkpoint-git")]
    fn diff_file_entries(
        repo: &git2::Repository,
        base_hex: &str,
        tree_hex: &str,
    ) -> Result<(Vec<DeliveryFileEntry>, bool), String> {
        use git2::Delta;
        let base_oid =
            git2::Oid::from_str(base_hex).map_err(|e| format!("base tree hex 解析失败: {e}"))?;
        let tree_oid =
            git2::Oid::from_str(tree_hex).map_err(|e| format!("tree hex 解析失败: {e}"))?;
        let base_tree = repo
            .find_tree(base_oid)
            .map_err(|e| format!("影子 base tree 不可达 {base_hex}: {e}"))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| format!("影子 tree 不可达 {tree_hex}: {e}"))?;
        let diff = repo
            .diff_tree_to_tree(Some(&base_tree), Some(&tree), None)
            .map_err(|e| format!("影子 tree diff 失败: {e}"))?;
        let mut files = Vec::new();
        for delta in diff.deltas() {
            match delta.status() {
                Delta::Added | Delta::Modified | Delta::Typechange | Delta::Deleted => {}
                // Unmodified/Conflicted/Unreadable 不应出现在 tree-to-tree
                // diff；保守跳过（与旧行为一致，不造第五类）。
                _ => continue,
            }
            let new_oid = delta.new_file().id();
            let old_oid = delta.old_file().id();
            let (kind, sha256, old_sha256) = match delta.status() {
                Delta::Added => {
                    // add 无变更前；new blob 必非零。
                    let sha = Self::blob_sha256(repo, new_oid)?;
                    (DeliveryFileKind::Add, sha, String::new())
                }
                Delta::Deleted => {
                    // delete 无新内容可哈希；old blob 承载证据。
                    let old_sha = Self::blob_sha256(repo, old_oid)?;
                    (DeliveryFileKind::Delete, String::new(), old_sha)
                }
                // modify/typechange：两侧都有内容。
                _ => {
                    let sha = Self::blob_sha256(repo, new_oid)?;
                    let old_sha = Self::blob_sha256(repo, old_oid)?;
                    (DeliveryFileKind::Modify, sha, old_sha)
                }
            };
            // 路径取两侧中在场的一侧（delete 只有 old，add 只有 new）。
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .ok_or_else(|| format!("交付清单 delta 无路径（{kind:?}）"))?;
            files.push(DeliveryFileEntry {
                path,
                kind,
                sha256,
                old_sha256,
            });
        }
        let truncated = files.len() > FILES_MAX;
        if truncated {
            files.truncate(FILES_MAX);
        }
        Ok((files, truncated))
    }

    /// blob 内容 sha256 hex（交付清单摘要单一来源；oid 零值调用方错误）。
    #[cfg(feature = "checkpoint-git")]
    fn blob_sha256(repo: &git2::Repository, oid: git2::Oid) -> Result<String, String> {
        let blob = repo
            .find_blob(oid)
            .map_err(|e| format!("交付清单 blob 不可达 {oid}: {e}"))?;
        let mut hasher = Sha256::new();
        hasher.update(blob.content());
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// P4 交付门：把当前工作区捕获为**可审交付分支**（git 形态专用）。
    ///
    /// - `base_tree`：diff 基线（通常传任务开始时的影子 tree——
    ///   `checkpoint_store().current_tree_hex()` 在 usage_before 旁记的
    ///   值）；`None` = 空树基线（全部文件计新增）。
    /// - `branch`：短名（如 `run-<task_id>`）；完整 ref =
    ///   `refs/heads/nemesis/<branch>`。同名重捕获 force 覆盖（调用方
    ///   用唯一名，重放场景覆盖同 id 的旧捕获是安全语义）。
    /// - JSON 形态（工作区无真实 `.git`）诚实 Err——可审分支只有在有
    ///   真实 .git 可合入时才有行动意义，不为无 .git 工作区造 standalone
    ///   影子（构造期后端选型契约不变）。
    #[cfg(feature = "checkpoint-git")]
    pub fn capture_delivery_branch(
        &self,
        base_tree: Option<&str>,
        branch: &str,
    ) -> Result<DeliveryBranchCapture, String> {
        Self::validate_branch_name(branch)?;
        let gb = self
            .git
            .as_ref()
            .ok_or_else(|| "影子库不可用（JSON 形态），无法捕获交付分支".to_string())?;
        let mut repo = gb.repo.lock();
        // 先做可变操作（index 写入 + commit + ref），再取 immutable 句柄——
        // git2 的 Tree/Diff 借用 repo，顺序不能反（同 git_restore 纪律）。
        let tree_hex = Self::write_current_tree(&mut repo)?;
        let base_hex = match base_tree {
            Some(h) => {
                // 校验可解析 + 影子可达（写坏 hex 提前响亮失败，不留半截 ref）。
                let oid =
                    git2::Oid::from_str(h).map_err(|e| format!("base tree hex 解析失败: {e}"))?;
                repo.find_tree(oid)
                    .map_err(|e| format!("base tree 不可达 {h}: {e}"))?;
                h.to_string()
            }
            None => Self::empty_tree_hex(&repo)?,
        };
        let (patch, files_changed, insertions, deletions, patch_truncated) =
            Self::diff_patch(&repo, &base_hex, &tree_hex)?;

        let tree_oid =
            git2::Oid::from_str(&tree_hex).map_err(|e| format!("tree hex 解析失败: {e}"))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| format!("影子 tree 不可达 {tree_hex}: {e}"))?;
        let sig = git2::Signature::now("nemesisbot", "nemesisbot@local")
            .map_err(|e| format!("影子 commit 签名构造失败: {e}"))?;
        let ref_name = format!("refs/heads/nemesis/{branch}");
        // commit（root commit——影子库无 commit 链，单点快照语义）。不带
        // ref 名建 commit（带 ref 名时 git2 校验 first-parent，重捕获同名
        // 分支会炸），ref 由下一步 force 写。
        let commit_oid = repo
            .commit(
                None,
                &sig,
                &sig,
                &format!("nemesis delivery: {branch}"),
                &tree,
                &[],
            )
            .map_err(|e| format!("影子交付 commit 写入失败: {e}"))?;
        repo.reference(&ref_name, commit_oid, true, "nemesis delivery capture")
            .map_err(|e| format!("影子交付 ref 写入失败 {ref_name}: {e}"))?;
        // typed 证据块 ②：逐文件清单派生。失败 = warn 降级空清单（files
        // 缺席是合法形态，验证侧 strict off 时放行）——不因增强字段炸掉
        // 整个 capture（commit/tree/patch 主证据面优先保住）。
        let (files, files_truncated) = match Self::diff_file_entries(&repo, &base_hex, &tree_hex) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    branch = %branch,
                    error = %e,
                    "[Checkpoint] 交付清单派生失败（files 块缺席降级，commit/tree/patch 主证据不受影响）"
                );
                (Vec::new(), false)
            }
        };
        Ok(DeliveryBranchCapture {
            ref_name,
            commit: commit_oid.to_string(),
            tree: tree_hex,
            base_tree: base_hex,
            files_changed,
            insertions,
            deletions,
            patch,
            patch_truncated,
            files,
            files_truncated,
        })
    }

    /// P4 交付门：从交付分支重算 patch（不重写工作区——审计/补拉查看面）。
    /// 返回 `(patch, truncated)`；分支不存在 = Err。
    #[cfg(feature = "checkpoint-git")]
    pub fn delivery_branch_patch(
        &self,
        branch: &str,
        base_tree: Option<&str>,
    ) -> Result<(String, bool), String> {
        Self::validate_branch_name(branch)?;
        let gb = self
            .git
            .as_ref()
            .ok_or_else(|| "影子库不可用（JSON 形态），无法读取交付分支".to_string())?;
        let repo = gb.repo.lock();
        let ref_name = format!("nemesis/{branch}");
        let commit = repo
            .find_branch(&ref_name, git2::BranchType::Local)
            .map_err(|e| format!("交付分支不存在 refs/heads/{ref_name}: {e}"))?
            .get()
            .peel_to_commit()
            .map_err(|e| format!("交付分支 {ref_name} 无法定位 commit: {e}"))?;
        let tree_hex = commit.tree_id().to_string();
        let base_hex = match base_tree {
            Some(h) => h.to_string(),
            None => Self::empty_tree_hex(&repo)?,
        };
        let (patch, _, _, _, truncated) = Self::diff_patch(&repo, &base_hex, &tree_hex)?;
        Ok((patch, truncated))
    }

    /// P4 交付门：列举全部交付分支（`refs/heads/nemesis/*` 短名，字典序）。
    #[cfg(feature = "checkpoint-git")]
    pub fn delivery_branch_refs(&self) -> Result<Vec<String>, String> {
        let gb = self
            .git
            .as_ref()
            .ok_or_else(|| "影子库不可用（JSON 形态），无交付分支".to_string())?;
        let repo = gb.repo.lock();
        let prefix = "refs/heads/nemesis/";
        let mut names: Vec<String> = repo
            .references()
            .map_err(|e| format!("影子库引用枚举失败: {e}"))?
            .names()
            .flatten()
            .filter_map(|n| n.strip_prefix(prefix).map(|s| s.to_string()))
            .collect();
        names.sort();
        Ok(names)
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
            } else {
                // BUG-20261025-43：损坏镜像不再静默吞——诚实留痕（崩溃撕裂/
                // 外部损坏的第一个可见信号），与 auth.json / 分享注册表同族
                // 口径；该 turn 的回退条目/封印指纹丢失由此可查。
                warn!(
                    file = %path.display(),
                    "[checkpoint] turn 镜像解析失败，跳过（该 turn 的回退条目丢失）"
                );
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
        self.begin_for_session(turn, prompt, None);
    }

    /// R31：带会话归属的 begin。`session_key` 落进 checkpoint 条目，会话级
    /// 回退（`truncate_from_scoped` / `conflict_scan_scoped`）据此把波及面
    /// 收窄到本会话的 turn。`begin`（无会话）委托 None——旧全局语义，测试
    /// 与兼容路径沿用。
    pub fn begin_for_session(
        &self,
        turn: usize,
        prompt: impl Into<String>,
        session_key: Option<&str>,
    ) {
        // R4：桶键——本会话的进行中 turn 归属（None = 旧全局语义）。
        let bucket: Option<String> = session_key.map(str::to_string);

        // P20 兜底封印：**本会话**上一 turn 若因崩溃未走到 process_admitted
        // 的收尾封印点，在开启新 turn 前补封（幂等——已封印的 turn 直接跳
        // 过）。只补封本会话桶：其他会话的进行中 turn 由其自身收尾/下次
        // begin 兜底，提前代封会把别人 turn 中途的盘面钉成 after 指纹
        // （其 turn 后半段的合法写入会被冲突预检误报为外部修改）。
        let fallback_seal = {
            let guard = self.inner.lock();
            guard.cur.get(&bucket).filter(|c| !c.sealed).map(|c| c.turn)
        };
        if let Some(t) = fallback_seal {
            self.seal_turn(t);
        }

        let prompt = prompt.into();

        // git track 先行（repo 锁独立短区间，不与 inner 锁嵌套）。
        #[cfg(feature = "checkpoint-git")]
        let tree_hex = match self.git.as_ref() {
            Some(gb) => {
                let mut repo = gb.repo.lock();
                match Self::write_current_tree(&mut repo) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        warn!(
                            "[checkpoint] turn {turn} 影子 tree 写入失败（本 turn 不可 git 回滚）: {e}"
                        );
                        // M2：写入失败面可观测——影子 tree 失败 = 该 turn 的
                        // git 回滚能力丢失，用户需要知道。
                        nemesis_utils::degraded::report(
                            "checkpoint_tree",
                            format!("turn {turn} 影子 tree 写入失败（本 turn 不可 git 回滚）: {e}"),
                        );
                        None
                    }
                }
            }
            None => None,
        };
        #[cfg(not(feature = "checkpoint-git"))]
        let tree_hex: Option<String> = None;

        let cp = Checkpoint {
            turn,
            time: chrono::Local::now().to_rfc3339(),
            prompt,
            files: Vec::new(),
            paths: Vec::new(),
            tree: tree_hex,
            after: Vec::new(),
            sealed: false,
            session: session_key.map(str::to_string),
        };

        let mut guard = self.inner.lock();
        // R4：只 finalize 本会话桶的上一个 cur（其他会话的进行中 turn 原位
        // 不动——旧单槽会被整体收编成跨会话串档）。
        let prev = guard.cur.remove(&bucket);
        if let Some(prev) = &prev {
            guard.done.push(prev.clone());
        }
        // 落盘纪律（git 模式）：tree 与上一 turn 一致 → 内容无变化，不落盘。
        let prev_tree = prev
            .as_ref()
            .and_then(|p| p.tree.clone())
            .or_else(|| guard.done.last().and_then(|c| c.tree.clone()));
        let changed_since_prev = cp.tree.is_some() && (prev_tree.is_none() || prev_tree != cp.tree);
        guard.cur.insert(bucket.clone(), cp.clone());
        // 首 touch 去重只重置本会话桶（新 turn 新去重集；其他会话不动）。
        guard.seen.insert(bucket, HashSet::new());
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
        self.snapshot_for_session(change, None).await;
    }

    /// R4：会话归属版 snapshot——写入落进**本会话**的进行中 turn 桶。
    /// `session_key = None` = 旧全局语义（`begin()` 兼容路径、存量测试）。
    /// 生产调用点（tool_dispatch preview 阶段）传 `Some(&ctx.session_key)`，
    /// 与 `admission.rs` 的 `begin_for_session` 同键——并发会话各归各位。
    pub async fn snapshot_for_session(&self, change: &FileChange, session_key: Option<&str>) {
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

        let bucket = session_key.map(str::to_string);
        let mut guard = self.inner.lock();
        // 借用拆分：cur / seen 是不相交字段，可同时可变借用。
        let Inner { cur, seen, .. } = &mut *guard;
        let Some(cp) = cur.get_mut(&bucket) else {
            return;
        };
        let seen = seen.entry(bucket).or_default();
        if !seen.insert(change.path.clone()) {
            return; // already snapshotted this turn
        }
        cp.files.push(FileSnap {
            path: change.path.clone(),
            content,
        });
        cp.paths.push(change.path.clone());
        let snap = cp.clone();
        drop(guard);
        self.persist(&snap);
    }

    // ------------------------------------------------------------------
    // P20（2026-09-25 能力扩展 WS7）：turn 收尾封印 + rewind 冲突预检
    // ------------------------------------------------------------------

    /// turn 收尾封印——对本 turn 声明过的每个变更路径读**现盘**内容
    /// sha256，作为「变更后指纹」记进该 turn 的 checkpoint（rewind 冲突
    /// 预检的比对基线）。幂等（已封印直接返回）；turn 不在索引中 = no-op。
    ///
    /// 两段锁纪律：锁内取路径集与幂等判定 → 放锁逐路径哈希 → 锁内复核
    /// 回填 + persist。哈希期间的窗口诚实边界：封印与工具执行非原子，极
    /// 端并发下封的是「封印时刻」盘面——冲突预检本就是防外部修改的护栏，
    /// 不是强一致锁。调用点：process_admitted 收尾（admission.rs）+ begin
    /// 顶部崩溃兜底。
    pub fn seal_turn(&self, turn: usize) {
        // 段一：定位 checkpoint + 收集待封印路径（幂等检查）。R4：turn 序号
        // loop 全局唯一——done 优先，其次遍历所有会话的 cur 桶。
        let (paths, cur_bucket) = {
            let guard = self.inner.lock();
            if let Some(cp) = guard.done.iter().find(|c| c.turn == turn) {
                if cp.sealed {
                    return; // 幂等
                }
                (Self::declared_paths(cp), None)
            } else if let Some((k, cp)) = guard.cur.iter().find(|(_, c)| c.turn == turn) {
                if cp.sealed {
                    return; // 幂等
                }
                (Self::declared_paths(cp), Some(k.clone()))
            } else {
                return;
            }
        };

        // 段二（锁外）：逐路径读现盘哈希。
        let hashed: Vec<FileFingerprint> = paths
            .into_iter()
            .map(|p| FileFingerprint {
                hash: self.hash_on_disk(&p),
                path: p,
            })
            .collect();

        // 段三：锁内回填 + 落盘（重写 turn-N.json，崩溃后重载仍带指纹）。
        // 落盘纪律保留（2026-08-30「空 turn 不留壳」）：无声明路径且本就没
        // 落过盘的空 turn 只改内存态（sealed 标记），不凭封印凭空造出
        // turn-N.json——无指纹可失，冲突预检也不需要它。
        let (snap, should_persist) = {
            let mut guard = self.inner.lock();
            let target = if let Some(k) = &cur_bucket {
                guard.cur.get_mut(k)
            } else {
                guard.done.iter_mut().find(|c| c.turn == turn)
            };
            let Some(cp) = target else { return };
            cp.after = hashed;
            cp.sealed = true;
            let snap = cp.clone();
            let already_on_disk = self
                .dir
                .as_ref()
                .map(|d| d.join(format!("turn-{}.json", snap.turn)).exists())
                .unwrap_or(false);
            let has_paths = !snap.paths.is_empty();
            (snap, already_on_disk || has_paths)
        };
        if should_persist {
            self.persist(&snap);
        }
    }

    /// R4：checkpoint 的声明路径集（paths 空时回退 files 的路径——老 JSON
    /// 无 paths 字段）。seal/conflict/list 多处共用，单一真相源。
    fn declared_paths(cp: &Checkpoint) -> Vec<String> {
        if cp.paths.is_empty() {
            cp.files.iter().map(|f| f.path.clone()).collect()
        } else {
            cp.paths.clone()
        }
    }

    /// P20：路径的现盘内容 sha256 hex（safe_path 语义解析，拒 `..` 逃逸）。
    /// 文件不存在 / 读取失败 → None（=「封印时刻不在盘上」语义，与指纹
    /// None 对齐比较）。
    fn hash_on_disk(&self, path: &str) -> Option<String> {
        let abs = self.safe_path(path)?;
        let bytes = std::fs::read(abs).ok()?;
        let mut h = Sha256::new();
        h.update(&bytes);
        Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    /// P20：rewind 冲突预检——每路径只认「turn >= from_turn 里**最后声明
    /// 它**的 checkpoint」的封印指纹与现盘内容比对（防假阳性：更旧的 seal
    /// 是更早状态，会把 agent 自己的后续变更误报成外部修改）。最后声明
    /// turn 未封印（trailing turn 进行中）→ unchecked 诚实跳过。
    ///
    /// 判定细则：现盘 hash 与封印 hash 不一致 = 冲突（含「封印时刻在盘、
    /// 现已不在」——外部删除也算修改，force 可过）；一致 / 封印即不在盘
    /// （None vs None）= 干净。
    ///
    /// git 形态补充覆盖（2026-09-26 复查 F6）：restore_code 实际做
    /// target..现状**全树 diff**——shell 副作用等未声明文件同样被恢复/删
    /// 除，预检若只看声明路径就对这部分失明。增量判定（与声明路径同语义
    /// 粒度，参照物 = 最新 begin tree）：候选路径在 target 与 reference 间
    /// 无差异 = 偏离发生在最后一个 begin 之后（trailing turn 进行中/外部
    /// 修改，无法区分）→ 诚实入 unchecked_paths（可见、不冒充冲突）；
    /// target..reference 间已偏离 = 被回退区间内 agent 自己的变更，回退本
    /// 意如此，不算冲突。
    pub fn conflict_scan(&self, from_turn: usize) -> CheckpointConflictReport {
        self.conflict_scan_scoped(from_turn, None)
    }

    /// R31：会话收窄版冲突预检。`rewind_session = Some(回退发起会话)` 时，
    /// 回退区间内**其他会话**（条目归属 ≠ 发起会话）声明过变更的路径进
    /// `other_session_paths`（默认拒绝，force 显式强过）——它们会被 restore
    /// 的全树恢复碾掉但不属于本次回退意图。None = 不收窄（旧全局语义）。
    /// 不可归属条目（session=None）按「同会话」处理：走既有指纹比对语义，
    /// 单会话与存量数据行为逐字节不变。其他会话路径不做指纹比对（封印态
    /// 通常=现盘态，冲突语义是「波及」而非「外部修改」；若其文件确被外部
    /// 改动，仍默认拒绝，只是标签归入波及面）。
    pub fn conflict_scan_scoped(
        &self,
        from_turn: usize,
        rewind_session: Option<&str>,
    ) -> CheckpointConflictReport {
        let mut report = CheckpointConflictReport::default();
        // 锁内收集 (path, sealed, sealed_hash) 快照，放锁后读盘比对
        // （锁纪律：不带着 inner 锁做文件 IO）。
        let mut last_declared: Vec<(String, bool, Option<String>)> = Vec::new();
        // R31：其他会话声明的路径（保序去重）。
        let mut other_paths: Vec<String> = Vec::new();
        // restore 同款守卫：from_turn 后无 checkpoint → no-op（锁内一次性赋值）。
        let has_post;
        {
            let guard = self.inner.lock();
            let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
            // R4：cur 桶化——所有会话的进行中 turn 都参与全量收集。
            all.extend(guard.cur.values());
            all.sort_by_key(|c| c.turn);
            has_post = all.iter().any(|c| c.turn >= from_turn);
            for c in all.iter().filter(|c| c.turn >= from_turn) {
                let paths: Vec<String> = if c.paths.is_empty() {
                    c.files.iter().map(|f| f.path.clone()).collect()
                } else {
                    c.paths.clone()
                };
                // R31：其他会话的 turn——单列进 other_session_paths，不进
                // last_declared（不重复出现在 unchecked/git-form 补充里）。
                if is_other_session(&c.session, rewind_session) {
                    for p in paths {
                        if !other_paths.contains(&p) {
                            other_paths.push(p);
                        }
                    }
                    continue;
                }
                for p in paths {
                    let fp = c
                        .after
                        .iter()
                        .find(|f| f.path == p)
                        .and_then(|f| f.hash.clone());
                    if let Some(slot) = last_declared.iter_mut().find(|(lp, _, _)| lp == &p) {
                        slot.1 = c.sealed;
                        slot.2 = fp;
                    } else {
                        last_declared.push((p, c.sealed, fp));
                    }
                }
            }
        }
        report.other_session_paths = other_paths;
        let declared: std::collections::HashSet<String> =
            last_declared.iter().map(|(p, _, _)| p.clone()).collect();
        for (path, sealed, sealed_hash) in last_declared {
            if !sealed {
                report.unchecked_paths.push(path);
                continue;
            }
            let Some(expected) = sealed_hash else {
                // 已封印但该路径无指纹（防御性：seal 遍历 paths 全集，正常
                // 不会发生）——诚实跳过不误报。
                report.unchecked_paths.push(path);
                continue;
            };
            match self.hash_on_disk(&path) {
                Some(cur_hash) if cur_hash == expected => {} // 干净
                cur => {
                    report.conflicts.push(CheckpointConflict {
                        path,
                        expected_hash: expected,
                        current_hash: cur.unwrap_or_default(), // 空 = 已不在盘
                    });
                }
            }
        }
        // git 形态：全树 diff 的未声明路径补进 unchecked（见 fn 文档）。排
        // 除集 = 声明路径 ∪ 已报冲突路径（后者已有精确封印判定，不得再以
        // unchecked 身份重复出现）∪ R31 其他会话路径（已单列波及面）；补进
        // 时对既有 unchecked 去重。
        // from_turn 后无 checkpoint（restore no-op）或无 target tree（hybrid
        // 兜底路径）时 restore 不做 tree 恢复，这里同样不报。
        #[cfg(feature = "checkpoint-git")]
        if has_post
            && let Some(gb) = self.git.as_ref()
            && let (Some(target_hex), Some(reference_hex)) = (
                self.tree_hex_at_or_before(from_turn),
                self.latest_tree_hex(),
            )
        {
            let mut excluded = declared;
            for c in &report.conflicts {
                excluded.insert(c.path.clone());
            }
            for p in &report.other_session_paths {
                excluded.insert(p.clone());
            }
            let mut repo = gb.repo.lock();
            let extra =
                Self::tree_diff_unchecked(&mut repo, &target_hex, &reference_hex, &excluded);
            drop(repo);
            for p in extra {
                if !report.unchecked_paths.contains(&p) {
                    report.unchecked_paths.push(p);
                }
            }
        }
        // checkpoint-git 关：全树 diff 段不编译，has_post 消费面消失。
        #[cfg(not(feature = "checkpoint-git"))]
        let _ = (has_post, declared);
        report
    }

    /// F6：target..现状 全树 diff 中「偏离发生在最新 begin 之后」的未声明
    /// 路径集。参照判定：路径在 target 与 reference（最新 begin tree）里
    /// 状态一致 = 偏离在 begin 后产生（trailing/外部，不可区分）；已偏离
    /// = 回退区间内变更（agent 本意）。任何失败 = 空集（预检降级，不误
    /// 报）。libgit2 借用规则同 git_restore：先可变（write_current_tree）
    /// 再拿 immutable Tree 句柄。
    #[cfg(feature = "checkpoint-git")]
    fn tree_diff_unchecked(
        repo: &mut git2::Repository,
        target_hex: &str,
        reference_hex: &str,
        declared: &std::collections::HashSet<String>,
    ) -> Vec<String> {
        let cur_hex = match Self::write_current_tree(repo) {
            Ok(h) => h,
            Err(_) => return Vec::new(),
        };
        let (Ok(target_oid), Ok(reference_oid), Ok(cur_oid)) = (
            git2::Oid::from_str(target_hex),
            git2::Oid::from_str(reference_hex),
            git2::Oid::from_str(&cur_hex),
        ) else {
            return Vec::new();
        };
        let (Ok(target_tree), Ok(reference_tree), Ok(cur_tree)) = (
            repo.find_tree(target_oid),
            repo.find_tree(reference_oid),
            repo.find_tree(cur_oid),
        ) else {
            return Vec::new();
        };
        let Ok(diff) = repo.diff_tree_to_tree(Some(&target_tree), Some(&cur_tree), None) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for delta in diff.deltas() {
            let Some(p) = delta.new_file().path().or_else(|| delta.old_file().path()) else {
                continue;
            };
            let rel = p.to_string_lossy().to_string();
            if rel.split(['/', '\\']).any(|seg| seg == "..") || declared.contains(&rel) {
                continue;
            }
            let t_state = target_tree.get_path(p).map(|e| e.id()).ok();
            let r_state = reference_tree.get_path(p).map(|e| e.id()).ok();
            if t_state == r_state {
                out.push(rel);
            }
        }
        out
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
    /// Returns a [`RestoreReport`]（R3：成功/失败分明，失败项带原因——此前
    /// (written, deleted) 两 vec 把失败静默吞掉，rewind 侧无条件盖
    /// 「applied」戳并摧毁 checkpoint，失败后无法重试）。
    ///
    /// git 模式：target = 最新 `turn <= from_turn` 的影子 tree；对 target..现状
    /// 的真实 diff 逐文件恢复/删除——覆盖 shell 副作用（preview 声明之外的
    /// 文件）。工作区外的 hybrid 内容快照（files 里 content Some 的）在 tree
    /// 恢复后按 JSON 语义补写。
    pub async fn restore_code(&self, from_turn: usize) -> RestoreReport {
        // ---- 第一段：inner 锁内定 target tree + hybrid 集合，随即放锁 ----
        let (target_hex, hybrid) = {
            let guard = self.inner.lock();
            let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
            // R4：cur 桶化——所有会话的进行中 turn 都参与全量收集。
            all.extend(guard.cur.values());
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
        #[cfg(feature = "checkpoint-git")]
        let mut result = match self.git.as_ref() {
            Some(gb) => {
                let mut repo = gb.repo.lock();
                Self::git_restore(&self.root, &mut repo, &target_hex)
            }
            None => {
                // 跨模式（会话曾以 git 模式记录，现 .git 消失回落 JSON）：tree
                // 段跳过，工作区内的恢复是诚实 no-op（绝无误删）。
                warn!("[checkpoint] 目标 tree {target_hex} 存在但影子库不可用，跳过工作区内恢复");
                RestoreReport::default()
            }
        };
        // checkpoint-git 关：git 时代索引（tree Some）诚实 no-op，不误删。
        #[cfg(not(feature = "checkpoint-git"))]
        let mut result: RestoreReport = {
            warn!(
                "[checkpoint] 目标 tree {target_hex} 存在但影子库未编译（checkpoint-git 关），跳过工作区内恢复"
            );
            RestoreReport::default()
        };

        // ---- 第三段：hybrid 工作区外内容恢复 ----
        let hw = self.hybrid_restore(hybrid).await;
        result.merge(hw);
        result
    }

    /// JSON 语义的内容恢复（git 模式下仅工作区外路径走这里）。
    /// R3：失败项进 `failures`（带原因），不再 `is_ok()` 静默吞。
    async fn hybrid_restore(&self, hybrid: Vec<(String, Option<String>)>) -> RestoreReport {
        let mut report = RestoreReport::default();
        for (path, content) in hybrid {
            let Some(abs) = self.safe_path(&path) else {
                continue;
            };
            match content {
                None => match tokio::fs::remove_file(&abs).await {
                    Ok(()) => report.deleted.push(path),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // 目标本就不存在 = 恢复语义已达成（快照也记录它不存在）。
                    }
                    Err(e) => report.failures.push((path, e.to_string())),
                },
                Some(body) => {
                    if let Some(parent) = abs.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    match tokio::fs::write(&abs, body).await {
                        Ok(()) => report.written.push(path),
                        Err(e) => report.failures.push((path, e.to_string())),
                    }
                }
            }
        }
        report
    }

    /// Metadata for all checkpoints (oldest turn first), for the rewind picker.
    pub fn list_meta(&self) -> Vec<CheckpointMeta> {
        let guard = self.inner.lock();
        let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
        // R4：cur 桶化——所有会话的进行中 turn 都参与全量收集。
        all.extend(guard.cur.values());
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
        self.truncate_from_scoped(from_turn, None);
    }

    /// R31：会话收窄版截断——只清「本会话」的 turn ≥ from_turn 条目（含落
    /// 盘 turn-N.json）。turn 序号是 loop 全局的，而 `sessions.rewind_to_message`
    /// 是会话级 API：无收窄的全局截断会把并发会话的回退历史整段清掉，永久
    /// 摧毁其 rewind/redo 能力。`rewind_session = None` = 不收窄（旧全局语义，
    /// 测试与兼容路径沿用）。
    ///
    /// 磁盘镜像判定：读 turn-N.json 解析 session 与内存 retain 同判定；解析
    /// 失败 = 不可归属 = 按本会话处理（删——与内存侧 None 语义一致）。
    pub fn truncate_from_scoped(&self, from_turn: usize, rewind_session: Option<&str>) {
        {
            let mut guard = self.inner.lock();
            // 保留：旧 turn（< from_turn）∪ 其他会话条目；清：本会话 turn ≥
            // from_turn。R4：cur 桶化——逐桶同判定，只清「本会话」的进行中
            // 桶（其他会话的 cur 连同其去重集原位不动）。
            guard
                .done
                .retain(|c| c.turn < from_turn || is_other_session(&c.session, rewind_session));
            let stale: Vec<Option<String>> = guard
                .cur
                .iter()
                .filter(|(_, c)| {
                    c.turn >= from_turn && !is_other_session(&c.session, rewind_session)
                })
                .map(|(k, _)| k.clone())
                .collect();
            for k in stale {
                guard.cur.remove(&k);
                guard.seen.remove(&k);
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
                    && !persisted_checkpoint_belongs_to_other_session(&ent.path(), rewind_session)
                {
                    // BUG-20261025-35：裸单发 remove + `let _` 吞错——Windows
                    // 上刚落盘的 turn 镜像被 Defender/索引器瞬时持句是常态
                    // （fsutil 模块头注记的 CI 实证教训），删除静默失败后
                    // `load()` 重启回载会把**已回退的 turn 复活**进索引：
                    // redo 陈旧性守卫被幻影 tree 永久误杀、回退选择器出幻
                    // 影条目、后续冲突预检对已回滚盘面假阳性逼 force。走
                    // 韧性重试；耗尽才 warn 留痕（诚实可见，不伪装成功）。
                    if let Err(e) = nemesis_utils::fsutil::remove_file_retry(ent.path()) {
                        tracing::warn!(
                            file = %ent.path().display(),
                            error = %e,
                            "[checkpoint] turn 镜像删除重试耗尽（重启后 load 将复活已回退 turn，redo 守卫失效/选择器幻影）: turn-{t}"
                        );
                    }
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
    #[cfg(feature = "checkpoint-git")]
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

    /// checkpoint-git 关：JSON 形态语义——恒 None（redo 文件步诚实跳过）。
    #[cfg(not(feature = "checkpoint-git"))]
    pub fn current_tree_hex(&self) -> Option<String> {
        None
    }

    /// 共享实现：`turn <= bound` 里最后一个带 tree 的。
    fn tree_hex_at_or_before(&self, bound: usize) -> Option<String> {
        let guard = self.inner.lock();
        let mut all: Vec<&Checkpoint> = guard.done.iter().collect();
        // R4：cur 桶化——所有会话的进行中 turn 都参与全量收集。
        all.extend(guard.cur.values());
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
    #[cfg(feature = "checkpoint-git")]
    pub fn restore_to_tree(&self, tree_hex: &str) -> Result<RestoreReport, String> {
        let Some(gb) = self.git.as_ref() else {
            return Err("影子库不可用（JSON 形态），无法按 tree 恢复文件".to_string());
        };
        let mut repo = gb.repo.lock();
        Ok(Self::git_restore(&self.root, &mut repo, tree_hex))
    }

    /// checkpoint-git 关：诚实报未编译（无 tree 可言）。
    #[cfg(not(feature = "checkpoint-git"))]
    pub fn restore_to_tree(&self, _tree_hex: &str) -> Result<RestoreReport, String> {
        Err("影子库未编译（checkpoint-git 关），无法按 tree 恢复文件".to_string())
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
        // R4：cur 桶化——所有会话的进行中 turn 都参与全量收集。
        all.extend(guard.cur.values());
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
    #[cfg(feature = "checkpoint-git")]
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

    /// checkpoint-git 关：诚实报未编译。
    #[cfg(not(feature = "checkpoint-git"))]
    pub fn read_file_from_tree(
        &self,
        _tree_hex: &str,
        _path: &str,
    ) -> Result<Option<String>, String> {
        Err("影子库未编译（checkpoint-git 关），无法按 tree 读文件".to_string())
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
        // BUG-20261025-43：改原子写。此前裸 std::fs::write 直写目标——崩溃/
        // 断电落在写中 → 半截 JSON，且 turn-N.json 每 turn 被 snapshot/seal
        // 多次覆写，撕裂毁掉的是先前已落盘的好镜像。重启后 load 解析失败
        // 静默跳过 → rewind 选择器丢条目、tree 基线/封印指纹消失（冲突预检
        // 对该 turn 致盲）、redo 陈旧性守卫基线漂移。同 crate 的
        // loop_continuation R14 同族先例；write_file_atomic 的临时名
        // `.tmp-*` 无 .json 扩展名，load/truncate 的 turn-*.json 扫描天然
        // 无视崩溃残留。
        if let Err(e) = nemesis_utils::write_file_atomic(&path.to_string_lossy(), &bytes, 0o600) {
            warn!(
                turn = cp.turn,
                error = %e,
                "[checkpoint] turn 镜像落盘失败（重启后该 turn 的回退条目/封印指纹将丢失）"
            );
        }
    }

    /// Resolve `p` against the workspace root, rejecting `..` traversal
    /// escapes.
    ///
    /// SAN-10 复核结论（2026-09-16）：**刻意不做工作区边界校验**。D2 hybrid
    /// 是已验收的既定设计（`git_mode_hybrid_restores_out_of_workspace_paths`）：
    /// `restrict=false` 部署下声明式工具合法写工作区外文件，undo 必须能回滚
    /// 它们（否则 rewind 诚实性破产）。快照路径只能经 live 安全审批过的工具
    /// 调用进入 checkpoint；restore 只是重放。checkpoint 存储本身即 restore
    /// 的信任锚（能篡改它的人早已能改写任意快照内容），边界拦不住新增威胁
    /// 反而砍掉合法功能。
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

// BUG-20261025-43（BUG-HUNT R120）：persist 原子写 + load 损坏镜像 loud 留痕
// 回归锁（R14 同族先例形态）。
#[cfg(test)]
mod r43_tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

// D2 (devtool-upgrade 阶段 5): git 影子库双形态回归（D4 验收）。
#[cfg(all(test, feature = "checkpoint-git"))]
mod d2_tests;

// E3 (devtool-upgrade 阶段 5): tree 查询 + restore_to_tree（消息级回退
// redo 的文件原语）测试。
#[cfg(all(test, feature = "checkpoint-git"))]
mod e3_tests;

// M3 (devtool-upgrade 阶段 5): base_for_path + read_file_from_tree（会话级
// diff 查看器的基线原语）测试。
#[cfg(all(test, feature = "checkpoint-git"))]
mod m3_tests;

// 覆盖率补充批次：影子库失败回落 / alternates 降级 / restore_to_tree 坏
// hex / checkout·删除失败面 / read_file_from_tree 错误面 / hybrid 失败 /
// 影子库外力消失降级。
#[cfg(all(test, feature = "checkpoint-git"))]
mod cov_tests;

// P20 (2026-09-25 能力扩展 WS7): turn 收尾封印（after 指纹）+ 冲突预检
// （last-declared 规则）+ 影子库自愈（幂等清扫）测试。
#[cfg(all(test, feature = "checkpoint-git"))]
mod p20_tests;

// P4 (2026-10-17 交付门): 交付分支捕获 / patch 重算 / 列举 + 分支名校验
// 测试。
#[cfg(all(test, feature = "checkpoint-git"))]
mod p4_tests;
