//! Tracing Layer that forwards events to an SSE EventHub.
//!
//! Production code uses `tracing::info!` / `tracing::warn!` / etc. directly (678+ call sites).
//! `NemesisLogger::log()` is only used in tests. To get those production logs onto the dashboard's
//! 实时事件流 tab, we need a `tracing_subscriber::Layer` that intercepts every tracing event,
//! serializes it, and forwards it to a callback (which the gateway wires to `EventHub::publish`).
//!
//! ## Why a callback, not a direct `EventHub` dep
//!
//! `nemesis-logger` is a low-level crate that must not depend on `nemesis-web` (that would be a
//! circular dep). Instead we expose a generic `Fn(SseLogEvent)` callback; the gateway owns the
//! `EventHub` and registers a closure that publishes to it. This keeps the layer testable in
//! isolation (visit a test event, assert the callback fires) without dragging in axum/tokio.
//!
//! ## Field visiting
//!
//! tracing events have a `message` field (the format string) plus user fields like `user_id=42`.
//! We extract `message` as a top-level string so the dashboard can render it without parsing;
//! everything else goes into the `fields` map as JSON values.

use chrono::Local;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Process boot time in unix milliseconds. Captured once on first access; used to
/// build globally-unique `seq` values so events from different process runs can never
/// collide on the frontend's `maxSeqSeen` dedup.
static BOOT_UNIX_MS: OnceLock<u64> = OnceLock::new();

/// Monotonic counter incremented for every event constructed in this process.
static SEQ_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn boot_unix_ms() -> u64 {
    *BOOT_UNIX_MS.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    })
}

/// Build a globally-unique sequence number.
///
/// Layout: high 44 bits = boot_unix_ms, low 20 bits = per-process counter (1M events
/// per boot before wraparound — far more than any realistic session produces between
/// restarts). The boot_ms prefix guarantees cross-restart uniqueness; the counter
/// guarantees in-process monotonicity.
pub fn next_seq() -> u64 {
    (boot_unix_ms() << 20) | (SEQ_COUNTER.fetch_add(1, Ordering::Relaxed) & 0xFFFFF)
}

/// A serialized log event ready to be pushed onto the SSE EventHub.
#[derive(Debug, Clone, Serialize)]
pub struct SseLogEvent {
    /// Globally-unique sequence number — frontend uses this to dedup between history
    /// (read from nemesisbot.log) and live SSE delivery during the initial page-load
    /// race window. See [`next_seq`] for layout.
    pub seq: u64,
    /// Log level: `TRACE` / `DEBUG` / `INFO` / `WARN` / `ERROR`.
    pub level: String,
    /// RFC3339 local timestamp.
    pub timestamp: String,
    /// tracing target (module path), e.g. `nemesis_agent::loop`.
    pub target: String,
    /// Source classification derived from the target prefix. One of:
    /// `general` / `llm` / `cluster` / `security`. Used by the frontend to filter by 来源.
    pub source: String,
    /// Short component name — first segment of the target (crate name).
    pub component: String,
    /// File path of the callsite, if known. Empty string if unavailable.
    pub file: String,
    /// Line number of the callsite, if known. `0` if unavailable.
    pub line: u32,
    /// The format-string message (event's `message` field). Empty if the event had no message.
    pub message: String,
    /// All other structured fields on the event, serialized to JSON.
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl SseLogEvent {
    /// Classify a tracing target into a dashboard source bucket.
    ///
    /// Mapping is by crate prefix — keeps the per-event cost down (a single `starts_with` chain).
    /// Anything unrecognized falls back to `general`.
    pub fn classify_source(target: &str) -> String {
        let first = target.split("::").next().unwrap_or(target);
        match first {
            "nemesis_providers" | "nemesis_llm_bridge" => "llm",
            "nemesis_cluster" | "nemesis_cluster_rpc" => "cluster",
            "nemesis_security" | "nemesis_scanner" => "security",
            _ => "general",
        }
        .to_string()
    }
}

/// Build an `SseLogEvent` from a tracing event: visits fields, classifies source,
/// stamps a globally-unique `seq`, and captures a local RFC3339 timestamp.
///
/// This is the **single source of truth** for event construction — both
/// `GlobalSseLogLayer::on_event` (for SSE delivery) and `JsonLinesFormatter::format_event`
/// (for nemesisbot.log file writes) call this function, guaranteeing that the file
/// format is byte-identical to what SSE pushes to the dashboard. Frontend dedup
/// relies on this consistency.
pub fn build_sse_log_event(event: &Event<'_>) -> SseLogEvent {
    let meta = event.metadata();

    let mut visitor = SseFieldVisitor::default();
    event.record(&mut visitor);

    let target = meta.target().to_string();
    SseLogEvent {
        seq: next_seq(),
        level: meta.level().to_string(),
        timestamp: Local::now().to_rfc3339(),
        source: SseLogEvent::classify_source(&target),
        component: target
            .split("::")
            .next()
            .unwrap_or(&target)
            .to_string(),
        target,
        file: meta.file().unwrap_or("").to_string(),
        line: meta.line().unwrap_or(0),
        message: visitor.message,
        fields: visitor.fields,
    }
}

/// Tracing Layer that forwards events to a callback.
pub struct SseLogLayer<F>
where
    F: Fn(SseLogEvent) + Send + Sync + 'static,
{
    callback: F,
}

impl<F> SseLogLayer<F>
where
    F: Fn(SseLogEvent) + Send + Sync + 'static,
{
    /// Create a new layer that invokes `callback` for every tracing event.
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<S, F> Layer<S> for SseLogLayer<F>
where
    S: Subscriber,
    F: Fn(SseLogEvent) + Send + Sync + 'static,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let sse_event = build_sse_log_event(event);
        (self.callback)(sse_event);
    }
}

