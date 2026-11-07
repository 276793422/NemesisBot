//! （持久 Agent 实体 -c，2026-10-17）：worker 侧交付分支捕获。
//!
//! PLAN「分支/PR 」的 B 端半边：任务执行前记基线 tree（git 形态才有
//! ——JSON 形态无影子库，诚实跳过不造证据），成功收口时把工作区实时 tree
//! 捕获为交付分支（`refs/heads/nemesis/run-<task_id>`，force 覆盖——重派
//! 重跑同任务同分支），patch 落盘 `logs/deliveries/<task_id>.patch` 留底，
//! capture 元数据 + patch 全文随回调 `delivery` 字段上行（≤1MB，
//! PATCH_MAX_BYTES 截断位）。A 端（master）把整包入 deliveries 表——
//! 证据包自包含，跨节点可审无需二次 RPC。
//!
//! 诚实边界：是增强证据面，任何失败（无.git / 影子库不可用 /
//! patch 写盘失败）= `None` + warn，绝不阻断任务回调主链路（回调才是
//! 结果交付的唯一路径；证据缺席可观测但不停摆）。

use nemesis_agent::checks_capture::CheckRecord;
use nemesis_agent::r#loop::AgentLoop;

/// 任务执行前的基线 tree 快照（git 形态才有；JSON 形态 = None）。
///
/// 调用方在 `run_with_trace`/`resume_execution_with_token` **之前**取值，
/// 成功收口时原样传给 [`capture_task_delivery`]——先执行后快照会把任务
/// 自己的改动算进基线（diff 恒空，证据失真）。
#[cfg(feature = "checkpoint-git")]
pub(crate) fn delivery_base_tree(agent_loop: &AgentLoop) -> Option<String> {
    agent_loop
        .checkpoint_store()
        .and_then(|store| store.current_tree_hex())
}

/// checkpoint-git 关：无影子库，基线恒 None（JSON 形态语义）。
#[cfg(not(feature = "checkpoint-git"))]
pub(crate) fn delivery_base_tree(_agent_loop: &AgentLoop) -> Option<String> {
    None
}

/// 成功收口时的交付分支捕获（B 端）：capture + patch 落盘 + wire JSON。
///
/// `base_tree` 传 [`delivery_base_tree`] 的执行前快照；`None` = 执行前就
/// 无基线（JSON 形态 / 影子库异常）→ 按空树基线整包 diff（capture 层
/// canonical 语义）。`checks` = 任务窗口内检查类工具的执行证据
/// （`checks_capture` 捕获，cluster_agent run 前后 begin/take；空 = 没跑
/// 过检查，诚实缺席不造值）。`checks_head_tree` = checks 窗口关闭时刻的
/// 实时影子 tree（take 后同源快照； 版本绑定——与 capture 时刻的
/// `tree` 独立派生，评审门交叉对账「检查就是在交付那棵树上跑的」）。
///
/// 返回 payload["delivery"] 值：`DeliveryBranchCapture` 序列化 + `branch`
/// /`task_id`/`node`/`patch_path`/`checks`/`checks_binds`/`repo_base_sha`
/// 上下文字段。任何失败 = None + warn。
#[cfg(feature = "checkpoint-git")]
pub(crate) fn capture_task_delivery(
    agent_loop: &AgentLoop,
    task_id: &str,
    base_tree: Option<&str>,
    self_node_id: &str,
    checks: Vec<CheckRecord>,
    checks_head_tree: Option<&str>,
) -> Option<serde_json::Value> {
    let store = agent_loop.checkpoint_store()?;
    capture_task_delivery_with_store(
        &store,
        task_id,
        base_tree,
        self_node_id,
        checks,
        checks_head_tree,
    )
}

