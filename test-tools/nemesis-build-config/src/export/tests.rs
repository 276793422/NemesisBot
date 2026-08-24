use super::*;

fn cfg_from(text: &str) -> BuildConfig {
    BuildConfig::parse(text).unwrap()
}

#[test]
fn features_arg_lists_enabled_only() {
    let cfg = cfg_from(
        r#"
[features]
channels-web = true
channels-rpc = true
migrate = false
"#,
    );
    assert_eq!(features_arg(&cfg), "channels-rpc,channels-web");
    assert!(enabled_features(&cfg).contains(&"channels-web".to_string()));
    assert!(!enabled_features(&cfg).contains(&"migrate".to_string()));
}

#[test]
fn profile_defaults_to_release() {
    let cfg = BuildConfig::default();
    assert_eq!(profile_arg(&cfg), "release");
}

#[test]
fn profile_reads_enum() {
    let cfg = cfg_from("[enums]\nbuild-profile = \"iotsmall\"\n");
    assert_eq!(profile_arg(&cfg), "iotsmall");
}

#[test]
fn render_cmd_includes_features_and_profile() {
    let cfg = cfg_from(
        r#"
[features]
channels-web = true
[enums]
build-profile = "iotsmall"
"#,
    );
    let cmd = render_cargo_cmd(&cfg);
    assert!(cmd.contains("--profile iotsmall"));
    assert!(cmd.contains("--no-default-features"));
    assert!(cmd.contains("--features \"channels-web\""));
}

#[test]
fn frontend_env_emits_all_non_enum_features() {
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "cluster"
default = false
[[feature]]
id = "channels-web"
default = true
[[feature]]
id = "build-profile"
type = "enum"
default = "release"
options = ["release"]
"#,
    )
    .unwrap();
    let mut cfg = BuildConfig::default();
    cfg.set_bool("cluster", false);
    cfg.set_bool("channels-web", true);
    let env = frontend_env(&cfg, &manifest);
    assert!(env.contains("VITE_FEATURE_CLUSTER=false"));
    assert!(env.contains("VITE_FEATURE_CHANNELS_WEB=true"));
    // enum feature excluded
    assert!(!env.contains("VITE_FEATURE_BUILD_PROFILE"));
}

#[test]
fn validate_catches_dependency_violation() {
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "cluster"
default = false
depends = ["channels-rpc"]
[[feature]]
id = "channels-rpc"
default = true
"#,
    )
    .unwrap();
    let mut cfg = BuildConfig::default();
    cfg.set_bool("cluster", true);
    cfg.set_bool("channels-rpc", false); // dependency unsatisfied
    let problems = validate(&cfg, &manifest);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("requires `channels-rpc`"))
    );
}

#[test]
fn validate_catches_conflict() {
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "a"
default = false
conflicts = ["b"]
[[feature]]
id = "b"
default = false
"#,
    )
    .unwrap();
    let mut cfg = BuildConfig::default();
    cfg.set_bool("a", true);
    cfg.set_bool("b", true);
    let problems = validate(&cfg, &manifest);
    assert!(problems.iter().any(|p| p.contains("conflicts")));
}

#[test]
fn validate_catches_bad_enum() {
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "build-profile"
type = "enum"
default = "release"
options = ["release", "iotsmall"]
"#,
    )
    .unwrap();
    let mut cfg = BuildConfig::default();
    cfg.set_enum("build-profile", "bogus");
    let problems = validate(&cfg, &manifest);
    assert!(problems.iter().any(|p| p.contains("not in")));
}
