//! 项目目录 git 仓库化 + 三方合并（看板项目档案 goal）。
//!
//! 档案目录既是工作集也是档案（goal）——本模块把它变成 git 仓库（master
//! 进程内 git2，静态编译，零外部 git 安装/零新进程/零新端口）：
//!
//! - [`ensure_repo`]：幂等 init（缺则建；对现有内容首 commit——用户选已有
//!   目录作初始基线是 合法形态；`.gitignore` 照常生效，投影文件不进库）；
//! - [`commit_worktree`]：工作集现状落一笔 commit（tree 无变化 = no-op）；
//! - [`export_head_tree`]：HEAD 树全量导出到目录（基线快照，天然只含
//!   跟踪树——投影文件已被.gitignore 排除，不会下发历史执行记录）；
//! - [`merge_changeset`]： 三方合并——基线 commit 为 ancestor、worker 树
//!   为 theirs、当前 HEAD 为 ours（行级合并，git2 [`Repository::merge_trees`]）；
//!   无冲突 = 自动合入 + commit + 工作区同步；冲突 = 逐文件明细（内容级
//!   二进制判定）原样返回，**不触碰仓库任何状态**。
//!
//! 并发模型：所有函数每次调用独立 open 仓库、用完即弃（`git2::Repository`
//! 是 Send + !Sync；短生命周期对象不做全局共享，天然免锁）。合并串行化由
//! 调用方保证（每收一个变更集合一次 commit 一次）。

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

/// 变更集归属校验 + 合并输入（worker 声明其基线 commit——祖先由此
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
    /// 真冲突：逐文件明细（`binary` = 内容级判定， 二进制不进行级合并）。
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

/// 变更集路径归一化：剥误带的项目档案目录自身前缀。
///
/// worker 变更集路径契约是「项目档案根相对」（exec 工作副本镜像档案树），
/// 但 worker LLM 偶尔按 master 工作区视角写文件——`board-projects/<项目名>/…`
/// 整段落进工作副本，合并后双重嵌套 `<项目>/board-projects/<项目>/…`
/// （三设备压测 / 实证；`docs/…` 等正确项目相对路径不受
/// 影响）。仅当首两段精确等于 [`crate::archive:BOARD_PROJECTS_DIR`] +
/// 档案目录名（root 的文件名）且后段非空时剥除，其余逐字节不动；别的项目
/// 前缀（不等档案目录名）不剥——宁嵌套不误剥。
fn normalize_repo_path(root: &Path, path: &str) -> String {
    let Some(project_dir) = root.file_name().and_then(|n| n.to_str()) else {
        return path.to_string();
    };
    let mut segs = path.split('/');
    if segs.next() != Some(crate::archive::BOARD_PROJECTS_DIR) || segs.next() != Some(project_dir) {
        return path.to_string();
    }
    let rest = segs.collect::<Vec<_>>().join("/");
    if rest.is_empty() {
        return path.to_string();
    }
    rest
}

/// 变更集路径形状校验：`/` 分隔、非空、无 `.`/`..`/空组件、无反斜杠/盘符、
/// 无 8.3 短名（archive.rs 同款canonicalize 前先词法拦截）。
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
// 仓库幂等 init + 工作集 commit
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

/// HEAD commit hex（空仓库 = None； 派发里程碑基线标记用）。
pub fn head_commit_hex(root: &Path) -> Result<Option<String>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    Ok(head_commit(&repo).map(|c| c.id().to_string()))
}

/// HEAD tree hex（空仓库 = None）：树等值核验用。exec 捕获路径的交付基线
/// commit 在 worker 侧 exec 仓（fresh init），主仓 commit 永远对不上——
/// 内容态一致（树等值）才是跨仓可移植的不变量。
pub fn head_tree_hex(root: &Path) -> Result<Option<String>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    Ok(head_commit(&repo).map(|c| c.tree_id().to_string()))
}

// ---------------------------------------------------------------------------
// 外循环：远端克隆（任务专属副本物化）
// ---------------------------------------------------------------------------

/// 克隆远端仓库到 `dest`，返回克隆完成时的 HEAD commit hex（= 钉住的
/// base SHA：副本起点即此刻远端默认分支，派发基线/交付 patch 都以它为
/// 锚）。已存在 `.git` 的 `dest` = 幂等复用（返回现有 HEAD，**不 fetch
/// 不重置**——副本一旦物化，后续合并/交付历史不能被外部重写打断）。
///
/// 凭据：`None` = 匿名（本地路径 / file:// / 公共仓库）；`Some((user,
/// pass))` = basic auth（PAT 形态由 connector 配置声明 user/pass 语义，
/// 本层只收通用二元组）。
/// 本地 config 钉 `core.autocrlf=false`（契约同 [`ensure_repo`]——基线
/// tree 字节精确对账依赖行尾稳定）。
/// 代理：见 [`apply_proxy`]——企业代理后仓库走 `proxy: Some(url)`。
///
/// 诚实边界：本 crate 的 git2 为 `default-features = false`（无 TLS 后端
/// ）——`https://` 远端会得到 libgit2 的「transport not supported」类
/// 错误原样透传；http:// 与本地路径可用。
pub fn clone_repo(
    url: &str,
    dest: &Path,
    cred: Option<(&str, &str)>,
    proxy: Option<&str>,
) -> Result<String, String> {
    if dest.join(".git").exists() {
        return head_commit_hex(dest)?.ok_or_else(|| {
            format!(
                "项目仓库 {} 已有 .git 但无 HEAD（空仓库？）",
                dest.display()
            )
        });
    }
    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some((user, pass)) = cred {
        callbacks.credentials(move |_url, _user_from_url, _allowed| {
            git2::Cred::userpass_plaintext(user, pass)
        });
    }
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(callbacks);
    fo.download_tags(git2::AutotagOption::None);
    let mut px = git2::ProxyOptions::new();
    apply_proxy(&mut px, proxy);
    fo.proxy_options(px);
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fo);
    // 克隆自带的 checkout **必须关过滤器**：local config 尚未落钉、libgit2
    // 落到系统/全局层（本机 autocrlf=true）会把 LF blob smudge 成 CRLF
    // 工作树——合并三方 base(blob LF) 与 ours/theirs(工作树 CRLF) 行尾
    // 不同源，未触及行全部伪冲突（全文件伪冲突冻结）。
    // 事后 reset --hard 不可靠（index oid 与目标相同时跳过重写，行尾漂移
    // 不被归一）——在 checkout 本身禁过滤器，工作树恒为 blob 原始字节。
    let mut co = git2::build::CheckoutBuilder::new();
    co.force();
    co.disable_filters(true);
    builder.with_checkout(co);
    builder
        .clone(url, dest)
        .map_err(|e| format!("clone {url} → {} 失败: {e}", dest.display()))?;
    // 钉 autocrlf（克隆后的 local config，管 clone 之后的 add/commit 路径
    // ——checkout 过滤器已由上面 with_checkout 关死；字节精确契约同
    // ensure_repo 注记）。
    {
        let repo = Repository::open(dest)
            .map_err(|e| format!("打开克隆产物 {} 失败: {e}", dest.display()))?;
        let mut cfg = repo.config().map_err(|e| e.to_string())?;
        cfg.set_bool("core.autocrlf", false)
            .map_err(|e| format!("钉 core.autocrlf=false 失败: {e}"))?;
    }
    head_commit_hex(dest)?.ok_or_else(|| format!("克隆完成但 {} 无 HEAD", dest.display()))
}

