//! P3 发布模块（publish.rs）单元测试。
//!
//! 覆盖面：纯函数（PR 响应解析 / head 对账 / task_id 校验 / arg 提取）+
//! 副本解析链（显式 copy_path 与 dispatch→issue→project 链）+ 凭据缺席
//! 语义 + [`preflight`] 前置核验全分支（真 TaskDb + 真 BoardStore + 真
//! git 副本仓——不触碰 agent loop；git 半边与 PR 半边由 nemesis-board
//! git_repo tests 与 IT/UAT 面覆盖）。

use super::*;

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

/// connector_invoke 桩：按 op 配置响应/失败；全部调用记账——「未调用」断言
/// 走记账而非桩内 panic（dispatch 有 panic 隔离，桩 panic 会变成 Tool error
/// 文本被上层吞掉，测试不会失败）。
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

/// 给副本配 origin（file:// bare）并建 bare——push 真跑通。
fn wire_remote(f: &PreflightFixture) {
    let remote = f.copy_root.parent().unwrap().join("remote.git");
    let remote_url = format!("file:///{}", remote.to_string_lossy().replace('\\', "/"));
    git2::Repository::open(&f.copy_root)
        .unwrap()
        .remote("origin", &remote_url)
        .unwrap();
    git2::Repository::init_bare(&remote).unwrap();
}

// ---------------------------------------------------------------------------
// F1 续作回归（2026-11-02 外部复评）：push/PR 阶段失败后的重试续作 +
// preflight 原始 patch 保真。
// ---------------------------------------------------------------------------

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
    let remote_url = format!("file:///{}", remote.to_string_lossy().replace('\\', "/"));
    git2::Repository::open(&f.copy_root)
        .unwrap()
        .remote("origin", &remote_url)
        .unwrap();
    let (stub, _) = StubConnector::new(None, None, None);
    let agent = stub_agent(stub);
    // 第一次发布：git 半边成功（分支/commit 生成），推送失败（对端 bare 不存在）。
    let first = publish_task_delivery(&f.db, &f.store, &agent, &f.copy_root, &f.args())
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
    // 复用同一 commit——不被自己上次的交付分支挡死。
    git2::Repository::init_bare(&remote).unwrap();
    let (stub, calls) = StubConnector::new(
        Some(json!({"number": 7, "html_url": "https://x/pr/7"}).to_string()),
        None,
        None,
    );
    let agent = stub_agent(stub);
    let retry = publish_task_delivery(&f.db, &f.store, &agent, &f.copy_root, &f.args())
        .await
        .unwrap();
    assert_eq!(retry["status"], "published", "{retry}");
    assert_eq!(retry["pr_number"], 7);
    assert_eq!(
        retry["commit"],
        first_commit.as_str(),
        "重试必须复用首次生成的 commit（分支幂等复用），不新建"
    );
    let rec = f.db.delivery_by_key("task-1:branch").unwrap().unwrap();
    assert_eq!(rec.payload["publish_stage"], "published");
    assert_eq!(rec.payload["pr_number"], 7);
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["create_pull"],
        "重试直奔 PR 创建（分支复用，非 pr_pending 现场无对账）"
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
    // 全链发布到 published（push 真跑，PR 走桩）。
    wire_remote(&f);
    let (stub, _) = StubConnector::new(
        Some(json!({"number": 7, "html_url": "https://x/pr/7"}).to_string()),
        None,
        None,
    );
    let agent = stub_agent(stub);
    let result = publish_task_delivery(&f.db, &f.store, &agent, &f.copy_root, &f.args())
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
    // 上次运行到达 PR 阶段后中断：pr_pending 现场在案。
    f.db.record_delivery(
        "branch",
        None,
        "task-1:branch",
        &f.payload(json!({
            "publish_stage": "pr_pending",
            "published_commit": "deadbeef",
            "pr_target": "o/r:main",
        })),
        false,
    )
    .unwrap();
    wire_remote(&f);
    // 对端已有同 head PR（上次其实建成了）——对账命中即收敛，绝不重复建。
    let (stub, calls) = StubConnector::new(
        None, // create_pull 未配置：误调用即 Err，测试必红
        Some(
            r#"[{"number": 9, "head": {"ref": "nemesis/task-task-1"}, "html_url": "https://x/pr/9"}]"#
                .to_string(),
        ),
        None,
    );
    let agent = stub_agent(stub);
    let retry = publish_task_delivery(&f.db, &f.store, &agent, &f.copy_root, &f.args())
        .await
        .unwrap();
    assert_eq!(retry["status"], "published", "{retry}");
    assert_eq!(retry["pr_number"], 9);
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
    assert_eq!(rec.payload["pr_number"], 9);
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
            "pr_target": "o/r:main",
        })),
        false,
    )
    .unwrap();
    wire_remote(&f);
    // pr_list 查询失败 → 对账未能扫尽 → 保守判失败，绝不盲建 PR；且中间态
    // 留痕不得把 pr_pending 覆盖回去（阶段单调不回退），下次重试仍先对账。
    let (stub, calls) = StubConnector::new(None, None, Some("connector 不可达".to_string()));
    let agent = stub_agent(stub);
    let retry = publish_task_delivery(&f.db, &f.store, &agent, &f.copy_root, &f.args())
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
use nemesis_board::assignment::Actor;
use nemesis_board::models::NewIssue;
use nemesis_board::store::BoardStore;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// parse_pr_response
// ---------------------------------------------------------------------------

