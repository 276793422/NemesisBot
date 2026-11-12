//! 运营链测试。
//!
//! 纪律：
//! - 系统集成（ARP/Run/快捷方式）实现已收编 bot（nemesis-sysint）——本
//!   文件用**假执行器**（记录调用 + 返回合成 Output）验证 spawn 契约与
//!   降级语义，绝不 spawn 真 bot、绝不写真实注册表/用户路径；
//! - `running_bots` 断言形状不断言空（本机可能有真实实例在跑）。

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use windows_sys::Win32::Storage::FileSystem as wfs;

use super::*;

// ---------- 退出码映射 ----------

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
    let running: anyhow::Error = OpsFail::Running(vec!["pid".into()]).into();
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
    // 泛化 anyhow = 通用失败。
    let generic: anyhow::Error = anyhow!("任意失败");
    assert_eq!(exit_code_of(&generic), 1);
}

// ---------- 磁盘预检 ----------

#[test]
fn required_disk_bytes_adds_margin() {
    assert_eq!(required_disk_bytes(0), 0);
    assert_eq!(required_disk_bytes(100), 120);
    assert_eq!(required_disk_bytes(1_000_000), 1_200_000);
}

#[test]
fn free_disk_probes_existing_ancestor() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    // 目录本体存在 → 直接可查。
    let free = free_disk_bytes(tmp.path())?;
    // 深层不存在目录 → 沿祖先查到 tmp（同卷）。
    let deep = tmp.path().join("a/b/c");
    let free2 = free_disk_bytes(&deep)?;
    assert!(free > 0);
    assert_eq!(free2, free, "同卷祖先查询应同一剩余量");
    Ok(())
}

// ---------- 系统集成 spawn 契约（纯构造面） ----------

#[test]
fn system_call_shape_matches_bot_cli_flags() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path();
    let opts = InstallOptions {
        package: target.join("pkg"),
        target_dir: target.to_path_buf(),
        accept_risk: false,
        force_downgrade: false,
        desktop_shortcut: true,
        no_shortcuts: false,
        autostart: true,
        launch: false,
        taiwei_opt_in: true,
        precise_opt_in: false,
    };
    let call = register_call(&target.join("nemesisbot.exe"), target, &opts);
    assert_eq!(call.exe, target.join("nemesisbot.exe"));
    assert!(
        call.args
            .starts_with(&["system".to_string(), "register".to_string()]),
        "子命令前缀: {:?}",
        call.args
    );
    for flag in ["--autostart", "--desktop"] {
        assert!(call.args.contains(&flag.to_string()), "旗标在场: {flag}");
    }
    let di = call
        .args
        .iter()
        .position(|a| a == "--dir")
        .expect("--dir 在场");
    assert_eq!(call.args[di + 1], target.to_string_lossy());

    // 关闭态旗标缺席（结构体更新：opts 移入，不再使用）。
    let opts2 = InstallOptions {
        autostart: false,
        desktop_shortcut: false,
        no_shortcuts: true,
        ..opts
    };
    let call2 = register_call(&target.join("nemesisbot.exe"), target, &opts2);
    assert!(!call2.args.contains(&"--autostart".to_string()));
    assert!(!call2.args.contains(&"--desktop".to_string()));
    assert!(call2.args.contains(&"--no-shortcuts".to_string()));

    // 注销调用形状。
    let un = unregister_call(&target.join("nemesisbot.exe"), target);
    assert_eq!(
        un.args,
        vec![
            "system".to_string(),
            "unregister".to_string(),
            "--dir".to_string(),
            target.to_string_lossy().into_owned(),
        ]
    );

    // 遥测 disable 调用形状（M4 opt-out 通道）。
    let dis = taiwei_disable_call(&target.join("nemesisbot.exe"));
    assert_eq!(dis.exe, target.join("nemesisbot.exe"));
    assert_eq!(dis.args, vec!["taiwei".to_string(), "disable".to_string()]);

    // 精确定位 enable-precise 调用形状（M6 opt-in 单独同意通道）。
    let prec = taiwei_precise_enable_call(&target.join("nemesisbot.exe"));
    assert_eq!(prec.exe, target.join("nemesisbot.exe"));
    assert_eq!(
        prec.args,
        vec!["taiwei".to_string(), "enable-precise".to_string()]
    );

    // 设备标识播种/删除调用形状（随机持久 ID 机器存储，2026-11-12）。
    let mid_ensure = taiwei_machine_id_ensure_call(&target.join("nemesisbot.exe"));
    assert_eq!(mid_ensure.exe, target.join("nemesisbot.exe"));
    assert_eq!(
        mid_ensure.args,
        vec![
            "taiwei".to_string(),
            "machine-id".to_string(),
            "ensure".to_string()
        ]
    );
    let mid_remove = taiwei_machine_id_remove_call(&target.join("nemesisbot.exe"));
    assert_eq!(mid_remove.exe, target.join("nemesisbot.exe"));
    assert_eq!(
        mid_remove.args,
        vec![
            "taiwei".to_string(),
            "machine-id".to_string(),
            "remove".to_string()
        ]
    );

    // 遥测 uninstall 调用形状（D1：None = 静默/跳过不带旗标；Some = 追加 --reason）。
    let tu_none = taiwei_uninstall_call(&target.join("nemesisbot.exe"), None);
    assert_eq!(tu_none.exe, target.join("nemesisbot.exe"));
    assert_eq!(
        tu_none.args,
        vec![
            "taiwei".to_string(),
            "emit".to_string(),
            "uninstall".to_string()
        ]
    );
    let tu_reason = taiwei_uninstall_call(&target.join("nemesisbot.exe"), Some("not_useful"));
    assert_eq!(
        tu_reason.args,
        vec![
            "taiwei".to_string(),
            "emit".to_string(),
            "uninstall".to_string(),
            "--reason".to_string(),
            "not_useful".to_string()
        ]
    );
}

