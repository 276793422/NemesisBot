//! Tests for Sandboxie.ini generation.
//!
//! Kept in a separate file per the project's "tests in `<stem>/tests.rs` discipline.

use super::*;

/// `allow_network` must map to the correct `AllowNetworkAccess=y|n` directive in the
/// generated ini. This is the config→ini half of the network switch — a wrong or
/// inverted mapping would flip the whole box's network state.
#[test]
fn write_sandboxie_ini_emits_allow_network_directive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ini_path = dir.path().join("Sandboxie.ini");
    let box_root = dir.path().join("box").join("NemesisBox");

    // allow_network=true → AllowNetworkAccess=y (and NOT =n)
    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, true).expect("write ini (true)");
    let content = std::fs::read_to_string(&ini_path).expect("read ini");
    assert!(
        content.contains("AllowNetworkAccess=y"),
        "allow_network=true must produce AllowNetworkAccess=y, got:\n{content}"
    );
    assert!(
        !content.contains("AllowNetworkAccess=n"),
        "allow_network=true must NOT also emit AllowNetworkAccess=n, got:\n{content}"
    );

    // allow_network=false → AllowNetworkAccess=n
    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, false).expect("write ini (false)");
    let content = std::fs::read_to_string(&ini_path).expect("read ini");
    assert!(
        content.contains("AllowNetworkAccess=n"),
        "allow_network=false must produce AllowNetworkAccess=n, got:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：subscriber 下让
// write_sandboxie_ini 的 info! 参数行真实求值；并钉 ini 内容的关键安全行。
// ---------------------------------------------------------------------------

#[test]
fn write_ini_logs_and_pins_security_directives() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let ini = tmp.path().join("Sandboxie.ini");
    let box_root = tmp.path().join("box").join("NemesisBox");
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    // 安全命门行逐条钉死（改任何一行都会被这条测试抓住）
    assert!(text.contains("[NemesisBox]"));
    assert!(text.contains("Enabled=y"));
    assert!(text.contains("AllowNetworkAccess=n"), "默认断网");
    assert!(text.contains("DropAdminRights=y"), "盒内去管理员");
    assert!(text.contains(r"OpenPipePath=\Device\NamedPipe\NemesisBox_*"));
    assert!(
        text.contains("SbieCtrl_EnableAutoStart=n"),
        "headless 禁 GUI 自启"
    );
    assert!(text.contains(&format!(r"FileRootPath=\??\{}", box_root.display())));
    // 幂等重写（allow_network 翻转）
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, true).unwrap();
    assert!(
        std::fs::read_to_string(&ini)
            .unwrap()
            .contains("AllowNetworkAccess=y")
    );
}

// ---------------------------------------------------------------------------
// P3 ② per-agent 常驻盒（NemesisAgent_*）：upsert 体系 + 指令集钉死。
// ---------------------------------------------------------------------------

/// `agent_pipe_nt_pattern` 的字面量形态：ini `OpenPipePath` 允许清单必须与
/// nemesis-agent 侧 `agent_pipe_name`（`\\.\pipe\NemesisAgent_<id>_<uniq>`）
/// 的 NT 前缀匹配，否则盒内子进程永远连不上管道（两 crate 测试互钉）。
#[test]
fn agent_pipe_nt_pattern_literal_pin() {
    assert_eq!(
        agent_pipe_nt_pattern("abc123"),
        r"\Device\NamedPipe\NemesisAgent_abc123_*"
    );
}

/// `agent_box_name`：前缀钉死 + 非法字符消毒（对手工 id 的防御；生产 id 是
/// uuid simple hex，本就不含非法字符）。
#[test]
fn agent_box_name_prefix_and_sanitization() {
    assert_eq!(agent_box_name("deadbeef"), "NemesisAgent_deadbeef");
    assert_eq!(agent_box_name("a/b\\c d"), "NemesisAgent_a_b_c_d");
    assert_eq!(agent_box_name("x:y"), "NemesisAgent_x_y");
    assert_eq!(agent_box_name(""), "NemesisAgent_");
}

/// `agent_box_directives` 指令集钉死（P3 ② 安全命门行——改任何一行都会被
/// 这条测试抓住）：Enabled/DropAdminRights/OpenPipePath/FileRootPath +
/// **两行 OpenFilePath 写穿透**（journal `.runtime` + browser-profile 登录态）。
#[test]
fn agent_box_directives_pin() {
    let dir = tempfile::tempdir().unwrap();
    let box_root = dir.path().join("box").join("NemesisAgent_x");
    let home = dir.path().join("agents").join("x").join("home");
    let d = agent_box_directives("x", &box_root, &home, false);

    assert_eq!(d[0], ("Enabled".into(), "y".into()));
    assert_eq!(d[1], ("AllowNetworkAccess".into(), "n".into()));
    assert_eq!(d[2], ("DropAdminRights".into(), "y".into()));
    assert_eq!(
        d[3],
        (
            "OpenPipePath".into(),
            r"\Device\NamedPipe\NemesisAgent_x_*".into()
        )
    );
    assert_eq!(
        d[4],
        (
            "FileRootPath".into(),
            format!(r"\??\{}", box_root.display())
        )
    );
    // 写穿透两行：journal 必须（崩溃恢复/幂等记账在真盘）+ 浏览器登录态
    // 必须（T6 浏览器身份跨任务保持）；其余工作区写入保持虚拟化。
    assert_eq!(
        d[5],
        (
            "OpenFilePath".into(),
            format!(r"{}\.runtime\*", home.display())
        )
    );
    assert_eq!(
        d[6],
        (
            "OpenFilePath".into(),
            format!(r"{}\browser-profile\*", home.display())
        )
    );
    assert_eq!(d.len(), 7, "指令集有增减必须先改这条钉死测试");

    // allow_network=true 翻转
    let d = agent_box_directives("x", &box_root, &home, true);
    assert_eq!(d[1], ("AllowNetworkAccess".into(), "y".into()));
}

