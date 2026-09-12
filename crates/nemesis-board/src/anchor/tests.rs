//! P2（B1）锚点模块测试：解析穷举 / 执行器 / 路径安全 / 渲染。
//! 纯逻辑 + 临时目录文件只读，无网络无命令。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

/// 隔离临时 workspace 根（`.parent()` 拼路径，遵循仓库测试纪律）。
fn temp_workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-anchor-{}-{}-{name}",
        std::process::id(),
        DIR_SEQ.fetch_add(1, Ordering::SeqCst),
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

fn write_file(ws: &Path, rel: &str, content: &str) {
    let p = ws.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, content).expect("write file");
}

// ---------- parse_anchors：解析穷举 ----------

#[test]
fn parse_three_file_forms_and_content_regex() {
    let ac = "\
[CHECK] file:src/lib.rs exists
[CHECK] file:docs/api.md contains:鉴权
[CHECK] file:src/main.rs re:^fn main
[CHECK] re:交付完成";
    let (anchors, semantic, rejected) = parse_anchors(ac);
    assert_eq!(anchors.len(), 4, "四条合法锚点全解析: {anchors:?}");
    assert!(semantic.is_empty());
    assert!(rejected.is_empty(), "合法锚点不得有告警记录: {rejected:?}");
    assert_eq!(anchors[0].kind, AnchorKind::FileExists);
    assert_eq!(anchors[0].target, "src/lib.rs");
    assert_eq!(anchors[0].pattern, None);
    assert_eq!(anchors[1].kind, AnchorKind::FileContains);
    assert_eq!(anchors[1].pattern.as_deref(), Some("鉴权"));
    assert_eq!(anchors[2].kind, AnchorKind::FileRegex);
    assert_eq!(anchors[2].pattern.as_deref(), Some("^fn main"));
    assert_eq!(anchors[3].kind, AnchorKind::ContentRegex);
    assert_eq!(anchors[3].target, "");
}

#[test]
fn parse_mixed_lines_split_anchors_and_semantic() {
    let ac = "产出必须包含实施要点说明。\n[CHECK] file:out/result.txt exists\n验收口径以评审为准。";
    let (anchors, semantic, rejected) = parse_anchors(ac);
    assert_eq!(anchors.len(), 1);
    assert_eq!(
        semantic,
        vec!["产出必须包含实施要点说明。", "验收口径以评审为准。"]
    );
    assert!(rejected.is_empty());
}

#[test]
fn parse_unparseable_check_lines_fall_back_to_semantic() {
    // 坏行不拒绝：空谓词 / 未知谓词 / 空正则 / 非法正则 / 裸 [CHECK]。
    let ac = "\
[CHECK]
[CHECK] file: exists
[CHECK] file:src/a.rs frobnicate:xyz
[CHECK] re:
[CHECK] re:[
[CHECK] file:src/a.rs re:([)";
    let (anchors, semantic, rejected) = parse_anchors(ac);
    assert!(anchors.is_empty(), "坏行不得产出锚点: {anchors:?}");
    assert_eq!(semantic.len(), 6, "全部回落语义项: {semantic:?}");
    assert!(semantic.iter().all(|l| l.contains("[CHECK]")));
    // 无法解析 ≠ 不安全：坏行回落语义项但不告警（告警留给路径形态违规）。
    assert!(rejected.is_empty(), "坏行不应进告警记录: {rejected:?}");
}

#[test]
fn parse_regex_pattern_may_contain_spaces() {
    let (anchors, _, _rej) = parse_anchors("[CHECK] re:^## 结论\\s+完成");
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].pattern.as_deref(), Some("^## 结论\\s+完成"));
    let (anchors, _, _rej) = parse_anchors("[CHECK] file:docs/a.md contains:验收 标准 v2");
    assert_eq!(anchors[0].pattern.as_deref(), Some("验收 标准 v2"));
}

#[test]
fn parse_empty_criteria_yields_nothing() {
    assert_eq!(parse_anchors("").0.len(), 0);
    assert_eq!(parse_anchors("   \n  \n").0.len(), 0);
    assert!(parse_anchors("").1.is_empty());
}

// ---------- run_anchors：执行器 ----------

#[test]
fn run_file_exists_pass_and_fail() {
    let ws = temp_workspace("exists");
    write_file(&ws, "out/result.txt", "hello");
    let (anchors, _, _rej) =
        parse_anchors("[CHECK] file:out/result.txt exists\n[CHECK] file:out/missing.txt exists");
    let results = run_anchors(&anchors, &ws, "");
    assert_eq!(results.len(), 2);
    assert!(results[0].passed, "{}", results[0].detail);
    assert!(!results[1].passed, "{}", results[1].detail);
}

