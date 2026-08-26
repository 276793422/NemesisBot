//! Shared test-only helpers for nemesis-forge unit tests.
//!
//! (quality-hardening goal 冲刺 S8)

/// Install a thread-local tracing subscriber for the current test thread and
/// return its guard.
///
/// Why: with no subscriber installed, `tracing` macro callsites are disabled
/// and field value expressions (`tracing::info!(a = f(), ...)`) are never
/// evaluated — coverage tools then mark those lines as missed even though the
/// surrounding code ran. This installs a TRACE-level `fmt` subscriber that
/// writes to the void, so field expressions execute without polluting test
/// output. Uses `set_default` (thread-local), so it never races with parallel
/// tests or a possible global subscriber.
pub fn quiet_trace_guard() -> tracing::subscriber::DefaultGuard {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish();
    tracing::subscriber::set_default(subscriber)
}
