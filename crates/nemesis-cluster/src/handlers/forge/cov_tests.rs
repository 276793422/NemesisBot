// handlers/forge.rs 覆盖率补充测试（远端反思报告落盘）。
//
// 豁免：无。
//
// 【T10 已修（2026-10-17 三轮审计）】receive_reflection 的文件名由对端可控
// 的 source_node 拼入，现已经 sanitize_path_segment 消毒（路径字符 1:1 映射
// `_`）。本文件保留消毒后合法落盘线：跨平台非法字符 NUL 亦被映射为 `_`，
// 写盘不再失败上抛；「不得逃出 remote_dir」由 tests.rs 的
// hostile_source_node_stays_in_remote_dir 钉住。
//
// 测试驱动字符用 **NUL**（跨平台非法——修复前 std 路径转 CString 必拒，
// 写盘必然失败，借此驱动 182 的错误上抛臂；Linux 上 `< > : | ?` 等只是
// Windows 非法、在 Linux 是合法文件名字符，写过一次假红）。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_forge(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nmb-forge-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// source_node 含跨平台非法文件名字符（NUL）→ 消毒映射 `_` 后正常落盘，
/// 不再走写失败上抛臂（T10 修复前此处 unwrap_err）。
#[test]
fn receive_reflection_invalid_filename_sanitized_and_stored() {
    let base = temp_forge("bad-name");
    let provider = FileForgeProvider::new(&base);

    provider
        .receive_reflection(&serde_json::json!({
            "source_node": "co\u{0}v",
            "report": {"findings": ["x"]},
        }))
        .expect("消毒后 NUL 映射 `_`，写盘不应失败");

    let list = provider.get_reflections_list_payload();
    let names: Vec<&str> = list["reflections"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["filename"].as_str())
        .collect();
    assert_eq!(names.len(), 1, "实际: {names:?}");
    assert!(
        names[0].starts_with("remote-co_v-") && names[0].ends_with(".json"),
        "文件名 = 消毒段（NUL → _）: {names:?}"
    );
}
