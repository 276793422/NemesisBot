//! board_evidence_read 单测：入参解析 / 档案围栏读（路径安全 + 分段 + 二进制
//! 诚实）/ issue→档案根解析链。纯函数直测（read_archive_file /
//! parse_evidence_args），store 链路一条集成测（真实 BoardStore + 临时项目
//! 目录）。

use super::*;
use nemesis_agent::r#loop::Tool;

// ---------- parse_evidence_args ----------

#[test]
fn parse_requires_issue_and_path() {
    assert!(parse_evidence_args("{}").is_err());
    assert!(parse_evidence_args(r#"{"issue":"NB-1"}"#).is_err());
    assert!(parse_evidence_args(r#"{"path":"docs/a.md"}"#).is_err());
    let ok = parse_evidence_args(r#"{"issue":"NB-1","path":"docs/a.md"}"#).unwrap();
    assert_eq!(ok.issue, "NB-1");
    assert_eq!(ok.path, "docs/a.md");
    assert_eq!(ok.offset, None);
    assert_eq!(ok.limit, None);
    // 非法 JSON / 非整数 offset 诚实报错。
    assert!(parse_evidence_args("not-json").is_err());
    assert!(parse_evidence_args(r#"{"issue":"NB-1","path":"a","offset":-1}"#).is_err());
    assert!(parse_evidence_args(r#"{"issue":"NB-1","path":"a","offset":"x"}"#).is_err());
}

// ---------- read_archive_file：路径安全（与锚点同闸） ----------

fn fixture_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "evidence-tool-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn read_rejects_escape_and_malformed_paths() {
    let root = fixture_root("escape");
    std::fs::write(root.join("a.md"), "# x\n").unwrap();
    let secret = root
        .parent()
        .unwrap()
        .join(format!("secret-{}.txt", std::process::id()));
    std::fs::write(&secret, "top secret").unwrap();

    // `..` 穿越（形态闸拒）。
    let err = read_archive_file(&root, "../evil.md", None, None).unwrap_err();
    assert!(err.contains("拒绝"), "{err}");
    // 绝对路径（形态闸拒）。
    let err = read_archive_file(&root, &secret.display().to_string(), None, None).unwrap_err();
    assert!(err.contains("拒绝"), "{err}");
    // 不存在（canonicalize 诚实报错）。
    let err = read_archive_file(&root, "missing.md", None, None).unwrap_err();
    assert!(err.contains("不存在或不可达"), "{err}");
    // 根外同盘路径经 canonicalize 前缀校验拒绝（先手工放一个符号外文件：
    // 用父目录真实存在的文件走形态合法路径不可行——形态闸已拦 `..`，此臂
    // 由 anchor 套件的 junction 用例覆盖，这里钉错误文案即可）。
    let _ = std::fs::remove_file(&secret);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn read_rejects_83_short_name_components() {
    let root = fixture_root("shortname");
    std::fs::write(root.join("README~1.md"), "x").unwrap();
    let err = read_archive_file(&root, "README~1.md", None, None).unwrap_err();
    assert!(err.contains("8.3"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------- read_archive_file：全文 / 分段 / 二进制 ----------

#[test]
fn read_full_text_without_offset() {
    let root = fixture_root("full");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(
        root.join("docs").join("prd.md"),
        "# PRD\n网格建造与拆除。\n",
    )
    .unwrap();
    let out = read_archive_file(&root, "docs/prd.md", None, None).unwrap();
    assert!(out.contains("path=docs/prd.md"), "{out}");
    assert!(out.contains("total_chars="), "{out}");
    assert!(out.contains("网格建造与拆除"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn read_pages_by_chars_multibyte_safe() {
    let root = fixture_root("page");
    let body: String = "需求条目甲乙丙丁。".repeat(100); // 900 chars, multibyte
    std::fs::write(root.join("big.md"), &body).unwrap();
    let out = read_archive_file(&root, "big.md", Some(800), Some(200)).unwrap();
    assert!(out.contains("offset=800 limit=200"), "{out}");
    let slice: String = body.chars().skip(800).take(200).collect();
    assert!(out.ends_with(&slice), "分片须与 chars().skip/take 逐字一致");
    // offset 越界诚实报错。
    let err = read_archive_file(&root, "big.md", Some(99999), None).unwrap_err();
    assert!(err.contains("超出文件长度"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn read_reports_binary_honestly() {
    let root = fixture_root("binary");
    std::fs::write(root.join("model.bin"), [0x00u8, 0x01, 0x02, 0xFF]).unwrap();
    let err = read_archive_file(&root, "model.bin", None, None).unwrap_err();
    assert!(err.contains("不是文本"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------- execute 链路：issue → project → 档案根（真实 store） ----------

#[tokio::test]
async fn execute_resolves_issue_to_project_archive() {
    let dir = std::env::temp_dir().join(format!("evidence-tool-exe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    let archive = dir.join("proj-archive");
    std::fs::create_dir_all(&archive).unwrap();
    let project = store
        .create_project("p", "d", None, "", "", Some(archive.to_str().unwrap()))
        .unwrap();
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "取证链路".to_string(),
            description: String::new(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    std::fs::write(archive.join("delivery.md"), "# 交付\nPRD 全文在场。\n").unwrap();

    let tool = BoardEvidenceTool::new(store.clone());
    let ctx = nemesis_agent::context::RequestContext::new("test", "chat", "tester", "sess");
    let args = format!(r#"{{"issue":"{}","path":"delivery.md"}}"#, issue.number);
    let out = tool.execute(&args, &ctx).await.unwrap();
    assert!(out.contains("PRD 全文在场"), "{out}");

    // 未挂项目的单：诚实报错（无档案可读）。
    let orphan = store
        .create_issue(nemesis_board::NewIssue {
            title: "无项目".to_string(),
            description: String::new(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            project_id: None,
            ..Default::default()
        })
        .unwrap();
    let args = format!(r#"{{"issue":"{}","path":"delivery.md"}}"#, orphan.number);
    let err = tool.execute(&args, &ctx).await.unwrap_err();
    assert!(err.contains("未挂项目"), "{err}");

    // 档案内围栏外路径：拒绝。
    let args = format!(r#"{{"issue":"{}","path":"../outside.md"}}"#, issue.number);
    let err = tool.execute(&args, &ctx).await.unwrap_err();
    assert!(err.contains("拒绝"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}
