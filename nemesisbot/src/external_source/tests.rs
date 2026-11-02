//! P1 外循环纯函数面测试：解析抢救 / 标签过滤 / 回写文案 / exclude 幂等。
//! （需要 AgentLoop/BoardStore 的装配路径不在此测——import 幂等由
//! `list_issues_by_origin` 去重 + store 侧既有测试覆盖，轮询循环本身
//! 依赖网关活体，P0 验证面。）

use super::*;

#[tokio::test]
async fn review_nov02_label_filtered_first_page_still_scans_matching_second_page() {
    let source = ExternalSourceConfig {
        owner: "o".into(),
        repo: "r".into(),
        labels: vec!["agent-ready".into()],
        clone: false,
        auto_dispatch: false,
        ..Default::default()
    };
    let first: Vec<_> = (1..=20)
        .map(|number| {
            serde_json::json!({
                "number":number, "labels":[{"name":"human-only"}]
            })
        })
        .collect();
    let second: Vec<_> = (21..=40)
        .map(|number| {
            serde_json::json!({
                "number":number, "labels":[{"name":"agent-ready"}]
            })
        })
        .collect();
    assert!(first.iter().all(|e| !labels_match(&source, e)));
    assert!(second.iter().all(|e| labels_match(&source, e)));
    let (rt, _dir, calls) = review_nov02_runtime(vec![
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
    ]);
    poll_source(&rt, &source, None).await;
    let queried = calls.lock().unwrap().clone();
    println!(
        "REVIEW first_page_matches=0 second_page_matches={} queried_pages={queried:?} status={:?}",
        second.len(),
        rt.statuses.lock()
    );
    // F5（2026-11-02 复评）：满页零新导入（全被标签滤掉）不再停页——第 2/3 页
    // 照扫，第 2 页匹配条目照常导入；第 3 页返回空数组（不足一页）= 末页停。
    assert_eq!(queried, vec![1, 2, 3]);
    assert!(rt.statuses.lock()[0].last_error.is_none());
    assert!(
        !rt.store
            .list_issues_by_origin("gitea", "gitea/o/r#21", 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(rt.statuses.lock()[0].imported_total, 20);
}

struct ReviewNov02SourceProvider;
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ReviewNov02SourceProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        panic!("no model calls")
    }
}
struct ReviewNov02Pages {
    pages: Vec<String>,
    calls: Arc<std::sync::Mutex<Vec<i64>>>,
}
#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for ReviewNov02Pages {
    async fn execute(
        &self,
        args: &str,
        _: &nemesis_agent::context::RequestContext,
    ) -> Result<String, String> {
        let value: serde_json::Value = serde_json::from_str(args).unwrap();
        let page = value["args"]["page"].as_i64().unwrap();
        self.calls.lock().unwrap().push(page);
        Ok(self
            .pages
            .get((page - 1) as usize)
            .cloned()
            .unwrap_or_else(|| "[]".into()))
    }
}
fn review_nov02_runtime(
    pages: Vec<String>,
) -> (
    ExternalSourceRuntime,
    tempfile::TempDir,
    Arc<std::sync::Mutex<Vec<i64>>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(BoardStore::open(&dir.path().join("board.db"), "NB").unwrap());
    let calls = Arc::new(std::sync::Mutex::new(vec![]));
    let mut agent = AgentLoop::new(
        Box::new(ReviewNov02SourceProvider),
        nemesis_agent::types::AgentConfig {
            model: "test".into(),
            ..Default::default()
        },
    );
    agent.register_tool(
        "connector_invoke".into(),
        Box::new(ReviewNov02Pages {
            pages,
            calls: calls.clone(),
        }),
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = ExternalSourceRuntime {
        agent_loop: Arc::new(agent),
        store,
        workspace: dir.path().to_path_buf(),
        home: dir.path().to_path_buf(),
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        #[cfg(feature = "cluster")]
        cluster: None,
        #[cfg(not(feature = "cluster"))]
        cluster: (),
        statuses: parking_lot::Mutex::new(vec![SourceStatus {
            source: "gitea/o/r".into(),
            enabled: true,
            last_poll: None,
            last_error: None,
            last_warning: None,
            imported_total: 0,
            last_imported: vec![],
            clone_base_sha: None,
            writeback_ok: 0,
            writeback_failed: 0,
            watermark: None,
        }]),
        writeback_tx: tx,
    };
    (rt, dir, calls)
}

#[tokio::test]
async fn review_nov02_partial_issue_page_imports_salvaged_entries_with_warning() {
    let source = ExternalSourceConfig {
        connector: "gitea".into(),
        owner: "o".into(),
        repo: "r".into(),
        clone: false,
        auto_dispatch: false,
        ..Default::default()
    };
    let small = serde_json::json!({"number":1,"title":"complete first issue","body":"small"});
    let (control, _control_dir, _) =
        review_nov02_runtime(vec![serde_json::to_string(&vec![small.clone()]).unwrap()]);
    poll_source(&control, &source, None).await;
    assert_eq!(
        control.statuses.lock()[0].imported_total,
        1,
        "normal complete response must import"
    );
    let full =
        serde_json::to_string(&serde_json::json!([small,{"number":2,"body":"x".repeat(9000)}]))
            .unwrap();
    let capped: String = full.chars().take(8192).collect();
    assert_eq!(parse_issue_list(&capped).0.len(), 1);
    let (rt, _dir, calls) = review_nov02_runtime(vec![capped]);
    for _ in 0..3 {
        poll_source(&rt, &source, None).await;
    }
    let status = rt.statuses.lock();
    println!(
        "REVIEW partial_page_salvaged=1 queried_pages={:?} status={status:?}",
        calls.lock().unwrap()
    );
    // F5（2026-11-02 复评）：截断页抢救出的完整条目照常导入（不再提前
    // return 丢弃），带 warning 收轮；截断页 fetched 不可信 → 不作末页判定
    // ——但截断即收轮，下轮从首页重扫（去重幂等），每轮恰好 1 次请求。
    assert_eq!(status[0].imported_total, 1);
    assert!(status[0].last_warning.is_some());
    assert!(status[0].last_error.is_none());
    assert!(
        !rt.store
            .list_issues_by_origin("gitea", "gitea/o/r#1", 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        calls.lock().unwrap().clone(),
        vec![1, 1, 1],
        "truncated page ends the round; next round rescans from page 1"
    );
}

// ---------------------------------------------------------------------------
// parse_issue_list：8K 截断抢救
// ---------------------------------------------------------------------------

#[test]
fn parse_issue_list_full_json_no_salvage() {
    let text = r#"[{"number":1,"title":"a"},{"number":2,"title":"b"}]"#;
    let (entries, salvaged) = parse_issue_list(text);
    assert!(!salvaged);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["number"], 1);
}

#[test]
fn parse_issue_list_truncated_tail_salvages_complete_entries() {
    // 模拟 HTTP_BODY_CAP 截断：第二条对象中途被斩断。
    let mut text = String::from(r#"[{"number":1,"title":"完整条目"},{"number":2,"title":"被截"#);
    text.push_str("断的条目体，无闭合");
    let (entries, salvaged) = parse_issue_list(&text);
    assert!(salvaged);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["number"], 1);
}

#[test]
fn parse_issue_list_salvage_respects_string_state() {
    // 花括号出现在字符串字面量内不得破坏配对深度。
    let text = r#"[{"number":3,"body":"正文里有 } 和 { 花括号"},{"number":4"#;
    let (entries, salvaged) = parse_issue_list(text);
    assert!(salvaged);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["number"], 3);
    assert_eq!(entries[0]["body"], "正文里有 } 和 { 花括号");
}

