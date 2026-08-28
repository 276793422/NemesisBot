//! Userland sandbox self-test probes (G7 / U-sandbox UI D2).
//!
//! One-shot probe runner executed inside a dedicated child process
//! (`nemesisbot sandbox selftest-child`): the parent (gateway WSAPI
//! `sandbox.self_test`) either wraps this child with a WrapCommand backend
//! (bwrap / Seatbelt) or lets the child apply a SelfApply backend (landlock)
//! to itself — landlock rules are irreversible, so the gateway process must
//! NEVER apply them in-process (a probe would permanently shrink the
//! gateway). The child runs the probes below and prints a single-line JSON
//! verdict to stdout.
//!
//! Probes (honest evidence, not verdicts):
//! 1. workspace-outside write (temp dir) — expected blocked under any
//!    correctly-engaged backend; expected allowed when unsandboxed.
//! 2. outbound TCP connect (1.1.1.1:80) — blocked under bwrap
//!    `--unshare-net` / Seatbelt deny-network; NOT covered by landlock
//!    (allowed + evidence says so — reporting the gap is the point).
//! 3. workspace-inside write (control) — must always succeed; a block here
//!    means the sandbox is mis-engaged (over-restrictive).

use std::io::Write as _;
use std::net::Ipv4Addr;
use std::path::Path;

/// Single probe verdict. `blocked = true` means the operation FAILED
/// (denied) — that is the *desired* outcome for isolation checks.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeCheck {
    pub name: String,
    pub blocked: bool,
    pub evidence: String,
}

/// The child process' single-line stdout payload.
#[derive(Debug, serde::Serialize)]
pub struct SelftestChildOut {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub checks: Vec<ProbeCheck>,
}

/// Run all probes against `workspace`. Blocking (network probe bounded by a
/// 3s connect timeout); the child process exits right after, so blocking the
/// caller is fine.
pub fn run_probes(workspace: &Path) -> Vec<ProbeCheck> {
    vec![
        probe_outside_write(workspace),
        probe_network(),
        probe_workspace_write(workspace),
    ]
}

/// Probe 1: write into the system temp dir (outside the workspace subtree).
fn probe_outside_write(workspace: &Path) -> ProbeCheck {
    let name = "workspace 外写入（系统临时目录）";
    let path = std::env::temp_dir().join(format!("nemesis_selftest_probe_{}.txt", std::process::id()));
    if path.starts_with(workspace) {
        // Degenerate layout (workspace contains temp dir): the probe would be
        // meaningless — report honestly instead of guessing.
        return ProbeCheck {
            name: name.to_string(),
            blocked: false,
            evidence: format!("跳过：临时目录 {} 在 workspace 内，探测无意义", path.display()),
        };
    }
    match std::fs::write(&path, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&path);
            ProbeCheck {
                name: name.to_string(),
                blocked: false,
                evidence: format!("写入成功：{}（未沙盒 / 隔离未生效）", path.display()),
            }
        }
        Err(e) => ProbeCheck {
            name: name.to_string(),
            blocked: true,
            evidence: format!("写入被拒：{e}"),
        },
    }
}

/// Probe 2: outbound TCP connect to 1.1.1.1:80 (IP literal — no DNS on the
/// probe path). Bounded by a 3s connect timeout.
///
/// 只有 **PermissionDenied** 才是隔离生效的证据；超时/不可达/拒连也可能是
/// 本机无外网或上游防火墙 —— 机器可读的 blocked 不得据此为真（诚实探针：
/// 宁可漏报也不假报绿）。原始错误始终进 evidence。
fn probe_network() -> ProbeCheck {
    let name = "网络出站（TCP 1.1.1.1:80）";
    let addr = std::net::SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 80));
    let timeout = std::time::Duration::from_secs(3);
    match std::net::TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => ProbeCheck {
            name: name.to_string(),
            blocked: false,
            // landlock 不覆盖网络 —— 连通不代表「无沙盒」，如实展示能力缺口。
            evidence: "连接成功（允许出站；注意：landlock 不覆盖网络，bwrap --unshare-net / Seatbelt deny network 才会拦截）".to_string(),
        },
        Err(e)
            if e.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            ProbeCheck {
                name: name.to_string(),
                blocked: true,
                evidence: format!("连接被系统拒绝（PermissionDenied）：{e}"),
            }
        }
        Err(e) => ProbeCheck {
            name: name.to_string(),
            blocked: false,
            evidence: format!(
                "连接失败但非权限拒绝（可能是本机无外网/上游拦截，无法据此判定隔离生效）：{e}"
            ),
        },
    }
}

/// Probe 3 (control): write INSIDE the workspace — must succeed.
fn probe_workspace_write(workspace: &Path) -> ProbeCheck {
    let name = "workspace 内写入（对照组）";
    let path = workspace.join(format!(".selftest_probe_{}.tmp", std::process::id()));
    match std::fs::write(&path, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&path);
            ProbeCheck {
                name: name.to_string(),
                blocked: false,
                evidence: "写入成功（工作区可写 — 符合预期）".to_string(),
            }
        }
        Err(e) => ProbeCheck {
            name: name.to_string(),
            // blocked here = the sandbox over-restricts (workspace is SUPPOSED
            // to be writable) — surface it loudly via blocked=true.
            blocked: true,
            evidence: format!("写入失败（异常：工作区应可写）：{e}"),
        },
    }
}

/// Print `out` as a single stdout line (the parent parses this).
pub fn emit(out: &SelftestChildOut) {
    let json = serde_json::to_string(out)
        .unwrap_or_else(|_| r#"{"ok":false,"error":"selftest serialize failed","checks":[]}"#.to_string());
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{json}");
    let _ = stdout.flush();
}

#[cfg(test)]
mod selftest_tests;
