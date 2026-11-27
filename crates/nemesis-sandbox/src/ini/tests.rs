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
    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, true, &[]).expect("write ini (true)");
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
    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, false, &[]).expect("write ini (false)");
    let content = std::fs::read_to_string(&ini_path).expect("read ini");
    assert!(
        content.contains("AllowNetworkAccess=n"),
        "allow_network=false must produce AllowNetworkAccess=n, got:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// 覆盖率批次（quality-hardening goal）：subscriber 下让
// write_sandboxie_ini 的 info! 参数行真实求值；并钉 ini 内容的关键安全行。
// ---------------------------------------------------------------------------

#[test]
fn write_ini_logs_and_pins_security_directives() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let ini = tmp.path().join("Sandboxie.ini");
    let box_root = tmp.path().join("box").join("NemesisBox");
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false, &[]).unwrap();
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
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, true, &[]).unwrap();
    assert!(
        std::fs::read_to_string(&ini)
            .unwrap()
            .contains("AllowNetworkAccess=y")
    );
}

// ---------------------------------------------------------------------------
// ② per-agent 常驻盒（NemesisAgent_*）：upsert 体系 + 指令集钉死。
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

/// 互钉面：pattern 的消毒形态与 nemesis-agent 侧
/// `agent_pipe_name` 的消毒实现用同一字面量组钉死（那边测试在
/// executor_pipe/tests.rs::agent_pipe_name_sanitized_matches_ini_openpipepath_
/// shape）。任意一侧消毒规则漂移（一侧消毒一侧原始）即两侧字面量断言脱钩，
/// 盒内 connect 被拒的形态在测试层暴露而不是生产 spawn 硬故障。
#[test]
fn agent_pipe_nt_pattern_sanitized_literal_pin() {
    assert_eq!(
        agent_pipe_nt_pattern("a/b\\c d"),
        r"\Device\NamedPipe\NemesisAgent_a_b_c_d_*"
    );
    assert_eq!(
        agent_pipe_nt_pattern("x:y"),
        r"\Device\NamedPipe\NemesisAgent_x_y_*"
    );
    assert_eq!(
        agent_pipe_nt_pattern("agent.1"),
        r"\Device\NamedPipe\NemesisAgent_agent_1_*"
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

/// `agent_box_directives` 指令集钉死（② 安全命门行——改任何一行都会被
/// 这条测试抓住）：Enabled/DropAdminRights/OpenPipePath/FileRootPath +
/// **两行 OpenFilePath 写穿透**（journal `.runtime` + browser-profile 登录态）。
#[test]
fn agent_box_directives_pin() {
    let dir = tempfile::tempdir().unwrap();
    let box_root = dir.path().join("box").join("NemesisAgent_x");
    let home = dir.path().join("agents").join("x").join("home");
    let d = agent_box_directives("x", &box_root, &home, false, &[]);

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
    // 必须（浏览器身份跨任务保持）；其余工作区写入保持虚拟化。
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
    let d = agent_box_directives("x", &box_root, &home, true, &[]);
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

    upsert_agent_box_section(&ini, "w1", &box_root, &home, false, &[]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(text.contains("[NemesisAgent_w1]"));
    assert!(text.contains(r"OpenPipePath=\Device\NamedPipe\NemesisAgent_w1_*"));
    assert!(text.contains(&format!(r"{}\.runtime\*", home.display())));
    assert!(text.contains(&format!(r"{}\browser-profile\*", home.display())));

    // 幂等：重复 upsert 不产生重复节
    upsert_agent_box_section(&ini, "w1", &box_root, &home, true, &[]).unwrap();
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
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false, &[]).unwrap();

    let agent_root = dir.path().join("box").join("NemesisAgent_w1");
    let agent_home = dir.path().join("agents").join("w1").join("home");
    upsert_agent_box_section(&ini, "w1", &agent_root, &agent_home, false, &[]).unwrap();
    assert!(
        std::fs::read_to_string(&ini)
            .unwrap()
            .contains("[NemesisAgent_w1]")
    );

    // install/start 路径：整文件重写 NemesisBox → agent 节被抹掉
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false, &[]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(
        !text.contains("[NemesisAgent_w1]"),
        "整文件重写抹掉 agent 节（文档化边界）"
    );

    // spawn 侧自愈：一次 upsert 恢复，NemesisBox 主节不受影响
    upsert_agent_box_section(&ini, "w1", &agent_root, &agent_home, false, &[]).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    assert!(text.contains("[NemesisAgent_w1]"));
    assert!(text.contains("[NemesisBox]"));
}

// ---------------------------------------------------------------------------
// 第四轮横扫（2026-11-27）：知识库配对双根写穿透。镜像写是「写盘即生效」
// 承诺本体，盒虚拟化困住即静默丢失且 `sandbox pending/commit`（工作区子树
// 口径）救不回——全局盒与 per-agent 盒的指令集都必须支持调用方按实体形态
// 追加写穿透树。空切片 = 旧行为逐字节不变。
// ---------------------------------------------------------------------------

/// `global_box_directives`：基线 5 指令 + 写穿透行追加（顺序=调用顺序）；
/// 空清单 = 基线 5 条（无任何 OpenFilePath 行 = 旧行为）。
#[test]
fn global_box_directives_write_through_shape() {
    let dir = tempfile::tempdir().unwrap();
    let box_root = dir.path().join("box").join("NemesisBox");
    let mirror = dir.path().join("mirror");
    let info = dir.path().join("info");

    let d = global_box_directives(
        "NemesisBox",
        &box_root,
        false,
        &[mirror.clone(), info.clone()],
    );
    assert_eq!(d.len(), 7, "基线 5 + 写穿透 2");
    assert_eq!(d[0], ("Enabled".into(), "y".into()));
    assert_eq!(
        d[4],
        (
            "FileRootPath".into(),
            format!(r"\??\{}", box_root.display())
        )
    );
    assert_eq!(
        d[5],
        ("OpenFilePath".into(), format!(r"{}\*", mirror.display()))
    );
    assert_eq!(
        d[6],
        ("OpenFilePath".into(), format!(r"{}\*", info.display()))
    );

    // 空清单 = 旧行为：基线 5 条、零 OpenFilePath
    let d = global_box_directives("NemesisBox", &box_root, false, &[]);
    assert_eq!(d.len(), 5, "空清单=基线 5 指令（旧行为）");
    assert!(d.iter().all(|(k, _)| k != "OpenFilePath"));
}

/// `write_sandboxie_ini` 落盘形态：带清单 = 写穿透行进 `[NemesisBox]` 段；
/// 摘掉写穿透行后与空清单输出**逐字节一致**（空切片=旧行为不变的可执行
/// 证明；全局盒基线本就无 OpenFilePath 行，摘除即还原）。
#[test]
fn write_sandboxie_ini_write_through_and_empty_slice_byte_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let box_root = dir.path().join("box").join("NemesisBox");
    let mirror = dir.path().join("mirror");
    let info = dir.path().join("info");
    let ini_a = dir.path().join("a.ini");
    let ini_b = dir.path().join("b.ini");

    write_sandboxie_ini(
        &ini_a,
        "NemesisBox",
        &box_root,
        false,
        &[mirror.clone(), info.clone()],
    )
    .unwrap();
    write_sandboxie_ini(&ini_b, "NemesisBox", &box_root, false, &[]).unwrap();

    let a = std::fs::read_to_string(&ini_a).unwrap();
    let b = std::fs::read_to_string(&ini_b).unwrap();
    // 空清单版：零写穿透行（旧行为逐字节保留——既有安全行钉死测试全覆盖）
    assert!(
        b.lines().all(|l| !l.starts_with("OpenFilePath=")),
        "空清单必须零写穿透行: {b}"
    );
    // 带清单版：两行 OpenFilePath 在段尾；摘除后与空清单版逐字节一致
    assert!(a.contains(&format!(r"OpenFilePath={}\*", mirror.display())));
    assert!(a.contains(&format!(r"OpenFilePath={}\*", info.display())));
    let stripped = a
        .lines()
        .filter(|l| !l.starts_with("OpenFilePath="))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        stripped,
        b.trim_end(),
        "摘除写穿透行后必须与空清单输出逐字节一致（空切片=旧行为不变）"
    );
}

