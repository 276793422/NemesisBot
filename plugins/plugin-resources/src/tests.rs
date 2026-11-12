//! 插件单测：机械裁剪规则 / manifest 组装校验 / UTC 格式化 / 内嵌产物与
//! ABI 导出面自检（cargo test 会先跑 build.rs——内嵌产物必然在場）。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use crate::catalog::catalog;
use crate::resource_filter::{
    self, OUT_FILE, assemble_catalog, date_part, format_epoch_secs, is_md, normalize_path,
    strip_tarball_root, validate_entry,
};
use crate::{prs_abi_version, prs_free, prs_get, prs_manifest};

// ---------------------------------------------------------------------------
// 机械裁剪与路径归一
// ---------------------------------------------------------------------------

#[test]
fn is_md_keeps_all_markdown() {
    assert!(is_md("engineering/toolsmith.md"));
    assert!(is_md("README.md"));
    assert!(is_md(".github/PULL_REQUEST_TEMPLATE.md"));
    assert!(is_md("X.MD"));
    // 语义过滤（README/CONTRIBUTING 等）不在这里——宿主消费侧职责。
    assert!(is_md("README.md"), "机械规则不做语义排除");
}

#[test]
fn is_md_rejects_non_markdown() {
    assert!(!is_md("divisions.json"));
    assert!(!is_md("LICENSE"));
    assert!(!is_md("tools.json"));
    assert!(!is_md("md")); // 无后缀
    assert!(!is_md("a.md.bak"));
}

#[test]
fn strip_tarball_root_strips_first_segment() {
    assert_eq!(
        strip_tarball_root("agency-agents-main/engineering/x.md"),
        Some("engineering/x.md".to_string())
    );
    assert_eq!(strip_tarball_root("agency-agents-main"), None);
    // 单段顶层文件（tarball 根下 README 等）
    assert_eq!(
        strip_tarball_root("agency-agents-main/README.md"),
        Some("README.md".to_string())
    );
}

#[test]
fn normalize_path_handles_separators() {
    assert_eq!(normalize_path(".\\a\\b.md"), "a/b.md");
    assert_eq!(normalize_path("./a/b.md"), "a/b.md");
    assert_eq!(normalize_path("dir/"), "dir");
    assert_eq!(normalize_path(""), "");
}

// ---------------------------------------------------------------------------
// catalog 组装与校验
// ---------------------------------------------------------------------------

fn sample_entry(tree: Vec<String>) -> resource_filter::ResourceEntry {
    resource_filter::ResourceEntry {
        kind: resource_filter::KIND_PERSONA_CATALOG.to_string(),
        id: resource_filter::CATALOG_ID.to_string(),
        upstream_repo: "msitarzewski/agency-agents".to_string(),
        upstream_commit: "8ae6164602c95ca78828b148b9052460ee426885".to_string(),
        fetched_at_utc: "2026-11-12T12:00:00Z".to_string(),
        file_count: 0, // assemble_catalog 重算
        license: "MIT".to_string(),
        tree,
    }
}

#[test]
fn assemble_catalog_builds_sorted_manifest() {
    let files = vec![
        ("b.md".to_string(), "b content".to_string()),
        ("a.md".to_string(), "a content".to_string()),
    ];
    let json = assemble_catalog(
        files,
        sample_entry(Vec::new()),
        "2026-11-12T12:00:00Z",
        "baseline",
        "bundled snapshot".to_string(),
        "MIT License\n\nCopyright (c) ...".to_string(),
    )
    .expect("组装成功");
    let cat: resource_filter::Catalog = serde_json::from_str(&json).expect("合法 JSON");
    assert_eq!(cat.manifest.abi, 1);
    assert_eq!(cat.manifest.source, "baseline");
    assert_eq!(cat.manifest.resources.len(), 1);
    assert!(
        cat.manifest.license_text.starts_with("MIT License"),
        "许可原文进 manifest（§8 产物含 LICENSE 原文）"
    );
    let e = &cat.manifest.resources[0];
    assert_eq!(e.tree, vec!["a.md".to_string(), "b.md".to_string()]);
    assert_eq!(e.file_count, 2);
    assert_eq!(e.kind, "persona-catalog");
    assert_eq!(cat.files.len(), 2);
}

#[test]
fn assemble_catalog_rejects_empty() {
    let err = assemble_catalog(
        Vec::new(),
        sample_entry(Vec::new()),
        "2026-11-12T12:00:00Z",
        "baseline",
        "x".to_string(),
        "MIT License".to_string(),
    )
    .expect_err("空 catalog 必须拒绝");
    assert!(err.contains("0 个"));
}

#[test]
fn assemble_catalog_rejects_missing_license_text() {
    let err = assemble_catalog(
        vec![("a.md".to_string(), "x".to_string())],
        sample_entry(Vec::new()),
        "2026-11-12T12:00:00Z",
        "baseline",
        "x".to_string(),
        "   ".to_string(),
    )
    .expect_err("许可原文缺失必须拒绝（分发合规硬前提）");
    assert!(err.contains("license_text"));
}

#[test]
fn assemble_catalog_rejects_non_markdown_only() {
    let files = vec![("LICENSE".to_string(), "MIT".to_string())];
    let err = assemble_catalog(
        files,
        sample_entry(Vec::new()),
        "2026-11-12T12:00:00Z",
        "baseline",
        "x".to_string(),
        "MIT License".to_string(),
    )
    .expect_err("非 md 必须拒绝");
    assert!(err.contains("裁剪后"));
}

