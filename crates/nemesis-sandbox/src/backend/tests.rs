//! backend 模块测试：跨平台纯函数测试（所有平台编译）+ Linux 真实
//! landlock/bwrap 测试（cfg(linux)，真机行为断言；环境缺能力时跳过并
//! eprintln 原因——不假装通过）。

use super::*;

// ---------------------------------------------------------------------------
// 跨平台：纯函数与 trait 契约
// ---------------------------------------------------------------------------

#[test]
fn bwrap_args_basic_shape() {
    let conf = SandboxConf {
        writable_roots: vec![std::path::PathBuf::from("/ws")],
        read_exec_roots: vec![std::path::PathBuf::from("/")],
        allow_network: true,
        label: "test".into(),
    };
    let args = bwrap_args(&conf);
    // 基础骨架（ro-bind 全盘 + dev/proc/tmpfs + 生命周期）
    let joined = args.join("\u{1}");
    for needle in [
        "--ro-bind\u{1}/\u{1}/",
        "--dev\u{1}/dev",
        "--proc\u{1}/proc",
        "--tmpfs\u{1}/tmp",
        "--die-with-parent",
        "--unshare-pid",
    ] {
        assert!(
            joined.contains(needle),
            "bwrap_args missing base segment {needle:?}: {args:?}"
        );
    }
    // workspace 读写挂载存在，且在 ro-bind / 之后（后项覆盖 = 该子树可写）
    let ro_pos = joined.find("--ro-bind\u{1}/\u{1}/").unwrap();
    let bind_pos = joined.find("--bind\u{1}/ws\u{1}/ws").unwrap();
    assert!(bind_pos > ro_pos, "workspace --bind must override --ro-bind /: {args:?}");
    // allow_network=true → 不加 --unshare-net
    assert!(!args.contains(&"--unshare-net".to_string()));
}

#[test]
fn bwrap_args_skips_duplicate_root_ro_bind() {
    let conf = SandboxConf {
        writable_roots: vec![],
        read_exec_roots: vec![
            std::path::PathBuf::from("/"),
            std::path::PathBuf::from("/usr"),
        ],
        allow_network: true,
        label: "test".into(),
    };
    let args = bwrap_args(&conf);
    // "/" 已有基础 ro-bind，不应重复；非根的 /usr 保留
    let ro_root_count = args
        .windows(3)
        .filter(|w| *w == ["--ro-bind", "/", "/"])
        .count();
    assert_eq!(ro_root_count, 1, "duplicate ro-bind / : {args:?}");
    let joined = args.join("\u{1}");
    assert!(joined.contains("--ro-bind\u{1}/usr\u{1}/usr"));
}

#[test]
fn bwrap_args_unshare_net_only_when_denied() {
    let conf = SandboxConf {
        writable_roots: vec![std::path::PathBuf::from("/ws")],
        read_exec_roots: vec![std::path::PathBuf::from("/")],
        allow_network: false,
        label: "test".into(),
    };
    let args = bwrap_args(&conf);
    assert!(args.contains(&"--unshare-net".to_string()));
}

#[test]
fn seatbelt_profile_shape() {
    let conf = SandboxConf {
        writable_roots: vec![
            std::path::PathBuf::from("/Users/x/ws"),
            std::path::PathBuf::from("/Users/x/ws2"),
        ],
        read_exec_roots: vec![std::path::PathBuf::from("/")],
        allow_network: false,
        label: "seatbelt-test".into(),
    };
    let profile = seatbelt_profile(&conf);
    assert!(profile.starts_with("(version 1)\n"), "profile head: {profile}");
    assert!(profile.contains("(deny default)"));
    assert!(profile.contains("(allow process-exec*)"));
    assert!(profile.contains("(allow process-fork)"));
    assert!(profile.contains("(allow file-read*)"));
    // 每个 writable root 一条 subpath literal 放行写
    assert!(profile.contains("(allow file-write* (subpath (literal \"/Users/x/ws\")))"));
    assert!(profile.contains("(allow file-write* (subpath (literal \"/Users/x/ws2\")))"));
    assert!(profile.contains("(deny network*)"));
    assert!(profile.contains("label: seatbelt-test"));
}