/// checkpoint-git 关：是 git 影子库增强证据面——恒 None。模块头
/// 诚实边界语义不变：证据缺席可观测但不停摆（回调主链路照常）。
#[cfg(not(feature = "checkpoint-git"))]
pub(crate) fn capture_task_delivery(
    _agent_loop: &AgentLoop,
    _task_id: &str,
    _base_tree: Option<&str>,
    _self_node_id: &str,
    _checks: Vec<CheckRecord>,
    _checks_head_tree: Option<&str>,
) -> Option<serde_json::Value> {
    None
}

/// 工作区根若是真实 git 仓库（任务专属 clone 等），取 HEAD commit SHA
/// ——交付版本链的「基于哪个 SHA」。 unborn HEAD（init 后无提交）/ 非
/// 仓库 / 打开失败 = None（诚实缺席，不造值）。
#[cfg(feature = "checkpoint-git")]
fn repo_head_sha(root: &std::path::Path) -> Option<String> {
    let repo = git2::Repository::open_ext(
        root,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::path::Path::new("/"),
    )
    .ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

/// exec 副本基线仓库本地排除面（`.git/info/exclude`——仓库本地文件不进
/// 工作树，零 diff 噪声）。只列 [`nemesis_path::is_workspace_ignored`]
/// 回调**不认识**的名字：基线 sidecar 与 `_debug/`（exec 管线自产）；
/// `target/`/`node_modules/` 等生态缓存由回调在 add_all 时排除（任意层级
/// ——与 capture 写树同一剪枝真相源，基线 ⊆ capture 树，diff 无幻影删除）。
#[cfg(feature = "checkpoint-git")]
const EXEC_REPO_EXCLUDE: &str = "# nemesisbot exec 基线仓库：管线自产物排除。生态缓存目录
# （target/ 等）由 checkpoint 写树回调按 is_workspace_ignored 排除，无需在此重复。
.baseline.json
_debug/
";

/// 档案任务执行前就绪 exec 副本的 git 基线仓库（交付账本
/// 空记录根修第一半：worker 全部文件改动发生在 `cluster/exec/<task_id>/`
/// 副本里，副本是裸文件树，主 workspace 影子库 JSON 形态捕获恒缺席）。
///
/// 首次进入 = `git init` + 钉 `core.autocrlf=false`（字节保真，先钉后动，
/// 同 `git_repo.rs::ensure_repo` 纪律；否则全局 autocrlf=true 机器上 add
/// 时 CRLF→LF 重写）+ 写仓库本地排除文件 + `index.add_all` 定格**基线
/// commit**——add 回调复用 `is_workspace_ignored`（与 checkpoint 写树同一
/// 剪枝真相源）加管线自产物。基线 SHA 同时持久化到 `.git/nemesis_baseline`
/// （gitdir 内文件，天然不进树不进 diff）——async 续行/进程重启后重入仍能
/// 取回真基线：worker 合法 commit 会移动 HEAD（实证），HEAD 当基线 =
/// diff 恒空失真（C 后半）。续行/重派重入 = 优先读持久化
/// 锚点，可解析即复用；锚点缺席（升级边缘）回落 HEAD + 诚实注记。config
/// 与排除文件每次重钉（幂等）。
///
/// 返回基线 commit SHA；任何失败 = Err（调用方 warn 后任务照常——交付
/// 证据缺席可观测，不阻断主链路）。
#[cfg(feature = "checkpoint-git")]
pub(crate) fn ensure_exec_repo(exec_dir: &std::path::Path) -> Result<String, String> {
    let dot_git = exec_dir.join(".git");
    if dot_git.exists() && !dot_git.is_dir() {
        // linked worktree / submodule 的.git 文件形态：照 checkpoint.rs
        // 同一裁决，不碰（init/open 语义歧义，诚实放弃）。
        return Err(format!(
            ".git 为文件形态（worktree 链接？），exec 基线仓库不可用：{}",
            dot_git.display()
        ));
    }
    let repo = if dot_git.is_dir() {
        git2::Repository::open_ext(
            exec_dir,
            git2::RepositoryOpenFlags::NO_SEARCH,
            std::path::Path::new("/"),
        )
        .map_err(|e| format!("exec 基线仓库打开失败: {e}"))?
    } else {
        git2::Repository::init(exec_dir).map_err(|e| format!("exec 基线仓库 init 失败: {e}"))?
    };

    // 行尾保真：先钉 local false 再动 index（clone/init 自带 checkout 的
    // smudge 坑在 init 场景不存在——init 无 checkout，但 add 路径仍会读
    // 配置，必须先钉）。
    {
        let mut cfg = repo
            .config()
            .map_err(|e| format!("exec 基线仓库 config 打开失败: {e}"))?;
        cfg.set_bool("core.autocrlf", false)
            .map_err(|e| format!("exec 基线仓库 config 钉 autocrlf 失败: {e}"))?;
    }

    // 仓库本地排除（管线自产物不进基线）。写失败不致命：捕获侧 add 回调
    // 仍剪掉同名路径（双保险的第二道依然 effective），只 warn。
    let info_dir = dot_git.join("info");
    if let Err(e) = std::fs::create_dir_all(&info_dir)
        .and_then(|()| std::fs::write(info_dir.join("exclude"), EXEC_REPO_EXCLUDE))
    {
        tracing::warn!(
            error = %e,
            "[DeliveryGate] exec 排除文件写入失败（捕获回调仍兜底）"
        );
    }

    // 重入锚点：优先持久化锚点（定格时落盘，见上）——HEAD 可能已被
    // worker 合法 commit 移动（实证：async 续行形态 worker 中途提交），
    // 不再是稳定锚点；锚点缺席（升级边缘）回落 HEAD + 诚实注记。
    let anchor_path = dot_git.join("nemesis_baseline");
    if let Ok(sha) = std::fs::read_to_string(&anchor_path) {
        let sha = sha.trim().to_string();
        if let Ok(oid) = git2::Oid::from_str(&sha)
            && repo.find_commit(oid).is_ok()
        {
            return Ok(sha);
        }
        tracing::warn!("[DeliveryGate] exec 基线锚点不可解析（仓库损坏？），回落 HEAD 探测");
    }
    if let Ok(head) = repo.head()
        && let Ok(commit) = head.peel_to_commit()
    {
        tracing::warn!(
            sha = %commit.id(),
            "[DeliveryGate] exec 无持久基线锚（升级边缘），以当前 HEAD 为基线——若任务期间 worker 已提交，基线已漂移"
        );
        return Ok(commit.id().to_string());
    }

    // 首次定格：基线 commit。pathspec 必须 `["**"]`——空 pathspec + 回调 =
    // libgit2 空指针崩（checkpoint.rs 坑注同源）。
    let mut index = repo
        .index()
        .map_err(|e| format!("exec 基线 index 打开失败: {e}"))?;
    index
        .add_all(
            ["**"],
            git2::IndexAddOption::DEFAULT,
            Some(&mut |path: &std::path::Path, _spec: &[u8]| {
                if path == std::path::Path::new(".baseline.json")
                    || path.starts_with(std::path::Path::new("_debug"))
                    || nemesis_path::is_workspace_ignored(path)
                {
                    1 // skip
                } else {
                    0 // add
                }
            }),
        )
        .map_err(|e| format!("exec 基线 index 写入失败: {e}"))?;
    index
        .write()
        .map_err(|e| format!("exec 基线 index 落盘失败: {e}"))?;
    let tree_oid = index
        .write_tree()
        .map_err(|e| format!("exec 基线 tree 写入失败: {e}"))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|e| format!("exec 基线 tree 读取失败: {e}"))?;
    let sig = git2::Signature::now("nemesisbot", "nemesisbot@local")
        .map_err(|e| format!("exec 基线签名构造失败: {e}"))?;
    let commit_oid = repo
        .commit(
            Some("HEAD"),
            &sig,
            &sig,
            "exec baseline: 档案工作副本基线定格",
            &tree,
            &[],
        )
        .map_err(|e| format!("exec 基线 commit 失败: {e}"))?;

    // 基线锚点持久化（.git 内文件：天然不进树不进 diff，读路径见重入块）
    // ——续行/进程重启后重入仍能取回真基线。失败只 warn（下次重入回落
    // HEAD 探测，证据面降级不阻断）。
    if let Err(e) = std::fs::write(&anchor_path, commit_oid.to_string()) {
        tracing::warn!(
            error = %e,
            "[DeliveryGate] exec 基线锚点落盘失败（重入将回落 HEAD 探测）"
        );
    }
    Ok(commit_oid.to_string())
}

