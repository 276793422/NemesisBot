use super::*;

// ---------------------------------------------------------------------------
// 名单构造
// ---------------------------------------------------------------------------

#[test]
fn default_list_covers_instruction_family() {
    let m = SensitiveReadMatcher::new(&[]);
    for name in DEFAULT_SENSITIVE_READ_NAMES {
        assert!(
            m.hit(name).is_some(),
            "default list must hit its own entry: {name}"
        );
    }
}

#[test]
fn extra_names_merge_with_dedup_case_insensitive() {
    let m = SensitiveReadMatcher::new(&[
        "SECRET.toml".to_string(),
        "agent.md".to_string(), // 与内置 AGENT.md 大小写不敏感去重
        "  ".to_string(),       // 空白项丢弃
        "".to_string(),         // 空项丢弃
    ]);
    assert!(m.hit("SECRET.toml").is_some(), "extra entry must merge");
    assert_eq!(m.hit("secret.toml").as_deref(), Some("SECRET.toml"));
    // 去重：agent.md 与内置 AGENT.md 同键——不产生重复条目（命中仍成功）。
    assert_eq!(m.hit("AGENT.md").as_deref(), Some("AGENT.md"));
}

// ---------------------------------------------------------------------------
// 命中判定
// ---------------------------------------------------------------------------

#[test]
fn basename_match_is_case_insensitive() {
    let m = SensitiveReadMatcher::new(&[]);
    assert_eq!(m.hit("C:/ws/agents.md").as_deref(), Some("AGENTS.md"));
    assert_eq!(m.hit("/home/u/ws/Claude.md").as_deref(), Some("CLAUDE.md"));
    assert_eq!(m.hit("AGENT.MD").as_deref(), Some("AGENT.md"));
    assert_eq!(m.hit("identity.md").as_deref(), Some("IDENTITY.md"));
}

#[test]
fn subdirectory_paths_hit_same_as_root() {
    let m = SensitiveReadMatcher::new(&[]);
    // 子目录同名文件与根同名同权（basename 匹配）。
    assert_eq!(m.hit("docs/AGENT.md").as_deref(), Some("AGENT.md"));
    assert_eq!(
        m.hit("C:\\ws\\nested\\deep\\SOUL.md").as_deref(),
        Some("SOUL.md")
    );
    assert_eq!(m.hit("./config/USER.md").as_deref(), Some("USER.md"));
}

#[test]
fn non_matching_and_variant_names_pass() {
    let m = SensitiveReadMatcher::new(&[]);
    // 普通文件放行。
    assert!(m.hit("C:/ws/src/main.rs").is_none());
    assert!(m.hit("README.md").is_none());
    // 精确语义：扩展名变体 / 前后缀粘连不命中。
    assert!(m.hit("AGENT.md.bak").is_none());
    assert!(m.hit("myAGENT.md").is_none());
    assert!(m.hit("AGENTS.md2").is_none());
    // basename 为空的形态不命中。
    assert!(m.hit("").is_none());
    assert!(m.hit("   ").is_none());
    assert!(m.hit("/").is_none());
}

// ---------------------------------------------------------------------------
// 结构化 args 深扫兜底（2026-11-20 第二轮横扫，批 4 漏网）
// ---------------------------------------------------------------------------

#[test]
fn hit_in_args_scans_dynamic_tool_unknown_schema() {
    // 动态注册 FileRead 工具（MCP 桥/WASM/connector）名字任意、schema
    // 任意——extract_target 按名穷举追不上（target 恒空）。深扫对嵌套
    // 结构里的路径字符串值命中。
    let m = SensitiveReadMatcher::new(&[]);
    let args = serde_json::json!({
        "options": {"file": ["docs/AGENTS.md"]},
        "mode": "text"
    });
    assert_eq!(m.hit_in_args(&args).as_deref(), Some("AGENTS.md"));
}

#[test]
fn hit_in_args_tokenizes_freeform_text() {
    // 自由文本多 token 形态（git args "-p -- AGENTS.md"）——target 通道
    // basename 匹配 miss，深扫逐空白 token 命中。
    let m = SensitiveReadMatcher::new(&[]);
    assert_eq!(
        m.hit_in_args(&serde_json::json!({"args": "-p -- AGENTS.md"}))
            .as_deref(),
        Some("AGENTS.md")
    );
    // rev:path 形态（git show HEAD:AGENTS.md，单 token 无空白）——
    // 深扫按 ':' 分段兜底命中。
    assert_eq!(
        m.hit_in_args(&serde_json::json!({"args": "HEAD:AGENTS.md"}))
            .as_deref(),
        Some("AGENTS.md")
    );
}

#[test]
fn hit_in_args_quiet_on_normal_payloads() {
    let m = SensitiveReadMatcher::new(&[]);
    // 普通载荷零命中。
    assert!(
        m.hit_in_args(&serde_json::json!({"path": "src/main.rs", "pattern": "fn main"}))
            .is_none()
    );
    assert!(m.hit_in_args(&serde_json::json!({})).is_none());
    assert!(m.hit_in_args(&serde_json::Value::Null).is_none());
    // 精确语义保持：变体名（AGENT.md.bak）不因深扫放宽。
    assert!(
        m.hit_in_args(&serde_json::json!({"path": "AGENT.md.bak"}))
            .is_none()
    );
    // 深度上限 4：第 5 层的命中静默放行（畸形深嵌套不挂死）。
    let deep = serde_json::json!({"a": {"b": {"c": {"d": {"e": {"f": "AGENT.md"}}}}}});
    assert!(m.hit_in_args(&deep).is_none());
    // 但限内嵌套命中照常（第 3 层）。
    let mid = serde_json::json!({"a": {"b": {"c": "SOUL.md"}}});
    assert_eq!(m.hit_in_args(&mid).as_deref(), Some("SOUL.md"));
}
