//! BUG-20261031（bot 占用扫描目标项目 `.git\objects\pack` 致项目无法删除）
//! 回归锁。
//!
//! 根因（已确认）：旧实现 `GitBackend.repo`（`Mutex<git2::Repository>`）随
//! CheckpointStore 常驻 gateway 进程；影子库 `objects/info/alternates` 指向
//! 目标项目 objects → libgit2 首次 `begin`（index.add_all 全工作区扫描）沿
//! alternates mmap 目标项目 pack 数据文件，句柄直到 Repository drop 才释放。
//! gateway 常驻 = 句柄常驻 = 目标项目无法删除。
//!
//! 修复（方案 A，非驻留化）：GitBackend 只存 (shadow, root) 路径对 +
//! op_lock 串行锁，每个操作临时 open → 操作 → drop（libgit2 drop = munmap
//! 立即释放）。本文件两用例把「store 存活全程目标项目 pack 可改名（= 可
//! 删除）+ drop 后可真删」钉死为回归锁。Windows-only 语义（Linux 对 mmap
//! 文件删除/改名不设锁，断言只在 Windows 成立），测试 fn 自挂
//! `#[cfg(windows)]`（Windows 形态测试标记约定）。

use super::*;

/// 造一个带真实 pack 的目标项目：git init + 两次 commit（git2）+
/// `git gc`（子进程把 loose objects 打包成 `.git/objects/pack/pack-*`，
/// repack -d 会移除冗余 loose——夹具保证 pack 在场且 loose 已清）。
fn packed_target_project() -> (tempfile::TempDir, std::path::PathBuf) {
    let guard = tempfile::tempdir().unwrap();
    let root = guard.path().join("target-project");
    std::fs::create_dir_all(&root).unwrap();
    let repo = git2::Repository::init(&root).unwrap();
    commit_file(&repo, "src/a.rs", "fn a() {}\n", "c1");
    commit_file(&repo, "src/b.rs", "fn b() {}\n", "c2");
    drop(repo);
    let st = std::process::Command::new("git")
        .args(["gc", "-q"])
        .current_dir(&root)
        .status()
        .expect("git 子进程执行失败（git 必须在 PATH）");
    assert!(st.success(), "git gc 失败: {st}");
    let packs = pack_files(&root);
    assert!(
        !packs.is_empty(),
        "夹具失败：git gc 后 .git/objects/pack 无 pack 文件"
    );
    (guard, root)
}

fn commit_file(repo: &git2::Repository, rel: &str, body: &str, message: &str) {
    let workdir = repo.workdir().unwrap();
    let path = workdir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, body).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(rel)).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = git2::Signature::now("pack-lock-test", "pack-lock@test.invalid").unwrap();
    let parents = repo
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .into_iter()
        .collect::<Vec<_>>();
    let parent_refs = parents.iter().collect::<Vec<_>>();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .unwrap();
}

/// 列目标项目 `.git/objects/pack` 下的 pack/idx（有序消费 read_dir 显式排序）。
fn pack_files(root: &Path) -> Vec<PathBuf> {
    let pack_dir = root.join(".git").join("objects").join("pack");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&pack_dir) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if let Some(ext) = p.extension().and_then(|e| e.to_str())
                && (ext == "pack" || ext == "idx")
            {
                out.push(p);
            }
        }
    }
    out
}

/// rename 探针：被 mmap/占用的文件在 Windows 上 rename 失败（非破坏性）。
/// 成功 = 未锁定（立即移回原位），Err = 锁定中。
fn rename_probe(p: &Path) -> std::io::Result<()> {
    let probe = p.with_extension("probe_tmp");
    std::fs::rename(p, &probe)?;
    std::fs::rename(&probe, p)?;
    Ok(())
}

/// 对全部 pack 文件跑探针，返回锁定文件名列表（空 = 全部可动）。
fn probe_locked(stage: &str, files: &[PathBuf]) -> Vec<String> {
    let mut locked = Vec::new();
    for f in files {
        if rename_probe(f).is_err() {
            locked.push(f.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    eprintln!(
        "[pack_lock_probe] stage={stage} locked={}/{} {:?}",
        locked.len(),
        files.len(),
        locked
    );
    locked
}

#[test]
#[cfg(windows)]
fn shadow_store_must_not_hold_target_pack_files() {
    let (guard, root) = packed_target_project();
    // 影子库落临时目录（模拟 L6++ 的 logs/project_checkpoints 形态：影子在
    // 项目外，root 仅作 .git 探测锚）。
    let shadow = guard.path().join("shadow.git");

    // 阶段 1：装配（repo open + alternates 后端初始化）
    let store = CheckpointStore::new_with_shadow(None, root.clone(), shadow);
    assert!(
        matches!(store.backend(), CheckpointBackend::Git),
        "git 影子库后端未装配（回落 JSON），测试前提不成立"
    );
    let files = pack_files(&root);
    let locked_assembly = probe_locked("after_new_with_shadow", &files);

    // 阶段 2：turn 边界（write_current_tree：add_all + write_tree + commit）
    store.begin(1, "scan target project");
    let locked_begin1 = probe_locked("after_begin_1", &files);

    // 阶段 3：第二轮（commit 带 parent）
    store.begin(2, "scan continue");
    let locked_begin2 = probe_locked("after_begin_2", &files);

    assert!(
        locked_assembly.is_empty() && locked_begin1.is_empty() && locked_begin2.is_empty(),
        "BUG 复现：store 存活期间目标项目 pack 被锁定——assembly={locked_assembly:?} \
         begin1={locked_begin1:?} begin2={locked_begin2:?}"
    );

    // 归属对照：drop 后必须可真删（证明句柄来自 store，而非外部持有者）。
    drop(store);
    for f in pack_files(&root) {
        std::fs::remove_file(&f).unwrap_or_else(|e| panic!("drop 后删除失败 {}: {e}", f.display()));
    }
}

/// drop 语义锁：store 存活期不锁（非驻留化的直接断言面），drop(store) 后
/// mmap 释放、目标项目 pack 可**真删除**（归属对照——句柄确曾来自 store
/// 的调用链而非外部持有者）。与上一用例互补：那则钉「多次 begin 全程不
/// 锁」，本则钉「begin 后 drop 即可真删」。
#[test]
#[cfg(windows)]
fn dropping_store_releases_target_pack_files() {
    let (guard, root) = packed_target_project();
    let shadow = guard.path().join("shadow.git");

    let store = CheckpointStore::new_with_shadow(None, root.clone(), shadow);
    store.begin(1, "scan target project");
    let locked = probe_locked("before_drop", &pack_files(&root));
    assert!(
        locked.is_empty(),
        "回归：store 存活期目标项目 pack 被锁定（非驻留化被破坏）——{locked:?}"
    );

    drop(store);
    // 给 OS 一拍 munmap 落地窗口（正常为同步释放；轮询 3 次容限）。
    let mut released = false;
    for _ in 0..3 {
        let files = pack_files(&root);
        if files.iter().all(|f| rename_probe(f).is_ok()) {
            released = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(
        released,
        "drop(store) 后 pack 仍被锁定——非驻留化方案前提不成立"
    );
    for f in pack_files(&root) {
        std::fs::remove_file(&f).unwrap_or_else(|e| panic!("drop 后删除失败 {}: {e}", f.display()));
    }
}
