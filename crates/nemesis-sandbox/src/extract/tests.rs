use super::*;

/// Download 7z.zip from the project GitHub + unzip → 7z.exe/7z.dll appear.
/// This exercises the download path directly (bypasses the system-7z check),
/// validating it for users who don't have 7-Zip pre-installed.
#[tokio::test]
async fn download_and_unzip_7z_brings_7z_exe_into_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    download_and_unzip_7z(tmp.path())
        .await
        .expect("download + unzip should succeed");
    let exe = tmp.path().join("7z").join("7z.exe");
    let dll = tmp.path().join("7z").join("7z.dll");
    assert!(exe.exists(), "7z.exe missing at {}", exe.display());
    assert!(dll.exists(), "7z.dll missing at {}", dll.display());
    // the zip itself should have been cleaned up.
    assert!(
        !tmp.path().join("7z.zip").exists(),
        "7z.zip should be removed after unzip"
    );
}
