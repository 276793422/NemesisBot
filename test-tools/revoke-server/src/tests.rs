//! main.rs 单测：嵌入式页面 handler + CLI 参数解析（main() 本体 bind 端口长驻，
//! 属结构性不可达——见任务报告；此处覆盖其外围可测件）。

use super::*;

#[tokio::test]
async fn index_serves_embedded_html() {
    let Html(page) = index().await;
    assert_eq!(page, INDEX_HTML);
    // 页面真实可用性最低门槛：是 HTML 且非空壳
    assert!(
        page.trim_start()
            .to_ascii_lowercase()
            .starts_with("<!doctype html")
    );
    assert!(page.contains("NemesisBot"));
}

#[tokio::test]
async fn admin_page_serves_embedded_html() {
    let Html(page) = admin_page().await;
    assert_eq!(page, ADMIN_HTML);
    assert!(
        page.trim_start()
            .to_ascii_lowercase()
            .starts_with("<!doctype html")
    );
}

#[test]
fn cli_defaults_match_docs() {
    // 缺省值与 README/CLAUDE.md 文档一致
    let cli = Cli::try_parse_from(["revoke-server"]).unwrap();
    assert_eq!(cli.bind, "127.0.0.1:7878");
    assert_eq!(cli.db_url, "revoke.db");
    assert_eq!(cli.keys_file, "keys.json");
    assert!(!cli.init_keys);
    assert_eq!(cli.admin_token, "admin-token-change-me");
}

#[test]
fn cli_explicit_flags_parsed() {
    let cli = Cli::try_parse_from([
        "revoke-server",
        "--keys-file",
        "/tmp/k.json",
        "--init-keys",
        "--bind",
        "0.0.0.0:9999",
        "--db-url",
        "/tmp/r.db",
        "--admin-token",
        "sekrit",
    ])
    .unwrap();
    assert_eq!(cli.bind, "0.0.0.0:9999");
    assert_eq!(cli.db_url, "/tmp/r.db");
    assert_eq!(cli.keys_file, "/tmp/k.json");
    assert!(cli.init_keys);
    assert_eq!(cli.admin_token, "sekrit");
}

#[test]
fn cli_rejects_unknown_flag() {
    assert!(Cli::try_parse_from(["revoke-server", "--nonsense"]).is_err());
}
