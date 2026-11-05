// default 集合对账回归锁（BUG-20261102-307）。
//
// 契约：`check` 的漂移检测必须覆盖 **default 集合**，而不只是 id 集合。
// features.toml 头注语义「default = true ⇒ on by default = current full
// build」——清单 default 是 TUI 从零保存 .config 的勾选初值
// （`BuildConfig::from_defaults`），若与 nemesisbot/Cargo.toml 的
// `default = [...]` 数组分叉，「TUI 默认全量」与 `cargo build` 默认全量
// 两个入口产出不同功能面。
//
// 案例：vm-display 首提（ee7604d9，2026-10-21）清单 default = true，
// Cargo default 数组从未收录——check 只对账 id 集合（STALE/MISSING）故
// 恒绿，TUI 从零保存的 .config 比 cargo 默认构建多出 VNC 显示通道；且
// `init` 的 scaffold 按 Cargo 实况刷新 default，跑一次 init 会把
// vm-display 静默翻回 false，证明清单值与机制设计相悖。
//
// 两个层面：
// 1. fixture 级：check_problems 必须对 default 集合分叉报 DEFAULT-DRIFT；
// 2. 真实仓级：scripts/customize/features.toml 与 nemesisbot/Cargo.toml
//    的 default 集合恒等（防再度分叉）。

use super::*;

fn fixture_scan(cargo_toml: &str) -> cargo_scan::ScanResult {
    cargo_scan::scan_text(cargo_toml).unwrap()
}

fn fixture_manifest(features_toml: &str) -> FeatureManifest {
    FeatureManifest::parse(features_toml).unwrap()
}

const CARGO_MATCHING: &str = r#"
[features]
default = ["alpha"]
alpha = []
beta = []
"#;

// 与 CARGO_MATCHING 同 id 集合，但 alpha 的清单 default=false——default
// 集合反方向分叉（Cargo default 数组有、清单没勾）。
const MANIFEST_MISSING_DEFAULT: &str = r#"
[[feature]]
id = "alpha"
default = false

[[feature]]
id = "beta"
default = false
"#;

const MANIFEST_DRIFT: &str = r#"
[[feature]]
id = "alpha"
default = true

[[feature]]
id = "beta"
default = true
"#;

#[test]
fn check_flags_manifest_default_not_in_cargo_default() {
    let scan = fixture_scan(CARGO_MATCHING);
    let manifest = fixture_manifest(MANIFEST_DRIFT);
    let problems = check_problems(&scan, &manifest);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("beta") && p.contains("DEFAULT")),
        "清单 default=true 而 Cargo default 数组没有的 feature 必须被报出（BUG-20261102-307）: {problems:?}"
    );
}

#[test]
fn check_flags_cargo_default_not_in_manifest_defaults() {
    let scan = fixture_scan(CARGO_MATCHING);
    let manifest = fixture_manifest(MANIFEST_MISSING_DEFAULT);
    let problems = check_problems(&scan, &manifest);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("alpha") && p.contains("DEFAULT")),
        "Cargo default 数组有而清单 default=false 的 feature 必须被报出（BUG-20261102-307 反方向）: {problems:?}"
    );
}

#[test]
fn check_silent_when_defaults_match() {
    let scan = fixture_scan(CARGO_MATCHING);
    let manifest = fixture_manifest(
        r#"
[[feature]]
id = "alpha"
default = true

[[feature]]
id = "beta"
default = false
"#,
    );
    let problems = check_problems(&scan, &manifest);
    assert!(
        problems.is_empty(),
        "id 与 default 集合均对齐时不得有漂移报告: {problems:?}"
    );
}

#[test]
fn enum_default_string_is_not_a_bool_default() {
    // enum feature（如 build-profile）的 default 是字符串，不得混入 bool
    // default 集合对账。
    let scan = fixture_scan(CARGO_MATCHING);
    let manifest = fixture_manifest(
        r#"
[[feature]]
id = "alpha"
default = true

[[feature]]
id = "beta"
default = false

[[feature]]
id = "build-profile"
type = "enum"
options = ["release", "iotsmall"]
default = "release"
"#,
    );
    let problems = check_problems(&scan, &manifest);
    assert!(
        problems.is_empty(),
        "enum default 字符串不得触发 DEFAULT 漂移: {problems:?}"
    );
}

// ---------------------------------------------------------------------------
// 真实仓文件对账（防再度分叉的回归锁本体）
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn real_repo_manifest_defaults_match_cargo_default_array() {
    let root = repo_root();
    let scan = cargo_scan::scan_file(&root.join("nemesisbot").join("Cargo.toml"))
        .expect("read real nemesisbot/Cargo.toml");
    let manifest =
        FeatureManifest::load(&root.join("scripts").join("customize").join("features.toml"))
            .expect("read real features.toml");
    let problems = check_problems(&scan, &manifest);
    assert!(
        problems.is_empty(),
        "真实仓 default 集合分叉（BUG-20261102-307）: {problems:?}"
    );
}

/// 行为锁：TUI 无 .config 打开时的初始勾选（`BuildConfig::from_defaults`，
/// 即「从零保存」的 .config）经 export 展开的 feature 集，必须与
/// `cargo build` 默认面等价——两个「默认全量」入口产出同一功能面。
#[test]
fn real_repo_tui_from_defaults_enables_cargo_default_surface() {
    let root = repo_root();
    let scan = cargo_scan::scan_file(&root.join("nemesisbot").join("Cargo.toml"))
        .expect("read real nemesisbot/Cargo.toml");
    let manifest =
        FeatureManifest::load(&root.join("scripts").join("customize").join("features.toml"))
            .expect("read real features.toml");

    let cfg = BuildConfig::from_defaults(&manifest);
    let mut tui_surface: Vec<String> = export::enabled_features(&cfg);
    tui_surface.retain(|f| f != "default");
    let tui_set: std::collections::BTreeSet<String> = tui_surface.into_iter().collect();
    let cargo_set: std::collections::BTreeSet<String> =
        scan.default_enabled().into_iter().collect();
    assert_eq!(
        tui_set,
        cargo_set,
        "TUI 从零保存面必须与 cargo default 构建面等价（BUG-20261102-307）\
         \n  TUI-only: {:?}\
         \n  cargo-only: {:?}",
        tui_set.difference(&cargo_set).collect::<Vec<_>>(),
        cargo_set.difference(&tui_set).collect::<Vec<_>>(),
    );
}
