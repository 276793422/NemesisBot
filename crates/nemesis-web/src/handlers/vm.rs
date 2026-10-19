//! P5 第 4 步 — `vm.*` WSAPI + 桌面级 VM 管理面。
//!
//! 依赖方向（同 [`crate::handlers::agents`] 先例）：nemesis-web 不反向依赖
//! nemesisbot（[`VmSupervisor`](../..) 与 QEMU 管线在那边），所以这里定义
//! [`VmBridge`] trait + 模块级槽，nemesisbot 在 gateway 装配时把实现装进
//! 槽。加速器探针（WHPX/KVM）也在 bridge 侧完成——本 crate 的 `vm` feature
//! 保持零依赖（`vm = []`），不把 nemesis-sandbox 拖进 web 依赖图。
//!
//! 命令族（实施计划锁定）：`vm.overview` / `vm.config.get` / `vm.config.set`
//! / `vm.image.list` / `vm.image.build` / `vm.instance.list` /
//! `vm.instance.sleep` / `vm.instance.wake` / `vm.instance.kill` /
//! `vm.gc.run`；B3 自举族：`vm.bootstrap.status`（只读，无闸）/
//! `vm.bootstrap.install_qemu` / `vm.bootstrap.enable_accel`（enabled 闸）。
//! 未装配（headless / 独立 web 测试 / `vm` feature 关）时
//! handler 不注册，命令诚实未知。

use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Bridge trait + 进程级槽
// ---------------------------------------------------------------------------

/// nemesisbot 侧 VM 监督能力的抽象（实现在 nemesisbot `VmSupervisor`）。
/// 查询面同步（文件/registry 快查）；生命周期与镜像面异步（QEMU 停起/
/// 构建子进程是 tokio IO）。config 面 Value 直传（typed 解析在实现侧，
/// web crate 不感知 nemesis-config 的 VM 类型）。
#[async_trait::async_trait]
pub trait VmBridge: Send + Sync {
    /// 总览：config + 加速器探针 + GC 用量 + 实例清单 + 镜像状态一次取齐
    ///（Dashboard 首屏单命令渲染）。
    fn overview(&self) -> serde_json::Value;

    /// typed 配置投影（`executor.vm` 段；`None` = 段缺席）。
    fn config_get(&self) -> Option<serde_json::Value>;

    /// 写配置（typed 解析 + validate + 持久化）。非法值 loud 拒绝
    ///（错误信息含违规字段与值域）。
    fn config_set(&self, vm: serde_json::Value) -> Result<(), String>;

    /// 镜像目录清单 + 构建任务状态（`<workspace>/tools/vm/images/`）。
    fn image_list(&self) -> serde_json::Value;

    /// 触发镜像构建（update=true = 升级流程：构建进 staging → 存量
    /// overlay 安全 rebase → 换 base）。返回启动确认；进度看 `image.list`
    ///（后台任务 + 日志落盘，不阻塞 WS 帧）。
    async fn image_build(&self, update: bool) -> Result<serde_json::Value, String>;

    /// 实例清单（registry 记录 + 运行标志）。
    fn instance_list(&self) -> Vec<serde_json::Value>;

    /// 睡眠（选默认睡眠线；无可用睡眠线 = 诚实报错不假睡）。
    async fn instance_sleep(&self, agent_id: &str) -> Result<serde_json::Value, String>;

    /// 唤醒/冷启（快照线优先 → 迁移线 → 冷启）。
    async fn instance_wake(&self, agent_id: &str) -> Result<serde_json::Value, String>;

    /// 停机（冷停；overlay 保留）。
    async fn instance_kill(&self, agent_id: &str) -> Result<serde_json::Value, String>;

    /// GC：孤儿 sweep + 磁盘用量 + 预算决策报告。
    async fn gc_run(&self) -> serde_json::Value;