/// 合成成功 Output（测试执行器用）。
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

// ---------- 进程检测（形状断言） ----------

#[test]
fn running_bots_returns_shape_not_emptiness() -> Result<()> {
    // 本机可能有真实实例在跑（用户自管进程）——只断言成功 + 行格式。
    let list = running_bots()?;
    for line in &list {
        assert!(
            line.to_ascii_lowercase().contains("nemesisbot.exe"),
            "过滤后的行应含镜像名: {line}"
        );
    }
    Ok(())
}

// ---------- 目录定位 ----------

#[test]
fn default_install_dir_under_localappdata() {
    let dir = default_install_dir().expect("本机必有 LOCALAPPDATA/USERPROFILE");
    let s = dir.to_string_lossy();
    assert!(
        s.ends_with("\\NemesisBot"),
        "缺省安装目录以 NemesisBot 收尾: {s}"
    );
    assert!(Path::new(&*s).is_absolute(), "缺省目录绝对路径: {s}");
}

// ---------- Windows API 存活性探针（编译期 feature 闸的直接证据） ----------

#[test]
fn get_disk_free_space_exw_links_and_runs() {
    // windows-sys 0.59 直接 *mut u64（非 ULARGE_INTEGER 结构）。
    let mut free: u64 = 0;
    let mut total: u64 = 0;
    let mut caller: u64 = 0;
    let root: Vec<u16> = "C:\\\0".encode_utf16().collect();
    let ok = unsafe { wfs::GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut total, &mut caller) };
    assert_eq!(ok, 1, "C 盘查询应成功");
    assert!(total > 0);
}

// ---------- 假程序模拟（离线全生命周期：进程检测/ARP 键注入） ----------

use setup_core::pack;

/// 假 tasklist 输出（表头 + 两条 nemesisbot.exe + 噪声行）。
const FAKE_TASKLIST: &str = "\
映像名称                       PID 会话名              会话#       内存使用
========================= ======== ================ =========== ============
nemesisbot.exe               4242 Console                    1     66,808 K
notepad.exe                  1234 Console                    1      1,024 K
nemesisbot.exe               5151 RDP-Tcp#0                  2     91,232 K
chrome.exe                   9999 Console                    1    512,000 K";

