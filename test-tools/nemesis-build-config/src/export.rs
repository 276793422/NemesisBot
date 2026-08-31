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
    cfg.get_enum("build-profile")
        .unwrap_or("release")
        .to_string()
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
        if f.is_enum()
            && let Some(chosen) = cfg.get_enum(&f.id)
                && !f.options.iter().any(|o| o == chosen) {
                    problems.push(format!(
                        "feature `{}` set to `{}` which is not in {:?}",
                        f.id, chosen, f.options
                    ));
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

/// Render `web/.env` content for the Vite frontend feature-gating.
///
/// Emits one `VITE_FEATURE_<ID>=<bool>` line per non-enum feature, where
/// `<ID>` is the feature id uppercased with `-`→`_` (`channels-web` →
/// `VITE_FEATURE_CHANNELS_WEB`). The frontend gates views on
/// `import.meta.env.VITE_FEATURE_X !== 'false'` (default-include), so only
/// `=false` lines actually hide views — both are emitted for clarity + so a
/// stale `.env` never silently re-enables a trimmed feature. Unset features
/// default to `false` (matches `--no-default-features` semantics).
pub fn frontend_env(cfg: &BuildConfig, manifest: &FeatureManifest) -> String {
    let mut lines = Vec::new();
    for f in &manifest.features {
        if f.is_enum() {
            continue;
        }
        let name = format!("VITE_FEATURE_{}", f.id.replace('-', "_").to_uppercase());
        let on = cfg.get_bool(&f.id).unwrap_or(false);
        lines.push(format!("{name}={on}"));
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod w5d_tests;