#[test]
fn seatbelt_profile_no_network_deny_when_allowed() {
    let conf = SandboxConf {
        writable_roots: vec![std::path::PathBuf::from("/ws")],
        read_exec_roots: vec![std::path::PathBuf::from("/")],
        allow_network: true,
        label: "test".into(),
    };
    let profile = seatbelt_profile(&conf);
    assert!(!profile.contains("(deny network*)"));
}

#[test]
fn for_executor_conf_shape() {
    let conf = SandboxConf::for_executor(std::path::Path::new("/home/bot/ws"), false);
    assert_eq!(conf.writable_roots, vec![std::path::PathBuf::from("/home/bot/ws")]);
    assert_eq!(conf.read_exec_roots, vec![std::path::PathBuf::from("/")]);
    assert!(!conf.allow_network);
    assert_eq!(conf.label, "executor");
}

/// trait 默认方法契约：不支持的能力返回 Err 且报后端名。
#[test]
fn trait_default_methods_reject_unsupported_forms() {
    struct DummyBackend;
    impl SandboxBackend for DummyBackend {
        fn name(&self) -> &str {
            "dummy"
        }
        fn form(&self) -> super::BackendForm {
            super::BackendForm::SelfApply
        }
        fn availability(&self) -> Availability {
            Availability::Full
        }
    }
    let b = DummyBackend;
    let conf = SandboxConf::for_executor(std::path::Path::new("/ws"), false);
    let err = b.apply_to_self(&conf).unwrap_err();
    assert!(err.contains("dummy"), "err names backend: {err}");
    let err = b
        .wrap_command(&conf, &std::process::Command::new("true"))
        .unwrap_err();
    assert!(err.contains("dummy"), "err names backend: {err}");
}

/// Windows 设计契约：不注册任何用户态后端（Sandboxie 承担，U11「Windows 不动」）。
#[cfg(target_os = "windows")]
#[test]
fn detect_backend_none_on_windows() {
    assert!(detect_backend().is_none());
}

// ---------------------------------------------------------------------------
// P5：read_executor_strict / probe_userland_backends
// ---------------------------------------------------------------------------

fn write_home_config(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("config.json"), body).expect("seed config.json");
}

#[test]
fn read_executor_strict_defaults_false_and_reads_true() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 无 config.json → false（fail-open 现状）
    assert!(!read_executor_strict(dir.path()));
    // 缺 executor 段 / 缺 strict 键 → false
    write_home_config(dir.path(), r#"{ "executor": { "sandbox": true } }"#);
    assert!(!read_executor_strict(dir.path()));
    // 显式 true → true
    write_home_config(
        dir.path(),
        r#"{ "executor": { "enabled": true, "sandbox": true, "strict": true } }"#,
    );
    assert!(read_executor_strict(dir.path()));
    // 损坏 JSON → false（不 panic）
    write_home_config(dir.path(), "{ not json");
    assert!(!read_executor_strict(dir.path()));
}

