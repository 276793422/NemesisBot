//! detect 纯函数面测试（独立测试文件——生产文件零内联测试纪律）。

use super::*;

#[test]
fn source_tag_always_matches_detection_presence() {
    // F24 回归锁（契约面）：来源在命中点结构化记录——有路径必有来源标记、
    // 无路径必空串，且标记值域闭合。旧 source_of 按路径内容猜（App Paths
    // 注册表值不含「app paths」字样 → 恒误标 known_path/path）的形态在本
    // 契约下不可能复现。
    use DriverKind::*;
    for kind in [Edge, Chrome, Brave, Vivaldi, Chromium, Firefox, Custom] {
        let (p, src) = find_executable_with_source(kind);
        assert_eq!(
            p.is_some(),
            !src.is_empty(),
            "kind {kind:?}: 来源与检出态不配对"
        );
        assert!(
            src.is_empty() || ["app_paths", "known_path", "path"].contains(&src),
            "kind {kind:?}: 未知来源标记 {src:?}"
        );
    }
}

#[cfg(windows)]
#[test]
fn expand_env_self_referential_var_terminates() {
    // F23 回归锁：自引用环境变量（值含自身标记，REG_EXPAND_SZ 自引用残留
    // 形态真实存在）曾让展开循环无限迭代——spawn 路径挂死。5 轮上限必须
    // 终止；终值保留标记原样（不再含可继续展开的完整 %VAR% 对也成立——
    // 上限语义是「停止展开」不是「清空」）。
    // SAFETY: 测试专用变量名全仓唯一，无并行读者/写者。
    unsafe { std::env::set_var("NB_BROWSER_TEST_LOOP_VAR", "%NB_BROWSER_TEST_LOOP_VAR%") };
    let p = super::expand_env("%NB_BROWSER_TEST_LOOP_VAR%");
    let s = p.to_string_lossy();
    assert!(
        s.contains("NB_BROWSER_TEST_LOOP_VAR"),
        "上限截断后标记丢失: {s}"
    );
    // 终止性即断言主体：若循环未封顶，本测试挂死（超时红）。
}

#[cfg(windows)]
#[test]
fn expand_env_normal_and_unmatched_forms() {
    // 常规形态：展开已知变量；未知变量原样保留；无闭合 % 原样保留。
    // SAFETY: 测试专用变量名全仓唯一。
    unsafe { std::env::set_var("NB_BROWSER_TEST_EXPAND_VAR", "hello") };
    assert_eq!(
        super::expand_env("%NB_BROWSER_TEST_EXPAND_VAR%/bin").to_string_lossy(),
        "hello/bin"
    );
    assert_eq!(
        super::expand_env("%NB_BROWSER_TEST_MISSING_VAR%/bin").to_string_lossy(),
        "%NB_BROWSER_TEST_MISSING_VAR%/bin"
    );
    assert_eq!(
        super::expand_env("%unclosed/bin").to_string_lossy(),
        "%unclosed/bin"
    );
}

#[cfg(windows)]
#[test]
fn expand_env_multibyte_after_percent_never_panics() {
    // 行为锁（多字节鲁棒性）：注册表 App Paths 值是外部输入（HKCU 任意
    // 用户态程序可写），% 后紧跟多字节 UTF-8 字符（Windows 环境变量名可含
    // 非 ASCII）是合法可达形态。现实现的两处切片点（start+1 / end+1）按
    // 构造恒为字符起点（前一字符是 ASCII '%'，下一字符首字节恰在切片点）
    // ——本测试钉住「畸形/多字节形态不 panic、诚实保留原样或正确展开」，
    // 防未来重构（换字节级操作/裸索引）引入 char boundary panic。
    // ① 变量名首字符就是多字节（未设变量 → 原样保留）。
    let s = "%你%";
    assert_eq!(
        super::expand_env(s).to_string_lossy(),
        s,
        "畸形形态保留原样"
    );
    // ② 变量命中展开后，闭合 % 的下一字符是多字节（展开 + 尾部保留）。
    // SAFETY: 测试专用变量名全仓唯一。
    unsafe { std::env::set_var("NB_BROWSER_TEST_MB_VAR", "ok") };
    let s2 = "%NB_BROWSER_TEST_MB_VAR%你tail";
    assert_eq!(
        super::expand_env(s2).to_string_lossy(),
        "ok你tail",
        "合法变量展开、尾部多字节保留"
    );
}