/// 远端推送（PR 发布）：把本地分支推到 `remote` 的 `remote_ref`。
/// 凭据语义同 [`clone_repo`]。push 前本地分支必须存在（缺 = Err）。
/// force = false（快进失败 = Err——交付分支被外部移动必须人裁，不覆盖）。
/// 代理语义同 [`apply_proxy`]（Gerrit refs/for 推送同路）。
pub fn push_branch(
    root: &Path,
    local_branch: &str,
    remote: &str,
    remote_ref: &str,
    cred: Option<(&str, &str)>,
    proxy: Option<&str>,
) -> Result<(), String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    let spec = format!("{local_branch}:{remote_ref}");
    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some((user, pass)) = cred {
        callbacks.credentials(move |_url, _user_from_url, _allowed| {
            git2::Cred::userpass_plaintext(user, pass)
        });
    }
    let mut po = git2::PushOptions::new();
    po.remote_callbacks(callbacks);
    let mut px = git2::ProxyOptions::new();
    apply_proxy(&mut px, proxy);
    po.proxy_options(px);
    let mut remote_obj = repo
        .find_remote(remote)
        .map_err(|e| format!("找不到远端 `{remote}`: {e}"))?;
    remote_obj
        .push(&[spec.as_str()], Some(&mut po))
        .map_err(|e| format!("推送 {spec} → {remote} 失败: {e}"))
}

// ---------------------------------------------------------------------------
// Gerrit 发布支撑（SC 套件批）：确定性 Change-Id + 单提交 atop
// ---------------------------------------------------------------------------

/// 从种子串确定性派生 Gerrit Change-Id（`I<40hex>`，SHA-1 of seed）。
/// 同一任务多次重试派生同一 Change-Id → Gerrit 服务端按 Change-Id 归并
/// （push 建评审天然幂等，对账按此锚点回查——宿主 `{{change_id}}` 运行期
/// token 的统一定义）。
pub fn derive_change_id(seed: &str) -> Result<String, String> {
    let oid = git2::Oid::hash_object(git2::ObjectType::Commit, seed.as_bytes())
        .map_err(|e| format!("Change-Id 派生失败: {e}"))?;
    Ok(format!("I{oid}"))
}

/// 任意 commit 的 tree hex（commit 缺席 = Err——发布面的诚实契约；
/// Gerrit 单提交臂从交付 commit 反查交付树用）。
pub fn tree_of_commit_hex(root: &Path, commit_hex: &str) -> Result<String, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开仓库 {} 失败: {e}", root.display()))?;
    let oid = git2::Oid::from_str(commit_hex).map_err(|e| format!("commit hash 形态非法: {e}"))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("commit {commit_hex} 不在仓库 {}: {e}", root.display()))?;
    Ok(commit.tree_id().to_string())
}

/// Gerrit 单提交形态：把交付树铸成**单亲 commit**，atop
/// `refs/remotes/origin/{target_branch}` 远端头。交付分支 HEAD 可能是
/// 档案管线「变更集合入」的 merge commit——Gerrit 对 merge change 评审
/// 体验差（diff 噪声/revert 复杂），评审期望单提交基于最新分支头。
///
/// 语义：
/// - 目标头 = `refs/remotes/origin/{target_branch}`（缺席 = Err——目标
///   分支未同步过就谈不团单提交，先 repo_sync）；
/// - 基线树 `base_tree_hex` 必给（调用方从 payload `base_tree` 或基线
///   commit 的 tree 解析；两侧都缺 = 调用方诚实报错，本层不猜）；
/// - 目标头树 == 基线树 → fast 形态：交付树直接单亲 atop 目标头（零合并）；
/// - 否则三方合并（ancestor=基线树，ours=交付树，theirs=目标头树）——
///   冲突 = Err（需人工 rebase，绝不产带冲突标记的提交）；
/// - 合并后树与目标头树一致（目标已含交付）= Err（无可发布差异）；
/// - 结果 force 落 `refs/heads/{branch}`（该分支正被工作树检出 = Err，
///   不背工作树失同步）。
///
/// 返回新 commit hex。
pub fn single_commit_atop(
    root: &Path,
    branch: &str,
    target_branch: &str,
    base_tree_hex: &str,
    delivery_tree_hex: &str,
    message: &str,
) -> Result<String, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开仓库 {} 失败: {e}", root.display()))?;

    if head_branch_name(&repo)? == branch {
        return Err(format!(
            "分支 {branch} 正被工作树检出——单提交重铸不允许在检出分支上进行"
        ));
    }

    let target_commit = repo
        .find_reference(&format!("refs/remotes/origin/{target_branch}"))
        .map_err(|_| {
            format!("远端 origin 上没有分支 `{target_branch}`——Gerrit 单提交需要先同步目标分支")
        })?
        .peel_to_commit()
        .map_err(|e| format!("origin/{target_branch} 解析 commit 失败: {e}"))?;

    let parse_tree = |hex: &str, what: &str| -> Result<git2::Tree<'_>, String> {
        let oid =
            git2::Oid::from_str(hex).map_err(|e| format!("{what} tree hash 形态非法: {e}"))?;
        repo.find_tree(oid)
            .map_err(|e| format!("{what} tree {hex} 不在仓库: {e}"))
    };
    let base_tree = parse_tree(base_tree_hex, "基线")?;
    let delivery_tree = parse_tree(delivery_tree_hex, "交付")?;

    let merged_tree_oid = if target_commit.tree_id() == base_tree.id() {
        delivery_tree.id()
    } else {
        let theirs = target_commit
            .tree()
            .map_err(|e| format!("目标头 tree 解析失败: {e}"))?;
        let mut merged = repo
            .merge_trees(&base_tree, &delivery_tree, &theirs, None)
            .map_err(|e| format!("三方合并失败: {e}"))?;
        if merged.has_conflicts() {
            return Err(format!(
                "交付与目标分支 origin/{target_branch} 最新提交存在内容冲突——Gerrit 单提交需人工 rebase 后重发，不产带冲突标记的提交"
            ));
        }
        merged
            .write_tree_to(&repo)
            .map_err(|e| format!("合并树落库失败: {e}"))?
    };

    if merged_tree_oid == target_commit.tree_id() {
        return Err(format!(
            "交付内容已包含在 origin/{target_branch} 最新提交中（合并后无差异）——无可发布变更"
        ));
    }

    let sig = signature()?;
    let merged_tree = repo
        .find_tree(merged_tree_oid)
        .map_err(|e| format!("合并树回读失败: {e}"))?;
    // commit 不绑 ref（Some(refname) 会在 ref 现头 ≠ 指定父时拒绝——重试
    // 重铸场景 ref 头是上一轮产物），落库后 force 更新分支 ref = 文档承诺
    // 的「force 落 refs/heads/{branch}」（同 Change-Id 重试 = 新 patchset）。
    let oid = repo
        .commit(None, &sig, &sig, message, &merged_tree, &[&target_commit])
        .map_err(|e| format!("单提交创建失败: {e}"))?;
    repo.reference(
        &format!("refs/heads/{branch}"),
        oid,
        true,
        "nemesis-board: gerrit single-commit recast",
    )
    .map_err(|e| format!("分支 {branch} force 更新失败: {e}"))?;
    Ok(oid.to_string())
}

