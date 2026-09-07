// K4 (devtool-upgrade 阶段 7)：IM 编码入口测试。
//
// 覆盖面：
// ① 派发语法解析 `parse_user_dispatch`——/build /plan 双前缀、裸命令与
//    无 repo: token 不匹配（落普通轮不猜）、节点 token 必须最后、空任务
//    文本拒绝；
// ② 自足续行快照构造 `build_dispatch_snapshot_messages`——最小合法序列
//    [user, assistant(tool_calls)]，tool_call id/name/args 与派发参数
//    一致（merge_real_tool_result push 后即严格 provider 接受的标准序列，
//    该合并行为本身已由 loop_continuation 侧覆盖）；
// ③ B 端变更摘要渲染 `render_turn_changes_summary`——空 None / 全量列出
//    / 封顶 + 溢出诚实注记 / kind 投影；确定性（同输入同字节）。
//
// 三者均为纯函数（快照/摘要渲染无 I/O），直接单元验证；端到端编排
// （gate → handle_tool_call → ACK → 续行）依赖集群 RPC 形态，由
// cluster-uat 真机场景覆盖（与 __ASYNC__ 路径共享生产机制）。

use super::*;

// ---------------------------------------------------------------------------
// ① 派发语法解析
// ---------------------------------------------------------------------------

#[test]
fn parse_build_with_repo_token() {
    let (task, node) = parse_user_dispatch("/build fix-login repo:node-b").unwrap();
    assert_eq!(task, "fix-login");
    assert_eq!(node, "node-b");
}

#[test]
fn parse_plan_prefix_accepted() {
    let (task, node) = parse_user_dispatch("/plan 重构登录模块 repo:worker-1").unwrap();
    assert_eq!(task, "重构登录模块");
    assert_eq!(node, "worker-1");
}

#[test]
fn parse_multi_word_task_and_trimming() {
    let (task, node) =
        parse_user_dispatch("  /build   修复登录  并补测试   repo:node-b  ").unwrap();
    assert_eq!(task, "修复登录 并补测试");
    assert_eq!(node, "node-b");
}

#[test]
fn parse_bare_command_not_matched() {
    // 裸 /build（无尾随参数）= F1 模式切换，不属于派发语法。
    assert!(parse_user_dispatch("/build").is_none());
    assert!(parse_user_dispatch("/plan").is_none());
    assert!(parse_user_dispatch("/build   ").is_none());
}

#[test]
fn parse_missing_repo_token_not_matched() {
    // 无 repo: token → 落普通轮（现有行为），不猜不做。
    assert!(parse_user_dispatch("/build fix-login").is_none());
}

#[test]
fn parse_repo_token_must_be_last() {
    // repo: 不是最后一个 token → 不匹配（可预测优先，不猜）。
    assert!(parse_user_dispatch("/build fix repo:node-b extra").is_none());
}

#[test]
fn parse_empty_parts_rejected() {
    // repo: 空节点名 / 空任务文本都拒绝。
    assert!(parse_user_dispatch("/build fix repo:").is_none());
    assert!(parse_user_dispatch("/build repo:node-b").is_none());
}

#[test]
fn parse_unrelated_slash_not_matched() {
    assert!(parse_user_dispatch("/compact").is_none());
    assert!(parse_user_dispatch("帮我修一下登录").is_none());
}

// ---------------------------------------------------------------------------
// ② 自足续行快照构造
// ---------------------------------------------------------------------------

#[test]
fn snapshot_messages_shape_is_minimal_valid_sequence() {
    let msgs = build_dispatch_snapshot_messages("fix-login", "node-b", "ud-abc");
    assert_eq!(msgs.len(), 2);

    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "fix-login");
    assert!(msgs[0].tool_calls.is_none());
    assert!(msgs[0].tool_call_id.is_none());

    assert_eq!(msgs[1].role, "assistant");
    let tcs = msgs[1].tool_calls.as_ref().unwrap();
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].id, "ud-abc");
    assert_eq!(tcs[0].name, "cluster_rpc");
    // args 与派发参数一致（JSON roundtrip 验证字段值）。
    let args: serde_json::Value = serde_json::from_str(&tcs[0].arguments).unwrap();
    assert_eq!(args["target"], "node-b");
    assert_eq!(args["message"], "fix-login");
    // merge_real_tool_result 无既有 tool 槽位 → push 到末尾 → tool 消息
    // 紧跟 assistant tool_calls 消息（严格 provider 接受）。
    let merged =
        crate::loop_continuation::merge_real_tool_result(msgs, "ud-abc", "done".to_string());
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[2].role, "tool");
    assert_eq!(merged[2].tool_call_id.as_deref(), Some("ud-abc"));
    assert_eq!(merged[2].content, "done");
}

// ---------------------------------------------------------------------------
// ③ B 端变更摘要渲染
// ---------------------------------------------------------------------------

fn fc(path: &str, kind: FileChangeKind) -> FileChange {
    FileChange {
        path: path.to_string(),
        kind,
    }
}

#[test]
fn summary_empty_changes_is_none() {
    assert!(render_turn_changes_summary(&[]).is_none());
}

#[test]
fn summary_lists_all_and_projects_kinds() {
    let changes = vec![
        fc("src/main.rs", FileChangeKind::Modify),
        fc("src/new.rs", FileChangeKind::Create),
        fc("src/old.rs", FileChangeKind::Delete),
    ];
    let s = render_turn_changes_summary(&changes).unwrap();
    assert!(s.contains("共 3 个"));
    assert!(s.contains("- src/main.rs (modify)"));
    assert!(s.contains("- src/new.rs (create)"));
    assert!(s.contains("- src/old.rs (delete)"));
    // 无溢出注记。
    assert!(!s.contains("未列出"));
}

#[test]
fn summary_caps_with_honest_overflow_note() {
    let changes: Vec<FileChange> = (0..25)
        .map(|i| fc(&format!("f{i}.rs"), FileChangeKind::Modify))
        .collect();
    let s = render_turn_changes_summary(&changes).unwrap();
    assert!(s.contains("共 25 个"));
    assert!(s.contains("- f19.rs (modify)"));
    assert!(!s.contains("- f20.rs"));
    assert!(s.contains("另有 5 个文件未列出"));
}

#[test]
fn summary_is_deterministic() {
    let changes = vec![
        fc("a.rs", FileChangeKind::Create),
        fc("b.rs", FileChangeKind::Modify),
    ];
    assert_eq!(
        render_turn_changes_summary(&changes),
        render_turn_changes_summary(&changes)
    );
}
