//! 会话回收站（2026-10-30）测试：move 三件套+清单回读 / 稀疏会话 /
//! restore 正常+冲突结构化返回+web 键改名恢复+复合键诚实拒绝 / purge /
//! purge_all / 同键二次删除双条目并存 / 孤儿目录列表与恢复语义。
//!
//! 隔离纪律：核心函数全部走 [`TrashLocations::with_dirs`] 注入——**每个测试
//! 一套 tempdir 三目录**（trash/session_logs/boundary），不触碰进程单例
//! home（OnceLock 固化不可重定向；真实 home 是在用数据目录，写它=污染）。
//! 夹具直接按 loc 寻址写盘（文件行格式自洽即可，trash 逻辑只数非空行），
//! 不经 chat_log::append_chat_log 单例。故无需串行锁。

use super::*;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// F3 回归锁（2026-11-02 复评，probe 转正）：恢复中途失败（boundary 被
/// share-mode 锁卡死）后——①改道改名恢复 = 材料分裂，诚实拒绝而非假成功
/// 会话分裂；②解除锁后同目标重试 = 按事务标记续作完成，而非 conflict:true
/// 永久卡死。Windows 形态（share_mode 锁 + rename 目标存在即败）。
#[cfg(windows)]
#[test]
fn review_nov02_partial_restore_resumes_and_rename_switch_refused() {
    use std::os::windows::fs::OpenOptionsExt;
    let t = TempTrash::new("nov02-partial-restore");
    let key = web_key("partial");
    put_msg(&t, &key, "user", "keep this history");
    put_boundary(&t, &key, "turn_end");
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();
    let boundary = t.entry_dir(&entry).join("boundary.jsonl");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&boundary)
        .unwrap();

    // 首次恢复：chat 移成功、boundary rename 重试耗尽 → 诚实 Err，条目保留。
    let first = trash_restore_in(&t.loc, &entry, None).unwrap_err();
    assert!(t.loc.log_path(&key).exists(), "chat 已半途移到目标");
    assert!(
        !t.entry_dir(&entry).join("chat.jsonl").exists(),
        "条目内 chat 已走，first={first}"
    );
    assert!(
        t.entry_dir(&entry).join(RESTORE_MARKER_FILE).is_file(),
        "事务标记随失败保留"
    );

    // 回归①（旧行为：假成功，chat 留原键、boundary/meta 去新键 = 会话分裂）。
    let elsewhere = format!(
        "{WEB_SESSION_KEY_PREFIX}{}",
        SessionStore::sanitize_session_id("nov02-elsewhere")
    );
    let err = trash_restore_in(&t.loc, &entry, Some("nov02-elsewhere")).unwrap_err();
    assert!(err.contains("分裂"), "拒绝文案须点名分裂: {err}");
    assert!(
        !t.loc.boundary_path(&elsewhere).exists() && !t.loc.log_path(&elsewhere).exists(),
        "拒绝路径不移动任何材料"
    );
    assert!(t.loc.log_path(&key).exists(), "chat 留在原键");

    // 回归②（旧行为：重试 conflict:true 被自己的半成品永久卡死）。
    drop(lock);
    let retry = trash_restore_in(&t.loc, &entry, None).expect("同目标重试须续作完成");
    assert_eq!(retry["restored"], serde_json::json!(key));
    assert_eq!(read_msgs(&t, &key).len(), 1, "消息完整");
    assert!(t.loc.boundary_path(&key).exists(), "boundary 补齐");
    assert!(
        !t.entry_dir(&entry).exists(),
        "成功后条目整目录清掉（标记随之消失）"
    );
}

