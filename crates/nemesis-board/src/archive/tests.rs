//! archive 项目档案目录测试（看板项目档案 goal P2/B1-B6；2026-09-17 补齐）。
//!
//! 覆盖：`sanitize_project_name` 纯函数矩阵（非法字符/连续塌缩/多字节截断/
//! 空名兜底）、`default_directory` 序号分配与 canonical 去重、
//! `resolve_project_directory` 校验矩阵（相对路径/8.3 短名/workspace 双向
//! 重叠/项目目录重叠/自动分配+mkdir）、`ensure_scaffold` 首建四件套 + 幂等、
//! `write_manifest`/`read_manifest` 往返与降级读、`append_timeline` 结构化行。
//!
//! 注：本文件在上游提交 `4edbaf61` 中声明（archive.rs 末尾 `mod tests;`）
//! 但从未被创建——`cargo test -p nemesis-board` / `--workspace` 全新构建
//! 即 E0583。2026-09-17 由修复 goal 补齐（先占位后实装）。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// 唯一临时目录（crate 无 tempfile 依赖，与 archive_writer/tests.rs 同款）。
fn temp_root(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-arch-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// 短名链直传说明：CI Windows runner 的 %TEMP% 落在 `C:\Users\RUNNER~1\...`
// （用户名本身是 8.3 短名形态）。2026-09-18 实录曾让 resolve 系列先撞
// 「8.3 词法拦截」而非被测分支——archive.rs 修复后 8.3 检查先展开盘上
// 存在的祖先链（RUNNER~1 → 真名）再匹配，因此这里**故意用 temp_root
// 原始形态直传**：本机（无短名）与 CI（短名链）都必须按被测分支分流。

// ---------------------------------------------------------------------------
// sanitize_project_name（B2 纯函数矩阵）
// ---------------------------------------------------------------------------

#[test]
fn sanitize_folds_whitespace_and_illegal_chars() {
    // 空白 → `_`；Windows 非法字符 `< > : " / \ | ? *` → `_`；控制字符 → `_`。
    assert_eq!(sanitize_project_name("hello world"), "hello_world");
    assert_eq!(
        sanitize_project_name("a<b>c:d\"e/f\\g|h|i?j*k"),
        "a_b_c_d_e_f_g_h_i_j_k"
    );
    assert_eq!(sanitize_project_name("a\u{0007}b"), "a_b");
}

#[test]
fn sanitize_collapses_underscores_and_trims_edges() {
    // 连续 `_` 塌缩；首尾 `._ ` 修剪（Windows 尾点/尾空格陷阱）。
    assert_eq!(sanitize_project_name("a____b"), "a_b");
    assert_eq!(sanitize_project_name("..__proj__.."), "proj");
    assert_eq!(sanitize_project_name("  spaced  "), "spaced");
}

#[test]
fn sanitize_truncates_multibyte_at_40_chars() {
    // 截断不变量：输出**字节数 ≤ 40** 且不撕裂多字节字符（实现 = 40 字节
    // 上限向下取整到 char 边界；doc 注释写"40 字符"对纯 ASCII 与实现一致，
    // 多字节下实际更短——安全方向，此处按实现语义钉死字节不变量）。
    let long_cn = "项".repeat(50); // 150 字节
    let out = sanitize_project_name(&long_cn);
    assert!(out.len() <= 40, "字节上限: {}", out.len());
    assert!(out.is_char_boundary(out.len()), "不得撕裂多字节字符");
    assert!(out.chars().all(|c| c == '项'), "截断不丢前缀: {out}");
    // 混合字节长度同样不越界 + 前缀保真。
    let mixed = format!("{}{}", "a".repeat(39), "项目名称很长很长");
    let out = sanitize_project_name(&mixed);
    assert!(out.len() <= 40, "字节上限: {}", out.len());
    assert!(out.starts_with(&"a".repeat(39)));
}

#[test]
fn sanitize_empty_yields_project_default() {
    assert_eq!(sanitize_project_name(""), "project");
    assert_eq!(sanitize_project_name("   "), "project");
    // 全部由可修剪字符构成 → 修剪后为空 → 兜底。
    assert_eq!(sanitize_project_name("._ ._"), "project");
}

// ---------------------------------------------------------------------------
// default_directory（B2 序号分配）
// ---------------------------------------------------------------------------

#[test]
fn default_directory_allocates_unique_with_sequence() {
    let ws = temp_root("alloc");
    std::fs::create_dir_all(ws.join(BOARD_PROJECTS_DIR)).unwrap();

    // 首次：`board-projects/<名>`。
    let first = default_directory(&ws, "alpha", &[]).unwrap();
    assert_eq!(first, ws.join(BOARD_PROJECTS_DIR).join("alpha"));

    // 落盘模拟已占用 → 序号 `-2`、`-3`。
    std::fs::create_dir_all(&first).unwrap();
    let second = default_directory(&ws, "alpha", &[]).unwrap();
    assert_eq!(
        second,
        ws.join(BOARD_PROJECTS_DIR).join("alpha-2"),
        "重名追加序号"
    );

    // existing 集合（不落盘）同样撞序号：传 second 的 canonical → `-3`。
    let existing = vec![nemesis_path::canonicalize_for_compare(&second)];
    let third = default_directory(&ws, "alpha", &existing).unwrap();
    assert_eq!(third, ws.join(BOARD_PROJECTS_DIR).join("alpha-3"));

    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn default_directory_respects_existing_canonical_set() {
    // 词法不同、canonical 相同的目录（`.\alpha` vs `alpha`）也要撞出序号
    // ——去重比对在 canonical 域进行。
    let ws = temp_root("alloc_canon");
    let base = ws.join(BOARD_PROJECTS_DIR).join("beta");
    std::fs::create_dir_all(&base).unwrap();
    let dotted = ws.join(".").join(BOARD_PROJECTS_DIR).join("beta");
    let existing = vec![nemesis_path::canonicalize_for_compare(&dotted)];
    let out = default_directory(&ws, "beta", &existing).unwrap();
    assert_ne!(out, base, "canonical 相同的 existing 必须撞出序号");
    assert_eq!(out, ws.join(BOARD_PROJECTS_DIR).join("beta-2"));
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// resolve_project_directory（B2 校验矩阵）
// ---------------------------------------------------------------------------

#[test]
fn resolve_rejects_relative_path() {
    let ws = temp_root("res_rel");
    let err = resolve_project_directory(Some("relative/dir"), &ws, "p", &[]).unwrap_err();
    assert!(err.contains("绝对路径"), "{err}");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn resolve_rejects_short_name_components() {
    // 8.3 短名（`AAAAAA~1`）词法拦截——canonicalize 之前就拒绝。
    let ws = temp_root("res_83");
    let evil = std::env::temp_dir().join("SOMEDI~1").join("proj");
    let err = resolve_project_directory(Some(&evil.to_string_lossy()), &ws, "p", &[]).unwrap_err();
    assert!(err.contains("8.3 短名"), "{err}");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn resolve_rejects_workspace_overlap_both_directions() {
    let ws = temp_root("res_ws");
    // 项目目录在主 workspace 之内 → 拒绝。
    let inside = ws.join("sub").join("proj");
    let err =
        resolve_project_directory(Some(&inside.to_string_lossy()), &ws, "p", &[]).unwrap_err();
    assert!(err.contains("主 workspace 重叠"), "{err}");
    // 主 workspace 在项目目录之内（反向 contains）→ 同样拒绝。
    let err = resolve_project_directory(Some(&ws.to_string_lossy()), &ws, "p", &[]).unwrap_err();
    assert!(err.contains("主 workspace 重叠"), "{err}");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn resolve_rejects_existing_project_overlap() {
    let ws = temp_root("res_peer");
    let other = temp_root("res_peer_other");
    std::fs::create_dir_all(&other).unwrap();
    let existing = vec![nemesis_path::canonicalize_for_compare(&other)];
    // 子目录重叠（项目目录在既有项目目录之下）→ 拒绝。
    let child = other.join("child");
    let err =
        resolve_project_directory(Some(&child.to_string_lossy()), &ws, "p", &existing).unwrap_err();
    assert!(err.contains("其他项目目录重叠"), "{err}");
    // 父目录重叠（既有项目目录在项目目录之下）→ 同样拒绝。
    let err =
        resolve_project_directory(Some(&other.to_string_lossy()), &ws, "p", &existing).unwrap_err();
    assert!(err.contains("其他项目目录重叠"), "{err}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn resolve_auto_allocates_and_creates_dir() {
    let ws = temp_root("res_auto");
    // requested = None → 自动分配 + mkdir + 返回 canonical 显示路径。
    let out = resolve_project_directory(None, &ws, "gamma", &[]).unwrap();
    assert!(out.is_dir(), "自动分配目录必须已创建");
    assert!(
        out.starts_with(nemesis_path::canonicalize_for_compare(
            &ws.join(BOARD_PROJECTS_DIR)
        )),
        "落在 board-projects/ 下: {}",
        out.display()
    );
    // 已存在目录是合法形态（B5：用户选已有目录作初始基线）→ 直接使用。
    let again = resolve_project_directory(None, &ws, "gamma", &[]).unwrap();
    assert_ne!(out, again, "第二次自动分配撞已存在目录 → 序号");
    // 已存在同名【文件】拒绝（路径须在 workspace 外——否则先命中重叠拒绝）。
    // 短名链直传（见文件头说明）：CI 的 %TEMP% 带 RUNNER~1 短名组件，
    // 8.3 检查须展开后放行，才能到达被测的「已是文件」分支。
    let outside = temp_root("res_auto_file");
    std::fs::create_dir_all(&outside).unwrap();
    let file = outside.join("afile");
    std::fs::write(&file, b"x").unwrap();
    let err = resolve_project_directory(Some(&file.to_string_lossy()), &ws, "p", &[]).unwrap_err();
    assert!(err.contains("已是文件") || err.contains("是文件"), "{err}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// ensure_scaffold + manifest + timeline（B3/B4/B5/B6）
// ---------------------------------------------------------------------------

#[test]
fn ensure_scaffold_first_run_creates_all_and_idempotent() {
    let root = temp_root("scaffold");

    // 首次：四件套 + 子目录 + fresh=true。
    assert!(ensure_scaffold(&root, 7, "proj", "active").unwrap());
    assert!(root.join("project.json").exists());
    assert!(root.join("timeline.jsonl").exists());
    assert!(root.join(".gitignore").exists());
    for sub in ["docs/review", "artifacts", "records"] {
        assert!(root.join(sub).is_dir(), "{sub} 必须存在");
    }
    // manifest 内容落盘正确。
    let m = read_manifest(&root).expect("manifest 可读");
    assert_eq!(m.project_id, 7);
    assert_eq!(m.name, "proj");
    assert_eq!(m.status, "active");
    assert_eq!(m.integrity, "ok");
    assert!(m.missing_blocks.is_empty());

    // 幂等：用户改动不被覆盖（B5 语义），fresh=false。
    std::fs::write(root.join("timeline.jsonl"), b"user data").unwrap();
    std::fs::write(root.join(".gitignore"), b"user rules").unwrap();
    assert!(!ensure_scaffold(&root, 9, "renamed", "done").unwrap());
    assert_eq!(
        std::fs::read(root.join("timeline.jsonl")).unwrap(),
        b"user data"
    );
    assert_eq!(
        std::fs::read(root.join(".gitignore")).unwrap(),
        b"user rules"
    );
    // project.json 未重建（保留首次内容）。
    assert_eq!(read_manifest(&root).unwrap().project_id, 7);

    // .gitignore 内容含四个投影排除项（B6）。
    assert!(GITIGNORE_CONTENT.contains("/project.json"));
    assert!(GITIGNORE_CONTENT.contains("/timeline.jsonl"));
    assert!(GITIGNORE_CONTENT.contains("/docs/"));
    assert!(GITIGNORE_CONTENT.contains("/records/"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn manifest_roundtrip_and_degraded_read() {
    let root = temp_root("manifest");
    std::fs::create_dir_all(&root).unwrap();

    // 缺失 → None（投影可重建，不炸）。
    assert!(read_manifest(&root).is_none());

    // 往返：字段保真。
    let m = ProjectManifest {
        project_id: 42,
        name: "roundtrip".into(),
        status: "active".into(),
        integrity: "ok".into(),
        missing_blocks: vec!["artifacts/x.bin".into()],
        created_at: "2026-09-17T12:00:00+08:00".into(),
    };
    write_manifest(&root, &m).unwrap();
    let back = read_manifest(&root).unwrap();
    assert_eq!(back.project_id, 42);
    assert_eq!(back.name, "roundtrip");
    assert_eq!(back.missing_blocks, vec!["artifacts/x.bin".to_string()]);

    // 损坏 JSON → None（诚实降级）。
    std::fs::write(root.join("project.json"), b"{broken json").unwrap();
    assert!(read_manifest(&root).is_none());

    // 二次序列化 fixpoint（写出的文件再读再写字节稳定）。
    write_manifest(&root, &m).unwrap();
    let first = std::fs::read_to_string(root.join("project.json")).unwrap();
    write_manifest(&root, &read_manifest(&root).unwrap()).unwrap();
    assert_eq!(
        first,
        std::fs::read_to_string(root.join("project.json")).unwrap()
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn append_timeline_writes_structured_lines() {
    let root = temp_root("timeline");
    std::fs::create_dir_all(&root).unwrap();

    append_timeline(&root, "dispatch", Some("12"), "worker-1", "派发子任务").unwrap();
    append_timeline(&root, "deliver", None, "worker-2", "交付完成").unwrap();

    let raw = std::fs::read_to_string(root.join("timeline.jsonl")).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 2, "两次追加 = 两行 jsonl");
    let first: serde_json::Value = serde_json::from_str(lines[0]).expect("每行是合法 JSON");
    assert_eq!(first["kind"], "dispatch");
    assert_eq!(first["issue"], "12");
    assert_eq!(first["actor"], "worker-1");
    assert_eq!(first["summary"], "派发子任务");
    assert!(first["ts"].as_str().is_some(), "ts 必填");
    // issue=None 序列化为 null（字段在场，值可空）。
    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert!(second["issue"].is_null());

    let _ = std::fs::remove_dir_all(&root);
}
