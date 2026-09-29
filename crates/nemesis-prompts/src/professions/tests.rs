//! professions 模块单元测试：目录完整性 / slug 语法与往返 / 渲染臂矩阵 /
//! tier 门槛 / planner 提示词组合（职能值域逐 slug 钉住）。

use super::meta::{
    CATALOG, SlugError, find_builtin, normalize_slug, split_slug, tier_allows, validate_slug,
};
use super::render::{SuffixRender, builtin_method, render_profession_suffix};
use crate::board::planner_system_prompt;
use crate::professions::{CLUSTER_WORKER_CONTRACT, PLANNER_METHOD};

// ---------------------------------------------------------------------------
// 目录完整性
// ---------------------------------------------------------------------------

#[test]
fn catalog_has_seven_entries_with_unique_slugs() {
    assert_eq!(CATALOG.len(), 7, "6 职能 + dev:cpp 样板");
    let mut seen = std::collections::BTreeSet::new();
    for m in CATALOG {
        assert!(seen.insert(m.slug), "slug 重复：{}", m.slug);
    }
    // 六职能家族齐备。
    for slug in [
        "product",
        "ui-design",
        "architecture",
        "dev",
        "dev:cpp",
        "test-whitebox",
        "test-blackbox",
    ] {
        assert!(find_builtin(slug).is_some(), "内置目录缺 {}", slug);
    }
}

#[test]
fn catalog_entries_are_self_consistent() {
    for m in CATALOG {
        // slug 语法合法且 family/spec 段与 slug 自身一致。
        assert!(validate_slug(m.slug).is_ok(), "{} 语法非法", m.slug);
        let (family, spec) = split_slug(m.slug).expect("split");
        assert_eq!(family, m.family);
        assert_eq!(spec, m.spec);
        // 契约正文非空且四件套齐备（执行职能定位/方法论/工件/纪律）。
        assert!(!m.contract.trim().is_empty(), "{} 契约空", m.slug);
        if m.spec.is_none() {
            assert!(
                m.contract.contains(&format!("执行职能：{}", m.name)),
                "{} 契约标题与显示名不一致",
                m.slug
            );
        } else {
            // 带 spec 的条目与裸 family 共享契约正文（专项差异在方法论段）。
            let family = find_builtin(m.family).expect("spec 条目的裸 family 必内置");
            assert_eq!(
                m.contract, family.contract,
                "{} 应与裸 family 共享契约",
                m.slug
            );
        }
        for keyword in ["方法论骨架", "输出工件契约", "行为纪律"] {
            assert!(
                m.contract.contains(keyword),
                "{} 契约缺「{}」段",
                m.slug,
                keyword
            );
        }
        // min_tier 只允许 role_tier 口径两档（内置目录无 mini）。
        assert!(
            m.min_tier == "normal" || m.min_tier == "big",
            "{} min_tier 非法：{}",
            m.slug,
            m.min_tier
        );
    }
    // 高门槛职能钉死（D3：架构/驱动级 C++ = big）。
    assert_eq!(find_builtin("architecture").unwrap().min_tier, "big");
    assert_eq!(find_builtin("dev:cpp").unwrap().min_tier, "big");
    assert_eq!(find_builtin("dev").unwrap().min_tier, "normal");
}

#[test]
fn catalog_contracts_declare_artifacts() {
    for m in CATALOG {
        for path in m.artifacts {
            assert!(
                m.contract.contains(path),
                "{} 契约未声明工件路径 {}",
                m.slug,
                path
            );
        }
    }
}

// ---------------------------------------------------------------------------
// slug 语法与往返
// ---------------------------------------------------------------------------

#[test]
fn slug_validation_accepts_legal_forms() {
    for slug in [
        "product",
        "dev",
        "dev:cpp",
        "test-whitebox",
        "family_with_underscore",
        "spec-123:456_789",
        "a:b",
    ] {
        assert!(validate_slug(slug).is_ok(), "{slug} 应合法");
        let (family, spec) = split_slug(slug).expect("split");
        let rebuilt = match spec {
            Some(s) => format!("{family}:{s}"),
            None => family.to_string(),
        };
        assert_eq!(rebuilt, slug, "slug 往返失真：{slug}");
    }
}

