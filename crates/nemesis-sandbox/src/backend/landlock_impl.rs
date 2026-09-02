//! Linux landlock 自装后端（U11 链条第一档）。
//!
//! landlock 是非特权进程 LSM：进程对**自身**施加 FS 访问限制，后代进程
//! 继承，**不可逆**——与 executor 子进程模型天然契合（子进程启动时装上，
//! 工具层无论怎么 spawn 都出不去）。见 `super` 模块文档的诚实边界
//! （FS-only、网络不可强制）。

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};

use super::{Availability, Enforcement, SandboxBackend, SandboxConf};

/// landlock 自装后端（无状态；每次 `apply_to_self` 建独立 ruleset）。
pub struct LandlockBackend;

impl LandlockBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LandlockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for LandlockBackend {
    fn name(&self) -> &str {
        "landlock"
    }

    fn form(&self) -> super::BackendForm {
        super::BackendForm::SelfApply
    }

    fn availability(&self) -> Availability {
        // 探测：HardRequirement + V1 基础权限集（Linux 5.13 全集）。landlock
        // crate 的 Ruleset::default() 会探测运行内核；HardRequirement 模式下
        // 内核不支持（ABI::Unsupported：未编入/启动参数禁用）→ 立即 Err
        // （crate 文档：immediately inform about unsupported Landlock features）。
        // 不装规则、无副作用、可反复调用。
        match Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(ABI::V1))
        {
            Ok(_) => Availability::Full,
            Err(err) => Availability::Unavailable(format!("kernel landlock probe failed: {err}")),
        }
    }

    fn apply_to_self(&self, conf: &SandboxConf) -> Result<Enforcement, String> {
        // 目标 ABI = V3（kernel 6.2+）：FS 权限全集的稳定点（V4+ 增量是
        // AccessNet/IOCTL 等，本后端不用）。BestEffort 在更老内核自动降到
        // 内核与请求的交集 → PartiallyEnforced → Partial 语义的来源。
        let abi = ABI::V3;
        let read_access = AccessFs::from_read(abi);
        let all_access = AccessFs::from_all(abi);

        let mut ruleset = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            // deny-by-default：handle 全部 FS 权限（无规则即全拒），再逐根放行。
            .handle_access(all_access)
            .map_err(|e| format!("landlock handle_access: {e}"))?
            .create()
            .map_err(|e| format!("landlock create ruleset: {e}"))?;

        for root in &conf.read_exec_roots {
            let fd = PathFd::new(root).map_err(|e| format!("landlock open {root:?}: {e}"))?;
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, read_access))
                .map_err(|e| format!("landlock read rule {root:?}: {e}"))?;
        }
        for root in &conf.writable_roots {
            let fd = PathFd::new(root).map_err(|e| format!("landlock open {root:?}: {e}"))?;
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, all_access))
                .map_err(|e| format!("landlock write rule {root:?}: {e}"))?;
        }

        // 装上（不可逆，只影响本进程树）。
        let status = ruleset
            .restrict_self()
            .map_err(|e| format!("landlock restrict_self: {e}"))?;

        // FS-only LSM：allow_network=false 在本后端不可强制（ABI 4+ 的
        // AccessNet 仅覆盖 TCP 且需 kernel 6.7+；ABI≤V3 上 from_all 返回空集、
        // handle_access 直接报错——见 landlock::AccessNet 文档）。禁网期望交给
        // bwrap 后端（--unshare-net），这里诚实记缺口。
        let mut gaps: Vec<String> = Vec::new();
        if !conf.allow_network {
            gaps.push(
                "network: landlock is a filesystem LSM — allow_network=false is NOT \
                 enforced by this backend (bwrap --unshare-net can)"
                    .to_string(),
            );
        }

        match status.ruleset {
            RulesetStatus::FullyEnforced => {
                if gaps.is_empty() {
                    Ok(Enforcement::Full)
                } else {
                    Ok(Enforcement::Partial(gaps))
                }
            }
            RulesetStatus::PartiallyEnforced => {
                gaps.insert(
                    0,
                    "kernel landlock ABI lower than requested — some access rights \
                     downgraded (BestEffort)"
                        .to_string(),
                );
                Ok(Enforcement::Partial(gaps))
            }
            // non_exhaustive 兜底 + NotEnforced（探测漏过的不支持内核等）：
            // 报 Err 让调用方走无盒降级——不能假装有盒。
            _ => Err(format!(
                "landlock ruleset not enforced ({:?}) — treating as unsandboxed",
                status.ruleset
            )),
        }
    }
}
