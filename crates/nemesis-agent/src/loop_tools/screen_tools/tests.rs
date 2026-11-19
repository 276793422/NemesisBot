// screen_tools 适配器测试（extract_saved_screenshot_path 纯函数面）。

use super::*;

#[test]
fn extract_parses_screen_capture_text() {
    let tmp = std::env::temp_dir().join(format!("st_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let p = tmp.join("shot.png");
    std::fs::write(&p, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
    let text = format!(
        "Screenshot saved to {} (size: 8 bytes, info: OK)",
        p.display()
    );
    assert_eq!(
        extract_saved_screenshot_path(&text),
        Some(p.to_string_lossy().to_string())
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn extract_rejects_missing_file() {
    let text = "Screenshot saved to C:\\definitely\\missing\\x.png (size: 1 bytes)";
    assert_eq!(extract_saved_screenshot_path(text), None);
}

#[test]
fn extract_rejects_non_image_content() {
    let tmp = std::env::temp_dir().join(format!("st2_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let p = tmp.join("fake.png");
    std::fs::write(&p, b"this is not an image at all").unwrap();
    let text = format!("Screenshot saved to {}\nmore", p.display());
    assert_eq!(extract_saved_screenshot_path(&text), None);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn extract_none_without_marker() {
    assert_eq!(extract_saved_screenshot_path("Found 3 window(s)"), None);
}
