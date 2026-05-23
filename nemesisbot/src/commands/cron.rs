//! Cron command - manage scheduled tasks.

use anyhow::Result;
use crate::common;

#[derive(clap::Subcommand)]
pub enum CronAction {
    /// List all scheduled jobs
    List,
    /// Add a new scheduled job
    Add {
        /// Job name
        #[arg(short, long)]
        name: String,
        /// Message for agent
        #[arg(short, long)]
        message: String,
        /// Run every N seconds
        #[arg(short, long)]
        every: Option<u64>,
        /// Cron expression
        #[arg(short, long)]
        cron: Option<String>,
        /// Deliver response to channel
        #[arg(short, long)]
        deliver: bool,
        /// Recipient for delivery
        #[arg(long)]
        to: Option<String>,
        /// Channel for delivery
        #[arg(long)]
        channel: Option<String>,
    },
    /// Remove a job by ID
    Remove {
        /// Job ID
        id: String,
    },
    /// Enable a job
    Enable {
        /// Job ID
        id: String,
    },
    /// Disable a job
    Disable {
        /// Job ID
        id: String,
    },
}

pub fn run(action: CronAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let store_path = common::cron_store_path(&home);

    match action {
        CronAction::List => {
            println!("Scheduled Jobs");
            println!("===============");
            if store_path.exists() {
                let data = std::fs::read_to_string(&store_path)?;
                let jobs: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(arr) = jobs.as_array() {
                    if arr.is_empty() {
                        println!("  No scheduled jobs.");
                    } else {
                        for job in arr {
                            let id = job.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                            let name = job.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            let enabled = job.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

                            // Structured schedule: try "display" field first, fall back to plain string
                            let schedule_display = job.get("schedule")
                                .and_then(|s| {
                                    // If schedule is an object with "display", use it
                                    if s.is_object() {
                                        s.get("display").and_then(|v| v.as_str())
                                    } else {
                                        s.as_str()
                                    }
                                })
                                .unwrap_or("?");

                            // Compute next run for interval-based jobs
                            let next_run = job.get("schedule")
                                .and_then(|s| s.get("every_ms").and_then(|v| v.as_u64()))
                                .map(|ms| {
                                    let secs = ms / 1000;
                                    format!("every {}s", secs)
                                })
                                .unwrap_or_else(|| schedule_display.to_string());

                            println!("  {} ({})", name, id);
                            println!("    Schedule: {}", next_run);
                            println!("    Status: {}", if enabled { "enabled" } else { "disabled" });
                            println!();
                        }
                    }
                } else {
                    println!("  No scheduled jobs.");
                }
            } else {
                println!("  No scheduled jobs.");
                println!("  Add one with: nemesisbot cron add -n <name> -m <message> -e <seconds>");
            }
        }
        CronAction::Add { name, message, every, cron, deliver, to, channel } => {
            let schedule = if let Some(secs) = every {
                serde_json::json!({
                    "kind": "interval",
                    "every_ms": secs * 1000,
                    "display": format!("every {}s", secs)
                })
            } else if let Some(ref cron_expr) = cron {
                serde_json::json!({
                    "kind": "cron",
                    "expr": cron_expr,
                    "display": format!("cron: {}", cron_expr)
                })
            } else {
                println!("Error: Either --every or --cron must be specified.");
                return Ok(());
            };

            let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
            let job = serde_json::json!({
                "id": id,
                "name": name,
                "message": message,
                "schedule": schedule,
                "deliver": deliver,
                "to": to,
                "channel": channel,
                "enabled": true,
            });

            // Load/create store
            let dir = store_path.parent().unwrap();
            let _ = std::fs::create_dir_all(dir);
            let mut jobs: Vec<serde_json::Value> = if store_path.exists() {
                let data = std::fs::read_to_string(&store_path)?;
                serde_json::from_str(&data).unwrap_or_default()
            } else {
                vec![]
            };
            jobs.push(job);
            std::fs::write(&store_path, serde_json::to_string_pretty(&serde_json::Value::Array(jobs)).unwrap_or_default())?;

            println!("Added job '{}' ({})", name, id);
        }
        CronAction::Remove { id } => {
            if store_path.exists() {
                let data = std::fs::read_to_string(&store_path)?;
                let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
                let before = jobs.len();
                jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some(&id));
                if jobs.len() < before {
                    std::fs::write(&store_path, serde_json::to_string_pretty(&serde_json::Value::Array(jobs)).unwrap_or_default())?;
                    println!("Removed job {}", id);
                } else {
                    println!("Job {} not found.", id);
                }
            } else {
                println!("Job {} not found.", id);
            }
        }
        CronAction::Enable { id } => {
            toggle_job(&store_path, &id, true)
        }
        CronAction::Disable { id } => {
            toggle_job(&store_path, &id, false)
        }
    }
    Ok(())
}

fn toggle_job(store_path: &std::path::Path, id: &str, enabled: bool) {
    if store_path.exists() {
        if let Ok(data) = std::fs::read_to_string(store_path) {
            let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
            let mut found = false;
            for job in &mut jobs {
                if job.get("id").and_then(|v| v.as_str()) == Some(id) {
                    if let Some(obj) = job.as_object_mut() {
                        obj.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                        found = true;
                    }
                }
            }
            if found {
                let _ = std::fs::write(store_path, serde_json::to_string_pretty(&serde_json::Value::Array(jobs)).unwrap_or_default());
                println!("Job {} {}", id, if enabled { "enabled" } else { "disabled" });
            } else {
                println!("Job {} not found.", id);
            }
        }
    } else {
        println!("Job {} not found.", id);
    }
}

#[cfg(test)]
mod tests;
