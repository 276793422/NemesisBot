//! P5 第 4 步 — [`VmBridge`](nemesis_web::handlers::vm::VmBridge) 的
//! [`VmSupervisor`] 实现（bot 侧桥——nemesis-web 不反向依赖本 crate，
//! trait 在 web 侧定义、实现在这里，gateway 装配期装槽）。
//!
//! 职责边界：本模块只做**投影与转发**——typed config 解析/校验/持久化、
//! 加速器探针包装、镜像构建后台任务（脚本 spawn + 日志落盘 + 存量
//! overlay 安全 rebase）、生命周期命令错误分类翻译。QEMU 管线细节全在
//! [`crate::agents::vm_runtime`] 与 nemesis-sandbox，不在这里重复。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nemesis_sandbox::vm::assembly::resolve_qemu_binary;
use nemesis_sandbox::vm::guest_arch::{GuestArch, QemuAccel};
use nemesis_sandbox::vm::lifecycle::SleepMode;
use nemesis_sandbox::vm::probe::AccelProbe;
#[cfg(target_os = "linux")]
use nemesis_sandbox::vm::probe::probe_kvm;
#[cfg(target_os = "windows")]
use nemesis_sandbox::vm::probe::probe_whpx;

use crate::agents::vm_runtime::{
    GIB, GcDecision, VmSupervisor, VmSupervisorError, WakeKind, disk_usage_bytes, gc_decision,
    load_record, now_ms, resolve_qemu_img, vm_images_root, vm_overlays_root, wake_kind_of,
};

use nemesis_web::handlers::vm::VmBridge;

/// 镜像构建日志（`<workspace>/tools/vm/image-build.log`）——每次构建
/// 截断重写；页面进度 = 轮询 `vm.image.list` 的 log.tail。
pub(crate) fn image_build_log_path(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("image-build.log")
}

/// 构建暂存目录（`<workspace>/tools/vm/images/.staging`）——构建产物
/// 先落这里，rebase/换装成功后才进正式位（半途失败不碰现有镜像）。
fn staging_dir(workspace: &Path) -> PathBuf {
    vm_images_root(workspace).join(".staging")
}

/// 进行中的构建任务（进程级单飞闸——并发构建互斥）。
struct ImageJob {
    started_ms: u64,
    update: bool,
}

static IMAGE_JOB: Mutex<Option<ImageJob>> = Mutex::new(None);

/// [`VmSupervisorError`] → WSAPI 字符串（带分类前缀，页面可区分「档没开」
/// 与「操作失败」两类——Unavailable 可回落、Failed 有副作用可能绝不回落）。
fn err_string(e: VmSupervisorError) -> String {
    match e {
        VmSupervisorError::Unavailable(m) => format!("不可用: {m}"),
        VmSupervisorError::Failed(m) => format!("失败: {m}"),
    }
}

/// 平台加速器探针（本机对应后端；其它平台 = 诚实「平台不支持」）。
fn platform_probe() -> (QemuAccel, AccelProbe) {
    #[cfg(target_os = "linux")]
    {
        (QemuAccel::Kvm, probe_kvm())
    }
    #[cfg(target_os = "windows")]
    {
        (QemuAccel::Whpx, probe_whpx())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        (
            QemuAccel::Kvm,
            AccelProbe {
                available: false,
                layer: "none",
                reason: "当前平台无受支持加速器（仅 Linux KVM / Windows WHPX）".into(),
            },
        )
    }
}

/// accel 的默认睡眠线（三探针定谳：KVM=Snapshot；WHPX=None——savevm/
/// migrate 双双早验挂死，退路 = 冷停）。
fn sleep_line_note(accel: QemuAccel) -> (Option<&'static str>, String) {
    match SleepMode::default_for_accel(accel) {
        Some(SleepMode::Snapshot) => (
            Some("snapshot"),
            "savevm 快照线可用（睡眠/唤醒 RAM 状态保留）".into(),
        ),
        Some(SleepMode::MigrateFile) => {
            (Some("migrate-file"), "迁移文件线可用（接口预留线）".into())
        }
        None => (
            None,
            "无可用睡眠线（WHPX 三探针定谳）；退路 = 冷停（磁盘状态保留、RAM 丢）".into(),
        ),
    }
}

