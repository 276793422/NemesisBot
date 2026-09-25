//! verify_policy 单测：§1 矩阵逐行钉死（消费版 / 锁定版两种 feature 形态
//! 分别编译验证；锚存在性依赖构建环境，断言按 `ROOT_ANCHOR` 实际值分支）。

use super::*;

/// 未知值 loud 拒绝——两种形态、有无锚都一致。
#[test]
fn unknown_value_loud_reject() {
    for bad in ["yes", "OFF", "enforcement", "0"] {
        let err = resolve_mode(Some(bad)).expect_err("未知值必须 Err");
        assert!(err.contains("非法值"), "报错应指认非法值: {err}");
    }
}

/// 消费版 off → Off，永不降级、永不升级（有无锚均然）；锁定版忽略 config，
/// off 同样被钉成 Enforce（与 locked_ignores_config_values 一致）。
#[test]
fn off_stays_off() {
    let r = resolve_mode(Some("off")).expect("off 合法");
    if VERIFY_ENFORCE_LOCKED {
        assert_eq!(r.mode, VerifyMode::Enforce);
    } else {
        assert_eq!(r.mode, VerifyMode::Off);
        assert!(!r.degraded);
    }
}

/// 缺省（None / 空串）= warn 请求。
#[test]
fn absent_value_means_warn_request() {
    for v in [None, Some("")] {
        let r = resolve_mode(v).expect("缺省合法");
        if VERIFY_ENFORCE_LOCKED {
            assert_eq!(r.mode, VerifyMode::Enforce);
        } else if ROOT_ANCHOR.is_some() {
            assert_eq!(r.mode, VerifyMode::Warn);
        } else {
            assert_eq!(r.mode, VerifyMode::Off);
            assert!(r.degraded);
        }
    }
}

/// enforce 请求：有锚兑现 / 无锚降级 off（响亮声明由 self_check 负责）。
#[test]
fn enforce_depending_on_anchor() {
    let r = resolve_mode(Some("enforce")).expect("enforce 合法");
    if VERIFY_ENFORCE_LOCKED {
        assert_eq!(r.mode, VerifyMode::Enforce);
        assert!(r.locked);
    } else if ROOT_ANCHOR.is_some() {
        assert_eq!(r.mode, VerifyMode::Enforce);
        assert!(!r.locked);
    } else {
        assert_eq!(r.mode, VerifyMode::Off);
        assert!(r.degraded);
        assert!(!r.locked);
    }
}

/// warn 请求 + 无锚（消费版）→ 降级 off；有锚 → Warn。
#[test]
fn warn_depending_on_anchor() {
    let r = resolve_mode(Some("warn")).expect("warn 合法");
    if VERIFY_ENFORCE_LOCKED {
        assert_eq!(r.mode, VerifyMode::Enforce);
    } else if ROOT_ANCHOR.is_some() {
        assert_eq!(r.mode, VerifyMode::Warn);
        assert!(!r.degraded);
    } else {
        assert_eq!(r.mode, VerifyMode::Off);
        assert!(r.degraded);
    }
}

/// 退出码契约（计划决策 6）。
#[test]
fn exit_code_is_86() {
    assert_eq!(EXIT_ENFORCE_REJECTED, 86);
}

/// 锁定版形态专属：任意合法 config 值被忽略，恒 Enforce；无锚时
/// degraded=true（self_check 侧拒启 86——exit 不进单测）。
#[cfg(feature = "verify-enforce-lock")]
#[test]
fn locked_ignores_config_values() {
    for v in [None, Some(""), Some("off"), Some("warn"), Some("enforce")] {
        let r = resolve_mode(v).expect("合法值");
        assert!(r.locked, "锁定形态 locked=true");
        assert_eq!(r.mode, VerifyMode::Enforce, "锁定形态忽略 config");
        assert_eq!(r.degraded, ROOT_ANCHOR.is_none());
    }
}

/// 消费版形态专属：locked 恒 false。
#[cfg(not(feature = "verify-enforce-lock"))]
#[test]
fn consumer_form_never_locked() {
    for v in [None, Some("off"), Some("warn"), Some("enforce")] {
        let r = resolve_mode(v).expect("合法值");
        assert!(!r.locked);
    }
}

// ===========================================================================
// Coverage 追加（2026-09-24）：as_str / read_config_value / verify_current_exe /
// self_check_and_enforce 的进程内可达路径。
// read_config_value / self_check 走 resolve_home（NEMESISBOT_HOME env 进程
// 全局态）→ 持 GLOBAL_STATE_LOCK + EnvHomeGuard 隔离（Windows 形态，同
// commands/session::tests 先例）。
// ===========================================================================

/// VerifyMode::as_str 三态文案（WSAPI/审计输出契约）。
#[test]
fn verify_mode_as_str_matches_contract() {
    assert_eq!(VerifyMode::Off.as_str(), "off");
    assert_eq!(VerifyMode::Warn.as_str(), "warn");
    assert_eq!(VerifyMode::Enforce.as_str(), "enforce");
}

