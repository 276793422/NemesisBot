//! crate 级端到端回归（合成载荷全链路：打包 → 检视 → 安装 → 卸载）。
//!
//! 纪律：
//! - 全部走 `tempfile` 隔离目录，互不串扰，不碰真实安装位置；
//! - 涉及 `NEMESIS_ROOT_ANCHOR` 环境变量的用例持进程级串行锁
//!   （env 是进程全局态，并行用例会互踩——主仓 TEST_LOCK 同纪律）；
//! - 篡改用例断言落在 `TypedFail::Integrity`（退出码 2 域），不是泛化
//!   anyhow——退出码映射的锚点在这里钉住。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::install::{self, InstallOptions, InstallPlan, SigPolicy, TypedFail};
use crate::pack::{self, PackOptions};
use crate::pkgfile::{self, Progress};

/// env 序列化锁（签名锚 env 用例专用）。
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 合成载荷：exe 形态必需文件在根（必需文件闸的口径）+ 深层目录 +
/// workspace 模板（升级跳过规则用）。
fn make_payload(dir: &Path, workspace_readme: &str) -> Result<()> {
    std::fs::create_dir_all(dir.join("share/icons"))?;
    std::fs::create_dir_all(dir.join("workspace"))?;
    std::fs::write(
        dir.join("nemesisbot.exe"),
        format!("fake-bot-binary {workspace_readme}"),
    )?;
    std::fs::write(dir.join("share/icons/icon.png"), "fake-png-bytes")?;
    std::fs::write(dir.join("README.md"), "# synthetic payload")?;
    std::fs::write(dir.join("workspace/README.md"), workspace_readme)?;
    Ok(())
}

/// 骨架：非 PE/ELF 任意字节（raw 载体路径——签名走 v4 footer）。
const SKELETON: &[u8] = b"NBSETUP-SKELETON-RAW-CARRIER\r\n";

struct TestKeys {
    keys_path: PathBuf,
    root_anchor_hex: String,
}

fn make_keys(dir: &Path, tag: &str) -> Result<TestKeys> {
    let keys_path = dir.join(format!("keys-{tag}.json"));
    let kh = nemesis_verify::keygen::generate()?;
    let anchor = hex::encode(kh.root_anchor_fingerprint());
    kh.save(keys_path.to_str().unwrap())?;
    Ok(TestKeys {
        keys_path,
        root_anchor_hex: anchor,
    })
}

fn pack_into(
    dir: &Path,
    payload: &Path,
    version: &str,
    keys: Option<&PathBuf>,
    out_name: &str,
) -> Result<PathBuf> {
    let out = dir.join(out_name);
    std::fs::write(dir.join("skeleton.bin"), SKELETON)?;
    let skeleton = dir.join("skeleton.bin");
    pack::run(&PackOptions {
        payload_dir: payload.to_path_buf(),
        skeleton,
        out: out.clone(),
        bot_version: version.to_string(),
        platform: "windows".to_string(),
        sign_keys: keys.cloned(),
        zstd_level: 3,
    })?;
    Ok(out)
}

fn install_opts(target: &Path, policy: SigPolicy) -> InstallOptions {
    InstallOptions {
        target_dir: target.to_path_buf(),
        force_downgrade: false,
        sig_policy: policy,
        keep_uninstall_copy: false,
    }
}

fn typed_of(err: &anyhow::Error) -> Option<&TypedFail> {
    err.downcast_ref::<TypedFail>()
}

fn assert_integrity(err: &anyhow::Error) {
    assert!(
        matches!(typed_of(err), Some(TypedFail::Integrity(_))),
        "期望 Integrity（退出码 2 域），实际: {err:#}"
    );
}

/// 在锁内临时改写 NEMESIS_ROOT_ANCHOR。
fn with_anchor<R>(anchor: Option<&str>, f: impl FnOnce() -> R) -> R {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved = std::env::var("NEMESIS_ROOT_ANCHOR").ok();
    // edition 2024：set_var/remove_var 是 unsafe（env 进程全局态）。
    match anchor {
        Some(v) => unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", v) },
        None => unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") },
    }
    let out = f();
    match saved {
        Some(v) => unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", &v) },
        None => unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") },
    }
    out
}

