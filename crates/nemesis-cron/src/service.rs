//! Cron service for scheduled job execution.

use chrono::Local;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Cron schedule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronSchedule {
    pub kind: String,
    pub at_ms: Option<i64>,
    pub every_ms: Option<i64>,
    pub expr: Option<String>,
    pub tz: Option<String>,
}

/// Cron job payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronPayload {
    pub kind: String,
    pub message: String,
    pub command: Option<String>,
    pub deliver: bool,
    pub channel: Option<String>,
    pub to: Option<String>,
    /// Target agent conversation key (e.g. `agent:main:session:<sid>`). When
    /// set, the fired InboundMessage targets that conversation so the exchange
    /// is persisted into it (and, if a live WS tab is bound, pushed in real
    /// time). Empty/None = fall through to routing-derived keying.
    #[serde(default)]
    pub session_key: Option<String>,
}

/// Partial update for a cron job. Each field is optional: `None` = leave
/// unchanged. For the nullable string fields (`channel`/`to`/`session_key`),
/// `Some("")` means "clear". Used by the web `tasks.cron.update` handler.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobPatch {
    pub name: Option<String>,
    pub schedule: Option<CronSchedule>,
    pub message: Option<String>,
    pub channel: Option<String>,
    pub to: Option<String>,
    pub session_key: Option<String>,
    pub enabled: Option<bool>,
}

/// One executed run of a cron job (for the task's run-history view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunRecord {
    pub at_ms: i64,
    /// "ok" | "error" | "executed" (mirrors `last_status`).
    pub status: String,
    pub error: Option<String>,
}

/// Cron job state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobState {
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    /// Recent run history (newest last), capped to `MAX_HISTORY`. `#[serde(default)]`
    /// so jobs created before this field existed load with an empty history.
    #[serde(default)]
    pub history: Vec<CronRunRecord>,
}

/// Maximum number of past runs kept per job in `history`.
const MAX_HISTORY: usize = 50;

impl CronJobState {
    /// Append a run record (newest last), trimming to `MAX_HISTORY`.
    pub fn push_history(&mut self, at_ms: i64, status: String, error: Option<String>) {
        self.history.push(CronRunRecord {
            at_ms,
            status,
            error,
        });
        if self.history.len() > MAX_HISTORY {
            let drop_n = self.history.len() - MAX_HISTORY;
            self.history.drain(..drop_n);
        }
    }
}

/// Cron job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    pub state: CronJobState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub delete_after_run: bool,
}

/// Cron store persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronStoreData {
    version: i32,
    jobs: Vec<CronJob>,
}

/// Job handler callback type. Mirrors Go's `JobHandler func(job *CronJob) (string, error)`.
#[allow(dead_code)]
type JobHandlerFn = Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync>;

/// Cron service.
pub struct CronService {
    store_path: String,
    store: Arc<Mutex<CronStoreData>>,
    running: Arc<Mutex<bool>>,
    stop_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    on_job: Arc<Mutex<Option<Box<dyn Fn(&CronJob) -> Result<String, String> + Send + Sync>>>>,
}

impl CronService {
    /// Create a new cron service.
    pub fn new(store_path: &str) -> Self {
        info!("[Cron] Service created, store_path={}", store_path);
        let svc = Self {
            store_path: store_path.to_string(),
            store: Arc::new(Mutex::new(CronStoreData {
                version: 1,
                jobs: vec![],
            })),
            running: Arc::new(Mutex::new(false)),
            stop_handle: Arc::new(Mutex::new(None)),
            on_job: Arc::new(Mutex::new(None)),
        };
        if let Err(e) = svc.load_store() {
            warn!("[Cron] Failed to load store on init: {}", e);
        }
        svc
    }

