//! P4 交付门（delivery_gate.rs）测试。
//!
//! - JSON 形态（无 .git）→ None（诚实降级，不造证据）。
//! - 空 task_id → None。
//! - git 形态 happy path：JSON 形状（branch/task_id/node/patch_path +
//!   capture 元数据）+ patch 落盘留底内容一致 + P2 版本绑定
//!   （checks_binds / repo_base_sha）。
//! - 坏 base tree hex → None（warn 不 panic，不阻断回调主链路）。
//! - 有效 hex 但影子库不认识的 base tree（跨仓库形态）→ None。

use super::*;
use nemesis_agent::checkpoint::CheckpointStore;
use std::sync::Arc;

/// 带真 git 仓的临时 workspace（同 nemesis-agent p4_tests 夹具语义）。
fn git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

/// 在工作区根真仓库建首个 commit（unborn HEAD → 有 HEAD，repo_base_sha 可测）。
fn seed_commit(root: &std::path::Path) -> String {
    std::fs::write(root.join("seed.txt"), "seed\n").unwrap();
    let repo = git2::Repository::open(root).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("seed.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])
        .unwrap();
    oid.to_string()
}

#[test]
fn json_backend_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(CheckpointStore::new(None, dir.path().to_path_buf()));
    assert_eq!(
        store.backend(),
        nemesis_agent::checkpoint::CheckpointBackend::Json
    );
    let out =
        capture_task_delivery_with_store(&store, "json-task", None, "node-b", Vec::new(), None);
    assert!(out.is_none(), "JSON 形态无影子库 → None（诚实降级）");
}

#[test]
fn empty_task_id_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(CheckpointStore::new(None, dir.path().to_path_buf()));
    assert!(
        capture_task_delivery_with_store(&store, "", None, "node-b", Vec::new(), None).is_none()
    );
}

#[test]
fn happy_path_json_shape_and_patch_file() {
    let (_d, root) = git_root();
    std::fs::write(root.join("a.txt"), "v1\n").unwrap();
    // P2：先落首个 commit——交付带 repo_base_sha（版本链「基于哪个 SHA」）。
    let repo_sha = seed_commit(&root);
    let store = Arc::new(CheckpointStore::new(None, root.clone()));
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().expect("git 形态有实时 tree");

    // 任务执行：改旧文件 + 增新文件。
    std::fs::write(root.join("a.txt"), "v2\nmore\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("new.rs"), "fn main() {}\n").unwrap();

    // ②typed checks 块：一条真实执行过的检查记录随 wire 上行。
    let checks = vec![CheckRecord {
        tool: "exec".into(),
        cmd: "cargo test -p nemesisbot".into(),
        ok: true,
        summary: "test result: ok. 5 passed".into(),
    }];
    // P2 版本绑定：checks 窗口关闭时刻的实时 tree（生产语义 = take 后同源
    // 快照；测试里 take 即 capture，恒等于 capture 时刻 tree）。
    let checks_head_tree = store.current_tree_hex();
    let out = capture_task_delivery_with_store(
        &store,
        "t-happy",
        Some(&base),
        "node-b",
        checks,
        checks_head_tree.as_deref(),
    )
    .expect("git 形态成功捕获");
    let obj = out.as_object().expect("delivery 是 JSON object");

    // 上下文字段。
    assert_eq!(obj["branch"], "run-t-happy");
    assert_eq!(obj["task_id"], "t-happy");
    assert_eq!(obj["node"], "node-b");
    assert_eq!(obj["patch_path"], "logs/deliveries/t-happy.patch");

    // capture 元数据（DeliveryBranchCapture 序列化字段）。
    assert_eq!(obj["ref_name"], "refs/heads/nemesis/run-t-happy");
    assert_eq!(obj["base_tree"], base.as_str());
    assert_eq!(obj["files_changed"], 2, "1 改 1 增");
    assert_eq!(obj["patch_truncated"], false);
    let patch = obj["patch"].as_str().expect("patch 是字符串");
    assert!(patch.contains("a.txt"));
    assert!(patch.contains("src/new.rs"));

    // ②typed checks 块 wire 透传（shape = CheckRecord 序列化字段）。
    let checks = obj["checks"].as_array().expect("checks 是数组");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0]["tool"], "exec");
    assert_eq!(checks[0]["cmd"], "cargo test -p nemesisbot");
    assert_eq!(checks[0]["ok"], true);
    assert_eq!(checks[0]["summary"], "test result: ok. 5 passed");

    // P2 版本绑定：binds 与 capture 元数据交叉一致（base 对 base、head 对
    // capture tree）；repo_base_sha = 真 git 仓库 HEAD commit。
    let binds = obj["checks_binds"].as_object().expect("checks_binds 在场");
    assert_eq!(binds["base_tree"], base.as_str());
    assert_eq!(
        binds["head_tree"], obj["tree"],
        "checks 关窗 tree = capture tree（同一快照点）"
    );
    assert_eq!(obj["repo_base_sha"], repo_sha.as_str());

    // patch 落盘留底与 wire JSON 一致（正斜杠相对路径）。
    let on_disk =
        std::fs::read_to_string(root.join("logs").join("deliveries").join("t-happy.patch"))
            .expect("patch 已落盘");
    assert_eq!(on_disk, patch, "盘上留底 = wire 证据");

    // commit 可在影子库解到同一 tree。
    let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
    let repo = git2::Repository::open_ext(
        &shadow,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::path::Path::new("/"),
    )
    .unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(obj["commit"].as_str().unwrap()).unwrap())
        .unwrap();
    assert_eq!(commit.tree_id().to_string(), obj["tree"]);
}