/// Visitor that pulls `message` out as a string and stuffs everything else into a JSON map.
#[derive(Default)]
struct SseFieldVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl SseFieldVisitor {
    fn record_value(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields.insert(field.name().to_string(), serde_json::Value::String(rendered));
        }
    }
}

impl Visit for SseFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_value(field, format!("{:?}", value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "message" {
            self.record_value(field, value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), serde_json::Value::Bool(value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "message" {
            self.record_value(field, value.to_string());
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Value::Number(serde_json::Number::from(value)),
            );
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "message" {
            self.record_value(field, value.to_string());
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::Number::from(value).into(),
            );
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        // f64 doesn't always fit in serde_json::Number (NaN/Inf). Fall back to string for safety.
        if field.name() == "message" {
            self.record_value(field, value.to_string());
        } else if let Some(n) = serde_json::Number::from_f64(value) {
            self.fields.insert(field.name().to_string(), n.into());
        } else {
            self.record_value(field, value.to_string());
        }
    }
}

/// Convenience: extract a stable summary HashMap from an `SseLogEvent` (for tests / debugging).
#[cfg(test)]
impl SseLogEvent {
    /// Flatten into a HashMap of String→String for snapshot-style assertions.
    pub fn to_summary_map(&self) -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("level".to_string(), self.level.clone());
        m.insert("source".to_string(), self.source.clone());
        m.insert("component".to_string(), self.component.clone());
        m.insert("message".to_string(), self.message.clone());
        m
    }
}

// ---------------------------------------------------------------------------
// Global callback variant (for production use with late-bound EventHub)
// ---------------------------------------------------------------------------

/// Type alias for the callback stored in the global slot.
pub type LogCallback = Arc<dyn Fn(SseLogEvent) + Send + Sync>;

/// Internal slot type: an `Option<callback>` guarded by a mutex.
pub type LogCallbackSlot = Arc<Mutex<Option<LogCallback>>>;

/// Returns the global slot that holds the currently-installed SSE log callback.
///
/// The slot is created lazily on first access and persists for the process lifetime.
/// `GlobalSseLogLayer` reads from this slot on every event — when the slot is `None`,
/// events are dropped. The gateway calls `set_global_log_callback` after the
/// `EventHub` is constructed (which happens after logger init).
pub fn global_log_callback_slot() -> LogCallbackSlot {
    static SLOT: OnceLock<LogCallbackSlot> = OnceLock::new();
    SLOT.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

/// Install a global callback that receives every `SseLogEvent` produced by
/// `GlobalSseLogLayer`. Replaces any previously-installed callback.
pub fn set_global_log_callback<F>(cb: F)
where
    F: Fn(SseLogEvent) + Send + Sync + 'static,
{
    let slot = global_log_callback_slot();
    *slot.lock().unwrap() = Some(Arc::new(cb));
}

/// Removes any globally-installed callback. Useful for tests that need isolation.
pub fn clear_global_log_callback() {
    let slot = global_log_callback_slot();
    *slot.lock().unwrap() = None;
}

/// A `tracing_subscriber::Layer` that forwards every event to the callback installed
/// via `set_global_log_callback`. If no callback is installed, events are silently
/// dropped (the cost is one `OnceLock::get` + one `Mutex::lock` per event).
///
/// The indirection through a global slot exists because the gateway initializes the
/// logger before creating the `EventHub` it eventually publishes into. The layer
/// is registered once at logger init; the callback is plugged in later when the
/// EventHub exists.
pub struct GlobalSseLogLayer;

impl<S> Layer<S> for GlobalSseLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let sse_event = build_sse_log_event(event);

        // Hold the lock only long enough to clone the Arc<callback> — never invoke
        // the callback while holding it, or a slow callback would block other threads'
        // logging.
        let callback = {
            let slot = global_log_callback_slot();
            let guard = slot.lock().unwrap();
            guard.clone()
        };
        if let Some(cb) = callback {
            cb(sse_event);
        }
    }
}

#[cfg(test)]
mod tests;
