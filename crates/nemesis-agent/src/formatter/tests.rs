//! A6：format-on-save 单元测试。
//!
//! 覆盖：内置表逐条 + 配置覆盖表优先 / render_args 占位符 / annotate 组合 /
//! spawn 链路（rustfmt 真格式化 .rs —— 工具链随 cargo 必在）/ 工具缺席静默 /
//! 超时 kill 静默（Windows=ping 计数挂起 / Unix=sleep）/ executor.enabled
//! 诚实停用 / enabled=false 零副作用 / 非 UTF-8 文件静默 / 成功但内容未变
//! 不注记。

use super::{annotate_reformat, format_on_save, render_args, resolve_spec};
use nemesis_config::FormatOnSaveConfig;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn cfg_enabled(formatters: BTreeMap<String, Vec<String>>) -> FormatOnSaveConfig {
    FormatOnSaveConfig {
        enabled: true,
        formatters,
    }
}

fn write_config(dir: &std::path::Path, config_json: serde_json::Value) -> PathBuf {
    let p = dir.join("config.json");
    std::fs::write(&p, config_json.to_string()).unwrap();
    p
}

/// `agents.defaults.format_on_save` 配置（可选 executor 段）。
fn config_json(format_on_save: serde_json::Value, executor: Option<bool>) -> serde_json::Value {
    let mut root = serde_json::json!({
        "agents": { "defaults": { "format_on_save": format_on_save } },
        "model_list": []
    });
    if let Some(b) = executor {
        root["executor"] = serde_json::json!({ "enabled": b });
    }
    root
}

fn enabled_json() -> serde_json::Value {
    serde_json::json!({ "enabled": true })
}

// ---------------------------------------------------------------------------
// resolve_spec：内置表 + 配置覆盖
// ---------------------------------------------------------------------------

#[test]
fn builtin_table_covers_planned_extensions() {
    let empty = BTreeMap::new();
    // rs → rustfmt
    let rs = resolve_spec("rs", &empty).unwrap();
    assert_eq!(rs.tool, "rustfmt");
    assert_eq!(rs.args, vec!["--edition", "2021", "{file}"]);
    // go → gofmt
    assert_eq!(resolve_spec("go", &empty).unwrap().tool, "gofmt");
    // 前端家族 → prettier
    for ext in ["ts", "tsx", "js", "jsx", "vue", "json", "md"] {
        assert_eq!(resolve_spec(ext, &empty).unwrap().tool, "prettier", "{ext}");
    }
    // py → black
    assert_eq!(resolve_spec("py", &empty).unwrap().tool, "black");
    // C 家族 → clang-format
    for ext in ["c", "h", "cpp"] {
        assert_eq!(
            resolve_spec(ext, &empty).unwrap().tool,
            "clang-format",
            "{ext}"
        );
    }
    // 未收录扩展名 → None
    assert!(resolve_spec("txt", &empty).is_none());
}

#[test]
fn config_override_wins_per_extension_and_falls_back_elsewhere() {
    let mut overrides = BTreeMap::new();
    overrides.insert(
        "py".to_string(),
        vec![
            "ruff".to_string(),
            "format".to_string(),
            "{file}".to_string(),
        ],
    );
    // 覆盖条目生效
    let py = resolve_spec("py", &overrides).unwrap();
    assert_eq!(py.tool, "ruff");
    assert_eq!(py.args, vec!["format", "{file}"]);
    // 未覆盖的扩展名回落内置
    assert_eq!(resolve_spec("rs", &overrides).unwrap().tool, "rustfmt");
    // 配置表可引入内置表没有的扩展名
    overrides.insert(
        "txt".to_string(),
        vec!["mytool".to_string(), "{file}".to_string()],
    );
    assert_eq!(resolve_spec("txt", &overrides).unwrap().tool, "mytool");
    // 空 argv 条目 = 未配置（回落内置，不炸）
    overrides.insert("rs".to_string(), vec![]);
    assert_eq!(resolve_spec("rs", &overrides).unwrap().tool, "rustfmt");
}

// ---------------------------------------------------------------------------
// render_args / annotate_reformat（纯函数）
// ---------------------------------------------------------------------------

#[test]
fn render_args_replaces_file_placeholder() {
    let args = vec!["--edition".to_string(), "{file}".to_string()];
    assert_eq!(render_args(&args, "a b.rs"), vec!["--edition", "a b.rs"]);
    // 无占位符参数原样
    assert_eq!(render_args(&["-q".to_string()], "x.rs"), vec!["-q"]);
}

#[test]
fn annotate_reformat_composes_result_tool_and_diff() {
    let out = annotate_reformat(
        "wrote 3 lines",
        "rustfmt",
        "src/lib.rs",
        "fn a(){\nlet x=1;\n}",
        "fn a() {\n    let x = 1;\n}\n",
    );
    assert!(out.starts_with("wrote 3 lines"));
    assert!(out.contains("[format] reformatted by rustfmt"));
    assert!(out.contains("--- a/src/lib.rs"));
    assert!(out.contains("-fn a(){"));
    assert!(out.contains("+fn a() {"));
}

// ---------------------------------------------------------------------------
// spawn 链路（真实进程）
// ---------------------------------------------------------------------------