#[test]
fn validate_entry_rejects_count_mismatch() {
    let mut e = sample_entry(vec!["a.md".into()]);
    e.file_count = 2;
    assert!(validate_entry(&e).is_err());
    e.file_count = 1;
    assert!(validate_entry(&e).is_ok());
    e.upstream_commit = String::new();
    assert!(validate_entry(&e).is_err());
}

// ---------------------------------------------------------------------------
// UTC 格式化（civil_from_days 已知锚点）
// ---------------------------------------------------------------------------

#[test]
fn format_epoch_secs_known_timestamps() {
    assert_eq!(format_epoch_secs(0), "1970-01-01T00:00:00Z");
    // 2026-11-12 00:00:00 UTC = 1794441600（2026-01-01=1767225600 + 315 天）
    assert_eq!(format_epoch_secs(1_794_441_600), "2026-11-12T00:00:00Z");
    // 跨日边界 + 闰年 2024-02-29
    assert_eq!(format_epoch_secs(1_709_164_800), "2024-02-29T00:00:00Z");
    assert_eq!(format_epoch_secs(86_399), "1970-01-01T23:59:59Z");
    assert_eq!(format_epoch_secs(86_400), "1970-01-02T00:00:00Z");
}

#[test]
fn date_part_extracts_yyyymmdd() {
    assert_eq!(date_part("2026-11-12T12:22:00Z"), "2026-11-12");
    assert_eq!(date_part("2026-11-12"), "2026-11-12");
    assert_eq!(date_part(""), "");
}

#[test]
fn out_file_literal_matches_const() {
    // lib.rs include_str! 用字面量路径——常量漂移由本测试钉死。
    assert_eq!(OUT_FILE, "persona_catalog.json");
}

// ---------------------------------------------------------------------------
// 内嵌产物 + ABI 导出面自检（build.rs 已在 cargo test 前跑完）
// ---------------------------------------------------------------------------

#[test]
fn abi_version_is_one() {
    assert_eq!(prs_abi_version(), 1);
}

#[test]
fn embedded_catalog_manifest_consistent() {
    let cat = catalog();
    let manifest: resource_filter::PluginManifest =
        serde_json::from_str(&cat.manifest_json).expect("manifest JSON 合法");
    assert_eq!(manifest.abi, 1);
    assert!(
        matches!(manifest.source.as_str(), "fetched" | "baseline"),
        "manifest.source 值集按设计 §3.2 钉死 fetched|baseline（脚本预下载的区分在 embed_source）"
    );
    assert_eq!(manifest.resources.len(), 1);
    let e = &manifest.resources[0];
    assert_eq!(e.kind, "persona-catalog");
    assert_eq!(e.id, "agency-agents");
    assert_eq!(e.license, "MIT");
    assert!(
        manifest.license_text.starts_with("MIT License"),
        "内嵌产物携带上游 LICENSE 原文（§8）"
    );
    assert_eq!(e.file_count, e.tree.len());
    assert_eq!(e.file_count, cat.files.len(), "manifest tree 与 files 一致");
    // tree 排序去重 + 全为 .md
    let mut sorted = e.tree.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(e.tree, sorted, "tree 已排序去重");
    assert!(e.tree.iter().all(|p| is_md(p)));
    assert!(!e.upstream_commit.is_empty());
}

#[test]
fn prs_manifest_round_trips() {
    let raw = unsafe { CStr::from_ptr(prs_manifest()) }
        .to_string_lossy()
        .into_owned();
    let v: serde_json::Value = serde_json::from_str(&raw).expect("manifest 合法 JSON");
    assert_eq!(v["abi"], 1);
    assert_eq!(v["resources"][0]["kind"], "persona-catalog");
    prs_free(prs_manifest()); // 释放纪律不炸即可（每次分配独立）
}

fn get_raw(kind: &str, id: &str) -> String {
    let k = CString::new(kind).unwrap();
    let i = CString::new(id).unwrap();
    unsafe {
        CStr::from_ptr(prs_get(
            k.as_ptr() as *const c_char,
            i.as_ptr() as *const c_char,
        ))
    }
    .to_string_lossy()
    .into_owned()
}

#[test]
fn prs_get_happy_path_returns_envelope() {
    let manifest: resource_filter::PluginManifest =
        serde_json::from_str(&catalog().manifest_json).unwrap();
    let first = &manifest.resources[0].tree[0];
    let raw = get_raw("persona-catalog", first);
    let v: serde_json::Value = serde_json::from_str(&raw).expect("envelope 合法 JSON");
    assert_eq!(v["ok"], true);
    let content = v["content"].as_str().expect("content 为字符串");
    assert_eq!(
        content,
        catalog().files[first].as_str(),
        "content 与内嵌文件逐字节一致"
    );
}

#[test]
fn prs_get_error_arms() {
    // 未知 kind
    let v: serde_json::Value = serde_json::from_str(&get_raw("nope", "a.md")).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["err_code"], "unknown_kind");
    // kind 对但 id 不存在
    let v: serde_json::Value =
        serde_json::from_str(&get_raw("persona-catalog", "no/such.md")).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["err_code"], "not_found");
    // NULL 参数
    let raw = unsafe { CStr::from_ptr(prs_get(std::ptr::null(), std::ptr::null())) }
        .to_string_lossy()
        .into_owned();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["err_code"], "invalid_arg");
}

#[test]
fn prs_free_reclaims_allocation() {
    let p = prs_manifest();
    assert!(!p.is_null());
    prs_free(p);
    // NULL 容忍（宿主重复 free 防御）
    prs_free(std::ptr::null_mut());
}
