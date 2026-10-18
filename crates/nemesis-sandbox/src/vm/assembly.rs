//! QEMU 命令行装配（纯函数，P5 第 2 步）——生命周期/镜像/网络/隔离语义
//! 的**唯一参数产地**。
//!
//! 设计纪律（实施计划 §5「GPU/音频」扩展点）：
//! - 所有 QEMU 参数集中在本模块纯函数产出（调用方只传 spec，不拼字符串）
//!   ——GPU/音频/显示恢复 = 只改这里 + 配置 schema 加键；
//! - `-display none` 固定（headless 服务器形态，guest 内浏览器走
//!   offscreen/Xvfb）；
//! - hostfwd **恒绑 127.0.0.1**（G2 宿主零暴露面闸——装配层钉死，测试
//!   断言；外部接口不可达由绑定语义保证）；
//! - 首帧 token 挑战鉴权的 token 经 `-smbios type=1,serial=<token>` 传入
//!   guest（guest launcher 读 DMI serial 注入 env）——per-instance token
//!   零镜像改动、零额外挂载。

use std::path::{Path, PathBuf};

use super::guest_arch::{GuestArch, QemuAccel, display_args, machine_spec};

/// guest 内 guest agent 的监听端口（约定常量；镜像内 systemd unit 与
/// hostfwd 装配两侧同源消费）。
pub const GUEST_AGENT_PORT: u16 = 48500;

/// QMP 绑定地址（恒 127.0.0.1——QMP 无鉴权，回环绑定是唯一暴露面控制）。
pub const QMP_BIND: &str = "127.0.0.1";

/// NAT hostfwd 绑定地址（恒 127.0.0.1——G2 宿主零暴露面闸）。
pub const NAT_BIND: &str = "127.0.0.1";

/// 9p 共享子树挂载 tag（guest 侧 cloud-init 按此 tag 挂到 /workspace）。
pub const WORKSPACE_MOUNT_TAG: &str = "workspace";

/// 一次 VM 启动的完整装配输入（caller 从 `ExecutorVmConfig` + 运行期
/// 端口/路径解析产出；本结构只承载数据不含逻辑）。
#[derive(Debug, Clone)]
pub struct VmLaunchSpec {
    /// qemu-system 二进制路径（宿主探测产出）。
    pub qemu_bin: PathBuf,
    /// pidfile 路径（孤儿 sweep 与诊断用，第 3 步消费）。
    pub pidfile: PathBuf,
    /// 宿主加速后端（构造期探针裁决产出，见 lifecycle::adjudicate_accel）。
    pub accel: QemuAccel,
    /// guest 架构。
    pub arch: GuestArch,
    /// vCPU 数（executor.vm.resources.vcpu）。
    pub vcpu: u32,
    /// 内存 MiB（executor.vm.resources.ram_mb）。
    pub ram_mb: u32,
    /// 本实例 qcow2 overlay 路径（base 作 backing，见 overlay 模块）。
    pub overlay_path: PathBuf,
    /// cloud-init nocloud seed ISO（首启装配 guest agent 用；已装好的持久
    /// overlay 复用启动可 None——seed 挂载只在首次引导有消费方）。
    pub seed_iso: Option<PathBuf>,
    /// QMP 监听端口（宿主 127.0.0.1 临时端口）。
    pub qmp_port: u16,
    /// hostfwd 宿主侧端口（宿主 127.0.0.1 临时端口 → guest agent 端口）。
    pub host_forward_port: u16,
    /// 首帧鉴权 token（per-instance 随机生成；经 SMBIOS serial 传入 guest）。
    pub token: String,
    /// 9p 共享子树（Some = `-virtfs` 装配；None = 不挂（Windows 宿主
    /// QEMU 无 virtfs 后端——诚实边界，guest 内 /workspace 落 VM 磁盘）。
    pub shared_host_path: Option<PathBuf>,
    /// wake 载体 A：`-loadvm <tag>`（savevm 快照 tag，qcow2 内部快照，
    /// KVM 首选线；与 [`Self::incoming_file`] 互斥使用——两条 wake 线
    /// 都以「已装配好的 overlay + 记录的睡眠载体」重挂，seed 恒 None）。
    pub loadvm_tag: Option<String>,
    /// wake 载体 B：`-incoming file:<path>`（migrate-to-file 状态文件，
    /// WHPX 备选线——早验判定 WHPX savevm 不可行，见实施计划附录）。
    pub incoming_file: Option<PathBuf>,
}