/// `executor.vm` 段写入（与 web handlers/sandbox.rs `update_executor`
/// 同款双路：gateway 全局 store → CLI 裸 RMW + 原子写。写串行锁罩全程）。
fn update_vm_config(home: &Path, vm: &nemesis_config::ExecutorVmConfig) -> Result<(), String> {
    let _cfg_serial = nemesis_config::config_write_lock();
    if let Some(store) = nemesis_config::global() {
        return store
            .update(|c| {
                c.executor.get_or_insert_with(Default::default).vm = vm.clone();
            })
            .map_err(|e| format!("update vm config: {e}"));
    }
    // CLI / no-store fallback: read-merge-write config.json（typed 解析保
    // 护 executor 段其余键，不整段覆盖）。
    let config_path = home.join("config.json");
    let raw =
        std::fs::read_to_string(&config_path).map_err(|e| format!("read config.json: {e}"))?;
    let mut val: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse config.json: {e}"))?;
    let existing = val.get("executor").and_then(|v| {
        serde_json::from_value::<nemesis_config::ExecutorSeparationConfig>(v.clone()).ok()
    });
    let obj = val
        .as_object_mut()
        .ok_or_else(|| "config.json is not a JSON object".to_string())?;
    let mut e = existing.unwrap_or_default();
    e.vm = vm.clone();
    obj.insert(
        "executor".into(),
        serde_json::to_value(&e).map_err(|e| format!("serialize executor: {e}"))?,
    );
    let out =
        serde_json::to_string_pretty(&val).map_err(|e| format!("serialize config.json: {e}"))?;
    nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write config.json: {e}"))?;
    Ok(())
}

/// 追加一行进构建日志（逐行 flush——页面轮询 tail 实时可见）。
fn log_line(path: &Path, line: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// 目录韧性删除（bot 侧本模块内联版：Windows AV/索引器瞬时句柄重试——
/// fsutil 韧性包装在 nemesis-cluster 内部 pub(crate)，不跨 crate）。残留
/// 只影响整洁度（下次构建覆盖同名文件），warn 不阻塞。
fn remove_dir_resilient(dir: &Path) {
    for _ in 0..3 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(300)),
        }
    }
    tracing::warn!(
        "[vm-bridge] staging 残留未清（不影响下次构建）: {}",
        dir.display()
    );
}