#[test]
fn parse_issue_list_non_array_is_empty_no_salvage() {
    let (entries, salvaged) = parse_issue_list("Tool error: HTTP 401: unauthorized");
    assert!(entries.is_empty());
    assert!(!salvaged);
}

#[test]
fn parse_issue_list_pr_entry_survives_salvage() {
    // Gitea issues_list 混入 PR（带 pull_request 字段）——截断抢救后仍可辨。
    let text = r#"[{"number":5,"pull_request":{"id":9}},{"number":6"#;
    let (entries, salvaged) = parse_issue_list(text);
    assert!(salvaged);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].get("pull_request").is_some());
}

// ---------------------------------------------------------------------------
// labels_match
// ---------------------------------------------------------------------------

fn entry_with_labels(names: &[&str]) -> serde_json::Value {
    let labels: Vec<serde_json::Value> = names
        .iter()
        .map(|n| serde_json::json!({ "name": n }))
        .collect();
    serde_json::json!({ "number": 1, "labels": labels })
}

#[test]
fn labels_match_empty_config_accepts_all() {
    let src = ExternalSourceConfig {
        owner: "o".into(),
        repo: "r".into(),
        ..Default::default()
    };
    assert!(labels_match(&src, &entry_with_labels(&["bug"])));
    assert!(labels_match(&src, &entry_with_labels(&[])));
}

#[test]
fn labels_match_filters_by_any_hit() {
    let src = ExternalSourceConfig {
        owner: "o".into(),
        repo: "r".into(),
        labels: vec!["agent-ready".into()],
        ..Default::default()
    };
    assert!(labels_match(
        &src,
        &entry_with_labels(&["bug", "agent-ready"])
    ));
    assert!(!labels_match(&src, &entry_with_labels(&["bug", "docs"])));
    assert!(!labels_match(&src, &entry_with_labels(&[])));
}