/// P2 诚实缺席：非 git 真仓库（unborn HEAD）→ repo_base_sha 键不出现；
/// base/head 双 None（JSON 形态调用方语义）→ checks_binds 键不出现（加法
/// 演进不破旧形态）。
#[test]
fn unborn_head_and_absent_binds_are_honest_absence() {
    let (_d, root) = git_root();
    std::fs::write(root.join("a.txt"), "v1\n").unwrap();
    let store = Arc::new(CheckpointStore::new(None, root.clone()));
    std::fs::write(root.join("a.txt"), "v2\n").unwrap();

    let out =
        capture_task_delivery_with_store(&store, "t-unborn", None, "node-b", Vec::new(), None)
            .expect("git 形态成功捕获（None 基线 = 空树整包 diff）");
    let obj = out.as_object().expect("delivery 是 JSON object");
    assert!(
        obj.get("repo_base_sha").is_none(),
        "unborn HEAD（init 后无 commit）→ repo_base_sha 键不出现"
    );
    assert!(
        obj.get("checks_binds").is_none(),
        "双缺席（JSON 形态调用方语义）→ checks_binds 键不出现"
    );
}

#[test]
fn bad_base_tree_hex_is_none_not_panic() {
    let (_d, root) = git_root();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();
    let store = Arc::new(CheckpointStore::new(None, root));
    // 坏 hex（非 40 位）：capture 层响亮失败 → 交付门兜成 None + warn。
    let out = capture_task_delivery_with_store(
        &store,
        "t-badbase",
        Some("deadbeef"),
        "node-b",
        Vec::new(),
        None,
    );
    assert!(out.is_none(), "坏基线 = None（不造证据不炸链路）");
}

/// 有效 40 位 hex 但对象不在本影子库（跨仓库 base 形态：tree 存在于别的
/// 仓库，这里解不到）→ capture 失败 → None（不 panic 不造证据）。
#[test]
fn unknown_base_tree_object_is_none() {
    let (_d, root) = git_root();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();
    let store = Arc::new(CheckpointStore::new(None, root));
    let foreign = "a".repeat(40); // 合法 hex、几乎不可能在影子库里
    let out = capture_task_delivery_with_store(
        &store,
        "t-foreign-base",
        Some(foreign.as_str()),
        "node-b",
        Vec::new(),
        None,
    );
    assert!(out.is_none(), "跨仓库 base tree = None（诚实失败）");
}