/// hex_encode 小写十六进制（Valid 摘要 key_fp 格式契约）。
#[test]
fn hex_encode_formats_lowercase() {
    assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    assert_eq!(hex_encode(&[0x00, 0x0f]), "000f");
    assert_eq!(hex_encode(&[]), "");
}

/// 安全配置文件路径辅助：把 config.security.json 写进临时 home 的
/// workspace/config/ 下（resolve_security_config_path_in_workspace 拼接点）。
/// 调用方必须已持 GLOBAL_STATE_LOCK 并用 EnvHomeGuard 指向该 home。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn write_security_config(home: &std::path::Path, raw: &str) {
    let path = crate::common::security_config_path(home);
    std::fs::create_dir_all(path.parent().expect("config path has parent")).unwrap();
    std::fs::write(path, raw).unwrap();
}

/// 进入 read_config_value 的隔离环境（锁 + env home），跑完自动恢复。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn with_isolated_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
    // 毒化容忍：别的测试持锁 panic 不许把这里放大成 140 连环假失败
    // （2026-09-25 全量跑实测一次：单 panic → 本模块 140 处 unwrap 连环炸）。
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);
    f(&home)
}

/// security 配置文件缺席 = 未配置（None），不炸不猜。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_absent_file_is_none() {
    with_isolated_home(|_home| {
        assert_eq!(read_config_value(false), None);
    });
}

/// JSON 解析失败：按未配置处理（None + 响亮提示），不 panic。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_invalid_json_is_none() {
    with_isolated_home(|home| {
        write_security_config(home, "{ not json at all");
        assert_eq!(read_config_value(false), None);
    });
}

/// 字符串值原样（trim 后）透传给裁决矩阵。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_string_passthrough_trimmed() {
    with_isolated_home(|home| {
        write_security_config(home, r#"{"signature_verify": "  enforce\n"}"#);
        assert_eq!(read_config_value(false), Some("enforce".to_string()));
    });
}

/// 空串 = 未配置（None）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_empty_string_is_none() {
    with_isolated_home(|home| {
        write_security_config(home, r#"{"signature_verify": "   "}"#);
        assert_eq!(read_config_value(false), None);
    });
}

/// null = 未配置（None）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_null_is_none() {
    with_isolated_home(|home| {
        write_security_config(home, r#"{"signature_verify": null}"#);
        assert_eq!(read_config_value(false), None);
    });
}

