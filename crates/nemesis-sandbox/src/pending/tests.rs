use super::*;
use tempfile::TempDir;

/// Build a temp box tree with one mirrored file: `<box>/drive/C/tmp/a.txt`.
fn one_pending() -> (TempDir, PendingFile) {
    let tmp = TempDir::new().unwrap();
    let box_root = tmp.path().to_path_buf();
    let file = box_root.join("drive").join("C").join("tmp").join("a.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"hello").unwrap();
    let pf = PendingFile {
        box_path: file,
        real_path: PathBuf::from(r"C:\tmp\a.txt"),
        size: 5,
    };
    (tmp, pf)
}

#[test]
fn delete_file_removes_box_file() {
    let (_tmp, pf) = one_pending();
    assert!(pf.box_path.exists());
    assert_eq!(delete_file(&pf).unwrap(), true);
    assert!(!pf.box_path.exists());
}

#[test]
fn delete_file_already_gone_is_false() {
    let (_tmp, pf) = one_pending();
    std::fs::remove_file(&pf.box_path).unwrap();
    // Already absent → Ok(false), NOT an error.
    assert_eq!(delete_file(&pf).unwrap(), false);
}

#[test]
fn delete_file_never_touches_real_path() {
    // Real path doesn't even exist in this temp setup — deleting the box
    // file must not create or modify anything at the real path.
    let (_tmp, pf) = one_pending();
    assert!(!pf.real_path.exists());
    assert_eq!(delete_file(&pf).unwrap(), true);
    assert!(!pf.real_path.exists());
}
