//! 运营链测试。
//!
//! 纪律：spawn 契约用例（SystemCall 构造面 + 假执行器生命周期）**跨平台
//! 可跑**（本机 Windows 也进）；带 `#[cfg(target_os = "linux")]` 的 IO/进程
//! 用例只在 Linux 编译执行（远程盒/CI 诚实覆盖——Windows 上静默缺席是本
//! crate 平台边界的显式形态）。系统集成实现住 bot（nemesis-sysint）——本
//! 文件不写真实用户路径，一律假执行器。

use super::*;
use anyhow::anyhow;
use setup_core::install::{InstallPlan, TypedFail};

// ---------- 系统集成 spawn 契约（任意主机可跑） ----------

#[test]
fn system_call_shape_matches_bot_cli_flags() {
    let target = Path::new("/opt/NemesisBot");
    let opts = InstallOptions {
        package: target.join("pkg"),
        target_dir: target.to_path_buf(),
        accept_risk: false,
        force_downgrade: false,
        no_shortcuts: false,
        autostart: true,
        launch: false,
    };
    let call = register_call(&target.join("nemesisbot"), target, &opts);
    assert_eq!(call.exe, target.join("nemesisbot"));
    assert!(
        call.args
            .starts_with(&["system".to_string(), "register".to_string()]),
        "子命令前缀: {:?}",
        call.args
    );
    assert!(call.args.contains(&"--autostart".to_string()));
    let di = call
        .args
        .iter()
        .position(|a| a == "--dir")
        .expect("--dir 在场");
    assert_eq!(call.args[di + 1], target.to_string_lossy());

    // 关闭态旗标缺席（结构体更新：opts 移入）。
    let opts2 = InstallOptions {
        autostart: false,
        no_shortcuts: true,
        ..opts
    };
    let call2 = register_call(&target.join("nemesisbot"), target, &opts2);
    assert!(!call2.args.contains(&"--autostart".to_string()));
    assert!(call2.args.contains(&"--no-shortcuts".to_string()));

    // 注销调用形状。
    let un = unregister_call(&target.join("nemesisbot"), target);
    assert_eq!(
        un.args,
        vec![
            "system".to_string(),
            "unregister".to_string(),
            "--dir".to_string(),
            target.to_string_lossy().into_owned(),
        ]
    );
}

/// 合成成功 Output（测试执行器用；跨平台裸 ExitStatus::from_raw）。
#[allow(unused)]
fn ok_output() -> std::process::Output {
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

/// 合成非零退出 Output（携带 stderr 尾巴）。
#[allow(unused)]
fn fail_output(stderr: &str) -> std::process::Output {
    let mut out = ok_output();
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        out.status = std::process::ExitStatus::from_raw(1);
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::ExitStatusExt;
        out.status = std::process::ExitStatus::from_raw(1);
    }
    out.stderr = stderr.as_bytes().to_vec();
    out
}

#[test]
fn plan_text_matrix() {
    assert_eq!(plan_text(&InstallPlan::Fresh), "全新安装");
    assert_eq!(
        plan_text(&InstallPlan::Repair("1.0.0".into())),
        "同版本修复重装（当前 1.0.0）"
    );
    assert_eq!(
        plan_text(&InstallPlan::Upgrade("1.0.0".into())),
        "覆盖升级（当前 1.0.0）"
    );
    assert!(
        plan_text(&InstallPlan::Downgrade {
            current: "2.0.0".into(),
            incoming: "1.0.0".into(),
        })
        .contains("--force")
    );
}

#[test]
fn required_disk_bytes_adds_margin() {
    assert_eq!(required_disk_bytes(0), 0);
    assert_eq!(required_disk_bytes(100), 120);
    assert_eq!(required_disk_bytes(1_000_000), 1_200_000);
}