/// 非字符串（数字/对象）= 按未配置处理 + 响亮提示，不做二次裁决。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_non_string_is_none() {
    with_isolated_home(|home| {
        write_security_config(home, r#"{"signature_verify": 42}"#);
        assert_eq!(read_config_value(false), None);
        write_security_config(home, r#"{"signature_verify": {"mode":"off"}}"#);
        assert_eq!(read_config_value(false), None);
    });
}

/// 键缺席 = 未配置（None）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn read_config_value_missing_key_is_none() {
    with_isolated_home(|home| {
        write_security_config(home, r#"{"other_field": "off"}"#);
        assert_eq!(read_config_value(false), None);
    });
}

/// verify_current_exe：锚指纹非 hex → 装配层失败，诚实标 "Error"（非九态）。
#[test]
fn verify_current_exe_bad_anchor_hex_is_error_summary() {
    let summary = verify_current_exe("zzzz-not-hex-at-all-_______________zz");
    assert_eq!(summary.state, "Error");
    assert!(summary.key_fp.is_none());
    assert!(
        summary.detail.contains("锚指纹解码失败"),
        "got: {}",
        summary.detail
    );
}

/// 奇数长度 hex 同样走解码失败臂。
#[test]
fn verify_current_exe_odd_length_anchor_is_error_summary() {
    let summary = verify_current_exe("abc");
    assert_eq!(summary.state, "Error");
}

/// 合法 64-hex 锚 + 未签名测试 exe → 九态 NoSignature（开发构建诚实态）。
#[test]
fn verify_current_exe_unsigned_test_binary_is_no_signature() {
    let anchor = "aa".repeat(32);
    let summary = verify_current_exe(&anchor);
    assert_eq!(summary.state, "NoSignature", "detail: {}", summary.detail);
    assert!(summary.key_fp.is_none());
    assert!(!summary.detail.is_empty());
}

/// self_check：START 是进程级 OnceLock（set-once）——两阶段在**同一个**
/// 测试内先后跑，避免并行测试竞速静默丢快照：
/// ① warn（锚存在 → 真跑自验，未签名测试 exe → 非 Valid，不 exit）；
/// ② off（跳过验签；快照保持首响——set 失败静默是设计语义）。
/// 无锚构建形态：warn 降级 off（outcome=None），两形态都不得 exit。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn self_check_snapshots_first_call_and_never_exits() {
    with_isolated_home(|home| {
        write_security_config(home, r#"{"signature_verify": "warn"}"#);
        self_check_and_enforce(false); // 不得 exit、不得 panic
        let snap = start_check().expect("首响必落快照");
        if ROOT_ANCHOR.is_some() {
            assert_eq!(snap.mode, VerifyMode::Warn);
            assert!(!snap.degraded);
            let outcome = snap.outcome.as_ref().expect("warn + 锚 = 必跑自验");
            assert_ne!(outcome.state, "", "九态名非空");

            // 第二响 off：跳过验签（不验签不 exit）；OnceLock 保持首响。
            write_security_config(home, r#"{"signature_verify": "off"}"#);
            self_check_and_enforce(false);
            let snap2 = start_check().unwrap();
            assert_eq!(
                snap2.mode,
                VerifyMode::Warn,
                "OnceLock set-once：快照保持首响"
            );
        } else {
            assert_eq!(snap.mode, VerifyMode::Off);
            assert!(snap.degraded);
            assert!(snap.outcome.is_none(), "降级 off 不验签");
        }
    });
}

/// anchor_from_env 三臂运行时真执行（const 常量求值不落行覆盖，运行时
/// 调用一次把 match 全臂钉进覆盖；私有 fn 由子 mod tests 直达）。
#[test]
fn anchor_from_env_runtime_arms() {
    assert_eq!(anchor_from_env(None), None);
    assert_eq!(anchor_from_env(Some("")), None);
    assert_eq!(anchor_from_env(Some("deadbeef")), Some("deadbeef"));
}

// ===========================================================================
// Wave5 round2（2026-09-25）：self_check_and_enforce 的两个 exit(86) 硬拒启
// 臂——进程级副作用（std::process::exit）无法在本进程内断言，走「测试二进制
// 自孵子进程」标准形态：父测以 --exact 过滤重入自身，子测凭
// NEMESISBOT_W5_VERIFY_CHILD 环境值进入真执行臂；home 指向临时目录
//（NEMESISBOT_HOME 优先级 2，绝不触碰真实 ~/.nemesisbot）。
// 无 env 值时子测立即空转返回——全套件并行跑无副作用。
// ===========================================================================

/// 子进程臂①：config 非法值 → resolve_mode Err → loud 拒绝 exit 86。
#[test]
fn w5_verify_child_bogus_config_exits() {
    if std::env::var("NEMESISBOT_W5_VERIFY_CHILD").as_deref() != Ok("bogus") {
        return; // 常规套件内空转（无 env = 父进程形态，什么都不做）
    }
    self_check_and_enforce(false);
    panic!("非法 signature_verify 值必须 exit 86（走到这里 = 拒启臂未触发）");
}

/// 子进程臂②：enforce + 未签名测试 exe → 验证失败 → 拒启 exit 86
///（覆盖 action=「拒绝启动」字面量 + 终局 exit）。
#[test]
fn w5_verify_child_enforce_unsigned_exits() {
    if std::env::var("NEMESISBOT_W5_VERIFY_CHILD").as_deref() != Ok("enforce") {
        return;
    }
    self_check_and_enforce(false);
    panic!("enforce + 未签名 exe 必须 exit 86（走到这里 = 拒启臂未触发）");
}

/// 父侧驱动：临时 home 写 security 配置 → 自孵测试二进制跑指定子测 →
/// 返回子进程退出码（home 用后即焚，无环境残留）。
fn w5_run_verify_child(child_test: &str, arm: &str, security_json: &str) -> i32 {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let sec = crate::common::security_config_path(&home);
    std::fs::create_dir_all(sec.parent().expect("config path has parent")).unwrap();
    std::fs::write(sec, security_json).unwrap();
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .args([child_test, "--exact", "--nocapture"])
        .env("NEMESISBOT_W5_VERIFY_CHILD", arm)
        .env("NEMESISBOT_HOME", tmp.path())
        .output()
        .expect("spawn child test binary");
    out.status
        .code()
        .expect("子进程应带码退出（exit 86 而非信号）")
}

/// 拒启路径①：config 非法值 → exit 86（计划 §1 loud 拒绝）。
#[test]
fn w5_self_check_bogus_value_exits_86() {
    let code = w5_run_verify_child(
        "verify_policy::tests::w5_verify_child_bogus_config_exits",
        "bogus",
        r#"{"signature_verify": "bogus-value"}"#,
    );
    assert_eq!(code, 86, "非法值必须 loud 拒启 exit 86");
}

/// 拒启路径②：enforce 模式 + 未签名 exe（测试二进制即未签名 PE）→
/// exit 86。
#[test]
fn w5_self_check_enforce_unsigned_exits_86() {
    let code = w5_run_verify_child(
        "verify_policy::tests::w5_verify_child_enforce_unsigned_exits",
        "enforce",
        r#"{"signature_verify": "enforce"}"#,
    );
    assert_eq!(code, 86, "enforce 验签失败必须拒启 exit 86");
}