#[test]
fn parse_tasklist_filters_fake_process_list() {
    let got = parse_tasklist_output(FAKE_TASKLIST);
    assert_eq!(got.len(), 2, "只留 nemesisbot.exe 行: {got:?}");
    assert!(got.iter().all(|l| l.contains("nemesisbot.exe")));
    // 空输出 / 无匹配形态 → 空清单。
    assert!(parse_tasklist_output("").is_empty());
    assert!(parse_tasklist_output("INFO: 没有运行的任务匹配指定标准。").is_empty());
}

/// 打一个假包：假载荷（假 nemesisbot.exe 字节 + 附属文件）+ 假骨架字节
/// （引擎对骨架只搬运不执行——uninstall.exe 副本 = 假字节，测试里永不运行）。
fn build_fake_package(dir: &Path, version: &str) -> Result<std::path::PathBuf> {
    let payload = dir.join("payload");
    std::fs::create_dir_all(payload.join("share"))?;
    std::fs::write(payload.join("nemesisbot.exe"), b"FAKE-BOT-BYTES")?;
    std::fs::write(payload.join("share/icon.png"), b"png")?;
    std::fs::write(payload.join("IDENTITY.md"), b"# fake identity")?;
    let skeleton = dir.join("skl.bin");
    std::fs::write(&skeleton, b"SKEL")?;
    let out = dir.join("sim.pkg");
    pack::run(&pack::PackOptions {
        payload_dir: payload,
        skeleton,
        out: out.clone(),
        bot_version: version.into(),
        platform: "windows".into(),
        sign_keys: None,
        zstd_level: 1,
    })?;
    Ok(out)
}

/// 模拟安装参数（不碰用户机器面：无快捷方式 / 无自启 / 不启动假 bot；
/// 遥测 opt-in 缺省开 = 生产静默默认；precise 缺省关 = 单独同意默认）。
fn sim_install_options(package: &Path, target: &Path) -> InstallOptions {
    InstallOptions {
        package: package.to_path_buf(),
        target_dir: target.to_path_buf(),
        accept_risk: false,
        force_downgrade: false,
        desktop_shortcut: false,
        no_shortcuts: true,
        autostart: false,
        launch: false,
        taiwei_opt_in: true,
        precise_opt_in: false,
    }
}