/// `agent_box_directives` 带额外写穿透清单：基线 7 + 清单行追加段尾；
/// 空清单 = 基线 7（与既有钉死测试的长度断言一致）。
#[test]
fn agent_box_directives_write_through_appends_extra_roots() {
    let dir = tempfile::tempdir().unwrap();
    let box_root = dir.path().join("box").join("NemesisAgent_x");
    let home = dir.path().join("agents").join("x").join("home");
    let mirror = dir.path().join("mirror");

    let d = agent_box_directives("x", &box_root, &home, false, std::slice::from_ref(&mirror));
    assert_eq!(d.len(), 8, "基线 7 + 写穿透 1");
    assert_eq!(
        d[7],
        ("OpenFilePath".into(), format!(r"{}\*", mirror.display()))
    );
    assert_eq!(
        agent_box_directives("x", &box_root, &home, false, &[]).len(),
        7,
        "空清单=基线 7（旧行为）"
    );
}

// ---------------------------------------------------------------------------
// 第十轮横扫（2026-11-27）：写穿透根控制字符过滤——ini 是**行格式**，根含
// LF/CR 即 Sandboxie 指令注入槽（根字符串 `D:\kb\nOpenFilePath=C:` 断行后
// 产生 `OpenFilePath=C:\*` 全盘写穿透行）。Dashboard 保存面 validate 已
// loud 拒绝（nemesis-config），手改文件绕过校验（读盘面 fail-closed 不跑
// validate）由本 sink 过滤兜底：坏根跳行 + WARN，其余行照常。
// ---------------------------------------------------------------------------

/// 混合清单只干净根产行（坏根跳过，顺序保持）；全坏清单 = 等价空清单
/// （零 OpenFilePath 行）。
#[test]
fn write_through_directives_skip_control_char_roots() {
    let dir = tempfile::tempdir().unwrap();
    let box_root = dir.path().join("box").join("NemesisBox");
    let clean = dir.path().join("info");
    // 注入形态复现：根字符串内嵌 LF + 伪指令行（JSON 手改可达——该形态
    // is_absolute 校验放行，唯有 sink 过滤能拦）
    let dirty_inject = PathBuf::from("D:\\kb\nOpenFilePath=C:");
    let dirty_tab = PathBuf::from("D:\\kb\t2");

    let d = global_box_directives(
        "NemesisBox",
        &box_root,
        false,
        &[dirty_inject, clean.clone(), dirty_tab],
    );
    assert_eq!(d.len(), 6, "基线 5 + 干净根 1（两个控制字符根跳行）");
    assert_eq!(
        d[5],
        ("OpenFilePath".into(), format!(r"{}\*", clean.display()))
    );

    // 全坏清单 = 基线 5（零 OpenFilePath 行 = 等价空清单语义）
    let d = global_box_directives(
        "NemesisBox",
        &box_root,
        false,
        &[PathBuf::from("D:\nx"), PathBuf::from("D:\ry")],
    );
    assert_eq!(d.len(), 5, "全坏清单 = 基线 5（等价空清单）");
    assert!(d.iter().all(|(k, _)| k != "OpenFilePath"));
}