    /// Start the cron service.
    pub async fn start(&self) -> Result<(), String> {
        info!("[Cron] Starting cron service");
        let mut running = self.running.lock();
        if *running {
            info!("[Cron] Cron service already running, skipping");
            return Ok(());
        }
        self.recompute_next_runs();
        self.save_store()?;

        *running = true;
        let running_flag = self.running.clone();
        let store = self.store.clone();
        let on_job = self.on_job.clone();
        let store_path = self.store_path.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                if !*running_flag.lock() {
                    break;
                }
                // Check jobs — mirrors Go's checkJobs()
                let now_ms = Local::now().timestamp_millis();
                let due: Vec<String> = {
                    let s = store.lock();
                    s.jobs
                        .iter()
                        .filter(|j| {
                            j.enabled && j.state.next_run_at_ms.map_or(false, |t| t <= now_ms)
                        })
                        .map(|j| j.id.clone())
                        .collect()
                };
                if due.is_empty() {
                    continue;
                }
                debug!("[Cron] Found {} due job(s)", due.len());

                // Reset next_run for due jobs and save (under lock), matching Go's
                // "reset before unlock to avoid duplicate execution" pattern.
                {
                    let mut s = store.lock();
                    for job in &mut s.jobs {
                        if due.contains(&job.id) {
                            job.state.next_run_at_ms = None;
                        }
                    }
                    let _ = save_store_to_path(&store_path, &s);
                }

                // Execute jobs outside lock — mirrors Go's checkJobs -> executeJobByID
                for job_id in &due {
                    // Read-lock: copy job for callback
                    let callback_job = {
                        let s = store.lock();
                        s.jobs.iter().find(|j| j.id == *job_id).cloned()
                    };
                    let Some(callback_job) = callback_job else {
                        continue;
                    };

                    info!(
                        "[Cron] Executing scheduled job: name={}, id={}",
                        callback_job.name, callback_job.id
                    );

                    // Call on_job handler (outside lock)
                    let handler_result = {
                        let on_job = on_job.lock();
                        match on_job.as_ref() {
                            Some(h) => Some(h(&callback_job)),
                            None => None,
                        }
                    };

                    // Write-lock: update job state after execution
                    let start_time = now_ms;
                    {
                        let mut s = store.lock();
                        if let Some(job) = s.jobs.iter_mut().find(|j| j.id == *job_id) {
                            job.state.last_run_at_ms = Some(start_time);
                            job.updated_at_ms = Local::now().timestamp_millis();

                            match &handler_result {
                                Some(Ok(_)) => {
                                    job.state.last_status = Some("ok".to_string());
                                    job.state.last_error = None;
                                    info!(
                                        "[Cron] Scheduled job completed: name={}, id={}, status=ok",
                                        job.name, job.id
                                    );
                                }
                                Some(Err(e)) => {
                                    job.state.last_status = Some("error".to_string());
                                    job.state.last_error = Some(e.clone());
                                    error!(
                                        "[Cron] Scheduled job failed: name={}, id={}, error={}",
                                        job.name, job.id, e
                                    );
                                }
                                None => {
                                    // No handler configured
                                    job.state.last_status = Some("ok".to_string());
                                    job.state.last_error = None;
                                    debug!(
                                        "[Cron] Scheduled job executed (no handler): name={}, id={}",
                                        job.name, job.id
                                    );
                                }
                            }

                            // Append to run history (capped).
                            job.state.push_history(
                                start_time,
                                job.state.last_status.clone().unwrap_or_default(),
                                job.state.last_error.clone(),
                            );

                            // Compute next run time
                            if job.schedule.kind == "at" {
                                if job.delete_after_run {
                                    // Will be removed below
                                } else {
                                    job.enabled = false;
                                    job.state.next_run_at_ms = None;
                                }
                            } else {
                                job.state.next_run_at_ms = compute_next_run(
                                    &job.schedule,
                                    Local::now().timestamp_millis(),
                                );
                            }
                        }
                        // Remove delete_after_run jobs that have completed
                        s.jobs.retain(|j| {
                            j.id != *job_id
                                || !j.delete_after_run
                                || j.state.last_status.as_deref() != Some("ok")
                        });
                        let _ = save_store_to_path(&store_path, &s);
                    }
                }
            }
        });

        *self.stop_handle.lock() = Some(handle);
        info!("[Cron] Cron service started successfully");
        Ok(())
    }

    /// Stop the cron service.
    pub fn stop(&self) {
        info!("[Cron] Stopping cron service");
        *self.running.lock() = false;
        if let Some(h) = self.stop_handle.lock().take() {
            h.abort();
        }
        info!("[Cron] Cron service stopped");
    }

    /// Add a new job (legacy 6-param signature — kept for existing callers and
    /// tests). Delegates to [`add_job_ext`] with `session_key=None, enabled=true`.
    pub fn add_job(
        &self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<&str>,
        to: Option<&str>,
    ) -> Result<CronJob, String> {
        self.add_job_ext(name, schedule, message, deliver, channel, to, None, true)
    }

    /// Add a new job with full control over `session_key` and `enabled`. This is
    /// the path used by the web UI (`tasks.cron.add`).
    pub fn add_job_ext(
        &self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<&str>,
        to: Option<&str>,
        session_key: Option<&str>,
        enabled: bool,
    ) -> Result<CronJob, String> {
        let cron_expr = schedule.expr.as_deref().unwrap_or(schedule.kind.as_str());
        info!(
            "[Cron] Job added: name={}, schedule_kind={}, cron={}, enabled={}",
            name, schedule.kind, cron_expr, enabled
        );
        let now_ms = Local::now().timestamp_millis();
        let delete_after_run = schedule.kind == "at";
        let job = CronJob {
            id: generate_id(),
            name: name.to_string(),
            enabled,
            schedule: schedule.clone(),
            payload: CronPayload {
                kind: "agent_turn".to_string(),
                message: message.to_string(),
                command: None,
                deliver,
                channel: channel.map(|s| s.to_string()),
                to: to.map(|s| s.to_string()),
                session_key: session_key.map(|s| s.to_string()),
            },
            state: CronJobState {
                next_run_at_ms: if enabled {
                    compute_next_run(&schedule, now_ms)
                } else {
                    None
                },
                last_run_at_ms: None,
                last_status: None,
                last_error: None,
                history: Vec::new(),
            },
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            delete_after_run,
        };
        self.store.lock().jobs.push(job.clone());
        self.save_store()?;
        Ok(job)
    }

    /// Remove a job.
    pub fn remove_job(&self, job_id: &str) -> bool {
        info!("[Cron] Job removed: id={}", job_id);
        let removed = {
            let mut store = self.store.lock();
            let before = store.jobs.len();
            store.jobs.retain(|j| j.id != job_id);
            store.jobs.len() < before
        };
        if removed {
            let _ = self.save_store();
        }
        removed
    }

    /// List jobs.
    pub fn list_jobs(&self, include_disabled: bool) -> Vec<CronJob> {
        let store = self.store.lock();
        let jobs: Vec<CronJob> = if include_disabled {
            store.jobs.clone()
        } else {
            store.jobs.iter().filter(|j| j.enabled).cloned().collect()
        };
        debug!(
            "[Cron] Listing jobs, count={}, include_disabled={}",
            jobs.len(),
            include_disabled
        );
        jobs
    }

    /// Get status. Matches Go's `Status()` return:
    /// `{"enabled": bool, "jobs": int, "nextWakeAtMS": Option<i64>}`
    pub fn status(&self) -> serde_json::Value {
        let store = self.store.lock();
        let next_wake = store
            .jobs
            .iter()
            .filter(|j| j.enabled)
            .filter_map(|j| j.state.next_run_at_ms)
            .min();
        serde_json::json!({
            "enabled": *self.running.lock(),
            "jobs": store.jobs.len(),
            "nextWakeAtMS": next_wake,
        })
    }

    /// Get a job by ID.
    pub fn get_job(&self, job_id: &str) -> Option<CronJob> {
        self.store
            .lock()
            .jobs
            .iter()
            .find(|j| j.id == job_id)
            .cloned()
    }

    /// Update a job's name and/or schedule.
    pub fn update_job(
        &self,
        job_id: &str,
        name: Option<&str>,
        schedule: Option<CronSchedule>,
    ) -> Result<(), String> {
        info!("[Cron] Job updated: id={}", job_id);
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        if let Some(n) = name {
            job.name = n.to_string();
        }
        if let Some(s) = schedule {
            job.schedule = s;
            job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
        }
        job.updated_at_ms = now_ms;
        drop(store);
        self.save_store()
    }

    /// Patch a job's fields (name/schedule/message/channel/to/session_key/enabled).
    /// Each `None` field is left unchanged; for `channel`/`to`/`session_key`,
    /// `Some("")` clears the value. Returns the updated job. Used by the web
    /// `tasks.cron.update` handler.
    pub fn patch_job(&self, job_id: &str, patch: &CronJobPatch) -> Result<CronJob, String> {
        info!("[Cron] Job patched: id={}", job_id);
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        if let Some(n) = &patch.name {
            job.name = n.clone();
        }
        if let Some(sched) = &patch.schedule {
            job.schedule = sched.clone();
            if job.enabled {
                job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
            }
        }
        if let Some(m) = &patch.message {
            job.payload.message = m.clone();
        }
        if let Some(c) = patch.channel.as_ref() {
            job.payload.channel = if c.is_empty() { None } else { Some(c.clone()) };
        }
        if let Some(t) = patch.to.as_ref() {
            job.payload.to = if t.is_empty() { None } else { Some(t.clone()) };
        }
        if let Some(sk) = patch.session_key.as_ref() {
            job.payload.session_key = if sk.is_empty() {
                None
            } else {
                Some(sk.clone())
            };
        }
        if let Some(en) = patch.enabled {
            job.enabled = en;
            job.state.next_run_at_ms = if en {
                compute_next_run(&job.schedule, now_ms)
            } else {
                None
            };
        }
        job.updated_at_ms = now_ms;
        let updated = job.clone();
        drop(store);
        self.save_store()?;
        Ok(updated)
    }

    /// Toggle a job's enabled state. Returns the new state.
    pub fn toggle_job(&self, job_id: &str) -> Result<bool, String> {
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        job.enabled = !job.enabled;
        if job.enabled {
            job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
        }
        let new_state = job.enabled;
        info!("[Cron] Job toggled: id={}, enabled={}", job_id, new_state);
        job.updated_at_ms = now_ms;
        drop(store);
        self.save_store()?;
        Ok(new_state)
    }

    /// Enable or disable a specific job. Mirrors Go's `EnableJob(jobID, enabled)`.
    /// Returns the updated job if found.
    pub fn enable_job(&self, job_id: &str, enabled: bool) -> Result<CronJob, String> {
        info!(
            "[Cron] Job enable/disable: id={}, enabled={}",
            job_id, enabled
        );
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        let job = store
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| format!("job not found: {}", job_id))?;
        job.enabled = enabled;
        if enabled {
            job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
        } else {
            job.state.next_run_at_ms = None;
        }
        job.updated_at_ms = now_ms;
        let updated = job.clone();
        drop(store);
        self.save_store()?;
        Ok(updated)
    }

    /// Reload jobs from disk. Mirrors Go's `CronService.Load()`.
    pub fn reload(&self) -> Result<(), String> {
        self.load_store()
    }

    /// Set the job handler callback. Mirrors Go's `SetOnJob(handler)`.
    /// When set, the handler is called when a cron job fires.
    pub fn set_on_job(
        &self,
        handler: impl Fn(&CronJob) -> Result<String, String> + Send + Sync + 'static,
    ) {
        *self.on_job.lock() = Some(Box::new(handler));
    }

    /// Execute a job immediately by ID. Unlike the scheduled fire loop this
    /// does NOT advance `next_run_at_ms` (the schedule still fires normally) —
    /// it's a manual "run now". Invokes the `on_job` handler so the agent
    /// actually runs (the previous implementation only updated state and never
    /// fired the handler, so "run now" was a no-op for delivery). State is
    /// marked `"executed"` on success / `"error"` on handler failure.
    pub fn execute_job(&self, job_id: &str) -> Result<(), String> {
        let start = std::time::Instant::now();
        info!("[Cron] Executing job: id={}", job_id);
        let now_ms = Local::now().timestamp_millis();

        // Read-lock: copy job for callback (mirror the fire loop pattern).
        let callback_job = {
            let s = self.store.lock();
            s.jobs.iter().find(|j| j.id == job_id).cloned()
        };
        let Some(callback_job) = callback_job else {
            error!("[Cron] Job not found for execution: id={}", job_id);
            return Err(format!("job not found: {}", job_id));
        };

        // Call on_job handler (outside lock).
        let handler_result = {
            let on_job = self.on_job.lock();
            match on_job.as_ref() {
                Some(h) => Some(h(&callback_job)),
                None => None,
            }
        };

        // Write-lock: update state after execution.
        {
            let mut store = self.store.lock();
            if let Some(job) = store.jobs.iter_mut().find(|j| j.id == job_id) {
                job.state.last_run_at_ms = Some(now_ms);
                job.updated_at_ms = Local::now().timestamp_millis();
                match &handler_result {
                    Some(Ok(_)) | None => {
                        job.state.last_status = Some("executed".to_string());
                        job.state.last_error = None;
                    }
                    Some(Err(e)) => {
                        job.state.last_status = Some("error".to_string());
                        job.state.last_error = Some(e.clone());
                        error!(
                            "[Cron] Manual execute failed: name={}, id={}, error={}",
                            job.name, job.id, e
                        );
                    }
                }
                // Append to run history (capped).
                job.state.push_history(
                    now_ms,
                    job.state.last_status.clone().unwrap_or_default(),
                    job.state.last_error.clone(),
                );
            }
            drop(store);
            self.save_store()?;
        }
        let elapsed = start.elapsed().as_millis();
        info!(
            "[Cron] Job executed: name={}, id={}, duration_ms={}",
            callback_job.name, job_id, elapsed
        );
        Ok(())
    }

    /// Validate a cron expression. Supports both 5-field (min hour day month weekday)
    /// and 6-field (sec min hour day month weekday) expressions.
    /// Also supports L, W, #, JAN-DEC, SUN-SAT names — matching Go's gronx.
    pub fn validate_schedule(expr: &str) -> Result<(), String> {
        if expr.trim().is_empty() {
            return Err("cron expression is empty".to_string());
        }
        croner::Cron::from_str(expr)
            .map_err(|e| format!("invalid cron expression '{}': {}", expr, e))?;
        Ok(())
    }

    /// Parse a cron expression and return a human-readable description.
    pub fn describe_schedule(expr: &str) -> String {
        // Accept both 5-field and 6-field
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() < 5 || parts.len() > 6 {
            return format!("Invalid: {}", expr);
        }

        // Determine offset: 6-field has seconds at index 0
        let has_seconds = parts.len() == 6;
        let off = if has_seconds { 1 } else { 0 };

        let minute = parts[off];
        let hour = parts[off + 1];
        let day = parts[off + 2];
        let month = parts[off + 3];
        let weekday = parts[off + 4];

        // Every minute: all wildcard
        if minute == "*" && hour == "*" && day == "*" && month == "*" && weekday == "*" {
            return "Every minute".to_string();
        }
        // Every N minutes
        if let Some(step) = minute.strip_prefix("*/") {
            if hour == "*" && day == "*" && month == "*" && weekday == "*" {
                return format!("Every {} minutes", step);
            }
        }
        // Daily at specific time
        if minute != "*" && hour != "*" && day == "*" && month == "*" && weekday == "*" {
            return format!("Daily at {}:{}", hour, minute);
        }
        // Weekday schedule
        if weekday != "*" && day == "*" && month == "*" {
            return format!("At {}:{} on weekdays {}", hour, minute, weekday);
        }

        expr.to_string()
    }

    fn load_store(&self) -> Result<(), String> {
        if !std::path::Path::new(&self.store_path).exists() {
            return Ok(());
        }
        let data = std::fs::read_to_string(&self.store_path).map_err(|e| format!("read: {}", e))?;
        let store: CronStoreData =
            serde_json::from_str(&data).map_err(|e| format!("parse: {}", e))?;
        *self.store.lock() = store;
        Ok(())
    }

    fn save_store(&self) -> Result<(), String> {
        if let Some(parent) = std::path::Path::new(&self.store_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        }
        let s = self.store.lock();
        save_store_to_path(&self.store_path, &s)
    }

    fn recompute_next_runs(&self) {
        let now_ms = Local::now().timestamp_millis();
        let mut store = self.store.lock();
        for job in &mut store.jobs {
            if job.enabled {
                job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms);
            }
        }
    }

    /// Get the next wake time in milliseconds (mirrors Go's getNextWakeMS).
    #[allow(dead_code)]
    fn get_next_wake_ms(&self) -> Option<i64> {
        let s = self.store.lock();
        s.jobs
            .iter()
            .filter(|j| j.enabled)
            .filter_map(|j| j.state.next_run_at_ms)
            .min()
    }
}

