//! A3 写穿侧车（msg_class 单轨 §10-8，2026-11-21 拍板）设施测试。
//!
//! 镜像写（内存态 = 盘上态）/ take-first 单次消费 / 旧格式改判
//! （legacy_class_coerce 三臂 + Drop）/ 会话键扫描器（含 `.taking` 崩溃
//! 残留）/ pending 扫描器排除 A3 族。unique key + 清理（本目录测试
//! 惯例——走进程单例 home，不重定向）。

use super::*;
use crate::inbox::{Inbox, MessagePriority, NoticeEntry, PersistedNotice};
use nemesis_types::channel::{InboundMessage, InstructionKind, MsgClass, NoticeKind};

fn unique_key(tag: &str) -> String {
    format!(
        "test:a3sidecar:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn cleanup(key: &str) {
    delete_chat_log(key);
    let _ = std::fs::remove_file(instruction_queue_path(key));
    let _ = std::fs::remove_file(notices_stash_path(key));
    let mut t = instruction_queue_path(key).into_os_string();
    t.push(".taking");
    let _ = std::fs::remove_file(std::path::PathBuf::from(t));
    let mut t2 = notices_stash_path(key).into_os_string();
    t2.push(".taking");
    let _ = std::fs::remove_file(std::path::PathBuf::from(t2));
}

fn instruction_msg(key: &str, content: &str) -> InboundMessage {
    InboundMessage::instruction(
        InstructionKind::GoalRound,
        "system",
        "",
        key,
        content.to_string(),
    )
}

/// 指令队列镜像往返：enqueue（busy 闸同路径）→ `.iqueue` 落盘（行 =
/// InboundMessage，class 保真）→ read 回读同 class → claim 后镜像删除。
#[test]
fn a3_iqueue_mirror_roundtrip_and_claim_clears() {
    let inbox = Inbox::new(8);
    let key = unique_key("iqueue-rt");

    // 镜像写：Instruction 类条目入队 → 侧车立即出现。
    let out = inbox.enqueue_for_mode(
        &key,
        crate::inbox::QueuedMessage {
            msg: instruction_msg(&key, "goal round"),
            timestamp: String::new(),
            priority: MessagePriority::Normal,
        },
        false,
    );
    assert_eq!(out, crate::inbox::EnqueueOutcome::QueuedForNextTurn);
    let sidecar = instruction_queue_path(&key);
    assert!(sidecar.exists(), "enqueue 后 `.iqueue` 必须落盘");
    let read_back = read_instruction_queue(&key);
    assert_eq!(read_back.len(), 1);
    assert_eq!(read_back[0].content, "goal round");
    assert_eq!(
        read_back[0].msg_class,
        MsgClass::Instruction(InstructionKind::GoalRound),
        "镜像行 class 保真（serde 往返不丢）"
    );
    assert_eq!(read_back[0].session_key, key);

    // User 类排队条目不落盘（用户消息语义有回执留痕，不属于 A3 三持有区）。
    let user_key = unique_key("iqueue-user");
    inbox.enqueue_for_mode(
        &user_key,
        crate::inbox::QueuedMessage {
            msg: InboundMessage::user_ingress(
                "web".to_string(),
                "u".to_string(),
                "c".to_string(),
                "hello".to_string(),
            )
            .with_session_key(user_key.clone()),
            timestamp: String::new(),
            priority: MessagePriority::Normal,
        },
        false,
    );
    assert!(
        !instruction_queue_path(&user_key).exists(),
        "User 类条目不得进 `.iqueue`"
    );

    // claim（轮边界同路径）→ 队空 = 镜像删除。
    let claimed = inbox.claim_next_turn_head(&key);
    assert!(claimed.is_some());
    assert!(
        !sidecar.exists(),
        "claim 清空指令队列后侧车必须删除（内存空 = 盘上空）"
    );

    cleanup(&key);
    cleanup(&user_key);
}

/// 通告 stash 镜像往返：stash → `.notices` 落盘（PersistedNotice 包络行，
/// session_key 在行内）→ drain 后镜像删除；kind 随行。
#[test]
fn a3_notices_mirror_roundtrip_and_drain_clears() {
    let inbox = Inbox::new(8);
    let key = unique_key("notices-rt");

    inbox.stash_notice(
        &key,
        NoticeEntry {
            kind: NoticeKind::Loop,
            channel: "web".to_string(),
            chat_id: "web:1".to_string(),
            content: "job done".to_string(),
        },
    );
    let sidecar = notices_stash_path(&key);
    assert!(sidecar.exists(), "stash 后 `.notices` 必须落盘");
    let entries = read_notice_stash(&key);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry.content, "job done");
    assert_eq!(
        entries[0].entry.kind,
        NoticeKind::Loop,
        "kind 随行（A2 徽标保真）"
    );
    assert_eq!(entries[0].session_key, key, "包络行必须烧入会话键");

    // drain（轮边界合并同路径）→ 侧车删除。
    assert!(inbox.drain_notices_merged(&key).is_some());
    assert!(!sidecar.exists(), "drain 清空 stash 后侧车必须删除");
    assert!(
        inbox.drain_notices_merged(&key).is_none(),
        "重复 drain 幂等"
    );

    // 包络行形态核验（take-first 读面——sweeper hydrate 消费的同一路径）：
    // session_key 在行内（扫描器按首行还原真实会话键的字节基础）。
    inbox.stash_notice(
        &key,
        NoticeEntry {
            kind: NoticeKind::Loop,
            channel: "web".to_string(),
            chat_id: "web:1".to_string(),
            content: "job done 2".to_string(),
        },
    );
    let rows = take_notice_stash(&key);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_key, key, "包络行必须烧入会话键");
    assert_eq!(rows[0].entry.content, "job done 2");
    assert!(!sidecar.exists(), "take 单次消费");

    cleanup(&key);
}

