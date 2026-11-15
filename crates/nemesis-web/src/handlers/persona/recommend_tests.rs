//! 推荐橱窗 + 源参数 + builtin 伪源（2026-11-15 策展映射，方案 A）测试。
//!
//! 离线策略：目录源走种子缓存（同 [`super::tests`] 口径，SHOP_TEST_LOCK
//! 串行）；builtin 臂直接消费编译期内嵌模板（零网络零写盘断言）。
//! 另含开发期生成闸（NEMESIS_GEN_BUILTIN=1）与漂移锁（已入库四件套 ==
//! 转换管线输出，字节相等）。

use super::tests::{SHOP_TEST_LOCK, shop_ctx};
use super::*;

fn seed_tree_and_fm_with(items: &[(&str, &str, &str, &str)]) {
    let mut tree = Vec::new();
    let mut fm = FM_CACHE.lock().unwrap();
    fm.clear();
    for (path, name, emoji, desc) in items {
        tree.push((path.to_string(), 100i64));
        // 复合键（2026-11-15）：`<source>/<id>`，browse 按 agency 复合键取。
        fm.insert(
            format!(
                "agency/{}",
                path.rsplit('/').next().unwrap().trim_end_matches(".md")
            ),
            Frontmatter {
                name: name.to_string(),
                emoji: emoji.to_string(),
                description: desc.to_string(),
                vibe: String::new(),
                color: String::new(),
                tools: String::new(),
                raw_yaml: String::new(),
            },
        );
    }
    *TREE_CACHE.lock().unwrap() = Some(tree);
}

// ------------------------------------------------------------
// 纯助手：ref 归一 / 档位注记 / 源参数裁决
// ------------------------------------------------------------

#[test]
fn normalize_ref_and_tier_helpers() {
    assert_eq!(
        normalize_recommend_ref("code-reviewer"),
        "agency/code-reviewer"
    );
    assert_eq!(
        normalize_recommend_ref("builtin/bug-analyst"),
        "builtin/bug-analyst"
    );

    // 六职能键有档位注记（数据源 = professions CATALOG min_tier）。
    assert_eq!(recommend_section_tier("product"), Some("normal"));
    assert_eq!(recommend_section_tier("architecture"), Some("big"));
    // 方向键不在 CATALOG → None（不硬编档位）。
    assert_eq!(recommend_section_tier("code-review"), None);
    assert_eq!(recommend_section_tier("bug-analysis"), None);

    // 源参数：缺省 = 默认源；未知 key loud 报错。
    assert_eq!(resolve_shop_source(None).unwrap(), "agency");
    assert_eq!(
        resolve_shop_source(Some(&serde_json::json!({}))).unwrap(),
        "agency"
    );
    assert_eq!(
        resolve_shop_source(Some(&serde_json::json!({"source": "builtin"}))).unwrap(),
        "builtin"
    );
    let err = resolve_shop_source(Some(&serde_json::json!({"source": "nope"}))).unwrap_err();
    assert!(err.contains("unknown shop source 'nope'"), "{err}");
}

// ------------------------------------------------------------
// browse：recommend_sections 结构 / 顺序 / 防死链
// ------------------------------------------------------------

