//! 系统浏览器枚举（设置子页签选择框数据源，计划 D14）。
//!
//! 探测序（每 kind 三路，任一命中即可用）：
//! 1. Windows：注册表 App Paths（HKLM/HKCU `...\App Paths\<exe>` 的默认值）；
//! 2. 已知安装路径（Program Files 家族 + LocalAppData）；
//! 3. PATH（`which` crate）。
//!    Linux/macOS：PATH + 已知路径。
//!
//! Firefox v1 **枚举可见不可驱动**（CDP 已死于 Firefox 129/141，BiDi 二期
//! P4 落地后才转可用）——`phase2: true`，选中即诚实报错。

use serde::Serialize;
use std::path::PathBuf;

/// 支持枚举的浏览器 kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DriverKind {
    Edge,
    Chrome,
    Brave,
    Vivaldi,
    Chromium,
    Firefox,
    /// 显式自定义可执行文件路径（kind=custom）；枚举列表不出现，仅 spawn 语义。
    Custom,
}

impl DriverKind {
    /// slug（config `browser.kind` 值域 + profile 目录名）。
    pub fn slug(&self) -> &'static str {
        match self {
            Self::Edge => "edge",
            Self::Chrome => "chrome",
            Self::Brave => "brave",
            Self::Vivaldi => "vivaldi",
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
            Self::Custom => "custom",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Some(match slug.trim().to_ascii_lowercase().as_str() {
            "edge" => Self::Edge,
            "chrome" => Self::Chrome,
            "brave" => Self::Brave,
            "vivaldi" => Self::Vivaldi,
            "chromium" => Self::Chromium,
            "firefox" => Self::Firefox,
            "custom" => Self::Custom,
            _ => return None,
        })
    }

    /// 展示名。
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Edge => "Microsoft Edge",
            Self::Chrome => "Google Chrome",
            Self::Brave => "Brave",
            Self::Vivaldi => "Vivaldi",
            Self::Chromium => "Chromium",
            Self::Firefox => "Firefox",
            Self::Custom => "自定义",
        }
    }

    /// v1 不可驱动（Firefox = 二期，P4 FirefoxBiDiDriver 落地后转 false）。
    pub fn is_phase2(&self) -> bool {
        matches!(self, Self::Firefox)
    }

    /// Windows 可执行文件名（注册表 App Paths 键 + PATH 探测用）。
    fn exe_name(&self) -> &'static str {
        match self {
            Self::Edge => "msedge.exe",
            Self::Chrome => "chrome.exe",
            Self::Brave => "brave.exe",
            Self::Vivaldi => "vivaldi.exe",
            Self::Chromium => "chromium.exe",
            Self::Firefox => "firefox.exe",
            Self::Custom => "",
        }
    }

    /// Linux 可执行文件名（PATH 探测用）。
    fn unix_exe_names(&self) -> &'static [&'static str] {
        match self {
            Self::Edge => &["microsoft-edge", "microsoft-edge-stable", "msedge"],
            Self::Chrome => &["google-chrome", "google-chrome-stable"],
            Self::Brave => &["brave-browser", "brave"],
            Self::Vivaldi => &["vivaldi", "vivaldi-stable"],
            Self::Chromium => &["chromium", "chromium-browser"],
            Self::Firefox => &["firefox"],
            Self::Custom => &[],
        }
    }
}

/// 枚举结果行。
#[derive(Debug, Clone, Serialize)]
pub struct DetectedBrowser {
    pub kind: DriverKind,
    /// 命中的可执行文件路径；未检出 = None（UI 显示「未检测到」）。
    pub path: Option<PathBuf>,
    /// true = 二期（枚举可见、spawn 诚实报错）。
    pub phase2: bool,
    /// 探测来源（app_paths / known_path / path），诊断用。
    pub source: String,
}

impl DetectedBrowser {
    fn detected(kind: DriverKind, path: PathBuf, source: &str) -> Self {
        Self {
            kind,
            path: Some(path),
            phase2: kind.is_phase2(),
            source: source.to_string(),
        }
    }

    fn missing(kind: DriverKind) -> Self {
        Self {
            kind,
            path: None,
            phase2: kind.is_phase2(),
            source: String::new(),
        }
    }
}

