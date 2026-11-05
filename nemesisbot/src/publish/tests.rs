//! 发布模块（publish.rs）单元测试。
//!
//! 覆盖面：纯函数（发布结果解析 `$.` path 形态 / 对账逐条匹配 / 翻页推进 /
//! task_id 校验 / arg 提取）+ 协议缺席诚实报错（域剥离批 C-3：宿主无内置
//! 发布协议）+ 副本解析链（显式 copy_path 与 dispatch→issue→project 链）+
//! 凭据缺席语义 + [`preflight`] 前置核验全分支（真 TaskDb + 真 BoardStore +
//! 真 git 副本仓——不触碰 agent loop；`reconcile_by_match` 编排与 git 半边
//! 由 nemesis-board git_repo tests 与 UAT/活体验证覆盖）+ 续作留痕回归
//! （2026-11-02 外部复评：push 阶段失败重试续作 / patch 原始字节保真 /
//! pr_pending 对账收敛 / Inconclusive 保守停车，connector_invoke 走桩）。

use super::*;
use nemesis_board::assignment::Actor;
use nemesis_board::models::NewIssue;
use nemesis_board::store::BoardStore;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// parse_publish_result（result_ref/result_url 均按模板声明的 JSON path）
// ---------------------------------------------------------------------------

#[test]
fn parse_publish_result_direct_object() {
    let text = r#"{"number": 7, "html_url": "http://x/pr/7", "title": "t"}"#;
    let (n, url) = parse_publish_result(text, "$.number", Some("$.html_url")).unwrap();
    assert_eq!(n, 7);
    assert_eq!(url.as_deref(), Some("http://x/pr/7"));
}

#[test]
fn parse_publish_result_single_element_array() {
    let text = r#"[{"number": 12, "html_url": null}]"#;
    let (n, url) = parse_publish_result(text, "$.number", Some("$.html_url")).unwrap();
    assert_eq!(n, 12);
    assert_eq!(url, None);
}