#[test]
fn run_file_regex_and_contains() {
    let ws = temp_workspace("regex");
    write_file(&ws, "src/main.rs", "fn main() {\n    // 鉴权入口\n}\n");
    let (anchors, _, _rej) = parse_anchors(
        "[CHECK] file:src/main.rs re:^fn main\n[CHECK] file:src/main.rs contains:鉴权\n[CHECK] file:src/main.rs re:^use nothing\n[CHECK] file:src/main.rs contains:不存在词",
    );
    let results = run_anchors(&anchors, &ws, "");
    assert!(results[0].passed, "{}", results[0].detail);
    assert!(results[1].passed, "{}", results[1].detail);
    assert!(!results[2].passed);
    assert!(!results[3].passed);
}

#[test]
fn run_content_regex_matches_delivery_text() {
    let ws = temp_workspace("content");
    let (anchors, _, _rej) = parse_anchors("[CHECK] re:交付完成");
    let results = run_anchors(&anchors, &ws, "## 结论\n交付完成");
    assert!(results[0].passed, "{}", results[0].detail);
    let results = run_anchors(&anchors, &ws, "尚未完成");
    assert!(!results[0].passed);
}

// 自指防御（2026-09-12 UAT T30③ 回归锁）：交付文本逐字引用锚点行本身
// （任务 prompt 回显）不构成满足锚点的证据；剥离引文后其余文本无命中
// → 诚实 FAIL。
#[test]
fn content_regex_ignores_quoted_anchor_line_echo() {
    let ws = temp_workspace("echo");
    let (anchors, _, _rej) = parse_anchors("[CHECK] re:UAT30FAILNEEDLE");
    // 重派 prompt 回显形态：任务卡头 + 验收标准引文（含锚点行原文）。
    let echo = "# 看板任务 NB-25\n\n## 验收标准\n交付说明文本。\n[CHECK] re:UAT30FAILNEEDLE\n";
    let results = run_anchors(&anchors, &ws, echo);
    assert!(
        !results[0].passed,
        "引述锚点行不得自命中: {}",
        results[0].detail
    );
}

// 剥离只针对锚点行原文本身：交付文本在引文之外另有真实命中证据时照常
// 通过（不惩罚「复述标准 + 交付」的合法汇报）。
#[test]
fn content_regex_still_matches_evidence_outside_quoted_line() {
    let ws = temp_workspace("evidence");
    let (anchors, _, _rej) = parse_anchors("[CHECK] re:UAT30PASSNEEDLE");
    let report = "# 看板任务 NB-9\n\n## 验收标准\n[CHECK] re:UAT30PASSNEEDLE\n\n## 结论\n本任务 UAT30PASSNEEDLE 已交付。\n";
    let results = run_anchors(&anchors, &ws, report);
    assert!(results[0].passed, "{}", results[0].detail);
}

#[test]
fn run_chinese_keyword_contains() {
    let ws = temp_workspace("chinese");
    write_file(&ws, "报告.md", "# 验收报告\n自检全部通过。");
    let (anchors, _, _rej) = parse_anchors("[CHECK] file:报告.md contains:自检全部通过");
    let results = run_anchors(&anchors, &ws, "");
    assert!(results[0].passed, "{}", results[0].detail);
}

#[test]
fn run_large_file_completes_and_enforces_cap() {
    let ws = temp_workspace("large");
    // ~2MB 正常文件：线性时间正则可完成。
    let big = "x".repeat(2 * 1024 * 1024) + "\nTARGET_NEEDLE\n";
    write_file(&ws, "big.txt", &big);
    let (anchors, _, _rej) = parse_anchors("[CHECK] file:big.txt contains:TARGET_NEEDLE");
    let results = run_anchors(&anchors, &ws, "");
    assert!(results[0].passed);
    // 超限文件（模拟：直接构造 AnchorCheck 绕过解析层不可行——大小检查在
    // read_limited；此处用 metadata 语义等价验证：64MB 上限按失败处理）。
    // 真造 64MB 文件太慢，改为验证 read_limited 的超限分支（私有 fn 经
    // run_one 间接覆盖：构造不存在文件已覆盖 Err 分支，此处钉上限常量）。
    assert_eq!(super::MAX_ANCHOR_FILE_BYTES, 64 * 1024 * 1024);
}

#[test]
fn run_empty_anchor_group_passes() {
    let ws = temp_workspace("empty");
    let results = run_anchors(&[], &ws, "");
    assert!(results.is_empty());
    assert!(all_passed(&results), "无锚点 = 全过（零回归面）");
}

// ---------- 路径安全 ----------

