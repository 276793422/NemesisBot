//! Custom tracing formatter and dual-writer (console + file) support.
//!
//! `GoStyleFormatter` produces log lines like:
//! ```text
//! 2026-05-22T05:50:26.935143 INFO nemesis_desktop::process::manager:100 [ProcessManager] Stopping...
//! ```
//!
//! `DualMakeWriter` / `DualWriter` write to stderr **and** an optional log file.

use std::fmt;
use std::fs::File;
use std::io;
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

// ---------------------------------------------------------------------------
// GoStyleFormatter
// ---------------------------------------------------------------------------

/// Tracing event formatter that emits Go-style log lines with line numbers.
///
/// Format: `{timestamp} {LEVEL} {target}:{line} {fields}`
pub struct GoStyleFormatter;

impl<S, N> FormatEvent<S, N> for GoStyleFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();

        // 1. Timestamp — microsecond precision, local time
        write!(
            writer,
            "{} ",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f")
        )?;

        // 2. Level — right-aligned, 5 chars (INFO, WARN, ERROR, DEBUG, TRACE)
        write!(writer, "{:>5} ", meta.level())?;

        // 3. Module path (target)
        write!(writer, "{}", meta.target())?;

        // 4. Line number
        if let Some(line) = meta.line() {
            write!(writer, ":{}", line)?;
        }

        write!(writer, " ")?;

        // 5. Formatted message + structured fields
        ctx.format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

// ---------------------------------------------------------------------------
// DualWriter / DualMakeWriter
// ---------------------------------------------------------------------------

/// A writer that outputs to an optional stderr console and an optional log file.
pub struct DualWriter {
    console: bool,
    file: Option<Arc<Mutex<File>>>,
}

impl io::Write for DualWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.console {
            let n = io::stderr().write(buf)?;
            if let Some(file) = &self.file {
                let _ = file.lock().write(buf);
            }
            Ok(n)
        } else if let Some(file) = &self.file {
            // File-only mode — no console output
            file.lock().write(buf)
        } else {
            // Neither console nor file — discard
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.console {
            io::stderr().flush()?;
        }
        if let Some(file) = &self.file {
            let _ = file.lock().flush();
        }
        Ok(())
    }
}

/// Factory for `DualWriter` instances.
///
/// - `console_only()` — stderr only, no file
/// - `with_file(path)` — stderr + file
/// - `file_only(path)` — file only, no stderr
pub struct DualMakeWriter {
    console: bool,
    file: Option<Arc<Mutex<File>>>,
}

impl DualMakeWriter {
    /// Create a writer that only outputs to stderr.
    pub fn console_only() -> Self {
        Self {
            console: true,
            file: None,
        }
    }

    /// Create a writer that outputs to stderr **and** the given file path.
    ///
    /// The file is opened in append mode (created if absent).  Parent
    /// directories are **not** automatically created.
    pub fn with_file(path: &str) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            console: true,
            file: Some(Arc::new(Mutex::new(file))),
        })
    }

    /// Create a writer that outputs to the given file path only (no stderr).
    pub fn file_only(path: &str) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            console: false,
            file: Some(Arc::new(Mutex::new(file))),
        })
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DualMakeWriter {
    type Writer = DualWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DualWriter {
            console: self.console,
            file: self.file.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
