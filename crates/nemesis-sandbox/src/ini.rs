//! Write Sandboxie.ini with the NemesisBox definition.
//!
//! Directives verified against the source research (2026-07-09):
//! - `Enabled=y` is REQUIRED (`core/drv/conf_user.c:381` — Start.exe gates on
//!   `SbieApi_IsBoxEnabled`).
//! - `AllowNetworkAccess=n` blocks egress (the `BlockNetAccess` directive does
//!   NOT exist — `core/drv/wfp.c:701`).
//! - `DropAdminRights=y` strips admin SIDs (`core/drv/token.c:558`).
//! - `OpenPipePath=\Device\NamedPipe\<box>_*` opens our IPC pipe
//!   (`core/drv/file.c`; examples throughout `install/Templates.ini`).
//! - `ForceProcess`/`ForceFolder` MUST NOT be set (would auto-box the gateway).
//!
//! The ini is written to `SandboxPaths::ini_path` (redirected via the SbieDrv
//! IniPath registry value), never `C:\Windows\Sandboxie.ini`.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};

// ---------------------------------------------------------------------------
// P3 ②：per-agent 常驻盒（`NemesisAgent_<agent_id>`）
// ---------------------------------------------------------------------------

/// per-agent 常驻盒名前缀（P3 T5）。盒名 / 管道名 / ini `OpenPipePath` 模式
/// 三处共用此前缀；nemesis-agent 的 `executor_pipe::agent_pipe_name` 以字面
/// 量对齐（无跨 crate 依赖，两测各钉一处防漂移）。
pub const AGENT_BOX_PREFIX: &str = "NemesisAgent_";

/// 盒名 / 管道名安全化：只保留字母数字 `_` `-`，其余替换 `_`（agent_id
/// 生产形态是 uuid simple hex，安全化是对手工 id 的防御，不是纠偏）。
fn sanitize_box_suffix(agent_id: &str) -> String {
    agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// per-agent 常驻盒名（ini section 名 = 盒名）。
pub fn agent_box_name(agent_id: &str) -> String {
    format!("{AGENT_BOX_PREFIX}{}", sanitize_box_suffix(agent_id))
}

/// per-agent 管道名的 ini 侧 NT 形态模式（`OpenPipePath` 值）。Win32 形态
/// 是 `\\.\pipe\` + 本模式去掉 `\Device\NamedPipe\` 前缀（spawn 侧
/// `executor_pipe::agent_pipe_name` 拼唯一 id 进尾段）。
pub fn agent_pipe_nt_pattern(agent_id: &str) -> String {
    format!(
        r"\Device\NamedPipe\{AGENT_BOX_PREFIX}{}_*",
        sanitize_box_suffix(agent_id)
    )
}

/// per-agent 盒段指令集（顺序即写入顺序）。写穿透两行：`.runtime\*`
/// （journal——crash 恢复 / 幂等重发的记账必须在真盘）+ `browser-profile\*`
/// （浏览器登录态——跨任务 / 跨重启保持的前提）。其余工作区写入保持
/// 虚拟化（盒内容，验收后 commit 回真盘）。
pub fn agent_box_directives(
    agent_id: &str,
    box_root: &Path,
    agent_home: &Path,
    allow_network: bool,
) -> Vec<(String, String)> {
    let box_root_nt = format!(r"\??\{}", box_root.display());
    let net = if allow_network { "y" } else { "n" };
    let home = agent_home.display().to_string();
    vec![
        ("Enabled".into(), "y".into()),
        ("AllowNetworkAccess".into(), net.into()),
        ("DropAdminRights".into(), "y".into()),
        ("OpenPipePath".into(), agent_pipe_nt_pattern(agent_id)),
        ("FileRootPath".into(), box_root_nt),
        ("OpenFilePath".into(), format!(r"{home}\.runtime\*")),
        ("OpenFilePath".into(), format!(r"{home}\browser-profile\*")),
    ]
}

/// 全局 ini 写锁：`write_sandboxie_ini`（整文件重写）与
/// `upsert_box_section`（读改写）串行——install/start 与 agent spawn 并发
/// 改同一文件时保序（进程内；跨进程由 Sandboxie 自身 ini 序列化兜底）。
static INI_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 在现有 ini 中幂等 upsert 一个盒 section，**逐字保留其余 section**（与
/// `write_sandboxie_ini` 的整文件重写不同——那是无 agent 盒时代的单盒形态，
/// 会抹掉其他盒段）。文件缺失 = 从零创建。同节已存在 = 整节替换为新指令。
pub fn upsert_box_section(
    ini_path: &Path,
    box_name: &str,
    directives: &[(String, String)],
) -> Result<()> {
    let _guard = INI_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(parent) = ini_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create ini dir {}", parent.display()))?;
    }
    let existing = std::fs::read_to_string(ini_path).unwrap_or_default();

    // 切分：首节前的行 = preamble；之后每个 `[name]` 起一节，节内容 = 原
    // 始行（不 trim 不归一，逐字保留）。
    let mut preamble: Vec<&str> = Vec::new();
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let name = trimmed[1..trimmed.len() - 1].to_string();
            sections.push((name, Vec::new()));
        } else if sections.is_empty() {
            preamble.push(line);
        } else {
            sections.last_mut().expect("non-empty above").1.push(line);
        }
    }

    // 摘除同节（大小写不敏感——Sandboxie 盒名语义），再追加新节。
    let target = box_name.to_ascii_lowercase();
    sections.retain(|(name, _)| name.to_ascii_lowercase() != target);
    let mut out = String::new();
    for line in preamble {
        out.push_str(line);
        out.push('\n');
    }
    for (name, body) in &sections {
        out.push_str(&format!("[{name}]\n"));
        for line in body {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!("[{box_name}]\n"));
    for (k, v) in directives {
        out.push_str(&format!("{k}={v}\n"));
    }
    std::fs::write(ini_path, out).with_context(|| format!("write {}", ini_path.display()))?;
    tracing::info!(
        "[sandbox] upserted box section [{box_name}] into {} ({} directives)",
        ini_path.display(),
        directives.len()
    );
    Ok(())
}