// ---------------------------------------------------------------------------
// repo_sync 原语（jira-gitlab-flow 批）：确定性 clone-or-pull
// ---------------------------------------------------------------------------

/// [`sync_repo_remote`] 结果形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// 目标目录原本无仓库——全新克隆。
    Cloned,
    /// 仓库已存在但目标分支本地缺席——从 `origin/<branch>` 物化并检出。
    Checkout,
    /// 本地已包含远端全部提交——零动作。
    UpToDate,
    /// 可快进——本地分支推进到远端头。
    FastForward,
    /// 已分叉——三方合并出一笔双亲 merge commit。
    Merged,
}

/// 工作区洁净判定：任何非 ignored 条目（改动/未跟踪）= 脏。
pub fn workdir_is_clean(root: &Path) -> Result<bool, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开仓库 {} 失败: {e}", root.display()))?;
    let statuses = repo
        .statuses(None)
        .map_err(|e| format!("读取工作区状态失败: {e}"))?;
    Ok(!statuses.iter().any(|e| !e.status().is_ignored()))
}

/// https 远端错误附加 git-https feature 提示（无 TLS 形态的诚实指路）。
fn tls_hint(e: String) -> String {
    if e.contains("transport") {
        format!(
            "{e}（提示：https 远端需要 `git-https` feature——桌面构建默认启用，定制构建以 --features git-https 开启）"
        )
    } else {
        e
    }
}

/// 代理语义（SC 套件批）：libgit2 **不读环境变量**——企业内网
/// 「仅经代理可达」的仓库必须显式注入 ProxyOptions。三档：
/// - `None` = libgit2 默认（不走代理，既有行为零变化——全部旧调用点即此态）；
/// - `Some("")` = auto（读 http.proxy 配置 / 环境探测）；
/// - `Some(url)` = 钉死指定代理（如 `socks5h://host:port`——DNS 解析移交
///   代理侧，本机对目标域名不可解析是代理存在的理由）。
fn apply_proxy(opts: &mut git2::ProxyOptions, proxy: Option<&str>) {
    match proxy {
        None => {}
        Some("") => {
            opts.auto();
        }
        Some(url) => {
            opts.url(url);
        }
    }
}

/// checkout 选项（force + 禁行尾过滤器——字节精确契约同 [`clone_repo`]）。
fn sync_checkout_options() -> git2::build::CheckoutBuilder<'static> {
    let mut co = git2::build::CheckoutBuilder::new();
    co.force();
    co.disable_filters(true);
    co
}

/// 目标目录已存在且非空（只探测存在性，不消费遍历顺序）。
fn dir_is_nonempty(dest: &Path) -> Result<bool, String> {
    if !dest.exists() {
        return Ok(false);
    }
    let mut it =
        std::fs::read_dir(dest).map_err(|e| format!("读取目录 {} 失败: {e}", dest.display()))?;
    Ok(it.next().is_some())
}

