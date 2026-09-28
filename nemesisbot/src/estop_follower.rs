//! 跨进程 estop 跟随器（2026-09-26 复检挂账高优修复）。
//!
//! 独立装配入口（mcp-serve / acp）的 [`EstopState`] 是进程内新建的 Arc，
//! gateway 的急停对其不可达——此前「MCP/编辑器出口不是安全旁路」的承诺
//! 对 estop 一项不成立：gateway 全局急停后，外部客户端仍可驱动本进程
//! 的工具/run。本模块补齐：后台任务周期 POST gateway `/api/internal`
//! `estop_status`，把 gateway 的急停态**镜像**到本地 EstopState（单向
//! 跟随，gateway 是唯一权威源；本进程不存在独立触发路径，镜像不会
//! 覆盖任何本地状态）。
//!
//! 发现面与 CLI `estop` 命令同源：`<workspace>/state/gateway.json` 拿
//! web_host/web_port，config.json `channels.web.auth_token` 做
//! X-Auth-Token。
//!
//! 语义边界（诚实声明）：
//! - gateway.json 缺失 / web_port=0 = 无 gateway 在跑 → 不跟随（独立
//!   进程没有权威源可跟，继续重试——gateway 后启动也能跟上）；
//! - 轮询失败（gateway 停机/网络抖动）→ 保持最后镜像态，安静重试；
//! - 远端状态只在**变化**时应用（trigger/release 幂等，但避免无谓唤醒
//!   agent loop 的 watch 订阅者）；
//! - 急停生效延迟上界 ≈ 轮询间隔（POLL_SECS）+ 一轮 LLM 内的检查点
//!   间隔（loop 顶 + 工具 dispatch 双检查点）。

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

/// 轮询间隔（秒）。急停是人工尺度的操作，2s 的最坏生效延迟可接受。
pub const POLL_SECS: u64 = 2;

static SHARED_ESTOP: OnceLock<Arc<nemesis_agent::estop::EstopState>> = OnceLock::new();

/// 进程级共享急停态。独立入口进程（mcp-serve / acp）的所有装配共用这一个
/// EstopState（acp 每会话装配一次，各自 new 会变成互不相通的孤岛——
/// 跟随器也只镜像这一个共享态）。首个调用创建，后续复用。
pub fn shared_estop() -> Arc<nemesis_agent::estop::EstopState> {
    Arc::clone(SHARED_ESTOP.get_or_init(|| Arc::new(nemesis_agent::estop::EstopState::new())))
}

/// 进程级一次性跟随任务（幂等：重复调用不重复 spawn——acp 每会话装配
/// 都会调，任务泄漏防线）。
pub fn ensure_spawned(home: std::path::PathBuf) {
    static SPAWNED: OnceLock<()> = OnceLock::new();
    if SPAWNED.set(()).is_ok() {
        spawn(shared_estop(), home);
    }
}

/// 跟随目标（gateway 内部 API 入口 + 鉴权 token）。
#[derive(Debug, Clone, PartialEq)]
pub struct FollowerTarget {
    pub base_url: String,
    pub auth_token: String,
}

/// 发现正在跑的 gateway（CLI `estop` 命令同源逻辑）。
///
/// 返回 None = 无 gateway 可跟（state 文件缺失 / 形态无效 / web_port=0）。
pub fn discover(home: &Path) -> Option<FollowerTarget> {
    let config_path = crate::common::config_path(home);
    let cfg_str = std::fs::read_to_string(config_path).ok()?;
    let cfg: Value = serde_json::from_str(&cfg_str).ok()?;
    let auth_token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let state_path =
        nemesis_path::resolve_gateway_state_path_in_workspace(&crate::common::workspace_path(home));
    let info = crate::commands::dashboard::read_gateway_state(&state_path)?;
    if info.web_port <= 0 {
        return None;
    }
    Some(FollowerTarget {
        base_url: format!("http://{}:{}", info.web_host, info.web_port),
        auth_token,
    })
}

/// 把远端急停态镜像进本地 state。返回是否实际应用（状态变化才动，
/// 避免无谓唤醒 agent loop 的 watch 订阅者）。
pub fn mirror(local: &nemesis_agent::estop::EstopState, engaged: bool) -> bool {
    if local.is_engaged() == engaged {
        return false;
    }
    if engaged {
        local.trigger();
    } else {
        local.release();
    }
    true
}

/// 复用 client 的单次轮询（spawn 循环体唯一 HTTP 路径；测试经子模块
/// 可见性直测）。Ok(None) = 响应形态不符合预期（当作本轮无信号，下轮
/// 重试）。
async fn poll_with(
    client: &reqwest::Client,
    target: &FollowerTarget,
) -> Result<Option<bool>, String> {
    let url = format!("{}/api/internal", target.base_url);
    let resp = client
        .post(&url)
        .header("X-Auth-Token", &target.auth_token)
        .json(&json!({ "cmd": "estop_status" }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("estop_status returned {status}: {body}"));
    }
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| format!("estop_status body parse failed: {e}"))?;
    Ok(v.get("engaged").and_then(|e| e.as_bool()))
}

/// 启动后台跟随任务：每 [`POLL_SECS`] 秒发现 + 轮询 + 镜像，直到进程
/// 退出。日志走 stderr（stdio 协议通道纪律），且只在**状态翻转**时
/// 打印（不刷屏）。
pub fn spawn(
    estop: Arc<nemesis_agent::estop::EstopState>,
    home: std::path::PathBuf,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut client: Option<reqwest::Client> = None;
        // 翻转沿日志标志（初值与第一轮实际状态比较后按需打印）。
        let mut announced_following = false;
        let mut announced_unreachable = false;
        loop {
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
            let Some(target) = discover(&home) else {
                if announced_following {
                    eprintln!("estop-follower: gateway state 消失（停机？），等待重新发现");
                    announced_following = false;
                }
                continue;
            };
            let client = client.get_or_insert_with(|| {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(3))
                    .build()
                    .expect("reqwest client build")
            });
            match poll_with(client, &target).await {
                Ok(engaged) => {
                    if !announced_following {
                        eprintln!("estop-follower: 跟随 gateway {}", target.base_url);
                        announced_following = true;
                    }
                    announced_unreachable = false;
                    if let Some(e) = engaged {
                        mirror(&estop, e);
                    }
                }
                Err(_) => {
                    // 不可达/非 2xx：保持最后镜像态，安静重试（只在翻转沿打印）。
                    if !announced_unreachable {
                        eprintln!("estop-follower: gateway 轮询失败，保持最后已知态并重试");
                        announced_unreachable = true;
                        announced_following = false;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
