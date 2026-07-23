//! Install / uninstall orchestration.
//!
//! Install sequence mirrors `SandboxieVS.nsi`:
//!   download → 7z extract → KmdUtil install SbieDrv → set IniPath →
//!   KmdUtil install SbieSvc → set SbieSvc service-key DWORDs →
//!   KmdUtil start SbieSvc → write Sandboxie.ini
//!
//! Uninstall is the reverse, tolerant of already-absent pieces.
//!
//! All steps here assume the caller is already elevated (run via the CLI's
//! `--internal` elevation path; this function does NOT self-elevate).

use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::kmdutil;
use crate::status::{ServiceState, service_state};
use crate::{DRIVER_SERVICE, SandboxPaths, USERMODE_SERVICE, download, extract, ini};

/// Poll `service_state(name)` until it reaches `target` or `timeout` elapses.
pub fn wait_for_state(name: &str, target: ServiceState, timeout: Duration) -> ServiceState {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let s = service_state(name);
        if s == target || std::time::Instant::now() >= deadline {
            return s;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Poll `service_state(name)` until the service EXISTS (state != NotFound) or
/// `timeout` elapses. Used by the install flow to confirm the service got
/// created (install no longer starts it — start is the user's explicit step).
pub fn wait_for_installed(name: &str, timeout: Duration) -> ServiceState {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let s = service_state(name);
        if s != ServiceState::NotFound || std::time::Instant::now() >= deadline {
            return s;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Acquire the Sandboxie runtime files: download the official installer +
/// extract it into `runtime/`. Does NOT install the driver/service or write ini
/// (that's [`start`]) and needs no elevation — just file I/O.
pub async fn install(paths: &SandboxPaths) -> Result<()> {
    let installer = download::download_release(
        crate::INSTALLER_URL,
        crate::CHECKSUMS_URL,
        crate::INSTALLER_FILENAME,
        &paths.runtime_dir,
    )
    .await
    .context("download release")?;

    // Resolve 7z (local-first: cached → system → download 7z.zip) + extract.
    extract::extract_release(&installer, &paths.runtime_dir)
        .await
        .context("extract")?;
    paths.verify_runtime().context("verify runtime files")?;

    tracing::info!(
        "[sandbox] files acquired at {} — run `sandbox start` to activate the engine",
        paths.runtime_dir.display()
    );
    Ok(())
}

/// Activate the Sandboxie engine: install the driver + service, redirect IniPath,
/// write Sandboxie.ini, and start SbieSvc. Requires the files acquired first
/// ([`install`]). Must be called elevated (kernel driver install → UAC).
pub fn start(paths: &SandboxPaths) -> Result<()> {
    paths
        .verify_runtime()
        .context("runtime files missing — run `nemesisbot sandbox install` first")?;

    kmdutil::run(
        kmdutil::install_driver(&paths.kmdutil(), &paths.sbiedrv_sys(), &paths.sbiemsg_dll()),
        false,
    )
    .context("install SbieDrv")?;
    kmdutil::set_ini_path(&paths.ini_path).context("set IniPath")?;
    kmdutil::run(
        kmdutil::install_service(&paths.kmdutil(), &paths.sbiesvc_exe(), &paths.sbiemsg_dll()),
        false,
    )
    .context("install SbieSvc")?;
    kmdutil::set_sbiesvc_service_key_dwounds().context("set SbieSvc service-key DWORDs")?;

    ini::write_sandboxie_ini(&paths.ini_path, crate::DEFAULT_BOX_NAME, &paths.box_root)
        .context("write Sandboxie.ini")?;

    kmdutil::run(kmdutil::start(&paths.kmdutil(), USERMODE_SERVICE), false)
        .context("start SbieSvc")?;
    let s = wait_for_state(
        USERMODE_SERVICE,
        ServiceState::Running,
        Duration::from_secs(15),
    );
    if s != ServiceState::Running {
        bail!("SbieSvc did not reach RUNNING (state={s:?})");
    }
    tracing::info!("[sandbox] engine activated — SbieSvc RUNNING");
    Ok(())
}

/// Deactivate the Sandboxie engine: stop + uninstall the driver and service.
/// The acquired files stay (so `start` can re-activate without re-downloading),
/// unless `purge` is set — then the runtime files, box, and ini are removed too.
/// Must be called elevated.
pub fn stop(paths: &SandboxPaths, purge: bool) -> Result<()> {
    let _ = kmdutil::run(kmdutil::stop(&paths.kmdutil(), USERMODE_SERVICE), true);
    let _ = kmdutil::run(kmdutil::stop(&paths.kmdutil(), DRIVER_SERVICE), true);
    let _ = kmdutil::run(kmdutil::delete(&paths.kmdutil(), USERMODE_SERVICE), true);
    let _ = kmdutil::run(kmdutil::delete(&paths.kmdutil(), DRIVER_SERVICE), true);

    let svc = service_state(USERMODE_SERVICE);
    let drv = service_state(DRIVER_SERVICE);
    tracing::info!("[sandbox] engine deactivated (SbieSvc={svc:?}, SbieDrv={drv:?})");

    if purge {
        let _ = std::fs::remove_dir_all(&paths.runtime_dir);
        let _ = std::fs::remove_dir_all(&paths.box_root);
        let _ = std::fs::remove_file(&paths.ini_path);
        tracing::info!(
            "[sandbox] --purge: removed runtime {}, box {}, ini {}",
            paths.runtime_dir.display(),
            paths.box_root.display(),
            paths.ini_path.display()
        );
    }
    Ok(())
}

/// Start ONLY the user-mode SbieSvc service — a runtime op that does NOT touch
/// the kernel driver and does NOT re-register the service. Used by
/// `ensure_sandbox_ready` at gateway startup when the driver is already
/// installed but SbieSvc is stopped (e.g. after a crash or an external
/// `sc stop`). Driver and service are independent: bringing the service up must
/// not re-trigger the driver-install UAC. Whether a non-elevated caller can
/// start the service depends on the SbieSvc service ACL — `kmdutil::start`
/// opens the existing service via the SCM (no `SC_MANAGER_CREATE_SERVICE`),
/// unlike the install verbs. Must NOT be used to (re)install the engine.
pub fn start_service(paths: &SandboxPaths) -> Result<()> {
    if matches!(service_state(USERMODE_SERVICE), ServiceState::Running) {
        return Ok(()); // already up (residual) — reuse, nothing to do
    }
    kmdutil::run(kmdutil::start(&paths.kmdutil(), USERMODE_SERVICE), false)
        .context("start SbieSvc")?;
    let s = wait_for_state(
        USERMODE_SERVICE,
        ServiceState::Running,
        Duration::from_secs(15),
    );
    if s != ServiceState::Running {
        bail!("SbieSvc did not reach RUNNING (state={s:?})");
    }
    tracing::info!("[sandbox] SbieSvc started (driver untouched)");
    Ok(())
}

/// Stop ONLY the user-mode SbieSvc service — a runtime op; the kernel driver
/// stays loaded and the service stays registered (reversible by
/// [`start_service`]). Used on gateway exit to stop OUR SbieSvc per-run. The
/// driver is deliberately left resident (uninstalling it needs UAC and is
/// BSOD-prone; see Route A). No UAC — `kmdutil::stop` opens the existing
/// service via the SCM, no `SC_MANAGER_CREATE_SERVICE`.
pub fn stop_service(paths: &SandboxPaths) -> Result<()> {
    let _ = kmdutil::run(kmdutil::stop(&paths.kmdutil(), USERMODE_SERVICE), true);
    tracing::info!("[sandbox] SbieSvc stop issued (driver untouched)");
    Ok(())
}

/// Tolerant "make engine installed + SbieSvc running" — the elevated worker
/// behind `ensure_sandbox_ready` (startup decision rows 3–6). Unlike [`start`],
/// this is tolerant of already-registered components, so it recovers ANY partial
/// install state to fully-installed + running:
///   - stops the service first (row 5: service running without a registered
///     driver → register driver, then restart service so it binds the driver);
///   - registers driver + service tolerant ("already exists" → no-op);
///   - (re)writes IniPath, service-key DWORDs, Sandboxie.ini;
///   - starts the service.
/// Must be called elevated (kernel driver install → UAC).
pub fn ensure_installed(paths: &SandboxPaths) -> Result<()> {
    paths
        .verify_runtime()
        .context("runtime files missing — run `nemesisbot sandbox install` first")?;

    // 归属门：不在别人的 Sandboxie 上动（名字冲突也装不了自己的）。
    if !crate::status::engine_owned(paths) {
        bail!(
            "foreign Sandboxie registered (SbieDrv/SbieSvc not under {}) — \
             refusing to install over it",
            paths.runtime_dir.display()
        );
    }

    // Row 5 safety: stop a running service before (re)registering the driver.
    let _ = kmdutil::run(kmdutil::stop(&paths.kmdutil(), USERMODE_SERVICE), true);

    // Register driver + service tolerant (ignore "already exists"); IniPath and
    // service-key DWORDs are idempotent registry writes.
    let _ = kmdutil::run(
        kmdutil::install_driver(&paths.kmdutil(), &paths.sbiedrv_sys(), &paths.sbiemsg_dll()),
        true,
    );
    let _ = kmdutil::set_ini_path(&paths.ini_path);
    let _ = kmdutil::run(
        kmdutil::install_service(&paths.kmdutil(), &paths.sbiesvc_exe(), &paths.sbiemsg_dll()),
        true,
    );
    let _ = kmdutil::set_sbiesvc_service_key_dwounds();

    ini::write_sandboxie_ini(&paths.ini_path, crate::DEFAULT_BOX_NAME, &paths.box_root)?;

    // Start the service (no-op if already running) + wait for RUNNING.
    start_service(paths)?;
    tracing::info!("[sandbox] engine ensured installed + SbieSvc running (tolerant)");
    Ok(())
}