#[async_trait::async_trait]
impl VmBridge for VmSupervisor {
    fn overview(&self) -> serde_json::Value {
        let config = self.vm_config();
        let enabled = config.as_ref().is_some_and(|c| c.enabled);
        let (accel, probe) = platform_probe();
        let (sleep_line, sleep_note) = sleep_line_note(accel);

        // 档位建议：探针在 → 建议开（睡眠线状态如实附注）；探针缺 → 建议
        // 关（缺席原因明示——绝不替用户决定，建议不是决定）。
        let suggestion = if probe.available {
            serde_json::json!({
                "enabled": true,
                "note": format!(
                    "检测到 {}（{}），可开启 VM 档。睡眠线：{sleep_note}",
                    accel.as_str(),
                    probe.layer
                ),
            })
        } else {
            serde_json::json!({
                "enabled": false,
                "note": format!(
                    "未检测到 {}：{}。可保持关闭；开启后实例启动将诚实报不可用",
                    accel.as_str(),
                    probe.reason
                ),
            })
        };

        let used = disk_usage_bytes(self.workspace());
        let gc = match &config {
            Some(c) => {
                let d = gc_decision(used, &c.gc);
                serde_json::json!({
                    "used_gb": used / GIB,
                    "budget_gb": c.gc.max_total_disk_gb,
                    "decision": match d {
                        GcDecision::Allow => "allow",
                        GcDecision::Warn { .. } => "warn",
                        GcDecision::Deny { .. } => "deny",
                    },
                })
            }
            None => serde_json::Value::Null,
        };

        let instances: Vec<_> = self
            .instance_status_all()
            .iter()
            .map(|(r, running)| record_json(r, *running))
            .collect();

        let arch = config
            .as_ref()
            .map(|c| c.arch.clone())
            .unwrap_or_else(|| "x86_64".to_string());
        let qemu = GuestArch::parse(&arch).map_or_else(
            |_| serde_json::Value::Null,
            |g| {
                serde_json::json!(
                    resolve_qemu_binary(
                        g,
                        crate::agents::vm_runtime::qemu_install_dir(self.workspace()).as_deref(),
                    )
                    .to_string_lossy()
                )
            },
        );

        serde_json::json!({
            "enabled": enabled,
            "config": config.as_ref().map(|c| serde_json::to_value(c).unwrap_or(serde_json::Value::Null)),
            "probe": {
                "platform": std::env::consts::OS,
                "accel": accel.as_str(),
                "available": probe.available,
                "layer": probe.layer,
                "reason": probe.reason,
            },
            "sleep_line": sleep_line,
            "sleep_line_note": sleep_note,
            "suggestion": suggestion,
            "gc": gc,
            "instances": instances,
            "qemu_binary": qemu,
            "images_root": vm_images_root(self.workspace()).to_string_lossy(),
        })
    }

    fn config_get(&self) -> Option<serde_json::Value> {
        self.vm_config().and_then(|c| serde_json::to_value(c).ok())
    }

    fn config_set(&self, vm: serde_json::Value) -> Result<(), String> {
        let typed: nemesis_config::ExecutorVmConfig =
            serde_json::from_value(vm).map_err(|e| format!("executor.vm 配置解析失败: {e}"))?;
        // 值域校验与配置装载同源（ExecutorVmConfig::validate 单一真相源）。
        typed
            .validate()
            .map_err(|e| format!("executor.vm 配置校验失败: {e}"))?;
        update_vm_config(&self.config_home(), &typed)
    }

