// extract.rs 覆盖率补充（wave 5）：resolve_seven_zip 缓存臂 / 系统回退臂、
// seven_zip_status 三态、extract 成功臂 + 7z 失败 bail + spawn 失败 context。
//
// 纪律：不下载（resolve 的下载臂 43-53 涉及 GitHub 网络，不测）；真 7z
// 只在系统已装（find_system_7z 命中）时才用于 extract 成功臂，否则 SKIP。

use super::*;
use std::io::Write as _;

/// PATH 注入用例的进程级互斥（tests.rs 的 PATH probe 与本文件 /
/// cov_wave6b_tests 的 PATH 用例共用）：set_var 是进程全局副作用，
/// 并行下必须串行，否则 `where` 解析互相污染。
pub(crate) static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 用 zip crate（extract 的 unzip 同款依赖）造一个最小压缩包。
fn write_minimal_zip(dest: &Path, name: &str, body: &[u8]) {
    let file = std::fs::File::create(dest).unwrap();
    let mut w = zip::ZipWriter::new(file);
    w.start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    w.write_all(body).unwrap();
    w.finish().unwrap();
}

/// find_system_7z 命中且**真的能跑**才算数：并行测试里别的用例会临时把
/// PATH 指到装了假 7z.exe 的 fakebin（进程全局环境，无法隔离），所以这里
/// 用「spawn 一次无参调用，退出码 0」做实靶校验，跑不动就 SKIP。
fn working_system_7z() -> Option<std::path::PathBuf> {
    let candidate = find_system_7z()?;
    let ok = std::process::Command::new(&candidate)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok { Some(candidate) } else { None }
}

/// 缓存臂：runtime/7z/7z.exe 存在 → 直接返回（resolve 31-34 + status cached）。
#[tokio::test]
async fn resolve_seven_zip_prefers_cached_copy() {
    let _logs = crate::test_util::capture_logs();
    let rt = tempfile::tempdir().unwrap();
    let cached = rt.path().join("7z").join("7z.exe");
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    std::fs::write(&cached, b"not a real exe - only exists() matters").unwrap();

    assert_eq!(seven_zip_status(rt.path()), (true, "cached"));
    let got = resolve_seven_zip(rt.path()).await.expect("cached hit");
    assert_eq!(got, cached, "resolve must return the cached path");
}

/// 系统回退臂：无缓存 → find_system_7z 命中则直接用（不触网）。
#[tokio::test]
async fn resolve_seven_zip_falls_back_to_system_install() {
    let _logs = crate::test_util::capture_logs();
    let rt = tempfile::tempdir().unwrap();
    if seven_zip_status(rt.path()).1 != "system" {
        eprintln!("SKIP: no system 7-Zip on this machine (download arm stays uncovered)");
        return;
    }
    let got = resolve_seven_zip(rt.path()).await.expect("system 7z hit");
    assert!(got.exists(), "system 7z path must exist: {got:?}");
    assert_eq!(seven_zip_status(rt.path()), (true, "system"));
}

/// extract 成功臂：真 7z 解一个真 zip（172-192 含 info 日志行）。
#[test]
fn extract_success_with_real_7z() {
    let _logs = crate::test_util::capture_logs();
    let Some(seven_zip) = working_system_7z() else {
        eprintln!("SKIP: no working system 7-Zip on this machine");
        return;
    };
    let ws = tempfile::tempdir().unwrap();
    let installer = ws.path().join("fake_installer.zip");
    write_minimal_zip(&installer, "payload/hello.txt", b"sandbox extract fixture");
    let out = ws.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    extract(&installer, &out, &seven_zip).expect("7z extracts a plain zip");
    let extracted = out.join("payload").join("hello.txt");
    assert!(extracted.is_file(), "extracted file missing: {extracted:?}");
    assert_eq!(
        std::fs::read(&extracted).unwrap(),
        b"sandbox extract fixture"
    );
}

/// extract 失败臂：非归档输入 → 7z 非零退出 → bail（179-186）。
#[test]
fn extract_failure_bails_with_status_context() {
    let _logs = crate::test_util::capture_logs();
    let Some(seven_zip) = working_system_7z() else {
        eprintln!("SKIP: no working system 7-Zip on this machine");
        return;
    };
    let ws = tempfile::tempdir().unwrap();
    let junk = ws.path().join("junk.bin");
    std::fs::write(&junk, b"this is definitely not an archive").unwrap();
    let out = ws.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    let err = extract(&junk, &out, &seven_zip).expect_err("non-archive must fail");
    assert!(
        err.to_string().contains("7z extraction failed"),
        "status-failure context missing: {err}"
    );
}

/// extract spawn 失败臂：seven_zip 路径不存在 → context 报 spawn（178）。
#[test]
fn extract_spawn_error_reports_context() {
    let _logs = crate::test_util::capture_logs();
    let ws = tempfile::tempdir().unwrap();
    let missing_exe = ws.path().join("no_such_7z.exe");
    let installer = ws.path().join("whatever.zip");
    std::fs::write(&installer, b"unused").unwrap();
    let err = extract(&installer, ws.path(), &missing_exe).expect_err("missing 7z must fail");
    assert!(
        err.to_string().contains("spawn 7z at"),
        "spawn context missing: {err}"
    );
}
