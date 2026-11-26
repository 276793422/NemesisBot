//! 名册发现循环（M2，决策 7）：register/members HTTP 往返搬入插件——宿主
//! 只保留成员入册漏斗与注册表（集群状态是宿主的）。拉到的全量成员经
//! vtable `notify_members` 推宿主（`{"members":[...]}` 全量包，宿主自过滤）。
//!
//! 语义对齐宿主被搬走前的实现（`nemesis_cluster::roaming::rendezvous::
//! sync_once`——错误文案逐字对齐，宿主 tests 断言可平移）；循环节奏 = 插件
//! 自持 watch 停止闸 + 连败指数退避（BUG-7 2026-11-25：30s 一拍无限重试
//! 数天刷 2000+ 条告警——现 30s→5min 封顶，首败 + 退避档位变化 + 触顶后
//! 每 10 连败心跳才打日志，恢复/降级状态翻转各一条汇总）。
//!
//! 身份一致性闸（载荷 `member.iroh_id` == 插件盘上身份）在 `registry_start`
//! FFI 入口同步裁决（err_code `identity_mismatch` 直接回宿主，不埋循环内）
//! ——见 lib.rs。

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;

use crate::ROAMING_LOG_ERROR;
use crate::roaming::{ROAMING_LOG_INFO, ROAMING_LOG_WARN};
use crate::types::RendezvousMember;

/// 默认同步间隔（宿主 config.cluster.json 同值语义）。
const DEFAULT_INTERVAL_SECS: u64 = 30;

/// 连败退避封顶（BUG-7 2026-11-25 远程 home 取证）：指数退避最长 5 分钟
/// ——中继不可达是外部问题，30s 一拍无限重试数天刷 2000+ 条 WARN/ERROR
/// 淹没有效信号（本案取证干扰判读）+ 无意义网络重试浪费资源。
const MAX_BACKOFF_SECS: u64 = 300;

/// 连败退避间隔（BUG-7）：连续第 N 次失败后，下一次尝试的等待 =
/// base × 2^(N-1)，封顶 max(base, 5min)。封顶取 max 是为尊重用户显式
/// 配的慢节拍（base > 5min 时退避不再加码——退避的语义是「比常态更稀」，
/// 不是把用户配的节拍改短）。首败不追加延迟（N=1 → 2^0 = 1 倍）。
/// 纯函数（loop 体从它取等待；单测钉死阶梯与封顶）。
pub(crate) fn backoff_after_fail(
    base: std::time::Duration,
    fail_streak: u64,
) -> std::time::Duration {
    if fail_streak == 0 {
        return base;
    }
    let factor = 1u32 << (fail_streak - 1).min(4);
    base.saturating_mul(factor)
        .min(base.max(Duration::from_secs(MAX_BACKOFF_SECS)))
}

