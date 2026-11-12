use super::*;
use std::fs;
use std::path::PathBuf;

/// 隔离临时目录（进程内唯一后缀，测试结束不清理——系统临时区自回收）。
fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xtask-copytree-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("有父目录")).expect("建父目录");
    fs::write(path, content).expect("写文件");
}

/// 模拟仓库树：根 bin 产物区、web/node_modules、.git 目录（根）、submodule
/// 挂载点里的 .git 指针**文件**、嵌套 target、正常源码，各形态一种。
fn seed_repo_like(src: &Path) {
    write_file(&src.join("Cargo.toml"), "[workspace]");
    write_file(&src.join("nemesisbot/src/main.rs"), "fn main() {}");
    write_file(&src.join("crates/foo/src/lib.rs"), "pub fn f() {}");
    // 根 bin/（SKIP_ROOT_ONLY）
    write_file(&src.join("bin/nemesisbot.exe"), "MZfake");
    // .git 目录（SKIP_ANY_DEPTH）
    write_file(&src.join(".git/HEAD"), "ref: refs/heads/main");
    // web/node_modules（SKIP_ANY_DEPTH 任意深度）
    write_file(&src.join("web/node_modules/vue/index.js"), "// dep");
    // 嵌套 target
    write_file(&src.join("crates/foo/target/debug/x.bin"), "artifact");
    // submodule 挂载点 .git 指针文件（SKIP_ANY_DEPTH 按名命中文件形态跳过）
    write_file(
        &src.join("slots/taiwei/impl/.git"),
        "gitdir: ../../.git/modules/x",
    );
    write_file(
        &src.join("slots/taiwei/impl/crates/taiwei-impl/src/lib.rs"),
        "pub fn t() {}",
    );
    // __pycache__
    write_file(&src.join("scripts/__pycache__/tool.pyc"), "bytecode");
}

#[test]
fn first_sync_copies_tree_and_skips_rules() {
    let root = temp_root("first");
    let src = root.join("src");
    let dst = root.join("dst");
    seed_repo_like(&src);

    let stats = sync_repo(&src, &dst).expect("同步");

    // 保留面
    assert!(dst.join("Cargo.toml").is_file());
    assert!(dst.join("nemesisbot/src/main.rs").is_file());
    assert!(dst.join("crates/foo/src/lib.rs").is_file());
    // 槽目录本体保留，只有 .git 指针文件被跳过
    assert!(
        dst.join("slots/taiwei/impl/crates/taiwei-impl/src/lib.rs")
            .is_file()
    );
    assert!(
        !dst.join("slots/taiwei/impl/.git").exists(),
        "submodule .git 指针必须跳过"
    );
    // 跳过面
    assert!(!dst.join("bin").exists(), "根 bin/ 必须跳过");
    assert!(!dst.join(".git").exists());
    assert!(!dst.join("web/node_modules").exists());
    assert!(!dst.join("crates/foo/target").exists());
    assert!(!dst.join("scripts/__pycache__").exists());

    let copied_expected = 4; // Cargo.toml + main.rs + crates lib.rs + impl lib.rs
    assert_eq!(stats.copied, copied_expected, "实际复制面：{stats:?}");
    assert!(stats.warnings.is_empty());
}

#[test]
fn second_sync_is_noop_and_mtime_is_preserved() {
    let root = temp_root("second");
    let src = root.join("src");
    let dst = root.join("dst");
    seed_repo_like(&src);
    sync_repo(&src, &dst).expect("首同步");

    // mtime 保持（cargo 增量缓存的前提）
    let a = fs::metadata(src.join("crates/foo/src/lib.rs")).expect("src meta");
    let b = fs::metadata(dst.join("crates/foo/src/lib.rs")).expect("dst meta");
    assert_eq!(a.modified().ok(), b.modified().ok(), "复制必须保持源 mtime");

    // 无变化的二轮同步 = 零复制（增量快路径）
    let stats = sync_repo(&src, &dst).expect("二同步");
    assert_eq!(stats.copied, 0, "未变文件不得重复制：{stats:?}");
    assert_eq!(stats.skipped_unchanged, 4);
}

#[test]
fn changed_file_recopies_and_untouched_stays() {
    let root = temp_root("changed");
    let src = root.join("src");
    let dst = root.join("dst");
    seed_repo_like(&src);
    sync_repo(&src, &dst).expect("首同步");

    // 改一个文件：size+mtime 双变
    let touched = src.join("crates/foo/src/lib.rs");
    write_file(&touched, "pub fn g() {} // changed");
    // mtime 精度兜底：显式把源 mtime 推后（有些 FS 时间粒度粗，秒内写入会撞等值）
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    fs::OpenOptions::new()
        .write(true)
        .open(&touched)
        .expect("打开")
        .set_modified(future)
        .expect("推 mtime");

    let stats = sync_repo(&src, &dst).expect("三同步");
    assert_eq!(stats.copied, 1, "只有被改文件重复制：{stats:?}");
    assert_eq!(
        fs::read_to_string(dst.join("crates/foo/src/lib.rs")).expect("读回"),
        "pub fn g() {} // changed"
    );
}