/// G4 截断契约（2026-10-18 批六）：patch 超 `PATCH_MAX_BYTES` 截断时，
/// wire payload `patch_truncated=true` 必须伴随非空 `patch_path`（完整
/// patch 在 worker 本地留底、`deliveries.patch` 可指路取回——截断不等于
/// 丢失证据线索）。消费方（交付账本/验收审计）据此可以信赖该不变量。
#[test]
fn truncated_patch_still_carries_patch_path() {
    let (_d, root) = git_root();
    std::fs::write(root.join("big.txt"), "x\n").unwrap();
    let store = Arc::new(CheckpointStore::new(None, root.clone()));
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().expect("git 形态有实时 tree");

    // 单行 ~1.2MB 新增内容 > 1MB 上限 → 截断（diff 含 -x/+yy…y 两行 ≈
    // 载荷字节数，必须实打实超 PATCH_MAX_BYTES）。
    let big = "y".repeat(1_200_000) + "\n";
    std::fs::write(root.join("big.txt"), &big).unwrap();

    let out = capture_task_delivery_with_store(
        &store,
        "t-trunc",
        Some(&base),
        "node-b",
        Vec::new(),
        None,
    )
    .expect("git 形态成功捕获");
    let obj = out.as_object().expect("delivery 是 JSON object");
    assert_eq!(obj["patch_truncated"], true, "1.2MB diff 必须触发截断");
    // ②typed checks 块诚实缺席：没跑检查 = 键不出现（绝不造空壳）。
    assert!(obj.get("checks").is_none(), "空 checks 不得出现在 wire");
    let path = obj["patch_path"].as_str().expect("patch_path 在场");
    assert!(
        !path.trim().is_empty(),
        "截断时 patch_path 必填（留底可取回）"
    );
    // 盘上留底 = wire 截断后原文（一字节不差，证据链自洽）。
    let on_disk =
        std::fs::read_to_string(root.join("logs").join("deliveries").join("t-trunc.patch"))
            .expect("patch 已落盘");
    let wire_patch = obj["patch"].as_str().expect("patch 是字符串");
    assert_eq!(on_disk, wire_patch, "盘上留底 = wire 证据");
    assert!(
        wire_patch.len() <= nemesis_agent::checkpoint::PATCH_MAX_BYTES,
        "wire patch 不超上限"
    );
}