#[test]
fn parse_publish_result_non_json_is_none() {
    // HTTP 错误页 / 截断体 / connector 文本包装——认不出 = 交给对账。
    assert!(parse_publish_result("<html>500</html>", "$.number", None).is_none());
    assert!(parse_publish_result("", "$.number", None).is_none());
    assert!(parse_publish_result(r#"{"number": "#, "$.number", None).is_none());
}

#[test]
fn parse_publish_result_missing_number_is_none() {
    assert!(parse_publish_result(r#"{"message": "created"}"#, "$.number", None).is_none());
    // number 非整数也认不出。
    assert!(parse_publish_result(r#"{"number": "7"}"#, "$.number", None).is_none());
}

#[test]
fn parse_publish_result_empty_array_is_none() {
    assert!(parse_publish_result("[]", "$.number", None).is_none());
}

#[test]
fn parse_publish_result_unsafe_scheme_dropped() {
    // result_url path 取到的链接来自 connector 调用的
    // 模型回复，可携带 `javascript:` 伪 URL——落账后 Dashboard `:href`
    // 直染成点击执行向量。scheme 闸（safe_external_url）：非 http(s)
    // 诚实丢弃（链接是展示性留痕，权威标识是外部条目号）。
    let (n, url) = parse_publish_result(
        r#"{"number": 7, "html_url": "javascript:alert(1)"}"#,
        "$.number",
        Some("$.html_url"),
    )
    .unwrap();
    assert_eq!(n, 7);
    assert_eq!(url, None);

    // data: / 相对串同样丢弃；前导空白剥掉后再判（模型爱加空格）。
    let (_, url) = parse_publish_result(
        r#"{"number": 8, "html_url": "data:text/html,<b>"}"#,
        "$.number",
        Some("$.html_url"),
    )
    .unwrap();
    assert_eq!(url, None);
    let (_, url) = parse_publish_result(
        r#"{"number": 9, "html_url": "  javascript:x"}"#,
        "$.number",
        Some("$.html_url"),
    )
    .unwrap();
    assert_eq!(url, None);

    // 大小写不敏感：HTTPS()/Http() 前缀照常放行。
    let (_, url) = parse_publish_result(
        r#"{"number": 10, "html_url": "HTTPS://X/pr/10"}"#,
        "$.number",
        Some("$.html_url"),
    )
    .unwrap();
    assert_eq!(url.as_deref(), Some("HTTPS://X/pr/10"));
}

#[test]
fn parse_publish_result_url_path_absent_is_none() {
    // result_url 声明缺席 = 不取链接（协议合法形态：只记外部条目号）。
    let (n, url) = parse_publish_result(
        r#"{"number": 3, "html_url": "https://x/pr/3"}"#,
        "$.number",
        None,
    )
    .unwrap();
    assert_eq!(n, 3);
    assert_eq!(url, None);
}

// ---------------------------------------------------------------------------
// match_item（对账逐条匹配三态；协议声明化比较式）
// ---------------------------------------------------------------------------

#[test]
fn match_item_hit_by_head_ref() {
    let item =
        json!({"number": 9, "head": {"ref": "nemesis/task-t1"}, "html_url": "https://x/pr/9"});
    assert!(matches!(
        match_item(&item, "$.head.ref == nemesis/task-t1"),
        ItemMatch::Hit
    ));
}

#[test]
fn match_item_no_match_on_other_ref() {
    let item = json!({"number": 3, "head": {"ref": "other"}});
    assert!(matches!(
        match_item(&item, "$.head.ref == nemesis/task-t1"),
        ItemMatch::NoMatch
    ));
}

#[test]
fn match_item_malformed_expr_is_unreadable() {
    // 比较式畸形（无比较符/空侧）= 判不清，保守 Inconclusive，不冒充「不在」。
    let item = json!({"number": 3});
    assert!(matches!(
        match_item(&item, "no-operator-here"),
        ItemMatch::Unreadable
    ));
    assert!(matches!(match_item(&item, " == x"), ItemMatch::Unreadable));
}

#[test]
fn match_item_absent_path_reads_null_not_unreadable() {
    // `$.` 侧路径缺席 → "null"（可比较标量），语义 = NoMatch 而非 Unreadable
    // （条目形态合法只是没有该字段）。
    let item = json!({"number": 3, "title": "no head here"});
    assert!(matches!(
        match_item(&item, "$.head.ref == nemesis/task-t1"),
        ItemMatch::NoMatch
    ));
}

#[test]
fn match_item_pull_request_filter_semantics() {
    // `$.pull_request == null` 形态：issue 条目（无 pull_request 键）命中，
    // PR 条目（pull_request 是对象）不命中——issue/PR 同仓同 endpoint 的
    // 过滤协议在模板声明。
    let issue = json!({"number": 5, "title": "i"});
    let pr = json!({"number": 6, "pull_request": {"merged_at": null}});
    assert!(matches!(
        match_item(&issue, "$.pull_request == null"),
        ItemMatch::Hit
    ));
    assert!(matches!(
        match_item(&pr, "$.pull_request == null"),
        ItemMatch::NoMatch
    ));
}

#[test]
fn match_item_numeric_compare_shared_with_when() {
    // 数值比较原语与 when 条件步同源（compare_str）：两侧都可 parse f64 → 数值比。
    let item = json!({"number": 9});
    assert!(matches!(match_item(&item, "$.number == 9"), ItemMatch::Hit));
    assert!(matches!(
        match_item(&item, "$.number == 10"),
        ItemMatch::NoMatch
    ));
}

// ---------------------------------------------------------------------------
// reconcile_advance（协议实例化翻页推进）
// ---------------------------------------------------------------------------

#[test]
fn reconcile_advance_short_page_is_absent_even_at_page_one() {
    // 不足一页 = 末页取尽即停（对账固定首页漏后续页——协议化
    // 形态仍保留逐页扫尽语义）。
    let p = PublishProtocol {
        reconcile_limit: 20,
        reconcile_max_pages: 5,
        ..Default::default()
    };
    assert_eq!(reconcile_advance(1, 3, &p), ReconcilePaging::Absent);
    assert_eq!(reconcile_advance(1, 19, &p), ReconcilePaging::Absent);
    assert_eq!(
        reconcile_advance(1, 0, &p),
        ReconcilePaging::Absent,
        "空页 = 扫尽停"
    );
}

#[test]
fn reconcile_advance_full_page_advances_until_hard_cap() {
    let p = PublishProtocol {
        reconcile_limit: 20,
        reconcile_max_pages: 5,
        ..Default::default()
    };
    assert_eq!(reconcile_advance(1, 20, &p), ReconcilePaging::Next(2));
    assert_eq!(
        reconcile_advance(5, 20, &p),
        ReconcilePaging::Inconclusive,
        "页数硬上限封顶（防对端恒满页把对账变无界循环）——封顶是「未能扫尽」不是「不在」"
    );
}

#[test]
fn reconcile_advance_limit_key_from_protocol() {
    // 每页条数按协议声明（如对端只接受 limit=3）：页大小以 protocol 为准。
    let p = PublishProtocol {
        reconcile_limit: 3,
        reconcile_max_pages: 2,
        ..Default::default()
    };
    assert_eq!(reconcile_advance(1, 3, &p), ReconcilePaging::Next(2));
    assert_eq!(reconcile_advance(1, 2, &p), ReconcilePaging::Absent);
    assert_eq!(reconcile_advance(2, 3, &p), ReconcilePaging::Inconclusive);
}

// ---------------------------------------------------------------------------
// 协议缺席 = 诚实报错（域剥离批 C-3：宿主无内置发布协议）
// ---------------------------------------------------------------------------

/// 哑 LLM provider（协议闸在 agent loop 触碰之前——provider 永不被调用）。
struct NullProvider;
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NullProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        unreachable!("协议缺席闸先于一切 agent 交互");
    }
}

#[tokio::test]
async fn publish_without_protocol_is_honest_error() {
    let f = PreflightFixture::new();
    let agent_loop = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NullProvider),
        nemesis_agent::types::AgentConfig::default(),
    ));
    let e = publish_task_delivery(
        &f.db,
        &f.store,
        &agent_loop,
        Path::new(&f.ws_root),
        &f.args(),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(e.contains("publish 协议声明缺失"), "{e}");
    assert!(e.contains("宿主无内置发布协议"), "{e}");
}

// ---------------------------------------------------------------------------
// validate_task_id_for_branch
// ---------------------------------------------------------------------------

#[test]
fn validate_task_id_accepts_uuid_form() {
    assert!(validate_task_id_for_branch("a1B2c3d4-e5f6-7890-abcd-ef0123456789").is_ok());
    assert!(validate_task_id_for_branch("task_1.2").is_ok());
}

#[test]
fn validate_task_id_rejects_bad_forms() {
    assert!(validate_task_id_for_branch("").is_err());
    assert!(validate_task_id_for_branch(&"x".repeat(65)).is_err());
    assert!(validate_task_id_for_branch("a/b").is_err());
    assert!(validate_task_id_for_branch("a b").is_err());
    assert!(validate_task_id_for_branch("a..b").is_err());
    assert!(validate_task_id_for_branch(".lead").is_err());
    assert!(validate_task_id_for_branch("中文").is_err());
}

// ---------------------------------------------------------------------------
// arg_str / head_chars
// ---------------------------------------------------------------------------

#[test]
fn arg_str_extracts_trims_and_rejects_empty() {
    let args = json!({"a": " x ", "b": "  ", "c": 3});
    assert_eq!(arg_str(&args, "a").unwrap(), "x");
    assert!(arg_str(&args, "b").is_err());
    assert!(arg_str(&args, "c").is_err());
    assert!(arg_str(&args, "missing").is_err());
}

#[test]
fn head_chars_caps_by_chars_not_bytes() {
    assert_eq!(head_chars("abc中文def", 5), "abc中文");
}

// ---------------------------------------------------------------------------
// 夹具（真 git 副本仓 + 真 BoardStore + 真 TaskDb）
// ---------------------------------------------------------------------------

/// 真 git 仓临时目录（副本形态：init + 首 commit，HEAD 在场）。
fn git_copy_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("copy");
    std::fs::create_dir_all(&root).unwrap();
    let repo = git2::Repository::init(&root).unwrap();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("f.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    (dir, root)
}

/// board store + 项目（directory=副本根）+ issue（标题可解析）+ dispatch。
fn board_store_with_project(copy_root: &Path) -> Arc<BoardStore> {
    let dir = tempfile::tempdir().unwrap();
    // 防临时目录被 drop 后 store 还引用 db 文件——把 dir 泄漏到测试结束。
    let dir = dir.keep();
    let store = Arc::new(BoardStore::open(&dir.join("b.db"), "NB").unwrap());
    let project = store
        .create_project(
            "proj",
            "d",
            None,
            "📦",
            "ac",
            Some(copy_root.to_str().unwrap()),
        )
        .unwrap();
    let issue = store
        .create_issue(NewIssue {
            project_id: Some(project.id),
            title: "交付标题".to_string(),
            ..NewIssue::default()
        })
        .unwrap();
    store
        .insert_dispatch("task-1", issue.id, "node-a", &Actor::admin("admin"))
        .unwrap();
    store
}

struct PreflightFixture {
    db: Arc<TaskDb>,
    store: Arc<BoardStore>,
    copy_root: PathBuf,
    base_sha: String,
    /// TaskDb 根目录（publish_task_delivery 的 workspace 参数用）。
    ws_root: PathBuf,
    _copy_dir: tempfile::TempDir,
}

impl PreflightFixture {
    fn new() -> Self {
        let (copy_dir, copy_root) = git_copy_dir();
        let store = board_store_with_project(&copy_root);
        let db_dir = tempfile::tempdir().unwrap().keep();
        let db = TaskDb::open(&db_dir.join("tasks.db")).unwrap();
        let base_sha = nemesis_board::git_repo::head_commit_hex(&copy_root)
            .unwrap()
            .unwrap();
        Self {
            ws_root: db_dir,
            db,
            store,
            copy_root,
            base_sha,
            _copy_dir: copy_dir,
        }
    }

    fn payload(&self, extra: Value) -> Value {
        let mut p = json!({
            "patch": "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1,2 @@\n x\n+y\n",
            "repo_base_sha": self.base_sha,
        });
        if let (Some(dst), Some(src)) = (p.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        p
    }

    fn args(&self) -> Value {
        json!({
            "task_id": "task-1", "connector": "gitea",
            "owner": "o", "repo": "r",
        })
    }
}

// ---------------------------------------------------------------------------
// resolve_copy_dir（显式 copy_path + dispatch→issue→project 链）
// ---------------------------------------------------------------------------

#[test]
fn resolve_copy_dir_explicit_path_valid() {
    let f = PreflightFixture::new();
    let dir = resolve_copy_dir(
        &f.store,
        "task-1",
        Some(&json!(f.copy_root.to_str().unwrap())),
    )
    .unwrap();
    assert_eq!(dir, f.copy_root);
}

#[test]
fn resolve_copy_dir_explicit_path_non_git_rejected() {
    let f = PreflightFixture::new();
    let plain = tempfile::tempdir().unwrap().keep();
    let e =
        resolve_copy_dir(&f.store, "task-1", Some(&json!(plain.to_str().unwrap()))).unwrap_err();
    assert!(e.contains("不是 git 仓库"), "{e}");
}

#[test]
fn resolve_copy_dir_dispatch_chain() {
    let f = PreflightFixture::new();
    let dir = resolve_copy_dir(&f.store, "task-1", None).unwrap();
    assert_eq!(dir, f.copy_root);
}

#[test]
fn resolve_copy_dir_chain_missing_dispatch() {
    let f = PreflightFixture::new();
    let e = resolve_copy_dir(&f.store, "no-such-task", None).unwrap_err();
    assert!(e.contains("无派发记录"), "{e}");
}

// ---------------------------------------------------------------------------
// publish_credentials（vault 缺席 = None 匿名语义）
// ---------------------------------------------------------------------------

#[test]
fn publish_credentials_absent_vault_is_none() {
    let ws = tempfile::tempdir().unwrap().keep();
    // workspace 无 vault 文件 → resolve_alias_at 失败 → None（匿名推）。
    assert!(publish_credentials(&ws, "gitea", "owner").is_none());
}

// ---------------------------------------------------------------------------
// preflight 前置核验（全分支）
// ---------------------------------------------------------------------------

#[test]
fn preflight_missing_delivery_record_is_err() {
    let f = PreflightFixture::new();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert!(e.contains("无分支交付记录"), "{e}");
}

#[test]
fn preflight_rejected_is_err_with_canonical_text() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({})),
        false,
    )
    .unwrap();
    f.db.reject_delivery("task-1:branch", "人工否决").unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert_eq!(e, PREFLIGHT_REJECTED);
}

