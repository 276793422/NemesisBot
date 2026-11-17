//! `load_browser_prefs` / `parse_browser_prefs` 覆盖（BUG-20261116-044）：
//! config.json 带 UTF-8 BOM（记事本「带 BOM 的 UTF-8」保存形态）时 browser
//! 段仍须解析成功——裸 `read_to_string` 会 parse 失败 → 用户 browser 配置
//! 被静默忽略（读盘半边走 `fsutil::read_config_string` 剥 BOM）。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// 测试隔离目录：仓库内 `target/nb044-test/<pid>-<seq>`。
///
/// 刻意不用 `%TEMP%` + tempfile：本机 TMP 路径是 8.3 短名形态
/// （`C:\Users\LOCALA~1\...`），nemesisbot 测试二进制进程内对它
/// 偶发 ERROR_PATH_NOT_FOUND（失败点在 tempdir / fs::write 间漂移，
/// bash 同路径 100/100 成功——机器特有 native 干扰，与 monitor DLL
/// 破坏 dev-shape 前提同族）。仓库内 target/ 无短名/ACL/清扫者干扰，
/// 测试关注点是 BOM 解析不是临时目录基建。
struct TempHome(PathBuf);

static SEQ: AtomicU32 = AtomicU32::new(0);

impl TempHome {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("nb044-test")
            .join(format!("{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir under target/");
        TempHome(dir)
    }
    fn config_json(&self, content: &str) -> PathBuf {
        let p = self.0.join("config.json");
        std::fs::write(&p, content).expect("write config.json");
        p
    }
}
impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// BOM 前缀 config.json：browser 段完整解析（核心回归锁）。
#[test]
fn browser_prefs_bom_config_still_parsed() {
    let t = TempHome::new();
    let p = t.config_json(
        "\u{feff}{\"browser\":{\"kind\":\"firefox\",\"executable\":\"C:/ff/firefox.exe\"}}",
    );
    let prefs = load_browser_prefs(&p);
    assert_eq!(prefs.kind, "firefox");
    assert_eq!(prefs.executable, "C:/ff/firefox.exe");
}

/// 无 BOM 合法 config.json：行为不变（读盘换 helper 不破既有形态）。
#[test]
fn browser_prefs_plain_config_parsed() {
    let t = TempHome::new();
    let p = t.config_json(r#"{"browser":{"kind":"edge"}}"#);
    let prefs = load_browser_prefs(&p);
    assert_eq!(prefs.kind, "edge");
}

/// browser 段缺席 / 文件缺失 / JSON 损坏 → 全默认（保底语义不变）。
#[test]
fn browser_prefs_defaults_preserved() {
    let t = TempHome::new();
    // 段缺席
    let p = t.config_json(r#"{"gateway":{"x":1}}"#);
    let prefs = load_browser_prefs(&p);
    assert!(prefs.kind.is_empty() && prefs.executable.is_empty());
    // 文件缺失
    let prefs = load_browser_prefs(&t.0.join("nope/config.json"));
    assert!(prefs.kind.is_empty() && prefs.executable.is_empty());
    // JSON 损坏
    let p = t.config_json("{not json");
    let prefs = load_browser_prefs(&p);
    assert!(prefs.kind.is_empty() && prefs.executable.is_empty());
}

/// 解析半边（parse_browser_prefs）独立验证：BOM 只在文件层，纯文本层
/// BOM 前缀 parse 失败 → 默认（剥 BOM 的职责在读盘半边）。
#[test]
fn parse_browser_prefs_raw_bom_text_falls_back_to_default() {
    let prefs = parse_browser_prefs("\u{feff}{\"browser\":{\"kind\":\"firefox\"}}");
    assert!(prefs.kind.is_empty() && prefs.executable.is_empty());
}
