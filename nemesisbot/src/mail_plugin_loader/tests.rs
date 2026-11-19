//! mail_plugin_loader 单测（无插件环境友好：CI/exe 旁一般无 plugin_mail.dll
//! ——在场断言条件化，缺席路径恒可测；roaming 同款纪律）。

use std::ffi::CString;
use std::path::Path;

use super::*;
use nemesis_config::plugins_native::NativePluginId;

#[test]
fn host_vtable_version_matches_contract() {
    // 双向闸锚点：宿主 vtable 首字段必须与契约版本常量一致——构造错位
    // 会被插件 init 侧 -2 拒绝，此处编译期形态前移到断言钉死。
    assert_eq!(HOST_VTABLE.abi_version, HOST_MAIL_ABI_VERSION);
    assert_eq!(HOST_MAIL_ABI_VERSION, 1);
    // 回调全部就位（冻结面：宿主半边不许 None）。
    assert!(HOST_VTABLE.log.is_some());
    assert!(HOST_VTABLE.get_home_dir.is_some());
    assert!(HOST_VTABLE.audit_event.is_some());
    assert!(HOST_VTABLE.notify_mail_in.is_some());
    assert!(HOST_VTABLE.notify_mail_sent.is_some());
}

#[test]
fn write_cstr_truncation_and_bounds() {
    let mut buf = [0 as c_char; 8];
    assert_eq!(write_cstr(buf.as_mut_ptr(), buf.len(), "abc"), 3);
    // 恰好放下（3 字节 + NUL = 4）。
    assert_eq!(write_cstr(buf.as_mut_ptr(), 4, "abc"), 3);
    // 截断 = -1（契约同文长度语义：>0 成功 / -1 缓冲不足）。
    assert_eq!(write_cstr(buf.as_mut_ptr(), 3, "abc"), -1);
    assert_eq!(write_cstr(buf.as_mut_ptr(), 0, "abc"), -1);
    assert_eq!(write_cstr(std::ptr::null_mut(), 8, "abc"), -1);
}

#[test]
fn load_absent_is_absent_not_error() {
    // 插件文件在 exe 旁缺席（常态）→ Absent；在场则本测退化为幂等探测
    // （重复 load → Loaded），两种环境都绿。名册开关缺省 = 启用（临时空
    // config_dir），闸放行走装载链。
    let cfg = tempfile::tempdir().unwrap();
    match load_mail_plugin(Path::new("."), cfg.path()) {
        MailLoadOutcome::Absent => assert!(!mail_plugin_present()),
        MailLoadOutcome::Loaded => assert!(mail_plugin_present()),
        MailLoadOutcome::Rejected(e) => {
            // 在场但被拒（签名/ABI/init）——记录形态即可，不算测红。
            assert!(!e.is_empty());
        }
        MailLoadOutcome::DisabledBySwitch => {
            panic!("缺省名册开关必须 = 启用（空 config_dir 无开关文件）")
        }
    }
}

#[test]
fn roster_switch_disabled_short_circuits() {
    let cfg = tempfile::tempdir().unwrap();
    nemesis_config::plugins_native::set_enabled(cfg.path(), NativePluginId::PluginMail, false)
        .expect("写开关");
    // 名册关 = DisabledBySwitch（文件在场与否无关——关了就是不装）。
    assert!(matches!(
        load_mail_plugin(Path::new("."), cfg.path()),
        MailLoadOutcome::DisabledBySwitch
    ));
}

// ---------------------------------------------------------------------------
// 总闸（config.mail.json enabled）
// ---------------------------------------------------------------------------

fn temp_home_with_config(body: &str) -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let cfg_dir = home.path().join("workspace").join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let path = cfg_dir.join("config.mail.json");
    std::fs::write(&path, body).unwrap();
    (home, path)
}