#[test]
fn parse_pr_response_direct_object() {
    let text = r#"{"number": 7, "html_url": "http://x/pr/7", "title": "t"}"#;
    let (n, url) = parse_pr_response(text).unwrap();
    assert_eq!(n, 7);
    assert_eq!(url.as_deref(), Some("http://x/pr/7"));
}

#[test]
fn parse_pr_response_single_element_array() {
    let text = r#"[{"number": 12, "html_url": null}]"#;
    let (n, url) = parse_pr_response(text).unwrap();
    assert_eq!(n, 12);
    assert_eq!(url, None);
}

#[test]
fn parse_pr_response_non_json_is_none() {
    // HTTP 错误页 / 截断体 / connector 文本包装——认不出 = 交给对账。
    assert!(parse_pr_response("<html>500</html>").is_none());
    assert!(parse_pr_response("").is_none());
    assert!(parse_pr_response(r#"{"number": "#).is_none());
}

#[test]
fn parse_pr_response_missing_number_is_none() {
    assert!(parse_pr_response(r#"{"message": "created"}"#).is_none());
    // number 非整数也认不出。
    assert!(parse_pr_response(r#"{"number": "7"}"#).is_none());
}

#[test]
fn parse_pr_response_empty_array_is_none() {
    assert!(parse_pr_response("[]").is_none());
}

#[test]
fn parse_pr_response_unsafe_scheme_dropped() {
    // BUG-20261101-274：html_url 来自 connector 调用的模型回复，可携带
    // `javascript:` 伪 URL——落账后 Dashboard `:href` 直染成点击执行向量。
    // scheme 闸：非 http(s) 诚实丢弃（链接是展示性留痕，权威标识是
    // pr_number）。
    let (n, url) =
        parse_pr_response(r#"{"number": 7, "html_url": "javascript:alert(1)"}"#).unwrap();
    assert_eq!(n, 7);
    assert_eq!(url, None);

    // data: / 相对串同样丢弃；前导空白剥掉后再判（模型爱加空格）。
    let (_, url) = parse_pr_response(r#"{"number": 8, "html_url": "data:text/html,<b>"}"#).unwrap();
    assert_eq!(url, None);
    let (_, url) = parse_pr_response(r#"{"number": 9, "html_url": "  javascript:x"}"#).unwrap();
    assert_eq!(url, None);

    // 大小写不敏感：HTTPS()/Http() 前缀照常放行。
    let (_, url) = parse_pr_response(r#"{"number": 10, "html_url": "HTTPS://X/pr/10"}"#).unwrap();
    assert_eq!(url.as_deref(), Some("HTTPS://X/pr/10"));
}

#[test]
fn find_pr_by_head_unsafe_scheme_dropped() {
    // 对账路径同闸：命中 head 的 PR 带伪 URL → number 照常返回、url 丢弃。
    let list = r#"[{"number": 5, "head": {"ref": "nemesis/task-t1"}, "html_url": "javascript:x"}]"#;
    let (n, url) = find_pr_by_head(list, "nemesis/task-t1").unwrap();
    assert_eq!(n, 5);
    assert_eq!(url, None);
}

// ---------------------------------------------------------------------------
// find_pr_by_head
// ---------------------------------------------------------------------------

#[test]
fn find_pr_by_head_finds_matching() {
    let list = r#"[{"number": 3, "head": {"ref": "other"}, "html_url": "https://x/pr/3"},
                   {"number": 9, "head": {"ref": "nemesis/task-t1"}, "html_url": "https://x/pr/9"}]"#;
    let (n, url) = find_pr_by_head(list, "nemesis/task-t1").unwrap();
    assert_eq!(n, 9);
    assert_eq!(url.as_deref(), Some("https://x/pr/9"));
}

#[test]
fn find_pr_by_head_no_match_is_none() {
    let list = r#"[{"number": 3, "head": {"ref": "other"}}]"#;
    assert!(find_pr_by_head(list, "nemesis/task-t1").is_none());
}

#[test]
fn find_pr_by_head_non_json_is_none() {
    // 对账响应本身坏（截断/非 JSON）= None，保守判失败不造 PR 号。
    assert!(find_pr_by_head("<html>", "b").is_none());
    assert!(find_pr_by_head(r#"{"not": "array"}"#, "b").is_none());
}

#[test]
fn find_pr_by_head_state_all_accepts_closed() {
    // state=all 对账语义：已关闭的同 head PR 也认（报既有 PR 比重复建诚实）。
    let list = r#"[{"number": 5, "head": {"ref": "nemesis/task-x"}, "state": "closed"}]"#;
    assert_eq!(find_pr_by_head(list, "nemesis/task-x").unwrap().0, 5);
}

#[test]
fn reconcile_advance_short_page_is_absent_even_at_page_one() {
    // 不足一页 = 末页取尽即停（BUG-20261028：对账固定首页漏后续页 PR）。
    assert_eq!(reconcile_advance(1, 3), ReconcilePaging::Absent);
    assert_eq!(
        reconcile_advance(1, RECONCILE_PAGE_LIMIT - 1),
        ReconcilePaging::Absent
    );
    assert_eq!(
        reconcile_advance(1, 0),
        ReconcilePaging::Absent,
        "空页 = 扫尽停"
    );
}

#[test]
fn reconcile_advance_full_page_advances_until_hard_cap() {
    assert_eq!(
        reconcile_advance(1, RECONCILE_PAGE_LIMIT),
        ReconcilePaging::Next(2)
    );
    assert_eq!(
        reconcile_advance(RECONCILE_MAX_PAGES, RECONCILE_PAGE_LIMIT),
        ReconcilePaging::Inconclusive,
        "页数硬上限封顶（防对端恒满页把对账变无界循环）——封顶是「未能扫尽」不是「不在」"
    );
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
            "owner": "o", "repo": "r", "base": "main",
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
        &json!({"pr_number": 42, "pr_url": "http://x/42"}),
    )
    .unwrap();
    match preflight(&f.db, &f.store, "task-1", &f.args()).unwrap() {
        Preflight::Already(v) => {
            assert_eq!(v["status"], ST_ALREADY);
            assert_eq!(v["pr_number"], 42);
            assert_eq!(v["pr_url"], "http://x/42");
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
            stage: _,
        } => {
            assert_eq!(copy_dir, f.copy_root);
            assert_eq!(expected_base, f.base_sha);
            assert_eq!(title, "交付标题");
            assert!(patch.starts_with("diff --git"));
        }
        _ => panic!("期望 Proceed"),
    }
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
