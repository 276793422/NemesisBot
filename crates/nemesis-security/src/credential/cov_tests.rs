// credential.rs 覆盖率补充测试（scan_tool_output 的命中-warn 与干净-debug
// 双臂 169/170/176）。
//
// 豁免（死防御臂）：98 / 104（匹配值 ≤4 字符的 else 臂）——内置
// PATTERNS 表（229-…）所有模式的最短匹配长度都大于 4（如 twilio_sid
// 2+32=34 字符、aws_access_key 20 字符），且扫描器无自定义模式注入
// 接口，`full.len() > 4` 恒真，两个 else 臂不可达。

use super::*;

/// 命中：warn 臂（169-170）+ 匹配明细脱敏正确。
#[test]
fn scan_tool_output_warns_and_reports_on_match() {
    let scanner = Scanner::new(true, "block");
    let secret = "aws_key = AKIAIOSFODNN7EXAMPLE";
    let r = scanner.scan_tool_output("write_file", secret);
    assert!(r.has_matches, "{:?}", r.summary);
    assert!(r.matches.iter().any(|m| m.pattern_name == "aws_access_key"));
    // 前缀保留头 4 字符、尾段含后 4 字符（len>4 的主臂）。
    let m = &r.matches[0];
    assert_eq!(m.full_match_start, "AKIA");

    // 再扫干净文本 → debug 臂（176）。
    let clean = scanner.scan_tool_output("read_file", "just some ordinary notes");
    assert!(!clean.has_matches);
}

/// 禁用扫描器 → 不报匹配（对照）。
#[test]
fn disabled_scanner_passes_through() {
    let scanner = Scanner::new(false, "block");
    let r = scanner.scan_tool_output("write_file", "AKIAIOSFODNN7EXAMPLE");
    assert!(!r.has_matches);
}