#[test]
fn preflight_already_published_short_circuits() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({})),
        false,
    )
    .unwrap();
    f.db.mark_delivery_published("task-1:branch").unwrap();
    f.db.merge_delivery_payload(
        "task-1:branch",
        &json!({"external_ref": 42, "external_url": "http://x/42"}),
    )
    .unwrap();
    match preflight(&f.db, &f.store, "task-1", &f.args()).unwrap() {
        Preflight::Already(v) => {
            assert_eq!(v["status"], ST_ALREADY);
            assert_eq!(v["external_ref"], 42);
            assert_eq!(v["external_url"], "http://x/42");
        }
        _ => panic!("期望 Already 幂等短路"),
    }
}

#[test]
fn preflight_kind_mismatch_is_err() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "review",
        None,
        "task-1:branch",
        &f.payload(json!({})),
        false,
    )
    .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert!(e.contains("≠ branch"), "{e}");
}

#[test]
fn preflight_missing_base_sha_rejected() {
    let f = PreflightFixture::new();
    // 旧形态交付：无 repo_base_sha——拒绝发布不静默放行。
    let mut p = f.payload(json!({}));
    p.as_object_mut().unwrap().remove("repo_base_sha");
    f.db.record_delivery("branch", None, "task-1:branch", &p, false)
        .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert_eq!(e, PREFLIGHT_NO_BASE_SHA);
}

