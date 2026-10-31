//! 产品性质回归锁：被 watch 的项目目录必须保持可删。
//!
//! 产品原则：bot 不得在用户文件系统路径上持有任何驻留锁——用户随时删除
//! 被 watch 的项目目录必须是合法事件（L6++「目录消失 → lenient 降级」
//! 同一条原则的另一面：目录消失前，删除动作本身也必须能成功）。
//!
//! 2026-10-31 实证结论（本测试即为实证载体）：notify `recommended_watcher`
//! （Windows 后端 = ReadDirectoryChangesW）watch 在场期间，目录内的文件、
//! 嵌套子目录、目录本体全部可正常删除（句柄以含 FILE_SHARE_DELETE 的
//! 共享模式打开，RemoveDirectoryW 成功；此前「目录句柄必钉住目录」的
//! 推断被本探针推翻——Windows 目录句柄语义取决于打开方 share mode，
//! 不是无条件硬锁）。
//!
//! 本测试把该性质钉死为回归锁：未来更换 watcher 后端 / 递归实现 / 共享
//! 模式时，一旦引入目录钉住立即红。Windows-only 语义，测试 fn 自挂
//! `#[cfg(windows)]`（Windows 形态测试标记约定）。

use super::start_with_debounce;
use nemesis_config::FsWatcherConfig;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn noop_callbacks() -> (
    std::sync::Arc<dyn Fn() + Send + Sync>,
    std::sync::Arc<dyn Fn(&str) + Send + Sync>,
) {
    (Arc::new(|| {}), Arc::new(|_: &str| {}))
}

/// 清空目录下的直接文件子项（平面夹具全为文件；有序消费 read_dir 不需
/// 排序——删除动作与遍历顺序无关）。
fn clear_files(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::remove_file(&p)
                .unwrap_or_else(|e| panic!("文件删除失败 {}: {e}", p.display()));
        }
    }
}

#[test]
#[cfg(windows)]
fn watched_directory_must_stay_deletable() {
    let guard = tempfile::tempdir().unwrap();
    let root = guard.path().join("target-project");
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    std::fs::write(sub.join("inner.txt"), "i").unwrap();

    let (on_instruction, on_external) = noop_callbacks();
    let cfg = FsWatcherConfig {
        enabled: true,
        ignore: Vec::new(),
    };
    let handle = start_with_debounce(
        &root,
        &cfg,
        Duration::from_millis(50),
        on_instruction,
        on_external,
    )
    .expect("watcher 启动失败（测试前提不成立）")
    .expect("cfg.enabled=true 应返回 Some(handle)");

    // 先造一次真实事件，确保 watch 管线活体（句柄确定在手）。
    std::fs::write(root.join("b.txt"), "y").unwrap();

    // 1) 文件删除（watch 在场）：必须可删。
    clear_files(&root);

    // 2) 嵌套子目录删除（watch 在场）：递归 watch 不得在子目录上持句柄。
    clear_files(&sub);
    std::fs::remove_dir(&sub)
        .unwrap_or_else(|e| panic!("watch 在场嵌套子目录不可删（性质被破坏）: {e}"));

    // 3) 目录本体删除（watch 在场）：真实用户「删项目目录」的最后一步
    //    （子项清空后 RemoveDirectory）。性质断言 = 必须成功。
    let rm = std::fs::remove_dir(&root);
    eprintln!("[watcher_deletable_probe] watch 在场 remove_dir(root)={rm:?}");
    assert!(
        rm.is_ok(),
        "性质回归：watch 在场项目根目录不可删除（watcher 引入了目录钉住）——{rm:?}"
    );

    // 4) drop 后目录确认消失（NotFound = 已删成功，正常）。
    drop(handle);
    assert!(!root.exists(), "drop 后项目目录仍存在——删除语义未达成");
}
