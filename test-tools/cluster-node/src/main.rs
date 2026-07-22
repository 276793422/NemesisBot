use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rand::seq::SliceRandom;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "cluster-node", about = "Cluster test node setup & run tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a cluster node environment at the target directory.
    Path {
        /// Target directory (relative or absolute).
        dir: String,
        /// Node name (default: auto-generated from directory name).
        #[arg(long)]
        name: Option<String>,
        /// Node role (default: random).
        #[arg(long)]
        role: Option<String>,
        /// Node category (default: random).
        #[arg(long)]
        category: Option<String>,
        /// Cluster discovery port (default: 11949).
        #[arg(long, default_value = "11949")]
        port: u16,
        /// Cluster RPC port (default: 21949).
        #[arg(long, default_value = "21949")]
        rpc_port: u16,
    },
    /// Run a cluster node from the target directory.
    Run {
        /// Target directory containing the node environment.
        dir: String,
    },
}

// ---------------------------------------------------------------------------
// Constants for random generation
// ---------------------------------------------------------------------------

const ROLES: &[&str] = &["manager", "coordinator", "worker", "observer", "standby"];
const CATEGORIES: &[&str] = &[
    "design",
    "development",
    "testing",
    "ops",
    "deployment",
    "analysis",
    "general",
];
const TAG_POOL: &[&str] = &[
    "ai", "python", "rust", "web", "database", "system", "monitor", "api", "ml", "devops",
];
const CAP_POOL: &[&str] = &[
    "llm", "tools", "code", "file", "web", "system", "memory", "cluster", "mcp", "cron", "forge",
    "skills",
];

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Path {
            dir,
            name,
            role,
            category,
            port,
            rpc_port,
        } => cmd_path(
            &dir,
            name.as_deref(),
            role.as_deref(),
            category.as_deref(),
            port,
            rpc_port,
        ),
        Cmd::Run { dir } => cmd_run(&dir),
    }
}

// ---------------------------------------------------------------------------
// `path` command
// ---------------------------------------------------------------------------

fn cmd_path(
    dir: &str,
    name: Option<&str>,
    role: Option<&str>,
    category: Option<&str>,
    port: u16,
    rpc_port: u16,
) -> Result<()> {
    let target = absolutize(dir)?;
    let dir_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "node".to_string());

    // 1. Create directory
    fs::create_dir_all(&target)
        .with_context(|| format!("Failed to create directory: {}", target.display()))?;
    println!("  OK Directory created: {}", target.display());

    // 2. Find nemesisbot binary
    let nemesisbot_src = find_nemesisbot()?;
    let nemesisbot_dst = target.join(nemesisbot_exe_name());
    if nemesisbot_src != nemesisbot_dst {
        fs::copy(&nemesisbot_src, &nemesisbot_dst).with_context(|| {
            format!(
                "Failed to copy {} -> {}",
                nemesisbot_src.display(),
                nemesisbot_dst.display()
            )
        })?;
    }
    println!("  OK NemesisBot binary copied");

    // 3. Run onboard default --local
    let exe = target.join(nemesisbot_exe_name());
    let status = Command::new(&exe)
        .args(["onboard", "default", "--local"])
        .current_dir(&target)
        .env("NEMESISBOT_NO_COLOR", "1")
        .status()
        .context("Failed to run nemesisbot onboard default")?;
    if !status.success() {
        bail!("nemesisbot onboard default failed with status: {}", status);
    }
    println!("  OK onboard default completed");

    // 4. Patch config.json — enable cluster, set ports
    let config_path = target.join(".nemesisbot").join("config.json");
    if config_path.exists() {
        let raw = fs::read_to_string(&config_path)?;
        let mut config: serde_json::Value = serde_json::from_str(&raw)?;
        if let Some(cluster) = config.get_mut("cluster") {
            cluster["enabled"] = serde_json::json!(true);
            cluster["port"] = serde_json::json!(port);
            cluster["rpc_port"] = serde_json::json!(rpc_port);
        }
        fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
        println!(
            "  OK config.json cluster enabled (port={}, rpc_port={})",
            port, rpc_port
        );
    }

    // 5. Generate random cluster parameters
    let mut rng = rand::thread_rng();
    let role = role
        .map(String::from)
        .unwrap_or_else(|| ROLES.choose(&mut rng).unwrap().to_string());
    let category = category
        .map(String::from)
        .unwrap_or_else(|| CATEGORIES.choose(&mut rng).unwrap().to_string());
    let name = name
        .map(String::from)
        .unwrap_or_else(|| format!("Node-{}", dir_name));

    let tags = pick_random(TAG_POOL, 1..=3, &mut rng);
    let caps = pick_random(CAP_POOL, 2..=4, &mut rng);

    // 6. Run cluster init
    let args = vec![
        "cluster".to_string(),
        "init".to_string(),
        "--name".to_string(),
        name.clone(),
        "--role".to_string(),
        role.clone(),
        "--category".to_string(),
        category.clone(),
        "--tags".to_string(),
        tags.join(","),
        "--capabilities".to_string(),
        caps.join(","),
        "--local".to_string(),
    ];
    let status = Command::new(&exe)
        .args(&args)
        .current_dir(&target)
        .env("NEMESISBOT_NO_COLOR", "1")
        .status()
        .context("Failed to run nemesisbot cluster init")?;
    if !status.success() {
        bail!("nemesisbot cluster init failed with status: {}", status);
    }
    println!("  OK cluster init completed");

    // 7. Summary
    println!();
    println!("  Node:     {}", name);
    println!("  Role:     {}", role);
    println!("  Category: {}", category);
    println!("  Tags:     {}", tags.join(", "));
    println!("  Caps:     {}", caps.join(", "));
    println!("  Port:     {} / RPC: {}", port, rpc_port);
    println!("  Path:     {}", target.display());
    println!();
    println!("  Run with: cluster-node run {}", dir);

    Ok(())
}

