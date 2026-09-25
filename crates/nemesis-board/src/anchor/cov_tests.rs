// anchor.rs 覆盖率补充测试（解析边角 128/147/171-172 / AnchorKind::as_str
// ContentRegex 臂 48 / run 缺文件与目录目标 216/272-273 / contains 命中
// 252 / 形态闸越界组件与 8.3 短名 308 / junction 越出 workspace 342）。
//
// 平台豁免：286（UNC）/ 299（`\foo` 根相对）在 Windows 上被更早的
// is_absolute 臂吞掉（Path::is_absolute 对 UNC/根相对恒真）——这两臂只为
// 非 Windows 平台可达，Windows 测试不硬凑。

use super::*;
use std::path::PathBuf;

fn temp_ws(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("nmb-anchor-cov-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

/// 解析边角：`[CHECK] re:` 纯文本正则形态（171-172）→ ContentRegex 锚点；
/// 无谓词 file:（128）；contains: 空关键词（147）回落语义项。
#[test]
fn parse_edge_forms() {
    let (anchors, _, _) = parse_anchors("[CHECK] re:fin.*done");
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].kind, AnchorKind::ContentRegex);
    assert_eq!(anchors[0].kind.as_str(), "交付文本正则");

    // file: 目标后无谓词 → 未知谓词 → 语义项回落（不产锚点）。
    let (plain, _, _) = parse_anchors("[CHECK] file:src/main.rs");
    assert!(plain.is_empty(), "无谓词不得产生锚点");

    // contains: 空关键词 → 同样回落（不产锚点）。
    let (empty, _, _) = parse_anchors("[CHECK] file:a.txt contains:");
    assert!(empty.is_empty(), "空关键词不得产生锚点");
}

/// FileExists 目标不存在 → fail 且明细带「不存在」（216 的 Err 臂）；
/// contains 谓词命中真实文件（252 的 true 臂）。
#[test]
fn run_missing_file_and_contains_hit() {
    let ws = temp_ws("run");
    std::fs::write(ws.join("src/main.rs"), "fn main() {}\n// NEEDLE\n").unwrap();

    let (anchors, _, _) = parse_anchors("[CHECK] file:src/gone.txt exists");
    let results = run_anchors(&anchors, &ws, "");
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("不存在"),
        "{}",
        results[0].detail
    );

    let (hit, _, _) = parse_anchors("[CHECK] file:src/main.rs contains:NEEDLE");
    let results = run_anchors(&hit, &ws, "");
    assert!(results[0].passed, "{}", results[0].detail);

    let _ = std::fs::remove_dir_all(&ws);
}

/// 目标是目录：形态闸与 resolve 都放行，read_to_string 失败 → fail
/// （272-273 的非 UTF-8/IO 错误臂）。
#[test]
fn run_directory_target_fails_honestly() {
    let ws = temp_ws("dir");
    let (anchors, _, _) = parse_anchors("[CHECK] file:src contains:x");
    let results = run_anchors(&anchors, &ws, "");
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("文件读取失败"),
        "{}",
        results[0].detail
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// 形态闸直调：空路径 / `..` 越界组件 / 8.3 短名（308 的 Normal+short
/// 分支）全部拒绝。
#[test]
fn validate_shape_rejects_edge_forms() {
    assert!(validate_anchor_path_shape("").is_err());
    assert!(validate_anchor_path_shape("a/../b.txt").is_err());
    assert!(validate_anchor_path_shape("RUNNE~1/evil.txt").is_err());
    assert!(validate_anchor_path_shape("BASE~1.tar").is_err());
    assert!(validate_anchor_path_shape("normal.txt").is_ok());
}

/// junction 指向 workspace 外 → 运行时闸判越出（342）。junction 创建
/// 失败（权限/环境）则静默跳过。
#[cfg(windows)]
#[test]
fn junction_out_of_workspace_is_rejected() {
    let ws = temp_ws("junc");
    let outside = temp_ws("outside");
    std::fs::write(outside.join("secret.txt"), "outside").unwrap();

    let link = ws.join("link");
    let made = std::process::Command::new("cmd")
        .args([
            "/c",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &outside.to_string_lossy(),
        ])
        .output();
    if let Ok(out) = made
        && out.status.success()
    {
        let (anchors, _, _) = parse_anchors("[CHECK] file:link/secret.txt exists");
        let results = run_anchors(&anchors, &ws, "");
        assert!(!results[0].passed, "junction 越界必须 fail");
        assert!(
            results[0].detail.contains("越出 workspace"),
            "{}",
            results[0].detail
        );
    }

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ===========================================================================
// wave6 追加：as_str FileRegex/FileContains 臂、解析自由文本回落、run_one
// 的防御臂（pattern 缺失 / 正则编译失败——手工构造 AnchorCheck 直达）、
// 文件内容类锚点目标缺失、超限文件拒绝读取。
// ===========================================================================

/// AnchorKind::as_str 的文件正则/文件包含臂（48-49）。
#[test]
fn w6_anchor_kind_as_str_regex_and_contains() {
    assert_eq!(AnchorKind::FileRegex.as_str(), "文件正则");
    assert_eq!(AnchorKind::FileContains.as_str(), "文件包含");
}

/// `[CHECK]` 后跟自由文本（非 file:/re: 谓词）→ Ok(None) 回落语义项
///（171-172）。
#[test]
fn w6_parse_plain_check_line_falls_back_to_semantic() {
    let (anchors, semantic, rejected) = parse_anchors("[CHECK] 验收通过即可");
    assert!(anchors.is_empty());
    assert!(rejected.is_empty());
    assert_eq!(semantic, vec!["[CHECK] 验收通过即可".to_string()]);
}

/// run_one：ContentRegex 的 pattern 缺失防御臂（200）——手工构造直达。
#[test]
fn w6_run_content_regex_missing_pattern() {
    let anchor = AnchorCheck {
        raw: "[CHECK] re:x".to_string(),
        kind: AnchorKind::ContentRegex,
        target: String::new(),
        pattern: None,
    };
    let results = run_anchors(&[anchor], std::path::Path::new("."), "交付完成");
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("缺少正则模式"),
        "{}",
        results[0].detail
    );
}

/// run_one：ContentRegex 正则编译失败防御臂（216）。
#[test]
fn w6_run_content_regex_compile_failure() {
    let anchor = AnchorCheck {
        raw: "[CHECK] re:(".to_string(),
        kind: AnchorKind::ContentRegex,
        target: String::new(),
        pattern: Some("(".to_string()),
    };
    let results = run_anchors(&[anchor], std::path::Path::new("."), "交付完成");
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("正则编译失败"),
        "{}",
        results[0].detail
    );
}