#[test]
fn simulated_full_lifecycle_with_fake_process_list() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    // 注入的假进程清单——不 spawn 任何真进程，机器上有无实例在跑都无关。
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let fake_bots = || -> Result<Vec<String>> {
        Ok(vec![
            "nemesisbot.exe               4242 Console   1   66,808 K".into(),
        ])
    };

    // 假系统集成执行器：记录调用、恒成功（不 spawn 真 bot）。
    let calls: Mutex<Vec<SystemCall>> = Mutex::new(Vec::new());
    let system = |call: &SystemCall| -> Result<std::process::Output> {
        calls.lock().expect("lock").push(call.clone());
        Ok(ok_output())
    };

    // ① 模拟「无实例」→ 全新安装成功（假载荷逐文件落位 + 卸载副本 + 单次 register spawn）。
    let out = run_install_with(&opts, None, &no_bots, &system)?;
    assert_eq!(out.bot_version, "9.9.9");
    assert!(out.plan_text.contains("全新安装"), "{}", out.plan_text);
    assert_eq!(
        out.files_installed, 5,
        "3 载荷文件 + 卸载副本 + install.json"
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert!(target.join("nemesisbot.exe").exists());
    assert!(target.join("share/icon.png").exists());
    assert!(target.join("IDENTITY.md").exists());
    assert!(target.join("uninstall.exe").exists(), "卸载副本落位");
    assert!(target.join("install.json").exists());
    // spawn 契约：恰好三次——register（--dir 指向安装目录）+ machine-id
    // ensure（emit 之前播种）+ taiwei emit install（--install-id 非空，M3）。
    {
        let got = calls.lock().expect("lock");
        assert_eq!(
            got.len(),
            3,
            "安装 spawn register + machine-id ensure + taiwei install: {got:?}"
        );
        assert_eq!(got[0].args[1], "register");
        let di = got[0]
            .args
            .iter()
            .position(|a| a == "--dir")
            .expect("--dir");
        assert_eq!(got[0].args[di + 1], target.to_string_lossy());
        assert_eq!(
            &got[1].args[..],
            &["taiwei", "machine-id", "ensure"],
            "第二发 = machine-id ensure（emit install 之前播种）: {:?}",
            got[1].args
        );
        assert_eq!(
            &got[2].args[..3],
            &["taiwei", "emit", "install"],
            "第三发 = taiwei emit install: {:?}",
            got[2].args
        );
        let ii = got[2]
            .args
            .iter()
            .position(|a| a == "--install-id")
            .expect("--install-id");
        assert!(!got[2].args[ii + 1].is_empty(), "install_id 非空透传");
    }

    // ② 模拟「有实例」→ 闸拦 exit 6，且闸在引擎与 spawn 之前——目录一字节不动、零调用。
    let record_before = std::fs::read(target.join("install.json"))?;
    let err = run_install_with(&opts, None, &fake_bots, &system).expect_err("假实例清单必须被闸拦");
    assert_eq!(exit_code_of(&err), 6);
    let record_after = std::fs::read(target.join("install.json"))?;
    assert_eq!(record_before, record_after, "闸在引擎之前，目录不被触碰");
    assert_eq!(calls.lock().expect("lock").len(), 3, "闸拦路径不 spawn");

    // ③ 模拟「无实例」→ 卸载干净（record.files 载荷清空、标记摘除、空目录剪除；
    //    卸载副本摘除：测试宿主不是 uninstall.exe → 走显式删除路径；副本
    //    自身运行形态才由延迟自删接手）。假注销执行器断言**顺序契约**：
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
    assert_eq!(un.deleted, 3, "record.files = 3 个载荷文件");
    assert!(
        !un.self_delete_scheduled,
        "测试二进制不是 uninstall.exe，不排自删"
    );
    assert!(un.warnings.is_empty(), "{:?}", un.warnings);
    assert!(!target.join("install.json").exists(), "标记摘除");
    assert!(!target.join("nemesisbot.exe").exists());
    assert!(!target.join("share").exists(), "空目录剪除");
    assert!(
        !target.join("uninstall.exe").exists(),
        "副本摘除（宿主非副本形态 → 显式删除路径；BUG-20261111-071 回归锁）"
    );
    assert!(!target.exists(), "副本摘除后目录空壳剪除");
    {
        let got = un_calls.lock().expect("lock");
        assert_eq!(
            got.len(),
            3,
            "卸载 spawn unregister + taiwei uninstall + machine-id remove: {got:?}"
        );
        assert_eq!(got[0].args[1], "unregister");
        assert_eq!(
            &got[1].args[..3],
            &["taiwei", "emit", "uninstall"],
            "第二发 = taiwei emit uninstall: {:?}",
            got[1].args
        );
        assert_eq!(
            &got[2].args[..],
            &["taiwei", "machine-id", "remove"],
            "第三发 = machine-id remove（emit 之后、引擎删除之前——卸载不留机器标识）: {:?}",
            got[2].args
        );
    }

    // ④ 模拟「有实例」→ 卸载同样被拦（闸先于 spawn 与 install.json 缺失检查——exit 6）。
    let err = run_uninstall_with(&target, &fake_bots, &un_runner).expect_err("卸载也要过实例闸");
    assert_eq!(exit_code_of(&err), 6);
    Ok(())
}

// ---------- 遥测 opt-out 序列（M4）：取消勾选 = disable 先于 emit ----------

