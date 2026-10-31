//! proxy_file 单测（临时目录隔离；纯读不落盘钉子 + RMW 语义 + 校验规则）。

use crate::proxy_file::*;
use std::path::PathBuf;
use std::sync::Mutex;

// 写路径走 config_write_lock（进程级串行锁）；本组测试各自独占临时目录，
// SERIAL 仅保证同进程多测试的写序卫生（plugin_wasm_tests 同款）。
static SERIAL: Mutex<()> = Mutex::new(());

fn temp_config_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nb_proxy_file_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn missing_file_loads_none_and_never_writes() {
    let dir = temp_config_dir("missing");
    let _g = SERIAL.lock().unwrap();
    // 文件缺失 = 未配置：读路径静默 None，纯读不得落盘（用户裁决：无文件
    // 不自动生成，首次保存才落盘）。
    assert!(load(&dir).unwrap().is_none());
    assert_eq!(load_env(&dir), EnvProxyConfig::default());
    assert_eq!(load_selfupdate_proxy(&dir), None);
    assert!(
        !proxy_config_path(&dir).exists(),
        "纯读不得落盘（不自动生成配置文件）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn first_save_creates_file_and_roundtrips() {
    let dir = temp_config_dir("roundtrip");
    let _g = SERIAL.lock().unwrap();
    let env = EnvProxyConfig {
        http_proxy: "http://127.0.0.1:7890".into(),
        https_proxy: "http://127.0.0.1:7890".into(),
        all_proxy: String::new(),
        no_proxy: "localhost,127.0.0.1".into(),
    };
    let path = save_env(&dir, &env).unwrap();
    assert_eq!(path, proxy_config_path(&dir));
    assert!(path.exists(), "首次保存创建文件");
    let loaded = load(&dir).unwrap().unwrap();
    assert_eq!(loaded.env, env);
    assert_eq!(loaded.selfupdate, SelfupdateProxyConfig::default());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sections_save_independently_no_clobber() {
    let dir = temp_config_dir("rmw");
    let _g = SERIAL.lock().unwrap();
    save_env(
        &dir,
        &EnvProxyConfig {
            http_proxy: "http://p1:1".into(),
            ..Default::default()
        },
    )
    .unwrap();
    save_selfupdate(&dir, "socks5://p2:2").unwrap();
    // selfupdate 保存不抹 env 段；反向亦然。
    let after_second = load(&dir).unwrap().unwrap();
    assert_eq!(after_second.env.http_proxy, "http://p1:1");
    assert_eq!(after_second.selfupdate.proxy, "socks5://p2:2");
    save_env(
        &dir,
        &EnvProxyConfig {
            https_proxy: "https://p3:3".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let after_third = load(&dir).unwrap().unwrap();
    assert_eq!(after_third.env.https_proxy, "https://p3:3");
    assert_eq!(
        after_third.selfupdate.proxy, "socks5://p2:2",
        "env 保存不抹 selfupdate 段"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_file_load_is_err_and_save_rebuilds() {
    let dir = temp_config_dir("corrupt");
    std::fs::write(proxy_config_path(&dir), "{ not valid json !!!").unwrap();
    // 损坏 = Err 带原因（调用方 WARN + 按未配置走，不阻断启动）。
    assert!(load(&dir).is_err());
    // 损坏文件上保存 = 以空底重建（无从解析保留）。
    save_selfupdate(&dir, "http://p:1").unwrap();
    let rebuilt = load(&dir).unwrap().unwrap();
    assert_eq!(rebuilt.selfupdate.proxy, "http://p:1");
    assert_eq!(rebuilt.env, EnvProxyConfig::default());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn half_schema_missing_keys_default() {
    let dir = temp_config_dir("half");
    std::fs::write(
        proxy_config_path(&dir),
        r#"{"env": {"http_proxy": "http://h:1"}}"#,
    )
    .unwrap();
    let c = load(&dir).unwrap().unwrap();
    assert_eq!(c.env.http_proxy, "http://h:1");
    assert_eq!(c.env.https_proxy, "", "缺键 serde default 兜底");
    assert_eq!(c.selfupdate.proxy, "");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validation_prefix_rules() {
    // 合法前缀族
    for p in [
        "http://a:1",
        "https://a:1",
        "socks://a:1",
        "socks5://a:1",
        "socks5h://a:1",
    ] {
        assert!(validate_proxy_value(p, false).is_ok(), "{p} 应合法");
    }
    // 非法：裸 host:port / 其它 scheme
    assert!(validate_proxy_value("127.0.0.1:7890", false).is_err());
    assert!(validate_proxy_value("ftp://a:1", false).is_err());
    // 空 = 合法（不设置该项）；no_proxy 豁免（自由文本）
    assert!(validate_proxy_value("", false).is_ok());
    assert!(validate_proxy_value("   ", false).is_ok());
    assert!(validate_proxy_value("localhost,127.0.0.1,.internal", true).is_ok());
    // 校验不过 = 保存 loud 拒绝不落盘
    let dir = temp_config_dir("reject");
    let _g = SERIAL.lock().unwrap();
    let err = save_env(
        &dir,
        &EnvProxyConfig {
            http_proxy: "127.0.0.1:7890".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.contains("http://"), "错误信息带合法前缀提示：{err}");
    assert!(!proxy_config_path(&dir).exists(), "校验拒绝不得落盘");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn env_vars_to_apply_pairs_and_skips_empty() {
    let env = EnvProxyConfig {
        http_proxy: " http://p:1 ".into(),
        https_proxy: String::new(),
        all_proxy: "socks5://a:2".into(),
        no_proxy: "localhost".into(),
    };
    let pairs = env_vars_to_apply(&env);
    // 非空键 = 大小写双对；空键不产出。
    assert_eq!(
        pairs,
        vec![
            ("HTTP_PROXY", "http://p:1".to_string()),
            ("http_proxy", "http://p:1".to_string()),
            ("ALL_PROXY", "socks5://a:2".to_string()),
            ("all_proxy", "socks5://a:2".to_string()),
            ("NO_PROXY", "localhost".to_string()),
            ("no_proxy", "localhost".to_string()),
        ]
    );
    // 全空 = 零注入（不碰任何变量）。
    assert!(env_vars_to_apply(&EnvProxyConfig::default()).is_empty());
}

#[test]
fn selfupdate_proxy_empty_is_none() {
    let dir = temp_config_dir("empty-selfupdate");
    let _g = SERIAL.lock().unwrap();
    save_selfupdate(&dir, "  ").unwrap();
    assert_eq!(
        load(&dir).unwrap().unwrap().selfupdate.proxy,
        "",
        "保存时 trim"
    );
    // 空白 = 直连（load_selfupdate_proxy trim 后判空）。
    assert_eq!(load_selfupdate_proxy(&dir), None);
    let _ = std::fs::remove_dir_all(&dir);
}