/// 枚举系统已装浏览器（Edge/Chrome/Brave/Vivaldi/Chromium/Firefox，固定序）。
pub fn detect_installed() -> Vec<DetectedBrowser> {
    let kinds = [
        DriverKind::Edge,
        DriverKind::Chrome,
        DriverKind::Brave,
        DriverKind::Vivaldi,
        DriverKind::Chromium,
        DriverKind::Firefox,
    ];
    kinds.iter().map(|&k| detect_one(k)).collect()
}

/// 探测单个 kind；返回 DetectedBrowser（检出或未检出）。
pub fn detect_one(kind: DriverKind) -> DetectedBrowser {
    let (p, src) = find_executable_with_source(kind);
    match p {
        Some(p) => DetectedBrowser::detected(kind, p, src),
        None => DetectedBrowser::missing(kind),
    }
}

/// 解析某 kind 的可执行文件路径（显式路径优先于一切枚举逻辑——见 manager
/// 解析序）。未检出 = None。
pub fn find_executable(kind: DriverKind) -> Option<PathBuf> {
    find_executable_with_source(kind).0
}

/// 带探测来源的解析（F24，2026-11-15 四审）：来源在命中点结构化记录，
/// 不再事后按路径内容猜——旧 `source_of` 用「路径含 app paths 字样」判
/// App Paths 命中，但注册表值是安装路径（不含该字样），恒误标
/// known_path/path（诊断字段撒谎）。返回 `(路径, 来源)`；未检出 = `(None, "")`。
pub fn find_executable_with_source(kind: DriverKind) -> (Option<PathBuf>, &'static str) {
    if cfg!(windows) {
        find_executable_windows_with_source(kind)
    } else {
        find_executable_unix_with_source(kind)
    }
}

fn find_executable_windows_with_source(kind: DriverKind) -> (Option<PathBuf>, &'static str) {
    // 1) 注册表 App Paths。
    if let Some(p) = windows_app_paths(kind) {
        return (Some(p), "app_paths");
    }
    // 2) 已知安装路径。
    for cand in windows_known_paths(kind) {
        if cand.is_file() {
            return (Some(cand), "known_path");
        }
    }
    // 3) PATH。
    if let Ok(p) = which::which(kind.exe_name()) {
        return (Some(p), "path");
    }
    (None, "")
}