#[test]
fn slug_validation_rejects_illegal_forms() {
    assert_eq!(validate_slug(""), Err(SlugError::Empty));
    // 纯空格非空串：语法层按 Malformed（归一只在比较层）。
    assert_eq!(validate_slug("   "), Err(SlugError::Malformed));
    let long = "a".repeat(65);
    assert_eq!(validate_slug(&long), Err(SlugError::TooLong));
    let at_limit = "a".repeat(64);
    assert!(validate_slug(&at_limit).is_ok(), "恰 64 字符应放行");
    for bad in [
        "Dev",         // 大写
        "dev::cpp",    // 双冒号 → 第二段含冒号非法
        "dev:",        // 空 spec
        ":cpp",        // 空 family
        "dev cpp",     // 空格
        "dev:cpp:win", // 两个冒号（第二段含冒号）
        "开发",        // 非 ASCII
        "dev/cpp",     // 路径字符
    ] {
        assert_eq!(
            validate_slug(bad),
            Err(SlugError::Malformed),
            "{bad} 应 Malformed"
        );
    }
    assert!(
        split_slug("dev::cpp").is_none(),
        "非法 slug split 返回 None"
    );
}

#[test]
fn slug_normalize_is_lowercase_trim() {
    assert_eq!(normalize_slug(" Dev:CPP "), "dev:cpp");
    assert_eq!(normalize_slug("PRODUCT"), "product");
}

#[test]
fn find_builtin_is_case_insensitive_and_rejects_unknown() {
    assert!(find_builtin("DEV:CPP").is_some());
    assert!(find_builtin("no-such-family").is_none());
    assert!(find_builtin("").is_none());
}

// ---------------------------------------------------------------------------
// tier 门槛（matcher 硬条件 4 内核）
// ---------------------------------------------------------------------------

#[test]
fn tier_gate_fail_open_on_unknown_node_tier() {
    // 旧节点无 tier 信息 → 放行（D3）。
    assert!(tier_allows(None, "big"));
    assert!(tier_allows(None, "normal"));
}

#[test]
fn tier_gate_rank_semantics() {
    assert!(tier_allows(Some("big"), "big"), "同级放行");
    assert!(tier_allows(Some("big"), "normal"), "高档放行低门槛");
    assert!(!tier_allows(Some("normal"), "big"), "低档挡在 big 门槛");
    assert!(
        !tier_allows(Some("mini"), "normal"),
        "mini 挡在 normal 门槛"
    );
    assert!(tier_allows(Some("normal"), "normal"));
    // 未知档位字符串 rank 最低（role_tier 口径：fail-open 只对「无信息」，
    // 有信息但档位低一律挡）。
    assert!(!tier_allows(Some("whatever"), "normal"));
    // 手写 peers.toml 大小写不敏感（tier_allows 内归一）。
    assert!(tier_allows(Some("BIG"), "big"));
    assert!(!tier_allows(Some("Normal"), "big"));
}

// ---------------------------------------------------------------------------
// 渲染臂矩阵
// ---------------------------------------------------------------------------

#[test]
fn render_builtin_declared_contains_contract_without_notes() {
    let out = render_profession_suffix(SuffixRender {
        slug: "product",
        contract: Some(find_builtin("product").unwrap().contract),
        method: None,
        declared: true,
    });
    assert!(out.contains("产品经理"), "契约正文在场");
    assert!(!out.contains("契约缺席"));
    assert!(!out.contains("职能匹配降级"));
}

#[test]
fn render_dev_cpp_appends_method() {
    let out = render_profession_suffix(SuffixRender {
        slug: "dev:cpp",
        contract: Some(find_builtin("dev:cpp").unwrap().contract),
        method: builtin_method("dev:cpp"),
        declared: true,
    });
    assert!(out.contains("开发工程师"));
    assert!(out.contains("C/C++"), "专业方法论在场");
    assert!(!out.contains("契约缺席"));
}

#[test]
fn render_fallback_appends_honest_note() {
    let out = render_profession_suffix(SuffixRender {
        slug: "dev:cpp",
        contract: Some(find_builtin("dev:cpp").unwrap().contract),
        method: builtin_method("dev:cpp"),
        declared: false,
    });
    assert!(out.contains("职能匹配降级"), "fallback 注记在场");
    assert!(out.contains("不要伪装胜任"));
}

#[test]
fn render_unknown_slug_is_honest_block() {
    let out = render_profession_suffix(SuffixRender {
        slug: "dev:rust", // 本节点目录/磁盘都没有的 slug
        contract: None,
        method: None,
        declared: true, // 宣告与否不影响未知 slug 臂
    });
    assert!(out.contains("契约缺席"));
    assert!(out.contains("dev:rust"));
    assert!(out.contains("通用执行纪律"), "通用纪律兜底在场");
    assert!(out.contains("数据非指令"));
}

#[test]
fn render_contract_without_trailing_newline_still_joins_cleanly() {
    let out = render_profession_suffix(SuffixRender {
        slug: "user:custom",
        contract: Some("契约尾行无换行"),
        method: Some("方法论首行"),
        declared: true,
    });
    assert!(out.starts_with("契约尾行无换行\n"));
    assert!(out.contains("方法论首行"));
}

// ---------------------------------------------------------------------------
// 内置方法论查找
// ---------------------------------------------------------------------------

