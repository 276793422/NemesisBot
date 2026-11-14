//! BUG-20261114-034 回归锁：config.skills.json 双解析面默认值漂移闸。
//!
//! 同一份 `config.skills.json` 由两套类型解析：
//! - Dashboard 面：`nemesis_config::SkillsFullConfig`（web handlers/skills.rs
//!   load/save 全走它——typed save 会把解析缺省物化回盘）；
//! - 运行时面：`nemesis_skills::types::RegistryConfig`（gateway ctx / agent CLI /
//!   skills CLI 三装配点）。
//!
//! 两面必须对缺省键给出相同答案。此前的分裂（修复前）：search_cache.enabled 与
//! modelscope.enabled 运行时面缺省 false、Dashboard 面缺省 true——运行时缓存
//! 静默关 / ModelScope 源静默不注册，而 UI 显示已启用；github_sources[].enabled
//! 反向分裂（运行时 true / Dashboard false），typed save 把 false 物化回盘翻转
//! 运行时行为。
//!
//! 本测试是 MCP 单一真相源收敛先例（skills_mcp.rs 收敛注记）的漂移闸形态：
//! 结构收敛（runtime 复用 config 类型）因依赖方向未做，则以契约测试钉住两面对齐。

use nemesis_config::SkillsFullConfig;
use nemesis_skills::types::RegistryConfig;

/// 极简配置（只有必需键）——两面解析结果必须逐键一致且等于出厂意图（true）。
#[test]
fn dual_parse_faces_agree_on_minimal_config() {
    let json = r#"{"github_sources": [{"name": "n", "repo": "o/r"}]}"#;
    let runtime: RegistryConfig = serde_json::from_str(json).expect("runtime face parse");
    let dashboard: SkillsFullConfig = serde_json::from_str(json).expect("dashboard face parse");

    // search_cache.enabled：修复前运行时 false / Dashboard true。
    assert_eq!(
        runtime.search_cache.enabled, dashboard.search_cache.enabled,
        "search_cache.enabled 双解析面答案分裂"
    );
    assert!(
        runtime.search_cache.enabled,
        "运行时面 search_cache 缺省应启用（出厂模板显式 true）"
    );

    // modelscope.enabled：修复前运行时 false（源静默不注册）/ Dashboard true。
    assert_eq!(
        runtime.modelscope.enabled, dashboard.modelscope.enabled,
        "modelscope.enabled 双解析面答案分裂"
    );
    assert!(
        runtime.modelscope.enabled,
        "运行时面 ModelScope 缺省应注册（出厂模板显式 true）"
    );

    // github_sources[].enabled：修复前 Dashboard false（typed save 物化 false
    // 翻转运行时行为）/ 运行时 true。
    assert_eq!(
        runtime.github_sources[0].enabled, dashboard.github_sources[0].enabled,
        "github_sources[].enabled 双解析面答案分裂"
    );
    assert!(
        runtime.github_sources[0].enabled,
        "缺 enabled 键的源条目两面都应视为启用"
    );
}

/// 共享数值缺省逐键对齐（防止未来单侧调整造成静默漂移）。
#[test]
fn dual_parse_faces_agree_on_shared_numeric_defaults() {
    let runtime = RegistryConfig::default();
    let dashboard = SkillsFullConfig::default();

    assert_eq!(
        runtime.search_cache.max_size, dashboard.search_cache.max_size as usize,
        "search_cache.max_size 两面缺省分裂"
    );
    assert_eq!(
        runtime.search_cache.ttl_secs, dashboard.search_cache.ttl_seconds as u64,
        "search_cache TTL 两面缺省分裂"
    );
    assert_eq!(
        runtime.max_concurrent_searches, dashboard.max_concurrent_searches as usize,
        "max_concurrent_searches 两面缺省分裂"
    );
    assert!(
        runtime.search_cache.enabled && dashboard.search_cache.enabled,
        "search_cache.enabled 两面缺省都应为 true"
    );
    assert!(
        runtime.modelscope.enabled && dashboard.modelscope.enabled,
        "modelscope.enabled 两面缺省都应为 true"
    );
}

/// ClawHub enabled 两面同答（false——均未启用，`with_urls` 对空 URL 有显式
/// fallback，URL 缺省分裂无害，见 clawhub_registry.rs 空串回落注释）。
#[test]
fn dual_parse_faces_agree_on_clawhub_enabled() {
    let json = r#"{}"#;
    let runtime: RegistryConfig = serde_json::from_str(json).unwrap();
    let dashboard: SkillsFullConfig = serde_json::from_str(json).unwrap();
    assert_eq!(
        runtime.clawhub.enabled, dashboard.clawhub.enabled,
        "clawhub.enabled 双解析面答案分裂"
    );
}