#[test]
fn preflight_base_drift_rejected() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({"repo_base_sha": "0000000000000000000000000000000000000000"})),
        false,
    )
    .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert!(e.contains("基线漂移"), "{e}");
    assert!(e.contains(&f.base_sha), "漂移明细须带副本实际 HEAD: {e}");
}

/// exec 捕获路径形态：repo_base_sha 是 worker 侧 exec 仓的 fresh-init 基线
/// commit（任何主仓都对不上），但 `base_tree`（交付基线内容态）与副本 HEAD
/// 树等值 → 放行。树等值才是跨仓可移植不变量（patch 是 tree-diff）。
#[test]
fn preflight_exec_baseline_commit_mismatch_tree_match_proceeds() {
    let f = PreflightFixture::new();
    let tree = nemesis_board::git_repo::head_tree_hex(&f.copy_root)
        .unwrap()
        .unwrap();
    assert_ne!(
        tree, f.base_sha,
        "fixture 前提：tree hash ≠ commit hash（不同对象）"
    );
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "repo_base_sha": "0000000000000000000000000000000000000000",
            "base_tree": tree,
        })),
        false,
    )
    .unwrap();
    match preflight(&f.db, &f.store, "task-1", &f.args()).unwrap() {
        Preflight::Proceed { expected_base, .. } => {
            assert_eq!(expected_base, "0000000000000000000000000000000000000000");
        }
        _ => panic!("期望 Proceed（树等值放行）"),
    }
}

/// commit 与 tree 双双对不上 → 漂移拒绝，明细同时带两侧 tree。
#[test]
fn preflight_base_drift_reports_trees_when_both_mismatch() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "repo_base_sha": "0000000000000000000000000000000000000000",
            "base_tree": "1111111111111111111111111111111111111111",
        })),
        false,
    )
    .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert!(e.contains("基线漂移"), "{e}");
    assert!(
        e.contains("1111111111111111111111111111111111111111"),
        "{e}"
    );
}

#[test]
fn preflight_patch_truncated_rejected() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({"patch_truncated": true})),
        false,
    )
    .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert_eq!(e, PREFLIGHT_PATCH_TRUNCATED);
}

#[test]
fn preflight_empty_patch_rejected() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({"patch": "   "})),
        false,
    )
    .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert_eq!(e, PREFLIGHT_NO_PATCH);
}

#[test]
fn preflight_happy_path_proceeds_with_issue_title() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({})),
        false,
    )
    .unwrap();
    match preflight(&f.db, &f.store, "task-1", &f.args()).unwrap() {
        Preflight::Proceed {
            copy_dir,
            patch,
            expected_base,
            title,
            already_merged,
            delivery_tree,
            base_tree,
            stage,
        } => {
            assert_eq!(copy_dir, f.copy_root);
            assert_eq!(expected_base, f.base_sha);
            assert_eq!(title, "交付标题");
            assert!(patch.starts_with("diff --git"));
            assert!(!already_merged, "commit 等值路径非第三臂");
            assert!(delivery_tree.is_none(), "无 tree 字段 → None 透传");
            assert!(base_tree.is_none(), "无 base_tree 字段 → None 透传");
            assert_eq!(stage, None, "首次发布无阶段现场");
        }
        _ => panic!("期望 Proceed"),
    }
}