/// exec 副本路径的交付捕获（根修第二半）：改动在
/// `cluster/exec/<task_id>/` 副本里，主 workspace 影子库（JSON 形态）捕获
/// 恒缺席——本函数从副本就地 diff。
///
/// 1. 基线锚定三链（C 后半：worker 合法 commit 移动 HEAD，
///    HEAD 当基线 = base==head、diff 恒空， 实证 files_changed=0）：
///    透传 `baseline_sha` → `.git/nemesis_baseline` 持久锚点（
///    [`ensure_exec_repo`] 定格时落盘）→ HEAD（旧形态 fallback + warn）；
///    作用域块内取完即关——libgit2 句柄不驻留， 同族纪律；
/// 2. 临时影子库 `logs/exec_checkpoints/<task_id>.git` 挂副本工作树
///    （`CheckpointStore::new_with_shadow` 复用 checkpoint.rs capture/
///    diff/patch/files 单一权威定义；alternates 直读副本对象库，零拷贝）；
///    影子库自钉 `core.autocrlf=false`（fresh bare init 无键 → libgit2
///    回落系统级 true → add 时 CRLF→LF 重写 → 字节失真伪 diff， smudge
///    同族）+ 本地排除文件副本一份（影子是独立 gitdir，副本仓库的 exclude
///    对它不可见；捕获侧回调已剪大头，这是双保险）；
/// 3. capture（base=副本基线 tree，head=副本实时树——同一剪枝真相源，
///    diff 无幻影删除）→ wire JSON 与主路径同一组装函数；
/// 4. patch 留底落**主 workspace**（副本交付后整树删除，留底必须在外部；
///    `repo_base_sha` 显式传基线 commit——HEAD 探测会被 worker 合法提交
///    移动，探测即失真「基于自己」）；影子库用后即清（成功失败都清——
///    临时证据，wire JSON + patch 留底是持久载体）。
///
/// 任何失败 = None + warn（交付证据缺席可观测，回调主链路照常）。
#[cfg(feature = "checkpoint-git")]
pub(crate) fn capture_task_delivery_exec(
    exec_dir: &std::path::Path,
    workspace_root: &std::path::Path,
    task_id: &str,
    self_node_id: &str,
    checks: Vec<CheckRecord>,
    baseline_sha: Option<&str>,
) -> Option<serde_json::Value> {
    if task_id.is_empty() {
        return None;
    }
    let branch = format!("run-{task_id}");

    // ① 基线 tree + repo_base_sha：定格基线 commit（透传参数 → 持久锚点 →
    // HEAD fallback）。作用域块内完成，句柄不跨捕获。
    let (base_tree, repo_base_sha) = {
        let repo = match git2::Repository::open_ext(
            exec_dir,
            git2::RepositoryOpenFlags::NO_SEARCH,
            std::path::Path::new("/"),
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    task_id = %task_id,
                    error = %e,
                    "[DeliveryGate] exec 副本仓库打开失败（交付证据缺席，回调照常）"
                );
                return None;
            }
        };
        let resolve = |sha: &str| -> Option<(String, String)> {
            let commit = repo.find_commit(git2::Oid::from_str(sha).ok()?).ok()?;
            Some((commit.tree_id().to_string(), commit.id().to_string()))
        };
        let mut resolved = baseline_sha.and_then(resolve);
        if resolved.is_none() {
            // 持久锚点自愈：透传缺席（未来调用点漏接线）仍可从定格落盘取回。
            let anchor = std::fs::read_to_string(exec_dir.join(".git").join("nemesis_baseline"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            resolved = anchor.and_then(|a| resolve(&a));
        }
        if resolved.is_none()
            && let Ok(head) = repo.head()
            && let Ok(c) = head.peel_to_commit()
        {
            tracing::warn!(
                task_id = %task_id,
                "[DeliveryGate] exec 交付捕获无基线锚（旧形态），回落 HEAD——若任务期间 worker 已提交，diff 失真"
            );
            resolved = Some((c.tree_id().to_string(), c.id().to_string()));
        }
        match resolved {
            Some(pair) => pair,
            None => {
                tracing::warn!(
                    task_id = %task_id,
                    "[DeliveryGate] exec 副本仓库无基线 commit（ensure_exec_repo 未就绪？），交付证据缺席"
                );
                return None;
            }
        }
    };

    // ② 临时影子库挂副本工作树。
    let shadow_root = nemesis_path::logs_dir_in_workspace(workspace_root)
        .join("exec_checkpoints")
        .join(format!("{task_id}.git"));
    if let Some(parent) = shadow_root.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            task_id = %task_id,
            error = %e,
            "[DeliveryGate] exec 影子库目录创建失败（交付证据缺席，回调照常）"
        );
        return None;
    }
    let store = std::sync::Arc::new(nemesis_agent::checkpoint::CheckpointStore::new_with_shadow(
        None,
        exec_dir.to_path_buf(),
        shadow_root.clone(),
    ));
    let cleanup = || nemesis_utils::fsutil::remove_dir_all_resilient(&shadow_root);
    if store.backend() != nemesis_agent::checkpoint::CheckpointBackend::Git {
        tracing::warn!(
            task_id = %task_id,
            "[DeliveryGate] exec 影子库回落 JSON 形态（init 失败？），交付证据缺席"
        );
        cleanup();
        return None;
    }

    // 影子库行尾保真 + 本地排除（失败两类分档：config 钉定失败 = 字节
    // 失真风险，诚实放弃；排除文件失败不致命，回调兜底）。
    {
        let shadow_repo = match git2::Repository::open_ext(
            &shadow_root,
            git2::RepositoryOpenFlags::NO_SEARCH,
            std::path::Path::new("/"),
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    task_id = %task_id,
                    error = %e,
                    "[DeliveryGate] exec 影子库打开失败（交付证据缺席，回调照常）"
                );
                cleanup();
                return None;
            }
        };
        let pinned = shadow_repo
            .config()
            .and_then(|mut c| c.set_bool("core.autocrlf", false));
        if let Err(e) = pinned {
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "[DeliveryGate] exec 影子库行尾保真钉定失败（交付证据缺席，回调照常）"
            );
            cleanup();
            return None;
        }
        let shadow_info = shadow_root.join("info");
        let _ = std::fs::create_dir_all(&shadow_info);
        let _ = std::fs::write(shadow_info.join("exclude"), EXEC_REPO_EXCLUDE);
    }

    // ③ capture：base = 副本基线 tree，head = 副本实时树。
    let cap = match store.capture_delivery_branch(Some(base_tree.as_str()), &branch) {
        Ok(cap) => cap,
        Err(e) => {
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "[DeliveryGate] exec 交付分支捕获失败（证据缺席，回调照常）"
            );
            cleanup();
            return None;
        }
    };
    let checks_head_tree = store.current_tree_hex();

    // ④ wire 组装 + patch 留底（主 workspace）+ repo_base_sha（定格基线
    // commit，显式传值——HEAD 探测会被 worker 合法提交移动）。
    let payload = delivery_payload_with_persist(
        cap,
        workspace_root,
        task_id,
        &branch,
        self_node_id,
        checks,
        Some(base_tree.as_str()),
        checks_head_tree.as_deref(),
        Some(repo_base_sha.as_str()),
    );
    cleanup();
    Some(payload)
}

