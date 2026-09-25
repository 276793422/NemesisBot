// archive.rs 覆盖率补充测试（wave6：default_directory 100 候选名耗尽的
// 诚实报错臂 97-100）。

use super::*;

/// 自动分配目录：基础名与 -2..-100 后缀候选全部被盘上占用 → 诚实报错，
/// 不静默换名（97-100）。
#[test]
fn w6_default_directory_exhaustion_is_honest_error() {
    let ws = std::env::temp_dir().join(format!(
        "nmb-archive-w6-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&ws);
    let projects = ws.join(BOARD_PROJECTS_DIR);
    std::fs::create_dir_all(projects.join("proj")).unwrap();
    for n in 2..=100 {
        std::fs::create_dir_all(projects.join(format!("proj-{n}"))).unwrap();
    }
    let err = default_directory(&ws, "proj", &[]).unwrap_err();
    assert!(err.contains("100 个候选名全部被占用"), "{err}");

    // 腾出一个中段候选（-57）→ 立刻可用，报错不是粘性的。
    std::fs::remove_dir_all(projects.join("proj-57")).unwrap();
    let got = default_directory(&ws, "proj", &[]).unwrap();
    assert!(got.ends_with("proj-57"), "{got:?}");

    let _ = std::fs::remove_dir_all(&ws);
}

/// resolve_project_directory 的 requested=None 形态同样走 default_directory
///（157 的 if 关闭括号所在函数主干；重叠拒绝臂已在既有测试覆盖）。
#[test]
fn w6_resolve_project_directory_auto_allocates_under_workspace() {
    let ws = std::env::temp_dir().join(format!(
        "nmb-archive-w6r-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let got = resolve_project_directory(None, &ws, "demo", &[]).unwrap();
    let expected = ws.join(BOARD_PROJECTS_DIR).join("demo");
    assert!(
        expected.is_dir(),
        "分配即 mkdir：{got:?} 应落在 {}",
        expected.display()
    );
    let norm = got.to_string_lossy().replace('\\', "/");
    assert!(
        norm.contains("board-projects/demo"),
        "自动分配路径形态：{got:?}"
    );
    let _ = std::fs::remove_dir_all(&ws);
}
