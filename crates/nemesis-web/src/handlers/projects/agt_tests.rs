//! projects.rs AGT 覆盖率批次（2026-09-25）。与 projects_tests（兄弟模块，
//! 经 bridge 槽走 handler 全链）互补，聚焦 trait **默认实现**体：
//! - display_label 默认 = pid 原样兜底（MockBridge 覆写了它，默认体从未
//!   执行；本批用不覆写的最小桩直接调）
//! - reload_provider_all 默认 no-op（agent.rs:188 / models.rs:477 的联动
//!   调用在 bridge 覆写形态下走实现方，默认体同样从未执行）
//!
//! 结构性豁免（见报告）：
//! - open_in_file_manager（190-215）：spawn 的是 explorer/open/xdg-open
//!   ——在宿主桌面真实弹窗口（违反 AGENTS.md 后台纪律：绝不弹窗）；错误
//!   臂要求系统常驻二进制 spawn 失败，无平台无关触发手段。测试不碰。

use super::*;

/// 最小桩：只实现必需方法（全部诚实空态），两个默认方法不覆写。
struct AgtNoopBridge;

impl ProjectsBridge for AgtNoopBridge {
    fn list(&self) -> Vec<ProjectInfo> {
        Vec::new()
    }
    fn create(&self, _name: &str, _path: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn remove(&self, _project_id: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn rename(&self, _project_id: &str, _new_name: &str) -> Result<ProjectInfo, String> {
        Err("noop".to_string())
    }
    fn owner_of(&self, _session_key: &str) -> Option<String> {
        None
    }
    fn loop_for_session(&self, _project_id: &str) -> Option<Arc<AgentLoop>> {
        None
    }
    fn project_path(&self, _project_id: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn bind_session(&self, _session_key: &str, _project_id: &str) -> Result<(), String> {
        Err("noop".to_string())
    }
    fn forget_session(&self, _session_key: &str) {}
}

#[test]
fn agt_bridge_default_display_label_and_reload_provider_all() {
    let b = AgtNoopBridge;
    // display_label 默认：pid 原样兜底（真实三级回落由 nemesisbot 侧覆写）。
    assert_eq!(b.display_label("p-xyz", "agent:main:session:s1"), "p-xyz");
    assert_eq!(b.display_label("", ""), "");
    // reload_provider_all 默认：no-op（可调用即覆盖默认体，无副作用可断言）。
    b.reload_provider_all();
}
