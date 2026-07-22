//! Tracing formatter that writes one JSON line per event to `nemesisbot.YYYY-MM-DD.log`.
//!
//! Each line is a serialized [`crate::sse_layer::SseLogEvent`] — byte-identical to what
//! the SSE EventHub pushes to the dashboard. This is what makes "load history on page
//! open" + "dedup against live SSE" work: the file format and the SSE format share a
//! single construction path (`build_sse_log_event`) and a single globally-unique `seq`.
//!
//! Console output continues to use [`crate::GoStyleFormatter`] for human readability.

use std::fmt;

use tracing::Event;
use tracing::Subscriber;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Tracing event formatter that serializes each event as a single-line JSON object
/// (JSONL / NDJSON), matching the SSE EventHub payload format.
pub struct JsonLinesFormatter;

impl<S, N> FormatEvent<S, N> for JsonLinesFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // Build the same struct the SSE layer produces, then serialize as one JSON line.
        // Field visiting happens inside build_sse_log_event; we deliberately don't call
        // ctx.format_fields because that would re-serialize via tracing-subscriber's
        // FormatFields machinery, producing a different shape.
        let sse_event = crate::sse_layer::build_sse_log_event(event);
        let json = serde_json::to_string(&sse_event).map_err(|_| fmt::Error)?;
        writeln!(writer, "{}", json)
    }
}

#[cfg(test)]
mod tests;
