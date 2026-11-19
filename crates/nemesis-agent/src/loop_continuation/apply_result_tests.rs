//! 续行回主路（2026-11-12）B1#2：`apply_real_result_to_history` 场景单测。
//!
//! 计划 §6 矩阵行「marker 替换四场景」：① 命中原位替换；② 压缩折叠→
//! user 兜底；③ 合成快照→user 兜底；④ task_failed→"Error:" 文本。
//! 另加：②.5 assistant tool_calls 在场但 tool 轮缺席→紧随后补 tool 轮
//! （l5 复扫哨兵依赖）、图片引用补挂、幂等覆盖、dangling 兜底不产 tool turn、
//! ⑤ 埋没聚焦（T8 并发回归根修：marker 不在尾部→替换+尾部聚焦轮）。

use super::Applied;
use super::apply_real_result_to_history;
use crate::types::{ConversationTurn, ToolCallInfo};

fn marker_tool_turn(call_id: &str, marker: &str) -> ConversationTurn {
    ConversationTurn {
        role: "tool".to_string(),
        content: marker.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: Some(call_id.to_string()),
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: Some("cluster_rpc".to_string()),
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

fn assistant_with_call(call_id: &str) -> ConversationTurn {
    ConversationTurn {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: call_id.to_string(),
            name: "cluster_rpc".to_string(),
            arguments: "{}".to_string(),
        }],
        tool_call_id: None,
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

fn user_turn(text: &str) -> ConversationTurn {
    ConversationTurn {
        role: "user".to_string(),
        content: text.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

/// ① 正常形态：marker 轮原位替换，轮数不变、role/tool_call_id 保持。
#[test]
fn scenario1_marker_turn_replaced_in_place() {
    let mut history = vec![
        user_turn("帮我审查"),
        assistant_with_call("c1"),
        marker_tool_turn("c1", "__ASYNC__"),
    ];
    let applied =
        apply_real_result_to_history(&mut history, "c1", "task-42", "审查完成：3 个问题", &[]);
    assert!(matches!(applied, Applied::ReplacedMarker));
    assert_eq!(history.len(), 3, "原位替换不增轮");
    let tool = history
        .iter()
        .find(|t| t.role == "tool")
        .expect("tool 轮仍在");
    assert_eq!(tool.content, "审查完成：3 个问题");
    assert_eq!(tool.tool_call_id.as_deref(), Some("c1"));
    assert!(
        !history
            .iter()
            .any(|t| t.role == "user" && t.content.contains("task-42")),
        "不追加 user turn"
    );
}

/// ②.5 快照恢复形态：assistant 轮带同 id tool_calls 但 tool 轮缺席
/// （l5 种子形态）→ 紧随其后补 tool 轮（provider 原生形态：结果=工具
/// 输出 + 补齐悬挂 tool_use；前置 assistant 带对应 tool_calls，非
/// dangling，repair Pass 1 不删）。幂等：二次回灌命中补出的 tool 轮 →
/// 原位替换。
#[test]
fn assistant_call_without_tool_slot_appends_tool_turn() {
    let mut history = vec![user_turn("帮我审查"), assistant_with_call("c1")];
    let applied =
        apply_real_result_to_history(&mut history, "c1", "task-42", "审查完成：3 个问题", &[]);
    assert!(matches!(applied, Applied::AppendedToolTurn));
    assert_eq!(history.len(), 3, "assistant 后紧邻补一轮");
    let tool = &history[2];
    assert_eq!(tool.role, "tool");
    assert_eq!(tool.content, "审查完成：3 个问题");
    assert_eq!(tool.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(
        tool.tool_name.as_deref(),
        Some("cluster_rpc"),
        "工具名取自 assistant tool_calls"
    );
    // 幂等：二次回灌命中已补的 tool 轮 → 替换不追加。
    let applied2 =
        apply_real_result_to_history(&mut history, "c1", "task-42", "第二版（最新）", &[]);
    assert!(matches!(applied2, Applied::ReplacedMarker));
    assert_eq!(history.len(), 3);
    assert_eq!(history[2].content, "第二版（最新）");
}

/// ② 压缩折叠：store 历史找不到 marker 轮（covers_up_to 越过）→ 追加
/// user turn 兜底（不追加 dangling tool turn——repair Pass 1 会删）。
#[test]
fn scenario2_compacted_marker_falls_back_to_user_turn() {
    let mut history = vec![user_turn("很早的问题"), user_turn("压缩后仍在前的新问题")];
    let applied =
        apply_real_result_to_history(&mut history, "c-gone", "task-7", "后台审查结果", &[]);
    assert!(matches!(applied, Applied::AppendedUserTurn));
    assert_eq!(history.len(), 3);
    let tail = history.last().unwrap();
    assert_eq!(tail.role, "user");
    assert!(tail.content.contains("task-7"), "兜底文本带任务号");
    assert!(tail.content.contains("后台审查结果"));
    assert!(
        !history
            .iter()
            .any(|t| t.role == "tool" && t.tool_call_id.as_deref() == Some("c-gone")),
        "兜底臂绝不产生 dangling tool turn"
    );
}

/// ③ 合成快照（/build 直派发，store 从无 marker 轮）→ 同 ② 兜底。
#[test]
fn scenario3_synthetic_snapshot_falls_back_to_user_turn() {
    let mut history = vec![user_turn("第一问"), user_turn("第二问")];
    let applied = apply_real_result_to_history(&mut history, "c-none", "task-9", "构建完成", &[]);
    assert!(matches!(applied, Applied::AppendedUserTurn));
    assert_eq!(history.len(), 3);
    assert_eq!(history.last().unwrap().role, "user");
}

/// ④ task_failed：content = "Error: {task_error}" 文本（与旧语义同文），
/// 走替换臂时 marker 内容被错误文本覆盖。
#[test]
fn scenario4_task_failed_error_text_replaces_marker() {
    let mut history = vec![
        user_turn("派个任务"),
        assistant_with_call("c1"),
        marker_tool_turn("c1", "__ASYNC__"),
    ];
    let applied =
        apply_real_result_to_history(&mut history, "c1", "task-err", "Error: worker 超时", &[]);
    assert!(matches!(applied, Applied::ReplacedMarker));
    let tool = history.iter().find(|t| t.role == "tool").unwrap();
    assert!(tool.content.starts_with("Error:"));
}

/// 幂等覆盖：重复回灌（同 call_id）→ 内容覆盖为最新、不追加第二轮。
#[test]
fn repeated_delivery_overwrites_latest() {
    let mut history = vec![
        user_turn("q"),
        assistant_with_call("c1"),
        marker_tool_turn("c1", "__ASYNC__"),
    ];
    apply_real_result_to_history(&mut history, "c1", "t", "第一版", &[]);
    apply_real_result_to_history(&mut history, "c1", "t", "第二版（最新）", &[]);
    assert_eq!(history.len(), 3);
    let tool = history.iter().find(|t| t.role == "tool").unwrap();
    assert_eq!(tool.content, "第二版（最新）");
}

/// ⑤ 埋没聚焦（T8 并发回归根修）：marker 轮在场但不在历史尾部（并发
/// 窗口内后续用户轮先跑完，尾部拖着别的任务的未决 async marker）→
/// 原位替换 + 尾部补聚焦 user turn——模型按尾部作答，聚焦轮保证回复
/// 对准被续行任务。
#[test]
fn scenario5_buried_marker_gets_focus_user_turn() {
    let mut history = vec![
        user_turn("第一问"),
        assistant_with_call("c1"),
        marker_tool_turn("c1", "__ASYNC__"),
        // 并发窗口：后续用户轮已推进，尾部是别的任务的未决 marker。
        user_turn("第二问（后到）"),
        assistant_with_call("c2"),
        marker_tool_turn("c2", "__ASYNC__"),
    ];
    let applied =
        apply_real_result_to_history(&mut history, "c1", "task-42", "审查完成：3 个问题", &[]);
    assert!(matches!(applied, Applied::ReplacedMarker));
    // 槽位原位替换仍在。
    let slot = history
        .iter()
        .find(|t| t.role == "tool" && t.tool_call_id.as_deref() == Some("c1"))
        .expect("c1 槽位仍在");
    assert_eq!(slot.content, "审查完成：3 个问题");
    // 尾部聚焦轮：user、带任务号与结果、位于历史末位。
    let tail = history.last().unwrap();
    assert_eq!(tail.role, "user");
    assert!(tail.content.contains("task-42"));
    assert!(tail.content.contains("审查完成：3 个问题"));
    // 尾部不再是别的任务的未决 marker。
    assert!(
        !matches!(tail.role.as_str(), "tool") || tail.tool_call_id.as_deref() != Some("c2"),
        "聚焦轮必须顶掉未决 marker 的尾部地位"
    );
}

/// ⑤ 对照臂：槽位恰在尾部（顺序续行主形态）不追加聚焦轮——请求字节
/// 不变保 prompt cache（scenario1 已断言轮数不变，此处再钉「尾部仍为
/// tool 轮」的精确形态）。
#[test]
fn tail_slot_stays_tool_turn_no_focus() {
    let mut history = vec![
        user_turn("帮我审查"),
        assistant_with_call("c1"),
        marker_tool_turn("c1", "__ASYNC__"),
    ];
    apply_real_result_to_history(&mut history, "c1", "task-42", "结果", &[]);
    let tail = history.last().unwrap();
    assert_eq!(tail.role, "tool");
    assert_eq!(tail.tool_call_id.as_deref(), Some("c1"));
}

/// ⑤ 兜底一级臂（快照恢复形态）+ 埋没：assistant 在历史中段、其后有
/// 后续轮 → 补 tool 轮 + 尾部聚焦轮双写。
#[test]
fn appended_tool_turn_buried_also_gets_focus() {
    let mut history = vec![
        assistant_with_call("c1"),
        // tool 轮缺席（快照恢复/l5 种子形态），其后已有后续轮。
        user_turn("后续问"),
        assistant_with_call("c2"),
        marker_tool_turn("c2", "__ASYNC__"),
    ];
    let applied = apply_real_result_to_history(&mut history, "c1", "task-42", "审查完成", &[]);
    assert!(matches!(applied, Applied::AppendedToolTurn));
    // 紧随 assistant 的 tool 轮已补。
    assert_eq!(history[1].role, "tool");
    assert_eq!(history[1].tool_call_id.as_deref(), Some("c1"));
    // 尾部聚焦轮。
    let tail = history.last().unwrap();
    assert_eq!(tail.role, "user");
    assert!(tail.content.contains("task-42") && tail.content.contains("审查完成"));
}

/// 兜底臂补挂图片引用（快照最后一条 user 轮的 image_refs 透传）。
#[test]
fn fallback_arm_carries_image_refs() {
    let mut history = vec![user_turn("q")];
    let imgs = vec!["/ws/uploads/a.png".to_string()];
    let applied = apply_real_result_to_history(&mut history, "cx", "t-1", "带图结果", &imgs);
    assert!(matches!(applied, Applied::AppendedUserTurn));
    assert_eq!(history.last().unwrap().image_refs, imgs);
}
