//! Service state detection via `sc.exe query` (L2.0 scaffolding — direct SCM
//! API via the `windows` crate is a cleanup option).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    /// Installed but not running (stopped / paused / start-pending).
    Stopped,
    /// Service not installed.
    NotFound,
}

/// Query a Windows service's state by name (e.g. "SbieSvc", "SbieDrv").
pub fn service_state(name: &str) -> ServiceState {
    let out = match std::process::Command::new("sc").arg("query").arg(name).output() {
        Ok(o) => o,
        Err(_) => return ServiceState::NotFound,
    };
    // sc.exe exits 1060 when the service doesn't exist.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if combined.contains("1060")
        || combined.contains("does not exist")
        || combined.contains("not exist")
    {
        return ServiceState::NotFound;
    }
    if combined.contains("RUNNING") {
        return ServiceState::Running;
    }
    ServiceState::Stopped
}
