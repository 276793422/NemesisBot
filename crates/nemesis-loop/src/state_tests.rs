//! state 文本闸单测（独立文件纪律——生产文件零内联测试）：
//! goal（`LoopState::new`）与进度条目（`push_progress`）超长封顶。
//!
//! 背景（BUG-20261128-012）：无界资源族——loops 模块（2026-11-17）落地
//! 晚于 R272 无界资源普查（2026-10-30），入口曾无长度闸；4000 字符与
//! goals gate 评估材料封顶同族口径。消费侧（唤醒消息对存量超长文件的
//! 兜底）见 wake_tests。

use crate::state::LoopState;

// 预期封顶值（字面量钉死契约值；实现侧常量漂移会被这里抓住）。
const EXPECTED_CAP: usize = 4000;
const CAP_MARKER: &str = "…（截断）";

#[test]
fn new_caps_oversize_goal() {
    let st = LoopState::new(
        "L1".to_string(),
        "G".repeat(5000),
        "agent:main:session:a".to_string(),
        0,
    );
    let n = st.goal.chars().count();
    assert!(
        n <= EXPECTED_CAP + CAP_MARKER.chars().count(),
        "goal 未封顶：{n} chars"
    );
    assert!(
        st.goal.starts_with(&"G".repeat(EXPECTED_CAP)),
        "封顶应保留头部"
    );
    assert!(st.goal.ends_with(CAP_MARKER), "封顶应带截断标记");
}

#[test]
fn push_progress_caps_oversize_entry() {
    let mut st = LoopState::new(
        "L1".to_string(),
        "ok".to_string(),
        "agent:main:session:a".to_string(),
        0,
    );
    st.push_progress(1, "N".repeat(5000));
    let last = st.progress_log.last().expect("entry 存在");
    let n = last.text.chars().count();
    assert!(
        n <= EXPECTED_CAP + CAP_MARKER.chars().count(),
        "进度条目未封顶：{n} chars"
    );
    assert!(last.text.ends_with(CAP_MARKER), "封顶应带截断标记");
}

#[test]
fn short_texts_pass_through() {
    let st = LoopState::new(
        "L1".to_string(),
        "每天站会前整理看板".to_string(),
        "agent:main:session:a".to_string(),
        0,
    );
    assert_eq!(st.goal, "每天站会前整理看板");
    let mut st = st;
    st.push_progress(1, "完成一步");
    assert_eq!(st.progress_log[0].text, "完成一步");
}