/// clear 级联：两族侧车一并删除（镜像语义：持有区清空 = 盘上空——不删
/// 会在重启 hydrate 出幽灵重放）。
#[test]
fn a3_clear_deletes_both_sidecars() {
    let inbox = Inbox::new(8);
    let key = unique_key("clear-both");
    inbox.enqueue_for_mode(
        &key,
        crate::inbox::QueuedMessage {
            msg: instruction_msg(&key, "tick"),
            timestamp: String::new(),
            priority: MessagePriority::Normal,
        },
        false,
    );
    inbox.stash_notice(
        &key,
        NoticeEntry {
            kind: NoticeKind::Events,
            channel: "web".to_string(),
            chat_id: "web:1".to_string(),
            content: "n".to_string(),
        },
    );
    assert!(instruction_queue_path(&key).exists());
    assert!(notices_stash_path(&key).exists());

    inbox.clear(&key);
    assert!(
        !instruction_queue_path(&key).exists(),
        "clear 必须删 `.iqueue`"
    );
    assert!(
        !notices_stash_path(&key).exists(),
        "clear 必须删 `.notices`"
    );

    cleanup(&key);
}

/// 旧格式改判（§10-7）：无 class 旧行按章改判（instruction_kind →
/// Instruction / cron_job_id → CronTick / system_notice → Notice），
/// 非法 slug 诚实 Drop；新格式行（class 在）Keep 直通。
#[test]
fn a3_iqueue_legacy_class_coerce_on_take() {
    let key = unique_key("coerce");

    // 手写旧格式行（serde 直写侧车——模拟退役生产者残留）。
    let mut old_instruction = InboundMessage::user_ingress(
        "system".to_string(),
        "system".to_string(),
        String::new(),
        "wal resume".to_string(),
    )
    .with_session_key(key.clone());
    old_instruction
        .metadata
        .insert("instruction_kind".to_string(), "wal_resume".to_string());
    // 抹掉 msg_class 的旧 wire 形态：重新序列化后删 "msg_class" 键。
    let mut v = serde_json::to_value(&old_instruction).unwrap();
    v.as_object_mut().unwrap().remove("msg_class");

    let mut old_cron = InboundMessage::user_ingress(
        "system".to_string(),
        "system".to_string(),
        String::new(),
        "tick".to_string(),
    )
    .with_session_key(key.clone());
    old_cron
        .metadata
        .insert("cron_job_id".to_string(), "job-9".to_string());
    let mut cv = serde_json::to_value(&old_cron).unwrap();
    cv.as_object_mut().unwrap().remove("msg_class");

    let mut old_notice = InboundMessage::user_ingress(
        "web".to_string(),
        "system".to_string(),
        "web:2".to_string(),
        "bg done".to_string(),
    )
    .with_session_key(key.clone());
    old_notice
        .metadata
        .insert("system_notice".to_string(), "events".to_string());
    let mut nv = serde_json::to_value(&old_notice).unwrap();
    nv.as_object_mut().unwrap().remove("msg_class");

    // 非法 slug（退役生产者残留，如 background）→ Drop 诚实丢弃。
    let mut dead_notice = old_notice.clone();
    *dead_notice.metadata.get_mut("system_notice").unwrap() = "background".to_string();
    let mut dv = serde_json::to_value(&dead_notice).unwrap();
    dv.as_object_mut().unwrap().remove("msg_class");

    // 新格式行（class 烤死在行内）→ Keep 直通不改判。
    let fresh = instruction_msg(&key, "modern row");

    // 逐行写入侧车（绕过 Inbox——直接构造盘上形态）。
    let lines = vec![&v, &cv, &nv, &dv, &serde_json::to_value(&fresh).unwrap()]
        .into_iter()
        .map(|j| serde_json::to_string(&j).unwrap())
        .collect::<Vec<_>>();
    mirror_write(&instruction_queue_path(&key), &lines);

    let taken = take_instruction_queue(&key);
    // dv（非法 slug）被 Drop；其余四条按章改判/保真。
    assert_eq!(taken.len(), 4, "非法 slug 行必须诚实丢弃");
    assert_eq!(
        taken[0].msg_class,
        MsgClass::Instruction(InstructionKind::WalResume),
        "instruction_kind 章改判"
    );
    assert_eq!(
        taken[1].msg_class,
        MsgClass::Instruction(InstructionKind::CronTick),
        "cron_job_id 章改判"
    );
    assert_eq!(
        taken[2].msg_class,
        MsgClass::Notice(NoticeKind::Events),
        "system_notice 章改判"
    );
    assert_eq!(
        taken[3].msg_class,
        MsgClass::Instruction(InstructionKind::GoalRound),
        "新格式行 class 直通"
    );
    assert!(!instruction_queue_path(&key).exists(), "take 单次消费");

    cleanup(&key);
}