// ---------------------------------------------------------------------------
// render_writeback
// ---------------------------------------------------------------------------

fn issue_with_origin(status: nemesis_board::IssueStatus) -> nemesis_board::Issue {
    // 手工构造最小 Issue（避免依赖 store）——字段集以当前 models 为准，
    // 新增字段会在此编译期暴露。
    let mut issue = minimal_issue();
    issue.status = status;
    issue
}

fn minimal_issue() -> nemesis_board::Issue {
    serde_json::from_value(serde_json::json!({
        "id": 7,
        "number": "NB-9",
        "title": "[gitea/o/r#12] 修复登录",
        "description": "",
        "status": "in_progress",
        "priority": 1,
        "creator": {"kind": "system", "id": "external-source"},
        "position": 0,
        "required_tags": [],
        "required_capabilities": [],
        "origin": {"origin_type": "gitea", "origin_id": "o/r#12"},
        "created_at": 0,
        "updated_at": 0,
    }))
    .expect("最小 Issue JSON 应可反序列化")
}

#[test]
fn render_writeback_covers_three_terminal_states() {
    let issue = issue_with_origin(nemesis_board::IssueStatus::Done);
    let done = render_writeback(&issue, "in_review", "done");
    assert!(done.contains("NB-9"));
    assert!(done.contains("✅"));
    let review = render_writeback(&issue, "in_progress", "in_review");
    assert!(review.contains("🔄") && review.contains("in_review"));
    let cancelled = render_writeback(&issue, "todo", "cancelled");
    assert!(cancelled.contains("⛔"));
}

// ---------------------------------------------------------------------------
// exclude_projection_files：幂等 + 不重复追加
// ---------------------------------------------------------------------------

#[test]
fn exclude_projection_files_writes_once_and_idempotent() {
    let tmp = std::env::temp_dir().join(format!("ext-src-excl-{}", uuid::Uuid::new_v4()));
    let root = tmp.join("repo");
    std::fs::create_dir_all(root.join(".git").join("info")).unwrap();
    exclude_projection_files(&root);
    let exclude = root.join(".git").join("info").join("exclude");
    let first = std::fs::read_to_string(&exclude).unwrap();
    for want in [
        "/project.json",
        "/timeline.jsonl",
        "/docs/",
        "/records/",
        "/.gitignore",
    ] {
        assert!(first.contains(want), "exclude 缺 {want}");
    }
    // 二次执行：字节不变（幂等）。
    exclude_projection_files(&root);
    let second = std::fs::read_to_string(&exclude).unwrap();
    assert_eq!(first, second);
    // 非 git 目录（普通项目目录）= 静默零操作不炸。
    let plain = tmp.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    exclude_projection_files(&plain); // 不 panic 即过
    std::fs::remove_dir_all(&tmp).ok();
}

// ---------------------------------------------------------------------------
// 来源键推导（回写 source_key 与状态表对齐）
// ---------------------------------------------------------------------------

// BUG-20261029-175 回归锁：origin_id 生产形态为三段
// "connector/owner/repo#N"（唯一构造点，自 P1 引入即三段），回写链路的
// source_key 推导与 owner/repo/index 拆解必须同源——旧实现停在两段假设
// "owner/repo#N"：source_key 重复拼 connector 成四段（与状态表键永不匹配，
// writeback 成败计数恒 0），owner 参数吃进 connector 段（Gitea
// /repos/{owner}/... 路径错位，回帖自落地起 100% 失败）。修复 =
// [`super::split_origin_id`] 单一真相源，observer 与 consumer 双侧共用。
#[test]
fn split_origin_id_three_segment_contract() {
    let (owner, repo, index, source_key) =
        split_origin_id("gitea/myowner/myrepo#42").expect("三段形态必须可拆");
    assert_eq!(owner, "myowner", "owner 不得吃进 connector 段");
    assert_eq!(repo, "myrepo");
    assert_eq!(index, 42);
    assert_eq!(source_key, "gitea/myowner/myrepo", "与状态表键同构");
}

#[test]
fn split_origin_id_rejects_malformed() {
    // 无 '#' / index 非整数 / 段数不足三 / 空段：一律 None（诚实跳过，
    // 不猜形态——旧 consumer 的 index unwrap_or(0) 会白发必 404 的请求）。
    assert!(split_origin_id("gitea/myowner/myrepo").is_none());
    assert!(split_origin_id("gitea/myowner/myrepo#x").is_none());
    assert!(split_origin_id("myrepo#42").is_none());
    assert!(split_origin_id("gitea/#42").is_none());
    assert!(split_origin_id("#42").is_none());
}