// ---------------------------------------------------------------------------
// `run` command
// ---------------------------------------------------------------------------

fn cmd_run(dir: &str) -> Result<()> {
    let target = absolutize(dir)?;
    let exe = target.join(nemesisbot_exe_name());

    if !exe.exists() {
        bail!("nemesisbot not found at {}", exe.display());
    }

    println!("  Starting node at {}", target.display());
    let mut child = Command::new(&exe)
        .args(["gateway", "--local"])
        .current_dir(&target)
        .env("NEMESISBOT_NO_COLOR", "1")
        .spawn()
        .context("Failed to start nemesisbot gateway")?;

    let status = child.wait().context("Failed to wait for nemesisbot")?;
    println!("  Node exited with status: {}", status);

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn absolutize(dir: &str) -> Result<PathBuf> {
    let p = PathBuf::from(dir);
    Ok(if p.is_absolute() {
        p
    } else {
        std::env::current_dir()?.join(p)
    })
}

fn nemesisbot_exe_name() -> &'static str {
    if cfg!(windows) {
        "nemesisbot.exe"
    } else {
        "nemesisbot"
    }
}

/// Find nemesisbot binary in order of priority.
fn find_nemesisbot() -> Result<PathBuf> {
    let exe_name = nemesisbot_exe_name();

    // 1. Same directory as this tool
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // 2. target/release/
    let candidate = PathBuf::from("target/release").join(exe_name);
    if candidate.exists() {
        return Ok(candidate);
    }

    // 3. target/debug/
    let candidate = PathBuf::from("target/debug").join(exe_name);
    if candidate.exists() {
        return Ok(candidate);
    }

    // 4. Try workspace-relative paths
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Possibly in bin/bin_windows/ or similar
            for rel in &[
                "../../target/release",
                "../../target/debug",
                "../target/release",
                "../target/debug",
            ] {
                let candidate = dir.join(rel).join(exe_name);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    bail!("nemesisbot binary not found. Build it first: cargo build --release -p nemesisbot");
}

/// Pick `n` random items from `pool` where n is in `range`.
fn pick_random(
    pool: &[&str],
    range: std::ops::RangeInclusive<usize>,
    rng: &mut impl rand::Rng,
) -> Vec<String> {
    let count = rng.gen_range(range);
    let mut shuffled: Vec<&str> = pool.to_vec();
    shuffled.shuffle(rng);
    shuffled.into_iter().take(count).map(String::from).collect()
}