/// 装配完整 QEMU 参数表（顺序稳定——可断言、可复现）。
pub fn build_qemu_args(spec: &VmLaunchSpec) -> Vec<String> {
    let ms = machine_spec(spec.arch, spec.accel);
    let mut args: Vec<String> = Vec::with_capacity(24);
    args.push("-machine".into());
    args.push(ms.machine.into());
    args.push("-accel".into());
    args.push(spec.accel.as_str().into());
    args.push("-cpu".into());
    args.push(ms.cpu.into());
    args.push("-smp".into());
    args.push(spec.vcpu.to_string());
    args.push("-m".into());
    args.push(spec.ram_mb.to_string());
    args.push("-drive".into());
    args.push(format!(
        "file={},format=qcow2,if=virtio",
        spec.overlay_path.display()
    ));
    if let Some(seed) = &spec.seed_iso {
        // cloud-init nocloud seed（fs label `cidata`——cloud-init 按标签
        // 发现数据源；media=cdrom 走 q35 的 AHCI）。
        args.push("-drive".into());
        args.push(format!(
            "file={},format=raw,media=cdrom,readonly=on",
            seed.display()
        ));
    }
    args.extend(display_args().iter().map(|s| s.to_string()));
    args.push("-nic".into());
    args.push(format!(
        "user,model=virtio-net-pci,hostfwd=tcp:{NAT_BIND}:{}-:{GUEST_AGENT_PORT}",
        spec.host_forward_port
    ));
    args.push("-qmp".into());
    args.push(format!(
        "tcp:{QMP_BIND}:{},server=on,wait=off",
        spec.qmp_port
    ));
    args.push("-smbios".into());
    args.push(format!("type=1,serial={}", spec.token));
    args.push("-pidfile".into());
    args.push(spec.pidfile.display().to_string());
    if let Some(host_dir) = &spec.shared_host_path {
        args.push("-virtfs".into());
        args.push(format!(
            "local,path={},mount_tag={WORKSPACE_MOUNT_TAG},security_model=none,id=workspace",
            host_dir.display()
        ));
    }
    if let Some(tag) = &spec.loadvm_tag {
        // wake 线 A：从 qcow2 内部 savevm 快照恢复（先于 guest 执行）。
        args.push("-loadvm".into());
        args.push(tag.clone());
    }
    if let Some(state) = &spec.incoming_file {
        // wake 线 B：migrate-to-file 状态恢复（`file:` URI 统一正斜杠——
        // Windows 路径反斜杠在 QEMU URI 里的解析版本差异是已知坑，探针
        // 实测正斜杠形态可行，装配层钉死）。
        args.push("-incoming".into());
        args.push(format!(
            "file:{}",
            state.display().to_string().replace('\\', "/")
        ));
    }
    args
}

/// 从 PATH / 安装目录解析 qemu-system 二进制（宿主探测辅助）：
/// 显式目录优先，否则 PATH 查找（直接把二进制名给 Command 交给 OS 解析）。
pub fn resolve_qemu_binary(arch: GuestArch, install_dir: Option<&Path>) -> PathBuf {
    let name = arch.qemu_system_binary();
    match install_dir {
        Some(dir) => {
            #[cfg(target_os = "windows")]
            let candidate = dir.join(format!("{name}.exe"));
            #[cfg(not(target_os = "windows"))]
            let candidate = dir.join(name);
            if candidate.exists() {
                candidate
            } else {
                PathBuf::from(name) // 目录里没有 = 落回 PATH（诚实探测裁决）
            }
        }
        None => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests;
