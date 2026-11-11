//! Linux IO 面测试（只在 Linux 编译执行，远程 zoo 盒跑）。
//!
//! 纪律（移植自 installer/linux ops/tests.rs 并扩展）：
//! - 路径全部走 `_at` 注入形态或 tempfile 临时 HOME（绝不碰 zoo 真实
//!   ~/.profile / ~/.local/bin）；
//! - 涉环境变量重定向的用例持 [`ENV_LOCK`] 串行（并行测试共享进程 env）；
//! - 证书只测**文件层**（cert_write_file_at / cert_remove_file_at）——
//!   `update-ca-certificates` 系统信任库变更按用户纪律默认永不触发。

#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use nemesis_sysint::content::{PATH_MARKER_BEGIN, PATH_MARKER_END};
use nemesis_sysint::linux::{
    append_path_snippet_at, cert_remove_file_at, cert_write_file_at, install_symlink,
    write_desktop_entry_at, write_unit_at,
};
use nemesis_sysint::{RegisterRequest, unregister};

/// 环境变量重定向串行锁（env 是进程级全局，并行用例必须互斥）。
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 把 HOME/XDG 三兄弟重定向进临时目录（守卫存活期间生效）。
struct EnvGuard {
    saved: Vec<(String, String)>,
}

impl EnvGuard {
    fn redirect_home(base: &Path) -> Self {
        let names = ["HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME"];
        let mut saved = Vec::new();
        for n in names {
            if let Ok(v) = std::env::var(n) {
                saved.push((n.to_string(), v));
            }
            // edition 2024：set_var 是 unsafe（并行测试持 ENV_LOCK 串行保证安全）。
            unsafe { std::env::set_var(n, base) };
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (n, v) in &self.saved {
            unsafe { std::env::set_var(n, v) };
        }
    }
}

fn fake_der() -> Vec<u8> {
    let mut v = vec![0x30, 0x82, 0x01, 0x00, 0xAB];
    v.extend(0u8..=250u8);
    v
}

// ---------- 符号链接（幂等重建） ----------

#[test]
fn symlink_install_is_idempotent_and_repointable() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let target1 = tmp.path().join("bin/nemesisbot");
    std::fs::create_dir_all(tmp.path().join("bin"))?;
    std::fs::write(&target1, b"#!/bin/sh\n")?;
    let link = tmp.path().join("links/deep/nemesisbot");

    install_symlink(&link, &target1)?;
    let meta = std::fs::symlink_metadata(&link)?;
    assert!(meta.file_type().is_symlink(), "应为符号链接");
    assert_eq!(std::fs::read_link(&link)?, target1);

    // 幂等：同目标重建 = 无操作不报错。
    install_symlink(&link, &target1)?;
    assert_eq!(std::fs::read_link(&link)?, target1);

    // 换目标：摘旧重建。
    let target2 = tmp.path().join("bin/nemesisbot2");
    std::fs::write(&target2, b"x")?;
    install_symlink(&link, &target2)?;
    assert_eq!(std::fs::read_link(&link)?, target2);
    Ok(())
}

// ---------- .desktop / systemd unit 落盘 ----------

#[test]
fn desktop_entry_and_unit_files_written() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let de = tmp.path().join("applications/nemesisbot.desktop");
    let unit = write_unit_at(
        &tmp.path().join("systemd/user/nemesisbot.service"),
        Path::new("/opt/NemesisBot/nemesisbot"),
    )?;
    write_desktop_entry_at(
        &de,
        Path::new("/opt/NemesisBot/nemesisbot"),
        Path::new("/opt/NemesisBot/share/icon.png"),
    )?;
    assert!(de.exists(), ".desktop 应落盘（含父目录创建）");
    assert!(unit.exists(), "unit 应落盘");
    let de_body = std::fs::read_to_string(&de)?;
    assert!(de_body.starts_with("[Desktop Entry]"));
    assert!(de_body.contains("dashboard"), "Exec 带 dashboard 参数");
    let unit_body = std::fs::read_to_string(&unit)?;
    assert!(unit_body.contains("gateway"), "ExecStart 带 gateway 参数");
    Ok(())
}

// ---------- PATH 哨兵块（幂等追加） ----------

#[test]
fn path_snippet_append_is_idempotent() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let profile = tmp.path().join(".profile");
    std::fs::write(&profile, "# user's own shell config\nexport EDITOR=vim\n")?;

    // 首次追加 = true；哨兵块 + 保留用户原有内容。
    assert!(append_path_snippet_at(&profile)?, "首次应追加");
    let body = std::fs::read_to_string(&profile)?;
    assert!(body.contains("export EDITOR=vim"), "用户原内容不得破坏");
    assert!(body.contains(".local/bin"), "哨兵块含 .local/bin PATH");

    // 二次 = false（哨兵在册，永不重复写）。
    assert!(!append_path_snippet_at(&profile)?, "哨兵在册应跳过");
    let body2 = std::fs::read_to_string(&profile)?;
    assert_eq!(
        body2.matches(".local/bin").count(),
        body.matches(".local/bin").count(),
        "重复注册不得叠加 PATH 块"
    );
    Ok(())
}

// ---------- 证书文件层（tempdir 注入，不碰真实信任库） ----------

#[test]
fn cert_file_layer_roundtrip() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let der = fake_der();

    let written = cert_write_file_at(tmp.path(), &der)?;
    assert!(written.exists(), "证书文件应落盘");
    assert!(
        written
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("nemesisbot-")),
        "落名带 nemesisbot- 前缀: {}",
        written.display()
    );
    let body = std::fs::read_to_string(&written)?;
    assert!(body.starts_with("-----BEGIN CERTIFICATE-----"), "PEM 载体");
    assert!(body.contains("-----END CERTIFICATE-----"));

    // 摘除 + 幂等语义（再摘 = Err 显式报）。
    cert_remove_file_at(tmp.path(), &der)?;
    assert!(!written.exists());
    assert!(
        cert_remove_file_at(tmp.path(), &der).is_err(),
        "缺席摘除应报错"
    );
    Ok(())
}

