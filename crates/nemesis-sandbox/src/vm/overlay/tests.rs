//! [`overlay`] 单测：参数装配纯函数断言（CI 必跑）+ qemu-img 实跑回归
//! （环境有 qemu-img 才跑，缺席诚实 SKIP——外部工具探针纪律）。

use std::path::{Path, PathBuf};

use super::*;

fn backing(tmp: &tempfile::TempDir) -> PathBuf {
    let p = tmp.path().join("base.qcow2");
    std::fs::write(&p, b"fake-qcow2-payload").expect("write backing stub");
    p
}

/// 参数表形态：-b 拿绝对 backing、-F qcow2、尺寸带 G 后缀。
#[test]
fn create_overlay_args_shape() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let b = backing(&tmp);
    let overlay = tmp.path().join("agents/a1.qcow2");
    let args = create_overlay_args(Path::new("qemu-img"), &overlay, &b, 20).expect("args");

    assert_eq!(args[0], "qemu-img");
    assert_eq!(args[1], "create");
    let bi = args.iter().position(|a| a == "-b").expect("-b 在位");
    let expected_abs = std::fs::canonicalize(&b).expect("canonicalize");
    #[cfg(target_os = "windows")]
    let expected_abs = {
        let s = expected_abs.as_os_str().to_string_lossy().to_string();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
    };
    assert_eq!(
        args[bi + 1],
        expected_abs.display().to_string(),
        "backing 绝对化"
    );
    assert_eq!(args[bi + 2], "-F");
    assert_eq!(args[bi + 3], "qcow2");
    assert_eq!(args.last().unwrap(), "20G");
    assert!(args.contains(&overlay.display().to_string()));
}

/// backing 缺席 = 诚实错误（不产参数、不留给 qemu-img 报模糊错）。
#[test]
fn missing_backing_refused_honestly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let err = create_overlay_args(
        Path::new("qemu-img"),
        &tmp.path().join("o.qcow2"),
        &tmp.path().join("no-such-base.qcow2"),
        20,
    )
    .unwrap_err();
    assert!(err.contains("backing 文件不存在"), "{err}");
}

/// 快照树预留点：backing 指向另一个 overlay 也照常装配（多层 backing 链）。
#[test]
fn overlay_as_backing_allowed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let b = backing(&tmp);
    let mid = tmp.path().join("mid.qcow2");
    let args = create_overlay_args(Path::new("qemu-img"), &mid, &b, 20).expect("mid ok");
    assert_eq!(args.last().unwrap(), "20G");
    let leaf = tmp.path().join("leaf.qcow2");
    let args = create_overlay_args(Path::new("qemu-img"), &leaf, &mid, 20);
    // mid 不存在 → 拒绝（存在性闸对任意 backing 层一致）。
    assert!(args.is_err());
    std::fs::write(&mid, b"stub").unwrap();
    let args = create_overlay_args(Path::new("qemu-img"), &leaf, &mid, 20).expect("leaf ok");
    // -b 值 = canonicalize 后的 mid（精确串随平台/8.3 短名漂移，断言尾部锚定）。
    let bi = args.iter().position(|a| a == "-b").expect("-b 在位");
    assert!(
        args[bi + 1].ends_with("mid.qcow2"),
        "-b 应指向 mid: {}",
        args[bi + 1]
    );
}

/// qemu-img 实跑回归：环境在场才跑（trivial 探针先核实可用），缺席 SKIP。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_qemu_img_roundtrip_when_available() {
    let probe = tokio::process::Command::new("qemu-img")
        .arg("--version")
        .output()
        .await;
    let probe = match probe {
        Ok(o) if o.status.success() => o,
        _ => {
            eprintln!("SKIP: qemu-img 不可用（本环境未安装/不可执行）");
            return;
        }
    };
    let _ = probe;
    let tmp = tempfile::tempdir().expect("tempdir");
    // backing 用 qemu-img 真造（新版 qemu-img create -b 立即验证 backing
    // 是合法 qcow2；假 payload stub 会被拒——zoo 机 QEMU 11 实证。测试
    // 不得依赖旧版宽容行为）。
    let b = tmp.path().join("base.qcow2");
    let mk = tokio::process::Command::new("qemu-img")
        .args(["create", "-f", "qcow2"])
        .arg(&b)
        .arg("1G")
        .output()
        .await
        .expect("qemu-img create backing");
    assert!(
        mk.status.success(),
        "backing 真造失败: {}",
        String::from_utf8_lossy(&mk.stderr)
    );
    let overlay = tmp.path().join("nested/a1.qcow2");
    create_overlay(Path::new("qemu-img"), &overlay, &b, 1)
        .await
        .expect("真实 create 应成功");
    // 产物在位且非空（qcow2 头部 magic）。
    let bytes = std::fs::read(&overlay).expect("overlay produced");
    assert!(bytes.len() > 16);
    assert_eq!(&bytes[0..4], b"QFI\xfb", "qcow2 magic");
}