/// 非空目录内的就地物化（等价克隆）。
///
/// 看板项目注册目录被 `project.create` 恒跑 `ensure_scaffold`（project.json /
/// timeline.jsonl /.gitignore / docs//artifacts//records/ 预置在场），而
/// libgit2 `RepoBuilder::clone` 拒绝非空目标——主路径 clone 必炸。此函数用
/// `Repository::init`（非空目录合法）+ 加 origin + fetch + 建本地分支检出
/// 完成同一件事，**scaffold 档案原样保留**（checkout 只写仓库跟踪文件，
/// 不触碰未跟踪内容）。
///
/// 分支选取（确定性降链）：远端 HEAD symref → `origin/main` →
/// `origin/master` → `origin/*` 字典序第一个。契约同 [`clone_repo`]：
/// checkout 禁过滤器 + 事后钉 `core.autocrlf=false`。
/// 返回 HEAD commit hex。
fn init_fetch_checkout(
    url: &str,
    dest: &Path,
    cred: Option<(&str, &str)>,
    proxy: Option<&str>,
) -> Result<String, String> {
    let repo = Repository::init(dest).map_err(|e| format!("init {} 失败: {e}", dest.display()))?;
    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some((user, pass)) = cred {
        callbacks.credentials(move |_url, _user_from_url, _allowed| {
            git2::Cred::userpass_plaintext(user, pass)
        });
    }
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(callbacks);
    fo.download_tags(git2::AutotagOption::None);
    let mut px = git2::ProxyOptions::new();
    apply_proxy(&mut px, proxy);
    fo.proxy_options(px);
    let mut remote = repo
        .remote("origin", url)
        .map_err(|e| format!("加 origin {url} 失败: {e}"))?;
    remote
        .fetch(&[] as &[String], Some(&mut fo), None)
        .map_err(|e| format!("fetch {url} 失败: {e}"))?;

    // 远端默认分支：HEAD symref 优先（smart 传输才可靠；本地路径取不到
    // 走降链），main/master 钉序兜底，最后字典序任选其一。
    let head_default = remote.default_branch().ok().and_then(|buf| {
        buf.as_str()
            .ok()
            .and_then(|s| s.strip_prefix("refs/heads/").map(str::to_string))
    });
    let branch = match head_default {
        Some(b)
            if repo
                .find_reference(&format!("refs/remotes/origin/{b}"))
                .is_ok() =>
        {
            b
        }
        _ => {
            let mut picked: Option<String> = None;
            for name in ["main", "master"] {
                if repo
                    .find_reference(&format!("refs/remotes/origin/{name}"))
                    .is_ok()
                {
                    picked = Some(name.to_string());
                    break;
                }
            }
            if picked.is_none() {
                let mut names: Vec<String> = repo
                    .references_glob("refs/remotes/origin/*")
                    .map_err(|e| format!("枚举 origin 分支失败: {e}"))?
                    .filter_map(|r| {
                        let r = r.ok()?;
                        let name = r.name().ok()?;
                        name.strip_prefix("refs/remotes/origin/")
                            .filter(|s| *s != "HEAD")
                            .map(str::to_string)
                    })
                    .collect();
                names.sort();
                picked = names.into_iter().next();
            }
            picked.ok_or_else(|| format!("远端 origin（{url}）没有任何分支——repo_sync 无从物化"))?
        }
    };

    let remote_ref = format!("refs/remotes/origin/{branch}");
    let commit = repo
        .find_reference(&remote_ref)
        .and_then(|r| r.peel_to_commit())
        .map_err(|e| format!("{remote_ref} 解析 commit 失败: {e}"))?;
    if repo
        .find_reference(&format!("refs/heads/{branch}"))
        .is_err()
    {
        let mut local = repo
            .branch(&branch, &commit, false)
            .map_err(|e| format!("建本地分支 {branch} 失败: {e}"))?;
        local
            .set_upstream(Some(&format!("origin/{branch}")))
            .map_err(|e| format!("设 {branch} upstream 失败: {e}"))?;
    }
    repo.set_head(&format!("refs/heads/{branch}"))
        .map_err(|e| format!("切分支 {branch} 失败: {e}"))?;
    repo.checkout_head(Some(&mut sync_checkout_options()))
        .map_err(|e| format!("检出 {branch} 失败: {e}"))?;
    {
        let mut cfg = repo.config().map_err(|e| e.to_string())?;
        cfg.set_bool("core.autocrlf", false)
            .map_err(|e| format!("钉 core.autocrlf=false 失败: {e}"))?;
    }
    Ok(commit.id().to_string())
}

/// HEAD 所在分支名（detached HEAD = Err——不知道同步哪个分支）。
fn head_branch_name(repo: &Repository) -> Result<String, String> {
    let head = repo.head().map_err(|e| format!("仓库无 HEAD: {e}"))?;
    if !head.is_branch() {
        return Err(
            "仓库处于 detached HEAD 且未声明 branch——repo_sync 不知道同步哪个分支".to_string(),
        );
    }
    head.shorthand()
        .map_err(|e| format!("HEAD 分支名解析失败: {e}"))
        .map(str::to_string)
}

