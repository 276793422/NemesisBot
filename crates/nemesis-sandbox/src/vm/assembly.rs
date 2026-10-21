//! QEMU 命令行装配（纯函数，P5 第 2 步）——生命周期/镜像/网络/隔离语义
//! 的**唯一参数产地**。
//!
//! 设计纪律（实施计划 §5「GPU/音频」扩展点）：
//! - 所有 QEMU 参数集中在本模块纯函数产出（调用方只传 spec，不拼字符串）
//!   ——GPU/音频/显示恢复 = 只改这里 + 配置 schema 加键；
//! - 显示面（P5-R2）：默认 `-display none`（headless 服务器形态字节不变，
//!   测试锚定）；`display_vnc_port = Some(port)` 时装配 `-vnc 127.0.0.1:<display>`
//!   （回环绑定——NAT 零暴露闸同源；port=真实 TCP 端口，display=port-5900
//!   反向换算，见 VNC_DISPLAY_BASE 注——QEMU 冒号后数字是显示器编号而非
//!   端口，直接填端口会错绑 5900 偏移，registry 永远连不上）。**readonly
//!   不在 QEMU 层做**：`-vnc
//!   ...,readonly` 会锁死所有 viewer 且改档需重启 VM——per-session 观察档
//!   由 plugin-vm 在 RFB 客户端→服务端方向过滤写类消息（类型 4/5/6）实现。
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

/// VNC 显示绑定地址（恒 127.0.0.1——与 NAT 闸同源：VNC 只绑回环，外部
/// 访问一律过网关 shim + dashboard token，裸协议不出回环）。
pub const VNC_BIND: &str = "127.0.0.1";

/// QEMU `-vnc` 的显示器编号基准（真实 TCP 端口 = 5900 + 编号——QEMU 全
/// 版本稳定语义；2026-10-21 真机探针定谳：`-vnc 127.0.0.1:12345` 绑定
/// 18245，`port=` 子选项不存在（Invalid parameter），无直写端口语法）。
pub const VNC_DISPLAY_BASE: u16 = 5900;

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
    /// 显示面（P5-R2）：Some(port) = VNC 监听**真实 TCP 端口**（127.0.0.1；
    /// QEMU 参数按 display=port-5900 反向换算，见 VNC_DISPLAY_BASE 注）；
    /// None = `-display none`（默认无头形态，字节不变）。端口由调用方走
    /// `alloc_loopback_port` 同机制分配（临时端口段恒 ≥5900）。
    pub display_vnc_port: Option<u16>,
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
    match spec.display_vnc_port {
        Some(port) => {
            // vnc 态：回环绑定（VNC_BIND 恒 127.0.0.1），QEMU 直接说 RFB——
            // 插件/网关面按字节泵中继，本层不含 VNC 语义。
            // QEMU 冒号后数字是**显示器编号**（真实端口 = 5900+编号，见
            // VNC_DISPLAY_BASE 注）；display_vnc_port 契约 = 真实 TCP 端口
            // （registry/plugin-vm 直连用），此处反向换算绑回 port。
            // 临时端口段（Win 49152+ / Linux 32768+）恒 ≥5900；低于 5900
            // 的 port 无合法 display 形态——诚实 panic（错绑 5900 端口比
            // 启动失败更危险：registry 记录的端口连不上=VNC 永久哑）。
            let display_no = port.checked_sub(VNC_DISPLAY_BASE).unwrap_or_else(|| {
                panic!("display_vnc_port {port} < {VNC_DISPLAY_BASE}（QEMU display 编号无法表达；端口分配必须走临时端口段）")
            });
            args.push("-vnc".into());
            args.push(format!("{VNC_BIND}:{display_no}"));
        }
        None => args.extend(display_args().iter().map(|s| s.to_string())),
    }
    // 绝对指点设备（usb-tablet）：q35 默认不带任何 pointer 硬件（探针
    // 实录：guest /proc/bus/input/devices 只有键盘类 event，X 无鼠标可
    // 拾取）——VNC 操作档的 PointerEvent 是绝对坐标，usb-tablet 是 QEMU
    // 绝对指针的标准载体（usbhid 通用驱动，guest 侧零额外配置）。
    // q35 也无默认 USB 总线（裸 `-device usb-tablet` 实测 `No 'usb-bus'
    // bus found`）——显式 xhci 控制器 + tablet 挂显式总线，不赌默认行为。
    args.push("-device".into());
    args.push("qemu-xhci,id=vm-usb".into());
    args.push("-device".into());
    args.push("usb-tablet,bus=vm-usb.0".into());
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

/// 装配前校验（`VmInstance::start` spawn 前的最后一道闸）。
///
/// 1. **逗号闸**：`-drive file=...,format=qcow2` / `-virtfs local,path=...` /
///    `-smbios type=1,serial=...` / `-incoming file:...`（QEMU 8.0+ 支持
///    `file:<path>,offset=` 子选项）都是「单 argv 内逗号分隔子选项」形态——
///    路径/值里的 `,` 会被 QEMU 当子选项分隔符（L6++ 项目路径用户任意、
///    可含逗号），产生离谱报错甚至静默错配。诚实拒绝优于猜测转义（QEMU
///    各解析点的转义语义无法本地验证、版本间有漂移）。
/// 2. **wake 载体互斥**：`-loadvm` 与 `-incoming` 同时装配互相打架（规格
///    约定互斥——spec 组装点分散，靠约定不如靠闸）。
///
/// `-pidfile` / `-loadvm` 的值是独立 argv 元素、无子选项解析，逗号无害，
/// 不在闸内（校验面贴着真实风险，不过度）。
pub fn validate_launch_spec(spec: &VmLaunchSpec) -> Result<(), String> {
    if spec.loadvm_tag.is_some() && spec.incoming_file.is_some() {
        return Err(
            "loadvm_tag 与 incoming_file 互斥（savevm 与 migrate-file 两条 wake 线不可同启）"
                .to_string(),
        );
    }
    let mut checked: Vec<(&'static str, &Path)> =
        vec![("overlay_path", spec.overlay_path.as_path())];
    if let Some(p) = &spec.shared_host_path {
        checked.push(("shared_host_path", p));
    }
    if let Some(p) = &spec.incoming_file {
        checked.push(("incoming_file", p));
    }
    let bad: Vec<String> = checked
        .iter()
        .filter(|(_, p)| p.to_string_lossy().contains(','))
        .map(|(label, p)| format!("{label}={}", p.display()))
        .collect();
    if !bad.is_empty() {
        return Err(format!(
            "以下路径含逗号（QEMU 子选项分隔符，无法安全装配）：{}——请改用不含逗号的工作区/项目目录",
            bad.join("; ")
        ));
    }
    if spec.token.contains(',') {
        return Err(
            "token 含逗号（-smbios serial 的子选项分隔符）——token 应为 generate_token 产出的纯 hex"
                .to_string(),
        );
    }
    Ok(())
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
