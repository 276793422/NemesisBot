//! 运行实例的路径级识别与强制停止编排（2026-11-10 用户拍板「卸载确认后
//! 强制停止正在运行的 bot」的平台中立核心）。
//!
//! 分工：本模块只做**纯流程**（目录子树过滤 + 停止→复核有限重试编排）；
//! 平台侧提供生产 query/kill 实现——windows = Toolhelp 进程快照 +
//! QueryFullProcessImageNameW 取镜像路径 + `taskkill /F /T`；linux =
//! `/proc/<pid>/exe` readlink 扫描 + `libc::kill(SIGKILL)`。
//!
//! 纪律：
//! - **路径级 scope**：只停可执行镜像落在目标安装目录子树内的实例——
//!   卸载目录 X 绝不误杀其他安装/开发 checkout 的 bot（同名进程多部署
//!   是常态；全局名匹配会殃及无辜）；
//! - **复核权威**：kill 调用的退出状态不裁决（批量 taskkill 在「已杀与
//!   已退出混批」下恒非零）——权威判定 = 复核重查目录内是否清零，有限
//!   重试容忍句柄释放的瞬时窗口（TerminateProcess 返回 ≠ 名字立即可
//!   重用），重试耗尽仍有存活 = [`BotsSurvive`]（退出码 6 域）；
//! - **路径不可读诚实边界**：无权限读镜像路径的进程（他用户/提权）路径
//!   为空串——不归属任何目录，既不误杀也不拦卸载（其文件锁面由韧性删除
//!   的 failures 清单如实兜底）。

use std::path::Path;
use std::time::Duration;

use anyhow::Result;

/// 一个运行中的 nemesisbot 进程（PID + 可执行镜像路径；路径不可读 = 空串）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotProcess {
    pub pid: u32,
    pub exe_path: String,
}

impl BotProcess {
    /// 人读描述（错误信息用）。
    pub fn describe(&self) -> String {
        if self.exe_path.is_empty() {
            format!("PID {}（镜像路径不可读）", self.pid)
        } else {
            format!("PID {}（{}）", self.pid, self.exe_path)
        }
    }
}

/// 强制停止后目录内仍有存活（退出码 6 域——两平台 exit_code_of 单点映射）。
#[derive(Debug)]
pub struct BotsSurvive(pub Vec<BotProcess>);

impl std::fmt::Display for BotsSurvive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = self
            .0
            .iter()
            .map(BotProcess::describe)
            .collect::<Vec<_>>()
            .join("\n");
        write!(
            f,
            "强制停止后仍有 {} 个 nemesisbot 进程存活:\n{}",
            self.0.len(),
            list
        )
    }
}
impl std::error::Error for BotsSurvive {}

/// 目录子树过滤（纯函数）：镜像路径在 `dir` 内（前缀边界 = 分隔符，
/// `C:\foo` 不匹配 `C:\foobar`）。Windows 大小写不敏感 + 尾分隔符剪除；
/// Unix 保留大小写。路径不可读（空串）= 不归属任何目录（诚实边界）。
pub fn bots_under_dir(bots: &[BotProcess], dir: &Path) -> Vec<BotProcess> {
    let d = normalize_scope(dir.to_string_lossy().as_ref());
    bots.iter()
        .filter(|b| {
            let p = normalize_scope(&b.exe_path);
            !p.is_empty()
                && p.len() >= d.len()
                && p.starts_with(&d)
                && (p.len() == d.len() || is_sep(p.as_bytes()[d.len()]))
        })
        .cloned()
        .collect()
}

fn is_sep(byte: u8) -> bool {
    byte == b'\\' || byte == b'/'
}

fn normalize_scope(s: &str) -> String {
    let mut t = s;
    while t.ends_with('\\') || t.ends_with('/') {
        t = &t[..t.len() - 1];
    }
    if cfg!(windows) {
        t.to_ascii_lowercase()
    } else {
        t.to_owned()
    }
}

/// 进程查询注入点（生产 = 平台侧 `running_bot_processes`；测试喂假清单）。
pub type ProcessQuery<'a> = &'a dyn Fn() -> Result<Vec<BotProcess>>;

/// 进程终止注入点（生产 = 平台侧 `kill_bot_pids`；测试记录 PID/恒 no-op）。
pub type ProcessKill<'a> = &'a dyn Fn(&[u32]) -> Result<()>;

/// 停止编排（纯流程，注入 query/kill）：查询 → 目录过滤 → 终止 → 复核
/// 重查（有限重试）。返回被停止的进程清单（空 = 目录内本就没有实例——
/// kill 不调用）；复核仍有存活 = `Err(BotsSurvive)`。
pub fn stop_bots_in_dir(
    dir: &Path,
    query: ProcessQuery<'_>,
    kill: ProcessKill<'_>,
    attempts: u32,
    retry_delay: Duration,
) -> Result<Vec<BotProcess>> {
    let found = query()?;
    let targets = bots_under_dir(&found, dir);
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let pids: Vec<u32> = targets.iter().map(|b| b.pid).collect();
    kill(&pids)?;
    let rounds = attempts.max(1);
    let mut survivors = bots_under_dir(&query()?, dir);
    let mut round = 1;
    while !survivors.is_empty() && round < rounds {
        std::thread::sleep(retry_delay);
        survivors = bots_under_dir(&query()?, dir);
        round += 1;
    }
    if survivors.is_empty() {
        Ok(targets)
    } else {
        Err(anyhow::Error::new(BotsSurvive(survivors)))
    }
}