#[test]
fn root_bin_skip_does_not_hit_nested_src_bin() {
    let root = temp_root("srcbin");
    let src = root.join("src");
    let dst = root.join("dst");
    // installer/core/src/bin 是合法源码目录（[[bin]] 目标）——根级 bin 跳过
    // 规则不得波及嵌套源码 bin
    write_file(&src.join("installer/core/src/bin/tool.rs"), "fn main() {}");
    write_file(&src.join("bin/artifact.exe"), "MZ");

    let stats = sync_repo(&src, &dst).expect("同步");
    assert!(
        dst.join("installer/core/src/bin/tool.rs").is_file(),
        "嵌套 src/bin 必须保留"
    );
    assert!(!dst.join("bin").exists(), "根 bin 必须跳过");
    assert_eq!(stats.copied, 1);
}

/// BUG-20261112-123 回归锁：镜像删除（同步三态的第三态）。仓库侧删除的
/// 文件/目录在二轮同步时从复制体摘除——修复前 sync 只加不删，include_dir!
/// 嵌入面（workspace/ 模板与技能、static/ 资产）对「删了但复制体还留着」
/// 完全失明：增量装配把陈旧幽灵文件烤进商业交付物（onboard 复活已删模板/
/// 技能、web 伺服幽灵资产），无警告无检测。
#[test]
fn prune_removes_entries_deleted_from_src() {
    let root = temp_root("prune-del");
    let src = root.join("src");
    let dst = root.join("dst");
    seed_repo_like(&src);
    // 额外摆两个将来被删的条目：文件一枚 + 目录子树一枚（嵌入面同构）。
    write_file(&src.join("crates/foo/src/extra.rs"), "pub fn e() {}");
    write_file(
        &src.join("nemesisbot/workspace/skills/old-skill/SKILL.md"),
        "x",
    );

    sync_repo(&src, &dst).expect("首同步");
    assert!(dst.join("crates/foo/src/extra.rs").is_file());
    assert!(
        dst.join("nemesisbot/workspace/skills/old-skill/SKILL.md")
            .is_file()
    );

    // 仓库侧删除。
    fs::remove_file(src.join("crates/foo/src/extra.rs")).expect("删文件");
    fs::remove_dir_all(src.join("nemesisbot/workspace/skills/old-skill")).expect("删目录");

    let stats = sync_repo(&src, &dst).expect("二同步");
    assert!(
        !dst.join("crates/foo/src/extra.rs").exists(),
        "仓库侧删除的文件必须镜像摘除"
    );
    assert!(
        !dst.join("nemesisbot/workspace/skills/old-skill").exists(),
        "仓库侧删除的目录必须整子树镜像摘除"
    );
    assert_eq!(stats.pruned_files, 1, "{stats:?}");
    assert_eq!(stats.pruned_dirs, 1, "{stats:?}");
    // 既有保留面不受误伤（删除只命中 src 缺席的名字）。
    assert!(dst.join("crates/foo/src/lib.rs").is_file());
    assert!(dst.join("nemesisbot/src/main.rs").is_file());
}

/// 跳过名单不参与镜像删除（同一豁免口径）：复制体侧的 skip 名单条目即便
/// src 缺席也不摘——构建产物/依赖树/git 元数据从未同步，不归同步管。
#[test]
fn prune_respects_skip_names() {
    let root = temp_root("prune-skip");
    let src = root.join("src");
    let dst = root.join("dst");
    seed_repo_like(&src);
    sync_repo(&src, &dst).expect("首同步");
    // 人为在复制体侧造一个 src 缺席的 skip 名单目录（模拟历史遗留产物）。
    write_file(&dst.join("web/node_modules/legacy/x.js"), "// stale");

    let stats = sync_repo(&src, &dst).expect("二同步");
    assert!(
        dst.join("web/node_modules/legacy/x.js").is_file(),
        "skip 名单内的 dst 条目不得被镜像删除"
    );
    assert_eq!(stats.pruned_dirs, 0, "{stats:?}");
    assert_eq!(stats.pruned_files, 0, "{stats:?}");
}

/// 改名 = 删除+新增的一轮同步双效应：旧名镜像摘除、新名照常复制——同一
/// 同步调用内两态都生效，无「先加后删」的中间态要求。
#[test]
fn rename_is_delete_plus_add_in_one_pass() {
    let root = temp_root("prune-rename");
    let src = root.join("src");
    let dst = root.join("dst");
    seed_repo_like(&src);
    write_file(&src.join("crates/foo/src/old.rs"), "pub fn o() {}");
    sync_repo(&src, &dst).expect("首同步");
    assert!(dst.join("crates/foo/src/old.rs").is_file());

    // 仓库侧改名 old.rs → new.rs。
    fs::rename(
        src.join("crates/foo/src/old.rs"),
        src.join("crates/foo/src/new.rs"),
    )
    .expect("改名");

    let stats = sync_repo(&src, &dst).expect("二同步");
    assert!(!dst.join("crates/foo/src/old.rs").exists(), "旧名必须摘除");
    assert!(
        dst.join("crates/foo/src/new.rs").is_file(),
        "新名必须照常复制"
    );
    assert_eq!(stats.pruned_files, 1, "{stats:?}");
    assert_eq!(stats.copied, 1, "{stats:?}");
}