/// 确定性 clone-or-pull（flow 引擎 `repo_sync` 步的 git 半边）。
///
/// 与 [`clone_repo`] 的分工：clone_repo 的「已有 `.git` 幂等复用不 fetch」
/// 是交付管线钉基线语义（任务副本一旦物化不被外部重写打断）；本函数是
/// **同步原语**——流水线每轮任务开始前把项目仓库拉到远端最新：
///
/// - `dest` 无 `.git` → 全新克隆（checkout 禁过滤器 + `core.autocrlf=false`
///   钉死契约同 clone_repo）；目录已存在且非空（看板项目 scaffold 已在场）
///   → 就地物化（`init_fetch_checkout`——libgit2 clone 拒绝非空目标，
///   init+fetch+建分支检出等价克隆，scaffold 档案原样保留）；`branch` 声明
///   且 ≠ 克隆默认分支 → 从 `origin/<branch>` 建本地分支并检出
///   （`SyncOutcome::Checkout`）；
/// - 有 `.git` → 工作区脏 = Err（列脏项样本，绝不 stash/destroy）；fetch
///   `origin/<branch>` 后对账：本地已含远端 = UpToDate；可快进 = 推进；分叉
///   = `merge_commits` 纯树合并出**双亲 merge commit**（libgit2 merge 不触
///   工作区，冲突在写树前整体发现——冲突 = Err 冲突路径明细，停车走人不
///   猜）；合并树与本地树一致（远端改动已被本地包含）= 不产空 commit；
/// - `branch` 缺省 = 本地当前分支（detached = Err）。
///
/// 返回 `(形态, 同步后的 HEAD commit hex)`。凭据语义同 [`clone_repo`]。
/// https 远端在本 crate 无 TLS 默认形态下报 transport 错误——附加
/// feature 启用提示后透传。
pub fn sync_repo_remote(
    url: &str,
    dest: &Path,
    branch: Option<&str>,
    cred: Option<(&str, &str)>,
    proxy: Option<&str>,
) -> Result<(SyncOutcome, String), String> {
    let want = branch.map(str::trim).filter(|b| !b.is_empty());

    if !dest.join(".git").exists() {
        let head = if dir_is_nonempty(dest)? {
            init_fetch_checkout(url, dest, cred, proxy)
                .map_err(|e| tls_hint(format!("repo_sync 就地物化失败: {e}")))?
        } else {
            clone_repo(url, dest, cred, proxy)
                .map_err(|e| tls_hint(format!("repo_sync 克隆失败: {e}")))?
        };
        let repo = Repository::open(dest).map_err(|e| format!("打开克隆产物失败: {e}"))?;
        if let Some(want) = want {
            let current = head_branch_name(&repo)?;
            if current != want {
                let remote_commit = repo
                    .find_reference(&format!("refs/remotes/origin/{want}"))
                    .map_err(|_| {
                        format!("远端 origin 上没有分支 `{want}`——repo_sync 无法检出声明分支")
                    })?
                    .peel_to_commit()
                    .map_err(|e| format!("origin/{want} 解析 commit 失败: {e}"))?;
                repo.branch(want, &remote_commit, false)
                    .map_err(|e| format!("建本地分支 {want} 失败: {e}"))?;
                repo.set_head(&format!("refs/heads/{want}"))
                    .map_err(|e| format!("切分支 {want} 失败: {e}"))?;
                repo.checkout_head(Some(&mut sync_checkout_options()))
                    .map_err(|e| format!("检出 {want} 失败: {e}"))?;
                return Ok((SyncOutcome::Checkout, remote_commit.id().to_string()));
            }
        }
        return Ok((SyncOutcome::Cloned, head));
    }

    let repo =
        Repository::open(dest).map_err(|e| format!("打开仓库 {} 失败: {e}", dest.display()))?;

    // 脏态闸（列样本；绝不 stash/destroy——仓库里可能有人工未提交内容）。
    {
        let statuses = repo
            .statuses(None)
            .map_err(|e| format!("读取工作区状态失败: {e}"))?;
        let mut sample: Vec<String> = Vec::new();
        let mut total = 0usize;
        for entry in statuses.iter().filter(|e| !e.status().is_ignored()) {
            total += 1;
            if sample.len() < 5 {
                match entry.path() {
                    Ok(p) => sample.push(p.to_string()),
                    Err(_) => sample.push("<非 UTF-8 路径>".to_string()),
                }
            }
        }
        if total > 0 {
            return Err(format!(
                "仓库 {} 工作区脏（{total} 项：{}）——repo_sync 拒绝在未提交状态上同步远端，请先处理",
                dest.display(),
                sample.join("、")
            ));
        }
    }

    // 目标分支解析：声明优先；缺省 = 当前分支（detached = Err）。
    let local_branch = match want {
        Some(b) => b.to_string(),
        None => head_branch_name(&repo)?,
    };

    // fetch 单 refspec（确定性 + 最小传输；凭据同 clone_repo）。
    let mut callbacks = git2::RemoteCallbacks::new();
    if let Some((user, pass)) = cred {
        callbacks.credentials(move |_url, _user_from_url, _allowed| {
            git2::Cred::userpass_plaintext(user, pass)
        });
    }
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(callbacks);
    fo.download_tags(git2::AutotagOption::None);
    let mut px = git2::ProxyOptions::new();
    apply_proxy(&mut px, proxy);
    fo.proxy_options(px);
    {
        // 步骤 url 是单一意图表达（确定性承诺：目录内容 = 该 url 远端最新）
        // ——既有 origin 指向别处（形态切换/仓库迁移）时必须先对账改写，
        // 否则 fetch 静默打到旧远端（跨远端凭据必 401，错误无从指认）。
        // 注意：Remote 对象创建时缓存自己的 URL，remote_set_url 只写 config
        // ——改写后必须重新 find_remote，fetch 才真正打新地址。
        let cur_url = {
            let remote = repo
                .find_remote("origin")
                .map_err(|e| format!("仓库 {} 没有 origin 远端: {e}", dest.display()))?;
            remote.url().unwrap_or("").to_string()
        };
        if cur_url.trim_end_matches('/') != url.trim_end_matches('/') {
            repo.remote_set_url("origin", url)
                .map_err(|e| format!("改写 origin 远端为步骤 url 失败: {e}"))?;
        }
        let mut remote = repo
            .find_remote("origin")
            .map_err(|e| format!("仓库 {} 没有 origin 远端: {e}", dest.display()))?;
        let spec = format!("+refs/heads/{local_branch}:refs/remotes/origin/{local_branch}");
        remote
            .fetch(&[spec.as_str()], Some(&mut fo), None)
            .map_err(|e| tls_hint(format!("fetch origin/{local_branch} 失败: {e}")))?;
    }

    let remote_ref_name = format!("refs/remotes/origin/{local_branch}");
    let remote_commit = repo
        .find_reference(&remote_ref_name)
        .map_err(|_| format!("远端 origin 上没有分支 `{local_branch}`（fetch 后仍缺席）"))?
        .peel_to_commit()
        .map_err(|e| format!("origin/{local_branch} 解析 commit 失败: {e}"))?;

    // 本地分支缺席：从刚 fetch 的远端物化并检出（fetch 先行，指向必新鲜）。
    if repo
        .find_branch(&local_branch, git2::BranchType::Local)
        .is_err()
    {
        repo.branch(&local_branch, &remote_commit, false)
            .map_err(|e| format!("建本地分支 {local_branch} 失败: {e}"))?;
        repo.set_head(&format!("refs/heads/{local_branch}"))
            .map_err(|e| format!("切分支 {local_branch} 失败: {e}"))?;
        repo.checkout_head(Some(&mut sync_checkout_options()))
            .map_err(|e| format!("检出 {local_branch} 失败: {e}"))?;
        return Ok((SyncOutcome::Checkout, remote_commit.id().to_string()));
    }

    // HEAD 必须停在对齐后的本地分支上（当前分支 ≠ 声明分支 → 检出切换）。
    let current_branch = head_branch_name(&repo)?;
    if current_branch != local_branch {
        repo.set_head(&format!("refs/heads/{local_branch}"))
            .map_err(|e| format!("切分支 {local_branch} 失败: {e}"))?;
        repo.checkout_head(Some(&mut sync_checkout_options()))
            .map_err(|e| format!("检出 {local_branch} 失败: {e}"))?;
    }

    let local_commit = repo
        .find_branch(&local_branch, git2::BranchType::Local)
        .map_err(|e| format!("本地分支 {local_branch} 解析失败: {e}"))?
        .get()
        .peel_to_commit()
        .map_err(|e| format!("分支 {local_branch} 解析 commit 失败: {e}"))?;

    // 合并形态判定（theirs = 刚 fetch 的远端引用）。
    let remote_ref = repo
        .find_reference(&remote_ref_name)
        .map_err(|e| format!("读远端引用失败: {e}"))?;
    let annotated = repo
        .reference_to_annotated_commit(&remote_ref)
        .map_err(|e| format!("远端引用转 annotated 失败: {e}"))?;
    let (analysis, _pref) = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| format!("合并形态判定失败: {e}"))?;
    if analysis.is_up_to_date() {
        return Ok((SyncOutcome::UpToDate, local_commit.id().to_string()));
    }

    let branch_ref = format!("refs/heads/{local_branch}");
    if analysis.is_fast_forward() {
        let mut co = sync_checkout_options();
        repo.checkout_tree(remote_commit.as_object(), Some(&mut co))
            .map_err(|e| format!("快进 checkout 失败: {e}"))?;
        repo.find_reference(&branch_ref)
            .map_err(|e| format!("读本地分支引用失败: {e}"))?
            .set_target(
                remote_commit.id(),
                &format!("repo_sync: fast-forward to origin/{local_branch}"),
            )
            .map_err(|e| format!("快进推进 {local_branch} 失败: {e}"))?;
        repo.set_head(&branch_ref)
            .map_err(|e| format!("回设 HEAD 失败: {e}"))?;
        repo.checkout_head(Some(&mut sync_checkout_options()))
            .map_err(|e| format!("快进后刷新工作区失败: {e}"))?;
        return Ok((SyncOutcome::FastForward, remote_commit.id().to_string()));
    }

    // 分叉：三方纯树合并（merge_commits 不触工作区，冲突在写树前整体暴露）。
    let mut merged_index = repo
        .merge_commits(&local_commit, &remote_commit, None)
        .map_err(|e| format!("三方合并失败: {e}"))?;
    if merged_index.has_conflicts() {
        let mut paths: Vec<String> = Vec::new();
        for conflict in merged_index
            .conflicts()
            .map_err(|e| format!("枚举冲突失败: {e}"))?
        {
            let conflict = conflict.map_err(|e| format!("读取冲突条目失败: {e}"))?;
            let src = conflict
                .our
                .as_ref()
                .or(conflict.ancestor.as_ref())
                .or(conflict.their.as_ref());
            if let Some(e) = src {
                let p = String::from_utf8_lossy(&e.path).into_owned();
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
        return Err(format!(
            "远端 origin/{local_branch} 与本地 {local_branch} 分叉且合并冲突（{} 个文件：{}）——repo_sync 不猜，请人工合并后重跑",
            paths.len(),
            paths.iter().take(5).cloned().collect::<Vec<_>>().join("、")
        ));
    }
    let tree_id = merged_index
        .write_tree_to(&repo)
        .map_err(|e| format!("写合并 tree 失败: {e}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| format!("读合并 tree 失败: {e}"))?;
    if tree_id == local_commit.tree_id() {
        // 合并树等于本地树（远端改动已被本地包含）——等价 UpToDate，不产空 commit。
        return Ok((SyncOutcome::UpToDate, local_commit.id().to_string()));
    }
    let sig = git2::Signature::now(AUTHOR_NAME, AUTHOR_EMAIL)
        .map_err(|e| format!("构造签名失败: {e}"))?;
    let msg = format!("repo_sync: merge origin/{local_branch} into {local_branch}");
    let merge_oid = repo
        .commit(
            Some(branch_ref.as_str()),
            &sig,
            &sig,
            &msg,
            &tree,
            &[&local_commit, &remote_commit],
        )
        .map_err(|e| format!("merge commit 失败: {e}"))?;
    let merge_commit = repo
        .find_commit(merge_oid)
        .map_err(|e| format!("回读 merge commit 失败: {e}"))?;
    let mut co = sync_checkout_options();
    repo.checkout_tree(merge_commit.as_object(), Some(&mut co))
        .map_err(|e| format!("同步工作区到合并结果失败: {e}"))?;
    Ok((SyncOutcome::Merged, merge_oid.to_string()))
}

// ---------------------------------------------------------------------------
// 外循环：交付分支发布原语（副本仓内重建交付的本地半边）
// ---------------------------------------------------------------------------

/// 发布链本地半边：从 HEAD 建新分支 + 应用 patch + commit，返回新 commit
/// hex。工作区**保持停在原分支原 commit**（commit 后 hard reset 回 HEAD
/// ——交付内容已安全落在发布分支上，副本工作区不留脏态）。
///
/// 失败语义（全部确定失败，仓库零残留）：
/// - 工作区有未提交/未跟踪改动 = Err（发布不混入无关改动）；
/// - 目标分支已存在：校验「单父 == 当前 HEAD 且内容与同一 patch 应用到同一
///   HEAD 的 tree 一致」→ **幂等复用**既有 commit（复评：push/
///   PR 阶段失败后重试续作，git 层不再被自己上次的分支挡死）；校验不过 =
///   Err（分支内容与本次交付不匹配，拒绝覆盖转人工——不静默吞下异源分支）；
///   探测经 index 旁路完成、工作区零触碰；
/// - patch 应用后零变更 = Err（空交付不产空 PR）；
/// - apply/commit 中途失败 = 回滚新建分支后 Err。
pub fn apply_patch_new_branch(
    root: &Path,
    branch: &str,
    patch: &[u8],
    commit_msg: &str,
) -> Result<String, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开仓库 {} 失败: {e}", root.display()))?;
    let dirty = repo
        .statuses(None)
        .map_err(|e| format!("读取工作区状态失败: {e}"))?
        .iter()
        .any(|e| !e.status().is_ignored());
    if dirty {
        return Err(format!(
            "副本工作区有未提交/未跟踪改动（{}）——发布要求干净工作区，请先处理",
            root.display()
        ));
    }
    let head = repo
        .head()
        .map_err(|e| format!("仓库无 HEAD: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("HEAD 解析 commit 失败: {e}"))?;
    if let Ok(existing) = repo.find_branch(branch, git2::BranchType::Local) {
        // 重试续作路径：分支已存在 = 上次发布的 git 半边可能已完成。校验
        // 通过（父基线一致 + 内容一致）→ 幂等复用既有 commit（上层续作
        // push/PR）；校验不过 → 维持拒绝（人工核查，不覆盖）。
        let commit = existing.get().peel_to_commit().map_err(|e| {
            format!("分支 {branch} 解析 commit 失败: {e}——上次发布残留异常，请人工核查")
        })?;
        let same_baseline = commit.parent_count() == 1
            && commit.parent_id(0).map(|p| p == head.id()).unwrap_or(false);
        let same_content =
            same_baseline && patch_tree_over_head(&repo, &head, patch)? == commit.tree_id();
        if same_content {
            tracing::info!(
                branch = %branch,
                commit = %commit.id(),
                "[git_repo] 交付分支已存在且与本次交付一致——幂等复用"
            );
            return Ok(commit.id().to_string());
        }
        return Err(format!(
            "分支 {branch} 已存在且与本次交付不一致（父基线或内容不匹配）——拒绝覆盖，请人工核查"
        ));
    }
    let branch_ref = format!("refs/heads/{branch}");
    let apply_and_commit = || -> Result<String, String> {
        let diff = git2::Diff::from_buffer(patch)
            .map_err(|e| format!("patch 解析失败: {e}（交付 patch 已损坏？）"))?;
        repo.apply(&diff, git2::ApplyLocation::WorkDir, None)
            .map_err(|e| format!("patch 应用失败: {e}（副本基线与交付基线漂移？）"))?;
        let mut index = repo.index().map_err(|e| format!("读 index 失败: {e}"))?;
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(|e| format!("暂存变更失败: {e}"))?;
        index.write().map_err(|e| format!("写 index 失败: {e}"))?;
        let tree_id = index
            .write_tree()
            .map_err(|e| format!("写 tree 失败: {e}"))?;
        if tree_id == head.tree_id() {
            return Err("patch 应用后零变更（空交付）——不产空 PR".to_string());
        }
        let tree = repo
            .find_tree(tree_id)
            .map_err(|e| format!("读 tree 失败: {e}"))?;
        let sig = git2::Signature::now(AUTHOR_NAME, AUTHOR_EMAIL)
            .map_err(|e| format!("构造签名失败: {e}"))?;
        Ok(repo
            .commit(Some(&branch_ref), &sig, &sig, commit_msg, &tree, &[&head])
            .map_err(|e| format!("commit 失败: {e}"))?
            .to_string())
    };
    // 先建分支（commit 写 ref 需要 ref 已存在），失败回滚分支保持零残留。
    repo.branch(branch, &head, false)
        .map_err(|e| format!("建分支 {branch} 失败: {e}"))?;
    match apply_and_commit() {
        Ok(commit_hex) => {
            // 工作区回到原分支状态（交付已 commit 到发布分支，不落脏态）。
            let _ = repo.reset(head.as_object(), ResetType::Hard, None);
            Ok(commit_hex)
        }
        Err(e) => {
            if let Ok(mut b) = repo.find_branch(branch, git2::BranchType::Local) {
                let _ = b.delete();
            }
            let _ = repo.reset(head.as_object(), ResetType::Hard, None);
            Err(e)
        }
    }
}

/// 纯探测：计算把 patch 应用到 head 之后的目标 tree id（**不触碰工作区**
/// ——index 从 head 树重建、应用后写 tree、随后还原 index 到 head 树）。
/// 供分支已存在时的幂等复用校验（「同一 patch 应用到同一基线 = 同一 tree」）。
fn patch_tree_over_head(
    repo: &Repository,
    head: &git2::Commit<'_>,
    patch: &[u8],
) -> Result<git2::Oid, String> {
    let diff = git2::Diff::from_buffer(patch)
        .map_err(|e| format!("patch 解析失败: {e}（交付 patch 已损坏？）"))?;
    let head_tree = head.tree().map_err(|e| format!("读 HEAD tree 失败: {e}"))?;
    let mut index = repo.index().map_err(|e| format!("读 index 失败: {e}"))?;
    index
        .read_tree(&head_tree)
        .map_err(|e| format!("index 重建失败: {e}"))?;
    index.write().map_err(|e| format!("写 index 失败: {e}"))?;
    repo.apply(&diff, git2::ApplyLocation::Index, None)
        .map_err(|e| format!("patch 应用失败: {e}（副本基线与交付基线漂移？）"))?;
    let mut index = repo.index().map_err(|e| format!("重读 index 失败: {e}"))?;
    let tree_id = index
        .write_tree()
        .map_err(|e| format!("写 tree 失败: {e}"))?;
    // 还原 index 到 HEAD 树（探测不留痕；失败无害——工作区本就停在 HEAD，
    // 下次 add_all 会重建）。
    let mut index = repo.index().map_err(|e| format!("重读 index 失败: {e}"))?;
    let _ = index.read_tree(&head_tree);
    let _ = index.write();
    Ok(tree_id)
}

/// 发布链本地半边（第三臂）：交付已并入副本 main 时（preflight 内容级等值
/// 判定：副本 HEAD 树 == 交付 payload 树），发布分支直接指向 HEAD——不套
/// patch、不产新 commit，返回 HEAD commit hex。分支已存在 = Err（与 patch
/// 路径契约一致：git 层不静默复用旧分支）。工作区脏态不检（分支指向已提交
/// 内容，不触工作区——HEAD 树已由 preflight 等值交付树）。
pub fn branch_at_head(root: &Path, branch: &str) -> Result<String, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开仓库 {} 失败: {e}", root.display()))?;
    let head = repo
        .head()
        .map_err(|e| format!("仓库无 HEAD: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("HEAD 解析 commit 失败: {e}"))?;
    if repo.find_branch(branch, git2::BranchType::Local).is_ok() {
        return Err(format!(
            "分支 {branch} 已存在——上次发布的 git 半边可能未清理，请人工核查"
        ));
    }
    repo.branch(branch, &head, false)
        .map_err(|e| format!("建分支 {branch} 失败: {e}"))?;
    Ok(head.id().to_string())
}

/// 查询分支当前指向（不修改仓库）：分支在场 → `Some((commit_hex, tree_hex))`；
/// 缺席 → `None`。供发布重试幂等判断（分支树 == 交付树 → 上次 git 半边已
/// 产物化，直接复用），仓库打开失败 → Err。
pub fn branch_tree_if_exists(
    root: &Path,
    branch: &str,
) -> Result<Option<(String, String)>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开仓库 {} 失败: {e}", root.display()))?;
    let Ok(b) = repo.find_branch(branch, git2::BranchType::Local) else {
        return Ok(None);
    };
    let commit = b
        .get()
        .peel_to_commit()
        .map_err(|e| format!("分支 {branch} 解析 commit 失败: {e}"))?;
    let tree_hex = commit.tree().map_err(|e| e.to_string())?.id().to_string();
    Ok(Some((commit.id().to_string(), tree_hex)))
}

// ---------------------------------------------------------------------------
// HEAD 树导出（基线快照；只含跟踪树）
// ---------------------------------------------------------------------------

/// HEAD 树全量导出到 `dest`（已存在的 `dest` 先清空）。返回 (文件数, 字节数)。
/// 空仓库 = Ok((0, 0))（dest 建空目录）。子模块/gitlink 跳过。
pub fn export_head_tree(root: &Path, dest: &Path) -> Result<(usize, u64), String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    if dest.exists() {
        // 韧性删除：导出目录可能刚被档案管线/索引器碰过，单发会瞬时假失败。
        nemesis_utils::fsutil::remove_dir_all_retry(dest)
            .map_err(|e| format!("清空导出目录失败: {e}"))?;
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
// 评审证据：commit 变更文件清单（F- 根修）
// ---------------------------------------------------------------------------

/// commit 相对其首个父提交的变更文件清单（`(路径, 变更类型)`，路径 `/` 分隔）。
///
/// 语义评审证据注入用（UAT F）：worker 自报的交付路径是一面之词
/// （集群 exec 沙箱路径会让评审误判文件未落盘），系统合并 commit 的实际
/// diff 才是「产物是否进入项目工作区」的客观数据源。根提交（无父）对空
/// 树 diff = 全量清单。
pub fn commit_changed_files(
    root: &Path,
    commit_oid: &str,
) -> Result<Vec<(String, String)>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    let oid = Oid::from_str(commit_oid.trim())
        .map_err(|e| format!("commit oid 非法（{commit_oid}）: {e}"))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("commit {commit_oid} 不存在: {e}"))?;
    let tree = commit
        .tree()
        .map_err(|e| format!("读 commit 树失败: {e}"))?;
    // 父树绑定在独立绑定变量上（diff 借用树对象，生命周期须覆盖 diff）。
    let parent_tree = match commit.parent_count() {
        0 => None,
        _ => {
            let parent = commit.parent(0).map_err(|e| format!("读父提交失败: {e}"))?;
            Some(parent.tree().map_err(|e| format!("读父树失败: {e}"))?)
        }
    };
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| format!("commit diff 失败: {e}"))?;
    let mut out = Vec::new();
    for delta in diff.deltas() {
        let status = match delta.status() {
            git2::Delta::Added => "新增",
            git2::Delta::Modified => "修改",
            git2::Delta::Deleted => "删除",
            git2::Delta::Renamed => "重命名",
            _ => "类型变更",
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if !path.is_empty() {
            out.push((path, status.to_string()));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 三方合并
// ---------------------------------------------------------------------------

/// 读 commit 内指定路径的 blob 文本（F-， 真机）：验收
/// 评审的证据段需要文件内容实物（变更集清单只证实「落盘」，评审员无法
/// 核对「内容」—— 实证 UNSURE 转人工的直因）。二进制 / 超限 /
/// 路径不存在 / 父 commit 删除态 = `Ok(None)` 诚实降级为只列清单。
pub fn commit_blob_text(
    root: &Path,
    commit_oid: &str,
    path: &str,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let repo =
        Repository::open(root).map_err(|e| format!("打开项目仓库 {} 失败: {e}", root.display()))?;
    let oid = Oid::from_str(commit_oid.trim())
        .map_err(|e| format!("commit oid 非法（{commit_oid}）: {e}"))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| format!("commit {commit_oid} 不存在: {e}"))?;
    let tree = commit
        .tree()
        .map_err(|e| format!("读 commit 树失败: {e}"))?;
    let entry = match tree.get_path(std::path::Path::new(path)) {
        Ok(e) => e,
        Err(_) => return Ok(None), // commit 内无此路径（删除/改名）
    };
    let object = entry
        .to_object(&repo)
        .map_err(|e| format!("读 blob 对象失败: {e}"))?;
    let Some(blob) = object.as_blob() else {
        return Ok(None); // 目录项非 blob（子模块等）
    };
    if blob.size() > max_bytes || looks_binary(blob.content()) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(blob.content()).to_string()))
}

/// 空树 oid（theirs 全删/基线空时的边界）。
fn empty_tree(repo: &Repository) -> Result<Tree<'_>, String> {
    let oid = repo
        .treebuilder(None)
        .and_then(|tb| tb.write())
        .map_err(|e| e.to_string())?;
    repo.find_tree(oid).map_err(|e| e.to_string())
}

/// 三方合并（调用方保证串行）。冲突 = 仓库零改动原样返回明细。
///
/// 入口先做路径归一化（[`normalize_repo_path`]：剥误带的工作区前缀），
/// 再全量过闸（任何一条畸形 = 拒绝整个变更集，不合入）。
///
/// 流程：ancestor = worker 声明的基线 commit（是否属于活跃 dispatch
/// 轮次由调用方先裁决，本函数只管 git 语义）→ theirs = 基线树 + 变更集
/// （内存 index 组装，不触碰盘上 index/workdir）→ `merge_trees` → 干净 =
/// commit + hard reset 同步工作区；冲突 = 收集逐文件明细（内容级二进制判定）。
pub fn merge_changeset(root: &Path, input: &MergeInput) -> Result<MergeOutcome, String> {
    // 路径归一化（压测⑤）：剥误带的 `board-projects/<项目名>/` 工作区前缀。
    let norm = |p: &str| normalize_repo_path(root, p);
    // 变更集路径先全量过闸（任何一条畸形 = 拒绝整个变更集，不合入）。
    for f in &input.upserts {
        validate_rel_path(&norm(&f.path))?;
    }
    for d in &input.deletions {
        validate_rel_path(&norm(d))?;
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
            path: norm(&f.path).into_bytes(),
        };
        theirs_index
            .add_frombuffer(&entry, &f.content)
            .map_err(|e| format!("变更集 upsert {} 进 index 失败: {e}", f.path))?;
    }
    for d in &input.deletions {
        let p = validate_rel_path(&norm(d))?;
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
            // /：三阶段内容随行带出（AI 硬解/审计明细卡的数据源；
            // blob 缺失降级 None，不致命——明细卡少一段展示而已）。
            let ancestor_data = conflict
                .ancestor
                .as_ref()
                .map(|a| blob_content(&repo, a.id));
            // 内容级二进制判定（任一侧二进制 = 不进行级合并，择边语义）。
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
            // 防逃逸（git 相对路径理论不含..，checkpoint.rs 同款防御保留）。
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
// /：冲突硬解落盘
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
mod cov_tests;
#[cfg(test)]
mod tests;
