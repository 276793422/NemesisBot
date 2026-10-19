//! D6.2 远程清单源测试：本地 git 源全链（add-source → search → install）
//! + 装配漏斗语义（unsigned 闸 / verified 直通 / 篡改拒装 / 目标冲突拒）。

use super::*;
use std::path::Path;

/// 外部工具纪律：git 二进制在场 ≠ 可用——先实跑 trivial 探针，不可用诚实 SKIP。
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 建一个本地「远程」git 仓库：connectors/<id>/connector.toml（+ 可选 sig）
/// 提交到 main 分支，返回可 clone 的路径（file 形态）。
fn seed_remote_repo(
    dir: &Path,
    connector_id: &str,
    toml_text: &str,
    sig_json: Option<&str>,
) -> String {
    let conn_dir = dir.join("connectors").join(connector_id);
    std::fs::create_dir_all(&conn_dir).expect("mkdir");
    std::fs::write(conn_dir.join(MANIFEST_FILE), toml_text).expect("write toml");
    if let Some(sig) = sig_json {
        std::fs::write(conn_dir.join(SIG_FILE), sig).expect("write sig");
    }
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git spawn");
        assert!(
            status.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    // 双保险：本机 autocrlf 设置不进测试——blob 恒 LF，与签名字节一致。
    run(&["config", "core.autocrlf", "false"]);
    run(&["add", "."]);
    run(&["commit", "-m", "seed"]);
    dir.to_string_lossy().replace('\\', "/")
}

const SAMPLE_TOML: &str = "[connector]\nid = \"remote-1\"\nname = \"Remote One\"\nversion = \"0.1.0\"\nkind = \"local\"\n\n[[op]]\nid = \"ping\"\nrisk = \"low\"\nclass = \"read\"\naction = \"builtin.demo\"\n";

#[test]
fn test_derive_source_name() {
    assert_eq!(
        derive_source_name("https://github.com/x/Y.git").unwrap(),
        "y"
    );
    assert_eq!(
        derive_source_name("https://example.com/repos/connectors-fed/").unwrap(),
        "connectors-fed"
    );
    assert_eq!(
        derive_source_name("/local/path/my_repo").unwrap(),
        "my_repo"
    );
    // 非法 charset（空格）与空名（协议尾巴）拒。
    assert!(
        derive_source_name("https://example.com/a b").is_err(),
        "含空格拒"
    );
    assert!(derive_source_name("https://").is_err(), "空名拒");
}

#[test]
fn test_add_source_dup_and_remove() {
    if !git_available() {
        eprintln!("SKIP: git 不可用（探针失败）");
        return;
    }
    let ws = tempfile::tempdir().expect("ws");
    let remote = tempfile::tempdir().expect("remote");
    let url = seed_remote_repo(remote.path(), "remote-1", SAMPLE_TOML, None);
    let s = add_source(ws.path(), &url, Some("dup-test"), None).expect("add");
    assert_eq!(s.branch, "main");
    assert!(sources_path(ws.path()).exists(), "源注册表应落 config/");
    // 同名同 URL = 拉取更新（非报错）。
    let s2 = add_source(ws.path(), &url, Some("dup-test"), None).expect("同名同 URL 拉取");
    assert_eq!(s2.url, s.url);
    // 同名异 URL = 拒。
    let err = add_source(
        ws.path(),
        "https://example.com/other.git",
        Some("dup-test"),
        None,
    )
    .expect_err("同名异 URL 拒");
    assert!(err.to_string().contains("URL 不同"), "实际: {err}");

    assert!(remove_source(ws.path(), "dup-test").expect("remove"));
    assert!(
        !remove_source(ws.path(), "dup-test").expect("remove again"),
        "二删 false"
    );
    // 缓存目录保留（诚实注明，不静默删）。
    assert!(cache_dir(ws.path(), "dup-test").join(".git").exists());
}

#[test]
fn test_search_and_install_unsigned_gate() {
    if !git_available() {
        eprintln!("SKIP: git 不可用（探针失败）");
        return;
    }
    let ws = tempfile::tempdir().expect("ws");
    let remote = tempfile::tempdir().expect("remote");
    let url = seed_remote_repo(remote.path(), "remote-1", SAMPLE_TOML, None);
    add_source(ws.path(), &url, Some("src"), None).expect("add");

    // search：命中 + 徽标 unsigned。
    let hits = search_sources(ws.path(), "remote-1", None).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].connector.as_ref().expect("parsed").id, "remote-1");
    assert_eq!(hits[0].trust, ConnectorTrust::Unsigned);
    // 空 query = 列全部。
    assert_eq!(
        search_sources(ws.path(), "", None).expect("search").len(),
        1
    );

    // install：unsigned 无旗标 → 拒。
    let err = install_connector(ws.path(), "src", "remote-1", false).expect_err("unsigned 拒");
    assert!(err.to_string().contains("--allow-unsigned"), "实际: {err}");

    // 带旗标 → 装上 + 只拷清单 + 回读一致。
    let report = install_connector(ws.path(), "src", "remote-1", true).expect("install");
    assert_eq!(report.trust, ConnectorTrust::Unsigned);
    assert_eq!(report.copied, vec![MANIFEST_FILE.to_string()]);
    let found = nemesis_connectors::registry::find_connector(ws.path(), "remote-1")
        .expect("装完可用")
        .0;
    assert_eq!(found.id, "remote-1");

    // 目标冲突 → 拒（不覆盖）。
    let err = install_connector(ws.path(), "src", "remote-1", true).expect_err("冲突拒");
    assert!(err.to_string().contains("已有 connector"), "实际: {err}");
}