/// checkpoint-git 关（裁剪形态）：exec 基线仓库整族不编译，诚实 Err
/// （调用方 warn 后任务照常——交付证据缺席可观测，不阻断主链路）。
#[cfg(not(feature = "checkpoint-git"))]
pub(crate) fn ensure_exec_repo(_exec_dir: &std::path::Path) -> Result<String, String> {
    Err("checkpoint-git feature 未编译，exec 基线仓库不可用".to_string())
}

/// checkpoint-git 关（裁剪形态）：exec 副本交付捕获恒 None。
#[cfg(not(feature = "checkpoint-git"))]
pub(crate) fn capture_task_delivery_exec(
    _exec_dir: &std::path::Path,
    _workspace_root: &std::path::Path,
    _task_id: &str,
    _self_node_id: &str,
    _checks: Vec<CheckRecord>,
    _baseline_sha: Option<&str>,
) -> Option<serde_json::Value> {
    None
}

/// [`capture_task_delivery`] 的可测内核：直接收影子库句柄（测试用
/// `CheckpointStore::new(None, root)` + 真实 git 夹具驱动，不必造 AgentLoop）。
#[cfg(feature = "checkpoint-git")]
pub(crate) fn capture_task_delivery_with_store(
    store: &std::sync::Arc<nemesis_agent::checkpoint::CheckpointStore>,
    task_id: &str,
    base_tree: Option<&str>,
    self_node_id: &str,
    checks: Vec<CheckRecord>,
    checks_head_tree: Option<&str>,
) -> Option<serde_json::Value> {
    if task_id.is_empty() {
        return None;
    }
    let branch = format!("run-{task_id}");
    let cap = match store.capture_delivery_branch(base_tree, &branch) {
        Ok(cap) => cap,
        Err(e) => {
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "[DeliveryGate] 交付分支捕获失败（证据缺席，回调照常）"
            );
            return None;
        }
    };
    // 主路径：patch 留底落 workspace；repo_base_sha 探测 workspace 仓库 HEAD。
    Some(delivery_payload_with_persist(
        cap,
        store.root(),
        task_id,
        &branch,
        self_node_id,
        checks,
        base_tree,
        checks_head_tree,
        repo_head_sha(store.root()).as_deref(),
    ))
}

