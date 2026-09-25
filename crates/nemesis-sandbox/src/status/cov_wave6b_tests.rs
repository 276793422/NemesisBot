//! status.rs 覆盖率收尾（Wave6B）：`service_binary_path` 的 None 收口
//! （sc qc 对不存在服务退出非零）与 `engine_owned_with_states` 的 foreign
//! false 臂（注册态服务 + binary path 查不到 → 不归我们 → 拒绝）。
//!
//! 只读 SCM 查询，服务名用唯一前缀确保 NotFound，零系统副作用。

use super::*;

/// sc qc 查不到服务 → None（62 收口行）。
#[test]
fn service_binary_path_none_for_unregistered_service() {
    let got = service_binary_path("nb_covw6b_nosuch_svc_xyz");
    assert!(got.is_none(), "未注册服务必须 None: {got:?}");
}

/// 注册态服务 + binary path 查不到 → 不归我们 → return false（100 行）。
#[test]
fn engine_owned_with_states_foreign_when_registered_and_unresolvable() {
    let owned = engine_owned_with_states(
        r"c:\covw6b\definitely\not\our\runtime",
        &[("nb_covw6b_nosuch_svc_xyz", ServiceState::Stopped)],
    );
    assert!(!owned, "注册态 + 查不到 binary → 必须按 foreign 处理");
}

/// 对照：零服务条目 → 全部名字空闲 → 归我们（103 收口行，true 臂）。
#[test]
fn engine_owned_with_states_true_when_nothing_registered() {
    assert!(engine_owned_with_states(r"c:\any\runtime", &[]));
}