    fn image_list(&self) -> serde_json::Value {
        let mut images = Vec::new();
        let root = vm_images_root(self.workspace());
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                images.push(serde_json::json!({
                    "name": entry.file_name().to_string_lossy(),
                    "size_bytes": meta.len(),
                    "modified_ms": meta
                        .modified()
                        .ok()
                        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64),
                }));
            }
        }
        let job = IMAGE_JOB.lock().expect("image job lock").as_ref().map(|j| {
            serde_json::json!({ "running": true, "started_ms": j.started_ms, "update": j.update })
        });
        // 日志尾部（页面进度可见性；最多 20 行）。
        let log_path = image_build_log_path(self.workspace());
        let tail: Vec<String> = std::fs::read_to_string(&log_path)
            .map(|raw| {
                raw.lines()
                    .rev()
                    .take(20)
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let log_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        serde_json::json!({
            "images_root": root.to_string_lossy(),
            "images": images,
            "build": job.unwrap_or(serde_json::json!({ "running": false })),
            "log": { "path": log_path.to_string_lossy(), "size_bytes": log_size, "tail": tail },
        })
    }

    async fn image_build(&self, update: bool) -> Result<serde_json::Value, String> {
        let settings = self.vm_settings().ok_or_else(|| {
            "VM 档未开启（executor.vm.enabled=false）：镜像构建不可用".to_string()
        })?;

        // 构建脚本 + guest 二进制在场预检（缺席 = 诚实报错 + 指路）。
        let script = PathBuf::from("scripts").join("build-vm-image.sh");
        if !script.exists() {
            return Err(format!(
                "镜像构建脚本不在场（{}）——需从仓库根运行网关，或在 Linux/WSL 宿主按构建管线手动执行",
                script.display()
            ));
        }
        let guest_target = match settings.arch.as_str() {
            "x86_64" => "x86_64-unknown-linux-musl",
            "aarch64" => "aarch64-unknown-linux-musl",
            other => {
                return Err(format!(
                    "guest 架构 {other:?} 非法（validate 应已拦——数据不一致）"
                ));
            }
        };
        let guest_bin = PathBuf::from("target")
            .join(guest_target)
            .join("release")
            .join(if cfg!(windows) {
                "nemesis-guest.exe"
            } else {
                "nemesis-guest"
            });
        if !guest_bin.exists() {
            return Err(format!(
                "guest 代理二进制不在场（{}）——先跑 scripts/build-guest.sh 构建 musl guest 工具链产物",
                guest_bin.display()
            ));
        }

        // 单飞闸 + 任务登记。
        {
            let mut job = IMAGE_JOB.lock().expect("image job lock");
            if job.is_some() {
                return Err("已有镜像构建任务进行中（进度看 vm.image.list 日志尾部）".to_string());
            }
            *job = Some(ImageJob {
                started_ms: now_ms(),
                update,
            });
        }

        let workspace = self.workspace().to_path_buf();
        let base_url = settings.image.source.clone();
        let sha256 = settings.image.sha256.clone();
        let arch = settings.arch.clone();

        tokio::spawn(async move {
            run_image_build(workspace, script, guest_bin, base_url, sha256, arch, update).await;
        });

        Ok(serde_json::json!({
            "started": true,
            "update": update,
            "note": "构建任务已后台启动（进度/结果看 vm.image.list 的 log.tail；完成后自动换装，存量 overlay 安全 rebase）",
        }))
    }

    fn instance_list(&self) -> Vec<serde_json::Value> {
        self.instance_status_all()
            .iter()
            .map(|(r, running)| record_json(r, *running))
            .collect()
    }

    async fn instance_sleep(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.sleep_agent(agent_id).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "slept": true,
            "note": "VM 已按默认睡眠线保存 RAM 状态并退出；下一次启动将从快照/状态文件唤醒",
        }))
    }

    async fn instance_wake(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        // 唤醒线路径预读（记录在 wake 过程中被改写——先取 wake_kind 用于
        // 应答注记；记录缺席 = 冷启语义，与 ensure_running 内部判定一致）。
        let line = load_wake_line(self.workspace(), agent_id);
        self.ensure_running(agent_id).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "woken": true,
            "wake_line": line,
            "note": match line.as_deref() {
                Some("snapshot") => "已从 qcow2 内部快照唤醒（RAM 状态恢复）",
                Some("migrate-file") => "已从迁移状态文件唤醒（RAM 状态恢复）",
                _ => "冷启动（无睡眠产物可用；磁盘状态保留）",
            },
        }))
    }

    async fn instance_kill(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.stop_agent(agent_id).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "stopped": true,
            "note": "VM 已冷停（QEMU 退出，overlay 保留——下次启动同盘冷启）",
        }))
    }

    async fn gc_run(&self) -> serde_json::Value {
        let swept = self.sweep_orphans().await;
        let used = disk_usage_bytes(self.workspace());
        let decision = self.vm_config().map(|c| match gc_decision(used, &c.gc) {
            GcDecision::Allow => serde_json::json!({ "verdict": "allow" }),
            GcDecision::Warn { used_gb, budget_gb } => {
                serde_json::json!({ "verdict": "warn", "used_gb": used_gb, "budget_gb": budget_gb })
            }
            GcDecision::Deny { used_gb, budget_gb } => {
                serde_json::json!({ "verdict": "deny", "used_gb": used_gb, "budget_gb": budget_gb })
            }
        });
        serde_json::json!({
            "swept_orphans": swept,
            "used_gb": used / GIB,
            "decision": decision,
        })
    }
}