/// HKLM/HKCU `Software\Microsoft\Windows\CurrentVersion\App Paths\<exe>`
/// 默认值（REG_SZ / REG_EXPAND_SZ）。注册表读失败 = 静默跳过（企业精简
/// 环境 / 权限受限场景，已知路径与 PATH 兜底）。
#[cfg(windows)]
fn windows_app_paths(kind: DriverKind) -> Option<PathBuf> {
    let exe = kind.exe_name();
    let sub = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
    for hive in [
        winreg::enums::HKEY_LOCAL_MACHINE,
        winreg::enums::HKEY_CURRENT_USER,
    ] {
        if let Ok(key) = winreg::RegKey::predef(hive).open_subkey(&sub)
            && let Ok(val) = key.get_value::<String, _>("")
            && !val.trim().is_empty()
        {
            let p = expand_env(&val);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn windows_app_paths(_kind: DriverKind) -> Option<PathBuf> {
    None
}

/// 展开 `%VAR%`（REG_EXPAND_SZ 形态；std 无内置，手写常见形态）。
/// 迭代上限 5（F23，2026-11-15 四审）：自引用环境变量（值含自身标记，
/// Windows REG_EXPAND_SZ 自引用残留形态真实存在）会让「展开→再含 %→再
/// 展开」永不收敛——spawn 路径无限循环挂死。上限到达即保留剩余原样。
#[cfg(windows)]
fn expand_env(s: &str) -> PathBuf {
    let mut out = s.to_string();
    for _ in 0..5 {
        let Some(start) = out.find('%') else {
            break;
        };
        let Some(end) = out[start + 1..].find('%').map(|i| i + start + 1) else {
            break;
        };
        let name = &out[start + 1..end];
        match std::env::var(name) {
            Ok(v) => out = format!("{}{}{}", &out[..start], v, &out[end + 1..]),
            Err(_) => break,
        }
    }
    PathBuf::from(out)
}

#[cfg(not(windows))]
fn expand_env(s: &str) -> PathBuf {
    PathBuf::from(s)
}

/// Windows 已知安装路径（64/32 位 Program Files + LocalAppData）。
#[cfg(windows)]
fn windows_known_paths(kind: DriverKind) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let exe = kind.exe_name();
    let mut push = |root: Option<std::path::PathBuf>, sub: &str| {
        if let Some(r) = root {
            out.push(r.join(sub).join(exe));
        }
    };
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let local_appdata = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    match kind {
        DriverKind::Edge => {
            push(program_files.clone(), r"Microsoft\Edge\Application");
            push(program_files_x86.clone(), r"Microsoft\Edge\Application");
        }
        DriverKind::Chrome => {
            push(program_files.clone(), r"Google\Chrome\Application");
            push(program_files_x86.clone(), r"Google\Chrome\Application");
            push(local_appdata.clone(), r"Google\Chrome\Application");
        }
        DriverKind::Brave => {
            push(
                program_files.clone(),
                r"BraveSoftware\Brave-Browser\Application",
            );
            push(
                program_files_x86.clone(),
                r"BraveSoftware\Brave-Browser\Application",
            );
            push(
                local_appdata.clone(),
                r"BraveSoftware\Brave-Browser\Application",
            );
        }
        DriverKind::Vivaldi => {
            push(program_files.clone(), r"Vivaldi\Application");
            push(program_files_x86.clone(), r"Vivaldi\Application");
            push(local_appdata.clone(), r"Vivaldi\Application");
        }
        DriverKind::Chromium => {
            push(local_appdata.clone(), r"Chromium\Application");
        }
        DriverKind::Firefox => {
            push(program_files.clone(), "Mozilla Firefox");
            push(program_files_x86.clone(), "Mozilla Firefox");
        }
        DriverKind::Custom => {}
    }
    if let Some(ad) = appdata {
        // firefox 装在 AppData（用户级安装形态）。
        if kind == DriverKind::Firefox {
            out.push(ad.join("Mozilla Firefox").join(exe));
        }
    }
    out
}

#[cfg(not(windows))]
fn windows_known_paths(_kind: DriverKind) -> Vec<PathBuf> {
    Vec::new()
}

fn find_executable_unix_with_source(kind: DriverKind) -> (Option<PathBuf>, &'static str) {
    // 1) PATH。
    for name in kind.unix_exe_names() {
        if let Ok(p) = which::which(name) {
            return (Some(p), "path");
        }
    }
    // 2) 已知路径（macOS app bundle / Linux /opt、/usr/local、snap 等）。
    // F25（2026-11-15 四审）：macOS 主流浏览器是 app bundle 形态（不进
    // PATH）——此前只给 Firefox 配了 bundle 路径，Chrome/Edge/Brave/
    // Vivaldi/Chromium 在 macOS 上只剩 PATH 探测 = 基本检不出。
    let known: &[&str] = match kind {
        DriverKind::Chrome => &[
            "/opt/google/chrome/chrome",
            "/usr/local/bin/chrome",
            "/snap/bin/chromium",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ],
        DriverKind::Edge => &[
            "/opt/microsoft/msedge/msedge",
            "/usr/local/bin/msedge",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ],
        DriverKind::Brave => &[
            "/opt/brave.com/brave/brave-browser",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ],
        DriverKind::Vivaldi => &[
            "/opt/vivaldi/vivaldi",
            "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
        ],
        DriverKind::Chromium => &[
            "/snap/bin/chromium",
            "/usr/local/bin/chromium",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ],
        DriverKind::Firefox => &[
            "/Applications/Firefox.app/Contents/MacOS/firefox",
            "/snap/bin/firefox",
            "/usr/local/bin/firefox",
        ],
        DriverKind::Custom => &[],
    };
    for p in known {
        let p = PathBuf::from(p);
        if p.is_file() {
            return (Some(p), "known_path");
        }
    }
    (None, "")
}

#[cfg(test)]
mod tests;