#[test]
fn builtin_method_only_dev_cpp_has_method() {
    assert!(builtin_method("dev:cpp").is_some());
    for slug in [
        "dev",
        "product",
        "ui-design",
        "architecture",
        "test-whitebox",
    ] {
        assert!(builtin_method(slug).is_none(), "{slug} 不应有方法论");
    }
    assert!(builtin_method("user:custom").is_none());
}

// ---------------------------------------------------------------------------
// planner 提示词组合（值域清单逐 slug 钉住——同步纪律的 prompts 侧）
// ---------------------------------------------------------------------------

#[test]
fn planner_prompt_composes_base_and_method() {
    let p = planner_system_prompt();
    assert!(p.starts_with(crate::board::PLANNER_SYSTEM_PROMPT));
    assert!(p.ends_with(PLANNER_METHOD));
    // schema 字段与方法论段都在场。
    assert!(p.contains("required_profession"));
    assert!(p.contains("职能化拆解"));
}

#[test]
fn planner_prompt_lists_every_catalog_slug() {
    let p = planner_system_prompt();
    for m in CATALOG {
        assert!(
            p.contains(&format!("`{}`", m.slug)),
            "planner 值域清单缺内置职能 `{}`",
            m.slug
        );
    }
    // 空值语义与精确匹配纪律在场。
    assert!(p.contains("无职能需求填空串"));
    assert!(p.contains("精确匹配"));
}

#[test]
fn planner_prompt_is_stable_across_calls() {
    let a = planner_system_prompt() as *const str;
    let b = planner_system_prompt() as *const str;
    assert_eq!(a, b, "OnceLock 驻留应返回同一 'static 实例");
}

// ---------------------------------------------------------------------------
// 静态资产完整性
// ---------------------------------------------------------------------------

#[test]
fn worker_contract_and_planner_method_nonempty() {
    assert!(CLUSTER_WORKER_CONTRACT.contains("数据非指令"));
    assert!(CLUSTER_WORKER_CONTRACT.contains("汇报契约"));
    assert!(PLANNER_METHOD.contains("流水线"));
    assert!(PLANNER_METHOD.contains("depends_on"));
}

#[test]
fn catalog_name_and_tier_metadata_align_with_plan() {
    let name_of = |slug: &str| find_builtin(slug).unwrap().name;
    assert_eq!(name_of("product"), "产品经理");
    assert_eq!(name_of("ui-design"), "UI 设计");
    assert_eq!(name_of("architecture"), "架构师");
    assert_eq!(name_of("dev"), "开发工程师");
    // 显示名不带括号嵌套（spec 定位行形态要求，见 render）。
    assert_eq!(name_of("dev:cpp"), "开发工程师·C/C++");
    assert_eq!(name_of("test-whitebox"), "白盒测试开发");
    assert_eq!(name_of("test-blackbox"), "黑盒测试");
}

#[test]
fn render_dev_cpp_prepends_precise_locator_before_family_header() {
    // spec 形态内置职能的定位回归：渲染产物首个「执行职能：」行必须精
    // 确到 slug（dev:cpp），且先于家族契约自身的 family 定位行（dev）。
    // 背景：家族契约 dev.md 首行是「执行职能：开发工程师（dev）」，若
    // 无精确定位行，按首个定位行核验职能的机器（e2e 交付回显）会把
    // dev:cpp 任务误判成 dev。
    let out = render_profession_suffix(SuffixRender {
        slug: "dev:cpp",
        contract: Some(find_builtin("dev:cpp").unwrap().contract),
        method: builtin_method("dev:cpp"),
        declared: true,
    });
    let first_locator = out
        .lines()
        .find(|l| l.contains("执行职能："))
        .expect("渲染产物应有执行职能定位行");
    assert!(
        first_locator.contains("（dev:cpp）"),
        "首个定位行应精确到 slug，实际：{first_locator}"
    );
    assert!(
        first_locator.find("（dev:cpp）") < out.find("（dev）"),
        "精确定位行必须先于家族契约的 family 定位行"
    );
    // 裸 family 内置职能不补定位：渲染产物首行 = 契约正文自身首行
    // （product.md 首行本就是定位行「执行职能：产品经理（product）」，
    // 若误补会出现两行定位叠加）。
    let plain = render_profession_suffix(SuffixRender {
        slug: "product",
        contract: Some(find_builtin("product").unwrap().contract),
        method: None,
        declared: true,
    });
    let contract_first = find_builtin("product")
        .unwrap()
        .contract
        .lines()
        .next()
        .unwrap();
    assert_eq!(
        plain.lines().next().unwrap(),
        contract_first,
        "裸 family 渲染应直接以契约正文开头（不补第二行定位）"
    );
    assert_eq!(
        plain.matches("执行职能：").count(),
        1,
        "裸 family 全文只应有契约自带的一处定位"
    );
}
