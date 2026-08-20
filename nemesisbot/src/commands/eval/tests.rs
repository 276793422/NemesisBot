//! eval.rs 单测。Windows-only 实现的纯函数测试（box 镜像推导 / 熔断换算）。

use super::*;

// ---------------------------------------------------------------------------
// box_mirror_for —— Sandboxie 盒内镜像路径推导（2026-08-21 修复的回归钉）
// ---------------------------------------------------------------------------

/// 在磁盘上搭一个盒镜像布局并验证 home 映射到预期镜像路径。
/// exists() 探测语义：只有搭出来的那个镜像存在时才返回它。
fn t() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn mirror_under_user_profile_uses_user_tree() {
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    // 盒内已存在 user-tree 镜像（正常 %TEMP% 在 profile 下的形态）。
    let mirrored = box_root.join("user").join("current").join("AppData")
        .join("Local").join("Temp").join(".tmpX");
    std::fs::create_dir_all(&mirrored).unwrap();
    let home = Path::new(r"C:\Users\zoo\AppData\Local\Temp\.tmpX");
    assert_eq!(box_mirror_for(home, &box_root, user_profile), mirrored);
}

#[test]
fn mirror_temp_on_other_drive_uses_drive_letter() {
    // 回归核心：TEMP 重定向到 D: 时旧代码拼 drive/C/...（永远读不到）。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"D:\Tmp\.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("D").join(r"Tmp\.tmpX"),
    );
}

#[test]
fn mirror_case_insensitive_profile_prefix() {
    // env 大小写不一致（USERPROFILE=C:\Users\Zoo vs 实际路径 c:\users\zoo）
    // 不得让 profile 前缀匹配失效。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\Zoo");
    let mirrored = box_root.join("user").join("current").join("Temp").join(".tmpX");
    std::fs::create_dir_all(&mirrored).unwrap();
    let home = Path::new(r"c:\users\zoo\Temp\.tmpX");
    assert_eq!(box_mirror_for(home, &box_root, user_profile), mirrored);
}

#[test]
fn mirror_missing_user_tree_falls_through_to_drive() {
    // home 在 profile 下但 user-tree 镜像不存在（如 box 刚被清）→
    // 回落 drive 镜像同路径，而不是返回不存在的 user 镜像。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"C:\Users\zoo\AppData\Local\Temp\.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("C")
            .join(r"Users\zoo\AppData\Local\Temp\.tmpX"),
    );
}

#[test]
fn mirror_forward_slash_drive_path() {
    // NEMESISBOT_HOME 环境变量常见正斜杠形态（resolve_home 后的输入）。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"D:/Tmp/.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("D").join(r"Tmp/.tmpX"),
    );
}

#[test]
fn mirror_verbatim_prefix_is_stripped() {
    // canonicalize() 的 \\?\ 前缀形态（run_eval 用 canonicalize 后的 real_home）。
    let td = t();
    let box_root = td.path().join("box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"\\?\D:\Tmp\.tmpX");
    assert_eq!(
        box_mirror_for(home, &box_root, user_profile),
        box_root.join("drive").join("D").join(r"Tmp\.tmpX"),
    );
}

#[test]
fn mirror_unc_path_returned_as_is() {
    // UNC 路径无盘符无镜像布局——原样返回，调用方 exists() 失败走标记。
    let box_root = Path::new(r"C:\fake\box");
    let user_profile = Path::new(r"C:\Users\zoo");
    let home = Path::new(r"\\server\share\tmp\.tmpX");
    assert_eq!(box_mirror_for(home, box_root, user_profile), home);
}

// ---------------------------------------------------------------------------
// wait_timeout_ms —— u32 毫秒饱和换算（避开 INFINITE=0xFFFFFFFF 哨兵）
// ---------------------------------------------------------------------------

#[test]
fn wait_ms_normal_values_pass_through() {
    assert_eq!(wait_timeout_ms(std::time::Duration::from_secs(0)), 0);
    assert_eq!(wait_timeout_ms(std::time::Duration::from_secs(1800)), 1_800_000);
}

#[test]
fn wait_ms_saturates_below_infinite() {
    // u32::MAX 正好是 WaitForSingleObject 的 INFINITE 哨兵——饱和值必须
    // 比它小，否则 49.7 天的等待会静默变成永久等待。
    let huge = std::time::Duration::from_secs(u64::from(u32::MAX) / 1000 + 600);
    assert_eq!(wait_timeout_ms(huge), u32::MAX - 1);
}

#[test]
fn wait_ms_just_under_limit_not_clamped() {
    let under = std::time::Duration::from_millis(u64::from(u32::MAX - 1));
    assert_eq!(wait_timeout_ms(under), u32::MAX - 1);
}
