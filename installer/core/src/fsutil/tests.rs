use super::*;
use anyhow::Result;

#[test]
fn delete_file_idempotent_and_real() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let f = tmp.path().join("a.txt");
    // 不存在 = Ok（幂等）。
    delete_file(&f)?;
    std::fs::write(&f, b"x")?;
    delete_file(&f)?;
    assert!(!f.exists());
    // 二次删仍 Ok。
    delete_file(&f)?;
    Ok(())
}

#[test]
fn remove_dir_all_recursive_and_idempotent() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("tree");
    std::fs::create_dir_all(root.join("a/b/c"))?;
    std::fs::write(root.join("a/b/c/f.txt"), b"data")?;
    remove_dir_all(&root)?;
    assert!(!root.exists());
    // 不存在 = Ok（幂等）。
    remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn prune_empty_dirs_stops_at_non_empty_and_boundary() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("root");
    let deep = root.join("l1/l2/l3");
    std::fs::create_dir_all(&deep)?;
    // 全链剪除，止于 root（边界不含）。
    prune_empty_dirs(&deep, &root);
    assert!(!root.join("l1").exists());
    assert!(root.exists(), "边界目录保留");

    // 非空目录剪不动。
    let deep2 = root.join("m1/m2");
    std::fs::create_dir_all(&deep2)?;
    std::fs::write(root.join("m1/keep.txt"), b"x")?;
    prune_empty_dirs(&deep2, &root);
    assert!(root.join("m1").exists(), "非空目录保留");
    assert!(!root.join("m1/m2").exists(), "空子目录剪除");
    Ok(())
}

#[test]
fn prune_ignores_out_of_boundary() -> Result<()> {
    // start 不在 stop_at 子树内 = 直接返回（不越界删）。
    let tmp = tempfile::tempdir()?;
    let outside = tmp.path().join("outside");
    let boundary = tmp.path().join("boundary");
    std::fs::create_dir_all(&outside)?;
    std::fs::create_dir_all(&boundary)?;
    prune_empty_dirs(&outside, &boundary);
    assert!(outside.exists());
    Ok(())
}
