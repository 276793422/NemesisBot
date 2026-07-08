//! Named-pipe transport for sandboxed executor children.
//!
//! `Start.exe` does NOT forward the parent's stdio to the boxed child (see
//! `docs/PLAN/2026-07-09_sandboxie-integration.md` §发现1), so when the
//! executor runs sandboxed we switch from stdio to a Windows named pipe: the
//! gateway creates `\\.\pipe\NemesisBox_<id>`, the child connects, and they
//! exchange the same newline-delimited JSON protocol as the stdio path.
//!
//! L2.1: the pipe works WITHOUT the box too — when the gateway's `start_exe`
//! is unset it spawns the child directly (no Start.exe), so the transport can
//! be validated independently of Sandboxie containment (L2.2 wraps the spawn
//! with Start.exe for the real box).
//!
//! Windows-only at runtime. The cross-platform helpers (`pipe_name`,
//! `unique_pipe_id`) compile everywhere; the server/client fns are `cfg(windows)`.
//!
//! (Uses `std::io::Result` rather than `anyhow` — nemesis-agent does not depend
//! on anyhow; callers map the io error to their own error type.)

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Build the Win32 pipe path for a unique id (`\\.\pipe\NemesisBox_<id>`).
pub fn pipe_name(id: &str) -> String {
    format!(r"\\.\pipe\NemesisBox_{id}")
}

static PIPE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Process-unique pipe id (`<pid>_<counter>`).
pub fn unique_pipe_id() -> String {
    let n = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}", std::process::id(), n)
}

// ---------------------------------------------------------------------------
// Windows named-pipe server/client
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

/// Gateway side: create (but do not yet connect) a named pipe server. Call
/// `server.connect().await` after spawning the child to wait for it to connect.
#[cfg(windows)]
pub fn create_server(name: &str) -> io::Result<NamedPipeServer> {
    ServerOptions::new().first_pipe_instance(true).create(name)
}

/// Child side: connect to a named pipe, retrying briefly (the gateway may not
/// have created the pipe yet at the moment the child starts).
#[cfg(windows)]
pub async fn connect_client(name: &str) -> io::Result<NamedPipeClient> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match ClientOptions::new().open(name) {
            Ok(c) => return Ok(c),
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
                // ERROR_FILE_NOT_FOUND (2) / ERROR_PIPE_BUSY (231) — retry.
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
}
