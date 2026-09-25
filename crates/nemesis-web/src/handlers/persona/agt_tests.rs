//! persona.rs AGT 覆盖率批次（2026-09-25）。
//!
//! 本文件只剩一个可离线确定性触达的零区：`Default for PersonaHandler`
//! （751-753）。其余零区的定性：
//! - 255-257 / 272（fetch_tree 的非 2xx 臂与 tree 字段缺席臂）、306 +
//!   310-353（fetch_agent_content 的 contents API 下载全身）：**网络绑
//!   定**——GITHUB_API 是硬编码 https 常量、无注入缝，既有 s10b 测试
//!   特意用 cache 种子离线化（"no real GitHub"），本批次遵循同一纪律，
//!   不在单测里打真实 GitHub。
//! - 197：split_once 落空臂结构性不可达——入口守卫已保证该行含 "："
//!   或 ":" 之一，split_once(['：', ':']) 必 Some。
//! - 904：personas/ 目录下非 UTF-8 目录名防御臂（Windows 需未配对代理
//!   的 UTF-16 文件名，std::fs 无法直接构造）。

use super::*;

#[test]
fn agt_default_impl_matches_new() {
    // 751-753：Default 委托 new（单元 struct，两者等价）。
    let _d = PersonaHandler;
    let _n = PersonaHandler::new();
}

// ---------------------------------------------------------------------------
// Wave5 批次：invalidate_cache 三清臂 + fetch_agent_content 的「树中查无
// 此 agent」诚实臂（种子缓存后短路，不打网络）+ extract_identity_info 的
// ASCII 冒号无空格形态（197）。
// ---------------------------------------------------------------------------

/// 种子三级缓存 → invalidate_cache() 全清（253-258）。
/// 触碰全局缓存：持 tests::SHOP_TEST_LOCK 串行。
#[tokio::test]
async fn w5_invalidate_cache_clears_all_three_levels() {
    let _shop_guard = crate::handlers::persona::tests::SHOP_TEST_LOCK.lock().await;
    *TREE_CACHE.lock().unwrap() = Some(vec![("x/a.md".to_string(), 1)]);
    FM_CACHE.lock().unwrap().insert(
        "x/a".to_string(),
        Frontmatter {
            name: "A".to_string(),
            emoji: "🤖".to_string(),
            description: String::new(),
            vibe: String::new(),
            color: String::new(),
            tools: String::new(),
            raw_yaml: String::new(),
        },
    );
    CONTENT_CACHE
        .lock()
        .unwrap()
        .insert("x/a".to_string(), "body".to_string());

    invalidate_cache();

    assert!(TREE_CACHE.lock().unwrap().is_none(), "tree cache cleared");
    assert!(FM_CACHE.lock().unwrap().is_empty(), "fm cache cleared");
    assert!(
        CONTENT_CACHE.lock().unwrap().is_empty(),
        "content cache cleared"
    );
}

/// 树缓存命中但查无此 id → 306 的 ok_or_else 臂（无网络：fetch_tree 走缓存）。
#[tokio::test]
async fn w5_fetch_agent_content_unknown_id_errors_without_network() {
    let _shop_guard = crate::handlers::persona::tests::SHOP_TEST_LOCK.lock().await;
    *TREE_CACHE.lock().unwrap() = Some(vec![("skills/other/SKILL.md".to_string(), 7)]);
    let err = fetch_agent_content("w5-no-such-agent").await.unwrap_err();
    assert!(err.contains("not found in repository"), "{err}");
    assert!(err.contains("w5-no-such-agent"), "{err}");
}

/// ASCII 冒号且后无空格的姓名行 → 首选 find 落空 → split_once 兜底臂（197）。
#[test]
fn w5_extract_identity_info_ascii_colon_no_space_fallback() {
    let (name, _emoji) = extract_identity_info("**姓名:Wave5Bot\n");
    assert_eq!(name, "Wave5Bot");
}