#[test]
fn test_install_signed_verified_and_tamper_rejected() {
    if !git_available() {
        eprintln!("SKIP: git 不可用（探针失败）");
        return;
    }
    let ws = tempfile::tempdir().expect("ws");
    let remote = tempfile::tempdir().expect("remote");
    let dir = remote.path();
    // 先在 remote repo 里签好再提交（sig 文件随 clone 走）。
    let pair = nemesis_connectors::signing::generate_key_pair().expect("keygen");
    let conn_dir = dir.join("connectors").join("signed-1");
    std::fs::create_dir_all(&conn_dir).expect("mkdir");
    std::fs::write(conn_dir.join(MANIFEST_FILE), SAMPLE_TOML).expect("toml");
    let pubkey = nemesis_connectors::signing::sign_connector_dir(&conn_dir, &pair.private_key)
        .expect("sign");
    let url = seed_remote_repo(dir, "signed-1", SAMPLE_TOML, None); // seed 重写 toml（同文）+ 提交 sig
    add_source(ws.path(), &url, Some("signed-src"), None).expect("add");

    // 信任库先 trust 该公钥（workspace 共享 skill_trust.json）。
    let store = workspace_trust_store(ws.path());
    use nemesis_connectors::signing::TrustLevel;
    store.add_key(&pubkey, "remote-signer", TrustLevel::Verified);

    // install：verified 直通（无旗标）。注意清单 id 是 remote-1（目录名只是存放位）。
    let report = install_connector(ws.path(), "signed-src", "remote-1", false).expect("install");
    assert_eq!(report.trust, ConnectorTrust::Verified);
    assert!(report.warnings.is_empty());
    assert_eq!(
        report.copied,
        vec![MANIFEST_FILE.to_string(), SIG_FILE.to_string()],
        "签名文件应随清单落位"
    );
    let target_trust =
        assess_connector_signature(&ws.path().join("connectors").join("remote-1"), &store)
            .expect("目标验签");
    assert_eq!(target_trust.trust, ConnectorTrust::Verified);

    // 篡改源仓清单 → 下一次 clone 拉到的就是坏清单 → scan corrupt + install 拒。
    std::fs::write(
        conn_dir.join(MANIFEST_FILE),
        SAMPLE_TOML.replace("risk = \"low\"", "risk = \"critical\""),
    )
    .expect("tamper");
    let status = std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .expect("git add");
    assert!(status.status.success());
    let status = std::process::Command::new("git")
        .args(["commit", "-m", "tamper"])
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .current_dir(dir)
        .output()
        .expect("git commit");
    assert!(status.status.success());
    // 拉取更新（同名同 URL = pull 语义，fetch+reset 拿到篡改后清单）。
    add_source(ws.path(), &url, Some("signed-src"), None).expect("pull");
    let hits = scan_source(ws.path(), "signed-src").expect("scan");
    let bad = hits
        .iter()
        .find(|h| h.error.is_some())
        .expect("corrupt 条目");
    assert!(bad.error.as_deref().unwrap().contains("签名验证失败"));
    // 篡改形态在 scan 已被滤出 connector=None → install 找不到可装条目。
    let err = install_connector(ws.path(), "signed-src", "remote-1", false).expect_err("篡改拒装");
    assert!(err.to_string().contains("找不到"), "实际: {err}");
}

#[test]
fn test_scan_missing_cache_is_honest_error() {
    let ws = tempfile::tempdir().expect("ws");
    let err = scan_source(ws.path(), "ghost").expect_err("缺缓存应报");
    assert!(err.to_string().contains("add-source"), "实际: {err}");
}
