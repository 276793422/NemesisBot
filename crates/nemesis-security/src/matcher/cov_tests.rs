// matcher.rs 覆盖率补充测试（wildcard_to_regex 特殊字符转义臂 81-85 +
// extract_interpreter_payloads 载荷提取/空载荷臂 186-198）。
//
// 豁免（死防御臂——regex 构造输入全部经转义/占位符保护，Regex::new 对
// 这些输入不可能失败，Err(_) 臂不可达，纯防御）：
// - 54：do_match 的 `Err(_) => false` —— wildcard_to_regex 只产出
//   `^`/`$`/`.*`/`[^/]*` 与转义后的字面量，恒为合法 regex。
// - 120：match_command_pattern 的 `Err(_)` —— 通配符先换占位符再
//   regex::escape，结果恒合法。
// - 256：match_domain_pattern 的 `Err(_)` —— 双占位符 + escape 恒合法。

use super::*;

/// 路径 pattern 含 regex 特殊字符 → 走转义臂（81-85），按字面量匹配。
#[test]
fn match_pattern_escapes_regex_specials() {
    // '+'、'('、')'、'['、']'、'|'、'$'、'^'、'{'、'}' 全部字面量命中。
    assert!(match_pattern(
        "C:/data [v1]/a+b.txt",
        "C:/data [v1]/a+b.txt"
    ));
    assert!(!match_pattern("a+b.txt", "axb.txt"), "+ 必须按字面量匹配");
    assert!(!match_pattern("(x)|y$.txt", "other.txt"));

    // 特殊字符 + 通配符混用：* 展开，特殊字符字面量收口。
    assert!(match_pattern("logs/*(1).txt", "logs/a(1).txt"));
    assert!(!match_pattern("logs/*(1).txt", "logs/a(2).txt"));
    assert!(match_pattern("conf {v^2}/$x.txt", "conf {v^2}/$x.txt"));
}

/// extract_interpreter_payloads：解释器包装拆段 + 空载荷跳过（186-198）。
#[test]
fn extract_interpreter_payloads_drives_wrapper_scan() {
    // powershell -command 载荷整段拆出（normalize 剥引号/合并空白/小写）。
    let n = normalize_exec_command("PowerShell -Command \"Remove-Item -Recurse C:/x\"");
    assert_eq!(
        extract_interpreter_payloads(&n),
        vec!["remove-item -recurse c:/x"]
    );

    // cmd /c 载荷尾部整段并入。
    let n2 = normalize_exec_command("cmd /c foo bar");
    assert_eq!(extract_interpreter_payloads(&n2), vec!["foo bar"]);

    // 旗标后无内容 → trim 后为空 → 不推入。
    let n3 = normalize_exec_command("python -c \" \"");
    assert!(
        extract_interpreter_payloads(&n3).is_empty(),
        "空载荷不得推入: {:?}",
        extract_interpreter_payloads(&n3)
    );

    // 无旗标解释器调用（脚本文件形态）→ 空。
    assert!(extract_interpreter_payloads(&normalize_exec_command("python script.py")).is_empty());
}

/// bin_key 取 basename（2026-10-13 复检批）：绝对路径解释器必须命中 WRAPPERS 表，
/// 否则 `C:\...\cmd.exe /c <恶意载荷>` 会因 key 带目录而漏扫。
#[test]
fn extract_interpreter_payloads_full_path_interpreter_hits_wrappers() {
    // Windows 绝对路径 cmd.exe（normalize 后正斜杠）→ 载荷拆出
    let win = normalize_exec_command("c:/windows/system32/cmd.exe /c del /q c:/x");
    assert_eq!(
        extract_interpreter_payloads(&win),
        vec!["del /q c:/x"],
        "全路径 cmd.exe 必须按 basename 命中 WRAPPERS"
    );

    // POSIX 绝对路径 sh -c → 载荷拆出
    let sh = normalize_exec_command("/bin/sh -c rm -rf tmp");
    assert_eq!(
        extract_interpreter_payloads(&sh),
        vec!["rm -rf tmp"],
        "全路径 /bin/sh 必须按 basename 命中 WRAPPERS"
    );

    // 相对形态含目录（./cmd.exe）同样按 basename 命中
    let rel = normalize_exec_command("./cmd.exe /c echo hi");
    assert_eq!(extract_interpreter_payloads(&rel), vec!["echo hi"]);
}

