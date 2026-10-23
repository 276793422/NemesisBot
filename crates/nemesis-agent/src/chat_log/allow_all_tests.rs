//! 会话级全部放行（2026-10-24）：sidecar 持久位三函数测试——write 开/关
//! round-trip、旧 meta 无字段=缺省 false、allow_all_sessions 扫描只认烧入
//! 真实会话键的条目。unique key + delete 清理（本目录测试惯例——走进程
//! 单例 home，不重定向）。

use super::*;

fn unique_key(tag: &str) -> String {
    format!(
        "test:allowall:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn allow_all_write_read_roundtrip() {
    let key = unique_key("roundtrip");

    // 未写时读 = None（无 meta）。
    assert!(read_session_allow_all(&key).is_none());

    // 开（显式 since 透传——调用方传内存态 set() 的返回值，单一真相）。
    write_session_allow_all(&key, true, "2026-10-24T08:00:00+08:00");
    let (enabled, since) = read_session_allow_all(&key).expect("meta 应存在");
    assert!(enabled);
    assert_eq!(since, "2026-10-24T08:00:00+08:00", "since 原样落盘");
    let meta = read_session_meta_full(&key).unwrap();
    assert_eq!(
        meta.session_key.as_deref(),
        Some(key.as_str()),
        "真实键须烧入"
    );

    // 空 since 回落现在（防御臂；正常调用不触发）。
    write_session_allow_all(&key, true, "");
    let (_, since) = read_session_allow_all(&key).unwrap();
    assert!(!since.is_empty(), "空 since 须回落现在");

    // 其余 meta 字段共存不互伤（RMW 保留语义）。
    write_session_meta(&key, "长任务会话");
    write_session_allow_all(&key, true, "2026-10-24T09:00:00+08:00"); // 幂等重开
    let meta = read_session_meta_full(&key).unwrap();
    assert_eq!(meta.title.as_deref(), Some("长任务会话"));
    assert!(meta.allow_all);

    // 关：allow_all 摘除（回默认），title 保留（since 入参关闭路径忽略）。
    write_session_allow_all(&key, false, "");
    let (enabled, since) = read_session_allow_all(&key).expect("关后 meta 仍在");
    assert!(!enabled);
    assert!(since.is_empty(), "关闭后 since 摘除");
    let meta = read_session_meta_full(&key).unwrap();
    assert_eq!(meta.title.as_deref(), Some("长任务会话"));
    assert!(meta.allow_all_since.is_none());

    // 关后再关 = no-op（不炸不空写）。
    write_session_allow_all(&key, false, "");
    assert!(!read_session_allow_all(&key).unwrap().0);

    let _ = std::fs::remove_file(super::meta_path(&key));
}

#[test]
fn allow_all_legacy_meta_without_field_defaults_false() {
    let key = unique_key("legacy");
    // 手写一个只带 title 的旧形态 meta（无 allow_all 字段）。
    let path = meta_path(&key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"title":"旧会话"}"#).unwrap();

    let (enabled, since) = read_session_allow_all(&key).unwrap();
    assert!(!enabled, "旧 meta 缺字段 = 缺省 false");
    assert!(since.is_empty());

    // allow_all_sessions 不得把旧 meta 扫进来。
    assert!(!allow_all_sessions().iter().any(|(k, _)| *k == key));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn allow_all_sessions_scans_enabled_only_with_real_key() {
    let key_on = unique_key("scan-on");
    let key_off = unique_key("scan-off");

    write_session_allow_all(&key_on, true, "2026-10-24T08:00:00+08:00");
    write_session_allow_all(&key_off, true, "2026-10-24T08:00:00+08:00");
    write_session_allow_all(&key_off, false, ""); // 开了又关

    let found = allow_all_sessions();
    let hit = found
        .iter()
        .find(|(k, _)| *k == key_on)
        .expect("开启会话须被扫到");
    assert!(!hit.1.is_empty(), "扫描结果带 since");
    assert!(
        !found.iter().any(|(k, _)| *k == key_off),
        "已关闭会话不得被扫到"
    );

    let _ = std::fs::remove_file(meta_path(&key_on));
    let _ = std::fs::remove_file(meta_path(&key_off));
}

#[test]
fn allow_all_scan_skips_missing_session_key_field() {
    // 手工改盘形态：allow_all=true 但没烧 session_key —— 防御性跳过
    // （真实键无从得知，还原不出的条目不进 hydrate）。
    let key = unique_key("nokey");
    let path = meta_path(&key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"allow_all":true,"allow_all_since":"2026-10-24T08:00:00+08:00"}"#,
    )
    .unwrap();

    assert!(
        !allow_all_sessions().iter().any(|(k, _)| *k == key),
        "缺 session_key 字段的条目不得进 hydrate"
    );

    let _ = std::fs::remove_file(&path);
}
