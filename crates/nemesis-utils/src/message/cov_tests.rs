// message.rs 覆盖率补充测试（无自然分割点的硬截断回退 123 与主推送
// 127）。
//
// 豁免：53-55（effective_limit 二次钳制）为死臂——code_block_buffer 在
// 41-43 已被钳到 max_len/2，effective_limit 恒 ≥ max_len/2。

use super::*;

/// 一整段无换行无空格的长文 → 找不到自然分割点 → 硬截断回退（123）
/// 逐块推送（127），块长不越界且拼接可复原。
#[test]
fn split_message_hard_truncates_unbroken_text() {
    // ASCII（多字节 UTF-8 硬截断会触发 char boundary panic——见交付报告
    // 疑似 bug #3，此处只按字节安全输入取覆盖）。
    let content = "a".repeat(250);
    let chunks = split_message(&content, 100);
    assert!(chunks.len() >= 3, "{:?}", chunks.len());
    for c in &chunks {
        assert!(!c.is_empty());
        assert!(c.len() <= 100, "chunk 越界: {}", c.len());
    }
    assert_eq!(chunks.concat().len(), 250);
}