#[test]
fn roundtrip_unsigned_install_and_uninstall() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "v1-data")?;
    let pkg = pack_into(tmp.path(), &payload_dir, "1.2.0", None, "unsigned.pkg")?;

    // 检视：未签 = 黄级。
    let info = pkgfile::inspect(&pkg)?;
    assert_eq!(info.manifest.bot_version, "1.2.0");
    assert!(!info.manifest_signed);
    assert_eq!(info.sig.grade, crate::sig::SigGrade::Yellow);
    assert_eq!(info.manifest.files.len(), 4);

    // 安装（黄级零交互直接过）。
    let res = install::perform(
        &pkg,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        None,
    )?;
    assert_eq!(res.bot_version, "1.2.0");
    assert_eq!(res.workspace_skipped, 0);
    // 4 载荷文件 + install.json。
    assert_eq!(res.files_installed, 5);
    assert_eq!(
        std::fs::read_to_string(target.join("nemesisbot.exe"))?,
        "fake-bot-binary v1-data"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("share/icons/icon.png"))?,
        "fake-png-bytes"
    );
    // staging 零残留（目录里不应再有 .nbsetup-stage 条目）。
    let no_staging = std::fs::read_dir(&target)?.filter_map(|e| e.ok()).any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(".nbsetup-stage")
    });
    assert!(!no_staging, "staging 目录不应残留");

    // 卸载：全删 + 目录摘除（deleted 计数 = record.files 的 4 个载荷文件；
    // install.json 是记录本体，单独摘除不计数）。
    let (deleted, failures) = install::uninstall(&target)?;
    assert!(failures.is_empty(), "卸载失败清单: {failures:?}");
    assert_eq!(deleted, 4);
    assert!(!target.exists(), "空安装目录应被摘除");
    Ok(())
}

#[test]
fn signed_package_green_with_runtime_anchor() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let keys = make_keys(tmp.path(), "green")?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "signed")?;
    let pkg = pack_into(
        tmp.path(),
        &payload_dir,
        "2.0.0",
        Some(&keys.keys_path),
        "signed.pkg",
    )?;

    let (grade, installed) = with_anchor(Some(&keys.root_anchor_hex), || -> Result<_> {
        let info = pkgfile::inspect(&pkg)?;
        let res = install::perform(
            &pkg,
            &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
            None,
        )?;
        Ok((info.sig.grade, res.files_installed))
    })?;
    assert_eq!(
        grade,
        crate::sig::SigGrade::Green,
        "锚集含测试根时签名应 Valid"
    );
    assert_eq!(installed, 5);
    Ok(())
}

#[test]
fn signed_package_red_without_anchor_and_risk_gate() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let keys = make_keys(tmp.path(), "red")?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "red")?;
    let pkg = pack_into(
        tmp.path(),
        &payload_dir,
        "2.0.0",
        Some(&keys.keys_path),
        "red.pkg",
    )?;

    with_anchor(None, || -> Result<()> {
        // 锚集空 = 签名 Untrusted = 红级。
        let info = pkgfile::inspect(&pkg)?;
        assert_eq!(info.sig.grade, crate::sig::SigGrade::Red);

        // 静默无知情确认 → SigRisk（退出码 5 域）。
        let err = install::perform(
            &pkg,
            &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
            None,
        )
        .expect_err("红级无确认必须拒绝");
        assert!(
            matches!(typed_of(&err), Some(TypedFail::SigRisk(_))),
            "期望 SigRisk（退出码 5 域），实际: {err:#}"
        );
        assert!(!target.join("install.json").exists(), "拒绝安装不得落标记");

        // 知情确认 → 放行。
        let res = install::perform(
            &pkg,
            &install_opts(&target, SigPolicy::Enforce { accept_risk: true }),
            None,
        )?;
        assert_eq!(res.files_installed, 5);
        Ok(())
    })
}

#[test]
fn tamper_payload_byte_hard_fails() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "tamper")?;
    let pkg = pack_into(
        tmp.path(),
        &payload_dir,
        "1.0.0",
        None,
        "tamper-payload.pkg",
    )?;
    let m = pkgfile::inspect(&pkg)?.manifest;

    // 翻 payload 区域第一个字节。
    let mut f = std::fs::OpenOptions::new().write(true).open(&pkg)?;
    use std::io::{Seek, SeekFrom};
    f.seek(SeekFrom::Start(m.payload_offset))?;
    f.write_all(&[0x5a])?;
    drop(f);

    let err = install::perform(
        &pkg,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        None,
    )
    .expect_err("payload 被改必须硬失败");
    assert_integrity(&err);
    assert!(!target.exists(), "失败安装不得污染目标目录");
    Ok(())
}

