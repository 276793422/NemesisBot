//! BUG-20261029-155 回归锁（绿锁）：VerQueryValueW 的 pulLen 契约是**字节**
//! 数——`/2` 修正后版本串抽取必须完好（真系统二进制对质）。
//!
//! 诚实边界：旧代码把字节长当 u16 元素数（from_raw_parts 读 2× 字节数，
//! 堆越界读 UB），但平时被 NUL 提前截断掩盖——UB 无确定性红证。本锁钉住
//! 修正不回归：真二进制抽取面完好 = 契约读法未被再次破坏。
#![cfg(windows)]

use nemesis_ziwei::verify::extract_version_resource;

#[test]
fn version_resource_extraction_intact_after_len_fix() {
    let p = std::path::Path::new(r"C:\Windows\System32\cmd.exe");
    if std::fs::metadata(p).is_err() {
        eprintln!("SKIP: {p:?} 不存在");
        return;
    }
    let m = extract_version_resource(p);
    assert!(
        m.contains_key("ProductName") || m.contains_key("FileVersion"),
        "cmd.exe 版本串抽取应完好，实得键：{:?}",
        m.keys().collect::<Vec<_>>()
    );
}