/// 循环运行状态槽（Phase 2 观测性：失败态出插件——此前 fail_streak/last_error
/// 只进日志，宿主状态面永远只见「最后成功」形态，Dashboard 失败徽标是死分支）。
/// 每 tick 更新；epoch ms 与宿主 `RendezvousClientState` 同源（UNIX_EPOCH）。
#[derive(Debug, Default, Clone)]
pub(crate) struct RegistryRunStatus {
    /// 本循环注册的名册地址（失败态从出生时宿主无快照可兜底——插件自带）。
    pub registry_url: String,
    /// 最近同步学到的成员数（含自己；0 = 尚未成功过）。
    pub last_member_count: usize,
    /// 连续失败计数（成功清零）。
    pub fail_streak: u64,
    /// 最近一次失败的错误文案（`sync_once` 原样）。
    pub last_error: String,
    /// 最近一次成功 tick（epoch ms；0 = 从未成功）。
    pub last_ok_ms: u64,
    /// 最近一次失败 tick（epoch ms；0 = 从未失败）。
    pub last_fail_ms: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 单次同步：register 自己 + 拉全量成员（两请求一次往返语义）。
///
/// 返回**含自己**的成员列表（调用方自过滤——保留全量便于观测 member_count）。
pub(crate) async fn sync_once(
    client: &reqwest::Client,
    registry_url: &str,
    token: &str,
    self_member: &RendezvousMember,
) -> Result<Vec<RendezvousMember>, String> {
    let base = registry_url.trim().trim_end_matches('/');
    let url = format!("{base}/v1/roaming/register");
    let resp = client
        .post(url)
        .bearer_auth(token)
        .json(self_member)
        .send()
        .await
        .map_err(|e| format!("register 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("register 被拒（HTTP {}）", resp.status().as_u16()));
    }
    let url = format!("{base}/v1/roaming/members");
    let resp = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("拉取成员失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("拉取成员被拒（HTTP {}）", resp.status().as_u16()));
    }
    #[derive(Deserialize)]
    struct MembersResp {
        members: Vec<RendezvousMember>,
    }
    let body: MembersResp = resp
        .json()
        .await
        .map_err(|e| format!("成员响应解析失败: {e}"))?;
    Ok(body.members)
}

/// 循环配置（`registry_start` 入参 JSON 解析形态）。
#[derive(Deserialize)]
pub(crate) struct RegistryLoopConfig {
    pub registry_url: String,
    pub token: String,
    pub member: RendezvousMember,
    #[serde(default)]
    pub interval_secs: Option<u64>,
}

/// 常驻循环句柄（watch 停止闸 + join + 运行状态槽）。幂等重入 = 停旧起新。
pub(crate) struct RegistryLoopHandle {
    stop_tx: tokio::sync::watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
    status: Arc<Mutex<RegistryRunStatus>>,
}

impl RegistryLoopHandle {
    pub fn running(&self) -> bool {
        !*self.stop_tx.borrow()
    }

    /// 运行状态快照（`roaming_status` 的 registry_sync 段数据源；cloned——
    /// 持锁窗口最小化）。
    pub fn status_snapshot(&self) -> RegistryRunStatus {
        self.status.lock().clone()
    }

    pub async fn shutdown(&mut self) {
        let _ = self.stop_tx.send(true);
        let _ = (&mut self.join).await;
    }
}

/// 在当前 runtime 上下文拉起常驻循环（FFI 导出内 `block_on` 调用）。
/// `on_members`：成员推宿主漏斗（vtable notify_members 包装，插件循环
/// 工作线程同步执行——宿主漏斗是纯内存入册，无回呼插件，无死锁面）。
pub(crate) async fn spawn_registry_loop(
    config: RegistryLoopConfig,
    on_members: impl Fn(Vec<RendezvousMember>) + Send + Sync + 'static,
) -> RegistryLoopHandle {
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let status = Arc::new(Mutex::new(RegistryRunStatus::default()));
    let join = tokio::spawn(run_loop(config, on_members, stop_rx, Arc::clone(&status)));
    RegistryLoopHandle {
        stop_tx,
        join,
        status,
    }
}

async fn run_loop(
    config: RegistryLoopConfig,
    on_members: impl Fn(Vec<RendezvousMember>) + Send + Sync + 'static,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    status: Arc<Mutex<RegistryRunStatus>>,
) {
    *status.lock() = RegistryRunStatus {
        registry_url: config.registry_url.clone(),
        last_member_count: 0,
        fail_streak: 0,
        last_error: String::new(),
        last_ok_ms: 0,
        last_fail_ms: 0,
    };
    // 构建失败 fail-closed（BUG-20261123-028 横向扫）：不再
    // `unwrap_or_default()` 静默回落默认 client——那会丢掉 15s 超时，
    // 挂死端点让整个名册循环无限卡住（BUG-20261114-040 同族语义：
    // 零超时默认 client = 挂死面原样重开）。后台循环诚实退出：loud
    // 日志 + status.last_error 留痕，循环不启动优于无超时裸奔。
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("rendezvous HTTP client build failed: {e}");
            crate::plugin_log(ROAMING_LOG_ERROR, "rendezvous", &msg);
            *status.lock() = RegistryRunStatus {
                registry_url: config.registry_url.clone(),
                last_member_count: 0,
                fail_streak: 0,
                last_error: msg,
                last_ok_ms: 0,
                last_fail_ms: 0,
            };
            return;
        }
    };
    let interval =
        Duration::from_secs(config.interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS).max(1));
    let mut fail_streak: u64 = 0;
    // 首拍即时（原 interval 首 tick 即时语义保持）；此后每轮等待按连败
    // 退避取值（BUG-7）。
    let mut first_tick = true;
    crate::plugin_log(
        ROAMING_LOG_INFO,
        "rendezvous",
        &format!(
            "名册循环已启动 url={} interval={}s node={}",
            config.registry_url.trim().trim_end_matches('/'),
            interval.as_secs(),
            config.member.iroh_id
        ),
    );
    loop {
        if !first_tick {
            let wait = backoff_after_fail(interval, fail_streak);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop_rx.changed() => break,
            }
        }
        first_tick = false;
        match sync_once(&client, &config.registry_url, &config.token, &config.member).await {
            Ok(members) => {
                // 状态翻转（降级 → 恢复）：一条汇总（BUG-7）。
                if fail_streak > 0 {
                    crate::plugin_log(
                        ROAMING_LOG_INFO,
                        "rendezvous",
                        &format!(
                            "名册同步恢复：连续失败 {fail_streak} 次后成功（成员 {} 个）",
                            members.len()
                        ),
                    );
                }
                fail_streak = 0;
                let mut st = status.lock();
                st.fail_streak = 0;
                st.last_error.clear();
                st.last_ok_ms = now_ms();
                st.last_member_count = members.len();
                drop(st);
                on_members(members);
            }
            Err(e) => {
                fail_streak += 1;
                {
                    let mut st = status.lock();
                    st.fail_streak = fail_streak;
                    st.last_error = e.clone();
                    st.last_fail_ms = now_ms();
                }
                // 告警面收敛（BUG-7）：首败 + 退避档位变化（升级）+ 触顶后
                // 每 10 连败心跳——稳态退避期不再每拍刷屏（此前 30s 一条，
                // 数天 2000+ 条）。状态槽每拍照常更新（Dashboard 面可见）。
                let next_wait = backoff_after_fail(interval, fail_streak);
                let prev_wait = backoff_after_fail(interval, fail_streak - 1);
                if fail_streak == 1 || next_wait != prev_wait || fail_streak.is_multiple_of(10) {
                    crate::plugin_log(
                        ROAMING_LOG_WARN,
                        "rendezvous",
                        &format!(
                            "名册同步失败（连续 {fail_streak} 次）: {e}；{}s 后重试",
                            next_wait.as_secs()
                        ),
                    );
                }
            }
        }
    }
    crate::plugin_log(ROAMING_LOG_INFO, "rendezvous", "名册循环停机");
}
