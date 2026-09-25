//! conflict_resolver wave-5 round-2 补测：复用 tests.rs 的跑台（pub(crate)
//! 暴露），补硬解成功路径里 ours / theirs 两择边臂的审计 JSON 装配。
//!
//! 既有 success 测试只用 merge 动作；本测脚本化 LLM 产出 ours+theirs，
//! 走通 record_auto_decide 的 ResolutionAction 全臂。
//!
//! 结构性边界（与 tests.rs 头注同裁）：重派接触循环（probe 按真睡眠
//! t0/+60s/+120s）与 run_detached 全链不进单测。
#![cfg(target_os = "windows")]

use std::sync::Arc;

use crate::conflict_resolver::tests::{
    ResolverLlm, attach_resolver_llm, conflict_text, issue_on_git_project, resolver_deps,
};

#[tokio::test]
async fn w5_hard_resolve_ours_and_theirs_records_all_action_arms() {
    let deps = resolver_deps("w5-ot");
    let (issue, root) = issue_on_git_project(&deps, "w5-ot");

    // 双冲突文件，一择我方一择对方（record_auto_decide 的 JSON match 全臂）。
    let raw = r#"{"resolutions":[
        {"path":"a.txt","action":"ours","reason":"以我方为准"},
        {"path":"b.txt","action":"theirs","reason":"采信对方版本"}
    ]}"#;
    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Ok(raw.to_string())])),
            fallback: String::new(),
            calls: std::sync::atomic::AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-w5ot".into(),
        vec![conflict_text("a.txt"), conflict_text("b.txt")],
    )
    .await;

    // 择边不带 content：落盘从冲突三阶段取对应侧 blob（HEAD 客观核验）。
    let exported = root.join("export-head");
    let (_n, _bytes) = nemesis_board::git_repo::export_head_tree(&root, &exported).unwrap();
    assert_eq!(
        std::fs::read(exported.join("a.txt")).unwrap(),
        b"ours line\n".to_vec(),
        "ours 择边必须落我方侧 blob"
    );
    assert_eq!(
        std::fs::read(exported.join("b.txt")).unwrap(),
        b"theirs line\n".to_vec(),
        "theirs 择边必须落对方侧 blob"
    );
    // 审计 JSON 里两臂的 action 字符串都在（ours/theirs 臂被真实走到）。
    let audit = deps
        .store
        .list_activity(issue.id)
        .unwrap()
        .into_iter()
        .find(|a| {
            a.details
                .as_deref()
                .map(|d| d.contains("conflict_auto_resolve"))
                .unwrap_or(false)
        })
        .expect("必须落 conflict_auto_resolve 审计");
    let details = audit.details.as_deref().unwrap_or("");
    assert!(
        details.contains("\"ours\""),
        "审计必须记录 ours 臂: {details}"
    );
    assert!(
        details.contains("\"theirs\""),
        "审计必须记录 theirs 臂: {details}"
    );
    // 未冻结（成功路径）。
    let pid = issue.project_id.unwrap();
    assert!(!deps.store.get_project(pid).unwrap().conflict_frozen);
    let _ = std::fs::remove_dir_all(&root);
}
