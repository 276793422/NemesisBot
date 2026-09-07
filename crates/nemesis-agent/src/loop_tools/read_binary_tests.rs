//! A8 (2026-09-04 devtool-upgrade): read_file 二进制/PDF 分支——非 UTF-8
//! 文件按 magic byte 分类诚实摘要（图片提示 vision 附加正道 / PDF 说明 /
//! 其他报字节数），文本路径行为不变（计划验收：png/pdf/exe/文本四类）。

use super::*;
use crate::context::RequestContext;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// args 构造单一入口（Windows 路径反斜杠必须 JSON 转义——手拼 format!
/// 会被 serde_json 拒绝走 raw-path fallback，整个 JSON 变成 path）。
fn args(path: &std::path::Path, extra: serde_json::Value) -> String {
    let mut v = serde_json::json!({ "path": path.display().to_string() });
    if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in extra {
            obj.insert(k.clone(), val.clone());
        }
    }
    v.to_string()
}

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "nemesis_test_read_bin_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn text_file_reads_normally() {
    let dir = TempDir::new("text");
    let p = dir.0.join("note.txt");
    std::fs::write(&p, "hello 世界\nsecond line\n").unwrap();
    let out = ReadFileTool
        .execute(&args(&p, serde_json::json!({})), &ctx())
        .await
        .unwrap();
    assert_eq!(out, "hello 世界\nsecond line\n", "文本路径字节不变");
}

#[tokio::test]
async fn png_gets_vision_attach_hint() {
    let dir = TempDir::new("png");
    let p = dir.0.join("pic.png");
    // PNG magic + 任意 body（read_to_string 必 InvalidData）。
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&[0xFF, 0xEE, 0xDD]);
    std::fs::write(&p, &bytes).unwrap();
    let out = ReadFileTool
        .execute(&args(&p, serde_json::json!({})), &ctx())
        .await
        .unwrap();
    assert!(
        out.contains("binary png image file")
            && out.contains("use vision by attaching")
            && out.contains(&format!("{} bytes", bytes.len())),
        "png 应提示 vision 附加正道: {}",
        out
    );
}

#[tokio::test]
async fn pdf_gets_honest_extraction_note() {
    let dir = TempDir::new("pdf");
    let p = dir.0.join("doc.pdf");
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend_from_slice(&[0x00, 0x91, 0xFF]);
    std::fs::write(&p, &bytes).unwrap();
    let out = ReadFileTool
        .execute(&args(&p, serde_json::json!({})), &ctx())
        .await
        .unwrap();
    assert!(
        out.contains("binary PDF file") && out.contains("text extraction is not supported"),
        "pdf 应诚实说明文本提取不支持: {}",
        out
    );
}

#[cfg(windows)]
#[tokio::test]
async fn exe_gets_unknown_binary_summary() {
    let dir = TempDir::new("exe");
    let p = dir.0.join("prog.exe");
    // PE 头：MZ + DOS stub 片段（不在 magic 图片表）。
    let mut bytes = b"MZ\x90\x00\x03\x00\x00\x00\x04\x00\x00\x00".to_vec();
    bytes.extend_from_slice(&[0xCC; 8]);
    std::fs::write(&p, &bytes).unwrap();
    let out = ReadFileTool
        .execute(&args(&p, serde_json::json!({})), &ctx())
        .await
        .unwrap();
    assert!(
        out.contains(&format!("{} bytes binary (type unknown)", bytes.len())),
        "exe 应报字节数 + type unknown: {}",
        out
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn random_binary_gets_unknown_summary() {
    let dir = TempDir::new("rand");
    let p = dir.0.join("blob.bin");
    // 确定性伪随机非 UTF-8 字节，无任何已知 magic。
    let bytes: Vec<u8> = (0..64u32).map(|i| (i * 37 + 11) as u8).collect();
    std::fs::write(&p, &bytes).unwrap();
    let out = ReadFileTool
        .execute(&args(&p, serde_json::json!({})), &ctx())
        .await
        .unwrap();
    assert!(
        out.contains(&format!("{} bytes binary (type unknown)", bytes.len())),
        "无 magic 二进制应报字节数: {}",
        out
    );
}

#[tokio::test]
async fn non_utf8_text_without_magic_reports_unknown() {
    let dir = TempDir::new("latin1");
    let p = dir.0.join("legacy.txt");
    // latin-1 高位字节文本：非 UTF-8 但无二进制 magic。
    let bytes = vec![b'C', b'a', b'f', b'\xe9', b'\n'];
    std::fs::write(&p, &bytes).unwrap();
    let out = ReadFileTool
        .execute(&args(&p, serde_json::json!({})), &ctx())
        .await
        .unwrap();
    assert!(
        out.contains("5 bytes binary (type unknown)"),
        "无 magic 非 UTF-8 文本应诚实报 unknown: {}",
        out
    );
}

#[tokio::test]
async fn binary_summary_ignores_offset_limit() {
    let dir = TempDir::new("offset");
    let p = dir.0.join("pic.jpg");
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
    bytes.extend_from_slice(&[0x00; 16]);
    std::fs::write(&p, &bytes).unwrap();
    let out = ReadFileTool
        .execute(
            &args(&p, serde_json::json!({ "offset": 0, "limit": 10 })),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(
        out.contains("binary jpg image file"),
        "二进制文件不进分段路径（分段只对文本有意义）: {}",
        out
    );
}

#[tokio::test]
async fn missing_file_still_errors() {
    let out = ReadFileTool
        .execute("{\"path\": \"Z:/definitely/not/here.txt\"}", &ctx())
        .await;
    assert!(out.is_err(), "不存在文件照常 Err");
}