// ---------------------------------------------------------------------------
// BUG-20261028-122 回归锁：外部 issue 轮询固定首页 + 8K 截断伪成功。
// 旧实现单发 issues_list（limit=20 无 page），第 21 条起的 issue 永远导
// 入不了；截断且 0 条可解析时 `[` 前缀守卫当空列表伪成功 + 「下轮重取」
// 空头支票（下轮还是同一个被截断的首页）。修复：翻页（末页/满页零新导
// 入即停 + 硬上限）+ 截断诚实化（0 条抢救 = last_error 落状态）。
// 端到端翻页行为（真 connector 拉真仓库）是网关活体验证面（P0，见模块
// 头测试哲学注记）——本组锁钉住决策逻辑与 wire 契约这三个可纯测点。
// ---------------------------------------------------------------------------

#[test]
fn page_args_carries_paging_params() {
    // wire 契约：翻页请求必须带 page（修复前根本不发 page——本锁修复前红）。
    let src = ExternalSourceConfig {
        owner: "o".into(),
        repo: "r".into(),
        ..Default::default()
    };
    let args = page_args(&src, 3);
    assert_eq!(args["page"], 3, "page 参数必须随轮次透传: {args}");
    assert_eq!(args["owner"], "o");
    assert_eq!(args["repo"], "r");
    assert_eq!(args["state"], "open");
    assert_eq!(args["limit"], 20, "单页条数 = POLL_PAGE_LIMIT");
}

#[test]
fn should_stop_paging_decision() {
    // F5（2026-11-02 复评）：停页只认可证明条件——末页（返回不足一页，
    // 对端已取尽）。
    assert!(should_stop_paging(5));
    assert!(should_stop_paging(0));
    // 满页恒续翻：标签过滤/全熟/PR 混入的满页零新导入不证明后续页没有
    // 可导入条目（旧逻辑在此停 = 深页永不扫描漏任务）；MAX_POLL_PAGES 兜底。
    assert!(!should_stop_paging(20));
}

#[test]
fn truncation_blocked_only_when_zero_salvaged() {
    // 截断且 0 条抢救 = 本轮失败（诚实化核心；修复前这里伪成功）。
    assert!(truncation_blocked(true, 0));
    // 抢救出条目 = 照常处理（截断尾部本轮丢失，WARN 可见，不算失败）。
    assert!(!truncation_blocked(true, 3));
    // 未截断 = 永不因截断失败。
    assert!(!truncation_blocked(false, 0));
    assert!(!truncation_blocked(false, 5));
}

#[test]
fn gitea_manifest_issues_list_declares_page_param() {
    // 真实 gitea connector.toml 契约锁（修复前红：schema 无 page——LLM
    // 可见契约缺页参数，翻页请求参数成为未声明魔法）。清单是分发产物，
    // schema 面变更必须随代码锁同步钉住。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/workspace/connectors/gitea/connector.toml"
    );
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {path} 失败: {e}"));
    let doc: toml::Value = toml::from_str(&raw).expect("connector.toml 必须是合法 TOML");
    let ops = doc["op"].as_array().expect("[[op]] 数组");
    let issues_list = ops
        .iter()
        .find(|o| o["id"].as_str() == Some("issues_list"))
        .expect("gitea 清单必须声明 issues_list");
    let props = &issues_list["args_schema"]["properties"];
    let page = props
        .get("page")
        .unwrap_or_else(|| panic!("issues_list args_schema 必须声明 page 参数"));
    assert_eq!(page["type"].as_str(), Some("integer"));
    assert_eq!(page["minimum"].as_integer(), Some(1), "页码从 1 起");
    // limit 的分页边界同步钉住（Gitea REST 真实上限 50）。
    assert_eq!(props["limit"]["maximum"].as_integer(), Some(50));
}

#[test]
fn gitea_manifest_pr_list_declares_page_param() {
    // 同上契约锁钉 pr_list（BUG-20261028 横扫补齐：issues_list 修复时未
    // 横扫同清单同类 op——pr_list 无 page，publish 对账固定首页，PR 总数
    // 超页后重试重复建 PR）。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/workspace/connectors/gitea/connector.toml"
    );
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {path} 失败: {e}"));
    let doc: toml::Value = toml::from_str(&raw).expect("connector.toml 必须是合法 TOML");
    let ops = doc["op"].as_array().expect("[[op]] 数组");
    let pr_list = ops
        .iter()
        .find(|o| o["id"].as_str() == Some("pr_list"))
        .expect("gitea 清单必须声明 pr_list");
    let props = &pr_list["args_schema"]["properties"];
    let page = props
        .get("page")
        .unwrap_or_else(|| panic!("pr_list args_schema 必须声明 page 参数"));
    assert_eq!(page["type"].as_str(), Some("integer"));
    assert_eq!(page["minimum"].as_integer(), Some(1), "页码从 1 起");
    assert_eq!(props["limit"]["maximum"].as_integer(), Some(50));
}
