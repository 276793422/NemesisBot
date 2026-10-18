//! P5 桌面级 VM 后端（第三类隔离后端：独立内核整机隔离）。
//!
//! 与既有两级后端的关系（轮次 1 拍板：互补不替代）：
//! - 沙盒 = 同一内核里的围栏（Sandboxie / landlock / bwrap / Seatbelt）；
//! - VM = 另一台完整的机器（独立内核 + 独立 OS，QEMU WHPX/KVM）；
//! - 逃逸面量级：沙盒机制 bug + 宿主内核漏洞 vs hypervisor 0day。
//!
//! 本模块总开关 = `executor.vm.enabled`（**默认 false**——关闭态 VM 路径
//! 零激活，铁律见 `docs/PLAN/2026-10-17_P5桌面级VM实施计划.md` §0）。
//! 硬件加速缺席 = 诚实 Unavailable；`executor.strict` 开启时拒绝执行而非
//! 降级假跑（对齐 U11 既有语义）。
//!
//! 子模块（按 P5 v2 实施计划逐步落地）：
//! - [`guest_arch`]：GuestArch / QemuAccel / QEMU 机型映射（aarch64 接口
//!   在位、当前不产出）。
//! - [`probe`]：宿主加速器/QEMU 二进制可用性探测（诚实缺席明原因）。
//! - [`qmp`]：QMP 客户端（握手/命令/事件跳过，fake-qmp 驱动回归）。
//! - [`assembly`]：QEMU 命令行装配纯函数（G2 宿主零暴露面闸落点）。
//! - [`overlay`]：qcow2 overlay 创建（base 差分；快照树预留点）。
//! - [`lifecycle`]：生命周期原语（裁决/spawn/**sleep→wake**/stop/鉴权连接/
//!   工具调用；睡眠双线 Snapshot/MigrateFile 机制面都实装，生产默认线
//!   KVM=Snapshot、WHPX=None（savevm/migrate 双双早验挂死，诚实不支持）
//!   ——三探针定谳见实施计划附录）。
//! - [`install`]：环境自举（QEMU ranged 并行下载 + SHA-512 双锚校验 +
//!   7z 直解落位，对齐 sandbox install 体验；第 10 层安装面）。

pub mod assembly;
pub mod guest_arch;
pub mod install;
pub mod lifecycle;
pub mod overlay;
pub mod probe;
pub mod qmp;
