//! A4 测试：五级模糊替换级联的样例矩阵（每级 ≥1 正 1 反）+ 歧义行号 +
//! 跨度保护拒绝 + 级联顺序。纯函数测试（content/old/new 直入）。

use super::*;

// ---------------------------------------------------------------------------
// 级 1：line-trimmed
// ---------------------------------------------------------------------------

#[test]
fn line_trimmed_positive_preserves_file_indentation() {
    // 文件 tab 缩进、模型给空格缩进：逐行 trim 相等 → 命中；替换保留
    // 文件原缩进（tab），模型缩进不覆盖文件风格。
    let content = "fn a() {\n\treturn 1;\n}\n";
    let old = "fn a() {\n  return 1;\n}\n";
    let new = "fn a() {\n  return 2;\n}\n";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "line-trimmed");
    assert_eq!(m.content, "fn a() {\n\treturn 2;\n}\n");
}

#[test]
fn line_trimmed_negative_line_count_differs_falls_through() {
    // 行数不一致 → line-trimmed 不适用；其余级也无锚（z 行不存在）→ 全败。
    let content = "x = 1\ny = 2\n";
    let old = "x = 1\n\nz = 9\n";
    let new = "ok\n";
    match cascade_replace(content, old, new) {
        Err(CascadeError::NoMatch { span_note: None }) => {}
        other => panic!("expected NoMatch without span note, got {other:?}"),
    }
}

#[test]
fn line_trimmed_multi_hit_reports_ambiguous_with_lines() {
    let content = "  foo\nbar\n  foo\nbar\n";
    let old = "foo\nbar\n";
    let new = "baz\nqux\n";
    let err = cascade_replace(content, old, new).unwrap_err();
    let CascadeError::Ambiguous(msg) = err else {
        panic!("expected Ambiguous, got {err:?}");
    };
    assert!(msg.contains("2 times via line-trimmed"), "{msg}");
    assert!(msg.contains("[1, 3]"), "{msg}");
}

// ---------------------------------------------------------------------------
// 级 2：block-anchor
// ---------------------------------------------------------------------------

#[test]
fn block_anchor_positive_similar_middle_replaced_verbatim() {
    // 首尾行 trim 唯一成对锚定；中段语言风格差异（int vs let）相似度足够。
    // line-trimmed 不接（中段逐行不等），block-anchor 接住，new 原样落。
    let content = "start\nint a = 1;\nint b = 2;\nend\n";
    let old = "start\nlet a = 1;\nlet b = 2;\nend\n";
    let new = "start\nNEW;\nend\n";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "block-anchor");
    assert_eq!(m.content, "start\nNEW;\nend\n");
}

#[test]
fn block_anchor_negative_dissimilar_middle() {
    // 中段内容毫不相干（相似度 <0.65）→ 该级 0 候选；其余级也不接 → 全败。
    let content = "start\nalpha beta gamma delta epsilon\nend\n";
    let old = "start\n1234567890 0987654321\nend\n";
    let new = "x\n";
    match cascade_replace(content, old, new) {
        Err(CascadeError::NoMatch { span_note: None }) => {}
        other => panic!("expected NoMatch, got {other:?}"),
    }
}

#[test]
fn block_anchor_ambiguous_pairs_reports_first_anchor_lines() {
    // old 4 行 vs 文件 3 行锚块（中段少一行但相似）：line-trimmed 因行数
    // 不符不接，block-anchor 找到两对锚 → 歧义报首锚行号。
    let content = "start\nm1\nend\nzzz\nstart\nm1\nend\n";
    let old = "start\nm1\nm2\nend\n";
    let new = "x\n";
    let err = cascade_replace(content, old, new).unwrap_err();
    let CascadeError::Ambiguous(msg) = err else {
        panic!("expected Ambiguous, got {err:?}");
    };
    assert!(msg.contains("2 times via block-anchor"), "{msg}");
    assert!(msg.contains("[1, 5]"), "{msg}");
}

#[test]
fn block_anchor_blank_anchor_lines_are_not_anchors() {
    // old 首尾都是空白行：空白锚处处命中无意义 → 该级弃权（交给
    // trimmed-boundary 接住：剥空行后核心精确匹配）。
    let content = "a\nb\nc\n";
    let old = "\na\nb\n";
    let new = "X\n";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "trimmed-boundary");
    assert_eq!(m.content, "X\nc\n");
}

// ---------------------------------------------------------------------------
// 级 3：whitespace-normalized
// ---------------------------------------------------------------------------

#[test]
fn whitespace_normalized_positive_collapses_inner_runs() {
    // 行内连续空白折叠：line-trimmed 不接（端点 trim 不动行中部的空白）。
    let content = "foo    bar\nbaz\n";
    let old = "foo bar\n";
    let new = "foo bar!\n";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "whitespace-normalized");
    assert_eq!(m.content, "foo bar!\nbaz\n");
}