#[test]
fn tamper_manifest_byte_hard_fails_at_inspect() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "tamper-manifest")?;
    let pkg = pack_into(
        tmp.path(),
        &payload_dir,
        "1.0.0",
        None,
        "tamper-manifest.pkg",
    )?;
    let info = pkgfile::inspect(&pkg)?;
    let blob_start = info.manifest.payload_offset + info.manifest.payload_len;

    let mut f = std::fs::OpenOptions::new().write(true).open(&pkg)?;
    use std::io::{Seek, SeekFrom};
    f.seek(SeekFrom::Start(blob_start))?;
    f.write_all(&[0x7f])?;
    drop(f);

    // manifest sha 对尾注不符 = 检视阶段硬失败（信任分级之前）。
    assert!(pkgfile::inspect(&pkg).is_err(), "manifest 被改必须检视即败");
    assert!(
        install::perform(
            &pkg,
            &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
            None
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn downgrade_rejected_then_force_allowed() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "base")?;
    let newer = pack_into(tmp.path(), &payload_dir, "1.2.0", None, "v120.pkg")?;
    let older = pack_into(tmp.path(), &payload_dir, "1.1.0", None, "v110.pkg")?;

    install::perform(
        &newer,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        None,
    )?;

    // 降级默认拒绝（退出码 1 域）。
    let err = install::perform(
        &older,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        None,
    )
    .expect_err("降级必须被拒");
    assert!(
        matches!(
            typed_of(&err),
            Some(TypedFail::DowngradeRejected { current, incoming })
                if current == "1.2.0" && incoming == "1.1.0"
        ),
        "期望 DowngradeRejected，实际: {err:#}"
    );
    // 被拒安装不落任何痕迹：记录仍是 1.2.0。
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("install.json"))?)?;
    assert_eq!(record["bot_version"], "1.2.0");

    // --force 豁免：覆盖降级成功。
    let opts = InstallOptions {
        force_downgrade: true,
        ..install_opts(&target, SigPolicy::Enforce { accept_risk: false })
    };
    let res = install::perform(&older, &opts, None)?;
    assert_eq!(res.bot_version, "1.1.0");
    Ok(())
}

#[test]
fn upgrade_skips_workspace_prefix() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "old-workspace")?;
    let v1 = pack_into(tmp.path(), &payload_dir, "1.0.0", None, "up-v1.pkg")?;
    // 同目录换内容再打包 v2。
    std::fs::write(payload_dir.join("workspace/README.md"), "new-workspace")?;
    std::fs::write(payload_dir.join("README.md"), "# payload v2")?;
    let v2 = pack_into(tmp.path(), &payload_dir, "2.0.0", None, "up-v2.pkg")?;

    install::perform(
        &v1,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        None,
    )?;
    assert_eq!(
        std::fs::read_to_string(target.join("workspace/README.md"))?,
        "old-workspace"
    );

    let res = install::perform(
        &v2,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        None,
    )?;
    assert_eq!(res.workspace_skipped, 1, "升级模式应跳过 workspace/ 前缀");
    // workspace 保留旧数据（用户数据不覆盖），其余更新。
    assert_eq!(
        std::fs::read_to_string(target.join("workspace/README.md"))?,
        "old-workspace"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("README.md"))?,
        "# payload v2"
    );

    // 记录 files 不含 workspace 条目（卸载不删用户数据）。
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("install.json"))?)?;
    let files = record["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .all(|f| !f.as_str().unwrap().starts_with("workspace/")),
        "install.json 不应含 workspace 条目: {files:?}"
    );
    Ok(())
}

