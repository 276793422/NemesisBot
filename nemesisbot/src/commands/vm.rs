//! VM command — 环境自举 + 状态（第 10 层安装面；对齐 sandbox CLI 形态）。
//!
//! `install` / `enable-accel` / `build-image` 是系统侧动作：`executor.vm.enabled`
//! 关闭时诚实拒绝（与 WSAPI `vm.bootstrap.*` 同款闸——关闭态零激活铁律；
//! 闸判定面单一真相源 = [`crate::agents::vm_runtime::VmSupervisor::vm_settings`]）。
//! `status` 只读探针无闸（查询不是安装动作）。安装链本体在 nemesis-sandbox
//! `vm::install`（B1 下载链 / B2 平台链），本模块只做装配与呈现。

use std::time::Duration;

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::agents::vm_runtime::VmSupervisor;
use crate::common;

#[derive(Subcommand, Debug)]
pub enum VmCommand {
    /// 安装 QEMU 环境（按平台矩阵配置分派：archive=下载+SHA-512 校验+7z 直解
    /// 落位；packages=包管理器半自动链，无免密 sudo 时给出精确命令）。幂等
    /// （已装早退）。矩阵模板内嵌，`<workspace>/config/config.vm_qemu.json`
    /// 用户可改（缺则释放，有则直接用）。
    Install,
    /// 硬件加速引导（Windows=WHPX DISM 一键，UAC 提权由用户确认；
    /// Linux=打印 kmod / kvm 组引导命令，不替用户跑 sudo）。
    EnableAccel,
    /// 环境自举状态全量（QEMU 在场三态 / 加速器引导八态 / 镜像在场 /
    /// 安装任务槽——只读探针，无闸）。
    Status,
    /// 构建基础镜像（前台等待完成；构建脚本跑在 Linux/WSL bash 宿主）。
    BuildImage {
        /// 升级流程（staging 构建 → 存量 overlay 安全 rebase → 换装）。
        #[arg(long)]
        update: bool,
    },
    /// 项目工作副本同步状态（B9-3；只读——不开闸、不拽起睡眠实例）。
    SyncStatus {
        /// 项目 id 或名称。
        project: String,
    },
    /// 项目工作副本一键提交（B9-3；VM 侧修改回拉宿主 + 基线刷新；冲突
    /// 落 Dashboard 提问卡裁决——CLI 见冲突即报并指向 Dashboard）。
    Commit {
        /// 项目 id 或名称。
        project: String,
    },
    /// 强制重同步（B9-3；清空 VM 工作副本后以宿主当前内容整体覆盖重建
    /// 基线——VM 内未回传的改动将丢失；交互确认后执行）。
    Resync {
        /// 项目 id 或名称。
        project: String,
        /// 跳过交互确认（脚本 / CI 用）。
        #[arg(long)]
        yes: bool,
    },
}

pub async fn run(action: VmCommand, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let workspace = home.join("workspace");
    match action {
        VmCommand::Status => status(&home, &workspace),
        VmCommand::Install => {
            require_vm_enabled(&home, "环境安装")?;
            install(&workspace).await
        }
        VmCommand::EnableAccel => {
            require_vm_enabled(&home, "加速器引导")?;
            enable_accel().await
        }
        VmCommand::BuildImage { update } => build_image(&home, &workspace, update).await,
        VmCommand::SyncStatus { project } => {
            sync_via_gateway(&home, "vm_sync_status", &project).await
        }
        VmCommand::Commit { project } => sync_via_gateway(&home, "vm_sync_commit", &project).await,
        VmCommand::Resync { project, yes } => {
            if !yes {
                print!(
                    "[vm] 即将对项目「{project}」强制重同步：VM 工作副本将被清空，\
以宿主当前内容整体覆盖重建（VM 内未回传的改动将丢失）。确认请输入 yes："
                );
                {
                    use std::io::Write as _;
                    std::io::stdout().flush()?;
                }
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if line.trim() != "yes" {
                    println!("[vm] 已取消（未做任何改动）");
                    return Ok(());
                }
            }
            sync_via_gateway(&home, "vm_sync_resync", &project).await
        }
    }
}