/// 逐后端探测：Windows 空（Sandboxie 承担）；Linux/macOS 至少列出本平台后端
/// 且名字唯一。结构断言不依赖机器能力（缺 bwrap 的内核也合法返回 Unavailable）。
#[test]
fn probe_userland_backends_shape_per_platform() {
    let probes = probe_userland_backends();
    if cfg!(target_os = "windows") {
        assert!(probes.is_empty(), "Windows 不注册用户态后端: {probes:?}");
        return;
    }
    assert!(!probes.is_empty(), "Linux/macOS 至少一个用户态后端");
    let mut names: Vec<_> = probes.iter().map(|p| p.name.clone()).collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total, "后端名重复: {probes:?}");
    for p in &probes {
        assert!(!p.name.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Linux：真实 landlock / bwrap 行为（真机=WSL2 Ubuntu，报告如实标注环境）
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux_live {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// 带超时的同步执行（防挂死测试套件）。
    fn run_with_timeout(cmd: &mut Command, secs: u64) -> std::io::Result<std::process::Output> {
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            match child.try_wait()? {
                Some(_status) => break,
                None => {
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let out = child.wait_with_output()?;
                        return Ok(std::process::Output {
                            status: out.status,
                            stdout: out.stdout,
                            stderr: out.stderr,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        child.wait_with_output()
    }

    #[test]
    fn landlock_availability_probe() {
        let backend = crate::backend::landlock_impl::LandlockBackend::new();
        match backend.availability() {
            Availability::Full => {}
            other => eprintln!("SKIP landlock live tests: {other:?}"),
        }
    }

    /// B7 核心验收：landlock 装上后写工作区外拒 / 写内放行。
    ///
    /// landlock 自装不可逆且只约束**本进程树**，不能在测试进程里直接装
    /// （会污染同进程的其他测试）——用 self-exec 子进程模式：spawn 自身
    /// test binary、--exact 指定子测试、经 env 传路径；子进程装上→探测写→
    /// 打印标记→exit 0（子进程内永不失败，父进程解析标记断言）。
    #[test]
    fn landlock_denies_write_outside_allows_inside() {
        let backend = crate::backend::landlock_impl::LandlockBackend::new();
        if !matches!(backend.availability(), Availability::Full) {
            eprintln!("SKIP: landlock unavailable on this kernel");
            return;
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let inside = tmp.path().join("inside");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let exe = std::env::current_exe().unwrap();
        let output = run_with_timeout(
            Command::new(&exe)
                // 子串过滤（不带 --exact：libtest 的 --exact 要求完整路径
                // `backend::tests::linux_live::landlock_child_apply`，短名会
                // 滤成 0 个测试）。该子串在本二进制内唯一。
                .args(["landlock_child_apply", "--nocapture", "--test-threads=1"])
                .env("NEMESIS_TEST_LL_INSIDE", &inside)
                .env("NEMESIS_TEST_LL_OUTSIDE", &outside)
                .env("NEMESIS_TEST_LL_CHILD", "1"),
            60,
        )
        .expect("spawn child");

        assert!(
            output.status.success(),
            "child exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("INSIDE_WRITE_OK"),
            "inside write did not succeed; child stdout:\n{stdout}"
        );
        assert!(
            stdout.contains("OUTSIDE_WRITE_DENIED"),
            "outside write was NOT denied; child stdout:\n{stdout}"
        );
        assert!(
            stdout.contains("ENFORCEMENT_PARTIAL_NETWORK_GAP") || stdout.contains("ENFORCEMENT_FULL"),
            "child did not report enforcement level; stdout:\n{stdout}"
        );
        // 「写内放行」的真凭实据：宿主侧文件确实写成
        let marker = std::fs::read_to_string(inside.join("ok.txt")).unwrap_or_default();
        assert_eq!(marker.trim(), "inside-ok");
    }

    /// 子进程侧：由父测试 self-exec 进入（见上）。装 landlock（writable 仅
    /// inside、全盘读、禁网）→ 两个写探针 → 打印标记。永不 assert 失败。
    #[test]
    fn landlock_child_apply() {
        // 必须从父测试带 env 进来；直接被 cargo test 跑到（无 env）时静默通过，
        // 真正的行为断言在父进程侧。
        let (Ok(inside), Ok(outside)) = (
            std::env::var("NEMESIS_TEST_LL_INSIDE"),
            std::env::var("NEMESIS_TEST_LL_OUTSIDE"),
        ) else {
            return;
        };
        let inside = std::path::PathBuf::from(inside);
        let outside = std::path::PathBuf::from(outside);

        let backend = crate::backend::landlock_impl::LandlockBackend::new();
        // for_executor 语义：workspace 可写、全盘读、禁网（禁网在 FS-only
        // 后端 → Partial 缺口，见模块文档）。
        let conf = SandboxConf::for_executor(&inside, false);
        match backend.apply_to_self(&conf) {
            Ok(Enforcement::Partial(gaps)) => {
                let net_gap = gaps.iter().any(|g| g.contains("network"));
                println!(
                    "ENFORCEMENT_PARTIAL_NETWORK_GAP={}",
                    net_gap
                );
            }
            Ok(Enforcement::Full) => println!("ENFORCEMENT_FULL"),
            Err(err) => {
                println!("ENFORCEMENT_ERROR:{err}");
                return;
            }
        }

        // 装上之后（同一线程上——landlock 只约束调用线程及其后代）做写探针。
        match std::fs::write(inside.join("ok.txt"), "inside-ok\n") {
            Ok(()) => println!("INSIDE_WRITE_OK"),
            Err(err) => println!("INSIDE_WRITE_FAILED:{err}"),
        }
        match std::fs::write(outside.join("bad.txt"), "should-be-denied\n") {
            Ok(()) => println!("OUTSIDE_WRITE_ALLOWED_BUG"),
            Err(err) => println!("OUTSIDE_WRITE_DENIED:{err}"),
        }
    }

    /// bwrap 可用性（Ubuntu 24.04 自带 /usr/bin/bwrap；缺失时跳过并注明）。
    #[test]
    fn bwrap_availability_probe() {
        let backend = crate::backend::bwrap_impl::BwrapBackend::new();
        match backend.availability() {
            Availability::Full => eprintln!("bwrap found at {:?}", backend.path()),
            other => eprintln!("SKIP bwrap live tests: {other:?}"),
        }
    }

    /// bwrap 写外拒 / 写内放行（宿主文件真落盘验证写穿）。
    /// 「外」= /etc 下（namespace 内只读 bind）；「内」= workspace bind。
    #[test]
    fn bwrap_denies_write_outside_allows_inside() {
        let backend = crate::backend::bwrap_impl::BwrapBackend::new();
        if !matches!(backend.availability(), Availability::Full) {
            eprintln!("SKIP: bwrap not installed");
            return;
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let inside = tmp.path().join("inside");
        std::fs::create_dir_all(&inside).unwrap();

        let conf = SandboxConf::for_executor(&inside, false);

        // 写内：exit 0 + 宿主文件落盘
        let mut ok_cmd = Command::new("/bin/sh");
        ok_cmd
            .arg("-c")
            .arg(format!("echo bwrap-ok > {}", inside.join("ok.txt").display()));
        let mut wrapped = backend.wrap_command(&conf, &ok_cmd).expect("wrap");
        let out = run_with_timeout(&mut wrapped, 60).expect("run inside-write");
        assert!(
            out.status.success(),
            "bwrap inside write failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let marker = std::fs::read_to_string(inside.join("ok.txt")).unwrap_or_default();
        assert_eq!(marker.trim(), "bwrap-ok", "write-through to host failed");

        // 写外（/etc 只读 bind）：必须失败
        let mut bad_cmd = Command::new("/bin/sh");
        bad_cmd
            .arg("-c")
            .arg("echo bad > /etc/nemesis_u11_probe_should_fail");
        let mut wrapped = backend.wrap_command(&conf, &bad_cmd).expect("wrap");
        let out = run_with_timeout(&mut wrapped, 60).expect("run outside-write");
        assert!(
            !out.status.success(),
            "bwrap outside write was ALLOWED (bug!): {}",
            String::from_utf8_lossy(&out.stdout)
        );
        // 宿主侧同样不应存在
        assert!(
            !std::path::Path::new("/etc/nemesis_u11_probe_should_fail").exists(),
            "outside write leaked to host"
        );
    }

    /// bwrap --unshare-net 真禁网（landlock 做不到的那半边）。
    #[test]
    fn bwrap_unshare_net_blocks_connect() {
        let backend = crate::backend::bwrap_impl::BwrapBackend::new();
        if !matches!(backend.availability(), Availability::Full) {
            eprintln!("SKIP: bwrap not installed");
            return;
        }
        if !std::path::Path::new("/bin/bash").exists() {
            eprintln!("SKIP: /bin/bash not present for /dev/tcp probe");
            return;
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let conf = SandboxConf::for_executor(tmp.path(), false);
        // bash /dev/tcp 探针：无网 namespace 里 connect 立即失败
        let mut probe = Command::new("/bin/bash");
        probe.arg("-c").arg(
            "exec 3<>/dev/tcp/203.0.113.1/80 && echo NET_CONNECTED && exec 3>&-",
        );
        let mut wrapped = backend.wrap_command(&conf, &probe).expect("wrap");
        let out = run_with_timeout(&mut wrapped, 30).expect("run net probe");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("NET_CONNECTED"),
            "network connection succeeded inside --unshare-net sandbox (bug!)"
        );
        assert!(
            !out.status.success(),
            "net probe should fail with non-zero exit, got success"
        );
    }
}