#[test]
fn plan_matrix_fresh_repair_upgrade() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("t");
    assert_eq!(install::plan(&target, Some("1.0.0"))?, InstallPlan::Fresh);
    assert_eq!(install::plan(&target, None)?, InstallPlan::Fresh);

    std::fs::create_dir_all(&target)?;
    std::fs::write(
        target.join("install.json"),
        r#"{"format_version":1,"bot_version":"1.5.0","install_time":0,"files":[],"uninstall_copy":false}"#,
    )?;
    assert_eq!(
        install::plan(&target, Some("1.5.0"))?,
        InstallPlan::Repair("1.5.0".into())
    );
    assert_eq!(
        install::plan(&target, Some("1.6.0"))?,
        InstallPlan::Upgrade("1.5.0".into())
    );
    assert_eq!(
        install::plan(&target, Some("1.4.0"))?,
        InstallPlan::Downgrade {
            current: "1.5.0".into(),
            incoming: "1.4.0".into()
        }
    );
    // 不可比较版本 loud 报错。
    assert!(install::plan(&target, Some("abc")).is_err());
    Ok(())
}

#[test]
fn inspect_rejects_non_packages() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let junk = tmp.path().join("junk.bin");
    std::fs::write(&junk, vec![0u8; 4096])?;
    assert!(pkgfile::inspect(&junk).is_err(), "全零文件非安装包");

    let tiny = tmp.path().join("tiny.bin");
    std::fs::write(&tiny, b"NB")?;
    assert!(pkgfile::inspect(&tiny).is_err(), "过小文件非安装包");

    // 合法包 + 追加尾部垃圾：payload 校验仍过（NBSETUP1 尾注定位在前），
    // 但 inspect 能如实给出 file_len——不额外咬尾（v1 边界：外壳签名形态
    // 下尾部属签名区，raw 骨架无签名区时尾部垃圾是包装层问题，检出归
    // 外壳自检域）。
    let (payload_dir, _) = (tmp.path().join("payload"), ());
    make_payload(&payload_dir, "trail")?;
    let pkg = pack_into(tmp.path(), &payload_dir, "1.0.0", None, "trail.pkg")?;
    let mut f = std::fs::OpenOptions::new().append(true).open(&pkg)?;
    f.write_all(b"trailing-garbage")?;
    drop(f);
    let info = pkgfile::inspect(&pkg)?;
    assert_eq!(
        info.manifest.files.len(),
        4,
        "尾注反向扫描应无视尾部垃圾定位成功"
    );
    Ok(())
}

#[test]
fn progress_events_flow() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (payload_dir, target) = (tmp.path().join("payload"), tmp.path().join("target"));
    make_payload(&payload_dir, "prog")?;
    let pkg = pack_into(tmp.path(), &payload_dir, "1.0.0", None, "prog.pkg")?;

    let events: std::sync::Mutex<Vec<Progress>> = std::sync::Mutex::new(Vec::new());
    install::perform(
        &pkg,
        &install_opts(&target, SigPolicy::Enforce { accept_risk: false }),
        Some(&|p| events.lock().unwrap().push(p)),
    )?;
    let ev = events.into_inner().unwrap();
    assert!(
        ev.iter().any(|p| matches!(p, Progress::Hashing(_))),
        "应有哈希进度事件"
    );
    assert!(
        ev.iter().any(|p| matches!(p, Progress::Extracting(_, _))),
        "应有释放进度事件"
    );
    Ok(())
}

#[test]
fn payload_sha256_recorded_in_manifest() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let payload_dir = tmp.path().join("payload");
    make_payload(&payload_dir, "sha")?;
    let pkg = pack_into(tmp.path(), &payload_dir, "1.0.0", None, "sha.pkg")?;
    let info = pkgfile::inspect(&pkg)?;
    // manifest 记载的 payload sha = 实际区域 sha（这里直接对文件区间算）。
    let mut f = std::fs::File::open(&pkg)?;
    let mut reader = pkgfile::take_region(
        &mut f,
        info.manifest.payload_offset,
        info.manifest.payload_len,
    )?;
    use std::io::Read;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    assert_eq!(
        hex::encode(hasher.finalize()),
        info.manifest.payload_sha256,
        "manifest 载荷 sha 记载必须与实际一致"
    );
    // total_size = 载荷文件字节数聚合（从实际文件现算，不手抄魔数）。
    let expected_total: u64 = [
        "nemesisbot.exe",
        "share/icons/icon.png",
        "README.md",
        "workspace/README.md",
    ]
    .iter()
    .map(|p| std::fs::metadata(payload_dir.join(p)).unwrap().len())
    .sum();
    assert_eq!(
        info.manifest.total_size, expected_total,
        "total_size 聚合口径"
    );
    Ok(())
}