/// ensure_exec_repo（BUG 2026-11-01 根修第一半）：首次进入定格基线 commit
/// ——lib.rs 进基线、sidecar/_debug 不进；排除文件就位；任务期间改文件后
/// 重入 = 同一 HEAD（基线恒定，是交付 diff 的稳定锚点）。
#[test]
fn ensure_exec_repo_freezes_baseline_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let exec = dir.path().join("exec");
    std::fs::create_dir_all(&exec).unwrap();
    std::fs::write(exec.join("lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(exec.join(".baseline.json"), "{}").unwrap();
    std::fs::create_dir_all(exec.join("_debug")).unwrap();
    std::fs::write(exec.join("_debug").join("probe.txt"), "x").unwrap();

    let sha1 = ensure_exec_repo(&exec).expect("首次就绪成功");
    {
        let repo = git2::Repository::open(&exec).unwrap();
        let commit = repo
            .find_commit(git2::Oid::from_str(&sha1).unwrap())
            .expect("返回 SHA 可解析为 commit");
        let tree = commit.tree().unwrap();
        assert!(tree.get_name("lib.rs").is_some(), "基线含 lib.rs");
        assert!(
            tree.get_name(".baseline.json").is_none(),
            "sidecar 不进基线"
        );
        assert!(
            tree.get_path(std::path::Path::new("_debug")).is_err(),
            "_debug 不进基线"
        );
        assert_eq!(
            repo.head().unwrap().peel_to_commit().unwrap().id(),
            commit.id(),
            "HEAD = 基线定格 commit"
        );
    }
    let exclude = std::fs::read_to_string(exec.join(".git").join("info").join("exclude")).unwrap();
    assert!(exclude.contains(".baseline.json"), "排除文件含 sidecar");
    assert!(exclude.contains("_debug/"), "排除文件含 _debug/");

    // 任务窗口 worker 改文件 → 重入不重新定格（幂等，读持久化锚点）。
    std::fs::write(exec.join("lib.rs"), "fn a() -> i32 { 1 }\n").unwrap();
    let sha2 = ensure_exec_repo(&exec).expect("重入成功");
    assert_eq!(sha1, sha2, "重入复用持久化锚点，基线恒定");

    // BUG 2026-11-01 C 后半回归锁：worker 合法 commit 移动 HEAD（T1 实证
    // 形态）→ 重入仍返回定格基线，不跟随 HEAD 漂移。
    {
        let repo = git2::Repository::open(&exec).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["**"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("worker", "worker@local").unwrap();
        let parent = repo
            .find_commit(git2::Oid::from_str(&sha1).unwrap())
            .unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "worker fix", &tree, &[&parent])
            .unwrap();
    }
    let head_after = git2::Repository::open(&exec)
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    assert_ne!(head_after, sha1, "前提：HEAD 已被 worker commit 移动");
    let sha3 = ensure_exec_repo(&exec).expect("worker 提交后重入成功");
    assert_eq!(sha1, sha3, "重入取持久化锚点，不跟随 HEAD 漂移");

    // 锚点文件落在 .git 内（不进树不进 diff 的位置纪律）。
    assert!(
        exec.join(".git").join("nemesis_baseline").is_file(),
        "基线锚点持久化在 gitdir 内"
    );
}

/// exec 路径端到端（BUG 2026-11-01 根修第二半）：真实改动进 patch；构建
/// 产物/调试目录/sidecar 不进；wire 与主路径同形（checks/checks_binds/
/// repo_base_sha）；patch 落**主 workspace**（副本内无 logs/）；影子库用后
/// 即清；repo_base_sha = 基线 commit。
#[test]
fn exec_capture_end_to_end_clean_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let exec = dir.path().join("exec");
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&exec).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(exec.join("lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(exec.join(".baseline.json"), "{}").unwrap();
    let base_sha = ensure_exec_repo(&exec).expect("基线就绪");
    let baseline_tree_hex = {
        let repo = git2::Repository::open(&exec).unwrap();
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree_id()
            .to_string()
    };

    // 任务窗口：worker 真实改动 + 构建产物 + 调试文件。
    std::fs::write(exec.join("lib.rs"), "fn a() -> i32 { 1 }\n").unwrap();
    std::fs::create_dir_all(exec.join("target").join("debug")).unwrap();
    std::fs::write(exec.join("target").join("debug").join("app.exe"), "MZfake").unwrap();
    std::fs::create_dir_all(exec.join("_debug")).unwrap();
    std::fs::write(exec.join("_debug").join("probe.txt"), "x").unwrap();

    let checks = vec![CheckRecord {
        tool: "exec".into(),
        cmd: "cargo test".into(),
        ok: true,
        summary: "test result: ok. 1 passed".into(),
    }];
    let out = capture_task_delivery_exec(&exec, &ws, "t-exec", "node-b", checks, Some(&base_sha))
        .expect("exec 路径捕获成功");
    let obj = out.as_object().expect("delivery 是 JSON object");

    // wire 形状与主路径同形。
    assert_eq!(obj["branch"], "run-t-exec");
    assert_eq!(obj["task_id"], "t-exec");
    assert_eq!(obj["node"], "node-b");
    assert_eq!(
        obj["base_tree"],
        baseline_tree_hex.as_str(),
        "base = 副本基线 commit 的 tree"
    );
    assert_eq!(
        obj["repo_base_sha"],
        base_sha.as_str(),
        "版本链基于副本基线 commit"
    );
    assert_eq!(obj["files_changed"], 1, "只有 lib.rs 一个真实改动");
    assert_eq!(obj["patch_path"], "logs/deliveries/t-exec.patch");

    // patch 内容：真实改动在场，三类垃圾全部不在。
    let patch = obj["patch"].as_str().expect("patch 是字符串");
    assert!(patch.contains("lib.rs"), "真实改动进 patch");
    assert!(!patch.contains("target"), "构建产物不进 patch");
    assert!(!patch.contains("app.exe"));
    assert!(!patch.contains("_debug"), "调试目录不进 patch");
    assert!(!patch.contains(".baseline.json"), "sidecar 不进 patch");

    // ②typed checks + P2 绑定随 wire 上行。
    let wire_checks = obj["checks"].as_array().expect("checks 是数组");
    assert_eq!(wire_checks.len(), 1);
    assert_eq!(wire_checks[0]["cmd"], "cargo test");
    assert_eq!(
        obj["checks_binds"]["base_tree"],
        baseline_tree_hex.as_str(),
        "检查窗口绑定基线 tree"
    );
    assert!(
        obj["checks_binds"]["head_tree"].is_string(),
        "head 绑定在场"
    );

    // patch 留底落主 workspace（副本交付后整树删除，留底必须在外部）。
    let on_disk = std::fs::read_to_string(ws.join("logs").join("deliveries").join("t-exec.patch"))
        .expect("patch 已落主 workspace");
    assert_eq!(on_disk, patch, "盘上留底 = wire 证据");
    assert!(
        !exec.join("logs").exists(),
        "副本内不得留 patch（副本交付后整树删除）"
    );

    // 影子库用后即清（临时证据，wire JSON + patch 留底是持久载体）。
    assert!(
        !ws.join("logs")
            .join("exec_checkpoints")
            .join("t-exec.git")
            .exists(),
        "影子库捕获完成即清"
    );
}

/// BUG 2026-11-01 C 后半回归锁：worker 合法 commit 移动 HEAD（T1 实证
/// async 续行形态）后，捕获仍以定格基线 diff——base_tree=基线 tree、
/// files_changed≥1、repo_base_sha=基线 commit（不再「基于自己」）。
/// 同时锁自愈链：透传参数缺席时从 `.git/nemesis_baseline` 持久锚点取回。
#[test]
fn exec_capture_survives_worker_commit_with_frozen_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let exec = dir.path().join("exec");
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&exec).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(exec.join("lib.rs"), "fn a() {}\n").unwrap();
    let base_sha = ensure_exec_repo(&exec).expect("基线就绪");

    // worker 合法 commit（T1 实证：任务中途提交移动 HEAD）。
    std::fs::write(exec.join("lib.rs"), "fn a() -> i32 { 1 }\n").unwrap();
    {
        let repo = git2::Repository::open(&exec).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["**"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("worker", "worker@local").unwrap();
        let parent = repo
            .find_commit(git2::Oid::from_str(&base_sha).unwrap())
            .unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "worker fix", &tree, &[&parent])
            .unwrap();
    }

    // 生产形态：ensure 捕获的基线 SHA 透传 → diff 面向定格基线。
    let out = capture_task_delivery_exec(
        &exec,
        &ws,
        "t-worker-commit",
        "node-b",
        Vec::new(),
        Some(&base_sha),
    )
    .expect("worker 提交后捕获成功");
    let obj = out.as_object().expect("delivery 是 JSON object");
    assert_eq!(
        obj["files_changed"], 1,
        "基线锚定后 worker 改动可见（不因 HEAD 移动而恒空）"
    );
    assert_eq!(
        obj["repo_base_sha"],
        base_sha.as_str(),
        "版本链基于定格基线，非 worker 自己的 commit"
    );
    assert_ne!(
        obj["commit"], obj["base_tree"],
        "交付 commit 是 worker 的，基线 tree 是定格的"
    );
    let patch = obj["patch"].as_str().expect("patch 是字符串");
    assert!(patch.contains("lib.rs"), "真实改动进 patch");

    // 自愈链：透传缺席 → `.git/nemesis_baseline` 持久锚点取回同一基线。
    let healed =
        capture_task_delivery_exec(&exec, &ws, "t-anchor-selfheal", "node-b", Vec::new(), None)
            .expect("持久锚点自愈捕获成功");
    let healed_obj = healed.as_object().expect("delivery 是 JSON object");
    assert_eq!(
        healed_obj["base_tree"], obj["base_tree"],
        "透传缺席时从持久锚点取回同一基线 tree"
    );
    assert_eq!(
        healed_obj["repo_base_sha"],
        base_sha.as_str(),
        "自愈路径 repo_base_sha 同样锚定基线 commit"
    );
}
