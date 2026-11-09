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