/// F3 回归锁（便携形态，不依赖 Windows 锁）：手工构造半成品状态（chat 已到
/// 目标键 + 事务标记在条目内），恢复须续作完成而非报 conflict。
#[test]
fn review_nov02_trash_marker_resume_completes_instead_of_conflict() {
    let t = TempTrash::new("nov02-resume");
    let key = make_session(&t, "resume", "半途恢复", &["half moved"]);
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();
    std::fs::rename(t.entry_dir(&entry).join("chat.jsonl"), t.loc.log_path(&key)).unwrap();
    std::fs::write(
        t.entry_dir(&entry).join(RESTORE_MARKER_FILE),
        serde_json::json!({ "target_key": key }).to_string(),
    )
    .unwrap();

    let out = trash_restore_in(&t.loc, &entry, None).expect("续作须完成而非 conflict");
    assert_eq!(out["restored"], serde_json::json!(key));
    assert_eq!(read_msgs(&t, &key).len(), 1);
    assert!(
        std::fs::read_to_string(t.loc.meta_path(&key))
            .unwrap()
            .contains("半途恢复"),
        "meta 补齐回原键"
    );
    assert!(!t.entry_dir(&entry).exists(), "条目清掉");
}

/// F3 回归锁（便携）：材料已分裂（chat 在标记目标、其余在条目）时换键恢复
/// 诚实拒绝，且拒绝不锁死条目——同目标重试仍可完成。
#[test]
fn review_nov02_trash_split_materials_refuse_target_switch() {
    let t = TempTrash::new("nov02-split");
    let key = make_session(&t, "split", "分裂前", &["s1"]);
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();
    std::fs::rename(t.entry_dir(&entry).join("chat.jsonl"), t.loc.log_path(&key)).unwrap();
    std::fs::write(
        t.entry_dir(&entry).join(RESTORE_MARKER_FILE),
        serde_json::json!({ "target_key": key }).to_string(),
    )
    .unwrap();

    let err = trash_restore_in(&t.loc, &entry, Some("nov02-elsewhere")).expect_err("换键须拒绝");
    assert!(err.contains("分裂"), "{err}");
    let elsewhere = format!(
        "{WEB_SESSION_KEY_PREFIX}{}",
        SessionStore::sanitize_session_id("nov02-elsewhere")
    );
    assert!(
        !t.loc.log_path(&elsewhere).exists() && !t.loc.meta_path(&elsewhere).exists(),
        "拒绝路径不移动任何材料"
    );
    assert!(t.entry_dir(&entry).join("meta.json").is_file(), "条目保留");

    // 拒绝 ≠ 锁死：同目标重试照常续作完成。
    let out = trash_restore_in(&t.loc, &entry, None).unwrap();
    assert_eq!(out["restored"], serde_json::json!(key));
    assert!(!t.entry_dir(&entry).exists());
}

/// F3 区分锁（2026-11-02 复评）：meta-only（真实空会话）改名恢复 = 全部材料
/// 一致地到新键、原键清空——与「半途分裂」是两回事，不得误伤。
#[test]
fn review_nov02_meta_only_rename_restore_is_consistent_not_split() {
    let t = TempTrash::new("nov02-metaonly");
    let key = web_key("metaonly");
    put_meta(&t, &key, "只有 meta");
    assert!(!t.loc.log_path(&key).exists(), "前置：无 chat");
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();

    let out = trash_restore_in(&t.loc, &entry, Some("nov02-metaonly-new")).unwrap();
    let new_key = out["restored"].as_str().unwrap();
    assert!(
        new_key.ends_with("nov02-metaonly-new"),
        "改名恢复 {new_key}"
    );
    assert_eq!(out["title"], serde_json::json!("只有 meta"), "投影照常");
    assert!(t.loc.meta_path(new_key).is_file(), "meta 到新键");
    assert!(!t.loc.log_path(new_key).exists(), "无 chat 不造假");
    assert!(!t.loc.meta_path(&key).exists(), "原键清空");
    assert!(!t.entry_dir(&entry).exists(), "条目清掉");
}

/// web 形态测试键（真实前缀 + 无 `_` 的 sid 段：反推无损、display 剥前缀、
/// 改名闸全走真语义）。hex 段让跨 run 残留 tempdir 也不撞名。
fn web_key(tag: &str) -> String {
    format!("{WEB_SESSION_KEY_PREFIX}nbtrash{}-{}", run_hex(), tag)
}

fn composite_key(tag: &str) -> String {
    format!("nbnode{}/chat-{}", run_hex(), tag)
}

fn run_hex() -> String {
    static RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    })
    .clone()
}

