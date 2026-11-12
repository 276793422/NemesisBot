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