    /// 环境自举状态全量（**只读探针，不受 `executor.vm.enabled` 闸约束**
    /// ——查询不是安装动作；总开关关闭态安装卡也要能显示引导）。QEMU
    /// 在场三态 + 加速器引导八态 + 基础镜像在场 + 安装任务槽。
    fn bootstrap_status(&self) -> serde_json::Value;

    /// 触发 QEMU 环境安装（后台任务，进度/结果看 [`Self::bootstrap_status`]
    /// 轮询）。**enabled 闸**：`executor.vm.enabled=false` 诚实拒绝（安装
    /// 是系统侧动作，对齐 image_build 先例——关闭态零激活铁律）。
    async fn bootstrap_install_qemu(&self) -> Result<serde_json::Value, String>;

    /// 加速器引导（Windows = WHPX DISM 提权链 fire；Linux = kmod/组命令
    /// 引导文案）。**enabled 闸**同上。同步返回——网关不自我提权（UAC
    /// 子进程 fire-and-forget），其余分支返回可复制的引导命令原文。
    async fn bootstrap_enable_accel(&self) -> Result<serde_json::Value, String>;

    /// B6 快速试玩状态（**只读探针，不受 enabled 闸约束**——查询不是
    /// 动作）：enabled / image_ready（base.qcow2 + seed.iso）/ arch_
    /// supported / running 一次取齐（Dashboard 试玩卡四态渲染的单命令面）。
    fn try_status(&self) -> serde_json::Value;

    /// 启动试玩实例（固定 agent_id，骑既有 supervisor 全套机制；命中
    /// 运行槽即返回，冷启阻塞至 guest agent 就绪——与 instance.wake 同
    /// 形态）。**enabled 闸**；estop 冻结闸在 handler 层（agent 活动形态）。
    async fn try_start(&self) -> Result<serde_json::Value, String>;

    /// 试玩实例内执行 shell 命令（guest `exec` 工具；未运行时自动先
    /// ensure_running）。**enabled 闸**；estop 冻结闸在 handler 层。
    async fn try_exec(&self, command: &str) -> Result<serde_json::Value, String>;

    /// 停止试玩实例（冷停；幂等——未运行返回「未运行」不算错）。停机
    /// 方向 = 安全方向：estop 中放行（handler 层不设冻结闸）。
    async fn try_stop(&self) -> Result<serde_json::Value, String>;
}

static VM_BRIDGE: std::sync::RwLock<Option<Arc<dyn VmBridge>>> = std::sync::RwLock::new(None);

/// gateway 装配期安装 bridge（nemesisbot `VmSupervisor` 的实现）。
pub fn install_vm_bridge(bridge: Arc<dyn VmBridge>) {
    *VM_BRIDGE.write().expect("vm bridge lock") = Some(bridge);
}

/// 取当前 bridge（clone Arc 出来，不持锁返回）。
pub fn vm_bridge() -> Option<Arc<dyn VmBridge>> {
    VM_BRIDGE.read().expect("vm bridge lock").clone()
}

fn require_vm_bridge() -> Result<Arc<dyn VmBridge>, String> {
    vm_bridge().ok_or_else(|| {
        "VM 管理未装配（vm bridge 未接线，网关需为装配 gateway 模式且 vm feature 开）".to_string()
    })
}

#[cfg(test)]
pub(crate) fn set_vm_bridge_for_test(bridge: Option<Arc<dyn VmBridge>>) {
    *VM_BRIDGE.write().expect("vm bridge lock") = bridge;
}

/// bridge 触碰类测试的共享串行锁（进程级槽是共享态——并行测试互相置换
/// 会 flake；同 AGENTS_BRIDGE_TEST_LOCK 家族纪律）。只在 tests 里 acquire。
#[cfg(test)]
pub(crate) static VM_BRIDGE_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ---------------------------------------------------------------------------
// WSAPI handler
// ---------------------------------------------------------------------------

pub struct VmHandler;

