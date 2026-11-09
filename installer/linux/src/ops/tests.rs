//! 运营链测试。
//!
//! 纪律：纯内容构造器用例**跨平台可跑**（本机 Windows 也进）；带 `#[cfg(target_os = "linux")]`
//! 的 IO/进程用例只在 Linux 编译执行（远程盒/CI 诚实覆盖——Windows 上静默缺席
//! 是本 crate 平台边界的显式形态）。

use super::*;
use anyhow::anyhow;
use setup_core::install::InstallPlan;

// ---------- 纯内容构造器（任意主机可跑） ----------

#[test]
fn path_snippet_has_sentinels_for_idempotency() {
    let s = path_snippet();
    assert!(s.contains(PATH_MARKER_BEGIN));
    assert!(s.contains(PATH_MARKER_END));
    assert!(s.contains(".local/bin"));
    assert!(s.contains("export PATH"));
}

#[test]
fn needs_path_append_matrix() {
    let bin = "/home/u/.local/bin";
    // 已有哨兵 = 永不重复写。
    let with_marker = format!("foo\n{}\nbar", PATH_MARKER_BEGIN);
    assert!(!needs_path_append("", &with_marker, bin));
    // PATH 已精确含 .local/bin = 不需要。
    assert!(!needs_path_append("/usr/bin:/home/u/.local/bin", "", bin));
    // 缺席 = 需要。
    assert!(needs_path_append("/usr/bin:/bin", "", bin));
    assert!(needs_path_append("", "", bin));
    // 形近路径不算（/usr/.local/bin 不是 ~/.local/bin）。
    assert!(needs_path_append("/usr/.local/bin", "", bin));
    // 哨兵缺席但 PATH 缺席 → 需要。
    assert!(needs_path_append("/usr/bin", "echo hi", bin));
}

#[test]
fn desktop_entry_content_is_valid_xdg() {
    let text = desktop_entry_text(
        Path::new("/opt/NemesisBot/nemesisbot"),
        Path::new("/opt/NemesisBot/share/icon.png"),
    );
    assert!(text.starts_with("[Desktop Entry]\n"));
    assert!(text.contains("Type=Application"));
    assert!(text.contains("Name=NemesisBot"));
    // Exec 路径 shell 引号包裹（含空格路径安全），参数在引号外。
    assert!(
        text.contains("Exec='/opt/NemesisBot/nemesisbot' dashboard"),
        "Exec 引号包裹 + dashboard 参数: {text}"
    );
    assert!(text.contains("Icon=/opt/NemesisBot/share/icon.png"));
    assert!(text.contains("Terminal=false"));
}

#[test]
fn systemd_unit_content_is_valid() {
    let text = systemd_unit_text(Path::new("/opt/NemesisBot/nemesisbot"));
    assert!(text.contains("[Unit]"));
    assert!(text.contains("[Service]"));
    assert!(text.contains("[Install]"));
    assert!(
        text.contains("ExecStart='/opt/NemesisBot/nemesisbot' gateway"),
        "ExecStart 引号包裹 + gateway 参数: {text}"
    );
    assert!(text.contains("Restart=on-failure"));
    assert!(
        text.contains("WantedBy=default.target"),
        "user unit 用 default.target 非 multi-user"
    );
}

#[test]
fn shell_quote_escapes_single_quotes() {
    assert_eq!(shell_quote("plain"), "'plain'");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote(""), "''");
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

    /// env 序列化锁（XDG_DATA_HOME 用例持锁——env 进程全局态，并行用例互踩）。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn free_disk_probes_existing_ancestor() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let free = free_disk_bytes(tmp.path())?;
        let deep = tmp.path().join("a/b/c");
        let free2 = free_disk_bytes(&deep)?;
        assert!(free > 0);
        assert_eq!(free2, free, "同卷祖先查询应同一剩余量");
        Ok(())
    }

    #[test]
    fn symlink_install_is_idempotent() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let real = tmp.path().join("nemesisbot");
        std::fs::write(&real, b"bin")?;
        let link = tmp.path().join("deep/bin/nemesisbot");
        install_symlink(&link, &real)?;
        assert!(link.exists());
        assert_eq!(std::fs::read_link(&link)?, real);
        // 重复安装幂等；换目标后重建。
        install_symlink(&link, &real)?;
        let real2 = tmp.path().join("nemesisbot2");
        std::fs::write(&real2, b"bin2")?;
        install_symlink(&link, &real2)?;
        assert_eq!(std::fs::read_link(&link)?, real2, "换目标应重建");
        Ok(())
    }

    #[test]
    fn desktop_entry_lands_under_xdg_data_home() -> anyhow::Result<()> {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir()?;
        // edition 2024：set_var/remove_var 是 unsafe（env 进程全局态）。
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path().join("data")) };
        let dir = tmp.path().join("NemesisBot");
        std::fs::create_dir_all(dir.join("share"))?;
        let p = install_desktop_entry(&dir)?;
        assert!(p.exists(), "desktop 入口应落盘: {}", p.display());
        assert_eq!(p, tmp.path().join("data/applications/nemesisbot.desktop"));
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
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
            no_shortcuts: true, // 测试不碰 ~/.local/bin、XDG、~/.profile
            autostart: false,
            launch: false, // 假 bot 不真启动
        };
        let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
        let fake_bots = || -> Result<Vec<String>> { Ok(vec!["4242".into(), "5151".into()]) };

        // ① 模拟「无实例」→ 全新安装成功。
        let out = run_install_with(&opts, None, &no_bots)?;
        assert_eq!(out.bot_version, "9.9.9");
        assert!(out.plan_text.contains("全新安装"), "{}", out.plan_text);
        assert_eq!(
            out.files_installed, 3,
            "2 载荷文件 + 卸载副本 + install.json"
        );
        assert!(target.join("nemesisbot").exists());
        assert!(target.join("share/icon.png").exists());
        assert!(target.join("uninstall").exists(), "卸载副本落位");
        assert!(target.join("install.json").exists());

        // ② 模拟「有实例」（假 PID 清单）→ 闸拦 exit 6，目录一字节不动。
        let before = std::fs::read(target.join("install.json"))?;
        let err = run_install_with(&opts, None as Option<&dyn Fn(Progress)>, &fake_bots)
            .expect_err("假实例清单必须被闸拦");
        assert_eq!(exit_code_of(&err), 6);
        assert_eq!(before, std::fs::read(target.join("install.json"))?);

        // ③ 模拟「无实例」→ 卸载干净（linux 形态显式摘卸载副本；空目录壳残留
        //    = 设计内——引擎剪目录时副本尚在场）。
        let un = run_uninstall_with(&target, &no_bots)?;
        assert_eq!(un.deleted, 2, "record.files = 2 个载荷文件");
        assert!(!target.join("install.json").exists(), "标记摘除");
        assert!(!target.join("nemesisbot").exists());
        assert!(!target.join("share").exists(), "空目录剪除");
        assert!(!target.join("uninstall").exists(), "副本显式摘除");
        assert!(
            !target.exists() || std::fs::read_dir(&target).is_ok_and(|mut it| it.next().is_none()),
            "目录至多剩空壳"
        );

        // ④ 模拟「有实例」→ 卸载同样被拦（闸先于 install.json 缺失检查）。
        let err = run_uninstall_with(&target, &fake_bots).expect_err("卸载也要过实例闸");
        assert_eq!(exit_code_of(&err), 6);
        Ok(())
    }
}
