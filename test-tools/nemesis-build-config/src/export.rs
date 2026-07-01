//! Turn a `.config` into cargo build arguments. This is the bridge between
//! the menuconfig selection and the actual `cargo build` invocation (cargo
//! cannot read features from a file, so we translate).

use crate::config::BuildConfig;
use crate::manifest::FeatureManifest;

/// All boolean features currently enabled in `cfg`.
pub fn enabled_features(cfg: &BuildConfig) -> Vec<String> {
    cfg.features
        .iter()
        .filter(|(_, v)| **v)
        .map(|(k, _)| k.clone())
        .collect()
}

/// Comma-separated `--features` argument value, e.g. `"channels-web,channels-rpc"`.
pub fn features_arg(cfg: &BuildConfig) -> String {
    enabled_features(cfg).join(",")
}

/// The selected build profile (defaults to "release" if unset).
pub fn profile_arg(cfg: &BuildConfig) -> String {
    cfg.get_enum("build-profile").unwrap_or("release").to_string()
}

/// Validate a config against a manifest: returns problems like features that
/// are enabled but not declared, enum selections outside allowed options, or
/// dependency/conflict violations. Empty vec = OK.
pub fn validate(cfg: &BuildConfig, manifest: &FeatureManifest) -> Vec<String> {
    let mut problems = Vec::new();
    let known: std::collections::HashSet<&str> =
        manifest.features.iter().map(|f| f.id.as_str()).collect();

    for (id, on) in &cfg.features {
        if !known.contains(id.as_str()) {
            problems.push(format!("feature `{id}` is selected but not in manifest"));
        }
        if !on {
            continue;
        }
        // dependency check
        if let Some(spec) = manifest.features.iter().find(|f| &f.id == id) {
            for dep in &spec.depends {
                if cfg.get_bool(dep) != Some(true) {
                    problems.push(format!("feature `{id}` requires `{dep}` (currently off)"));
                }
            }
            for conf in &spec.conflicts {
                if cfg.get_bool(conf) == Some(true) {
                    problems.push(format!("feature `{id}` conflicts with `{conf}`"));
                }
            }
        }
    }
    // enum range check
    for f in &manifest.features {
        if f.is_enum() {
            if let Some(chosen) = cfg.get_enum(&f.id) {
                if !f.options.iter().any(|o| o == chosen) {
                    problems.push(format!(
                        "feature `{}` set to `{}` which is not in {:?}",
                        f.id, chosen, f.options
                    ));
                }
            }
        }
    }
    problems
}

/// Render the full cargo invocation line for display/debug.
pub fn render_cargo_cmd(cfg: &BuildConfig) -> String {
    let feats = features_arg(cfg);
    let profile = profile_arg(cfg);
    let mut s = format!("cargo build --profile {profile} -p nemesisbot --no-default-features");
    if !feats.is_empty() {
        s.push_str(&format!(" --features \"{feats}\""));
    }
    s
}

#[cfg(test)]
mod tests {
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
        assert!(problems.iter().any(|p| p.contains("requires `channels-rpc`")));
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
}
