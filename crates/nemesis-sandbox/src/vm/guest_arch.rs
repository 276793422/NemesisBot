//! GuestArch / QemuAccel / QEMU 机型映射表（P5 底座，轮次 5「接口必须留好」）。
//!
//! **aarch64 预留纪律**（用户拍板原话：「我们可以不做相关的功能，但是接口
//! 必须留好，arm64 架构，不管是 windows 还是 linux 都是有大量需求的，只是
//! 我们目前还没支持」）：枚举与机型/accel 映射表带全 arm 条目（条目在位、
//! 标注「当前不产出」）；选了 aarch64 = 诚实报不支持（不是静默改道）。

/// Guest CPU 架构。单一真相源——配置解析、QEMU 装配、镜像描述三面同源消费。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuestArch {
    /// 当前唯一产出档（两台目标宿主均 x86_64）。
    X86_64,
    /// 接口在位、当前不产出（2026-10-17）：不产镜像、不启动运行。
    /// 恢复路径见实施计划 §5：cloud image 换 arm 变体 → 镜像管线 target
    /// 切换 → KVM/WHPX arm 档启用。
    AArch64,
}

impl GuestArch {
    pub const ALL: &'static [GuestArch] = &[GuestArch::X86_64, GuestArch::AArch64];

    /// 配置字符串形态（与 `executor.vm.arch` 值域一一对应）。
    pub fn as_str(&self) -> &'static str {
        match self {
            GuestArch::X86_64 => "x86_64",
            GuestArch::AArch64 => "aarch64",
        }
    }

    /// 解析配置字符串。未知值 loud 拒绝（对齐 `executor.backend` P24 先例
    /// ——选型错误不静默改道）。
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "x86_64" => Ok(GuestArch::X86_64),
            "aarch64" => Ok(GuestArch::AArch64),
            other => Err(format!(
                "executor.vm.arch 未知值 {other:?}（合法值：x86_64 | aarch64；\
                 aarch64 当前为接口预留档，选择后镜像管线会诚实报不产出）"
            )),
        }
    }

    /// QEMU 系统模拟器二进制名（宿主 PATH / 安装目录内查找用）。
    pub fn qemu_system_binary(&self) -> &'static str {
        match self {
            GuestArch::X86_64 => "qemu-system-x86_64",
            GuestArch::AArch64 => "qemu-system-aarch64",
        }
    }

    /// 该架构是否已产出（镜像管线/启动路径的诚实闸——aarch64 恒 false）。
    pub fn is_produced(&self) -> bool {
        matches!(self, GuestArch::X86_64)
    }
}

/// QEMU 硬件加速后端（只收真实硬件虚拟化；无加速 = 诚实 Unavailable，
/// 不做 TCG 纯模拟降级——性能不可用级，假跑违反诚实降级铁律）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QemuAccel {
    /// Linux KVM（`/dev/kvm`）。
    Kvm,
    /// Windows Hypervisor Platform（WHPX）。
    Whpx,
}

impl QemuAccel {
    pub const ALL: &'static [QemuAccel] = &[QemuAccel::Kvm, QemuAccel::Whpx];

    pub fn as_str(&self) -> &'static str {
        match self {
            QemuAccel::Kvm => "kvm",
            QemuAccel::Whpx => "whpx",
        }
    }

    /// `executor.vm.hypervisor` 解析：`"auto"` → `Ok(None)`（探测裁决）；
    /// 显式值 → `Ok(Some(..))`；未知值 loud 拒绝。
    pub fn parse_hypervisor(s: &str) -> Result<Option<Self>, String> {
        match s {
            "auto" => Ok(None),
            "kvm" => Ok(Some(QemuAccel::Kvm)),
            "whpx" => Ok(Some(QemuAccel::Whpx)),
            other => Err(format!(
                "executor.vm.hypervisor 未知值 {other:?}（合法值：auto | kvm | whpx）"
            )),
        }
    }
}

/// QEMU 机型装配条目（映射表的值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineSpec {
    /// `-machine` 值。
    pub machine: &'static str,
    /// `-cpu` 值（硬件加速档恒 `host`——直通宿主 CPU 特性）。
    pub cpu: &'static str,
    /// 接口在位 vs 当前产出（aarch64 条目 = `Reserved`）。
    pub production: Production,
}

/// 条目产出状态（接口预留的可观测化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Production {
    /// 当前批次真实产出。
    Produced,
    /// 接口在位、当前不产出（选了 = 诚实报不支持）。
    Reserved,
}

/// 机型/accel 映射表（`arch × accel` 全组合在位——编译期存在性由单测断言，
/// G0 门「aarch64 条目编译期存在性」）。
///
/// 机型选型：
/// - x86_64 → `q35`（现代 PCIe 机型，Debian generic cloud image 标准启动面）。
/// - aarch64 → `virt`（QEMU ARM 通用 virt 机型，KVM/WHPX arm 档的标准面）。
pub fn machine_spec(arch: GuestArch, accel: QemuAccel) -> MachineSpec {
    match (arch, accel) {
        (GuestArch::X86_64, QemuAccel::Kvm) => MachineSpec {
            machine: "q35",
            cpu: "host",
            production: Production::Produced,
        },
        (GuestArch::X86_64, QemuAccel::Whpx) => MachineSpec {
            machine: "q35",
            cpu: "host",
            production: Production::Produced,
        },
        // aarch64 条目：接口在位、当前不产出（恢复路径见模块文档）。
        (GuestArch::AArch64, QemuAccel::Kvm) => MachineSpec {
            machine: "virt",
            cpu: "host",
            production: Production::Reserved,
        },
        // Windows ARM64 的 WHPX 档（同「接口在位」纪律）。
        (GuestArch::AArch64, QemuAccel::Whpx) => MachineSpec {
            machine: "virt",
            cpu: "host",
            production: Production::Reserved,
        },
    }
}

/// guest 显示/音频装配（§5 边界与预留的扩展点落点）：
/// 当前固定 `-display none` + 无音频设备（headless 服务器形态——guest 内
/// 浏览器走 offscreen/Xvfb）。GPU/音频恢复路径 = 装配表加 virtio-gpu/audio
/// 段 + 配置 schema 加 display/audio 键；本函数是唯一改动点。
pub fn display_args() -> &'static [&'static str] {
    &["-display", "none"]
}

#[cfg(test)]
mod tests;