#[test]
fn mail_enabled_gate_defaults_false() {
    let home = tempfile::tempdir().unwrap();
    // 缺文件 = false（显式开启形态——静默关闭是默认态不是错误）。
    assert!(!mail_enabled(home.path()));
    // 空对象 / 缺 enabled 键 = false。
    let (h, _) = temp_home_with_config("{}");
    assert!(!mail_enabled(h.path()));
    let (h, _) = temp_home_with_config(r#"{"smtp":{"port":2525}}"#);
    assert!(!mail_enabled(h.path()));
    // enabled=false 显式 = false。
    let (h, _) = temp_home_with_config(r#"{"enabled":false}"#);
    assert!(!mail_enabled(h.path()));
    // enabled=true = true。
    let (h, _) = temp_home_with_config(r#"{"enabled":true}"#);
    assert!(mail_enabled(h.path()));
    // BOM 容忍（config 家族带 BOM 是已知形态）。
    let (h, _) = temp_home_with_config("\u{feff}{\"enabled\":true}");
    assert!(mail_enabled(h.path()));
    // 坏 JSON = false（诚实降级，不炸装配点）。
    let (h, _) = temp_home_with_config("not json at all");
    assert!(!mail_enabled(h.path()));
}

// ---------------------------------------------------------------------------
// init JSON 构造（password_ref 解析注入；fail-closed）
// ---------------------------------------------------------------------------

#[test]
fn init_json_literal_passthrough_and_password_ref_resolution() {
    // 字面量 password 原样透传（无 password_ref 时不注入不覆写）。
    let (h, _) = temp_home_with_config(
        r#"{"enabled":true,"pop3":{"enabled":true,"password":"plain1"},"client":{"enabled":true,"password":"plain2"}}"#,
    );
    let json = mail_init_json(h.path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["pop3"]["password"], "plain1");
    assert_eq!(v["client"]["password"], "plain2");
    assert!(v.get("pop3").and_then(|p| p.get("password_ref")).is_none());

    // password_ref 字面量 = 解析注入 + 键摘除（插件不见 vault 通道字段）。
    let (h, _) = temp_home_with_config(
        r#"{"pop3":{"password_ref":"literal-secret"},"client":{"password_ref":"client-secret"}}"#,
    );
    let json = mail_init_json(h.path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["pop3"]["password"], "literal-secret");
    assert_eq!(v["client"]["password"], "client-secret");
    assert!(v["pop3"].get("password_ref").is_none());
    assert!(v["client"].get("password_ref").is_none());

    // 空 password_ref = 不解析（保持缺省空密码形态）。
    let (h, _) = temp_home_with_config(r#"{"pop3":{"password_ref":""}}"#);
    let json = mail_init_json(h.path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v["pop3"].get("password").is_none());
}

#[test]
fn init_json_env_resolution_and_fail_closed() {
    // env: 引用 = 解析注入。
    unsafe { std::env::set_var("NB_MAIL_LOADER_TEST_PW", "env-secret-42") };
    let (h, _) = temp_home_with_config(r#"{"pop3":{"password_ref":"env:NB_MAIL_LOADER_TEST_PW"}}"#);
    let json = mail_init_json(h.path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["pop3"]["password"], "env-secret-42");
    unsafe { std::env::remove_var("NB_MAIL_LOADER_TEST_PW") };

    // 显式引用解析不出 = fail-closed Err（静默落空密码 = 开放认证面）。
    let (h, _) =
        temp_home_with_config(r#"{"pop3":{"password_ref":"env:NB_MAIL_LOADER_MISSING_VAR_XYZ"}}"#);
    assert!(
        mail_init_json(h.path()).is_err(),
        "解析失败必须拒绝（fail-closed）"
    );

    // vault: 未知别名同理 fail-closed。
    let (h, _) =
        temp_home_with_config(r#"{"pop3":{"password_ref":"vault:nonexistent-alias-xyz"}}"#);
    assert!(mail_init_json(h.path()).is_err());
}

// ---------------------------------------------------------------------------
// 通知槽分发
// ---------------------------------------------------------------------------

#[test]
fn notify_sink_dispatch() {
    // 注入 sink → host_notify_mail_in 分发；摘除后回落 tracing 留痕路径
    // （不炸）。extern fn 直调——真实路径经 vtable 传入插件。
    let got = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let sink_clone = got.clone();
    set_notify_in_sink(Arc::new(move |meta: &str| {
        sink_clone.lock().push(meta.to_string());
    }));
    let c = CString::new(r#"{"id":"m1","mailbox":"zoo@corp.example"}"#).unwrap();
    host_notify_mail_in(c.as_ptr());
    assert_eq!(got.lock().len(), 1);
    assert!(got.lock()[0].contains("zoo@corp.example"));

    let got_sent = Arc::new(parking_lot::Mutex::new(false));
    let sink_clone = got_sent.clone();
    set_notify_sent_sink(Arc::new(move |_r: &str| {
        *sink_clone.lock() = true;
    }));
    let c = CString::new(r#"{"ok":true}"#).unwrap();
    host_notify_mail_sent(c.as_ptr());
    assert!(*got_sent.lock());

    // 摘除（None 路径 = tracing 留痕，不得炸）。
    *NOTIFY_IN_SINK.write() = None;
    host_notify_mail_in(c.as_ptr());
}

#[test]
fn audit_sink_dispatch() {
    // 注入审计 sink → host_audit_event 分发 (event, detail) 二元组；
    // 摘除后回落 tracing 留痕路径（不炸）。extern fn 直调——真实路径经
    // vtable 传入插件（收信/发信/删信/地址变更全事件同管道）。
    let got = Arc::new(parking_lot::Mutex::new(Vec::<(String, String)>::new()));
    let sink_clone = got.clone();
    set_audit_sink(Arc::new(move |event: &str, detail: &str| {
        sink_clone
            .lock()
            .push((event.to_string(), detail.to_string()));
    }));
    let e = CString::new("mail.server_start").unwrap();
    let d = CString::new(r#"{"smtp_port":8025}"#).unwrap();
    host_audit_event(e.as_ptr(), d.as_ptr());
    let entries = got.lock();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "mail.server_start");
    assert!(entries[0].1.contains("8025"));
    drop(entries);

    // 摘除（None 路径 = tracing 留痕，不得炸）。
    *AUDIT_SINK.write() = None;
    host_audit_event(e.as_ptr(), d.as_ptr());
}