// ===== 运行实例路径级识别与强制停止编排（procs 模块，跨平台） =====

use crate::procs::{self, BotProcess};

fn bot(pid: u32, path: &str) -> BotProcess {
    BotProcess {
        pid,
        exe_path: path.into(),
    }
}

#[test]
fn bots_under_dir_scopes_subtree_and_boundary() {
    let dir = if cfg!(windows) {
        Path::new(r"C:\apps\NemesisBot")
    } else {
        Path::new("/opt/NemesisBot")
    };
    let in_bot = if cfg!(windows) {
        bot(1, r"C:\apps\NemesisBot\nemesisbot.exe")
    } else {
        bot(1, "/opt/NemesisBot/nemesisbot")
    };
    let sibling = if cfg!(windows) {
        bot(2, r"C:\apps\NemesisBot2\nemesisbot.exe")
    } else {
        bot(2, "/opt/NemesisBot2/nemesisbot")
    };
    let ancestor = if cfg!(windows) {
        bot(3, r"C:\apps\nemesisbot.exe")
    } else {
        bot(3, "/opt/nemesisbot")
    };
    let unreadable = bot(4, "");
    let elsewhere = if cfg!(windows) {
        bot(5, r"D:\other\nemesisbot.exe")
    } else {
        bot(5, "/home/x/nemesisbot")
    };
    let scoped = procs::bots_under_dir(
        &[
            in_bot.clone(),
            sibling,
            ancestor,
            unreadable.clone(),
            elsewhere,
        ],
        dir,
    );
    // 前缀边界（NemesisBot2 不匹配）、祖先目录不匹配、路径不可读不归属——
    // 只留子树内的那个。
    assert_eq!(scoped, vec![in_bot], "只留目录子树内实例: {scoped:?}");
}

#[test]
#[cfg(windows)]
fn bots_under_dir_windows_case_insensitive_and_trailing_sep() {
    let dir = Path::new(r"c:\APPS\NemesisBot\");
    let got = procs::bots_under_dir(&[bot(7, r"C:\APPS\NEMESISBOT\nemesisbot.EXE")], dir);
    assert_eq!(got.len(), 1, "大小写与尾分隔符均归一: {got:?}");
}

#[test]
fn stop_bots_in_dir_kills_only_scoped_and_verifies() -> Result<()> {
    let dir = Path::new("/t/NB");
    let ours = bot(42, "/t/NB/nemesisbot");
    let other = bot(99, "/elsewhere/nemesisbot");
    let killed: std::sync::Mutex<Vec<Vec<u32>>> = std::sync::Mutex::new(Vec::new());
    let query_counts = std::sync::atomic::AtomicUsize::new(0);
    let query = || -> Result<Vec<BotProcess>> {
        let n = query_counts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            // 首查：本目录 1 个 + 他处 1 个（scope 外不可杀）。
            Ok(vec![ours.clone(), other.clone()])
        } else {
            // 复核：全清。
            Ok(vec![other.clone()])
        }
    };
    let kill = |pids: &[u32]| -> Result<()> {
        killed.lock().expect("lock").push(pids.to_vec());
        Ok(())
    };
    let stopped = procs::stop_bots_in_dir(dir, &query, &kill, 3, Duration::ZERO)?;
    assert_eq!(stopped, vec![ours], "返回被停止清单");
    assert_eq!(
        killed.lock().expect("lock").as_slice(),
        [&[42u32][..]],
        "只杀 scope 内 PID"
    );
    Ok(())
}

#[test]
fn stop_bots_in_dir_no_targets_skips_kill() -> Result<()> {
    let killed = std::sync::atomic::AtomicBool::new(false);
    let query = || -> Result<Vec<BotProcess>> { Ok(vec![bot(1, "/other/nemesisbot")]) };
    let kill = |_: &[u32]| -> Result<()> {
        killed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    };
    let stopped = procs::stop_bots_in_dir(Path::new("/t/NB"), &query, &kill, 3, Duration::ZERO)?;
    assert!(stopped.is_empty(), "目录内无实例 = 空清单");
    assert!(
        !killed.load(std::sync::atomic::Ordering::SeqCst),
        "无目标不调用 kill"
    );
    Ok(())
}

