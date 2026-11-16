//! builtin_registry 测试：内嵌技能面 + **统一漏斗分档探针**（回归锁）。
//!
//! 分档纪律（2026-11-16 用户拍板「全量上架，装时拦」）：13 个技能全部进
//! 源列表；安装时统一内容安全关照常裁决——过闸的可装，运维技能（taskkill
//! 清理 / wsl --shutdown / netstat 端口检查 / rm -rf 清理脚本是功能本体，
//! 不改文案不特例）装时诚实报拦截理由。本探针钉定「过闸 / 被拦」分档：
//! 技能内容或 lint 规则任何一方变化导致分档漂移，立刻在这里红出来供
//! 人工确认（新技能默认应落在过闸集，落不进去 = 上架前先想清楚）。

use std::path::Path;

use crate::builtin_registry::BuiltinSkillsRegistry;
use crate::registry::SkillRegistry;
use crate::security_check::check_skill_security_dir;
use crate::types::BrowseSort;

/// 内嵌技能总数锚（仓库 `Skills/` 目录当前 13 个；加技能改此锚 + 通知
/// 全链路——探针自动覆盖新技能，新技能默认须落在过闸集）。
const EXPECTED_SKILL_COUNT: usize = 13;

/// 统一内容安全关**被拦**分档锚（装时拦拍板的钉定集，字典序）。
/// 命中面是这些技能的功能本体（运维命令），不走文案改写、不走特例放行；
/// 列表全量上架，安装时由漏斗诚实报拦截理由。分档漂移（改内容/改 lint
/// 规则）= 本探针红，人工确认后才能动这份清单。
const EXPECTED_BLOCKED: &[&str] = &[
    "approval-popup-test",
    "automated-testing",
    "memory-uat",
    "scanner-e2e-test",
    "wsl-operations",
];

fn temp_target(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("builtin-reg-{tag}-"))
        .tempdir()
        .expect("tempdir")
}

/// 分档探针：13 个内嵌技能逐个物化整树 → 统一安全关 → 断言「过闸 / 被
/// 拦」分档与钉定清单一致（装时拦拍板的可执行形态）。失败信息自带现场：
/// 全部技能的 score / reason / warning 数逐条列出，分档漂移时定位依据。
#[tokio::test]
async fn probe_unified_security_gate_partition_matches_pinned_sets() {
    let slugs = BuiltinSkillsRegistry::skill_slugs();
    assert_eq!(
        slugs.len(),
        EXPECTED_SKILL_COUNT,
        "内嵌技能数漂移：{slugs:?}——确认仓库 Skills/ 目录后同步 EXPECTED_SKILL_COUNT"
    );

    let reg = BuiltinSkillsRegistry::new();
    let mut passed: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut scene: Vec<String> = Vec::new();
    for slug in &slugs {
        let target = temp_target("probe");
        let installed = reg
            .download_and_install(slug, "", target.path().to_string_lossy().as_ref())
            .await
            .unwrap_or_else(|e| panic!("install {slug} failed: {e}"));
        assert!(!installed.is_malware_blocked);

        let skill_md_path = target.path().join("SKILL.md");
        assert!(
            skill_md_path.exists(),
            "{slug}: SKILL.md missing after tree extraction"
        );
        let content = std::fs::read_to_string(&skill_md_path).expect("read SKILL.md");
        let check = check_skill_security_dir(target.path(), &content, slug, "");
        scene.push(format!(
            "{slug}: blocked={} score={:.0}/100 reason={} warnings={}",
            check.blocked,
            check.lint_result.score * 100.0,
            check.block_reason,
            check.lint_result.warnings.len()
        ));
        if check.blocked {
            blocked.push(slug.clone());
        } else {
            passed.push(slug.clone());
        }
    }
    // 断言双向一致：被拦集 == 钉定集（不多不少）；过闸集 == 补集。
    assert_eq!(
        blocked,
        EXPECTED_BLOCKED,
        "统一内容安全关分档漂移（装时拦拍板 2026-11-16）：\n现场：\n{}",
        scene.join("\n")
    );
    assert_eq!(
        passed.len(),
        EXPECTED_SKILL_COUNT - EXPECTED_BLOCKED.len(),
        "过闸集与被拦集不互补：{passed:?}"
    );
}

#[tokio::test]
async fn search_empty_query_lists_all_sorted() {
    let reg = BuiltinSkillsRegistry::new();
    let all = reg.search("", 100).await.expect("search");
    assert_eq!(all.len(), EXPECTED_SKILL_COUNT);
    // 显式排序（slug 字典序）——顺序是平台实现细节，不得依赖文件系统序。
    let mut sorted = all.iter().map(|r| r.slug.clone()).collect::<Vec<_>>();
    sorted.sort();
    let slugs: Vec<String> = all.iter().map(|r| r.slug.clone()).collect();
    assert_eq!(slugs, sorted);
    for r in &all {
        assert_eq!(r.registry_name, "builtin");
        assert!(!r.display_name.is_empty());
    }
}

