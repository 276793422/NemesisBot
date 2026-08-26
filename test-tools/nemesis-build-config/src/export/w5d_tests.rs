//! W5d batch tests: validate() decision matrix corners and export rendering
//! contracts (empty feature sets, frontend-env defaults).

use super::*;

fn man(text: &str) -> FeatureManifest {
    FeatureManifest::parse(text).unwrap()
}

#[test]
fn w5d_validate_flags_unknown_feature() {
    let m = man("[[feature]]\nid = \"known\"\ndefault = true\n");
    let mut cfg = BuildConfig::default();
    cfg.set_bool("ghost", true);
    let problems = validate(&cfg, &m);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("`ghost` is selected but not in manifest")),
        "problems: {problems:?}"
    );
}

#[test]
fn w5d_validate_clean_config_yields_no_problems() {
    let m = man(
        r#"
[[feature]]
id = "cluster"
default = false
depends = ["channels-rpc"]
conflicts = ["forge"]
[[feature]]
id = "channels-rpc"
default = true
[[feature]]
id = "forge"
default = false
[[feature]]
id = "build-profile"
type = "enum"
default = "release"
options = ["release", "iotsmall"]
"#,
    );
    let mut cfg = BuildConfig::default();
    cfg.set_bool("channels-rpc", true);
    cfg.set_bool("cluster", true); // dependency satisfied
    cfg.set_enum("build-profile", "iotsmall"); // valid choice
    assert!(validate(&cfg, &m).is_empty());
}

#[test]
fn w5d_validate_skips_dep_check_when_feature_off() {
    // a disabled feature with an unsatisfied dependency is not a problem
    let m = man(
        r#"
[[feature]]
id = "cluster"
default = false
depends = ["channels-rpc"]
[[feature]]
id = "channels-rpc"
default = true
"#,
    );
    let mut cfg = BuildConfig::default();
    cfg.set_bool("cluster", false);
    cfg.set_bool("channels-rpc", false);
    assert!(validate(&cfg, &m).is_empty());
}

#[test]
fn w5d_validate_conflict_not_flagged_when_other_off() {
    let m = man(
        r#"
[[feature]]
id = "a"
default = false
conflicts = ["b"]
[[feature]]
id = "b"
default = false
"#,
    );
    let mut cfg = BuildConfig::default();
    cfg.set_bool("a", true);
    cfg.set_bool("b", false);
    assert!(validate(&cfg, &m).is_empty());
}

#[test]
fn w5d_validate_enum_unchosen_is_ok() {
    let m = man(
        r#"
[[feature]]
id = "build-profile"
type = "enum"
default = "release"
options = ["release", "iotsmall"]
"#,
    );
    // config never sets the enum (e.g. built from a partial .config)
    assert!(validate(&BuildConfig::default(), &m).is_empty());
}

#[test]
fn w5d_frontend_env_unset_feature_defaults_false() {
    // documented contract: unset features render =false (matches
    // --no-default-features semantics; stale .env never re-enables a trim)
    let m = man(
        r#"
[[feature]]
id = "cluster"
default = true
"#,
    );
    let env = frontend_env(&BuildConfig::default(), &m);
    assert!(
        env.contains("VITE_FEATURE_CLUSTER=false"),
        "env was: {env}"
    );
}

#[test]
fn w5d_frontend_env_multi_dash_uppercases_all_segments() {
    let m = man("[[feature]]\nid = \"channels-webhook-x\"\ndefault = true\n");
    let mut cfg = BuildConfig::default();
    cfg.set_bool("channels-webhook-x", true);
    let env = frontend_env(&cfg, &m);
    assert!(
        env.contains("VITE_FEATURE_CHANNELS_WEBHOOK_X=true"),
        "env was: {env}"
    );
}

#[test]
fn w5d_frontend_env_one_line_per_feature_with_trailing_newline() {
    let m = man(
        r#"
[[feature]]
id = "b"
default = false
[[feature]]
id = "a"
default = false
"#,
    );
    let mut cfg = BuildConfig::default();
    cfg.set_bool("a", true);
    cfg.set_bool("b", true);
    let env = frontend_env(&cfg, &m);
    // manifest declaration order (not alphabetical) and a single trailing \n
    let lines: Vec<&str> = env.trim_end().split('\n').collect();
    assert_eq!(lines, vec!["VITE_FEATURE_B=true", "VITE_FEATURE_A=true"]);
    assert!(env.ends_with('\n'));
}

#[test]
fn w5d_render_cmd_without_features_omits_features_flag() {
    let mut cfg = BuildConfig::default();
    cfg.set_bool("migrate", false);
    let cmd = render_cargo_cmd(&cfg);
    assert!(!cmd.contains("--features"), "cmd was: {cmd}");
    assert!(cmd.contains("--no-default-features"));
    assert!(cmd.contains("--profile release"));
}

#[test]
fn w5d_features_arg_empty_when_nothing_enabled() {
    let mut cfg = BuildConfig::default();
    cfg.set_bool("off-feature", false);
    assert_eq!(features_arg(&cfg), "");
    assert!(enabled_features(&cfg).is_empty());
}

#[test]
fn w5d_enabled_features_deterministic_sorted_order() {
    // BTreeMap => stable, sorted order; the bridge scripts rely on this
    let mut cfg = BuildConfig::default();
    cfg.set_bool("zeta", true);
    cfg.set_bool("alpha", true);
    cfg.set_bool("mid", true);
    assert_eq!(enabled_features(&cfg), vec!["alpha", "mid", "zeta"]);
    assert_eq!(features_arg(&cfg), "alpha,mid,zeta");
}

#[test]
fn w5d_profile_arg_reads_last_written_enum() {
    let mut cfg = BuildConfig::default();
    cfg.set_enum("build-profile", "release");
    cfg.set_enum("build-profile", "iotsmall");
    assert_eq!(profile_arg(&cfg), "iotsmall");
}

#[test]
fn w5d_frontend_env_enum_only_manifest_renders_bare_newline() {
    // degenerate but reachable: a manifest with only enum features emits no
    // VITE_ lines (frontend keeps its default-include semantics)
    let m = man(
        r#"
[[feature]]
id = "build-profile"
type = "enum"
default = "release"
options = ["release"]
"#,
    );
    let env = frontend_env(&BuildConfig::default(), &m);
    assert_eq!(env, "\n");
}