#[test]
fn stop_bots_in_dir_survivors_after_retry_is_error() {
    let dir = Path::new("/t/NB");
    let ours = bot(7, "/t/NB/nemesisbot");
    let query = || -> Result<Vec<BotProcess>> { Ok(vec![ours.clone()]) };
    let kill = |_: &[u32]| -> Result<()> { Ok(()) }; // 假 kill 无效
    let err = procs::stop_bots_in_dir(dir, &query, &kill, 3, Duration::ZERO)
        .expect_err("复核不清零必须报错");
    let survive = err
        .downcast_ref::<procs::BotsSurvive>()
        .expect("错误类型必须是 BotsSurvive");
    assert_eq!(survive.0, vec![ours.clone()]);
    assert!(
        survive.to_string().contains("PID 7"),
        "人读文本带 PID: {}",
        survive
    );
}

#[test]
fn stop_bots_in_dir_retry_then_success() -> Result<()> {
    let dir = Path::new("/t/NB");
    let ours = bot(8, "/t/NB/nemesisbot");
    let query_counts = std::sync::atomic::AtomicUsize::new(0);
    let query = || -> Result<Vec<BotProcess>> {
        let n = query_counts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n <= 1 {
            // 首查 + 第一轮复核仍在（句柄释放瞬时窗口）。
            Ok(vec![ours.clone()])
        } else {
            Ok(vec![])
        }
    };
    let kill = |_: &[u32]| -> Result<()> { Ok(()) };
    let stopped = procs::stop_bots_in_dir(dir, &query, &kill, 3, Duration::ZERO)?;
    assert_eq!(stopped, vec![ours], "重试窗口内清零 = 成功");
    Ok(())
}

// ===== Unix 执行位回归（2026-11-10 Linux 全链 E2E 实录：裸 write/原子写回
// 按 umask 落 644，包本体与卸载副本运行 rc 126「权限不够」）——只在 unix
// 编译执行（Windows 无执行位语义，诚实缺席）。=====

#[test]
#[cfg(unix)]
fn pack_output_and_shell_sign_keep_exec_bit_on_unix() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir()?;
    let payload_dir = tmp.path().join("payload");
    make_payload(&payload_dir, "exec-bit")?;
    let pkg = pack_into(tmp.path(), &payload_dir, "1.0.0", None, "execbit.pkg")?;

    let mode = std::fs::metadata(&pkg)?.permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o755,
        "pack 输出必须自带执行位（.run 直接运行形态）"
    );

    // 整包外壳签名（sign_file = tmp + rename 换 inode）不得打回 644。
    let keys = make_keys(tmp.path(), "execbit")?;
    pack::sign_file(&pkg, &keys.keys_path)?;
    let mode_after = std::fs::metadata(&pkg)?.permissions().mode();
    assert_eq!(
        mode_after & 0o777,
        0o755,
        "外壳签名原子写回必须保留原执行位"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn uninstall_copy_has_exec_bit_on_unix() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir()?;
    let payload_dir = tmp.path().join("payload");
    make_payload(&payload_dir, "copyexec")?;
    // linux 平台包：必需文件闸要 `nemesisbot`（非 .exe）。
    std::fs::write(payload_dir.join("nemesisbot"), "fake-linux-bot")?;
    let out = tmp.path().join("linux.pkg");
    std::fs::write(tmp.path().join("skeleton.bin"), SKELETON)?;
    pack::run(&PackOptions {
        payload_dir: payload_dir.clone(),
        skeleton: tmp.path().join("skeleton.bin"),
        out: out.clone(),
        bot_version: "1.0.0".to_string(),
        platform: "linux".to_string(),
        sign_keys: None,
        zstd_level: 3,
    })?;

    let target = tmp.path().join("target");
    let res = install::perform(
        &out,
        &InstallOptions {
            target_dir: target.clone(),
            force_downgrade: false,
            sig_policy: SigPolicy::Enforce { accept_risk: false },
            keep_uninstall_copy: true,
        },
        None,
    )?;
    // 5 载荷（make_payload 4 件 + linux 必需的 nemesisbot）+ 卸载副本 + install.json。
    assert_eq!(res.files_installed, 7);

    let copy = target.join("uninstall");
    let mode = std::fs::metadata(&copy)?.permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o755,
        "卸载副本必须带执行位（linux 卸载入口）"
    );
    Ok(())
}