#[test]
fn exit_code_maps_typed_and_ops_failures() {
    let integrity: anyhow::Error = TypedFail::Integrity("x".into()).into();
    assert_eq!(exit_code_of(&integrity), 2);
    let risk: anyhow::Error = TypedFail::SigRisk("x".into()).into();
    assert_eq!(exit_code_of(&risk), 5);
    let downgrade: anyhow::Error = TypedFail::DowngradeRejected {
        current: "2".into(),
        incoming: "1".into(),
    }
    .into();
    assert_eq!(exit_code_of(&downgrade), 1);
    let running: anyhow::Error = OpsFail::Running(vec!["123".into()]).into();
    assert_eq!(exit_code_of(&running), 6);
    // 确认卸载强停失败（存活）同属「bot 正在运行」码域。
    let survive: anyhow::Error =
        setup_core::procs::BotsSurvive(vec![setup_core::procs::BotProcess {
            pid: 7,
            exe_path: String::new(),
        }])
        .into();
    assert_eq!(exit_code_of(&survive), 6);
    let disk: anyhow::Error = OpsFail::DiskSpace { needed: 1, free: 0 }.into();
    assert_eq!(exit_code_of(&disk), 4);
    let cancelled: anyhow::Error = OpsFail::Cancelled.into();
    assert_eq!(exit_code_of(&cancelled), 3);
    let generic: anyhow::Error = anyhow!("任意失败");
    assert_eq!(exit_code_of(&generic), 1);
}

// ---------- 假程序模拟（进程清单解析层，跨平台可跑） ----------

#[test]
fn parse_pgrep_filters_fake_pid_list() {
    // pgrep -x 输出 = 裸 PID 行。
    let got = parse_pgrep_output("4242\n 5151 \n\n9999\n");
    assert_eq!(
        got,
        vec!["4242", "5151", "9999"],
        "空行滤除 + trim: {got:?}"
    );
    assert!(parse_pgrep_output("").is_empty());
}

// ---------- Linux IO/进程用例（只在 Linux 编译执行） ----------

