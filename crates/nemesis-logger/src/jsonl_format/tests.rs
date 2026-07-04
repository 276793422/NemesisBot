use super::*;
use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Registry, fmt};

/// Spin up a subscriber using JsonLinesFormatter + a buf writer, run `run`,
/// return everything that landed in the buffer.
fn collect_jsonl<F>(run: F) -> String
where
    F: FnOnce(),
{
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let buf_clone = buf.clone();
    let make_writer = move || {
        let arc = buf_clone.clone();
        // tracing-subscriber wants 'static — wrap in a struct that impls io::Write.
        struct ArcBuf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for ArcBuf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        ArcBuf(arc)
    };
    let layer = fmt::layer()
        .event_format(JsonLinesFormatter)
        .with_writer(make_writer);

    let subscriber = Registry::default().with(layer);
    tracing::subscriber::with_default(subscriber, run);

    let guard = buf.lock().unwrap();
    String::from_utf8_lossy(&guard).into_owned()
}

#[test]
fn formatter_emits_one_json_line_per_event() {
    let out = collect_jsonl(|| {
        tracing::info!("first");
        tracing::warn!("second");
    });

    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected exactly 2 lines, got: {:?}", lines);

    // Each line must be a single JSON object.
    for line in &lines {
        let parsed: serde_json::Value =
            serde_json::from_str(line).expect("line must be valid JSON");
        assert!(parsed.is_object(), "expected object, got: {}", parsed);
    }
}

#[test]
fn formatter_includes_required_fields() {
    let out = collect_jsonl(|| {
        tracing::info!(user_id = 7, "login event");
    });

    let line = out.lines().next().expect("at least one line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let obj = v.as_object().unwrap();

    // Required fields per SseLogEvent
    for key in [
        "seq",
        "level",
        "timestamp",
        "target",
        "source",
        "component",
        "message",
        "fields",
    ] {
        assert!(obj.contains_key(key), "missing field: {} in {}", key, line);
    }
    assert_eq!(obj.get("level").and_then(|v| v.as_str()), Some("INFO"));
    assert_eq!(
        obj.get("message").and_then(|v| v.as_str()),
        Some("login event")
    );
    assert_eq!(
        obj.get("fields")
            .and_then(|v| v.get("user_id"))
            .and_then(|v| v.as_i64()),
        Some(7)
    );
    // seq must be > 0 (boot_unix_ms is non-zero after the system clock is sane).
    let seq = obj.get("seq").and_then(|v| v.as_u64()).unwrap();
    assert!(seq > 0, "seq should be non-zero, got {}", seq);
}