// ---------------------------------------------------------------------------
// 注入夹具
// ---------------------------------------------------------------------------

/// 每测试一套 tempdir 三目录 + Drop 清理守卫（panic 也清，清不掉留 OS temp
/// 不碍事）。
struct TempTrash {
    root: PathBuf,
    loc: TrashLocations,
}

impl TempTrash {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "nb_trash_t_{}_{:x}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            tag
        ));
        let logs = root.join("session_logs");
        let boundary = root.join("boundary");
        let trash = root.join("trash");
        let sessions = root.join("sessions"); // todo 文件挂这里（真相源同构）
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(&boundary).unwrap();
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        let loc = TrashLocations::with_dirs(trash, logs, boundary, sessions);
        Self { root, loc }
    }

    fn entry_dir(&self, entry: &str) -> PathBuf {
        self.loc.trash_root().join(entry)
    }
}

impl Drop for TempTrash {
    fn drop(&mut self) {
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(&self.root);
    }
}

/// 追加一条 chat.jsonl 行（自洽格式；trash 只数非空行、恢复测试按本格式回读）。
fn put_msg(t: &TempTrash, key: &str, role: &str, content: &str) {
    let p = t.loc.log_path(key);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .unwrap();
    writeln!(
        f,
        "{}",
        serde_json::json!({"role": role, "content": content})
    )
    .unwrap();
}

/// 写 meta sidecar（SessionMeta 全字段 default，最小 JSON 可解析）。
fn put_meta(t: &TempTrash, key: &str, title: &str) {
    let p = t.loc.meta_path(key);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, serde_json::json!({ "title": title }).to_string()).unwrap();
}

/// 追加一条 boundary 行。
fn put_boundary(t: &TempTrash, key: &str, kind: &str) {
    let p = t.loc.boundary_path(key);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .unwrap();
    writeln!(f, "{}", serde_json::json!({"kind": kind})).unwrap();
}

/// 写 todo 文件（TodoWriteTool 落盘格式：TodoItem 数组）。
fn put_todo(t: &TempTrash, key: &str, items: &[serde_json::Value]) {
    let p = t.loc.todo_path(key);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, serde_json::to_string(&items).unwrap()).unwrap();
}