#[cfg(target_os = "linux")]
mod linux_io {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    #[test]
    fn free_disk_probes_existing_ancestor() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let free = free_disk_bytes(tmp.path())?;
        let deep = tmp.path().join("a/b/c");
        let free2 = free_disk_bytes(&deep)?;
        assert!(free > 0);
        // 同卷祖先查询应同一剩余量——容差比较：两次 statvfs 之间盘面可以合法
        // 变动（常驻进程写日志、别的进程建删文件），逐字节相等是环境敏感假红
        // （2026-11-10 远程盒实录：8KB 漂移）。同卷语义 = 差异远小于卷容量。
        let drift = free.abs_diff(free2);
        assert!(
            drift <= 64 * 1024 * 1024,
            "同卷祖先查询剩余量漂移异常（{drift} 字节）: {free} vs {free2}"
        );
        // 反证：不存在的心卷路径语义由调用方回落祖先承担——这里只钉
        // 「未创建目录也能查」（实现走既有祖先探测）。
        Ok(())
    }

    #[test]
    fn running_bots_returns_shape_not_emptiness() {
        let list = running_bots().expect("pgrep 应可执行");
        for pid in &list {
            assert!(
                pid.chars().all(|c| c.is_ascii_digit()),
                "pgrep 输出应为 PID 数字: {pid}"
            );
        }
    }

    /// 假载荷打假包（假 nemesisbot 字节；引擎对骨架只搬运不执行）。
    fn build_fake_package(dir: &Path, version: &str) -> anyhow::Result<std::path::PathBuf> {
        let payload = dir.join("payload");
        std::fs::create_dir_all(payload.join("share"))?;
        std::fs::write(payload.join("nemesisbot"), b"FAKE-BOT-BYTES")?;
        std::fs::write(payload.join("share/icon.png"), b"png")?;
        let skeleton = dir.join("skl.bin");
        std::fs::write(&skeleton, b"SKEL")?;
        let out = dir.join("sim.pkg");
        setup_core::pack::run(&setup_core::pack::PackOptions {
            payload_dir: payload,
            skeleton,
            out: out.clone(),
            bot_version: version.into(),
            platform: "linux".into(),
            sign_keys: None,
            zstd_level: 1,
        })?;
        Ok(out)
    }

    #[test]
    fn simulated_full_lifecycle_with_fake_pid_list() -> anyhow::Result<()> {
        use setup_core::pkgfile::Progress;

        let tmp = tempfile::tempdir()?;
        let package = build_fake_package(tmp.path(), "9.9.9")?;
        let target = tmp.path().join("root");
        let opts = InstallOptions {
            package: package.clone(),
            target_dir: target.clone(),
            accept_risk: false,
            force_downgrade: false,
            no_shortcuts: true,
            autostart: false,
            launch: false, // 假 bot 不真启动
        };
        let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
        let fake_bots = || -> Result<Vec<String>> { Ok(vec!["4242".into(), "5151".into()]) };

        // 假系统集成执行器：记录调用、恒成功（不 spawn 真 bot）。
        let calls: Mutex<Vec<SystemCall>> = Mutex::new(Vec::new());
        let system = |call: &SystemCall| -> Result<std::process::Output> {
            calls.lock().expect("lock").push(call.clone());
            Ok(ok_output())
        };

        // ① 模拟「无实例」→ 全新安装成功 + 单次 register spawn。
        let out = run_install_with(&opts, None, &no_bots, &system)?;
        assert_eq!(out.bot_version, "9.9.9");
        assert!(out.plan_text.contains("全新安装"), "{}", out.plan_text);
        assert_eq!(
            out.files_installed, 4,
            "2 载荷文件 + 卸载副本 + install.json（计数契约见 core tests）"
        );
        assert!(out.notes.is_empty(), "{:?}", out.notes);
        assert!(target.join("nemesisbot").exists());
        assert!(target.join("share/icon.png").exists());
        assert!(target.join("uninstall").exists(), "卸载副本落位");
        assert!(target.join("install.json").exists());
        {
            let got = calls.lock().expect("lock");
            assert_eq!(got.len(), 1, "安装只 spawn 一次 register: {got:?}");
            assert_eq!(got[0].args[1], "register");
            let di = got[0]
                .args
                .iter()
                .position(|a| a == "--dir")
                .expect("--dir");
            assert_eq!(got[0].args[di + 1], target.to_string_lossy());
        }

        // ② 模拟「有实例」（假 PID 清单）→ 闸拦 exit 6，目录一字节不动、零调用。
        let before = std::fs::read(target.join("install.json"))?;
        let err = run_install_with(
            &opts,
            None as Option<&dyn Fn(Progress)>,
            &fake_bots,
            &system,
        )
        .expect_err("假实例清单必须被闸拦");
        assert_eq!(exit_code_of(&err), 6);
        assert_eq!(before, std::fs::read(target.join("install.json"))?);
        assert_eq!(calls.lock().expect("lock").len(), 1, "闸拦路径不 spawn");

        // ③ 模拟「无实例」→ 卸载干净（linux 形态显式摘卸载副本 + 剪空目录
        //    壳）。假注销执行器断言**顺序契约**：
        //    注销时 install.json（bot 账本）还在 = 先 unregister 后删文件。
        let un_calls = std::sync::Arc::new(Mutex::new(Vec::<SystemCall>::new()));
        let un_calls_in = std::sync::Arc::clone(&un_calls);
        let order_probe = target.clone();
        let un_runner = move |call: &SystemCall| -> Result<std::process::Output> {
            assert!(
                order_probe.join("install.json").exists(),
                "system unregister 必须先于引擎删除执行（bot 已落位才可注销）"
            );
            un_calls_in.lock().expect("lock").push(call.clone());
            Ok(ok_output())
        };
        let un = run_uninstall_with(&target, &no_bots, &un_runner)?;
        assert_eq!(un.deleted, 2, "record.files = 2 个载荷文件");
        assert!(un.notes.is_empty(), "{:?}", un.notes);
        assert!(!target.join("install.json").exists(), "标记摘除");
        assert!(!target.join("nemesisbot").exists());
        assert!(!target.join("share").exists(), "空目录剪除");
        assert!(!target.join("uninstall").exists(), "副本显式摘除");
        assert!(
            !target.exists(),
            "副本摘除后空目录壳一并剪除（2026-11-10 E2E 实录回归锁）"
        );
        {
            let got = un_calls.lock().expect("lock");
            assert_eq!(got.len(), 1, "卸载只 spawn 一次 unregister: {got:?}");
            assert_eq!(got[0].args[1], "unregister");
        }

        // ④ 模拟「有实例」→ 卸载同样被拦（闸先于 spawn 与 install.json 缺失检查）。
        let err =
            run_uninstall_with(&target, &fake_bots, &un_runner).expect_err("卸载也要过实例闸");
        assert_eq!(exit_code_of(&err), 6);
        Ok(())
    }

    // ---------- 确认卸载链（路径级强停，2026-11-10 用户拍板） ----------

    #[test]
    fn confirmed_uninstall_stops_then_unregisters_in_order() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let package = build_fake_package(tmp.path(), "9.9.9")?;
        let target = tmp.path().join("root");
        let opts = InstallOptions {
            package: package.clone(),
            target_dir: target.clone(),
            accept_risk: false,
            force_downgrade: false,
            no_shortcuts: true,
            autostart: false,
            launch: false,
        };
        let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
        let system = |_: &SystemCall| -> Result<std::process::Output> { Ok(ok_output()) };
        run_install_with(&opts, None, &no_bots, &system)?;

        // 假强停：记录调用时序 + 返回 1 个「被停止」实例。
        let seq: std::sync::Arc<Mutex<Vec<&'static str>>> =
            std::sync::Arc::new(Mutex::new(Vec::new()));
        let seq_stop = std::sync::Arc::clone(&seq);
        let probe = target.clone();
        let stop = move || -> Result<Vec<setup_core::procs::BotProcess>> {
            assert!(
                probe.join("install.json").exists(),
                "强停必须先于引擎删除（停止时账本应在场）"
            );
            seq_stop.lock().expect("lock").push("stop");
            Ok(vec![setup_core::procs::BotProcess {
                pid: 4242,
                exe_path: probe.join("nemesisbot").to_string_lossy().into_owned(),
            }])
        };
        let seq_sys = std::sync::Arc::clone(&seq);
        let sys_probe = target.clone();
        let runner = move |_call: &SystemCall| -> Result<std::process::Output> {
            assert!(
                sys_probe.join("install.json").exists(),
                "unregister 顺序契约保持"
            );
            seq_sys.lock().expect("lock").push("unregister");
            Ok(ok_output())
        };

        let out = run_uninstall_confirmed_with(&target, &stop, &runner)?;
        assert_eq!(out.bots_stopped, 1, "强停计数进结果");
        assert_eq!(
            *seq.lock().expect("lock"),
            vec!["stop", "unregister"],
            "强停先于注销"
        );
        assert!(!target.exists(), "卸载完成目录剪除");
        Ok(())
    }

    #[test]
    fn confirmed_uninstall_survivors_block_before_unregister() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let package = build_fake_package(tmp.path(), "9.9.9")?;
        let target = tmp.path().join("root");
        let opts = InstallOptions {
            package: package.clone(),
            target_dir: target.clone(),
            accept_risk: false,
            force_downgrade: false,
            no_shortcuts: true,
            autostart: false,
            launch: false,
        };
        let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
        let system = |_: &SystemCall| -> Result<std::process::Output> { Ok(ok_output()) };
        run_install_with(&opts, None, &no_bots, &system)?;
        let record = std::fs::read(target.join("install.json"))?;

        // 假强停失败（存活）→ 整链失败：不注销、不删文件、exit 6。
        let never = |call: &SystemCall| -> Result<std::process::Output> {
            panic!("存活失败路径不应推进到注销: {call}")
        };
        let stop = || -> Result<Vec<setup_core::procs::BotProcess>> {
            Err(anyhow::Error::new(setup_core::procs::BotsSurvive(vec![
                setup_core::procs::BotProcess {
                    pid: 5151,
                    exe_path: target.join("nemesisbot").to_string_lossy().into_owned(),
                },
            ])))
        };
        let err =
            run_uninstall_confirmed_with(&target, &stop, &never).expect_err("强停失败必须拦停整链");
        assert_eq!(exit_code_of(&err), 6);
        assert_eq!(
            std::fs::read(target.join("install.json"))?,
            record,
            "失败路径账本原样（零删除）"
        );
        Ok(())
    }

    /// 真实查询/终止的活体形态：单用途二进制副本改名 nemesisbot 在临时目录
    /// 下跑起来，验证 /proc 扫描 + readlink 镜像路径 + 路径级强停全链。
    /// 优先 python3（单用途二进制，任意 argv[0] 可跑）；sleep 兜底（GNU
    /// 单调用发行版可用——uutils/busybox 多调用形态按 argv[0] 分派，改名
    /// 后报 unknown program 拒跑，2026-11-10 远程盒实录）。
    /// 守卫 cleanup 兜底杀残留，防测试失败泄漏进程。
    #[test]
    fn running_bot_processes_scope_and_stop_live() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let fake_bot = tmp.path().join("nemesisbot");
        // (源, 运行参数) 候选序：python3 → sleep。
        let (src, args) = [
            (
                "/usr/bin/python3",
                vec!["-c", "import time; time.sleep(60)"],
            ),
            ("/bin/sleep", vec!["60"]),
            ("/usr/bin/sleep", vec!["60"]),
        ]
        .into_iter()
        .find(|(p, _)| std::path::Path::new(p).exists())
        .expect("python3 或 sleep 应至少一个在场");
        std::fs::copy(src, &fake_bot)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_bot, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut child = std::process::Command::new(&fake_bot).args(&args).spawn()?;
        // 副本若是多调用形态会立即退出（unknown program）——wait 一次把这种
        // 环境不满足显式化为诚实 SKIP，而不是让后续 /proc 断言盲红。
        std::thread::sleep(std::time::Duration::from_millis(200));
        if let Some(_status) = child.try_wait()? {
            eprintln!("SKIP: 改名副本在该环境拒绝常驻（多调用二进制分派 argv[0]）");
            return Ok(());
        }
        struct KillGuard(i32);
        impl Drop for KillGuard {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                }
            }
        }
        let _guard = KillGuard(child.id() as i32);

        // 首查：/proc 里找得到它，且镜像路径落在临时目录子树内。
        let found = running_bot_processes()?;
        let mine = found
            .iter()
            .find(|b| b.pid == child.id())
            .expect("/proc 扫描应含刚 spawn 的改名进程");
        assert_eq!(
            mine.exe_path,
            fake_bot.to_string_lossy(),
            "镜像路径 = readlink /proc/<pid>/exe"
        );

        // 路径级强停：只清本目录（他处实例绝不触碰——盒上常驻实例不涉杀）。
        let stopped = stop_install_bots(tmp.path())?;
        assert!(
            stopped.iter().any(|b| b.pid == child.id()),
            "被停止清单应含目标: {stopped:?}"
        );
        // 复核：子进程已退出（SIGKILL = 信号终止退出码）。
        let wait = child.wait()?;
        assert!(!wait.success(), "进程应已被 SIGKILL 终止");
        let after = running_bot_processes()?;
        assert!(
            !after.iter().any(|b| b.pid == child.id()),
            "复核清单不应再有目标"
        );
        Ok(())
    }

    #[test]
    fn system_spawn_failure_degrades_to_note_not_install_failure() -> anyhow::Result<()> {
        // 铁律回归锁：系统集成失败 = notes 降级，绝不回滚已落位产物、绝不炸安装。
        let tmp = tempfile::tempdir()?;
        let package = build_fake_package(tmp.path(), "9.9.9")?;
        let target = tmp.path().join("root");
        let opts = InstallOptions {
            package: package.clone(),
            target_dir: target.clone(),
            accept_risk: false,
            force_downgrade: false,
            no_shortcuts: true,
            autostart: false,
            launch: false,
        };
        let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };

        // ① bot 进程跑了但退出非零（stderr 尾巴进 notes）。
        let failing = |_: &SystemCall| -> Result<std::process::Output> {
            Ok(fail_output("Error: 注册全部失败\nboom 细节"))
        };
        let out = run_install_with(&opts, None, &no_bots, &failing)?;
        assert_eq!(out.notes.len(), 1, "单条降级注记: {:?}", out.notes);
        assert!(
            out.notes[0].contains("system register"),
            "注记点名子命令: {}",
            out.notes[0]
        );
        assert!(
            out.notes[0].contains("boom 细节"),
            "stderr 尾巴进注记: {}",
            out.notes[0]
        );
        assert!(target.join("nemesisbot").exists());
        assert!(target.join("install.json").exists());

        // ② spawn 本身失败（执行器 Err 形态）同样只注记（同版本修复重装路径）。
        let spawn_err = |_: &SystemCall| -> Result<std::process::Output> {
            Err(anyhow!("spawn 系统集成失败"))
        };
        let out2 = run_install_with(&opts, None, &no_bots, &spawn_err)?;
        assert_eq!(out2.notes.len(), 1, "{:?}", out2.notes);
        Ok(())
    }
}