/// 会话键扫描器：`.iqueue`/`.notices`/`.taking` 崩溃残留三形态都能按首行
/// 还原真实会话键；裸 `.taking`（pending 族）不归 A3 扫描器认领。
#[test]
fn a3_sidecar_sessions_scanner() {
    let key = unique_key("scan");
    // 活镜像形态。
    restore_instruction_queue(&key, &[instruction_msg(&key, "s1")]);
    let found = instruction_queue_sessions();
    assert!(
        found.iter().any(|k| k == &key),
        "instruction_queue_sessions 必须发现活镜像: {found:?}"
    );

    // `.taking` 崩溃残留形态（rename 后删除前终止的窗口）。
    let mut tmp = instruction_queue_path(&key).into_os_string();
    tmp.push(".taking");
    std::fs::write(
        std::path::PathBuf::from(&tmp),
        serde_json::to_string(&instruction_msg(&key, "residue")).unwrap(),
    )
    .unwrap();
    let found = instruction_queue_sessions();
    assert!(
        found.iter().any(|k| k == &key),
        "崩溃残留必须被发现: {found:?}"
    );

    // notices 族同型（PersistedNotice 包络行）。
    let persisted = PersistedNotice {
        session_key: key.clone(),
        entry: NoticeEntry {
            kind: NoticeKind::Merged,
            channel: "web".to_string(),
            chat_id: "c".to_string(),
            content: "n".to_string(),
        },
    };
    restore_notice_stash(&key, &[persisted]);
    let found = notice_stash_sessions();
    assert!(
        found.iter().any(|k| k == &key),
        "notices 扫描必须发现: {found:?}"
    );

    // pending 扫描器不得认领 A3 族 `.taking`（跨族 tmp 撞名防护的读侧
    // 对应面——漏了会把指令残留当 pending 重放，双重投递）。
    let pending_found = pending_sessions();
    assert!(
        !pending_found.iter().any(|k| k == &key),
        "pending_sessions 不得认领 `.iqueue.taking`/`.notices.taking`: {pending_found:?}"
    );

    cleanup(&key);
}

/// 通告 stash 侧车 → 构造器重建 Notice 条目（sweeper hydrate 路径的
/// 字节级预演）：PersistedNotice 行经 take 后逐条转 notice 构造器，
/// class/kind/路由四面保真。
#[test]
fn a3_notice_stash_hydrate_reconstruction() {
    let key = unique_key("hydrate");
    let persisted = PersistedNotice {
        session_key: key.clone(),
        entry: NoticeEntry {
            kind: NoticeKind::ApprovalContinuation,
            channel: "rpc".to_string(),
            chat_id: "rpc:9".to_string(),
            content: "approved".to_string(),
        },
    };
    restore_notice_stash(&key, &[persisted]);

    let rows = take_notice_stash(&key);
    assert_eq!(rows.len(), 1);
    let m = InboundMessage::notice(
        rows[0].entry.kind.clone(),
        rows[0].entry.channel.clone(),
        rows[0].entry.chat_id.clone(),
        rows[0].session_key.clone(),
        rows[0].entry.content.clone(),
    );
    assert_eq!(
        m.msg_class,
        MsgClass::Notice(NoticeKind::ApprovalContinuation)
    );
    assert_eq!(m.channel, "rpc");
    assert_eq!(m.chat_id, "rpc:9");
    assert_eq!(m.session_key, key);
    assert_eq!(m.sender_id, "system", "构造器烤死 sender");
    assert!(!notices_stash_path(&key).exists(), "take 单次消费");

    cleanup(&key);
}