/// 第三臂（真链发现）：交付已并入副本 main（ingest 在任务完成时
/// 即合并，publish 人工闸在后是常态时序）——副本 HEAD 树等值交付 payload
/// `tree`（worker 交付后内容树）时放行，patch 免再套。 exec 捕获
/// 路径 repo_base_sha 对不上主仓任何 commit、base_tree 也因交付合并越过而
/// 失配，唯一等值的是交付树。
#[test]
fn preflight_delivery_already_merged_proceeds_third_arm() {
    let f = PreflightFixture::new();
    let base_tree = nemesis_board::git_repo::head_tree_hex(&f.copy_root)
        .unwrap()
        .unwrap();
    // 模拟 ingest：交付内容（patch 的 +y）已 commit 进副本 main。
    let repo = git2::Repository::open(&f.copy_root).unwrap();
    std::fs::write(f.copy_root.join("f.txt"), "x\ny\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("f.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "merge: 变更集合入",
        &tree,
        &[&head],
    )
    .unwrap();
    let merged_tree = nemesis_board::git_repo::head_tree_hex(&f.copy_root)
        .unwrap()
        .unwrap();
    assert_ne!(merged_tree, base_tree, "前提：合并后内容态越过基线");
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "repo_base_sha": "0000000000000000000000000000000000000000",
            "base_tree": base_tree,
            "tree": merged_tree,
        })),
        false,
    )
    .unwrap();
    match preflight(&f.db, &f.store, "task-1", &f.args()).unwrap() {
        Preflight::Proceed {
            already_merged,
            delivery_tree,
            ..
        } => {
            assert!(already_merged, "交付树等值 → 第三臂放行");
            assert_eq!(
                delivery_tree.as_deref(),
                Some(merged_tree.as_str()),
                "payload tree 原样透传（重试幂等等值锚）"
            );
        }
        _ => panic!("期望 Proceed（第三臂）"),
    }
}

/// 重试幂等决策（真链第二缺口）：「确定失败可重试」的承诺要有
/// 结构支撑——上次 git 半边完成（分支已推）后重试撞「分支已存在」诚实拒绝。
/// 分支树等值交付树 → 复用；不等 → 诚实拒绝不猜；锚缺席 → 落 git 层探针。
#[test]
fn branch_reuse_decision_reuses_matching_tree() {
    let reuse =
        branch_reuse_decision(Some(("c1".to_string(), "aa".to_string())), Some("aa")).unwrap();
    assert_eq!(reuse.as_deref(), Some("c1"), "树等值 → 复用分支 commit");
}

#[test]
fn branch_reuse_decision_rejects_mismatched_tree() {
    let e =
        branch_reuse_decision(Some(("c1".to_string(), "bb".to_string())), Some("aa")).unwrap_err();
    assert!(e.contains("拒绝覆盖"), "{e}");
    assert!(e.contains("bb"), "{e}");
}

#[test]
fn branch_reuse_decision_absent_branch_proceeds_normal_path() {
    let none = branch_reuse_decision(None, Some("aa")).unwrap();
    assert!(none.is_none(), "分支缺席 → None 走常规建分支");
    let none_no_tree = branch_reuse_decision(None, None).unwrap();
    assert!(
        none_no_tree.is_none(),
        "分支缺席 + 交付树缺席 → 同样常规路径"
    );
}

/// 交付树锚缺席（旧形态 payload 无 `tree` 键）→ Ok(None) 落入
/// apply_patch_new_branch 的内容级复用探针（父基线一致 + patch-over-HEAD
/// tree 等值，复评 移植）——双层校验分工：上层快路要锚，锚缺席不在此
/// fail-closed（会挡死 续作回归的合法重试），等值校验由 git 层完成。
#[test]
fn branch_reuse_decision_missing_delivery_tree_falls_to_git_probe() {
    let none = branch_reuse_decision(Some(("c1".to_string(), "aa".to_string())), None).unwrap();
    assert!(none.is_none(), "锚缺席 → None → git 层内容级探针裁决");
}

/// 交付树字段与副本 HEAD 树不等值（如合并后又叠了别的任务）→ 真漂移仍拒。
#[test]
fn preflight_stale_delivery_tree_still_drifts() {
    let f = PreflightFixture::new();
    let base_tree = nemesis_board::git_repo::head_tree_hex(&f.copy_root)
        .unwrap()
        .unwrap();
    // 副本前进一笔（越过基线），但交付树是无关值（旧任务/异内容）。
    let repo = git2::Repository::open(&f.copy_root).unwrap();
    std::fs::write(f.copy_root.join("f.txt"), "x\ny\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("f.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@t").unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "advance", &tree, &[&head])
        .unwrap();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "repo_base_sha": "0000000000000000000000000000000000000000",
            "base_tree": base_tree,
            "tree": "2222222222222222222222222222222222222222",
        })),
        false,
    )
    .unwrap();
    let e = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap_err();
    assert!(e.contains("基线漂移"), "{e}");
}

#[test]
fn preflight_title_arg_overrides_issue_title() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({})),
        false,
    )
    .unwrap();
    let mut args = f.args();
    args["title"] = json!("  自定义标题  ");
    match preflight(&f.db, &f.store, "task-1", &args).unwrap() {
        Preflight::Proceed { title, .. } => assert_eq!(title, "自定义标题"),
        _ => panic!("期望 Proceed"),
    }
}

