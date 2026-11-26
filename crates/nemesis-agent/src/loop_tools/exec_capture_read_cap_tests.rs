//! ShellOutputCapture 受限文件读回读侧内存闸测试（BUG-20261126-102 回归锁）：
//! 读回对齐 piped 形态的家族排水帽（`PIPE_CAPTURE_CAP_BYTES` 丢头保尾 +
//! `pipe_cap::truncated_head_marker` 单一真相源标记）——超帽捕获文件 seek
//! 尾读（读窗口 = 帽值），`yes` 类高吞吐输出在 exec 超时窗口写出的 GB 级
//! 捕获文件不再整读进 executor 子进程内存；帽内文件原语义不变（字节一致、
//! 无标记）。测试经 `create_restricted` 直连构造（免 env 全局态污染）。

use super::exec::{ShellOutputCapture, read_capture_file_capped};
use nemesis_utils::pipe_cap::{PIPE_CAPTURE_CAP_BYTES, truncated_head_marker};
use tempfile::TempDir;

/// 帽内文件：字节一致原样读回，无截断标记（原语义不变臂）。
#[test]
fn read_capture_under_cap_exact_no_marker() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("small.bin");
    let payload: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&p, &payload).unwrap();

    let got = read_capture_file_capped(&p).unwrap();
    assert_eq!(got, payload, "帽内文件必须字节一致原样读回");
}

/// 超帽文件：丢头保尾——标记的 dropped 数 = 超帽量，保尾段逐字节等于
/// 原文件末尾帽值字节，返回体量 = 标记 + 帽值。
#[test]
fn read_capture_over_cap_keeps_tail_with_marker() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("big.bin");
    let cap = PIPE_CAPTURE_CAP_BYTES;
    let total = cap + 4096;
    let payload: Vec<u8> = (0..total as u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&p, &payload).unwrap();

    let got = read_capture_file_capped(&p).unwrap();
    let marker = truncated_head_marker(4096);
    assert!(got.starts_with(marker.as_bytes()), "超帽必须带丢头保尾标记");
    assert_eq!(
        &got[marker.len()..],
        &payload[4096..],
        "保尾段必须逐字节等于原文件末尾帽值字节"
    );
    assert_eq!(got.len(), marker.len() + cap, "返回体量必须 = 标记 + 帽值");
}

/// read_back 双流接线：out/err 各自过闸（都超帽 → 都带标记），读回后
/// 捕获文件照常删除（清理语义不因闸改动回归）。
#[test]
fn read_back_caps_both_streams_and_cleans_up() {
    let tmp = TempDir::new().unwrap();
    let mut capture = ShellOutputCapture::create_restricted(tmp.path()).unwrap();
    let cap = PIPE_CAPTURE_CAP_BYTES;
    // payload 用 ASCII（read_back 返回 String，经 from_utf8_lossy——非 ASCII
    // 字节会被替换成 EF BF BD 破坏字节级断言；本测试锁的是读闸接线，非
    // 编码语义，ASCII 使 lossy 转换为恒等）。
    let payload: Vec<u8> = (0..(cap + 2048) as u32)
        .map(|i| (48 + i % 10) as u8)
        .collect();
    // 持有句柄先放掉（read_back 只按路径读；生产形态句柄已移交 stdio）。
    capture.out_file.take();
    capture.err_file.take();
    std::fs::write(&capture.out_path, &payload).unwrap();
    std::fs::write(&capture.err_path, &payload).unwrap();
    let out_path = capture.out_path.clone();
    let err_path = capture.err_path.clone();

    let (stdout, stderr) = capture.read_back();
    let marker = truncated_head_marker(2048);
    assert!(
        stdout.starts_with(marker.as_str()),
        "stdout 读回必须过闸带标记"
    );
    assert!(
        stderr.starts_with(marker.as_str()),
        "stderr 读回必须过闸带标记"
    );
    assert_eq!(
        &stdout.as_bytes()[marker.len()..],
        &payload[2048..],
        "保尾段逐字节等于原文件末尾帽值字节"
    );
    assert!(!out_path.exists(), "读回后捕获文件必须删除");
    assert!(!err_path.exists(), "读回后捕获文件必须删除");
}