#[tokio::test]
async fn recommend_sections_shape_order_and_dead_ref_drop() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    // 只种三棵树（真实上游形态：<dir>/<dir>-<name>.md，id 带目录前缀）：
    // engineering-code-reviewer 是真实策展 ref（应正行命中），其余两棵
    // 不在策展表 → 多数 ref 解析不到被自动丢弃（不做假分类）。
    seed_tree_and_fm_with(&[
        (
            "engineering/engineering-rust-dev.md",
            "Rusty",
            "🦀",
            "Rust expert",
        ),
        (
            "security/security-pentest-pro.md",
            "Pentest",
            "🛡️",
            "Pentest expert",
        ),
        (
            "engineering/engineering-code-reviewer.md",
            "Code Reviewer",
            "👁️",
            "Reviewer",
        ),
    ]);

    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd("shop.browse", None, &ctx)
        .await
        .unwrap()
        .unwrap();

    let sections = out["recommend_sections"].as_array().unwrap();
    assert_eq!(sections.len(), RECOMMEND_SECTIONS.len());

    // 行排序：六职能在前（CATALOG 顺序），代码 Review / BUG 分析紧随。
    let keys: Vec<&str> = sections
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        &keys[..8],
        &[
            "product",
            "ui-design",
            "architecture",
            "dev",
            "test-whitebox",
            "test-blackbox",
            "code-review",
            "bug-analysis"
        ]
    );

    let by_key = |k: &str| sections.iter().find(|s| s["key"] == k).unwrap();
    // 档位注记只出现在六职能行（方向行 null）。
    assert_eq!(by_key("product")["tier"], "normal");
    assert_eq!(by_key("architecture")["tier"], "big");
    assert_eq!(by_key("test-whitebox")["tier"], "normal");
    // 方向行不在 CATALOG → null（不硬编档位）。
    assert!(by_key("code-review")["tier"].is_null());
    assert!(by_key("bug-analysis")["tier"].is_null());
    // 行头跳转预置 division（跨域方向 null）。
    assert_eq!(by_key("product")["division"], "产品");
    assert_eq!(by_key("ui-design")["division"], "创意");
    assert!(by_key("architecture")["division"].is_null());

    // 防死链：未播种的 agency ref 全部被丢弃；BUG 分析行只剩 builtin。
    let bug_refs = by_key("bug-analysis")["refs"].as_array().unwrap();
    assert_eq!(bug_refs.len(), 1);
    assert_eq!(bug_refs[0], "builtin/bug-analyst");
    let prod_refs = by_key("product")["refs"].as_array().unwrap();
    assert_eq!(prod_refs.len(), 0);

    // 正行命中：种下的真实策展 ref 落进白盒测试 + 代码 Review 两行
    //（一 id 多行），其余 ref 死链丢弃。
    for k in ["test-whitebox", "code-review"] {
        let refs = by_key(k)["refs"].as_array().unwrap();
        assert_eq!(refs, &["agency/engineering-code-reviewer"], "{k}");
    }

    // 每个 ref 都能落到 items（id + source 复合存在）。
    let items = out["items"].as_array().unwrap();
    for s in sections {
        for r in s["refs"].as_array().unwrap() {
            let (src, id) = r.as_str().unwrap().rsplit_once('/').unwrap();
            assert!(
                items.iter().any(|i| i["id"] == id && i["source"] == src),
                "ref {r} 没有对应条目"
            );
        }
    }
    invalidate_cache();
}

// ------------------------------------------------------------
// browse/search 投影：source + recommended_for 注入
// ------------------------------------------------------------