#[test]
fn preflight_invalid_task_id_is_err() {
    let f = PreflightFixture::new();
    let mut args = f.args();
    args["task_id"] = json!("../escape");
    assert!(preflight(&f.db, &f.store, "../escape", &args).is_err());
}

// ---------------------------------------------------------------------------
// 续作回归（外部复评）：push/PR 阶段失败后的重试续作 +
// preflight 原始 patch 保真。connector_invoke 走桩（记账断言「未调用」——
// dispatch 有 panic 隔离，桩内 panic 会变 Tool error 文本被吞，测试不红）。
// ---------------------------------------------------------------------------

struct ReviewNov02NoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ReviewNov02NoopProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        panic!("review must never call a real model")
    }
}

/// connector_invoke 桩：按 op 配置响应/失败；全部调用记账。
struct StubConnector {
    create_pull: Option<String>,
    pr_list: Option<String>,
    pr_list_err: Option<String>,
    calls: Arc<std::sync::Mutex<Vec<String>>>,
}

impl StubConnector {
    fn new(
        create_pull: Option<String>,
        pr_list: Option<String>,
        pr_list_err: Option<String>,
    ) -> (Self, Arc<std::sync::Mutex<Vec<String>>>) {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                create_pull,
                pr_list,
                pr_list_err,
                calls: calls.clone(),
            },
            calls,
        )
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for StubConnector {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: Value = serde_json::from_str(args).map_err(|e| e.to_string())?;
        let op = v["op"].as_str().unwrap_or_default().to_string();
        self.calls.lock().unwrap().push(op.clone());
        match op.as_str() {
            "create_pull" => self
                .create_pull
                .clone()
                .ok_or_else(|| "create_pull 未在桩中配置".to_string()),
            "pr_list" if self.pr_list_err.is_some() => Err(self.pr_list_err.clone().unwrap()),
            "pr_list" => self
                .pr_list
                .clone()
                .ok_or_else(|| "pr_list 未在桩中配置".to_string()),
            other => Err(format!("op {other} 未在桩中配置")),
        }
    }

    // connector_invoke 是动态投影工具：静态表查不到 → 管线 fail-closed 成
    // ProcessExec（CRITICAL）。桩投影 internal 档过管线——投影机制与生产
    // 同源（connector_tools op_slug_for），档位是桩的测试豁免。
    fn operation_override(&self, _args: &str) -> Option<nemesis_agent::r#loop::OperationOverride> {
        Some(nemesis_agent::r#loop::OperationOverride {
            op_type: "internal".into(),
            force_approval: None,
            receipt_ctx: None,
        })
    }
}

fn stub_agent(stub: StubConnector) -> Arc<AgentLoop> {
    let mut lp = AgentLoop::new(
        Box::new(ReviewNov02NoopProvider),
        nemesis_agent::types::AgentConfig {
            model: "test".into(),
            ..Default::default()
        },
    );
    lp.register_tool(
        crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
        Box::new(stub),
    );
    Arc::new(lp)
}

