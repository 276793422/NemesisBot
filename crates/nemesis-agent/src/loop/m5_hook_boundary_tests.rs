//! M5（2026-10-27 整改）：写后钩子路径解析边界测试。
//!
//! 缺陷：写后钩子（format-on-save / 诊断回灌）从**原始 args** 抠 path——
//! 模型给相对路径而进程 cwd ≠ 工作区根时，钩子摸到 cwd 下的同名文件
//! （误格式化 / 误诊断），与执行器两套真相源。修复 = 钩子处经
//! `resolve_tool_path` + loop 持有的同一 `WorkspaceBoundary` 重算。
//!
//! 本文件钉死三层：注入回路（set → getter 同 Arc）、相对路径锚定工作区根
//! （cwd 同名文件不背锅）、越界绝对路径解析失败（钩子诚实跳过）。

use std::sync::Arc;

use async_trait::async_trait;

use super::AgentLoop;
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use crate::loop_tools::{WorkspaceBoundary, resolve_tool_path};
use crate::types::ChatOptions;

struct NeverProvider;

#[async_trait]
impl LlmProvider for NeverProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("NeverProvider must not be called".to_string())
    }
}

fn test_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(NeverProvider),
        crate::AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 3,
            tools: Vec::new(),
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

fn ws_boundary(root: std::path::PathBuf) -> WorkspaceBoundary {
    WorkspaceBoundary {
        root,
        restrict: true,
        extra_roots: Vec::new(),
    }
}

/// 注入回路：set_workspace_boundary 后 getter 拿到**同一个 Arc**（与工厂
/// 「tool_config.workspace_boundary 与 loop 钩子边界同一实例」的契约对齐）。
#[test]
fn set_workspace_boundary_roundtrip_shares_arc() {
    let lp = test_loop();
    assert!(
        lp.hook_boundary_for_test().is_none(),
        "未装配 = None（standalone 形态）"
    );
    let boundary = Arc::new(ws_boundary(std::env::temp_dir()));
    lp.set_workspace_boundary(Arc::clone(&boundary));
    let got = lp.hook_boundary_for_test().expect("装配后必须可读回");
    assert!(
        Arc::ptr_eq(&got, &boundary),
        "getter 必须拿到同一 Arc（单一真相源）"
    );
}

/// 相对路径锚定工作区根——cwd 下同名文件不被钩子摸到（缺陷面钉死）。
#[test]
fn hook_resolution_anchors_relative_paths_to_workspace_root_not_cwd() {
    let ws = tempfile::tempdir().unwrap();
    let boundary = ws_boundary(ws.path().to_path_buf());
    let resolved =
        resolve_tool_path("sub/a.rs", Some(&boundary)).expect("界内相对路径必须解析成功");
    let expected = ws.path().join("sub").join("a.rs");
    assert_eq!(resolved, expected, "落点必须是工作区根拼接");
    assert_ne!(
        resolved,
        std::env::current_dir().unwrap().join("sub").join("a.rs"),
        "解析基点绝不能是进程 cwd（钩子误摸同名文件的根因）"
    );
}

/// 越界绝对路径解析失败 → 钩子 filter_map 诚实跳过（resolve 返回 Err）。
#[test]
fn hook_resolution_rejects_out_of_workspace_absolute_path() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let boundary = ws_boundary(ws.path().to_path_buf());
    let target = outside.path().join("x.rs");
    let err = resolve_tool_path(&target.to_string_lossy(), Some(&boundary))
        .expect_err("越界绝对路径必须被拒");
    assert!(err.contains("outside the workspace"), "{err}");
}

/// 无界形态（None）：路径原样透传（基线/测试行为不变——M5 修复对无界
/// 形态零行为差）。
#[test]
fn hook_resolution_without_boundary_passes_through() {
    let p = resolve_tool_path("rel/a.rs", None).expect("无界透传");
    assert_eq!(p, std::path::PathBuf::from("rel/a.rs"));
}
