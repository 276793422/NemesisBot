//! BUG-20261123-015（R2480 jsonl/行帧面）：撕裂尾安全读回归锁。
//!
//! 断电/崩溃下 append 写可留下撕裂尾行；尾行劈中多字节 UTF-8 字符时整个
//! 文件非 UTF-8——旧 `read_to_string` 读法直接 `Err(InvalidData)`，坏一行
//! 毒化整个文件（历史/标题输入/pending 计数/侧车全部空）。修复 =
//! `nemesis_utils::file::read_lossy_to_string`（U+FFFD 降级，坏尾行走既有
//! 逐行 parse 跳过分支，前缀行全部保全）。本组测试用真实撕裂字节
//! （多字节字符截半）钉三条主路径 + utils helper 两则。

use super::*;

/// 生成撕裂尾字节：把「好」(E5 A5 BD) 截半只留首字节——追加写断电残留的
/// 真实形态（无尾随换行）。
fn append_torn_multibyte_tail(path: &std::path::Path) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append open");
    f.write_all(&[0xE5]).expect("torn byte write");
}

fn uniq(tag: &str) -> String {
    format!(
        "test:torntail:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// 主 log：撕裂尾不毒化 first_user_message（标题输入）与 read_chat_log
///（历史读）；撕裂尾行本身不进行集。
#[test]
fn torn_tail_main_log_preserves_prefix() {
    let k = uniq("main");
    append_chat_log(&k, "user", "第一条用户消息");
    append_chat_log(&k, "assistant", "回复内容");
    append_torn_multibyte_tail(&log_path(&k));

    // 修复前：read_to_string 整文件 Err → None（标题输入整文件丢失）。
    assert_eq!(
        first_user_message(&k, 200).as_deref(),
        Some("第一条用户消息"),
        "撕裂尾不得毒化首条 user 消息"
    );

    let (rows, _total, _more, _oldest) = read_chat_log(&k, usize::MAX, None);
    assert_eq!(rows.len(), 2, "前缀两行必须保全（撕裂尾行跳过）");
}

/// pending jsonl：撕裂尾不毒化计数（修复前返回 0）。
#[test]
fn torn_tail_pending_count_preserves_prefix() {
    let k = uniq("pending");
    let m = |c: &str| {
        nemesis_types::channel::InboundMessage::user_ingress(
            "web".to_string(),
            "ttuser".to_string(),
            "ttchat".to_string(),
            c.to_string(),
        )
    };
    append_session_pending(&k, &m("待投一")).expect("append");
    append_session_pending(&k, &m("待投二")).expect("append");
    append_torn_multibyte_tail(&pending_path(&k));

    assert_eq!(
        session_pending_count(&k),
        2,
        "撕裂尾不得毒化 pending 计数（修复前 read_to_string Err → 0）"
    );
}

/// 侧车（instruction_queue 经 mirror_read）：撕裂尾行 parse 失败跳过，
/// 前缀行全部保全。
#[test]
fn torn_tail_mirror_read_preserves_prefix() {
    let k = uniq("mirror");
    let path = instruction_queue_path(&k);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    let line = |c: &str| {
        serde_json::to_string(&nemesis_types::channel::InboundMessage::user_ingress(
            "web".to_string(),
            "ttuser".to_string(),
            "ttchat".to_string(),
            c.to_string(),
        ))
        .expect("serialize")
    };
    std::fs::write(&path, format!("{}\n{}\n", line("指令一"), line("指令二")))
        .expect("sidecar write");
    append_torn_multibyte_tail(&path);

    let rows = read_instruction_queue(&k);
    assert_eq!(rows.len(), 2, "撕裂尾不得毒化侧车前缀行");
}
