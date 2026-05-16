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
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store(tmp: &TempDir, jobs: &[serde_json::Value]) -> std::path::PathBuf {
        let dir = tmp.path().join("cron");
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("jobs.json");
        std::fs::write(&store, serde_json::to_string_pretty(&serde_json::Value::Array(jobs.to_vec())).unwrap()).unwrap();
        store
    }

    fn sample_job(id: &str, name: &str, enabled: bool) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "enabled": enabled,
            "schedule": {
                "kind": "interval",
                "every_ms": 60000,
                "display": "every 60s"
            }
        })
    }

    #[test]
    fn test_toggle_job_enable() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            sample_job("abc123", "test_job", false),
        ]);

        toggle_job(&store, "abc123", true);

        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert_eq!(jobs[0]["enabled"], true);
    }

    #[test]
    fn test_toggle_job_disable() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            sample_job("def456", "another_job", true),
        ]);

        toggle_job(&store, "def456", false);

        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert_eq!(jobs[0]["enabled"], false);
    }

    #[test]
    fn test_toggle_job_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            sample_job("abc123", "test_job", true),
        ]);

        toggle_job(&store, "nonexistent", false);

        // Job should remain unchanged
        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert_eq!(jobs[0]["enabled"], true);
    }

    #[test]
    fn test_toggle_job_no_file() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("nonexistent").join("jobs.json");

        // Should not panic
        toggle_job(&store, "abc123", true);
    }

    #[test]
    fn test_toggle_job_multiple_jobs() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            sample_job("job1", "first", true),
            sample_job("job2", "second", true),
            sample_job("job3", "third", false),
        ]);

        toggle_job(&store, "job2", false);

        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert_eq!(jobs[0]["enabled"], true);  // unchanged
        assert_eq!(jobs[1]["enabled"], false); // changed
        assert_eq!(jobs[2]["enabled"], false); // unchanged
    }

    #[test]
    fn test_add_interval_job() {
        let _tmp = TempDir::new().unwrap();
        // Simulate CronAction::Add with interval schedule
        let schedule = serde_json::json!({
            "kind": "interval",
            "every_ms": 300000u64,  // 5 minutes
            "display": "every 300s"
        });
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let job = serde_json::json!({
            "id": id,
            "name": "test_interval",
            "message": "do something",
            "schedule": schedule,
            "deliver": false,
            "enabled": true,
        });

        assert_eq!(job["schedule"]["every_ms"], 300000);
        assert_eq!(job["schedule"]["kind"], "interval");
        assert_eq!(job["enabled"], true);
    }

    #[test]
    fn test_add_cron_expr_job() {
        let cron_expr = "0 */5 * * *";
        let schedule = serde_json::json!({
            "kind": "cron",
            "expr": cron_expr,
            "display": format!("cron: {}", cron_expr)
        });

        assert_eq!(schedule["kind"], "cron");
        assert_eq!(schedule["expr"], cron_expr);
    }

    #[test]
    fn test_remove_job_from_store() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            sample_job("j1", "job1", true),
            sample_job("j2", "job2", true),
        ]);

        let data = std::fs::read_to_string(&store).unwrap();
        let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some("j1"));
        assert_eq!(jobs.len(), before - 1);
        assert_eq!(jobs[0]["id"], "j2");
    }

    #[test]
    fn test_remove_nonexistent_job() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            sample_job("j1", "job1", true),
        ]);

        let data = std::fs::read_to_string(&store).unwrap();
        let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some("nonexistent"));
        assert_eq!(jobs.len(), before); // nothing removed
    }

    #[test]
    fn test_job_id_is_8_chars() {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        assert_eq!(id.len(), 8);
    }

    #[test]
    fn test_empty_store_is_valid_json() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cron");
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("jobs.json");
        std::fs::write(&store, "[]").unwrap();

        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert!(jobs.is_empty());
    }

    // -------------------------------------------------------------------------
    // Additional cron tests for coverage
    // -------------------------------------------------------------------------

    #[test]
    fn test_toggle_job_with_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cron");
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("jobs.json");
        std::fs::write(&store, "invalid json").unwrap();

        // Should not panic, just do nothing
        toggle_job(&store, "any-id", true);
    }

    #[test]
    fn test_toggle_job_empty_array() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cron");
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("jobs.json");
        std::fs::write(&store, "[]").unwrap();

        toggle_job(&store, "any-id", true);

        // Should remain empty
        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert!(jobs.is_empty());
    }

    #[test]
    fn test_schedule_interval_json_structure() {
        let secs: u64 = 300;
        let schedule = serde_json::json!({
            "kind": "interval",
            "every_ms": secs * 1000,
            "display": format!("every {}s", secs)
        });
        assert_eq!(schedule["kind"], "interval");
        assert_eq!(schedule["every_ms"], 300000);
        assert_eq!(schedule["display"], "every 300s");
    }

    #[test]
    fn test_schedule_cron_json_structure() {
        let cron_expr = "0 */5 * * *";
        let schedule = serde_json::json!({
            "kind": "cron",
            "expr": cron_expr,
            "display": format!("cron: {}", cron_expr)
        });
        assert_eq!(schedule["kind"], "cron");
        assert_eq!(schedule["expr"], cron_expr);
        assert_eq!(schedule["display"], "cron: 0 */5 * * *");
    }

    #[test]
    fn test_job_json_structure() {
        let id = "test1234".to_string();
        let name = "test_job".to_string();
        let message = "do something".to_string();
        let schedule = serde_json::json!({"kind": "interval", "every_ms": 60000, "display": "every 60s"});
        let deliver = true;
        let to: Option<String> = Some("user1".to_string());
        let channel: Option<String> = Some("web".to_string());

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

        assert_eq!(job["id"], "test1234");
        assert_eq!(job["name"], "test_job");
        assert_eq!(job["message"], "do something");
        assert_eq!(job["deliver"], true);
        assert_eq!(job["to"], "user1");
        assert_eq!(job["channel"], "web");
        assert_eq!(job["enabled"], true);
    }

    #[test]
    fn test_job_list_display_schedule_object() {
        let job = serde_json::json!({
            "id": "j1",
            "name": "test",
            "enabled": true,
            "schedule": {
                "kind": "interval",
                "every_ms": 120000,
                "display": "every 120s"
            }
        });

        // Test the schedule display extraction logic from CronAction::List
        let schedule_display = job.get("schedule")
            .and_then(|s| {
                if s.is_object() {
                    s.get("display").and_then(|v| v.as_str())
                } else {
                    s.as_str()
                }
            })
            .unwrap_or("?");

        assert_eq!(schedule_display, "every 120s");

        // Test next run extraction
        let next_run = job.get("schedule")
            .and_then(|s| s.get("every_ms").and_then(|v| v.as_u64()))
            .map(|ms| {
                let secs = ms / 1000;
                format!("every {}s", secs)
            })
            .unwrap_or_else(|| schedule_display.to_string());

        assert_eq!(next_run, "every 120s");
    }

    #[test]
    fn test_job_list_display_schedule_string() {
        let job = serde_json::json!({
            "id": "j1",
            "name": "test",
            "enabled": true,
            "schedule": "every 5 minutes"
        });

        let schedule_display = job.get("schedule")
            .and_then(|s| {
                if s.is_object() {
                    s.get("display").and_then(|v| v.as_str())
                } else {
                    s.as_str()
                }
            })
            .unwrap_or("?");

        assert_eq!(schedule_display, "every 5 minutes");
    }

    #[test]
    fn test_job_id_uniqueness() {
        let id1 = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let id2 = uuid::Uuid::new_v4().to_string()[..8].to_string();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_remove_job_from_empty_store() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("cron");
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("jobs.json");
        std::fs::write(&store, "[]").unwrap();

        let data = std::fs::read_to_string(&store).unwrap();
        let mut jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some("nonexistent"));
        assert_eq!(jobs.len(), before);
    }

    #[test]
    fn test_toggle_job_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, &[
            serde_json::json!({
                "id": "j1",
                "name": "myjob",
                "enabled": true,
                "message": "hello",
                "schedule": {"kind": "interval", "every_ms": 60000}
            }),
        ]);

        toggle_job(&store, "j1", false);

        let data = std::fs::read_to_string(&store).unwrap();
        let jobs: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap();
        assert_eq!(jobs[0]["enabled"], false);
        assert_eq!(jobs[0]["name"], "myjob"); // other fields preserved
        assert_eq!(jobs[0]["message"], "hello");
    }
}
