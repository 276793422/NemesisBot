//! proxy_keys_file 单测（临时目录隔离；缺文件空表不落盘钉子 + RMW 查重 +
//! 启停幂等 + 损坏重建 + 宽和读回落）。

use crate::proxy_keys_file::*;
use std::path::PathBuf;
use std::sync::Mutex;

// 写路径走 config_write_lock（进程级串行锁）；本组测试各自独占临时目录，
// SERIAL 仅保证同进程多测试的写序卫生（proxy_file_tests 同款）。
static SERIAL: Mutex<()> = Mutex::new(());

fn temp_config_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nb_proxy_keys_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn entry(name: &str, key: &str) -> ProxyKeyEntry {
    ProxyKeyEntry {
        name: name.to_string(),
        key: key.to_string(),
        enabled: true,
        created_at: 0,
        note: String::new(),
    }
}

#[test]
fn missing_file_empty_list_and_never_writes() {
    let dir = temp_config_dir("missing");
    let _g = SERIAL.lock().unwrap();
    // 文件缺失 = 未配置零 key：读路径空表，纯读不得落盘（无默认模板，
    // 首次 add 才落盘）。
    assert!(load(&dir).unwrap().is_none());
    assert!(list(&dir).is_empty());
    assert!(
        !proxy_keys_config_path(&dir).exists(),
        "纯读不得落盘（不自动生成配置文件）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_key_creates_file_and_roundtrips() {
    let dir = temp_config_dir("roundtrip");
    let _g = SERIAL.lock().unwrap();
    let path = add_key(
        &dir,
        ProxyKeyEntry {
            note: "测试 bot A".to_string(),
            ..entry("bot-a", "nbk_aaa")
        },
    )
    .unwrap();
    assert_eq!(path, proxy_keys_config_path(&dir));
    assert!(path.exists(), "首次 add 创建文件");
    let keys = list(&dir);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "bot-a");
    assert_eq!(keys[0].key, "nbk_aaa");
    assert_eq!(keys[0].note, "测试 bot A");
    assert!(keys[0].enabled, "缺省启用");
    assert!(keys[0].created_at > 0, "created_at=0 自动补 Unix 秒戳");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_rejects_duplicate_name_and_key() {
    let dir = temp_config_dir("dup");
    let _g = SERIAL.lock().unwrap();
    add_key(&dir, entry("bot-a", "nbk_aaa")).unwrap();
    assert!(
        add_key(&dir, entry("bot-a", "nbk_bbb")).is_err(),
        "重名拒绝"
    );
    assert!(
        add_key(&dir, entry("bot-b", "nbk_aaa")).is_err(),
        "重 key 拒绝（不同调用方不能共用同一 key）"
    );
    assert_eq!(list(&dir).len(), 1, "拒绝后文件内容不变");
    // 边界：空 name / 空 key loud 拒绝。
    assert!(add_key(&dir, entry("  ", "nbk_ccc")).is_err());
    assert!(add_key(&dir, entry("bot-c", " ")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn add_trims_name_and_key() {
    let dir = temp_config_dir("trim");
    let _g = SERIAL.lock().unwrap();
    add_key(&dir, entry("  bot-a  ", "  nbk_aaa ")).unwrap();
    let keys = list(&dir);
    assert_eq!(keys[0].name, "bot-a");
    assert_eq!(keys[0].key, "nbk_aaa");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_and_set_enabled_semantics() {
    let dir = temp_config_dir("rmw");
    let _g = SERIAL.lock().unwrap();
    add_key(&dir, entry("bot-a", "nbk_aaa")).unwrap();
    add_key(&dir, entry("bot-b", "nbk_bbb")).unwrap();
    // 启停：改值生效；重复设同值幂等（不报错）。
    assert!(set_enabled(&dir, "bot-a", false).unwrap());
    assert!(
        !list(&dir)
            .iter()
            .find(|k| k.name == "bot-a")
            .unwrap()
            .enabled
    );
    assert!(
        set_enabled(&dir, "bot-a", false).unwrap(),
        "幂等：同值再设仍 Ok"
    );
    // 不存在名称：false 不落盘。
    assert!(!set_enabled(&dir, "ghost", true).unwrap());
    assert!(!remove_key(&dir, "ghost").unwrap());
    // 删除：只删目标，其余保留。
    assert!(remove_key(&dir, "bot-a").unwrap());
    let keys = list(&dir);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "bot-b");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_file_lenient_read_and_rebuild_on_write() {
    let dir = temp_config_dir("corrupt");
    let _g = SERIAL.lock().unwrap();
    std::fs::write(proxy_keys_config_path(&dir), "{not json").unwrap();
    // 宽和读：损坏 = 空 key 表 + WARN（回落主 token 校验，不锁死老路径）。
    assert!(list(&dir).is_empty());
    // 损坏上写 = 空底重建（proxy_file 同口径）。
    add_key(&dir, entry("bot-a", "nbk_aaa")).unwrap();
    let keys = list(&dir);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "bot-a");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_all_validates_and_replaces() {
    let dir = temp_config_dir("saveall");
    let _g = SERIAL.lock().unwrap();
    add_key(&dir, entry("bot-a", "nbk_aaa")).unwrap();
    // 全量校验：重名 / 重 key / 空 name 拒绝，原文件不动。
    assert!(save_all(&dir, &[entry("x", "k1"), entry("x", "k2")]).is_err());
    assert!(save_all(&dir, &[entry("x", "k1"), entry("y", "k1")]).is_err());
    assert!(save_all(&dir, &[entry("", "k1")]).is_err());
    assert_eq!(list(&dir).len(), 1, "校验失败不得落盘");
    // 合法全量替换。
    save_all(
        &dir,
        &[entry("bot-x", "nbk_xxx"), entry("bot-y", "nbk_yyy")],
    )
    .unwrap();
    let keys = list(&dir);
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].name, "bot-x");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_generation_increments_on_every_successful_write() {
    let dir = temp_config_dir("writgen");
    let _g = SERIAL.lock().unwrap();
    // 写代号每次成功落盘 +1（消费侧缓存新鲜度合取信号——同 tick 双写
    // mtime 相同时代号是唯一失配源，BUG-20261103-350）。
    let g0 = write_generation();
    add_key(&dir, entry("bot-a", "nbk_aaa")).unwrap();
    assert_eq!(write_generation(), g0 + 1, "add_key 落盘必须递增代号");
    set_enabled(&dir, "bot-a", false).unwrap();
    assert_eq!(write_generation(), g0 + 2, "set_enabled 落盘必须递增代号");
    // 幂等（状态不变不落盘）不递增。
    set_enabled(&dir, "bot-a", false).unwrap();
    assert_eq!(write_generation(), g0 + 2, "幂等不落盘不得递增代号");
    remove_key(&dir, "bot-a").unwrap();
    assert_eq!(write_generation(), g0 + 3, "remove_key 落盘必须递增代号");
    // 失败操作（重名拒绝）不落盘不递增。
    assert!(add_key(&dir, entry("", "")).is_err());
    assert_eq!(write_generation(), g0 + 3, "校验失败不落盘不得递增代号");
    let _ = std::fs::remove_dir_all(&dir);
}