/// 工具链随 cargo 必在（rust-toolchain.toml 钉死 1.95.0，Windows/Linux CI
/// 与开发机同源）——真格式化一条烂格式 .rs。
#[tokio::test]
async fn rustfmt_reformats_bad_rs_file_and_annotates_result() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ugly.rs");
    std::fs::write(&file, "fn a(){let x=1;}\n").unwrap();

    let config_path = write_config(dir.path(), config_json(enabled_json(), None));
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote file").await;

    assert!(
        out.contains("[format] reformatted by rustfmt"),
        "out: {out}"
    );
    assert!(out.contains("+fn a() {"), "out: {out}");
    // 磁盘上文件真的被格式化了（contains 断言容忍 rustfmt 版本细节差异）
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(on_disk.contains("fn a() {"), "on_disk: {on_disk}");
    assert!(on_disk.contains("let x = 1;"), "on_disk: {on_disk}");
}

/// 工具缺席：spawn NotFound → 静默原样（无注记、文件不动、不 panic）。
#[tokio::test]
async fn missing_formatter_tool_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.rs");
    std::fs::write(&file, "fn a(){}\n").unwrap();

    let overrides = BTreeMap::from([(
        "rs".to_string(),
        vec![
            "definitely-not-a-real-tool-a6xyz".to_string(),
            "{file}".to_string(),
        ],
    )]);
    let config_path = write_config(
        dir.path(),
        config_json(serde_json::json!(cfg_enabled(overrides)), None),
    );
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote file").await;
    assert_eq!(out, "wrote file", "missing tool must be silent");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn a(){}\n");
}

/// 超时：挂起工具被 kill，静默原样（挂起器 ping 计数/sleep，预算 3s 内必杀）。
#[tokio::test]
async fn hanging_formatter_is_killed_and_silent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.rs");
    std::fs::write(&file, "fn a(){}\n").unwrap();

    let argv: Vec<String> = if cfg!(windows) {
        vec!["ping".into(), "-n".into(), "30".into(), "127.0.0.1".into()]
    } else {
        vec!["sleep".into(), "30".into()]
    };
    let overrides = BTreeMap::from([("rs".to_string(), argv)]);
    let config_path = write_config(
        dir.path(),
        config_json(serde_json::json!(cfg_enabled(overrides)), None),
    );
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote file").await;
    assert_eq!(out, "wrote file", "timeout must be silent");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn a(){}\n");
}

/// executor.enabled=true → 诚实停用（开关开着也不 spawn、不注记）。
#[tokio::test]
async fn executor_separation_disables_format_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ugly.rs");
    std::fs::write(&file, "fn a(){let x=1;}\n").unwrap();

    let config_path = write_config(dir.path(), config_json(enabled_json(), Some(true)));
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote file").await;
    assert_eq!(out, "wrote file");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn a(){let x=1;}\n",
        "file must stay untouched under executor separation"
    );
}

/// 默认关：enabled=false（或缺段/standalone）零副作用。
#[tokio::test]
async fn disabled_by_default_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ugly.rs");
    std::fs::write(&file, "fn a(){let x=1;}\n").unwrap();

    // 显式 false
    let config_path = write_config(
        dir.path(),
        config_json(serde_json::json!({ "enabled": false }), None),
    );
    let out = format_on_save(Some(config_path.clone()), &file.to_string_lossy(), "wrote").await;
    assert_eq!(out, "wrote");
    // standalone（无 config 路径）
    let out = format_on_save(None, &file.to_string_lossy(), "wrote").await;
    assert_eq!(out, "wrote");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "fn a(){let x=1;}\n"
    );
}

/// 非 UTF-8 文件：前置读失败 → 静默（二进制不在文本表语义内）。
#[tokio::test]
async fn non_utf8_file_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blob.rs");
    std::fs::write(&file, [0xffu8, 0xfe, 0x00, 0x01]).unwrap();

    let config_path = write_config(dir.path(), config_json(enabled_json(), None));
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote").await;
    assert_eq!(out, "wrote");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        vec![0xffu8, 0xfe, 0x00, 0x01]
    );
}

/// 未收录扩展名（resolve 无命中）→ 静默。
#[tokio::test]
async fn unmatched_extension_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();

    let config_path = write_config(dir.path(), config_json(enabled_json(), None));
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote").await;
    assert_eq!(out, "wrote");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
}

/// 格式化成功但内容未变 → 不注记（Windows: `cmd /c rem` 成功不动文件；
/// Unix: `touch` 只动 mtime）。
#[tokio::test]
async fn formatter_success_but_unchanged_no_annotation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("x.rs");
    std::fs::write(&file, "fn a(){}\n").unwrap();

    let argv: Vec<String> = if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "rem".into()]
    } else {
        vec!["touch".into(), "{file}".into()]
    };
    let overrides = BTreeMap::from([("rs".to_string(), argv)]);
    let config_path = write_config(
        dir.path(),
        config_json(serde_json::json!(cfg_enabled(overrides)), None),
    );
    let out = format_on_save(Some(config_path), &file.to_string_lossy(), "wrote").await;
    assert_eq!(out, "wrote", "success-but-unchanged must not annotate");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fn a(){}\n");
}