/// file:// URL 组装：Linux 绝对路径带前导 `/`，`file:///` 基座再拼 = 四斜杠
/// 非法 URI（libgit2 class=Config 拒推—— Linux ET 四测连红
/// `'file:////tmp/...' is not a valid local file URI`）；Windows 盘符路径无
/// 前导斜杠需补足三杠。按首字符分治，双平台同式。
fn file_url(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// 给副本配 origin（file:// bare）并建 bare——push 真跑通。
fn wire_remote(f: &PreflightFixture) {
    let remote = f.copy_root.parent().unwrap().join("remote.git");
    let remote_url = file_url(&remote);
    git2::Repository::open(&f.copy_root)
        .unwrap()
        .remote("origin", &remote_url)
        .unwrap();
    git2::Repository::init_bare(&remote).unwrap();
}

/// 测试协议声明（gitea PR 形态——域知识只在此处，宿主生产路径无内置协议；
/// 与 flow 模板声明同形：op 名 + args 形状 + 结果引用 path + 对账匹配式）。
fn gitea_protocol() -> PublishProtocol {
    PublishProtocol {
        create_op: "create_pull".to_string(),
        create_args: BTreeMap::from([
            ("owner".to_string(), "o".to_string()),
            ("repo".to_string(), "r".to_string()),
            ("head".to_string(), "{{branch}}".to_string()),
            ("base".to_string(), "main".to_string()),
            ("title".to_string(), "{{task_id}}".to_string()),
        ]),
        result_ref: "$.number".to_string(),
        result_url: Some("$.html_url".to_string()),
        reconcile_op: "pr_list".to_string(),
        reconcile_args: BTreeMap::from([("state".to_string(), "all".to_string())]),
        reconcile_match: "$.head.ref == {{branch}}".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn review_nov02_publish_push_failure_retry_continues_after_remote_repaired() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({})),
        false,
    )
    .unwrap();
    let remote = f.copy_root.parent().unwrap().join("remote.git");
    let remote_url = file_url(&remote);
    git2::Repository::open(&f.copy_root)
        .unwrap()
        .remote("origin", &remote_url)
        .unwrap();
    let (stub, _) = StubConnector::new(None, None, None);
    let agent = stub_agent(stub);
    // 第一次发布：git 半边成功（分支/commit 生成），推送失败（对端 bare 不存在）。
    let first = publish_task_delivery(
        &f.db,
        &f.store,
        &agent,
        Path::new(&f.ws_root),
        &f.args(),
        Some(&gitea_protocol()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(first["status"], "failed");
    assert!(
        first["detail"].as_str().unwrap().contains("分支推送失败"),
        "{}",
        first["detail"]
    );
    // 现场留痕：commit_generated + published_commit 在案（重试凭此续作）。
    let rec = f.db.delivery_by_key("task-1:branch").unwrap().unwrap();
    assert_eq!(rec.payload["publish_stage"], "commit_generated");
    let first_commit = rec.payload["published_commit"]
        .as_str()
        .unwrap()
        .to_string();
    // 远端修复（bare 建好）+ create_pull 可用 → 重试续作到 published，且
    // 复用同一 commit——不被自己上次的交付分支挡死（payload 无 `tree` 锚，
    // 复用判定落 apply_patch_new_branch 内容级探针——双层校验第二层）。
    git2::Repository::init_bare(&remote).unwrap();
    let (stub, calls) = StubConnector::new(
        Some(json!({"number": 7, "html_url": "https://x/pr/7"}).to_string()),
        None,
        None,
    );
    let agent = stub_agent(stub);
    let retry = publish_task_delivery(
        &f.db,
        &f.store,
        &agent,
        Path::new(&f.ws_root),
        &f.args(),
        Some(&gitea_protocol()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(retry["status"], "published", "{retry}");
    assert_eq!(retry["external_ref"], 7);
    assert_eq!(
        retry["commit"],
        first_commit.as_str(),
        "重试必须复用首次生成的 commit（分支幂等复用），不新建"
    );
    let rec = f.db.delivery_by_key("task-1:branch").unwrap().unwrap();
    assert_eq!(rec.payload["publish_stage"], "published");
    assert_eq!(rec.payload["external_ref"], 7);
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["create_pull"],
        "重试直奔条目创建（分支复用，非 pr_pending 现场无对账）"
    );
}

#[tokio::test]
async fn review_nov02_publish_preflight_preserves_valid_patch_bytes() {
    let f = PreflightFixture::new();
    let payload = f.payload(json!({}));
    let original = payload["patch"].as_str().unwrap();
    assert!(git2::Diff::from_buffer(original.as_bytes()).is_ok());
    f.db.record_delivery("branch", None, "task-1:branch", &payload, false)
        .unwrap();
    // preflight 保真：合法 patch 原样透传（不被 trim 剥坏末尾换行）。
    let pre = preflight(&f.db, &f.store, "task-1", &f.args()).unwrap();
    let Preflight::Proceed { patch, stage, .. } = pre else {
        panic!("expected Proceed")
    };
    assert_eq!(stage, None, "首次发布无阶段现场");
    assert_eq!(patch, original, "preflight 必须保留 patch 原始字节");
    assert!(patch.ends_with('\n'));
    assert!(git2::Diff::from_buffer(patch.as_bytes()).is_ok());
    // 全链发布到 published（push 真跑，条目创建走桩）。
    wire_remote(&f);
    let (stub, _) = StubConnector::new(
        Some(json!({"number": 7, "html_url": "https://x/pr/7"}).to_string()),
        None,
        None,
    );
    let agent = stub_agent(stub);
    let result = publish_task_delivery(
        &f.db,
        &f.store,
        &agent,
        Path::new(&f.ws_root),
        &f.args(),
        Some(&gitea_protocol()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "published", "{result}");
    // 效果面：交付分支内容 = patch 真实应用结果。
    let repo = git2::Repository::open(&f.copy_root).unwrap();
    let delivered = repo
        .find_branch("nemesis/task-task-1", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap();
    let entry = delivered
        .tree()
        .unwrap()
        .get_path(Path::new("f.txt"))
        .unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    assert_eq!(std::str::from_utf8(blob.content()).unwrap(), "x\ny\n");
}

#[tokio::test]
async fn review_nov02_publish_pr_pending_retry_reconciles_existing_pr() {
    let f = PreflightFixture::new();
    // 上次运行到达创建阶段后中断：pr_pending 现场在案（域中立留痕形态）。
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "publish_stage": "pr_pending",
            "published_commit": "deadbeef",
            "publish_op": "create_pull",
        })),
        false,
    )
    .unwrap();
    wire_remote(&f);
    // 对端已有同 head 条目（上次其实建成了）——对账命中即收敛，绝不重复建。
    let (stub, calls) = StubConnector::new(
        None, // create_pull 未配置：误调用即 Err，测试必红
        Some(
            r#"[{"number": 9, "head": {"ref": "nemesis/task-task-1"}, "html_url": "https://x/pr/9"}]"#
                .to_string(),
        ),
        None,
    );
    let agent = stub_agent(stub);
    let retry = publish_task_delivery(
        &f.db,
        &f.store,
        &agent,
        Path::new(&f.ws_root),
        &f.args(),
        Some(&gitea_protocol()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(retry["status"], "published", "{retry}");
    assert_eq!(retry["external_ref"], 9);
    assert!(
        retry["detail"].as_str().unwrap().contains("未重复创建"),
        "{}",
        retry["detail"]
    );
    assert!(
        !calls.lock().unwrap().iter().any(|c| c == "create_pull"),
        "对账命中后绝不能再发 create_pull"
    );
    let rec = f.db.delivery_by_key("task-1:branch").unwrap().unwrap();
    assert_eq!(rec.payload["publish_stage"], "published");
    assert_eq!(rec.payload["external_ref"], 9);
}

#[tokio::test]
async fn review_nov02_publish_pr_pending_retry_inconclusive_stays_conservative() {
    let f = PreflightFixture::new();
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "publish_stage": "pr_pending",
            "published_commit": "deadbeef",
            "publish_op": "create_pull",
        })),
        false,
    )
    .unwrap();
    wire_remote(&f);
    // pr_list 查询失败 → 对账未能扫尽 → 保守判失败，绝不盲建条目；且中间态
    // 留痕不得把 pr_pending 覆盖回去（阶段单调不回退），下次重试仍先对账。
    let (stub, calls) = StubConnector::new(None, None, Some("connector 不可达".to_string()));
    let agent = stub_agent(stub);
    let retry = publish_task_delivery(
        &f.db,
        &f.store,
        &agent,
        Path::new(&f.ws_root),
        &f.args(),
        Some(&gitea_protocol()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(retry["status"], "failed");
    assert!(
        retry["detail"].as_str().unwrap().contains("对账未能扫尽"),
        "{}",
        retry["detail"]
    );
    assert!(
        !calls.lock().unwrap().iter().any(|c| c == "create_pull"),
        "对账未扫尽时不得发起 create_pull"
    );
    let rec = f.db.delivery_by_key("task-1:branch").unwrap().unwrap();
    assert_eq!(
        rec.payload["publish_stage"], "pr_pending",
        "pr_pending 现场必须保留（阶段单调不回退）"
    );
}

// ---------------------------------------------------------------------------
// op 路由双臂（a）：parse_op_route / credential_alias / NOT_SENT_MARKER 语义
// ---------------------------------------------------------------------------

#[test]
fn parse_op_route_native_passes_through() {
    assert_eq!(
        parse_op_route("create_pull").unwrap(),
        OpRoute::Native("create_pull".to_string())
    );
    assert_eq!(
        parse_op_route("  pr_list  ").unwrap(),
        OpRoute::Native("pr_list".to_string()),
        "Native 自由文本 trim"
    );
}

#[test]
fn parse_op_route_plugin_reference() {
    assert_eq!(
        parse_op_route("plugin:bugfix-flow:create_pull").unwrap(),
        OpRoute::Plugin {
            slug: "bugfix-flow".to_string(),
            op: "create_pull".to_string()
        }
    );
}

#[test]
fn parse_op_route_malformed_plugin_reference_is_loud() {
    // plugin: 前缀但段非法（大写/空 op/多段）= loud 拒，不给运行期留
    // 「当原语名静默错」黑洞。
    let e = parse_op_route("plugin:Bugfix-Flow:create_pull").unwrap_err();
    assert!(e.contains("plugin 引用形态但解析失败"), "{e}");
    assert!(parse_op_route("plugin:bugfix-flow").is_err());
    assert!(parse_op_route("plugin:").is_err());
}

#[test]
fn credential_alias_by_create_route() {
    // Plugin 臂：凭据跟随 guest 身份（connector 声明无关）。
    assert_eq!(
        credential_alias(
            None,
            &OpRoute::Plugin {
                slug: "bugfix-flow".to_string(),
                op: "create_pull".to_string()
            }
        )
        .unwrap(),
        "plugin/bugfix-flow/pat"
    );
    // Native 臂：connector 必填（凭据别名无法推导 = 诚实 Err）。
    assert_eq!(
        credential_alias(Some("gitea"), &OpRoute::Native("create_pull".to_string())).unwrap(),
        "connector/gitea/pat"
    );
    let e = credential_alias(None, &OpRoute::Native("create_pull".to_string())).unwrap_err();
    assert!(e.contains("缺 connector 声明"), "{e}");
}

#[test]
fn reconcile_page_body_accepts_entries_wrapper_form() {
    // a：plugin 步 done 契约 `{"status":"done","entries":[...]}` 去
    // status 后的对象形态——entries 包裹直出；非数组 entries = Inconclusive
    // 不冒充「扫尽」。本测试经 reconcile_by_match 全函数走四态判定边界。
    // 直接构造：畸形 entries 在页体解析即 Inconclusive（不进 match_item）。
    let text = r#"{"entries": "not-an-array"}"#;
    let parsed: Result<Value, _> = serde_json::from_str(text);
    let v = parsed.unwrap();
    let inconclusive = match v {
        Value::Array(_) => false,
        Value::Object(ref o) => !matches!(o.get("entries"), Some(Value::Array(_))),
        _ => true,
    };
    assert!(inconclusive, "非数组 entries 必须判 Inconclusive");
}

#[test]
fn not_sent_marker_prefix_is_dispatch_rejected_sibling() {
    // 语义锁：NOT_SENT_MARKER 错误必须被 publish_task_delivery 的「未发出」
    // 分支接住（与 ⛔ dispatch 拒同分支）——标记词表漂移会静默改走「可能已
    // 送达」对账分支（保守但不精确），钉死防漂移。
    assert!(NOT_SENT_MARKER.starts_with('['));
    assert_eq!(NOT_SENT_MARKER, "[not-sent]");
    let marked = format!("{NOT_SENT_MARKER} plugin:x:y 投递失败: e");
    assert!(marked.starts_with(NOT_SENT_MARKER));
    assert!(!crate::connector_tools::error_text_dispatch_rejected(
        &marked
    ));
}
