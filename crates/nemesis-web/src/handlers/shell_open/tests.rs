//! shell_open 单测（纯函数面——spawn 臂与看门线程会真开窗口，对齐
//! agt_tests「spawn 臂不进单测」豁免纪律；前台看门决策逻辑是纯函数，
//! 直接钉住）。

use super::win::pick_focus_target;

fn leaves(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// 看门决策：新出现的 CabinetWClass 窗口优先（本次开窗产物）。
#[test]
fn pick_focus_new_window_wins() {
    let before = vec![(100isize, "旧窗口".to_string())];
    let now = vec![
        (100isize, "旧窗口".to_string()),
        (200isize, "新建文件夹".to_string()),
    ];
    let ls = leaves(&["docs"]);
    assert_eq!(pick_focus_target(&before, &now, &ls, false), Some(200));
}

/// 看门决策：预算半程前不允许回落臂（explorer 慢启动窗口尚未出现时
/// 不误切到无关旧窗）。
#[test]
fn pick_focus_no_fallback_before_halfway() {
    let before = vec![(100isize, "别的目录".to_string())];
    let now = vec![(100isize, "别的目录".to_string())];
    let ls = leaves(&["docs"]);
    // 无新窗 + 未到回落时点 → None（继续轮询）。
    assert_eq!(pick_focus_target(&before, &now, &ls, false), None);
}

/// 看门决策回落臂：explorer 复用旧窗（无新窗出现）时，标题含目标候选名
/// 的已知窗被拉前台。
#[test]
fn pick_focus_fallback_matches_leaf_title() {
    let before = vec![
        (100isize, "别的目录".to_string()),
        (300isize, "myproj".to_string()),
    ];
    let now = vec![
        (100isize, "别的目录".to_string()),
        (300isize, "myproj".to_string()),
    ];
    let ls = leaves(&["myproj"]);
    // 未到回落时点：不切。
    assert_eq!(pick_focus_target(&before, &now, &ls, false), None);
    // 到点：复用窗命中。
    assert_eq!(pick_focus_target(&before, &now, &ls, true), Some(300));
}

/// 看门决策回落臂：reveal（选中文件）场景候选 = [文件叶名, 父目录名]，
/// 窗口标题（父目录名）经第二候选命中。
#[test]
fn pick_focus_fallback_parent_leaf_hits() {
    let before = vec![
        (100isize, "别的目录".to_string()),
        (300isize, "reports".to_string()),
    ];
    let now = vec![
        (100isize, "别的目录".to_string()),
        (300isize, "reports".to_string()),
    ];
    let ls = leaves(&["a.md", "reports"]);
    assert_eq!(pick_focus_target(&before, &now, &ls, true), Some(300));
}

/// 看门决策：候选全空禁用回落臂（根盘等无候选形态不误切）。
#[test]
fn pick_focus_empty_leaves_disable_fallback() {
    let before = vec![(100isize, "C:".to_string())];
    let now = vec![(100isize, "C:".to_string())];
    assert_eq!(pick_focus_target(&before, &now, &[], true), None);
}

/// 看门决策：回落臂候选不命中 = None（继续等/放弃），不误切无关窗。
#[test]
fn pick_focus_fallback_leaf_miss_returns_none() {
    let before = vec![(100isize, "别的目录".to_string())];
    let now = vec![(100isize, "别的目录".to_string())];
    let ls = leaves(&["docs"]);
    assert_eq!(pick_focus_target(&before, &now, &ls, true), None);
}

/// 看门决策：空集边界（第一次开 explorer，before 空、now 有新窗）。
#[test]
fn pick_focus_first_ever_window() {
    let now = vec![(42isize, "notes".to_string())];
    let ls = leaves(&["notes"]);
    assert_eq!(pick_focus_target(&[], &now, &ls, false), Some(42));
}