#[tokio::test]
async fn search_matches_slug_and_summary_and_respects_limit() {
    let reg = BuiltinSkillsRegistry::new();
    let hit = reg.search("problem-analysis", 10).await.expect("search");
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].slug, "problem-analysis");

    // 摘要/展示名命中面（中文查询——「方法论」出现在 problem-analysis 正文）。
    let zh = reg.search("方法论", 10).await.expect("search zh");
    assert!(
        zh.iter().any(|r| r.slug == "problem-analysis"),
        "summary hit face missing: {zh:?}"
    );

    let limited = reg.search("", 3).await.expect("search limit");
    assert_eq!(limited.len(), 3);
}

#[tokio::test]
async fn meta_and_content_roundtrip() {
    let reg = BuiltinSkillsRegistry::new();
    let meta = reg.get_skill_meta("bug-fix").await.expect("meta");
    assert_eq!(meta.registry_name, "builtin");
    assert_eq!(meta.latest_version, "latest");
    assert!(!meta.is_malware_blocked);

    let content = reg.get_skill_content("bug-fix").await.expect("content");
    assert_eq!(content.filename, "SKILL.md");
    assert!(content.content.contains("改 BUG"), "title line missing");

    assert!(reg.get_skill_meta("no-such-skill").await.is_err());
    assert!(reg.get_skill_content("../escape").await.is_err());
}

#[tokio::test]
async fn install_writes_whole_tree_including_subdirs() {
    // automated-testing 是多文件技能（examples/phases/scripts 子目录）——
    // 整树落位（不能只拷 SKILL.md）由本测试钉住。
    let reg = BuiltinSkillsRegistry::new();
    let target = temp_target("tree");
    reg.download_and_install(
        "automated-testing",
        "",
        target.path().to_string_lossy().as_ref(),
    )
    .await
    .expect("install");

    let embedded = BuiltinSkillsRegistry::skill_slugs();
    assert!(embedded.contains(&"automated-testing".to_string()));
    let mut nested = Vec::new();
    collect_relative_files(target.path(), target.path(), &mut nested);
    assert!(
        nested.len() > 1,
        "multi-file skill extracted only {} files",
        nested.len()
    );
    assert!(
        nested.iter().any(|p| p.starts_with("scripts")
            || p.starts_with("examples")
            || p.starts_with("phases")),
        "subdirectory files missing from extraction: {nested:?}"
    );
    // 无 `..`/绝对路径逃逸：所有落点都在 target 内（相对形态）。
    for p in &nested {
        assert!(Path::new(p).is_relative(), "absolute escape: {p:?}");
    }
}