/// wire JSON 组装 + patch 留底的单一组装函数（主路径与 exec 副本路径共用
/// ——`delivery` 字段逐字段一致，账本/评审门对两侧同形消费）。
/// `patch_root` = patch 留底落点（`logs/deliveries/<task_id>.patch` 相对
/// 该根申报 wire）；`repo_base_sha` = 显式版本链「基于哪个 SHA」（主路径 =
/// workspace 仓库 HEAD 探测；exec 路径 = 定格基线 commit——HEAD 会被
/// worker 合法提交移动，探测即失真「基于自己」， C 后半；
/// None = 键不出现，诚实缺席）。
#[cfg(feature = "checkpoint-git")]
#[allow(clippy::too_many_arguments)]
fn delivery_payload_with_persist(
    cap: nemesis_agent::checkpoint::DeliveryBranchCapture,
    patch_root: &std::path::Path,
    task_id: &str,
    branch: &str,
    self_node_id: &str,
    checks: Vec<CheckRecord>,
    base_tree: Option<&str>,
    checks_head_tree: Option<&str>,
    repo_base_sha: Option<&str>,
) -> serde_json::Value {
    // patch 落盘留底（worker 本地证据，logs/deliveries/<task_id>.patch）。
    // 失败只 warn——patch 已随 wire JSON 上行，A 端 deliveries 表自包含。
    let patch_path = nemesis_path::logs_dir_in_workspace(patch_root)
        .join("deliveries")
        .join(format!("{task_id}.patch"));
    let patch_display = patch_path
        .strip_prefix(patch_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| format!("logs/deliveries/{task_id}.patch"));
    if let Some(dir) = patch_path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(
            task_id = %task_id,
            error = %e,
            "[DeliveryGate] patch 目录创建失败（wire 证据不受影响）"
        );
    } else if let Err(e) = std::fs::write(&patch_path, cap.patch.as_bytes()) {
        tracing::warn!(
            task_id = %task_id,
            error = %e,
            "[DeliveryGate] patch 落盘失败（wire 证据不受影响）"
        );
    }

    tracing::info!(
        task_id = %task_id,
        branch = %branch,
        files_changed = cap.files_changed,
        insertions = cap.insertions,
        deletions = cap.deletions,
        patch_bytes = cap.patch.len(),
        truncated = cap.patch_truncated,
        "[DeliveryGate] 交付分支已捕获"
    );

    let mut value = serde_json::to_value(&cap).unwrap_or_else(|_| {
        // DeliveryBranchCapture 是纯数据结构，序列化实际不会失败——兜底臂
        // 保类型完整（unwrap 免疫纪律）。
        serde_json::json!({
            "ref_name": cap.ref_name,
            "commit": cap.commit,
            "tree": cap.tree,
            "base_tree": cap.base_tree,
            "patch_truncated": cap.patch_truncated,
        })
    });
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "branch".into(),
            serde_json::Value::String(branch.to_string()),
        );
        obj.insert(
            "task_id".into(),
            serde_json::Value::String(task_id.to_string()),
        );
        obj.insert(
            "node".into(),
            serde_json::Value::String(self_node_id.to_string()),
        );
        obj.insert(
            "patch_path".into(),
            serde_json::Value::String(patch_display),
        );
        // typed checks 块：任务窗口内检查类工具的执行证据。空 = 没跑过
        // 检查（诚实缺席——文档类职能的合法形态，绝不造空壳条目）。
        if !checks.is_empty()
            && let Ok(v) = serde_json::to_value(&checks)
        {
            obj.insert("checks".into(), v);
        }
        // 版本绑定：checks 窗口的基线/关闭 tree 与 capture
        // 时刻的 `base_tree`/`tree` 独立派生——评审门交叉对账「这些检查就
        // 是在交付的那棵树上跑的」。两者都缺席（JSON 形态调用方）= 键不出
        // 现（诚实缺席，加法演进不破旧 payload 形态）。
        if base_tree.is_some() || checks_head_tree.is_some() {
            let mut binds = serde_json::Map::new();
            if let Some(b) = base_tree {
                binds.insert("base_tree".into(), serde_json::Value::String(b.to_string()));
            }
            if let Some(h) = checks_head_tree {
                binds.insert("head_tree".into(), serde_json::Value::String(h.to_string()));
            }
            obj.insert("checks_binds".into(), serde_json::Value::Object(binds));
        }
        // 版本链「基于哪个 SHA」：显式传值（主路径 = workspace 仓库 HEAD
        // 探测；exec 路径 = 定格基线 commit——HEAD 会被 worker 合法提交移动，
        // 路径探测即失真「基于自己」， C 后半）。None = 键不
        // 出现（诚实缺席，不造值）。
        if let Some(sha) = repo_base_sha {
            obj.insert(
                "repo_base_sha".into(),
                serde_json::Value::String(sha.to_string()),
            );
        }
    }
    value
}

/// checkpoint-git 关（minimal-iot / desktop 等裁剪形态）：影子库 git 代码面
/// 整族不编译（`checkpoint.rs` 交付族随门裁剪）——按模块诚实边界恒
/// None（证据缺席可观测但不停摆，回调主链路零变化）。
#[cfg(not(feature = "checkpoint-git"))]
pub(crate) fn capture_task_delivery_with_store(
    store: &std::sync::Arc<nemesis_agent::checkpoint::CheckpointStore>,
    task_id: &str,
    base_tree: Option<&str>,
    self_node_id: &str,
    _checks: Vec<CheckRecord>,
    _checks_head_tree: Option<&str>,
) -> Option<serde_json::Value> {
    let _ = (store, task_id, base_tree, self_node_id);
    None
}

#[cfg(all(test, feature = "checkpoint-git"))]
mod tests;