// ===== 提交窗口中途失败（升级 + 目标文件被独占锁定——Windows 杀软深扫/
// 编辑器占用的真实形态）：staging 零残留、旧 install.json 完好、被锁文件
// 字节未破坏；已 rename 的部分保持新版（半新半旧窗口）= 设计语义——不做
// 备份回滚（IO 翻倍 + 丢同卷 rename 原子性），自愈路径 = 重试即修复重装
// （install.json 是旧账本，plan() 正确裁决）。只在 Windows 编译执行
// （share_mode 独占锁是 win32 语义）。=====
#[test]
#[cfg(windows)]
fn commit_window_locked_target_file_fails_clean_and_record_survives() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("target");

    // ① v1 全新安装落位（5 件 = 4 载荷 + install.json）。
    let payload_v1 = tmp.path().join("payload_v1");
    make_payload(&payload_v1, "v1-data")?;
    let pkg_v1 = pack_into(tmp.path(), &payload_v1, "1.0.0", None, "v1.pkg")?;
    install::perform(
        &pkg_v1,
        &install_opts(&target, SigPolicy::PreApproved),
        None,
    )?;
    let v1_record = std::fs::read_to_string(target.join("install.json"))?;
    let exe_v1_bytes = std::fs::read(target.join("nemesisbot.exe"))?;

    // ② 锁定 nemesisbot.exe（share_mode=FILE_SHARE_READ：允许读、不允许
    //    删/换名——rename-over 必失败 ERROR_SHARING_VIOLATION；share_mode(0)
    //    会连本进程后续读断言一起挡掉，os error 32 实录）。
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(target.join("nemesisbot.exe"))?;

    // ③ v2 升级安装 → 提交循环 rename 到被锁目标 → 失败。
    let payload_v2 = tmp.path().join("payload_v2");
    make_payload(&payload_v2, "v2-data")?;
    let pkg_v2 = pack_into(tmp.path(), &payload_v2, "2.0.0", None, "v2.pkg")?;
    let result = install::perform(
        &pkg_v2,
        &install_opts(&target, SigPolicy::PreApproved),
        None,
    );
    assert!(result.is_err(), "锁定目标的升级提交必须失败");
    assert!(
        typed_of(&result.unwrap_err()).is_none(),
        "失败是提交流 IO 错误，不是 Integrity 域"
    );

    // ④ 失败现场断言：
    //    - staging 零残留；
    let staging_left = std::fs::read_dir(&target)?.filter_map(|e| e.ok()).any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(".nbsetup-stage")
    });
    assert!(!staging_left, "提交失败后 staging 必须清掉");
    //    - 旧 install.json 完好（重试自愈的依据：plan() 走修复重装/升级裁决）；
    assert_eq!(
        std::fs::read_to_string(target.join("install.json"))?,
        v1_record,
        "install.json 哨兵最后提交——失败时旧账本必须原样"
    );
    //    - 被锁文件字节未破坏（rename 失败 = 目标原样）；
    assert_eq!(
        std::fs::read(target.join("nemesisbot.exe"))?,
        exe_v1_bytes,
        "被锁目标字节必须原样"
    );
    //    - 字母序在前的文件已 rename = 半新半旧窗口（诚实语义：无备份回滚）。
    assert_eq!(
        std::fs::read_to_string(target.join("README.md"))?,
        "# synthetic payload",
        "载荷公共文件照常提交"
    );

    // ⑤ 锁释放后重试 → 自愈（修复重装/升级成功，账本翻到 v2）。
    drop(lock);
    install::perform(
        &pkg_v2,
        &install_opts(&target, SigPolicy::PreApproved),
        None,
    )?;
    // read_record 是私有面——账本版本用 serde_json 直读验证。
    let record_after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("install.json"))?)?;
    assert_eq!(
        record_after["bot_version"], "2.0.0",
        "重试自愈后账本=新版本"
    );
    assert_eq!(
        std::fs::read(target.join("nemesisbot.exe"))?,
        std::fs::read(payload_v2.join("nemesisbot.exe"))?,
        "重试后 exe 翻到 v2 字节"
    );
    Ok(())
}