// ---------------------------------------------------------------------------
// exec_rule_segments / exec_rule_bucket_match（闭环批 2026-10-13 上移的
// 单一真相源——auditor 执行管线与 security.rs CLI 预览共用，桶语义在此
// 钉死：deny=任一段/整串兜底、allow=全段、空段显式不命中、env 逃生堵死）
// ---------------------------------------------------------------------------

fn deny_match(pattern: &str, target: &str) -> bool {
    let segs = exec_rule_segments(target);
    exec_rule_bucket_match(
        "deny",
        &pattern.to_lowercase(),
        &normalize_exec_command(target),
        &segs,
    )
}

fn allow_match(pattern: &str, target: &str) -> bool {
    let segs = exec_rule_segments(target);
    exec_rule_bucket_match(
        "allow",
        &pattern.to_lowercase(),
        &normalize_exec_command(target),
        &segs,
    )
}

/// deny：任一段命中即拦 + 显式 env 头不构成逃生（整串锚定兜底也剥 env）。
#[test]
fn exec_bucket_deny_any_segment_and_env_escape_blocked() {
    // 基线：非首段命中
    assert!(deny_match("rm -rf *", "echo hi && rm -rf y"));
    // env 前缀段：`FOO=1 rm -rf y` 的段剥 env 后命中 `rm -rf *`
    assert!(
        deny_match("rm -rf *", "echo hi && FOO=1 rm -rf y"),
        "env 头不得把段从 deny 眼皮下带走"
    );
    // 纯 env 无命令：段剥 env 为空串，`*` 规则（`.*` 匹配空）照拦；
    // 非 `*` 规则对空段/空串都打不中（deny 不误伤、也不凭空造拦截面）
    assert!(deny_match("*", "FOO=1"), "空命令仍被 `*` 拦");
    assert!(!deny_match("cargo *", "FOO=1"), "非 `*` 规则不打空命令");
}

/// 显式 env 形 deny 规则命中本尊形态（未剥整串臂）：pattern 侧剥 env 的
/// 只有 target，带 env 头的 pattern 只能靠未剥整串命中其字面输入——§九
/// D1 F4 声称的三臂联合在上移单一真相源时曾丢失（多角度审计批 D5 抓回、
/// D8 独立收敛），此锁钉死不丢第二次。
#[test]
fn exec_bucket_deny_env_form_pattern_matches_literal_target() {
    assert!(
        deny_match(
            "http_proxy=proxy.corp curl *",
            "HTTP_PROXY=proxy.corp curl http://evil",
        ),
        "显式 env 形 deny 规则必须命中其字面输入形态"
    );
    // 未剥臂只朝 pattern 作者写下的字面形态增长拦截面：非 env pattern
    // 的既有拦截面（剥 env 段/剥 env 整串）不变，纯 env 空命令不误拦。
    assert!(deny_match("cargo *", "FOO=1 cargo build"));
    assert!(!deny_match("cargo *", "FOO=1"));
    assert!(deny_match("*", "FOO=1"));
}

/// allow：全部段命中才放——任一段不命中即不放；空段集显式不放。
#[test]
fn exec_bucket_allow_requires_all_segments() {
    // 镜像洞基线：`echo hi && cargo publish` 不得被 `cargo *` 整体放行
    assert!(!allow_match("cargo *", "echo hi && cargo publish"));
    // 全段命中才放
    assert!(allow_match("cargo *", "cargo build && cargo test"));
    // env 头段剥后照常命中（allow 与 deny 同一剥 env 语义）
    assert!(
        allow_match("cargo *", "RUSTFLAGS=-g cargo build"),
        "env 头不得打断合法 allow"
    );
    // 空段集（纯操作符）显式不放
    let segs = exec_rule_segments("&&");
    assert!(segs.is_empty());
    assert!(!allow_match("*", "&&"), "空命令不得自动放行");
}

/// CLI 预览与 auditor 的输入契约：segment 输入须 trim 过——空白段原样
/// 透传（strip_env_assignments 契约），桶匹配前段已由 command_segments
/// trim，预览侧不存在空白段输入。
#[test]
fn exec_rule_segments_trims_via_command_segments() {
    let segs = exec_rule_segments("  cargo build  &&  rm x  ");
    assert_eq!(segs, vec!["cargo build", "rm x"]);
}