#[test]
fn taiwei_opt_out_spawns_disable_before_emit() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let mut opts = sim_install_options(&package, &target);
    opts.taiwei_opt_in = false;
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let calls: Mutex<Vec<SystemCall>> = Mutex::new(Vec::new());
    let system = |call: &SystemCall| -> Result<std::process::Output> {
        calls.lock().expect("lock").push(call.clone());
        Ok(ok_output())
    };

    let out = run_install_with(&opts, None, &no_bots, &system)?;
    assert!(
        out.warnings.is_empty(),
        "假执行器恒成功，取消勾选不产生警告: {:?}",
        out.warnings
    );
    let got = calls.lock().expect("lock");
    assert_eq!(
        got.len(),
        4,
        "取消勾选 = register + taiwei disable + machine-id ensure + taiwei emit install: {got:?}"
    );
    assert_eq!(got[0].args[1], "register");
    assert_eq!(
        &got[1].args[..],
        &["taiwei", "disable"],
        "第二发 = taiwei disable（opt-out 权威位落盘）: {:?}",
        got[1].args
    );
    assert_eq!(
        &got[2].args[..],
        &["taiwei", "machine-id", "ensure"],
        "第三发 = machine-id ensure（设备标识与遥测开关解耦：禁上报≠不留机器标识）: {:?}",
        got[2].args
    );
    assert_eq!(
        &got[3].args[..3],
        &["taiwei", "emit", "install"],
        "第四发 = taiwei emit install（emit 读开关链诚实短路零网络）: {:?}",
        got[3].args
    );
    Ok(())
}

// ---------- 精确定位序列（M6）：勾选 = enable-precise 先于 emit；默认不勾 ----------

#[test]
fn taiwei_precise_opt_in_spawns_enable_precise() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let mut opts = sim_install_options(&package, &target);
    opts.precise_opt_in = true;
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let calls: Mutex<Vec<SystemCall>> = Mutex::new(Vec::new());
    let system = |call: &SystemCall| -> Result<std::process::Output> {
        calls.lock().expect("lock").push(call.clone());
        Ok(ok_output())
    };

    let out = run_install_with(&opts, None, &no_bots, &system)?;
    assert!(
        out.warnings.is_empty(),
        "假执行器恒成功: {:?}",
        out.warnings
    );
    let got = calls.lock().expect("lock");
    assert_eq!(
        got.len(),
        4,
        "勾选精确定位 = register + enable-precise + machine-id ensure + emit install 四发: {got:?}"
    );
    assert_eq!(got[0].args[1], "register");
    assert_eq!(
        &got[1].args[..],
        &["taiwei", "enable-precise"],
        "第二发 = taiwei enable-precise（precise 权威位落盘）: {:?}",
        got[1].args
    );
    assert_eq!(
        &got[2].args[..],
        &["taiwei", "machine-id", "ensure"],
        "第三发 = machine-id ensure: {:?}",
        got[2].args
    );
    assert_eq!(
        &got[3].args[..3],
        &["taiwei", "emit", "install"],
        "第四发 = taiwei emit install: {:?}",
        got[3].args
    );
    Ok(())
}

#[test]
fn taiwei_precise_default_off_means_no_enable_precise_spawn() -> Result<()> {
    // 默认（precise_opt_in=false）：序列不变——register + emit install 两发，
    // 绝无 enable-precise（单独同意纪律：不勾 = 不写任何 precise 位）。
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    assert!(
        !opts.precise_opt_in,
        "sim 构造默认 precise_opt_in=false（单独同意纪律）"
    );
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let calls: Mutex<Vec<SystemCall>> = Mutex::new(Vec::new());
    let system = |call: &SystemCall| -> Result<std::process::Output> {
        calls.lock().expect("lock").push(call.clone());
        Ok(ok_output())
    };

    let out = run_install_with(&opts, None, &no_bots, &system)?;
    assert!(
        out.warnings.is_empty(),
        "假执行器恒成功: {:?}",
        out.warnings
    );
    let got = calls.lock().expect("lock");
    assert_eq!(
        got.len(),
        3,
        "默认 = register + machine-id ensure + emit install 三发: {got:?}"
    );
    assert_eq!(got[0].args[1], "register");
    assert_eq!(
        &got[1].args[..],
        &["taiwei", "machine-id", "ensure"],
        "第二发 = machine-id ensure: {:?}",
        got[1].args
    );
    assert_eq!(&got[2].args[..3], &["taiwei", "emit", "install"]);
    for c in got.iter() {
        assert!(
            !c.args.contains(&"enable-precise".to_string()),
            "默认绝不 spawn enable-precise: {:?}",
            c.args
        );
    }
    Ok(())
}