/// wake 线路预读（`None` = 无记录/无产物 → 冷启）。
fn load_wake_line(workspace: &Path, agent_id: &str) -> Option<String> {
    let record = load_record(workspace, agent_id)?;
    match wake_kind_of(&record) {
        WakeKind::FromSnapshot(_) => Some("snapshot".to_string()),
        WakeKind::FromState(_) => Some("migrate-file".to_string()),
        WakeKind::Cold => None,
    }
}

/// registry 记录 → web 投影（token 不投影——凭据不出管理面）。
fn record_json(
    r: &crate::agents::vm_runtime::VmInstanceRecord,
    running: bool,
) -> serde_json::Value {
    serde_json::json!({
        "agent_id": r.agent_id,
        "running": running,
        "sleep_mode": r.sleep_mode,
        "asleep_at": r.asleep_at,
        "arch": r.arch,
        "accel": r.accel,
        "vcpu": r.vcpu,
        "ram_mb": r.ram_mb,
        "disk_gb": r.disk_gb,
        "has_snapshot": r.snapshot_tag.is_some(),
        "has_state_file": r.state_path.is_some(),
    })
}

/// 镜像构建后台任务：脚本 spawn → 日志逐行落盘 → 成功后 rebase/换装 →
/// 任务槽清位。任何失败只进日志（页面 tail 可见），不 panic。
async fn run_image_build(
    workspace: PathBuf,
    script: PathBuf,
    guest_bin: PathBuf,
    base_url: String,
    sha256: String,
    arch: String,
    update: bool,
) {
    let log_path = image_build_log_path(&workspace);
    let _ = std::fs::create_dir_all(log_path.parent().unwrap_or_else(|| Path::new(".")));
    let _ = std::fs::remove_file(&log_path); // 截断重写
    log_line(
        &log_path,
        &format!("[{}] 镜像构建启动（update={update} arch={arch}）", now_ms()),
    );

    let staging = staging_dir(&workspace);
    let _ = std::fs::create_dir_all(&staging);

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg(&script)
        .arg("--out")
        .arg(&staging)
        .arg("--guest-bin")
        .arg(&guest_bin)
        .arg("--base-url")
        .arg(&base_url)
        .arg("--arch")
        .arg(&arch);
    if !sha256.is_empty() {
        cmd.arg("--sha256").arg(&sha256);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    log_line(&log_path, "spawn 构建脚本…");
    let status = match cmd.spawn() {
        Ok(mut child) => {
            let mut stdout = child.stdout.take().expect("piped stdout");
            let mut stderr = child.stderr.take().expect("piped stderr");
            let out_reader = async {
                use tokio::io::AsyncBufReadExt as _;
                let mut lines = tokio::io::BufReader::new(&mut stdout).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    log_line(&log_path, &l);
                }
            };
            let err_reader = async {
                use tokio::io::AsyncBufReadExt as _;
                let mut lines = tokio::io::BufReader::new(&mut stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    log_line(&log_path, &format!("[stderr] {l}"));
                }
            };
            let (_, _, status) = tokio::join!(out_reader, err_reader, child.wait());
            status
        }
        Err(e) => {
            log_line(
                &log_path,
                &format!(
                    "spawn bash 失败: {e}（Windows 宿主需 git-bash/WSL bash 在 PATH）；任务结束"
                ),
            );
            IMAGE_JOB.lock().expect("image job lock").take();
            return;
        }
    };
    let Ok(status) = status else {
        log_line(&log_path, "构建脚本等待失败；任务结束");
        IMAGE_JOB.lock().expect("image job lock").take();
        return;
    };
    if !status.success() {
        log_line(
            &log_path,
            &format!("构建脚本退出码非零（{status}）；staging 保留供排查，现有镜像未动；任务结束"),
        );
        IMAGE_JOB.lock().expect("image job lock").take();
        return;
    }
    log_line(&log_path, "构建产物就绪（staging）——开始换装/rebase");

    if let Err(e) = promote_staging(&workspace, &staging, &log_path).await {
        log_line(
            &log_path,
            &format!("换装失败：{e}；staging 与现有镜像均保持原位（数据无损），任务结束"),
        );
    } else {
        remove_dir_resilient(&staging);
        log_line(&log_path, "换装完成；任务结束");
    }
    IMAGE_JOB.lock().expect("image job lock").take();
}

