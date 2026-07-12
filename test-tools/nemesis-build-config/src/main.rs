//! nemesis-config — CLI entry point.
//!
//! Run with no subcommand to launch the TUI:
//!     nemesis-config
//!
//! Subcommands:
//!     init                 scaffold/refresh features.toml from nemesisbot/Cargo.toml
//!     check                verify manifest <-> Cargo.toml in sync
//!     list                 print features + current selection
//!     export --features    print comma-separated enabled features
//!     export --profile     print build profile
//!     export --cmd         print full cargo command
//!     has-config           exit 0 if a .config exists, else 1
//!     load <preset>        copy scripts/customize/profiles/<preset>.config to .config

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use nemesis_build_config::{
    cargo_scan, config::BuildConfig, export, manifest::{DefaultVal, FeatureManifest, FeatureSpec},
};

#[derive(Parser)]
#[command(
    name = "nemesis-build-config",
    about = "menuconfig-style build configurator for NemesisBot feature trimming"
)]
struct Cli {
    /// Project root (default: current directory).
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold (or refresh) scripts/customize/features.toml from nemesisbot/Cargo.toml.
    Init,
    /// Verify the manifest is in sync with nemesisbot/Cargo.toml.
    Check,
    /// Print features + current selection.
    List,
    /// Translate the saved .config into cargo build arguments.
    Export {
        #[arg(long)]
        features: bool,
        #[arg(long)]
        profile: bool,
        #[arg(long)]
        cmd: bool,
        /// Render `web/.env` (VITE_FEATURE_<ID>=<bool>) for frontend gating.
        #[arg(long)]
        frontend_env: bool,
    },
    /// Exit 0 if a .config exists, else 1.
    HasConfig,
    /// Load a preset from scripts/customize/profiles/<name>.config.
    Load { name: String },
}

// ----- path helpers (all relative to project root) -----
fn manifest_path(root: &Path) -> PathBuf {
    root.join("scripts/customize/features.toml")
}
fn config_path(root: &Path) -> PathBuf {
    root.join("scripts/customize/.config")
}
fn profiles_dir(root: &Path) -> PathBuf {
    root.join("scripts/customize/profiles")
}
fn nemesisbot_cargo(root: &Path) -> PathBuf {
    root.join("nemesisbot/Cargo.toml")
}

fn category_for(id: &str) -> &'static str {
    if id.starts_with("channels-") {
        "channels"
    } else if id == "build-profile" {
        "build"
    } else {
        "subsystems"
    }
}

/// Build a manifest scaffold from a Cargo.toml scan, merging any human-curated
/// metadata (label/desc/category/options) from an existing manifest.
fn scaffold(scan: &cargo_scan::ScanResult, existing: Option<&FeatureManifest>) -> FeatureManifest {
    let mut features: Vec<FeatureSpec> = Vec::new();
    for name in scan.names() {
        let prev = existing.and_then(|m| m.features.iter().find(|f| f.id == name)).cloned();
        let spec = if let Some(p) = prev {
            // keep curated fields; refresh default from reality
            FeatureSpec {
                default: DefaultVal::Bool(scan.is_default(&name)),
                ..p
            }
        } else {
            FeatureSpec {
                id: name.clone(),
                label: name.clone(),
                desc: String::new(),
                category: category_for(&name).to_string(),
                feature_type: None,
                default: DefaultVal::Bool(scan.is_default(&name)),
                options: Vec::new(),
                depends: Vec::new(),
                conflicts: Vec::new(),
            }
        };
        features.push(spec);
    }
    // always (re)ensure the build-profile enum entry
    if !features.iter().any(|f| f.id == "build-profile") {
        let prev = existing.and_then(|m| m.features.iter().find(|f| f.id == "build-profile")).cloned();
        features.push(prev.unwrap_or(FeatureSpec {
            id: "build-profile".to_string(),
            label: "构建 profile".to_string(),
            desc: "release（默认，带 unwind）/ iotsmall（最小体积，panic=abort）".to_string(),
            category: "build".to_string(),
            feature_type: Some("enum".to_string()),
            default: DefaultVal::Str("release".to_string()),
            options: vec!["release".to_string(), "iotsmall".to_string()],
            depends: Vec::new(),
            conflicts: Vec::new(),
        }));
    }
    FeatureManifest { features }
}

fn run_init(root: &Path) -> Result<()> {
    let scan = cargo_scan::scan_file(&nemesisbot_cargo(root))
        .with_context(|| "scanning nemesisbot/Cargo.toml")?;
    let mpath = manifest_path(root);
    let existing = if mpath.exists() {
        FeatureManifest::load(&mpath).ok()
    } else {
        None
    };
    let fresh = scaffold(&scan, existing.as_ref());
    if let Some(parent) = mpath.parent() {
        std::fs::create_dir_all(parent)?;
    }
    fresh.save(&mpath)?;
    println!("✓ wrote {} ({} features)", mpath.display(), fresh.features.len());
    println!("  hand-edit descriptions/categories there, then run `nemesis-build-config` to configure.");
    Ok(())
}