/// 系统侧动作的统一闸（判定面单一真相源：`vm_settings()` typed 现读 +
/// enabled 过滤——与 WSAPI bootstrap 命令族同源）。
fn require_vm_enabled(home: &std::path::Path, what: &str) -> Result<()> {
    let sup = VmSupervisor::new(home.join("workspace"), common::config_path(home));
    if sup.vm_settings().is_some() {
        return Ok(());
    }
    bail!(
        "VM 档未开启（executor.vm.enabled=false）：{what}不可达——先在 Dashboard 虚拟化页开启总开关（或 config.json executor.vm.enabled=true）后重试"
    );
}

/// supervisor 现读辅助（status / build-image 共用）。
fn supervisor(home: &std::path::Path, workspace: &std::path::Path) -> VmSupervisor {
    VmSupervisor::new(workspace.to_path_buf(), common::config_path(home))
}

fn status(home: &std::path::Path, workspace: &std::path::Path) -> Result<()> {
    use nemesis_web::handlers::vm::VmBridge as _;
    let v = supervisor(home, workspace).bootstrap_status();
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

async fn install(workspace: &std::path::Path) -> Result<()> {
    use nemesis_sandbox::vm::install::{QemuInstallDirs, install_qemu_for_host_workspace};
    let runtime = workspace.join("tools").join("vm").join("runtime");
    // 矩阵真相源目录 = <workspace>/config/（缺则释放内嵌模板，有则直接用）。
    let ws_config = workspace.join("config");
    let dirs = QemuInstallDirs {
        install_dir: runtime.join("qemu"),
        work_dir: runtime.join("install-work"),
        // 复用 sandbox 的 7z 缓存（同一份 7z 服务两条安装链，零重复下载）。
        seven_zip_dir: workspace.join("tools").join("sandboxie").join("runtime"),
        ..Default::default()
    };
    let (report, meta) = install_qemu_for_host_workspace(&ws_config, &dirs).await?;
    println!(
        "[vm] 平台 {}：kind={}，矩阵来源 {:?}",
        meta.platform_key,
        meta.kind.as_str(),
        meta.source
    );
    if !meta.note.is_empty() {
        println!("[vm] note: {}", meta.note);
    }
    print_report(&report);
    Ok(())
}

fn print_report(report: &nemesis_sandbox::vm::install::QemuInstallReport) {
    if report.already_installed {
        println!("[vm] QEMU 已就绪（幂等早退，未重装）");
    } else {
        println!("[vm] 安装完成");
    }
    println!("  qemu-system: {}", report.qemu_system.display());
    println!("  qemu-img:    {}", report.qemu_img.display());
    println!("  version:     {}", report.version);
    println!("[vm] 复检：nemesisbot vm status");
}

async fn enable_accel() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use nemesis_sandbox::vm::install::enable_accel_windows;
        let msg = enable_accel_windows()?;
        println!("[vm] {msg}");
        println!("[vm] 复检：nemesisbot vm status（功能挂起重启则需重启 Windows）");
    }
    #[cfg(target_os = "linux")]
    {
        use nemesis_sandbox::vm::install::{modprobe_kvm_command, usermod_kvm_command};
        use nemesis_sandbox::vm::probe::{
            AccelBootstrap, classify_accel_bootstrap, collect_accel_facts,
        };
        let facts = collect_accel_facts();
        match classify_accel_bootstrap(&facts) {
            AccelBootstrap::Ok => println!("[vm] 加速器已就绪（无需引导）"),
            AccelBootstrap::NeedsKmod { module } => {
                println!("[vm] KVM 内核模块未加载——终端执行后复检：");
                println!("  {}", modprobe_kvm_command(module));
                println!("[vm] 复检：nemesisbot vm status");
            }
            AccelBootstrap::NeedsGroup => {
                println!("[vm] /dev/kvm 当前用户不可写——终端执行后重新登录生效：");
                println!("  {}", usermod_kvm_command("$USER"));
            }
            AccelBootstrap::NeedsReboot => {
                println!("[vm] 功能已启用但挂起重启——重启后复检：nemesisbot vm status")
            }
            AccelBootstrap::NeedsBios => {
                println!("[vm] 功能已启用但加速器仍缺席——进 BIOS 开启虚拟化（Intel VT-x / AMD-V）")
            }
            other => bail!("当前态（{other:?}）无自动引导动作——详见 nemesisbot vm status 全量诊断"),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        bail!(
            "当前平台（{}）无受支持加速器（仅 Linux KVM / Windows WHPX）",
            std::env::consts::OS
        );
    }
    Ok(())
}

