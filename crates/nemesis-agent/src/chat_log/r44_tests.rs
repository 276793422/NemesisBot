//! BUG-20261025-44（BUG-HUNT R121）：会话持久化面读侧损坏静默吞 → loud 留痕
//! 回归锁（nemesis-session load_chat_sessions 的镜像面在本 crate 的三处
//! meta 读取点：read_meta_full / snoozed_sessions / allow_all_sessions）。
//!
//! 写侧早已原子（write_meta_file_atomic，2026-10-24 R1）；读侧遇到的损坏
//! 只剩外部篡改/历史残留/磁盘问题——正是需要第一个可见信号的场景。修复前
//! 四点全部 `let Ok(...) else continue/None` 静默吞，其中 read_meta_full 的
//! 静默 None 会被 upsert 以空基线重写放大成 title/血缘/项目归属/allow_all
//! 的静默全抹。
//!
//! 修复 = 四点全部 loud warn，跳过语义不变。warn 本身无法在进程内断言（无
//! tracing 捕获夹具，BUG-43 同族诚实边界）；本批锁可观察行为：
//! ① 损坏 meta 读取返 None（跳过语义）+ 后续 upsert 空基线重写自愈成合法
//!    文件（rebaseline 语义未变，但现在有迹可查）；
//! ② snoozed / allow_all 扫描对混档目录（好+坏并存）分拣正确、不炸。

use super::*;

fn r44_unique_key(tag: &str) -> String {
    format!(
        "test:r44:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// ① 损坏 meta：read 侧返 None（跳过语义不变），后续 upsert 以空基线重写
/// 后文件自愈成合法 JSON（rebaseline 行为未变——修复只加可观测性，不改
/// 语义；损坏本身现在有 warn 留痕）。
#[test]
fn corrupt_meta_reads_none_then_upsert_rebaselines_and_heals() {
    let key = r44_unique_key("heal");
    let path = meta_path(&key);
    // 模拟修复前直写时代/外部损坏的撕裂 meta。
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"title": "半截"#).unwrap();

    // 读侧：None（不炸、不返垃圾）。
    assert!(read_session_meta_full(&key).is_none(), "损坏 meta 返 None");
    assert!(read_session_meta(&key).is_none());

    // upsert：空基线重写（title/血缘不复活——语义未变），文件自愈。
    write_session_snooze(&key, "2026-12-01T00:00:00+08:00", None);
    let meta = read_session_meta_full(&key).expect("upsert 后 meta 必须重新可解析（自愈）");
    assert_eq!(meta.title, None, "空基线 rebaseline：损坏前的 title 不复活");
    assert_eq!(
        meta.snoozed_until.as_deref(),
        Some("2026-12-01T00:00:00+08:00")
    );

    delete_chat_log(&key);
}

/// ② snoozed_sessions：好+坏混档扫描分拣正确——好档条目照常返回，坏档
/// 静默跳过变 loud 跳过后语义不变（不炸、不产出垃圾条目）。
#[test]
fn snoozed_scan_skips_corrupt_but_finds_valid() {
    let key_ok = r44_unique_key("scan-ok");
    write_session_snooze(&key_ok, "2026-12-01T00:00:00+08:00", Some("nb"));

    let key_bad = r44_unique_key("scan-bad");
    let bad_path = meta_path(&key_bad);
    std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    // 坏档内容含 snoozed_until 字段——证明跳过点在解析层而非字段缺失。
    std::fs::write(&bad_path, r#"{"snoozed_until": "2026-12-01T00"#).unwrap();

    let stem_ok = nemesis_utils::sanitize::sanitize_path_segment(&key_ok);
    let stem_bad = nemesis_utils::sanitize::sanitize_path_segment(&key_bad);
    let found = snoozed_sessions();
    let hit = found
        .iter()
        .find(|(s, _, _)| *s == stem_ok)
        .expect("好档条目必须照常返回");
    assert_eq!(hit.2.as_deref(), Some("nb"));
    assert!(
        !found.iter().any(|(s, _, _)| *s == stem_bad),
        "坏档不得产出垃圾条目"
    );

    delete_chat_log(&key_ok);
    delete_chat_log(&key_bad);
}

/// ③ allow_all_sessions：同②——放行状态 hydrate 扫描对坏档跳过（语义
/// 未变，fail-safe）+ 好档照常返回。
#[test]
fn allow_all_scan_skips_corrupt_but_finds_valid() {
    let key_ok = r44_unique_key("allow-ok");
    write_session_allow_all(&key_ok, true, "2026-10-25T12:00:00+08:00").expect("落盘成功");

    let key_bad = r44_unique_key("allow-bad");
    let bad_path = meta_path(&key_bad);
    std::fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    std::fs::write(&bad_path, r#"{"allow_all": true, "session_ke"#).unwrap();

    let found = allow_all_sessions();
    assert!(
        found
            .iter()
            .any(|(k, since)| k == &key_ok && since == "2026-10-25T12:00:00+08:00"),
        "好档放行条目必须照常 hydrate，实际 {found:?}"
    );
    assert!(
        !found.iter().any(|(k, _)| k == &key_bad),
        "坏档不得产出垃圾条目"
    );

    delete_chat_log(&key_ok);
    delete_chat_log(&key_bad);
}
