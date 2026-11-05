//! workflow_chat_reply_observer.rs AGT 覆盖率批次（2026-09-25）：Observer
//! 名牌臂（45-47）——既有 s10b/tests 只驱动 on_event，从未触碰 name()。

use super::*;
use crate::session::SessionManager;

#[test]
fn agt_observer_name_is_stable_contract() {
    let observer = WorkflowChatReplyObserver::new(
        std::sync::Arc::new(SessionManager::with_default_timeout()),
        std::sync::Arc::new(nemesis_workflow::engine::WorkflowEngine::new()),
    );
    // <WorkflowObserver as 本模块 impl>::name()——经 trait 对象同款路径调用。
    let n: &str = WorkflowObserver::name(&observer);
    assert_eq!(n, "workflow_chat_reply_observer");
}