#[test]
fn whitespace_normalized_negative_different_content() {
    let content = "foo    bar\nbaz\n";
    let old = "foo qux\n";
    let new = "x\n";
    match cascade_replace(content, old, new) {
        Err(CascadeError::NoMatch { span_note: None }) => {}
        other => panic!("expected NoMatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 级 4：indentation-flexible（常规被 line-trimmed 遮蔽，直测本函数）
// ---------------------------------------------------------------------------

#[test]
fn indentation_flexible_positive_applies_delta_to_new_lines() {
    // 文件整块比 old 深 4 空格（统一 delta=Add(4sp)）：匹配成功且 new 各
    // 行获得 +4sp 平移（空白行不动）。
    let content = "        return 1;\n        }\n";
    let old = "    return 1;\n    }\n";
    let new = "    return 2;\n    }\n";
    let m = indentation_flexible(content, old, new).unwrap();
    let LevelOutcome::Match { content } = m else {
        panic!("expected Match, got {m:?}");
    };
    assert_eq!(content, "        return 2;\n        }\n");
}

#[test]
fn indentation_flexible_negative_non_uniform_delta() {
    // 各行缩进差不一致（+2sp 与 +4sp 混排）→ 无统一 delta → 0 候选。
    let content = "      return 1;\n        }\n";
    let old = "    return 1;\n    }\n";
    let new = "x\n";
    let m = indentation_flexible(content, old, new).unwrap();
    assert!(matches!(m, LevelOutcome::NoCandidate), "{m:?}");
}

#[test]
fn indentation_flexible_strip_delta_removes_indent_from_new() {
    // 文件整块比 old 浅 4 空格（delta=Strip）：new 的对应缩进被剥掉。
    let content = "  return 1;\n  }\n";
    let old = "      return 1;\n      }\n";
    let new = "      return 2;\n      }\n";
    let m = indentation_flexible(content, old, new).unwrap();
    let LevelOutcome::Match { content } = m else {
        panic!("expected Match, got {m:?}");
    };
    assert_eq!(content, "  return 2;\n  }\n");
}

// ---------------------------------------------------------------------------
// 级 5：trimmed-boundary
// ---------------------------------------------------------------------------

#[test]
fn trimmed_boundary_positive_strips_old_blank_lines() {
    // 见 block_anchor_blank_anchor_lines_are_not_anchors 的正向路径；
    // 这里测 old 尾部空白行差异。
    let content = "a\nb\nc\n";
    let old = "a\nb\n\n";
    let new = "A\nB\n";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "trimmed-boundary");
    assert_eq!(m.content, "A\nB\nc\n");
}

#[test]
fn trimmed_boundary_negative_ambiguous_core() {
    let content = "a\nb\na\nb\n";
    let old = "\na\nb\n";
    let new = "x\n";
    let err = cascade_replace(content, old, new).unwrap_err();
    let CascadeError::Ambiguous(msg) = err else {
        panic!("expected Ambiguous, got {err:?}");
    };
    assert!(msg.contains("2 times via trimmed-boundary"), "{msg}");
    assert!(msg.contains("[1, 3]"), "{msg}");
}

// ---------------------------------------------------------------------------
// 跨度保护 + 级联纪律
// ---------------------------------------------------------------------------

#[test]
fn span_guard_rejects_disproportionate_candidate() {
    // old 无缩进（每行内容唯一，block-anchor 锚唯一成对不歧义）、文件全量
    // 30 空格缩进（span ≈3.2KB > 4×|old|+1024）：各级唯一候选全部超限被拒
    // → 全败且 span_note 说明拒绝原因。
    let bloated: String = (0..100)
        .map(|k| format!("{}x{k}\n", " ".repeat(30)))
        .collect();
    let old: String = (0..100).map(|k| format!("x{k}\n")).collect();
    let new = "y\n".repeat(100);
    match cascade_replace(&bloated, &old, &new) {
        Err(CascadeError::NoMatch {
            span_note: Some(note),
        }) => {
            assert!(note.contains("disproportionate"), "{note}");
            assert!(note.contains("limit"), "{note}");
        }
        other => panic!("expected NoMatch with span note, got {other:?}"),
    }
}

#[test]
fn span_within_limit_still_matches_via_line_trimmed() {
    // 同形态但规模缩小到限内：line-trimmed 正常命中（缩进保留语义）。
    let bloated: String = (0..5).map(|_| format!("{}x\n", " ".repeat(10))).collect();
    let old: String = "x\n".repeat(5);
    let new = "y\n".repeat(5);
    let m = cascade_replace(&bloated, &old, &new).unwrap();
    assert_eq!(m.level, "line-trimmed");
    let expected: String = (0..5).map(|_| format!("{}y\n", " ".repeat(10))).collect();
    assert_eq!(m.content, expected);
}

#[test]
fn empty_old_text_goes_straight_to_no_match() {
    match cascade_replace("abc\n", "", "x") {
        Err(CascadeError::NoMatch { span_note: None }) => {}
        other => panic!("expected NoMatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// CRLF + EOF 末行边界
// ---------------------------------------------------------------------------

#[test]
fn crlf_file_stays_crlf_after_fuzzy_replace() {
    // CRLF 文件：替换行的终止符沿用 span 首行风格，文件整体保持 CRLF。
    let content = "fn a() {\r\n\treturn 1;\r\n}\r\n";
    let old = "fn a() {\n\treturn 1;\n}\n";
    let new = "fn a() {\n\treturn 2;\n}\n";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "line-trimmed");
    assert_eq!(m.content, "fn a() {\r\n\treturn 2;\r\n}\r\n");
}

#[test]
fn eof_last_line_without_terminator_is_replaced_cleanly() {
    // EOF 末行无终止符：替换不引入多余换行。
    let content = "a\n\treturn 1;";
    let old = "a\n  return 1;";
    let new = "a\n  return 2;";
    let m = cascade_replace(content, old, new).unwrap();
    assert_eq!(m.level, "line-trimmed");
    assert_eq!(m.content, "a\n\treturn 2;");
}