// ---------- facade 全链路（HOME 重定向） ----------

#[test]
fn facade_register_status_unregister_roundtrip() -> Result<()> {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir()?;
    let _guard = EnvGuard::redirect_home(tmp.path());

    let req = RegisterRequest {
        install_dir: Path::new("/opt/NemesisBot").to_path_buf(),
        version: "9.9.9".into(),
        autostart: true,
        shortcuts: true,
        desktop: false,
    };
    let rep = nemesis_sysint::register(&req);
    assert!(
        rep.ok() && !rep.registered.is_empty(),
        "注册应有成功项: {:?} / {:?}",
        rep.registered,
        rep.warnings
    );
    // systemd 激活 best-effort：systemctl 缺席/失败只进 warnings，不算失败。
    assert!(
        rep.warnings.iter().all(|w| w.contains("systemctl")),
        "warnings 只应含 systemctl 激活告警: {:?}",
        rep.warnings
    );

    // 三件套 + unit 落点齐全。symlink 断言走 symlink_metadata（exists() 会
    // 跟随链接——目标 /opt/... 在测试机上不存在，悬空链接也是合法产物）；
    // XDG_DATA_HOME/XDG_CONFIG_HOME 已重定向到 tmp 根，实现正确尊重 XDG
    // （.desktop 落 tmp/applications/、unit 落 tmp/systemd/user/）。
    assert!(
        std::fs::symlink_metadata(tmp.path().join(".local/bin/nemesisbot"))
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "symlink（悬空也合法）"
    );
    assert!(
        tmp.path().join("applications/nemesisbot.desktop").exists(),
        ".desktop（XDG_DATA_HOME 直指）"
    );
    let profile = std::fs::read_to_string(tmp.path().join(".profile"))?;
    assert!(
        profile.contains(PATH_MARKER_BEGIN) && profile.contains(PATH_MARKER_END),
        "PATH 哨兵"
    );
    assert!(
        tmp.path().join("systemd/user/nemesisbot.service").exists(),
        "unit（XDG_CONFIG_HOME 直指）"
    );

    // status 反映在册。
    let rows = nemesis_sysint::status(Path::new("/opt/NemesisBot"))?;
    assert!(
        rows.iter()
            .any(|(k, v)| k.contains("符号链接") && v.contains("在册")),
        "status 应报在册: {rows:?}"
    );

    // 注销：文件层全清 + PATH 哨兵按设计保留。
    let urep = unregister(Path::new("/opt/NemesisBot"));
    assert!(
        urep.warnings.is_empty(),
        "注销不应有警告: {:?}",
        urep.warnings
    );
    assert!(
        std::fs::symlink_metadata(tmp.path().join(".local/bin/nemesisbot")).is_err(),
        "符号链接应已摘除"
    );
    assert!(!tmp.path().join("applications/nemesisbot.desktop").exists());
    assert!(!tmp.path().join("systemd/user/nemesisbot.service").exists());
    let profile_after = std::fs::read_to_string(tmp.path().join(".profile"))?;
    assert!(
        profile_after.contains(PATH_MARKER_BEGIN),
        "PATH 哨兵按设计保留"
    );
    Ok(())
}

#[test]
fn facade_unregister_is_idempotent_with_empty_home() -> Result<()> {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir()?;
    let _guard = EnvGuard::redirect_home(tmp.path());
    let rep = unregister(Path::new("/opt/NemesisBot"));
    assert!(
        rep.warnings.is_empty(),
        "空 HOME 注销幂等无警告: {:?}",
        rep.warnings
    );
    assert!(
        rep.removed.iter().any(|r| r.contains("PATH")),
        "PATH 保留注记恒在: {:?}",
        rep.removed
    );
    Ok(())
}

// ---------- 删除失败必须说话（BUG-20261111-070 回归锁） ----------

#[test]
fn unregister_removal_failure_goes_to_warnings_not_false_removed() -> Result<()> {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir()?;
    let _guard = EnvGuard::redirect_home(tmp.path());

    // 确定性制造「在场但删不掉」：条目建成目录 → remove_file 撞
    // IsADirectory 必失败。删除失败必须进 warnings 且不得假报 removed
    //（假成功会让卸载链退码 0 + 残留悬挂 symlink 指向已删安装目录）。
    std::fs::create_dir_all(tmp.path().join(".local/bin/nemesisbot"))?;
    std::fs::create_dir_all(tmp.path().join("applications/nemesisbot.desktop"))?;

    let rep = unregister(Path::new("/opt/NemesisBot"));
    assert!(
        rep.warnings.iter().any(|w| w.contains("符号链接")),
        "符号链接删除失败必须进 warnings: {:?}",
        rep.warnings
    );
    assert!(
        rep.warnings.iter().any(|w| w.contains("桌面入口")),
        ".desktop 删除失败必须进 warnings: {:?}",
        rep.warnings
    );
    assert!(
        !rep.removed.iter().any(|r| r.contains("符号链接")),
        "符号链接删除失败不得假报 removed: {:?}",
        rep.removed
    );
    assert!(
        !rep.removed.iter().any(|r| r.contains("桌面入口")),
        ".desktop 删除失败不得假报 removed: {:?}",
        rep.removed
    );
    // 条目原样保留（确实没删成）。
    assert!(tmp.path().join(".local/bin/nemesisbot").is_dir());
    assert!(tmp.path().join("applications/nemesisbot.desktop").is_dir());
    Ok(())
}
