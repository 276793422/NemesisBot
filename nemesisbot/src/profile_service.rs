//! 用户习惯画像周期任务（Forge 化重构，2026-11-24：编排面薄壳）。
//!
//! sweep 逻辑（统计通道/确定性挖掘/LLM 反思/衰减/遥测前因）单一真相源 =
//! `nemesis-forge::userprofile::sweep_once`；本文件只剩 bot 侧编排装配：
//! OnceLock 单次 spawn 守卫 + 可变周期 tick + caller 每 tick 现场解析。
//!
//! caller 解析纪律（比旧实现更诚实的降级）：解析失败 = warn + 本 tick 无
//! 反思（确定性挖掘照跑），不再整体放弃 sweep——旧实现 build_caller Err
//! 会 abort 整个 sweep，把与反思无关的统计通道一起陪葬。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nemesis_forge::userprofile;

use crate::commands::gateway::shutdown::{terminal_cancel_token, terminal_loop_tick};

static SWEEP_SPAWNED: OnceLock<()> = OnceLock::new();

/// gateway 装配点调用：保证 sweep 任务只 spawn 一份。`data_store` 随任务
/// 带出——反思通道的 LLM 调用落 aux 账本（forge_review 归因族，经 judge
/// 回落链 → ForgeProviderBridge 同一链路）。`estop` 急停闸（2026-11-25
/// R2880）：engaged 时每 tick 诚实跳过（与 cron 域 autopilot/dreaming
/// 触发闸同语义；release 后下一 tick 照常补采——滚动证据窗锚点不动）。
pub fn ensure_sweep_spawned(
    home: PathBuf,
    data_store: Option<std::sync::Arc<nemesis_data::DataStore>>,
    estop: std::sync::Arc<nemesis_agent::estop::EstopState>,
) {
    if SWEEP_SPAWNED.set(()).is_err() {
        return; // 已 spawn（loop 重建多次调用只生效一次）
    }
    let handle = tokio::runtime::Handle::current();
    tokio::spawn(async move {
        // 间隔每次循环从 config 现读（user_profile.stats_scan_interval_secs，
        // 缺省 3600s）——改配置无需重启。sleep 而非 interval：周期可变。
        loop {
            let secs = tokio::task::spawn_blocking({
                let h = home.clone();
                move || load_scan_interval_secs(&h)
            })
            .await
            .unwrap_or(3600);
            // BUG-20261115-023：终态令牌等待点（sleep 型变周期循环）——
            // cancel 后 break 不再 re-park timer（家族纪律见 shutdown.rs
            // 模块文档）。
            if !terminal_loop_tick(
                terminal_cancel_token(),
                tokio::time::sleep(std::time::Duration::from_secs(secs)),
            )
            .await
            {
                break;
            }
            // sweep_once 内含 LLM 调用（反思通道）——阻塞段放 blocking 线程，
            // LLM 调用经 Handle::block_on 桥回运行时。
            let h = home.clone();
            let rh = handle.clone();
            let ds = data_store.clone();
            let es = estop.clone();
            if let Err(e) =
                tokio::task::spawn_blocking(move || sweep_tick(&h, &rh, ds.as_ref(), &es))
                    .await
                    .unwrap_or_else(|e| Err(format!("sweep task panicked: {e}")))
            {
                tracing::warn!("[UserProfile] sweep failed: {e}");
            }
        }
    });
}

/// 统计扫描间隔（config 现读；缺省 3600s）。
fn load_scan_interval_secs(home: &Path) -> u64 {
    nemesis_config::load_config(&crate::common::config_path(home))
        .ok()
        .and_then(|cfg| cfg.user_profile)
        .map(|u| u.stats_scan_interval_secs_or(3600))
        .unwrap_or(3600)
}

/// 单 tick：急停闸（engaged = 整个 tick 跳过——反思 LLM 与确定性挖掘都是
/// 自主 agent 活动，急停不变量「engaged → 自主活动不发起」覆盖；滚动证据
/// 窗锚点不动，release 后下一 tick 照常补采）+ caller 现场解析（模型热切
/// 即时生效，无陈旧 Arc）+ forge 编排调用。caller 解析失败 = warn + None
/// （反思通道本 tick 跳过，确定性挖掘照跑——不陪葬）。
fn sweep_tick(
    home: &Path,
    rt: &tokio::runtime::Handle,
    data_store: Option<&std::sync::Arc<nemesis_data::DataStore>>,
    estop: &nemesis_agent::estop::EstopState,
) -> Result<usize, String> {
    if estop.is_engaged() {
        tracing::info!("[UserProfile] estop engaged — 跳过本轮 sweep（释放后下一 tick 照常）");
        return Ok(0);
    }
    let workspace = crate::common::workspace_path(home);
    let caller = match nemesis_web::handlers::forge::resolve_judge_caller(
        &home.to_string_lossy(),
        &workspace.to_string_lossy(),
        data_store,
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "[UserProfile] reflect caller resolve failed, skip reflection this tick: {e}"
            );
            None
        }
    };
    userprofile::sweep_once(
        &crate::common::config_path(home),
        &workspace,
        rt,
        caller.as_ref(),
    )
}

#[cfg(test)]
mod tests;