/// `upsert_box_section`：缺文件从零创建；已有其他 section 逐字保留；同节
/// 幂等替换（旧内容不残留）；大小写不敏感摘除同节。
#[test]
fn upsert_box_section_creates_preserves_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("nested").join("Sandboxie.ini");

    // 1) 缺文件（含缺失父目录）从零创建
    upsert_box_section(&ini, "BoxB", &[("Enabled".into(), "y".into())]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(text.contains("[BoxB]"));
    assert!(text.contains("Enabled=y"));

    // 2) 已有内容：其他 section + preamble 逐字保留，新节追加
    std::fs::write(
        &ini,
        "; preamble comment\n[BoxA]\nAllowNetworkAccess=n\nCustom=keep\n",
    )
    .unwrap();
    upsert_box_section(&ini, "BoxB", &[("Enabled".into(), "y".into())]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(text.contains("; preamble comment"), "preamble 保留");
    assert!(text.contains("[BoxA]"));
    assert!(text.contains("Custom=keep"), "其他节内容逐字保留");
    assert!(text.contains("[BoxB]"));

    // 3) 同节幂等替换：旧指令不残留（BoxB 重写为不同指令集）
    upsert_box_section(&ini, "BoxB", &[("Enabled".into(), "n".into())]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert_eq!(text.matches("[BoxB]").count(), 1, "同节只出现一次: {text}");
    assert!(!text.contains("Enabled=y\n"), "旧指令不残留: {text}");
    assert!(text.contains("Enabled=n"));

    // 4) 大小写不敏感摘除同节（BoxB 被摘，追加 boxb；BoxA 原样保留）
    upsert_box_section(&ini, "boxb", &[("Enabled".into(), "y".into())]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert_eq!(
        text.matches("[BoxB]").count() + text.matches("[boxb]").count(),
        1,
        "大小写不敏感摘除旧节，只剩新节: {text}"
    );
    assert!(text.contains("[BoxA]"), "无关节不受影响");
}

/// `upsert_agent_box_section` 端到端：写进的是 `[NemesisAgent_<id>]` 节、
/// 指令集与 `agent_box_directives` 一致；再跑一次幂等（节不重复）。
#[test]
fn upsert_agent_box_section_end_to_end_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("Sandboxie.ini");
    let box_root = dir.path().join("box").join("NemesisAgent_w1");
    let home = dir.path().join("agents").join("w1").join("home");

    upsert_agent_box_section(&ini, "w1", &box_root, &home, false).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(text.contains("[NemesisAgent_w1]"));
    assert!(text.contains(r"OpenPipePath=\Device\NamedPipe\NemesisAgent_w1_*"));
    assert!(text.contains(&format!(r"{}\.runtime\*", home.display())));
    assert!(text.contains(&format!(r"{}\browser-profile\*", home.display())));

    // 幂等：重复 upsert 不产生重复节
    upsert_agent_box_section(&ini, "w1", &box_root, &home, true).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert_eq!(text.matches("[NemesisAgent_w1]").count(), 1);
    assert!(text.contains("AllowNetworkAccess=y"), "翻转生效");
}

/// 与 `write_sandboxie_ini` 共存：整文件重写会抹掉 agent 盒段（已知语义，
/// spawn 侧每次幂等 upsert 自愈）——本测试钉住两个行为：① 重写后 agent 节
/// 确实消失（文档化该边界，不是遗漏）；② upsert 一次即恢复。
#[test]
fn write_full_ini_wipes_agent_sections_upsert_heals() {
    let dir = tempfile::tempdir().unwrap();
    let ini = dir.path().join("Sandboxie.ini");
    let box_root = dir.path().join("box").join("NemesisBox");
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false).unwrap();

    let agent_root = dir.path().join("box").join("NemesisAgent_w1");
    let agent_home = dir.path().join("agents").join("w1").join("home");
    upsert_agent_box_section(&ini, "w1", &agent_root, &agent_home, false).unwrap();
    assert!(
        std::fs::read_to_string(&ini)
            .unwrap()
            .contains("[NemesisAgent_w1]")
    );

    // install/start 路径：整文件重写 NemesisBox → agent 节被抹掉
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(
        !text.contains("[NemesisAgent_w1]"),
        "整文件重写抹掉 agent 节（文档化边界）"
    );

    // spawn 侧自愈：一次 upsert 恢复，NemesisBox 主节不受影响
    upsert_agent_box_section(&ini, "w1", &agent_root, &agent_home, false).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(text.contains("[NemesisAgent_w1]"));
    assert!(text.contains("[NemesisBox]"));
}