async fn build_image(
    home: &std::path::Path,
    workspace: &std::path::Path,
    update: bool,
) -> Result<()> {
    use nemesis_web::handlers::vm::VmBridge as _;
    let sup = supervisor(home, workspace);
    let settings = sup.vm_settings().ok_or_else(|| {
        anyhow::anyhow!("VM 档未开启（executor.vm.enabled=false）：镜像构建不可用")
    })?;
    let inputs = crate::agents::vm_bridge::image_build_inputs(&settings, workspace)
        .map_err(anyhow::Error::msg)?;
    println!(
        "[vm] 镜像构建启动（update={update} arch={} 脚本={}）",
        inputs.arch,
        inputs.script.display()
    );
    // 与 WSAPI 同一执行体（后台任务 + 日志落盘 + 单飞闸）；CLI 额外前台
    // 等任务槽清位，然后以成功收尾日志行裁决成败（单一真相源常量）。
    sup.image_build(update).await.map_err(anyhow::Error::msg)?;
    println!(
        "[vm] 构建进行中（前台等待；日志 {}）",
        crate::agents::vm_bridge::image_build_log_path(workspace).display()
    );
    while crate::agents::vm_bridge::image_job_running() {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let log = std::fs::read_to_string(crate::agents::vm_bridge::image_build_log_path(workspace))
        .unwrap_or_default();
    let tail: Vec<&str> = log
        .lines()
        .rev()
        .take(15)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    for l in &tail {
        println!("  | {l}");
    }
    if log.contains(crate::agents::vm_bridge::IMAGE_BUILD_DONE_LINE) {
        println!("[vm] 构建完成（换装/存量 overlay rebase 已执行）");
        Ok(())
    } else {
        bail!("镜像构建未成功（日志尾部如上；staging 保留供排查，现有镜像未动）")
    }
}

/// B9-3 同步命令的 gateway 转发（VM 实例常驻 gateway 进程，CLI 进程不
/// 持有 supervisor——与 estop 同一条 /api/internal 通道；请求-响应形态，
/// 恒 200 `{status: ok|error, ...}` 按体解析：业务失败（项目不存在 /
/// 非 VM 档 / 冲突）不是传输错误）。
async fn sync_via_gateway(home: &std::path::Path, cmd: &str, project: &str) -> Result<()> {
    let config_path = home.join("config.json");
    let cfg_str = std::fs::read_to_string(&config_path).map_err(|e| {
        anyhow::anyhow!(
            "无法读取 {}：{}。若 gateway 以 --local 启动，请用：nemesisbot --local vm commit {project}",
            config_path.display(),
            e
        )
    })?;
    let cfg: serde_json::Value = serde_json::from_str(&cfg_str)?;
    let auth_token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let state_path =
        nemesis_path::resolve_gateway_state_path_in_workspace(&crate::common::workspace_path(home));
    let info = crate::commands::dashboard::read_gateway_state(&state_path).ok_or_else(|| {
        anyhow::anyhow!("Gateway 未运行（找不到 state 文件）。先用 `nemesisbot gateway` 启动它。")
    })?;
    if info.web_port <= 0 {
        bail!("Gateway state 无效（web_port=0）");
    }
    let base_url = format!("http://{}:{}", info.web_host, info.web_port);
    crate::commands::dashboard::check_health(&base_url)
        .await
        .map_err(|_| anyhow::anyhow!("Gateway 在 {base_url} 不可达。确认它正在跑。"))?;

    // 全量重建可能传大 tar（loopback 但量大），超时放宽到 30 分钟——
    // 诚实等待优先于假超时。
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1800))
        .build()?;
    let url = format!("{base_url}/api/internal");
    let resp = client
        .post(&url)
        .header("X-Auth-Token", &auth_token)
        .json(&serde_json::json!({ "cmd": cmd, "project": project }))
        .send()
        .await?;
    let status = resp.status();
    let body: serde_json::Value = serde_json::from_str(&resp.text().await.unwrap_or_default())
        .unwrap_or_else(|_| serde_json::json!({}));
    if !status.is_success() {
        bail!("Internal command failed: {status} {body}");
    }
    if body["status"].as_str() != Some("ok") {
        bail!(
            "{}",
            body["error"]
                .as_str()
                .unwrap_or("未知错误（响应缺 error 字段）")
        );
    }
    println!("{}", serde_json::to_string_pretty(&body["result"])?);
    Ok(())
}
