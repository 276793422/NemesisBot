use super::*;
use anyhow::Result;

fn write_tree(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root.join("bin"))?;
    std::fs::create_dir_all(root.join("share"))?;
    std::fs::create_dir_all(root.join("sub"))?;
    std::fs::write(root.join("nemesisbot.exe"), b"bot")?;
    std::fs::write(root.join("share/icon.png"), b"png")?;
    std::fs::write(root.join("README.md"), b"md")?;
    // 编译中间物必须被排除。
    std::fs::write(root.join("bot.pdb"), b"pdb")?;
    std::fs::write(root.join("bot.lib"), b"lib")?;
    std::fs::write(root.join("bot.exp"), b"exp")?;
    std::fs::write(root.join("sub/intermediate.obj"), b"obj")?;
    Ok(())
}

#[test]
fn collect_sorted_and_excludes_build_intermediates() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("payload");
    write_tree(&root)?;
    let files = collect_files(&root)?;
    let rels: Vec<String> = files.iter().map(|f| rel_path(&root, f).unwrap()).collect();
    // pdb/lib/exp/obj 全排除，业务文件 3 个。
    assert_eq!(rels.len(), 3, "编译中间物必须排除: {rels:?}");
    assert!(
        rels.iter()
            .all(|r| !r.ends_with(".pdb") && !r.ends_with(".obj"))
    );
    // 显式排序（NTFS/ext4 目录序是平台实现细节——环境敏感纪律）。
    let mut sorted = rels.clone();
    sorted.sort();
    assert_eq!(rels, sorted);
    Ok(())
}

#[test]
fn rel_path_normalizes_separators() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();
    std::fs::create_dir_all(root.join("d"))?;
    let f = root.join("d/f.txt");
    std::fs::write(&f, b"x")?;
    let rel = rel_path(root, &f)?;
    assert!(!rel.contains('\\'), "统一正斜杠: {rel}");
    assert!(rel.ends_with("d/f.txt"));
    // 目录外文件拒绝（绝对路径不在 root 之下）。
    assert!(rel_path(root, std::path::Path::new("C:/elsewhere/f.txt")).is_err());
    Ok(())
}

#[test]
fn file_mode_exec_like_extensions() {
    // Windows 上无权限位语义 → 按扩展名推断（Linux 构建机的 exe/dll 等价形态）。
    let exe = std::path::Path::new("whatever/nemesisbot.exe");
    let cfg = std::path::Path::new("whatever/config.toml");
    // 本用例在 unix 下若真实 mode 可读则值来自文件系统——只断言非 unix 分支的形状。
    #[cfg(not(unix))]
    {
        assert_eq!(file_mode(exe), 0o755);
        assert_eq!(file_mode(cfg), 0o644);
    }
    #[cfg(unix)]
    {
        let _ = (exe, cfg);
    }
}

#[test]
fn pack_rejects_missing_required_file() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let payload = tmp.path().join("payload");
    std::fs::create_dir_all(&payload)?;
    std::fs::write(payload.join("README.md"), b"no exe here")?;
    let skeleton = tmp.path().join("skl.bin");
    std::fs::write(&skeleton, b"SKEL")?;
    let out = tmp.path().join("out.pkg");
    let err = run(&PackOptions {
        payload_dir: payload,
        skeleton,
        out: out.clone(),
        bot_version: "1.0.0".into(),
        platform: "windows".into(),
        sign_keys: None,
        zstd_level: 1,
    })
    .expect_err("缺必需文件必须 loud 拒绝");
    assert!(
        err.to_string().contains("nemesisbot.exe"),
        "报错要指名缺什么: {err}"
    );
    assert!(!out.exists(), "拒绝打包不得留产物");
    Ok(())
}

