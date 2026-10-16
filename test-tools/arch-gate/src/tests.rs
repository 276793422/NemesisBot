//! arch-gate 自检（gate 本身能发现违规 + 扫描器语义）。
//! （自生产文件内联块迁出——2026-07-17 起测试代码放独立文件的纪律。）

use super::*;

fn workspace_root() -> PathBuf {
    // test-tools/arch-gate → 上两级是 workspace 根。
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

#[test]
fn architecture_dependency_matrix_holds() {
    let root = workspace_root();
    assert!(
        root.join("Cargo.toml").is_file(),
        "workspace 根定位失败: {}",
        root.display()
    );
    let violations = evaluate(&root);
    assert!(
        violations.is_empty(),
        "架构依赖矩阵违规 {} 项：\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[test]
fn internal_dep_scanner_sees_workspace_and_renamed_deps() {
    let root = workspace_root();
    // bus 只有 types；security 含 config/utils/path（白名单形态的现实样本）。
    let bus = internal_deps(&root.join("crates/nemesis-bus"));
    assert_eq!(bus, vec!["nemesis-types".to_string()]);
    let agent = internal_deps(&root.join("crates/nemesis-agent"));
    assert!(agent.contains(&"nemesis-types".to_string()));
    assert!(!agent.contains(&"nemesis-web".to_string()));
}

#[test]
fn rules_reference_existing_workspace_crates() {
    let root = workspace_root();
    let members = workspace_members(&root);
    for (name, _) in rules() {
        let found = members.iter().any(|m| {
            let dir = root.join(m);
            dir.join("Cargo.toml").is_file()
                && std::fs::read_to_string(dir.join("Cargo.toml"))
                    .map(|raw| raw.contains(&format!("name = \"{name}\"")))
                    .unwrap_or(false)
        });
        assert!(found, "规则引用的 crate '{name}' 不在 workspace 成员里");
    }
}

#[test]
fn rule_count_at_least_ten() {
    // §四.4（2026-10-16）：R1-R7 基础矩阵 + R8-R12 扩展（connectors/web/
    // board/forge/cluster 方向约束）——规则被悄悄删到 10 条以下 = 门禁
    // 缩水，测试红出提醒（新增规则请连本断言一起抬）。
    let n = rules().len();
    assert!(n >= 10, "架构规则数 {n} < 10：门禁规则被删除或未注册？");
}