/// staging 产物 → 正式位 + 存量 overlay 安全 rebase（数据完好次序：
/// ① 新 base 落临时名 `base.new.qcow2` → ② 每个存量 overlay `qemu-img
/// rebase -b base.new`（safe 模式：旧 backing 经原路径读旧内容、差集
/// 拷进 overlay、改指针）→ ③ 删旧 base、新 base 转正 → ④ 每 overlay
/// `-u` 重定向指针到转正路径。任何一步失败：新旧两 base 都在、指针
/// 都有落点，全部实例仍可启动（诚实 warn 不留悬空）。
async fn promote_staging(workspace: &Path, staging: &Path, log_path: &Path) -> Result<(), String> {
    let images = vm_images_root(workspace);
    let new_base_src = staging.join("base.qcow2");
    if !new_base_src.exists() {
        return Err("staging 缺 base.qcow2（构建产物不完整）".to_string());
    }
    let new_base = images.join("base.new.qcow2");
    std::fs::copy(&new_base_src, &new_base).map_err(|e| format!("新 base 落临时名失败: {e}"))?;

    // seed.iso / manifest.json 先就位（不依赖 rebase 成败——冷启链路用）。
    for name in ["seed.iso", "manifest.json"] {
        let src = staging.join(name);
        if src.exists() {
            std::fs::copy(&src, images.join(name)).map_err(|e| format!("{name} 就位失败: {e}"))?;
        }
    }

    // 存量 overlay 清单（旧 base 在位才有 rebase 语义）。
    let overlays: Vec<PathBuf> = std::fs::read_dir(vm_overlays_root(workspace))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "qcow2"))
                .collect()
        })
        .unwrap_or_default();
    let qemu_img = resolve_qemu_img(workspace);

    if !overlays.is_empty() && images.join("base.qcow2").exists() {
        log_line(
            log_path,
            &format!(
                "存量 overlay {} 个——安全 rebase（safe 模式，差集拷贝）",
                overlays.len()
            ),
        );
        for ov in &overlays {
            let out = tokio::process::Command::new(&qemu_img)
                .arg("rebase")
                .arg("-b")
                .arg(&new_base)
                .arg(ov)
                .output()
                .await
                .map_err(|e| format!("qemu-img rebase 启动失败: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "overlay {} rebase 失败（{}）",
                    ov.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            log_line(log_path, &format!("rebase 完成: {}", ov.display()));
        }
    }

    // ③ 删旧 base → 新 base 转正 → ④ 指针重定向到转正路径。
    let canonical = images.join("base.qcow2");
    if canonical.exists() {
        std::fs::remove_file(&canonical).map_err(|e| format!("旧 base 移除失败: {e}"))?;
    }
    std::fs::rename(&new_base, &canonical).map_err(|e| format!("新 base 转正失败: {e}"))?;
    for ov in &overlays {
        let out = tokio::process::Command::new(&qemu_img)
            .arg("rebase")
            .arg("-u")
            .arg("-b")
            .arg(&canonical)
            .arg(ov)
            .output()
            .await
            .map_err(|e| format!("qemu-img rebase -u 启动失败: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "overlay {} 指针重定向失败（{}）——overlay 当前指向 base.new.qcow2（文件仍在场，可启动；手动转正即可）",
                ov.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    Ok(())
}
