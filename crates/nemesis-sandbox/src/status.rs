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
    let out = match std::process::Command::new("sc")
        .arg("query")
        .arg(name)
        .output()
    {
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

/// Return a service's registered binary path (`BINARY_PATH_NAME` from
/// `sc qc <name>`), with surrounding quotes stripped. `None` if the service
/// doesn't exist or the line can't be parsed. Used to decide whether a running
/// service (e.g. SbieSvc) is OURS — i.e. its binary lives under our runtime dir
/// — vs a system / someone-else's Sandboxie, so we stop only our own on exit.
pub fn service_binary_path(name: &str) -> Option<String> {
    let out = std::process::Command::new("sc")
        .arg("qc")
        .arg(name)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(idx) = line.find("BINARY_PATH_NAME") {
            // e.g. `        BINARY_PATH_NAME   : "C:\...\SbieSvc.exe"`
            let after = &line[idx + "BINARY_PATH_NAME".len()..];
            let path = after.trim_start_matches([' ', ':', '\t']).trim();
            return Some(path.trim_matches('"').to_string());
        }
    }
    None
}

/// True if no FOREIGN Sandboxie is registered — i.e. every registered
/// `SbieDrv` / `SbieSvc` either isn't registered (name free) or has its binary
/// under our runtime dir. **False if any registered one is NOT ours** (a system
/// / someone-else's Sandboxie owns the name → we must not touch it, and can't
/// install ours over the name conflict). Use this as the ownership gate before
/// touching (stop/start/reuse/install) the engine.
pub fn engine_owned(paths: &crate::SandboxPaths) -> bool {
    let runtime = paths.runtime_dir.to_string_lossy().to_lowercase();
    for name in [crate::DRIVER_SERVICE, crate::USERMODE_SERVICE] {
        if matches!(service_state(name), ServiceState::NotFound) {
            continue; // not registered — name free, fine
        }
        // registered — must be ours (binary under our runtime dir)
        let ours = service_binary_path(name)
            .map(|b| b.to_lowercase().contains(&runtime))
            .unwrap_or(false);
        if !ours {
            return false; // registered but not ours → foreign Sandboxie
        }
    }
    true
}