#[tokio::test]
async fn install_rejects_bad_slugs() {
    let reg = BuiltinSkillsRegistry::new();
    let target = temp_target("bad");
    let dir = target.path().to_string_lossy().to_string();
    assert!(
        reg.download_and_install("../escape", "", &dir)
            .await
            .is_err()
    );
    assert!(reg.download_and_install("a/b", "", &dir).await.is_err());
    assert!(reg.download_and_install("", "", &dir).await.is_err());
    assert!(
        reg.download_and_install("no-such-skill", "", &dir)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn browse_paginates_with_cursor() {
    let reg = BuiltinSkillsRegistry::new();
    let page1 = reg
        .browse(&BrowseSort::Trending, 5, "")
        .await
        .expect("browse p1");
    assert_eq!(page1.items.len(), 5);
    let cursor = page1.next_cursor.clone().expect("cursor");
    let page2 = reg
        .browse(&BrowseSort::Trending, 5, &cursor)
        .await
        .expect("browse p2");
    // limit 是页大小上限：第二页仍 5 条（不是「剩余全部」），翻第三页收尾。
    assert_eq!(page2.items.len(), 5);
    let cursor2 = page2.next_cursor.clone().expect("cursor p2");
    let page3 = reg
        .browse(&BrowseSort::Trending, 5, &cursor2)
        .await
        .expect("browse p3");
    assert_eq!(page3.items.len(), EXPECTED_SKILL_COUNT - 10);
    assert!(page3.next_cursor.is_none());
    // 三页合起来恰好多不重叠地覆盖全部技能（分页完整性 + 无重叠单断言）。
    let mut all: Vec<String> = page1
        .items
        .iter()
        .chain(&page2.items)
        .chain(&page3.items)
        .map(|i| i.slug.clone())
        .collect();
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        EXPECTED_SKILL_COUNT,
        "pages overlap or miss items: {all:?}"
    );
}

/// 漏斗级端到端（真实内容 × 真实漏斗）：被拦技能走 `SkillInstaller::install`
/// 全漏斗——内容关硬 Err（拦截发生在 InstallPlan 审批卡**之前**）、staging
/// 清理、目标目录不存在。分档探针是 download+gate 两步拼装；本测试补上
/// 「装时拦」的用户可感行为（Err 文案 + 零残留）。
#[tokio::test]
async fn funnel_blocks_pinned_operational_skill_before_plan_card() {
    use crate::installer::SkillInstaller;
    use std::sync::Arc as StdArc;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(StdArc::new(BuiltinSkillsRegistry::new()));
    installer.set_registry_manager(manager);

    let err = installer
        .install("builtin", "wsl-operations", "")
        .await
        .expect_err("运维技能必须装时拦");
    let msg = format!("{err}");
    assert!(
        msg.contains("blocked by security check"),
        "拦截文案漂移: {msg}"
    );
    assert!(msg.contains("wsl-operations"), "报错缺 slug: {msg}");

    // 目标目录不存在 + staging 零残留（漏斗不变量：审批未过的内容不得残留）。
    assert!(!dir.path().join("skills/wsl-operations").exists());
    let staging_root = dir.path().join(".skill-staging");
    if staging_root.exists() {
        for e in std::fs::read_dir(&staging_root).expect("read staging") {
            let name = e.expect("entry").file_name().to_string_lossy().to_string();
            assert!(!name.contains("wsl-operations"), "staging 残留: {name}");
        }
    }
    // 检查结果已记录（WSAPI security 面可回查拦截理由）。
    let check = installer.last_security_check().expect("check recorded");
    assert!(check.blocked);
    assert!(!check.block_reason.is_empty());
}

/// 漏斗级端到端：过闸技能完整落位——文件在盘、origin=builtin、安全检查
/// 干净（闭环「装时拦」的另一侧：过闸技能不被误伤）。
#[tokio::test]
async fn funnel_installs_passing_skill_end_to_end() {
    use crate::installer::SkillInstaller;
    use std::sync::Arc as StdArc;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(StdArc::new(BuiltinSkillsRegistry::new()));
    installer.set_registry_manager(manager);

    let result = installer
        .install("builtin", "bug-fix", "")
        .await
        .expect("过闸技能应可装");
    assert!(!result.is_malware_blocked);

    let skill_dir = dir.path().join("skills/bug-fix");
    assert!(skill_dir.join("SKILL.md").exists());
    let content = std::fs::read_to_string(skill_dir.join("SKILL.md")).expect("read");
    assert!(content.contains("改 BUG"), "内容不完整: {}", &content[..80]);

    let origin = installer.get_origin_tracking("bug-fix").expect("origin");
    assert_eq!(origin.registry, "builtin");
    assert_eq!(origin.slug, "bug-fix");

    let check = installer.last_security_check().expect("check recorded");
    assert!(!check.blocked);
}

/// 统计 target 下相对文件路径（含嵌套子目录），验证落位形态。
fn collect_relative_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .collect();
    // 显式排序（read_dir 顺序是平台实现细节）。
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            collect_relative_files(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// 内嵌体积预算回归锁（BUG-20261116-030）：内嵌树必须零 ≥1MB 载荷——
/// build.rs staging 过滤被绕过（include_dir 直指仓库 `Skills/`）或阈值
/// 放宽时，47.5MB 的 ClamAV zip + window-mcp.exe 会重新塞进所有构建
/// （iotsmall ≈10MB 预算依赖此闸）。递归遍历——include_dir 的
/// `files()`/`dirs()` 都只返回直接子条目，`Skills/` 根层零文件、载荷
/// 全在嵌套 slug 目录下，只扫根层永远测不到（红证轮实录）。顺带锚 13
/// 个技能的 SKILL.md 全部在内嵌树（过滤只裁载荷不丢文档面）。
#[test]
fn embedded_tree_excludes_large_payloads_and_keeps_all_skill_docs() {
    const MAX_EMBED_FILE_BYTES: usize = 1024 * 1024;

    fn walk(dir: &include_dir::Dir, files: &mut Vec<std::path::PathBuf>) {
        for f in dir.files() {
            files.push(f.path().to_path_buf());
        }
        for sub in dir.dirs() {
            walk(sub, files);
        }
    }

    let mut files = Vec::new();
    walk(&super::BUILTIN_SKILLS_DIR, &mut files);
    assert!(!files.is_empty(), "内嵌树为空——build.rs staging 失效");
    for path in &files {
        let f = super::BUILTIN_SKILLS_DIR
            .get_file(path)
            .unwrap_or_else(|| panic!("embedded file vanished: {}", path.display()));
        assert!(
            f.contents().len() < MAX_EMBED_FILE_BYTES,
            "内嵌树混入 ≥1MB 载荷: {}（{} 字节）——build.rs 过滤被绕过或阈值放宽",
            path.display(),
            f.contents().len()
        );
    }
    for slug in BuiltinSkillsRegistry::skill_slugs() {
        assert!(
            super::BUILTIN_SKILLS_DIR
                .get_file(format!("{slug}/SKILL.md"))
                .is_some(),
            "技能 {slug} 的 SKILL.md 未入内嵌树"
        );
    }
}
