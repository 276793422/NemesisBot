// cluster_config.rs 覆盖率补充测试（peers 表缺失早退 / 字面键+legacy
// sanitize 键双清 / load_peer_udp_endpoints 坏文件空回退）。
//
// 豁免：
// - 「root 非表」三臂（404-408 / 500-504 / 590-594）：TOML 文档解析
//   恒产出表根（`[[x]]` 也只是根表里的数组键），doc.as_table_mut()
//   不会失败——死防御，无法从公共 API 触发。
// - to_string_pretty 错误臂（462-466 / 528-532 / 620-624）：对
//   Value::Table 内容无法触发。
// - `}` 形态行（208/279/385/576）：if-let 面伪行，不追。

use super::*;

fn tmp(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{}.toml", tag));
    (dir, path)
}

/// 删除时字面键与 legacy sanitize 键并存 → 双清（一次调用不留残键）。
#[test]
fn load_peer_udp_endpoints_corrupt_file_returns_empty() {
    let (_dir, path) = tmp("udp-endpoints-corrupt");
    std::fs::write(&path, "not [valid toml").unwrap();
    assert!(load_peer_udp_endpoints(&path).is_empty());
}

/// 删除时字面键与 legacy sanitize 键并存 → 双清（一次调用不留残键）。
#[test]
fn remove_peer_cleans_literal_and_legacy_keys_together() {
    let (_dir, path) = tmp("dual-key");
    std::fs::write(
        &path,
        "[node]\nid = \"local\"\n\n[peers.\"node.a\"]\naddress = \"10.0.0.1:11950\"\n\n[peers.node_a]\naddress = \"10.0.0.1:11950\"\n",
    )
    .unwrap();

    remove_peer_from_file(&path, "node.a").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains("node.a"), "{content}");
    assert!(!content.contains("node_a"), "{content}");
    assert!(content.contains("[node]"), "[node] 必须保留：{content}");

    // 双键都不存在（再删一次）→ 幂等 Ok。
    remove_peer_from_file(&path, "node.a").unwrap();
}
