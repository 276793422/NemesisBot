//! 宿主浏览器接管框架（host_browser，2026-11-15 计划落地）。
//!
//! bot 在**用户当前机器**上以调试旗标启动专用浏览器窗口（CDP 路线），人机
//! 共享驾驶：用户可以继续用自己的鼠标键盘操作同一窗口，bot 通过 CDP 观察
//! 页面 + 注入可信输入事件。
//!
//! 分层（真相源 `docs/REPORT/2026-11-15_浏览器接管框架交付报告.md` §1）：
//! - [`driver`]：`BrowserDriver` trait——动作语义按「网页操作」描述
//!   （navigate/click/type/key/scroll/extract/screenshot/wait），不做 CDP
//!   特有假设；`capabilities()` 能力位；v1 实现 = [`driver::ChromiumCdpDriver`]。
//! - [`cdp`]：CDP 客户端——HTTP/1.1 发现端点（chromium 155 明拒 HTTP/1.0、
//!   keep-alive EOF 挂死，只认 Content-Length 框架读）+ 常驻 WS 命令通道
//!   （id 多路复用 + 事件订阅，移植自 nemesis-guest 手写件 + 多路复用改造）。
//! - [`session`]：`BrowserSession` 会话编排——进程生命周期（spawn/探测就绪/
//!   关闭）+ 专用 profile + tab 注册表 + 「工作 tab」指针 + AI 操作徽标。
//! - [`manager`]：`BrowserManager`——会话注册表 + 操作事件环形缓冲（日志页
//!   编排操作路数据源）+ 浏览器选择解析（显式配置 > Chrome 偏好 > 平台保底）。
//! - [`detect`]：系统浏览器枚举（Windows 注册表 App Paths + 已知路径 + PATH；
//!   Firefox 二期 = 枚举可见不可驱动）。
//! - [`creds`]：凭据别名命名空间（`vault:browser/<域名>/<name>`）+ origin
//!   域名匹配 + 密码框元素校验 JS——凭据守卫三条件的 crate 侧支撑件。
//!
//! estop 契约（计划 D13，明文例外）：estop 冻结 AI 操作面（工具调用在
//! dispatch 瀑布被拦），但**不杀浏览器窗口**——用户可能还在用；gateway 退出
//! 同理不杀（cookie 落盘，下次 spawn 原样恢复）。

pub mod cdp;
pub mod creds;
pub mod detect;
pub mod driver;
pub mod manager;
pub mod session;

pub use creds::{browser_alias_domain, origin_matches_host};
pub use detect::{DetectedBrowser, detect_installed};
pub use driver::{BrowserDriver, DriverCaps, DriverKind, TabInfo, WebAction};
pub use manager::{BrowserManager, BrowserPrefs, DEFAULT_LOG_LIMIT};
pub use session::BrowserSession;

#[cfg(test)]
mod tests;