#[tokio::test]
async fn items_carry_source_and_recommended_for() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    // code-reviewer 同属白盒测试 + 代码 Review 两行（一 id 多行的反查注入）。
    // 真实上游形态：<dir>/<dir>-<name>.md，id 带目录前缀。
    seed_tree_and_fm_with(&[
        (
            "engineering/engineering-rust-dev.md",
            "Rusty",
            "🦀",
            "Rust expert",
        ),
        (
            "engineering/engineering-code-reviewer.md",
            "Code Reviewer",
            "👁️",
            "Reviewer",
        ),
    ]);

    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd("shop.browse", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let items = out["items"].as_array().unwrap();

    let reviewer = items
        .iter()
        .find(|i| i["id"] == "engineering-code-reviewer")
        .unwrap();
    let rec = reviewer["recommended_for"].as_array().unwrap();
    let titles: Vec<&str> = rec.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(titles.contains(&"白盒测试"), "{titles:?}");
    assert!(titles.contains(&"代码 Review"), "{titles:?}");
    assert_eq!(reviewer["source"], "agency");

    // 无策展映射的条目 recommended_for 空数组（不假分类）。
    let rusty = items
        .iter()
        .find(|i| i["id"] == "engineering-rust-dev")
        .unwrap();
    assert_eq!(rusty["recommended_for"].as_array().unwrap().len(), 0);

    // BUG 分析行随种子联动（test-results-analyzer 未播种不出现）。
    let bug = out["recommend_sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "bug-analysis")
        .unwrap();
    let refs: Vec<&str> = bug["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert_eq!(refs, vec!["builtin/bug-analyst"]);
    invalidate_cache();
}

// ------------------------------------------------------------
// builtin 伪源：download 释放 / 未知源报错 / 专家雇佣内嵌臂
// ------------------------------------------------------------

/// 集群聘用免先下载臂：builtin 源未安装 → live 强败（离线确定性）→ 内嵌
/// 模板释放后直接聘用（B2 语义与 download 兜底臂一致）。
#[cfg(feature = "cluster")]
#[tokio::test]
async fn builtin_hire_without_download_releases_template() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let _forced = ForcedBuiltinLiveFail::arm();
    let dir = tempfile::tempdir().unwrap();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    assert!(
        !dir.path().join("personas/bug-analyst").exists(),
        "前置：工作区无该人格"
    );
    let out = h
        .handle_cmd(
            "shop.hire",
            Some(serde_json::json!({"id": "bug-analyst", "source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["hired"], true);
    assert!(
        dir.path()
            .join("personas/bug-analyst/PERSONA.json")
            .exists(),
        "按需释放应落四件套"
    );
    // 目录源维持「必须先下载」前置不松动。
    let err = h
        .handle_cmd(
            "shop.hire",
            Some(serde_json::json!({"id": "rust-dev"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not installed"), "{err}");
}

#[tokio::test]
async fn builtin_download_releases_template_and_rejects_unknown_source() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    // live 强败 = 离线确定性走内嵌兜底臂（live 臂另行单测）。
    let _forced = ForcedBuiltinLiveFail::arm();
    let dir = tempfile::tempdir().unwrap();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);

    // 未知源 loud 报错（不静默落默认源）。
    let err = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "x", "source": "nope"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown shop source"), "{err}");

    // builtin 下载兜底臂：零网络释放内嵌五件套 + HEARTBEAT.md，
    // snapshot_info 携带「离线兜底」人读注记。
    let out = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "bug-analyst", "source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["downloaded"], true);
    assert_eq!(out["source"], "builtin");
    assert_eq!(out["name"], "Bug Analyst");
    assert_eq!(
        out["snapshot_info"]["label"], "编译期内嵌模板（离线兜底）",
        "兜底臂必须带人读注记"
    );

    let pdir = dir.path().join("personas/bug-analyst");
    for f in [
        "PERSONA.json",
        "IDENTITY.md",
        "SOUL.md",
        "AGENT.md",
        "TOOLS.md",
        "HEARTBEAT.md",
        "raw.md",
    ] {
        assert!(pdir.join(f).exists(), "缺 {f}");
    }

    // 重复下载诚实报错（安装态 = PERSONA.json 在场）。
    let err = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "bug-analyst", "source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("already installed"), "{err}");
    invalidate_cache();
}

#[tokio::test]
async fn builtin_expert_hire_payload_embedded_arm_without_writes() {
    // SHOP_TEST_LOCK 必持：本测试读 CONTENT_CACHE（取材链 live 臂先查缓存），
    // 并行套件下持锁测试（download live 注入 "builtin/bug-analyst"）的
    // seed→invalidate 窗口若被撞进，会读到 "Bug Analyst Live" 假名（全量
    // 套件实录 2026-11-15）；锁内互斥后窗口不可见。
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    // builtin live 强败（取材链 live 臂排在内嵌前）→ 命中内嵌臂：
    // 零写盘（personas/ 目录不应出现）。
    let _forced = ForcedBuiltinLiveFail::arm();
    let payload = expert_hire_payload(&ws, "bug-analyst").await.unwrap();
    assert_eq!(payload["slug"], "bug-analyst");
    assert_eq!(payload["name"], "Bug Analyst");
    let prompt = payload["prompt"].as_str().unwrap();
    assert!(prompt.contains("根因"), "prompt 应含纪律正文");
    assert!(!prompt.contains("name:"), "prompt 应已剥 frontmatter");
    assert!(
        !dir.path().join("personas").exists(),
        "内嵌臂零写盘：雇佣不落工作区文件"
    );
}

// ------------------------------------------------------------
// builtin 独立源（2026-11-15 自仓拉取）：browse 双臂 / download 取材序 /
// 复合键隔离 / 雇佣取材链
// ------------------------------------------------------------

/// browse{source:"builtin"} 内嵌兜底臂：live 强败（离线确定性）→ 编译期
/// 内嵌清单 + fm，`source=embedded` + 人读 label，策展推荐注记照常注入。
#[tokio::test]
async fn builtin_browse_embedded_fallback_when_live_forced_fail() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let _forced = ForcedBuiltinLiveFail::arm();
    let dir = tempfile::tempdir().unwrap();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.browse",
            Some(serde_json::json!({"source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["source"], "embedded");
    assert_eq!(
        out["snapshot_info"]["label"],
        format!("编译期内嵌模板（{} 个人格）", builtin_persona_ids().len())
    );
    let items = out["items"].as_array().unwrap();
    assert!(!items.is_empty(), "内嵌清单不应为空");
    let bug = items.iter().find(|i| i["id"] == "bug-analyst").unwrap();
    assert_eq!(bug["name"], "Bug Analyst", "内嵌 fm 应喂卡片名");
    assert_eq!(bug["division"], "内置");
    assert_eq!(bug["source"], "builtin");
    let rec = bug["recommended_for"].as_array().unwrap();
    assert!(
        rec.iter().any(|t| t == "BUG 分析"),
        "策展注记应注入：{rec:?}"
    );
    // 集群/默认 browse 合并臂不受影响（缺省 browse 仍合并 builtin）。
    invalidate_cache();
}

/// browse{source:"builtin"} live 臂（种子缓存离线验证）：清单+复合内容缓存
/// 齐备 → `source=live`、snapshot_info null、fm 进卡片、无 fm 条目 id 回落。
#[tokio::test]
async fn builtin_browse_live_arm_via_seeded_caches() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    *BUILTIN_LIST_CACHE.lock().unwrap() =
        Some(vec!["bug-analyst".to_string(), "ghost-persona".to_string()]);
    // 块作用域收 guard（sync Mutex 不得跨 await——clippy await_holding_lock）。
    {
        let mut content = CONTENT_CACHE.lock().unwrap();
        content.insert(
            "builtin/bug-analyst".to_string(),
            builtin_persona_raw("bug-analyst").unwrap().to_string(),
        );
        content.insert(
            "builtin/ghost-persona".to_string(),
            "---\nname: Ghost\ndescription: seeded live body\n---\n\nbody".to_string(),
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.browse",
            Some(serde_json::json!({"source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["source"], "live");
    assert!(out["snapshot_info"].is_null());
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let bug = items.iter().find(|i| i["id"] == "bug-analyst").unwrap();
    assert_eq!(bug["name"], "Bug Analyst");
    let ghost = items.iter().find(|i| i["id"] == "ghost-persona").unwrap();
    assert_eq!(ghost["name"], "Ghost");
    assert_eq!(ghost["description"], "seeded live body");
    invalidate_cache();
}

/// download builtin live 优先臂：复合内容缓存命中（= live 内容）→ 转换
/// 落盘走 live 原文而非内嵌模板（注入不同名内容区分两臂）。
#[tokio::test]
async fn builtin_download_prefers_live_over_embedded() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let live_raw = "---\nname: Bug Analyst Live\n---\n\nlive body marker";
    CONTENT_CACHE
        .lock()
        .unwrap()
        .insert("builtin/bug-analyst".to_string(), live_raw.to_string());

    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "bug-analyst", "source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        out["name"], "Bug Analyst Live",
        "live 原文必须优先于内嵌模板"
    );
    assert!(out["snapshot_info"].is_null(), "live 臂无兜底注记");
    let pj = std::fs::read_to_string(dir.path().join("personas/bug-analyst/PERSONA.json")).unwrap();
    assert!(
        pj.contains("Bug Analyst Live"),
        "落盘应来自 live 转换：{pj}"
    );
    let raw_saved =
        std::fs::read_to_string(dir.path().join("personas/bug-analyst/raw.md")).unwrap();
    assert_eq!(raw_saved, live_raw, "raw.md 存档 = live 原文");
    invalidate_cache();
}

/// 复合键隔离回归：同 id 双源并存，preview 按 source 精确取各自内容，
/// 不串号（裸键时代后写覆盖先写）。
#[tokio::test]
async fn composite_cache_keys_isolate_sources() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    {
        let mut content = CONTENT_CACHE.lock().unwrap();
        content.insert(
            "agency/dual".to_string(),
            "---\nname: Agency Side\n---\n\nA".to_string(),
        );
        content.insert(
            "builtin/dual".to_string(),
            "---\nname: Builtin Side\n---\n\nB".to_string(),
        );
    }
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let a = h
        .handle_cmd(
            "shop.preview",
            Some(serde_json::json!({"id": "dual", "source": "agency"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a["name"], "Agency Side");
    let b = h
        .handle_cmd(
            "shop.preview",
            Some(serde_json::json!({"id": "dual", "source": "builtin"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b["name"], "Builtin Side");
    invalidate_cache();
}

/// 雇佣取材链（2026-11-15 拍板）：本地缺席 → builtin live（落盘存档）
/// 优先于内嵌（内嵌臂由 [`builtin_expert_hire_payload_embedded_arm_without_writes`]
/// 在 live 强败下覆盖）。
#[tokio::test]
async fn expert_hire_uses_builtin_live_before_embedded() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let live_raw = "---\nname: Bug Analyst\ndescription: d\n---\n\nLIVE BODY MARKER";
    CONTENT_CACHE
        .lock()
        .unwrap()
        .insert("builtin/bug-analyst".to_string(), live_raw.to_string());

    let payload = expert_hire_payload(&ws, "bug-analyst").await.unwrap();
    assert_eq!(payload["prompt"], "LIVE BODY MARKER");
    // live 原文落盘存档（此后雇佣走本地，不再依赖上游）。
    let saved = std::fs::read_to_string(dir.path().join("personas/bug-analyst/raw.md")).unwrap();
    assert_eq!(saved, live_raw);
    invalidate_cache();
}

/// 搜索同 id 双源去重（2026-11-15 二轮逐行复查修）：与 browse 合并臂
/// 「目录源已有同名条目 → 不重复陈列」同款口径——agency 树已有同 id 条目
/// 时 builtin 条目不进搜索结果（防同名双卡；下载/雇佣落同一 personas/<id>/
/// 目录互扰）。
#[tokio::test]
async fn search_dedupes_builtin_against_agency_same_id() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    // agency 树种一个与内置同 id（bug-analyst）的条目 + 一个无关条目。
    seed_tree_and_fm_with(&[
        (
            "engineering/bug-analyst.md",
            "Agency Bug Analyst",
            "📌",
            "agency side",
        ),
        ("engineering/other-dev.md", "Other", "🛠️", "other"),
    ]);
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.search",
            Some(serde_json::json!({"query": "analyst"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "同 id 双源只陈列 agency 侧：{:?}",
        items
            .iter()
            .map(|i| (i["source"].clone(), i["id"].clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(items[0]["source"], "agency");
    assert_eq!(items[0]["id"], "bug-analyst");
    invalidate_cache();
}

/// division 过滤态 browse：策展 ref 解析不受过滤门控（known_refs 全集
/// 不变量，2026-11-15 逐行复查修）——BUG 分析行仍命中 builtin/bug-analyst，
/// 不误打「策展 id 错或上游改名/删除」WARN 假阳性；网格照常滤净 builtin。
#[tokio::test]
async fn division_filtered_browse_keeps_builtin_refs_resolvable() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    seed_tree_and_fm_with(&[(
        "security/security-pentest-pro.md",
        "Pentest",
        "🛡️",
        "Pentest expert",
    )]);
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.browse",
            Some(serde_json::json!({"division": "安全"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // 过滤态网格：只有 agency 安全类条目，builtin 条目不进列表。
    let items = out["items"].as_array().unwrap();
    assert!(
        !items.iter().any(|i| i["source"] == "builtin"),
        "过滤态网格不得混入 builtin"
    );
    // 策展解析不受过滤影响：BUG 分析行 builtin ref 照常命中（全集不变量）。
    let bug = out["recommend_sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "bug-analysis")
        .unwrap();
    let refs: Vec<&str> = bug["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert!(
        refs.contains(&"builtin/bug-analyst"),
        "过滤态策展 ref 不得假阳性丢弃：{refs:?}"
    );
    invalidate_cache();
}

/// preview id 闸（2026-11-15 逐行复查修）：builtin live 臂用裸 id 拼 raw
/// 直链，非法 id（穿越/路径分隔/冒号）必须在 dispatch loud 拒绝——与
/// download/hire/hire_expert 四臂同形，不给 URL 改写留直达面。
#[tokio::test]
async fn shop_preview_rejects_invalid_persona_id() {
    let dir = tempfile::tempdir().unwrap();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    for bad in ["../evil", "a/b", "x:y"] {
        let err = h
            .handle_cmd(
                "shop.preview",
                Some(serde_json::json!({"id": bad, "source": "builtin"})),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("invalid"), "{bad}: {err}");
    }
}

/// shop 写入面 id=default 保留名闸（2026-11-15 三轮逐行复查修）：download/
/// hire（builtin 免下载臂）/hire_expert（raw.md 存档）都物化写
/// personas/<id>/，default 归档不可被覆盖——cmd_remove 拒删 default 同一
/// 保留语义的写侧对称闸。preview 只读不设闸（假想上游 default.md 可预览
/// 不可下载）。不持 SHOP_TEST_LOCK：dispatch 拒绝发生在任何缓存触达之前。
/// "Default" 大写变体（2026-11-15 四轮补）：验证器无大小写约束，NTFS
/// 大小写不敏感文件系统上 personas/Default/ 与 personas/default/ 同目录，
/// 保留名比对必须 ASCII 大小写不敏感。
#[tokio::test]
async fn shop_write_arms_reject_reserved_default_id() {
    let dir = tempfile::tempdir().unwrap();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    for id in ["default", "Default", "DEFAULT"] {
        for cmd in ["shop.download", "shop.hire_expert"] {
            let err = h
                .handle_cmd(cmd, Some(serde_json::json!({"id": id})), &ctx)
                .await
                .unwrap_err();
            assert!(err.contains("保留"), "{cmd}/{id}: {err}");
        }
    }
    #[cfg(feature = "cluster")]
    {
        let err = h
            .handle_cmd(
                "shop.hire",
                Some(serde_json::json!({"id": "Default", "source": "builtin"})),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("保留"), "shop.hire: {err}");
    }
}

/// release 幂等续传 + PERSONA.json 压轴（2026-11-15 四轮逐行复查修）：
/// 模拟「部分态」（六件在场、PERSONA.json 缺席=假想写序中途失败）——
/// is_persona_installed 诚实呈未安装，重跑 release 在场跳过补齐压轴件，
/// 全量文件与模板逐字节一致。
#[test]
fn builtin_release_resumes_partial_state_with_persona_json_last() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    release_builtin_persona(&ws, "bug-analyst").unwrap();
    let root = dir.path().join("personas/bug-analyst");
    assert!(
        root.join("PERSONA.json").exists(),
        "首次释放应完整（含压轴 PERSONA.json）"
    );
    // 部分态注入：删掉提交标记（其余六件保留），模拟旧写序中途失败的现场。
    std::fs::remove_file(root.join("PERSONA.json")).unwrap();
    assert!(
        !is_persona_installed(&ws, "bug-analyst"),
        "部分态不得误判已安装"
    );
    // 重跑：在场跳过 + 缺席补齐（幂等续传）。
    release_builtin_persona(&ws, "bug-analyst").unwrap();
    assert!(
        is_persona_installed(&ws, "bug-analyst"),
        "续传后补齐压轴件=安装态成立"
    );
    // 续传产物与模板逐字节一致（在场跳过不吞模板更新面——此处模板只读）。
    let template = builtin_persona_dir("bug-analyst").unwrap();
    for f in template.files() {
        let name = f.path().file_name().and_then(|n| n.to_str()).unwrap();
        let disk = std::fs::read(root.join(name)).unwrap();
        assert_eq!(disk, f.contents(), "续传后 {name} 与模板不一致");
    }
}

// ------------------------------------------------------------
// 漂移锁 + 开发期生成闸
// ------------------------------------------------------------

/// 漂移锁：模板里已入库的四件套/PERSONA.json 必须 == 转换管线对内嵌
/// raw.md 的输出（字节相等）。raw.md 修改后重跑生成闸 + 提交，否则此
/// 测试红——消费链（对话安装）拿到的文件与创作真相源永不分叉。
#[test]
fn builtin_persona_files_match_conversion_pipeline() {
    let ids = builtin_persona_ids();
    assert!(
        ids.contains(&"bug-analyst".to_string()),
        "内嵌模板缺 bug-analyst（模板路径/结构损坏？）：{ids:?}"
    );
    for id in ids {
        let Some(raw) = builtin_persona_raw(&id) else {
            panic!("builtin '{id}' 缺 raw.md（发行物五件齐发口径）");
        };
        let c = convert_agent_content(raw);
        // include_dir get_file 匹配相对内嵌根的完整路径（<id>/<name>）。
        let read = |name: &str| -> String {
            BUILTIN_PERSONAS_DIR
                .get_file(format!("{id}/{name}"))
                .unwrap_or_else(|| panic!("builtin '{id}' 缺 {name}（先跑生成闸）"))
                .contents_utf8()
                .unwrap_or_else(|| panic!("builtin '{id}'/{name} 非 UTF-8"))
                .to_string()
        };
        assert_eq!(
            read("PERSONA.json"),
            c.persona_json,
            "{id}/PERSONA.json 漂移"
        );
        assert_eq!(read("IDENTITY.md"), c.identity, "{id}/IDENTITY.md 漂移");
        assert_eq!(read("SOUL.md"), c.soul, "{id}/SOUL.md 漂移");
        assert_eq!(read("AGENT.md"), c.agent_full, "{id}/AGENT.md 漂移");
        assert_eq!(read("TOOLS.md"), c.tools_full, "{id}/TOOLS.md 漂移");
    }
}

/// 策展 ref ↔ 上游真实 id 漂移锁：每个 agency ref 必须 ∈ 当日捕获的
/// 上游 id 快照（testdata_agency_roster.txt）。初版策展表手写裸 basename
/// （漏 `<dir>-` 目录前缀）全表错位，运行期防死链静默丢弃、真机冒烟才
/// 暴露 15 行只活 4 行——此锁把同类错误提前到 `cargo test` loud 红。
#[test]
fn recommend_refs_resolve_against_captured_roster() {
    let roster = include_str!("testdata_agency_roster.txt");
    let ids: std::collections::HashSet<&str> = roster
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .collect();
    // 快照完整性下限（防 fixture 被意外截断；不钉死总数——上游增长不该红测试）。
    assert!(ids.len() > 200, "roster 快照疑似损坏：{} 个 id", ids.len());
    for s in RECOMMEND_SECTIONS {
        for r in s.refs {
            if r.contains('/') {
                continue; // builtin 等非 agency ref 由内嵌模板漂移锁覆盖
            }
            assert!(
                ids.contains(r),
                "策展 ref '{}'（行 '{}'）不在上游 id 快照中——手写错 id 或上游已改名；取真实文件名去 .md 后更新表与快照",
                r,
                s.title
            );
        }
    }
}

/// 开发期生成闸（不进常规测试流）：NEMESIS_GEN_BUILTIN=1 时把每个内嵌
/// raw.md 走转换管线写回模板目录（四件套 + PERSONA.json + HEARTBEAT.md）。
/// 生成后重跑测试（重新编译内嵌）漂移锁自然绿；产物随模板一起提交。
#[test]
fn generate_builtin_persona_files_dev_gate() {
    if std::env::var("NEMESIS_GEN_BUILTIN").ok().as_deref() != Some("1") {
        return; // 诚实跳过：日常测试零 IO 零信号
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../nemesisbot/workspace/personas");
    for id in builtin_persona_ids() {
        let Some(raw) = builtin_persona_raw(&id) else {
            eprintln!("skip builtin '{id}': no raw.md");
            continue;
        };
        let c = convert_agent_content(raw);
        let dir = root.join(&id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PERSONA.json"), &c.persona_json).unwrap();
        std::fs::write(dir.join("IDENTITY.md"), &c.identity).unwrap();
        std::fs::write(dir.join("SOUL.md"), &c.soul).unwrap();
        std::fs::write(dir.join("AGENT.md"), &c.agent_full).unwrap();
        std::fs::write(dir.join("TOOLS.md"), &c.tools_full).unwrap();
        std::fs::write(dir.join("HEARTBEAT.md"), "").unwrap();
        eprintln!("generated builtin '{id}' template files");
    }
}