/// run_one：文件内容类锚点目标缺失 → 路径解析失败臂（228）。
#[test]
fn w6_run_file_content_anchor_missing_file() {
    let ws = temp_ws("w6miss");
    let (anchors, _, _) = parse_anchors("[CHECK] file:src/gone.txt re:foo");
    assert_eq!(anchors.len(), 1);
    let results = run_anchors(&anchors, &ws, "");
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("不存在"),
        "{}",
        results[0].detail
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// run_one：FileRegex 的 pattern 缺失 / 编译失败防御臂（237 / 247）。
#[test]
fn w6_run_file_regex_defensive_arms() {
    let ws = temp_ws("w6fr");
    std::fs::write(ws.join("a.txt"), b"content\n").unwrap();
    let missing = AnchorCheck {
        raw: "[CHECK] file:a.txt re:x".to_string(),
        kind: AnchorKind::FileRegex,
        target: "a.txt".to_string(),
        pattern: None,
    };
    let results = run_anchors(&[missing], &ws, "");
    assert!(
        results[0].detail.contains("缺少正则模式"),
        "{}",
        results[0].detail
    );

    let bad = AnchorCheck {
        raw: "[CHECK] file:a.txt re:(".to_string(),
        kind: AnchorKind::FileRegex,
        target: "a.txt".to_string(),
        pattern: Some("(".to_string()),
    };
    let results = run_anchors(&[bad], &ws, "");
    assert!(
        results[0].detail.contains("正则编译失败"),
        "{}",
        results[0].detail
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// run_one：FileContains 的 pattern 缺失防御臂（252）。
#[test]
fn w6_run_file_contains_missing_pattern() {
    let ws = temp_ws("w6fc");
    std::fs::write(ws.join("a.txt"), b"content\n").unwrap();
    let anchor = AnchorCheck {
        raw: "[CHECK] file:a.txt contains:x".to_string(),
        kind: AnchorKind::FileContains,
        target: "a.txt".to_string(),
        pattern: None,
    };
    let results = run_anchors(&[anchor], &ws, "");
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("缺少关键词"),
        "{}",
        results[0].detail
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// read_limited：超限文件拒绝读取（269-273）。64MB+1 全零文件一次性写入。
#[test]
fn w6_read_limited_rejects_oversize_file() {
    let ws = temp_ws("w6big");
    let big = ws.join("big.bin");
    let mut data = vec![0u8; MAX_ANCHOR_FILE_BYTES as usize + 1];
    data[0] = b'x';
    std::fs::write(&big, &data).unwrap();
    let (anchors, _, _) = parse_anchors("[CHECK] file:big.bin contains:x");
    let results = run_anchors(&anchors, &ws, "");
    assert!(!results[0].passed);
    assert!(results[0].detail.contains("超过"), "{}", results[0].detail);
    assert!(results[0].detail.contains("64MB"), "{}", results[0].detail);
    let _ = std::fs::remove_dir_all(&ws);
}
