//! WIT 合同漂移测试。
//!
//! 真相源 = 宿主 crate 内 `wit/plugin.wit`（合同由宿主定义；SDK 绑定直接
//! 引用该目录，无本地副本）。本测试钉两件事：
//!
//! ① 全部已入库示例的随包副本与真相源字节相等（2026-09-30 插件体系复查
//!    #13：示例副本此前无漂移保护——合同升级忘同步示例 = 三方按旧合同
//!    开发，装新宿主即实例化失败）。名单静态维护，只收 git 已入库示例
//!    （与 devkit 打包名单同口径）。
//! ② wit 文件里的 package 版本 ↔ 宿主 [`nemesis_plugins_wasm::CONTRACT_API_VERSION`]
//!    机械对齐（合同代际 = package semver 的 minor：0.N.0 ↔ N）。bump 合同
//!    忘改常量（或反之）在此处红，不再靠人工对齐。
//!
//! ```text
//! cargo test -p nemesis-plugins-wasm --test wit_drift
//! ```

use std::path::{Path, PathBuf};

/// 已入库示例的 wit 副本（相对仓库根）。
const EXAMPLE_WIT_COPIES: &[&str] = &[
    "plugins/wasm/translate/wit/plugin.wit",
    "plugins/wasm/activity-log/wit/plugin.wit",
    "plugins/wasm/textstat/wit/plugin.wit",
    "plugins/wasm/wsinsight/wit/plugin.wit",
];

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn example_wit_copies_are_in_sync() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/plugin.wit");
    let root_s = read(&root);
    for rel in EXAMPLE_WIT_COPIES {
        let copy = repo_root().join(rel);
        assert_eq!(
            read(&copy),
            root_s,
            "示例 wit 副本漂移：请同步 {} → {}",
            root.display(),
            copy.display()
        );
    }
}

#[test]
fn contract_api_version_matches_wit_package_version() {
    let wit = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/plugin.wit"));
    let line = wit
        .lines()
        .find(|l| l.trim_start().starts_with("package "))
        .expect("package declaration present");
    // 形态：package nemesis:plugin@0.1.0;（纯字符串解析，不引 semver 依赖）
    let ver = line
        .split('@')
        .nth(1)
        .and_then(|v| {
            v.trim_end_matches(';')
                .trim()
                .split('.')
                .nth(1)
                .map(String::from)
        })
        .unwrap_or_else(|| panic!("cannot parse package minor version from: {line}"));
    assert_eq!(
        nemesis_plugins_wasm::CONTRACT_API_VERSION.to_string(),
        ver,
        "合同代际失配：wit package 版本与 CONTRACT_API_VERSION 必须同步 bump"
    );
}