/// upsert per-agent 常驻盒段（指令集见 [`agent_box_directives`]）。
pub fn upsert_agent_box_section(
    ini_path: &Path,
    agent_id: &str,
    box_root: &Path,
    agent_home: &Path,
    allow_network: bool,
) -> Result<()> {
    let box_name = agent_box_name(agent_id);
    let directives = agent_box_directives(agent_id, box_root, agent_home, allow_network);
    upsert_box_section(ini_path, &box_name, &directives)
}

/// Write the NemesisBox definition to `ini_path`.
pub fn write_sandboxie_ini(
    ini_path: &Path,
    box_name: &str,
    box_root: &Path,
    allow_network: bool,
) -> Result<()> {
    // 与 upsert_box_section 共用写锁（install/start 的整文件重写和 agent
    // spawn 的节级 upsert 串行——整文件重写不感知 agent 盒段，锁内排序只
    // 保证不交错损坏；start 后的首次 agent spawn 会重新 upsert 自愈）。
    let _guard = INI_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(parent) = ini_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create ini dir {}", parent.display()))?;
    }
    let box_root_nt = format!(r"\??\{}", box_root.display());
    // `AllowNetworkAccess` is the box-level WFP network switch (core/drv/wfp.c).
    // y = box processes may use the network; n = egress blocked for the whole box.
    let net = if allow_network { "y" } else { "n" };
    let content = format!(
        "[GlobalSettings]\n\
         \n\
         # Prevent SbieCtrl (the Classic GUI) from auto-launching when a sandboxed\n\
         # program starts — without this, SbieSvc auto-starts SbieCtrl.exe on every\n\
         # boxed spawn (sbieiniserver.cpp:1543 SbieCtrl_EnableAutoStart), popping its\n\
         # window + message dialogs. Headless use must keep this at n.\n\
         [UserSettings_Default]\n\
         SbieCtrl_EnableAutoStart=n\n\
         \n\
         [{box_name}]\n\
         Enabled=y\n\
         AllowNetworkAccess={net}\n\
         DropAdminRights=y\n\
         OpenPipePath=\\Device\\NamedPipe\\{box_name}_*\n\
         FileRootPath={box_root_nt}\n"
    );
    std::fs::write(ini_path, content).with_context(|| format!("write {}", ini_path.display()))?;
    tracing::info!(
        "[sandbox] wrote {} (box={box_name}, root={})",
        ini_path.display(),
        box_root.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cov_tests;

// Wave6B 覆盖率收尾：见 cov_wave6b_tests.rs 头注释。
#[cfg(test)]
mod cov_wave6b_tests;