// ---------- 确认卸载链（路径级强停，2026-11-10 用户拍板） ----------

/// 真实查询/终止的活体形态（Windows 专用——ping.exe 副本改名 nemesisbot.exe
/// 在临时目录下跑起来，验证快照枚举 + 镜像路径 + 路径级强停全链；不用
/// cmd.exe 副本——「改名 cmd 落临时目录」是杀软已知启发式命中形态，易假红）。
/// 守卫 cleanup 兜底杀残留，防测试失败泄漏进程。
#[test]
#[cfg(windows)]
fn running_bot_processes_scope_and_stop_live() -> Result<()> {
    use std::os::windows::process::CommandExt;

    let tmp = tempfile::tempdir()?;
    let fake_bot = tmp.path().join("nemesisbot.exe");
    std::fs::copy(r"C:\Windows\System32\ping.exe", &fake_bot)?;
    let mut child = Command::new(&fake_bot)
        .args(["-n", "60", "127.0.0.1"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    // 进程出现进快照是异步的（CreateProcess 返回 ≠ 快照可见）——轮询首查
    // 至多 2s，环境不满足显式失败，不与调度竞速假红。
    let mut found = running_bot_processes()?;
    let mut appeared = found.iter().any(|b| b.pid == child.id());
    for _ in 0..40 {
        if appeared {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        found = running_bot_processes()?;
        appeared = found.iter().any(|b| b.pid == child.id());
    }
    // 守卫：无论如何收尾（含断言失败路径）不留孤儿。
    struct KillGuard(u32);
    impl Drop for KillGuard {
        fn drop(&mut self) {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.0.to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
    }
    let _guard = KillGuard(child.id());

    // 首查：快照里找得到它，且镜像路径落在临时目录子树内。
    let mine = found
        .iter()
        .find(|b| b.pid == child.id())
        .expect("快照应含刚 spawn 的改名 ping 进程（2s 轮询窗口内）");
    assert!(
        mine.exe_path
            .to_ascii_lowercase()
            .starts_with(tmp.path().to_string_lossy().to_ascii_lowercase().as_str()),
        "镜像路径应落在本目录: {}",
        mine.exe_path
    );

    // 路径级强停：只清本目录（他处实例——用户自管进程——绝不触碰）。
    let stopped = stop_install_bots(tmp.path())?;
    assert!(
        stopped.iter().any(|b| b.pid == child.id()),
        "被停止清单应含目标: {stopped:?}"
    );
    // 复核：子进程已退出（强杀 = 非成功退出码）。
    let wait = child.wait()?;
    assert!(!wait.success(), "进程应已被 taskkill 终止");
    let after = running_bot_processes()?;
    assert!(
        !after.iter().any(|b| b.pid == child.id()),
        "复核清单不应再有目标"
    );
    Ok(())
}

#[test]
#[cfg(windows)]
fn confirmed_uninstall_stops_then_unregisters_in_order() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let system = |_: &SystemCall| -> Result<std::process::Output> { Ok(ok_output()) };
    run_install_with(&opts, None, &no_bots, &system)?;

    // 假强停：记录调用时序 + 返回 1 个「被停止」实例。
    let seq: std::sync::Arc<Mutex<Vec<&'static str>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
    let seq_stop = std::sync::Arc::clone(&seq);
    let probe = target.clone();
    let stop = move || -> Result<Vec<setup_core::procs::BotProcess>> {
        // 强停先于注销执行——此刻账本必须还在（bot 还锁着镜像）。
        assert!(
            probe.join("install.json").exists(),
            "强停必须先于引擎删除（停止时账本应在场）"
        );
        seq_stop.lock().expect("lock").push("stop");
        Ok(vec![setup_core::procs::BotProcess {
            pid: 4242,
            exe_path: probe.join("nemesisbot.exe").to_string_lossy().into_owned(),
        }])
    };
    let seq_sys = std::sync::Arc::clone(&seq);
    let sys_probe = target.clone();
    let runner = move |call: &SystemCall| -> Result<std::process::Output> {
        assert!(
            sys_probe.join("install.json").exists(),
            "unregister 顺序契约保持"
        );
        // 按子命令记标签：system unregister 与 taiwei emit（M3 同窗）。
        let label = match call.args.get(1).map(String::as_str) {
            Some("unregister") => "unregister",
            Some("emit") => "taiwei",
            Some("machine-id") => "hid_remove",
            other => panic!("意外系统调用: {other:?}"),
        };
        seq_sys.lock().expect("lock").push(label);
        Ok(ok_output())
    };

    let out = run_uninstall_confirmed_with(&target, &stop, &runner, None)?;
    assert_eq!(out.bots_stopped, 1, "强停计数进结果");
    assert_eq!(
        *seq.lock().expect("lock"),
        vec!["stop", "unregister", "taiwei", "hid_remove"],
        "强停先于注销，遥测随注销同窗，设备标识删除收尾（先于引擎删除）"
    );
    assert!(!target.join("install.json").exists(), "卸载完成账本摘除");
    Ok(())
}

#[test]
#[cfg(windows)]
fn confirmed_uninstall_forwards_reason_to_taiwei() -> Result<()> {
    // D1 透传契约：问卷选择经 run_uninstall_confirmed_with 原样进 emit args。
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let system = |_: &SystemCall| -> Result<std::process::Output> { Ok(ok_output()) };
    run_install_with(&opts, None, &no_bots, &system)?;

    let seen: std::sync::Arc<Mutex<Option<Vec<String>>>> = std::sync::Arc::new(Mutex::new(None));
    let seen_runner = std::sync::Arc::clone(&seen);
    let stop = || -> Result<Vec<setup_core::procs::BotProcess>> { Ok(vec![]) };
    let runner = move |call: &SystemCall| -> Result<std::process::Output> {
        if call.args.get(1).map(String::as_str) == Some("emit") {
            *seen_runner.lock().expect("lock") = Some(call.args.clone());
        }
        Ok(ok_output())
    };
    let out = run_uninstall_confirmed_with(&target, &stop, &runner, Some("issue"))?;
    assert_eq!(out.bots_stopped, 0);
    assert_eq!(
        seen.lock().expect("lock").as_ref().map(|a| a.as_slice()),
        Some(
            &[
                "taiwei".to_string(),
                "emit".to_string(),
                "uninstall".to_string(),
                "--reason".to_string(),
                "issue".to_string()
            ][..]
        ),
        "问卷原因原样透传 emit uninstall args"
    );
    Ok(())
}

#[test]
#[cfg(windows)]
fn confirmed_uninstall_survivors_block_before_unregister() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
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
                exe_path: target.join("nemesisbot.exe").to_string_lossy().into_owned(),
            },
        ])))
    };
    let err = run_uninstall_confirmed_with(&target, &stop, &never, None)
        .expect_err("强停失败必须拦停整链");
    assert_eq!(exit_code_of(&err), 6);
    assert_eq!(
        std::fs::read(target.join("install.json"))?,
        record,
        "失败路径账本原样（零删除）"
    );
    Ok(())
}