#[async_trait::async_trait]
impl ModuleHandler for VmHandler {
    fn module_name(&self) -> &str {
        "vm"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "overview",
            "config.get",
            "config.set",
            "image.list",
            "image.build",
            "instance.list",
            "instance.sleep",
            "instance.wake",
            "instance.kill",
            "gc.run",
            "bootstrap.status",
            "bootstrap.install_qemu",
            "bootstrap.enable_accel",
            "try.status",
            "try.start",
            "try.exec",
            "try.stop",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let bridge = require_vm_bridge()?;
        match cmd {
            "overview" => Ok(Some(bridge.overview())),
            "config.get" => Ok(Some(serde_json::json!({
                "config": bridge.config_get(),
            }))),
            "config.set" => {
                let data = data.ok_or("missing data")?;
                let vm = data
                    .get("vm")
                    .cloned()
                    .ok_or("缺少 vm 配置对象（data.vm）")?;
                bridge.config_set(vm)?;
                Ok(Some(serde_json::json!({
                    "saved": true,
                    "note": "配置已写回 config.json（即时生效：监督器每操作现读）",
                })))
            }
            "image.list" => Ok(Some(bridge.image_list())),
            "image.build" => {
                let update = data
                    .as_ref()
                    .and_then(|d| d.get("update"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let res = bridge.image_build(update).await?;
                Ok(Some(res))
            }
            "instance.list" => {
                let instances = bridge.instance_list();
                let count = instances.len();
                Ok(Some(
                    serde_json::json!({ "instances": instances, "count": count }),
                ))
            }
            "instance.sleep" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let res = bridge.instance_sleep(&agent_id).await?;
                Ok(Some(res))
            }
            "instance.wake" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let res = bridge.instance_wake(&agent_id).await?;
                Ok(Some(res))
            }
            "instance.kill" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let res = bridge.instance_kill(&agent_id).await?;
                Ok(Some(res))
            }
            "gc.run" => Ok(Some(bridge.gc_run().await)),
            "bootstrap.status" => Ok(Some(bridge.bootstrap_status())),
            "bootstrap.install_qemu" => Ok(Some(bridge.bootstrap_install_qemu().await?)),
            "bootstrap.enable_accel" => Ok(Some(bridge.bootstrap_enable_accel().await?)),
            "try.status" => Ok(Some(bridge.try_status())),
            "try.start" => {
                refuse_if_estopped(ctx)?;
                Ok(Some(bridge.try_start().await?))
            }
            "try.exec" => {
                refuse_if_estopped(ctx)?;
                let data = data.ok_or("missing data")?;
                let command = crate::handlers::get_str(&data, "command")?;
                let command = command.trim();
                if command.is_empty() {
                    return Err("命令为空".to_string());
                }
                if command.chars().count() > TRY_EXEC_MAX_CHARS {
                    return Err(format!(
                        "命令过长（上限 {TRY_EXEC_MAX_CHARS} 字符）——试玩通道不承载长脚本"
                    ));
                }
                Ok(Some(bridge.try_exec(command).await?))
            }
            "try.stop" => Ok(Some(bridge.try_stop().await?)),
            _ => Err(format!("unknown vm cmd: {cmd}")),
        }
    }
}

/// 试玩命令长度上限（防把试玩通道当脚本通道用——长任务走 agent 面）。
const TRY_EXEC_MAX_CHARS: usize = 2000;

/// estop 冻结闸（B6 试玩 start/exec：agent 活动形态——急停触发时诚实
/// 拒绝；stop/status 不经此闸，停机与只读方向恒放行）。检查点在 handler
/// 层——`ctx.state.estop` 与 estop WSAPI / 托盘 / CLI 四入口同源。
fn refuse_if_estopped(ctx: &RequestContext) -> Result<(), String> {
    let engaged = ctx
        .state
        .estop
        .as_ref()
        .map(|e| e.is_engaged())
        .unwrap_or(false);
    if engaged {
        return Err(
            "急停生效中：试玩启动/执行已冻结（estop --release 或侧栏释放后恢复）".to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod vm_ws_tests;
