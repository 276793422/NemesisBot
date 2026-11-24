//! 回归锁：画像注入缓存键必须含 config.json mtime。
//!
//! 此前缓存键只有画像文件（user_profile.json）mtime——enabled 开关翻转
//! 只动 config.json 不动画像文件，且 disable 后 sweep no-op 永不再写
//! 画像（mtime 冻结）→ 缓存永不过期：关闭的开关在进程生命周期内不
//! 生效，画像段仍持续注入每条消息（隐私开关失效；enable 反向延迟一个
//! sweep 周期）。修复后 config 或画像任一变化即重渲染。

use super::*;

fn cov_mock_loop() -> AgentLoop {
    struct StaticProvider;
    #[async_trait]
    impl LlmProvider for StaticProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "ok".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }
    AgentLoop::new(
        Box::new(StaticProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            system_prompt_split: None,
            max_turns: 2,
            tools: vec![],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

fn write_config(path: &std::path::Path, enabled: bool) {
    std::fs::write(
        path,
        serde_json::json!({"user_profile": {"enabled": enabled}}).to_string(),
    )
    .unwrap();
}

fn write_profile(ws: &std::path::Path) {
    let dir = ws.join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("user_profile.json"),
        serde_json::json!({
            "enabled_at": "2026-10-25T00:00:00+08:00",
            "habits": [{
                "category": "style",
                "content": "用户消息简短直接，回复宜简洁",
                "confidence": 0.8,
                "hits": 3,
                "first_seen": "2026-10-25T00:00:00+08:00",
                "last_seen": "2026-10-25T00:00:00+08:00",
                "status": "active",
                "pinned": false
            }]
        })
        .to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn profile_section_reflects_enabled_flip_without_profile_file_change() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_profile(&ws);
    let cfg_path = ws.join("config").join("config.json");
    write_config(&cfg_path, true);

    let al = cov_mock_loop();
    al.set_workspace_root(ws.clone());
    al.set_config_path(cfg_path.clone());

    // enabled=true → 画像段注入。
    let on = al.user_profile_section_now();
    assert!(
        on.contains("用户消息简短直接"),
        "enabled=true must inject profile, got: {on:?}"
    );

    // 翻转 enabled=false（只动 config.json，画像文件不动）→ 立即停注。
    // 修复前：缓存键只看画像 mtime → 命中旧段 → 此断言红（开关失效）。
    std::thread::sleep(std::time::Duration::from_millis(60)); // mtime 分辨率
    write_config(&cfg_path, false);
    let off = al.user_profile_section_now();
    assert!(
        off.is_empty(),
        "BUG-20261025-36: disable must take effect immediately, got: {off:?}"
    );

    // 翻回 true → 立即恢复注入。
    std::thread::sleep(std::time::Duration::from_millis(60));
    write_config(&cfg_path, true);
    let back = al.user_profile_section_now();
    assert!(
        back.contains("用户消息简短直接"),
        "re-enable must restore injection, got: {back:?}"
    );
}

/// 项目会话画像注入缺口修复回归锁（2026-11-24）：项目 loop 的
/// workspace_root 是项目目录，画像资产在主 workspace——不注入 profile_root
/// 时画像段恒空（读项目目录下永不存在的 user_profile.json）。修复后
/// set_profile_root(主workspace) 显式注入优先。
#[tokio::test]
async fn profile_root_override_reads_main_workspace_profile_for_project_loops() {
    let dir = tempfile::tempdir().unwrap();
    let main_ws = dir.path().join("main-ws");
    let project_dir = dir.path().join("my-app");
    std::fs::create_dir_all(&main_ws).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();
    // 画像只在主 workspace；项目目录下无画像文件。
    write_profile(&main_ws);
    let cfg_path = main_ws.join("config").join("config.json");
    write_config(&cfg_path, true);

    // 未注入 profile_root（旧行为）：workspace_root=项目目录 → 画像读
    // 项目目录 → 恒空段（= 修复前的缺口形态）。
    let bare = cov_mock_loop();
    bare.set_workspace_root(project_dir.clone());
    bare.set_config_path(cfg_path.clone());
    assert!(
        bare.user_profile_section_now().is_empty(),
        "workspace_root=project dir without override must NOT inject (pre-fix gap shape)"
    );

    // 注入 profile_root=主 workspace：画像段照常注入（修复形态）。
    let fixed = cov_mock_loop();
    fixed.set_workspace_root(project_dir.clone());
    fixed.set_profile_root(main_ws.clone());
    fixed.set_config_path(cfg_path.clone());
    let section = fixed.user_profile_section_now();
    assert!(
        section.contains("用户消息简短直接"),
        "profile_root override must read main workspace profile, got: {section:?}"
    );

    // 主/集群 loop 回落形态：只设 workspace_root（=画像根）行为不变。
    let main_loop = cov_mock_loop();
    main_loop.set_workspace_root(main_ws.clone());
    main_loop.set_config_path(cfg_path.clone());
    assert!(
        main_loop
            .user_profile_section_now()
            .contains("用户消息简短直接"),
        "no override = fallback to workspace_root (main/cluster loops unchanged)"
    );
}