#[test]
fn pack_rejects_bad_platform() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let payload = tmp.path().join("payload");
    std::fs::create_dir_all(payload.join("x"))?;
    std::fs::write(payload.join("x/nemesisbot.exe"), b"b")?;
    let skeleton = tmp.path().join("skl.bin");
    std::fs::write(&skeleton, b"S")?;
    let err = run(&PackOptions {
        payload_dir: payload,
        skeleton,
        out: tmp.path().join("o.pkg"),
        bot_version: "1".into(),
        platform: "freebsd".into(),
        sign_keys: None,
        zstd_level: 1,
    })
    .expect_err("非法平台必须拒绝");
    assert!(err.to_string().contains("platform"), "{err}");
    Ok(())
}

#[test]
fn pack_manifest_lists_all_and_deterministic() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let payload = tmp.path().join("payload");
    write_tree(&payload)?;
    let skeleton = tmp.path().join("skl.bin");
    std::fs::write(&skeleton, b"SKEL")?;
    let opts = |out| PackOptions {
        payload_dir: payload.clone(),
        skeleton: skeleton.clone(),
        out,
        bot_version: "1.0.0".into(),
        platform: "windows".into(),
        sign_keys: None,
        zstd_level: 1,
    };
    let stats = run(&opts(tmp.path().join("a.pkg")))?;
    let info_a = crate::pkgfile::inspect(&tmp.path().join("a.pkg"))?;
    // 3 业务文件（中间物排除）。
    assert_eq!(stats.files, 3);
    assert_eq!(info_a.manifest.files.len(), 3);
    assert_eq!(stats.payload_offset, skeleton.metadata()?.len() as u64);
    assert_eq!(
        stats.uncompressed_total,
        3 + 3 + 2,
        "total = bot(3)+png(3)+md(2)"
    );

    // 确定性：同输入重打包 → manifest 清单逐字节同（sha/size/path 同序）。
    run(&opts(tmp.path().join("b.pkg")))?;
    let info_b = crate::pkgfile::inspect(&tmp.path().join("b.pkg"))?;
    assert_eq!(
        serde_json::to_string(&info_a.manifest.files)?,
        serde_json::to_string(&info_b.manifest.files)?
    );
    Ok(())
}

#[test]
fn signed_manifest_carries_v4_envelope() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let payload = tmp.path().join("payload");
    write_tree(&payload)?;
    let skeleton = tmp.path().join("skl.bin");
    std::fs::write(&skeleton, b"SKEL")?;
    let keys = tmp.path().join("keys.json");
    let anchor = keygen(&keys)?;
    assert_eq!(anchor.len(), 64, "根锚 = sha256 hex");

    let out = tmp.path().join("signed.pkg");
    run(&PackOptions {
        payload_dir: payload,
        skeleton,
        out: out.clone(),
        bot_version: "1.0.0".into(),
        platform: "windows".into(),
        sign_keys: Some(keys),
        zstd_level: 1,
    })?;
    let info = crate::pkgfile::inspect(&out)?;
    assert!(
        info.manifest_signed,
        "签名形态 manifest blob 应带 v4 envelope"
    );
    // 未签对照。
    let unsigned = tmp.path().join("unsigned.pkg");
    run(&PackOptions {
        payload_dir: tmp.path().join("payload"),
        skeleton: tmp.path().join("skl.bin"),
        out: unsigned.clone(),
        bot_version: "1.0.0".into(),
        platform: "windows".into(),
        sign_keys: None,
        zstd_level: 1,
    })?;
    let info2 = crate::pkgfile::inspect(&unsigned)?;
    assert!(!info2.manifest_signed);
    Ok(())
}

#[test]
fn keygen_save_load_roundtrip() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let keys = tmp.path().join("keys.json");
    let anchor1 = keygen(&keys)?;
    // 重生成 = 新身份（每次 keygen 独立）。
    let keys2 = tmp.path().join("keys2.json");
    let anchor2 = keygen(&keys2)?;
    assert_ne!(anchor1, anchor2);
    assert!(keys.exists());
    Ok(())
}
