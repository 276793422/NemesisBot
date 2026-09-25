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
