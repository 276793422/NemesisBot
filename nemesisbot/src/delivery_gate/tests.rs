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
