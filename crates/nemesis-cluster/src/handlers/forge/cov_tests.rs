// handlers/forge.rs 覆盖率补充测试（远端反思报告落盘写失败上抛臂）。
//
// 豁免：无。
//
// 【疑似缺陷记录，仅记录不修】receive_reflection 的文件名由对端可控的
// source_node 直接拼入（forge.rs:170 `format!("remote-{}-{}.json", ...)`，
// 未消毒）：含平台非法字符时写盘必然失败（本测试即借此驱动 182 的错误
// 上抛臂）；含 `..` / 路径分隔符时更可写出 remote_dir 之外（路径穿越面）。
// 建议对 source_node 做文件名消毒。
//
// 测试驱动字符用 **NUL**（跨平台非法——std 路径转 CString 必拒，Linux 上
// `< > : | ?` 等只是 Windows 非法、在 Linux 是合法文件名字符，写过一次
// 假红）。

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

/// source_node 含 NUL 字节（跨平台非法文件名字符）→ 拼出的文件名不可
/// 创建 → fs::write 失败上抛（182）。
#[test]
fn receive_reflection_invalid_filename_propagates_error() {
    let provider = FileForgeProvider::new(temp_forge("bad-name"));

    let err = provider
        .receive_reflection(&serde_json::json!({
            "source_node": "co\u{0}v",
            "report": {"findings": ["x"]},
        }))
        .unwrap_err();

    assert!(!err.is_empty(), "写失败必须带错误信息上抛：{err}");
}