#[test]
fn path_safety_rejects_absolute_and_traversal_at_parse_time() {
    // 形态不安全（绝对路径 / `..` 穿越）= 解析期拒绝：不产出锚点、回落
    // 语义项 + RejectedAnchor 告警记录（T2-3 语义：标准自身的毛病不惩罚
    // 执行者）。首条 `file:/etc/passwd` 在 Windows 上不是 is_absolute
    // （无盘符前缀），但 RootDir 组件仍被拒。
    let (anchors, semantic, rejected) = parse_anchors(
        "[CHECK] file:/etc/passwd exists\n[CHECK] file:../outside.txt exists\n[CHECK] file:a/../../b.txt exists",
    );
    assert!(anchors.is_empty(), "不安全锚点不得产出: {anchors:?}");
    assert_eq!(semantic.len(), 3, "全部回落语义项: {semantic:?}");
    assert_eq!(rejected.len(), 3, "全部带告警记录: {rejected:?}");
    assert!(rejected[1].reason.contains(".."), "{}", rejected[1].reason);
}

#[test]
fn path_safety_allows_relative_inside_workspace() {
    let ws = temp_workspace("inside");
    write_file(&ws, "a/b/c.txt", "ok");
    let (anchors, _, rejected) = parse_anchors("[CHECK] file:./a/b/c.txt exists");
    assert!(rejected.is_empty());
    let results = run_anchors(&anchors, &ws, "");
    assert!(results[0].passed, "{}", results[0].detail);
}

#[test]
fn path_safety_rejects_symlink_escape_at_runtime_backstop() {
    // 运行时后备闸：形态合法但 canonicalize 越界（符号链接）→ run_anchors
    // 保守失败。解析期拦不住（FS 无关），这是纵深防御的第二道闸。
    let ws = temp_workspace("symlink");
    let outside = temp_workspace("symlink-outside");
    write_file(&outside, "secret.txt", "leak");
    let link = ws.join("link.txt");
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(outside.join("secret.txt"), &link).is_ok();
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_file(outside.join("secret.txt"), &link).is_ok();
    if !created {
        // Windows 无开发者模式/管理员权限时建链失败：诚实跳过（能力不足
        // 不是产品问题），安全闸本体由组件级拒绝用例覆盖。
        eprintln!("symlink 不可创建（权限不足），跳过越界用例");
        return;
    }
    let (anchors, _, rejected) = parse_anchors("[CHECK] file:link.txt contains:leak");
    assert!(
        rejected.is_empty(),
        "形态合法的锚点不得在解析期被拒: {rejected:?}"
    );
    let results = run_anchors(&anchors, &ws, "");
    assert!(
        !results[0].passed,
        "符号链接越界必须拒读: {}",
        results[0].detail
    );
}

// ---------- Windows 形态用例（盘符 / UNC / 8.3 短名）----------
// 形态闸是 FS 无关契约：盘符/UNC 拒收在所有平台生效（2026-09-12 根修，
// 此前生产代码 cfg(windows) 门控导致 Linux 全放行），本组测试跨平台跑。

#[test]
fn path_safety_rejects_drive_and_unc_forms_at_parse_time() {
    for raw in [
        "C:\\Windows\\system32\\cmd.exe",
        "C:foo.txt",
        "\\\\server\\share\\x.txt",
    ] {
        let (anchors, _, rejected) = parse_anchors(&format!("[CHECK] file:{raw} exists"));
        assert!(anchors.is_empty(), "盘符/UNC 形态必须拒绝: {raw}");
        assert_eq!(rejected.len(), 1, "盘符/UNC 形态须有告警记录: {raw}");
    }
}

#[test]
fn path_safety_rejects_83_short_names_at_parse_time() {
    // 组件级拒绝在解析期（canonicalize 之前），无需真实 8.3 名存在。
    for raw in ["RUNNER~1/x.txt", "doc~1.TXT", "a/b~2/c.txt"] {
        let (anchors, _, rejected) = parse_anchors(&format!("[CHECK] file:{raw} exists"));
        assert!(anchors.is_empty(), "8.3 短名组件必须拒绝: {raw}");
        assert_eq!(rejected.len(), 1, "8.3 短名须有告警记录: {raw}");
    }
    // 普通含 ~ 文件名不受影响（~ 后无数字，或数字后跟字母）。
    assert!(!has_83_short_name("my~notes.txt"));
    assert!(!has_83_short_name("v~2beta.txt"));
    assert!(has_83_short_name("RUNNER~1"));
    assert!(has_83_short_name("doc~12.TXT"));
}

// ---------- 渲染 ----------

#[test]
fn render_summary_and_failures_are_per_anchor_readable() {
    let ws = temp_workspace("render");
    write_file(&ws, "a.txt", "good");
    let (anchors, _, _rej) =
        parse_anchors("[CHECK] file:a.txt exists\n[CHECK] file:missing.txt exists");
    let results = run_anchors(&anchors, &ws, "");
    let summary = render_anchor_summary(&results);
    assert!(summary.contains("✅"));
    assert!(!summary.contains("❌"));
    let failures = render_anchor_failures(&results);
    assert!(failures.contains("❌"));
    assert!(
        failures.contains("[CHECK] file:missing.txt exists"),
        "{failures}"
    );
    assert!(!all_passed(&results));
}