fn run_check(root: &Path) -> Result<()> {
    let scan = cargo_scan::scan_file(&nemesisbot_cargo(root))?;
    let manifest = FeatureManifest::load(&manifest_path(root))?;
    let cargo_names: std::collections::BTreeSet<String> = scan.names().into_iter().collect();
    let manifest_bool_ids: std::collections::BTreeSet<String> = manifest
        .features
        .iter()
        .filter(|f| !f.is_enum() && f.id != "build-profile")
        .map(|f| f.id.clone())
        .collect();

    let mut problems = 0;
    for id in manifest_bool_ids.iter() {
        if !cargo_names.contains(id) {
            println!("STALE: manifest has `{id}` but nemesisbot/Cargo.toml does not");
            problems += 1;
        }
    }
    for id in cargo_names.iter() {
        if !manifest_bool_ids.contains(id) {
            println!("MISSING: nemesisbot/Cargo.toml has `{id}` but manifest does not (run `init`)");
            problems += 1;
        }
    }
    if problems == 0 {
        println!("✓ manifest in sync with nemesisbot/Cargo.toml ({} features)", cargo_names.len());
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn load_config_or_default(root: &Path, manifest: &FeatureManifest) -> Result<BuildConfig> {
    let cpath = config_path(root);
    if cpath.exists() {
        BuildConfig::load(&cpath)
    } else {
        Ok(BuildConfig::from_defaults(manifest))
    }
}

fn run_list(root: &Path) -> Result<()> {
    let manifest = FeatureManifest::load(&manifest_path(root))?;
    let cfg = load_config_or_default(root, &manifest)?;
    let problems = export::validate(&cfg, &manifest);
    for f in &manifest.features {
        let val = if f.is_enum() {
            cfg.get_enum(&f.id).unwrap_or("?").to_string()
        } else {
            match cfg.get_bool(&f.id) {
                Some(true) => "on".to_string(),
                _ => "off".to_string(),
            }
        };
        println!("{:<20} {:<6} {}", f.id, val, f.label);
    }
    if !problems.is_empty() {
        println!("\n⚠ config validation problems:");
        for p in &problems {
            println!("  - {p}");
        }
    }
    Ok(())
}

fn run_export(root: &Path, feats: bool, profile: bool, cmd: bool, frontend_env: bool) -> Result<()> {
    let cpath = config_path(root);
    if !cpath.exists() {
        // No customization => signal to the bridge script to do a default build.
        anyhow::bail!("no .config at {} (run `nemesis-config` to create one)", cpath.display());
    }
    let cfg = BuildConfig::load(&cpath)?;
    if frontend_env {
        let manifest = FeatureManifest::load(&manifest_path(root))?;
        print!("{}", export::frontend_env(&cfg, &manifest));
        return Ok(());
    }
    if feats {
        println!("{}", export::features_arg(&cfg));
    } else if profile {
        println!("{}", export::profile_arg(&cfg));
    } else if cmd {
        println!("{}", export::render_cargo_cmd(&cfg));
    } else {
        // default: print everything for inspection
        println!("features: {}", export::features_arg(&cfg));
        println!("profile:  {}", export::profile_arg(&cfg));
        println!("cmd:      {}", export::render_cargo_cmd(&cfg));
    }
    Ok(())
}

fn run_has_config(root: &Path) -> Result<()> {
    if config_path(root).exists() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn run_load(root: &Path, name: &str) -> Result<()> {
    let src = profiles_dir(root).join(format!("{name}.config"));
    if !src.exists() {
        anyhow::bail!("preset not found: {}", src.display());
    }
    let dst = config_path(root);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&src, &dst)?;
    println!("✓ loaded preset `{name}` → {}", dst.display());
    Ok(())
}

fn run_tui(root: &Path) -> Result<()> {
    let mpath = manifest_path(root);
    if !mpath.exists() {
        println!("manifest not found at {} — running `init` first...", mpath.display());
        run_init(root)?;
    }
    let manifest = FeatureManifest::load(&mpath)?;
    let mut cfg = load_config_or_default(root, &manifest)?;
    nemesis_build_config::tui::run(&manifest, &mut cfg, &config_path(root))?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.canonicalize().unwrap_or_else(|_| cli.root.clone());
    match cli.cmd {
        None => run_tui(&root),
        Some(Cmd::Init) => run_init(&root),
        Some(Cmd::Check) => run_check(&root),
        Some(Cmd::List) => run_list(&root),
        Some(Cmd::Export { features, profile, cmd, frontend_env }) => run_export(&root, features, profile, cmd, frontend_env),
        Some(Cmd::HasConfig) => run_has_config(&root),
        Some(Cmd::Load { name }) => run_load(&root, &name),
    }
}