#[test]
fn system_spawn_failure_degrades_to_warning_not_install_failure() -> Result<()> {
    // 铁律回归锁：系统集成失败 = 警告降级，绝不回滚已落位产物、绝不炸安装。
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };

    // ① bot 进程跑了但退出非零（stderr 尾巴进警告）。
    let failing = |_: &SystemCall| -> Result<std::process::Output> {
        Ok(fail_output("Error: 注册全部失败\nboom 细节"))
    };
    let out = run_install_with(&opts, None, &no_bots, &failing)?;
    assert_eq!(
        out.warnings.len(),
        3,
        "register + machine-id ensure + taiwei 三条降级警告: {:?}",
        out.warnings
    );
    assert!(
        out.warnings[0].contains("system register"),
        "警告点名子命令: {}",
        out.warnings[0]
    );
    assert!(
        out.warnings[0].contains("boom 细节"),
        "stderr 尾巴进警告: {}",
        out.warnings[0]
    );
    assert!(
        out.warnings[1].contains("machine-id ensure"),
        "播种警告点名子命令: {}",
        out.warnings[1]
    );
    assert!(
        out.warnings[2].contains("taiwei emit install"),
        "遥测警告点名子命令: {}",
        out.warnings[1]
    );
    // 文件产物原样在位（不回滚）。
    assert!(target.join("nemesisbot.exe").exists());
    assert!(target.join("install.json").exists());

    // ② spawn 本身失败（执行器 Err 形态）同样只警告。
    let spawn_err = |_: &SystemCall| -> Result<std::process::Output> {
        Err(anyhow::anyhow!("spawn 系统集成失败"))
    };
    let opts2 = sim_install_options(&package, &target);
    let out2 = run_install_with(&opts2, None, &no_bots, &spawn_err)?;
    assert_eq!(out2.warnings.len(), 3, "{:?}", out2.warnings);
    Ok(())
}