/// Save store data to a file path. Used by both the service method and the spawn loop.
fn save_store_to_path(path: &str, data: &CronStoreData) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(data).map_err(|e| format!("ser: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("write: {}", e))
}

pub fn compute_next_run(schedule: &CronSchedule, now_ms: i64) -> Option<i64> {
    match schedule.kind.as_str() {
        "at" => schedule.at_ms.filter(|&t| t > now_ms),
        "every" => schedule.every_ms.filter(|&ms| ms > 0).map(|ms| now_ms + ms),
        "cron" => {
            let expr = match &schedule.expr {
                Some(e) if !e.is_empty() => e,
                _ => return None,
            };

            let now = chrono::DateTime::from_timestamp_millis(now_ms)
                .map(|dt| dt.with_timezone(&chrono::Local))
                .unwrap_or_else(|| chrono::Local::now());

            let cron = match croner::Cron::from_str(expr) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("[cron] failed to parse expr '{}': {}", expr, e);
                    return None;
                }
            };

            // Interpret the cron fields in the configured timezone, or LOCAL
            // when none is specified. The web UI never sets tz, so an expression
            // like "0 9 * * *" must mean 09:00 local — the previous default of
            // UTC made "0 9" fire at 09:00 UTC (e.g. 17:00 in UTC+8). `now` is
            // already DateTime<Local>, so passing it to croner directly makes it
            // evaluate the fields against local time.
            let tz_to_use: Option<chrono_tz::Tz> = match schedule.tz.as_deref() {
                Some(tz_str) => match tz_str.parse() {
                    Ok(tz) => Some(tz),
                    Err(_) => {
                        tracing::warn!("[cron] invalid timezone '{}', using local", tz_str);
                        None
                    }
                },
                None => None,
            };
            let next_ms = match tz_to_use {
                Some(tz) => cron
                    .find_next_occurrence(&now.with_timezone(&tz), false)
                    .map(|dt| dt.with_timezone(&chrono::Local).timestamp_millis()),
                None => cron
                    .find_next_occurrence(&now, false)
                    .map(|dt| dt.with_timezone(&chrono::Local).timestamp_millis()),
            };
            match next_ms {
                Ok(ms) => Some(ms),
                Err(e) => {
                    tracing::warn!(
                        "[cron] failed to compute next run for expr '{}': {}",
                        expr,
                        e
                    );
                    None
                }
            }
        }
        _ => None,
    }
}

fn generate_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.r#gen();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests;