/// 回读恢复后的消息（与 put_msg 格式配对）。
fn read_msgs(t: &TempTrash, key: &str) -> Vec<(String, String)> {
    let raw = std::fs::read_to_string(t.loc.log_path(key)).unwrap();
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            (
                v["role"].as_str().unwrap_or_default().to_string(),
                v["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// 造一个带消息 + meta 的 web 会话。
fn make_session(t: &TempTrash, tag: &str, title: &str, msgs: &[&str]) -> String {
    let key = web_key(tag);
    for m in msgs {
        put_msg(t, &key, "user", m);
    }
    put_meta(t, &key, title);
    key
}

// ---------------------------------------------------------------------------
// 移入 + 清单
// ---------------------------------------------------------------------------

#[test]
fn trash_moves_three_files_and_writes_manifest() {
    let t = TempTrash::new("full");
    let key = make_session(&t, "full", "回收站标题", &["第一问", "第二问"]);
    put_boundary(&t, &key, "turn_end");

    let entry = trash_chat_log_in(&t.loc, &key, "single").expect("移入须成功");
    let entry = entry.expect("三件套在场的会话须产出条目");

    let dir = t.entry_dir(&entry);
    assert!(dir.join("chat.jsonl").is_file(), "chat.jsonl 已移入");
    assert!(dir.join("meta.json").is_file(), "meta.json 已移入");
    assert!(
        dir.join("boundary.jsonl").is_file(),
        "boundary.jsonl 已移入"
    );
    assert!(dir.join("trash.json").is_file(), "清单已写");
    assert!(dir.is_dir());

    // 原三件套消失（jsonl 移走 = sessions.list 不再可见）。
    assert!(!t.loc.log_path(&key).exists(), "原 jsonl 已移走");
    assert!(!t.loc.meta_path(&key).exists(), "原 meta 已移走");
    assert!(!t.loc.boundary_path(&key).exists(), "原 boundary 已移走");

    // 清单回读。
    let raw = std::fs::read_to_string(dir.join("trash.json")).unwrap();
    let m: TrashManifest = serde_json::from_str(&raw).unwrap();
    assert_eq!(m.original_session_key, key);
    assert_eq!(m.title.as_deref(), Some("回收站标题"));
    assert_eq!(m.message_count, 2, "两条消息");
    assert!(m.bytes > 0, "三件套字节合计");
    assert_eq!(m.source, "single");
    // web 键 → display_id 剥前缀 = 裸 sid。
    assert_eq!(m.display_id, format!("nbtrash{}-full", run_hex()));

    // 列表投影。
    let list = trash_list_in(&t.loc);
    assert!(list.iter().any(|e| e["entry"] == serde_json::json!(entry)
        && e["title"] == serde_json::json!("回收站标题")
        && e["message_count"] == 2
        && e["source"] == serde_json::json!("single")
        && e["orphan"] == false));

    trash_purge_in(&t.loc, &entry).expect("purge 须成功");
    assert!(!dir.exists());
}

#[test]
fn trash_sparse_session_meta_only() {
    let t = TempTrash::new("sparse");
    // 只建 meta（无消息会话：sessions.create 即写 meta），无 jsonl/boundary。
    let key = web_key("sparse");
    put_meta(&t, &key, "空会话");
    assert!(!t.loc.log_path(&key).exists());

    let entry = trash_chat_log_in(&t.loc, &key, "batch").expect("移入须成功");
    let entry = entry.expect("meta 在场即有可恢复内容");
    let m: TrashManifest = serde_json::from_str(
        &std::fs::read_to_string(t.entry_dir(&entry).join("trash.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(m.message_count, 0);
    assert_eq!(m.title.as_deref(), Some("空会话"));
    trash_purge_in(&t.loc, &entry).unwrap();
}

#[test]
fn trash_never_materialized_session_returns_none() {
    let t = TempTrash::new("ghost");
    let key = web_key("ghost");
    let entry = trash_chat_log_in(&t.loc, &key, "single").expect("移入须成功");
    assert!(entry.is_none(), "三件套全缺席 = 无可恢复内容，不建条目");
    assert!(trash_list_in(&t.loc).is_empty(), "根目录保持空");
}

#[test]
fn same_key_double_delete_coexists() {
    let t = TempTrash::new("twice");
    let key = make_session(&t, "twice", "第一世", &["m1"]);
    let e1 = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();
    // 同键再来一次：重建 + 再删。
    put_msg(&t, &key, "user", "m2");
    let e2 = trash_chat_log_in(&t.loc, &key, "batch").unwrap().unwrap();
    assert_ne!(e1, e2, "ms 前缀让两次删除并存");
    assert!(t.entry_dir(&e1).is_dir() && t.entry_dir(&e2).is_dir());
    assert_eq!(trash_list_in(&t.loc).len(), 2);
    trash_purge_in(&t.loc, &e1).unwrap();
    trash_purge_in(&t.loc, &e2).unwrap();
}

// ---------------------------------------------------------------------------
// 恢复
// ---------------------------------------------------------------------------

#[test]
fn restore_moves_back_and_session_reappears() {
    let t = TempTrash::new("restore");
    let key = make_session(&t, "restore", "回魂", &["q1", "q2", "q3"]);
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();
    assert!(!t.loc.log_path(&key).exists());

    let out = trash_restore_in(&t.loc, &entry, None).expect("恢复须成功");
    assert_eq!(out["restored"], serde_json::json!(key));
    assert_eq!(out["renamed"], false);
    assert_eq!(out["title"], serde_json::json!("回魂"));

    // 会话复活：jsonl 在场 + 消息完整 + meta 标题保留。
    let msgs = read_msgs(&t, &key);
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].1, "q1");
    let meta_raw = std::fs::read_to_string(t.loc.meta_path(&key)).unwrap();
    let meta: SessionMeta = serde_json::from_str(&meta_raw).unwrap();
    assert_eq!(meta.title.as_deref(), Some("回魂"));

    // 条目目录已清。
    assert!(!t.entry_dir(&entry).exists(), "残留清单整目录清掉");
}

#[test]
fn restore_conflict_returns_structured_not_error() {
    let t = TempTrash::new("conflict");
    let key = make_session(&t, "conflict", "同名会话", &["a"]);
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();

    // 目标键冲突（同 id 又物化了新会话）。
    put_msg(&t, &key, "user", "新的生命");
    put_meta(&t, &key, "占位者");

    let out = trash_restore_in(&t.loc, &entry, None).expect("冲突是 Ok 不是 Err");
    assert_eq!(out["conflict"], true);
    assert_eq!(out["target_session_key"], serde_json::json!(key));
    let suggested = out["suggested_sid"].as_str().unwrap().to_string();
    assert!(
        suggested.contains("conflict-restored-"),
        "建议 sid 形态 {suggested}"
    );

    // 冲突路径不移动任何文件（三件套仍在回收站）。
    assert!(t.entry_dir(&entry).join("chat.jsonl").is_file());

    // 拍板②：提示界面带建议 sid 改名恢复。
    let out = trash_restore_in(&t.loc, &entry, Some(&suggested)).expect("改名恢复须成功");
    assert_eq!(out["renamed"], true);
    let new_key = out["restored"].as_str().unwrap().to_string();
    assert!(new_key.starts_with(WEB_SESSION_KEY_PREFIX));
    assert!(new_key.ends_with(&suggested), "新键 = 前缀 + 消毒后 sid");
    let msgs = read_msgs(&t, &new_key);
    assert_eq!(msgs.len(), 1, "恢复的是回收站里的旧内容");
    assert_eq!(msgs[0].1, "a");
    assert!(!t.entry_dir(&entry).exists(), "改名恢复成功后条目也清");
}

#[test]
fn restore_rename_composite_key_honestly_refused() {
    let t = TempTrash::new("renamec");
    let key = composite_key("nodeA");
    put_msg(&t, &key, "user", "集群侧消息");
    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();

    let err = trash_restore_in(&t.loc, &entry, Some("new-name")).expect_err("复合键改名须拒绝");
    assert!(err.contains("复合通道键"), "拒绝文案须点名键语义: {err}");
    // 目录原样保留（拒绝 ≠ 破坏）。
    assert!(t.entry_dir(&entry).join("chat.jsonl").is_file());

    // 复合键原名恢复不受限。
    let out = trash_restore_in(&t.loc, &entry, None).expect("原名恢复须成功");
    assert_eq!(out["restored"], serde_json::json!(key));
    assert_eq!(read_msgs(&t, &key).len(), 1);
}

#[test]
fn restore_project_fields_projected() {
    let t = TempTrash::new("proj");
    // 手工写带 project 归属的 meta（模拟项目会话被删）。
    let key = web_key("proj");
    put_msg(&t, &key, "user", "项目里的消息");
    let meta_json = serde_json::json!({
        "title": "项目会话",
        "project_id": "p1",
        "project_path": "C:\\proj\\demo"
    });
    std::fs::write(t.loc.meta_path(&key), meta_json.to_string()).unwrap();

    let entry = trash_chat_log_in(&t.loc, &key, "single").unwrap().unwrap();
    let out = trash_restore_in(&t.loc, &entry, None).unwrap();
    assert_eq!(out["project_id"], serde_json::json!("p1"));
    assert_eq!(out["project_path"], serde_json::json!("C:\\proj\\demo"));
    // meta 原样移回，投影只是 handler 编排依据。
    let meta_raw = std::fs::read_to_string(t.loc.meta_path(&key)).unwrap();
    let meta: SessionMeta = serde_json::from_str(&meta_raw).unwrap();
    assert_eq!(meta.project_id.as_deref(), Some("p1"));
}

// ---------------------------------------------------------------------------
// 孤儿目录
// ---------------------------------------------------------------------------

#[test]
fn orphan_dir_listed_and_web_key_restorable() {
    let t = TempTrash::new("orphan-web");
    // 手工造孤儿目录（清单缺失）：目录名按 sanitize 真实形态反推 web 键
    // （sid 段无 `_` → 反推无损）。
    let sid = format!("nbtrash{}-orphanweb", run_hex());
    let name = format!(
        "1700000000000_{}",
        nemesis_utils::sanitize::sanitize_path_segment(&format!("{WEB_SESSION_KEY_PREFIX}{sid}"))
    );
    let dir = t.entry_dir(&name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("chat.jsonl"),
        "{\"role\":\"user\",\"content\":\"孤儿\"}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("meta.json"),
        serde_json::json!({"title": "孤儿标题"}).to_string(),
    )
    .unwrap();

    let key = format!("{WEB_SESSION_KEY_PREFIX}{sid}");
    let list = trash_list_in(&t.loc);
    let found = list.iter().find(|e| e["entry"] == serde_json::json!(name));
    let found = found.expect("孤儿须诚实列出");
    assert_eq!(found["orphan"], true);
    assert_eq!(found["original_session_key"], serde_json::json!(key));
    assert_eq!(found["display_id"], serde_json::json!(sid));
    assert_eq!(found["title"], serde_json::json!("孤儿标题"));
    assert_eq!(found["message_count"], 1, "孤儿消息数按文件实数");

    let out = trash_restore_in(&t.loc, &name, None).expect("web 键孤儿可恢复");
    assert_eq!(out["restored"], serde_json::json!(key));
    assert_eq!(read_msgs(&t, &key).len(), 1);
}

#[test]
fn orphan_composite_key_restore_refused_but_listed() {
    let t = TempTrash::new("orphan-comp");
    // 复合键孤儿：目录名 = 真实 sanitize 形态；反推走 naive `_`→`:`（有损：
    // `/` 段也被替换成 `_` 再翻回 `:`）——诚实列出，恢复拒绝。
    let key = composite_key("n1");
    let name = format!(
        "1700000001000_{}",
        nemesis_utils::sanitize::sanitize_path_segment(&key)
    );
    let dir = t.entry_dir(&name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("chat.jsonl"),
        "{\"role\":\"user\",\"content\":\"x\"}\n",
    )
    .unwrap();

    let list = trash_list_in(&t.loc);
    let found = list.iter().find(|e| e["entry"] == serde_json::json!(name));
    let found = found.expect("孤儿须列出");
    assert_eq!(found["orphan"], true);
    // 反推键非 web 前缀（复合键的 sanitize 形态翻回 `:` 后无 web 前缀）。
    assert!(
        !found["original_session_key"]
            .as_str()
            .unwrap()
            .starts_with(WEB_SESSION_KEY_PREFIX)
    );

    let err = trash_restore_in(&t.loc, &name, None).expect_err("复合键孤儿恢复须拒绝");
    assert!(err.contains("有损") || err.contains("孤儿"), "{err}");
    assert!(dir.is_dir(), "拒绝 ≠ 破坏，目录保留");
}

// ---------------------------------------------------------------------------
// purge / purge_all / 校验
// ---------------------------------------------------------------------------

#[test]
fn purge_all_clears_own_entries() {
    let t = TempTrash::new("pa");
    let k1 = make_session(&t, "pa1", "t1", &["x"]);
    let k2 = make_session(&t, "pa2", "t2", &["y"]);
    let e1 = trash_chat_log_in(&t.loc, &k1, "batch").unwrap().unwrap();
    let e2 = trash_chat_log_in(&t.loc, &k2, "batch").unwrap().unwrap();
    assert_eq!(trash_list_in(&t.loc).len(), 2);

    let n = trash_purge_all_in(&t.loc).expect("清空须成功");
    assert_eq!(n, 2, "清掉本 loc 全部条目");
    assert!(trash_list_in(&t.loc).is_empty());
    assert!(!t.entry_dir(&e1).exists() && !t.entry_dir(&e2).exists());
}

#[test]
fn invalid_entry_names_rejected() {
    let t = TempTrash::new("invalid");
    for bad in ["../escape", "a/b", "a\\b", "..", "", "x/../y"] {
        assert!(
            trash_restore_in(&t.loc, bad, None).is_err(),
            "restore 须拒绝非法条目名 {bad:?}"
        );
        assert!(
            trash_purge_in(&t.loc, bad).is_err(),
            "purge 须拒绝非法条目名 {bad:?}"
        );
    }
    assert!(
        trash_restore_in(&t.loc, "不存在条目", None).is_err(),
        "不存在的条目诚实报错"
    );
}

#[test]
fn entry_name_shows_deleted_at_prefix_and_sort_order() {
    let t = TempTrash::new("order");
    let k = make_session(&t, "order", "序", &["z"]);
    let e = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();
    let (ms, safe) = e.split_once('_').unwrap();
    let ms: u64 = ms.parse().unwrap();
    assert_eq!(
        safe,
        nemesis_utils::sanitize::sanitize_path_segment(&k),
        "目录名 safe 段与生产寻址同源"
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    assert!(
        now >= ms && now - ms < 60_000,
        "目录名 ms 前缀 ≈ 删除时刻（{ms} vs now {now}）"
    );

    // 两条目并存时新删在前（ms desc + entry desc）。
    put_msg(&t, &k, "user", "z2");
    let e2 = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();
    let list = trash_list_in(&t.loc);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["entry"], serde_json::json!(e2), "新删在前");
    assert_eq!(list[1]["entry"], serde_json::json!(e));
    trash_purge_in(&t.loc, &e).unwrap();
    trash_purge_in(&t.loc, &e2).unwrap();
}

// ---------------------------------------------------------------------------
// 四件套（2026-11-05 BUG①）：todo 文件随会话进出回收站
// ---------------------------------------------------------------------------

/// BUG① 根修回归：软删把 todo 一并移入条目（live 不残留孤儿）。
#[test]
fn trash_moves_todo_into_entry() {
    let t = TempTrash::new("todo-in");
    let k = make_session(&t, "todo-in", "带清单", &["q"]);
    put_todo(
        &t,
        &k,
        &[serde_json::json!({"content": "任务A", "status": "completed"})],
    );

    let entry = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();
    // live todo 已随移入消失（BUG① 的孤儿形态不再产生）。
    assert!(!t.loc.todo_path(&k).exists(), "live todo 须移走");
    // 条目内 todo 副本 + 清单标记。
    assert!(t.entry_dir(&entry).join("todo.json").exists());
    let raw = std::fs::read_to_string(t.entry_dir(&entry).join("trash.json")).unwrap();
    let m: TrashManifest = serde_json::from_str(&raw).unwrap();
    assert!(m.todo, "清单 todo 标记须为 true");
    assert!(m.bytes > 0);
}

/// 无 todo 的会话软删 = 条目形态与旧版逐字节同构（todo:false、无副本）。
#[test]
fn trash_without_todo_keeps_legacy_entry_shape() {
    let t = TempTrash::new("todo-absent");
    let k = make_session(&t, "todo-absent", "无清单", &["q"]);
    let entry = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();
    assert!(!t.entry_dir(&entry).join("todo.json").exists());
    let raw = std::fs::read_to_string(t.entry_dir(&entry).join("trash.json")).unwrap();
    let m: TrashManifest = serde_json::from_str(&raw).unwrap();
    assert!(!m.todo);
}

/// 恢复把 todo 移回原键（且先清目标键 live 孤儿防 rename 撞车卡死）。
#[test]
fn restore_brings_todo_back_to_key() {
    let t = TempTrash::new("todo-restore");
    let k = make_session(&t, "todo-restore", "恢复清单", &["q"]);
    put_todo(
        &t,
        &k,
        &[serde_json::json!({"content": "B", "status": "pending"})],
    );
    let entry = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();
    // 模拟 BUG① 实例形态：条目外残留一份孤儿 live todo（旧版本产物）。
    put_todo(
        &t,
        &k,
        &[serde_json::json!({"content": "孤儿", "status": "pending"})],
    );

    let out = trash_restore_in(&t.loc, &entry, None).unwrap();
    assert_eq!(out["restored"], serde_json::json!(k));
    // 恢复后 todo 以条目内副本为准（孤儿被先清，不被覆盖失败卡死）。
    let raw = std::fs::read_to_string(t.loc.todo_path(&k)).unwrap();
    let todos: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(todos[0]["content"], serde_json::json!("B"));
    assert!(!t.entry_dir(&entry).exists());
}

/// 改名恢复：todo 落新键（与 chat/meta 同键）。
#[test]
fn restore_renamed_lands_todo_on_new_key() {
    let t = TempTrash::new("todo-rename");
    let k = make_session(&t, "todo-rename", "改名清单", &["q"]);
    put_todo(
        &t,
        &k,
        &[serde_json::json!({"content": "C", "status": "pending"})],
    );
    let entry = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();

    let out = trash_restore_in(&t.loc, &entry, Some("todo-new-sid")).unwrap();
    let new_key = out["restored"].as_str().unwrap();
    assert!(new_key.starts_with(WEB_SESSION_KEY_PREFIX));
    let raw = std::fs::read_to_string(t.loc.todo_path(new_key)).unwrap();
    let todos: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(todos[0]["content"], serde_json::json!("C"));
    assert!(!t.loc.todo_path(&k).exists(), "旧键不得残留");
}

/// purge 顺带清 live 孤儿（旧形态条目：清单无 todo 字段 + 盘上有孤儿残留
/// ——BUG① 实例的收口路径）；清单缺失（孤儿目录）不寻址清 todo。
#[test]
fn purge_clears_live_todo_orphan() {
    let t = TempTrash::new("todo-purge");
    let k = make_session(&t, "todo-purge", "孤儿收口", &["q"]);
    let entry = trash_chat_log_in(&t.loc, &k, "single").unwrap().unwrap();
    // 模拟旧形态：清单被改写为无 todo 字段（serde default = false），
    // live 孤儿残留（2026-11-05 之前删除的产物）。
    let manifest_path = t.entry_dir(&entry).join("trash.json");
    let mut m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    m.as_object_mut().unwrap().remove("todo");
    std::fs::write(&manifest_path, m.to_string()).unwrap();
    put_todo(
        &t,
        &k,
        &[serde_json::json!({"content": "孤儿", "status": "pending"})],
    );

    trash_purge_in(&t.loc, &entry).expect("purge 须成功");
    assert!(!t.loc.todo_path(&k).exists(), "live 孤儿须随 purge 清掉");
    assert!(!t.entry_dir(&entry).exists());

    // 孤儿目录（清单缺失）：purge 正常删目录，且不按不可信反推键清 todo。
    let k2 = make_session(&t, "todo-purge2", "孤儿目录", &["q"]);
    put_todo(
        &t,
        &k2,
        &[serde_json::json!({"content": "留守", "status": "pending"})],
    );
    let entry2 = trash_chat_log_in(&t.loc, &k2, "single").unwrap().unwrap();
    std::fs::remove_file(t.entry_dir(&entry2).join("trash.json")).unwrap();
    put_todo(
        &t,
        &k2,
        &[serde_json::json!({"content": "新会话", "status": "pending"})],
    );
    trash_purge_in(&t.loc, &entry2).unwrap();
    assert!(!t.entry_dir(&entry2).exists());
    assert!(
        t.loc.todo_path(&k2).exists(),
        "孤儿目录 purge 不得按反推键清 live todo"
    );
}

/// purge_all 走 trash_purge_in 单点（live todo 清扫语义一致）。
#[test]
fn purge_all_clears_live_todo_orphans_too() {
    let t = TempTrash::new("todo-pa");
    let k = make_session(&t, "todo-pa", "批量收口", &["q"]);
    let entry = trash_chat_log_in(&t.loc, &k, "batch").unwrap().unwrap();
    put_todo(
        &t,
        &k,
        &[serde_json::json!({"content": "孤儿", "status": "pending"})],
    );

    let n = trash_purge_all_in(&t.loc).unwrap();
    assert_eq!(n, 1);
    assert!(!t.loc.todo_path(&k).exists(), "批量清空须同样清 live 孤儿");
    assert!(!t.entry_dir(&entry).exists());
}