#[test]
fn simulated_corrupted_payload_fails_closed_exit2() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    // 翻转 payload 区首字节（释放前 sha256 硬闸必咬）。
    let info = pkgfile::inspect(&package)?;
    let off = info.manifest.payload_offset;
    {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&package)?;
        f.seek(SeekFrom::Start(off))?;
        let mut b = [0u8; 1];
        f.read_exact(&mut b)?;
        b[0] ^= 0xFF;
        f.seek(SeekFrom::Start(off))?;
        f.write_all(&b)?;
    }
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    // 完整性闸在 spawn 之前——假执行器若被调到即测试失败（零调用断言兜底）。
    let never = |call: &SystemCall| -> Result<std::process::Output> {
        panic!("完整性失败路径不应 spawn 系统集成: {call}")
    };
    let err = run_install_with(&opts, None, &no_bots, &never).expect_err("载荷被改必须硬失败");
    assert_eq!(
        exit_code_of(&err),
        2,
        "完整性域退出码 2（非信任决策，不可知情越过）"
    );
    assert!(
        matches!(
            err.downcast_ref::<TypedFail>(),
            Some(TypedFail::Integrity(_))
        ),
        "类型化完整性失败: {err:#}"
    );
    // fail-closed：目标目录不落任何 bot 文件（staging 已清理）。
    assert!(!target.join("nemesisbot.exe").exists());
    Ok(())
}

// ---------- 自删脚本串（纯函数，回归锁：raw_arg 引号纪律） ----------

#[test]
fn self_delete_script_quotes_path_without_escape_artifacts() {
    let p = Path::new(r"C:\Program Files\NemesisBot\uninstall.exe");
    let s = self_delete_script(p);
    assert!(
        s.starts_with(r#"ping -n 3 127.0.0.1 >nul & del /f /q "C:\Program Files"#),
        "del 段引号包裹含空格路径: {s}"
    );
    assert!(
        s.contains(r#"" & rd "C:\Program Files\NemesisBot""#),
        "rd 段引号包裹目录: {s}"
    );
    // 回归锁（2026-11-10 实装测试真 bug）：脚本串经 raw_arg 原样上命令行，
    // 串内不得出现 Rust/MSVCRT 转义形态 `\"`——cmd 收到即字面反斜杠引号
    // = del/rd「文件名、目录名或卷标语法不正确」且 CREATE_NO_WINDOW 无声吞错。
    assert!(
        !s.contains("\\\""),
        "串内出现 \\\" 转义形态（cmd 语法错误根因）: {s}"
    );
    assert!(s.ends_with('"'), "rd 目录收尾引号闭合: {s}");
}

#[test]
fn self_is_copy_predicate_false_in_test_host() {
    // 测试宿主二进制不叫 uninstall.exe → 非副本形态（卸载走显式删除路径，
    // 自删排程不触发——与 ③ 的副本摘除断言同判据）。
    assert!(!self_is_uninstall_copy());
}
