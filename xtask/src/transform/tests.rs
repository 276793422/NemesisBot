use super::*;

fn manifest_with_stub() -> String {
    format!(
        "# nemesisbot manifest（节选形态）\n\
         [dependencies]\n\
         nemesis-path = {{ path = \"../crates/nemesis-path\" }}\n\
         {STUB_DEP}\n\
         nemesis-board = {{ workspace = true }}\n"
    )
}

#[test]
fn swap_replaces_exactly_one_line() {
    let out = swap(&manifest_with_stub()).expect("换指");
    assert_eq!(out.matches(STUB_DEP).count(), 0, "stub 行必须消失");
    assert_eq!(out.matches(IMPL_DEP).count(), 1, "实现行恰 1 处");
    // 邻行原样保留
    assert!(out.contains("nemesis-path = { path = \"../crates/nemesis-path\" }"));
    assert!(out.contains("nemesis-board = { workspace = true }"));
}

#[test]
fn swap_rejects_zero_hits() {
    let drifted = manifest_with_stub().replace("../slots/taiwei\"", "../slots/taiwei-renamed\"");
    assert!(swap(&drifted).is_err(), "措辞漂移必须 loud 拒绝");
}

#[test]
fn swap_rejects_already_impl_form() {
    let already = manifest_with_stub().replace(STUB_DEP, IMPL_DEP);
    assert!(
        swap(&already).is_err(),
        "已换指形态（0 命中）必须拒绝，不做幂等吞并"
    );
}

#[test]
fn swap_rejects_multiple_hits() {
    let twice = manifest_with_stub().replace(
        "nemesis-board = { workspace = true }",
        &format!("{STUB_DEP}\nnemesis-board = {{ workspace = true }}"),
    );
    assert!(swap(&twice).is_err(), "多命中必须拒绝");
}

#[test]
fn apply_to_file_roundtrip_on_temp_manifest() {
    let dir = std::env::temp_dir().join(format!(
        "xtask-transform-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let path = dir.join("Cargo.toml");
    std::fs::write(&path, manifest_with_stub()).expect("种子");

    apply_to_file(&path).expect("换指落盘");
    let after = std::fs::read_to_string(&path).expect("读回");
    assert!(after.contains(IMPL_DEP));
    assert!(!after.contains(STUB_DEP));
}
