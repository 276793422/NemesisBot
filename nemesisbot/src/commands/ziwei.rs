//! ziwei CLI —— 紫微系统命令面（总控 §8.2；feature=`ziwei` 门控）。
//!
//! 双闸之二（运行闸）：config `ziwei.enabled` 默认 false——开启前全部子命令
//! 诚实报未启用（对齐 WSAPI GATE_CLOSED 文案口径）。
//!
//! M1 范围：list / show / remove（数据层）+ attach / detach / status 实装
//! ——会话管理器常驻 gateway 进程（管道 server 生命周期与一次性 CLI 进程
//! 解耦），CLI 经 `/api/internal` 直连转发（estop 同款跨进程模式）。
//! invoke 已实装（M2，poll 一次性快照）；setup 步骤化子命令已实装（M3，
//! 六步向导：select-target → snapshot → analyze → diff/candidates →
//! annotate/define-struct/define-tool → commit）；refresh（找回）随 M5。

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::common;
use nemesis_ziwei::profile::ProfileStore;

/// 与 WSAPI `handlers/ziwei.rs::GATE_CLOSED_MSG` 同口径（IT 契约串）。
pub const GATE_CLOSED_MSG: &str = "紫微系统未启用（config ziwei.enabled=false）";

#[derive(Subcommand, Debug)]
pub enum ZiweiCommand {
    /// Profile 列表 + 状态
    List,
    /// Profile 详情（函数/结构/工具/锚/笔记）
    Show {
        /// profile id（zw-pro-xxxx）
        id: String,
    },
    /// 进入配置期（M3：步骤化子命令，可脚本化；向导状态在常驻 gateway）
    Setup {
        #[command(subcommand)]
        step: SetupStep,
    },
    /// 使用期附加（M1 实装；经 gateway 转发）
    Attach {
        /// profile id
        id: String,
        /// 目标进程 PID；缺省按 profile 目标 exe 名扫描（多命中列出让用户选）
        #[arg(long)]
        pid: Option<u32>,
    },
    /// 卸载会话（降险动作，无审批）
    Detach {
        /// profile id
        id: String,
    },
    /// 活动会话列表
    Status,
    /// 手动触发子工具（M2 实装）
    Invoke {
        /// profile id
        id: String,
        /// 子工具名（profile.tools[].tool_name）
        tool: String,
        /// 工具入参（JSON 对象，可选）
        args: Option<String>,
    },
    /// 手动触发附加验证链（M5 T5.1：hash→version→锚投票→状态机）
    Verify {
        /// profile id
        id: String,
        /// 目标 exe 路径（缺省 = profile.target.exe_path_hint）
        #[arg(long)]
        exe: Option<String>,
    },
    /// 对 stale profile 发起找回（M5 T5.3：逐锚重扫 + 找回报告）
    Refresh {
        /// profile id
        id: String,
        /// 目标 exe 路径（缺省 = profile.target.exe_path_hint）
        #[arg(long)]
        exe: Option<String>,
    },
    /// 找回确认（M5 T5.3 后半：用户逐条确认后入库；全接受 = 恢复 active）
    RefreshConfirm {
        /// profile id
        id: String,
        /// 目标 exe 路径（缺省 = profile.target.exe_path_hint）
        #[arg(long)]
        exe: Option<String>,
        /// 接受的函数语义名（逗号分隔；缺省 = 全部锚命中的函数）
        #[arg(long)]
        accept: Option<String>,
    },
    /// 吊销 profile（M5 状态机终态：不可自动复活；资产记录保留）
    Revoke {
        /// profile id
        id: String,
    },
    /// 删除 profile（用户资产，含确认）
    Remove {
        /// profile id
        id: String,
        /// 跳过交互确认（脚本用）
        #[arg(long)]
        yes: bool,
    },
}

/// 配置期六步向导的步骤化子命令（全部经 gateway `/api/internal ziwei_setup`
/// 转发——payload/会话/向导状态在常驻 gateway 进程，CLI 进程一次性）。
#[derive(Subcommand, Debug)]
pub enum SetupStep {
    /// 第 1 步：选目标进程（exe 文件名自动扫 PID，或直接给 PID）
    SelectTarget {
        /// 目标 exe 文件名（如 dummy-draw.exe）或 PID
        target: String,
    },
    /// 第 2 步：内存快照（头页 + .text/.rdata/.data）
    Snapshot,
    /// 第 3 步：分析（函数恢复 / xref / CRT 折叠）
    Analyze,
    /// 第 4a 步：差分窗口（当前内存 vs 快照；.data 变化页 = DataArea 线索）
    Diff {
        /// 取现值前等待秒数（默认 2，上限 60）
        #[arg(long)]
        wait_secs: Option<u64>,
        /// T4.4：观测窗并入临时 hook 计数（draft 已标注函数命中数）
        #[arg(long)]
        hook: bool,
    },
    /// 第 4b 步：定位候选清单（nopsled / 独特立即数 / 字符串引用 / 导入序列）
    Candidates,
    /// 第 5a 步：标注函数（强制 ≥1 锚；byte_pattern 锚兼作函数定位）
    Annotate {
        /// 函数 RVA（0x… 或十进制）
        rva: String,
        /// 语义名（如 push_record；后续 fn: 引用与锚都按它）
        semantic: String,
        /// byte_pattern 锚（十六进制字节串，空格分隔，如 "90 90 b8 8d 7c 6b 5a"）
        #[arg(long)]
        pattern: Option<String>,
        /// imm_const 锚（独特立即数，0x… 或十进制）
        #[arg(long)]
        imm: Option<String>,
        /// 调用约定（ms_x64 | stdcall | cdecl；缺省 ms_x64）
        #[arg(long)]
        conv: Option<String>,
        /// 函数注记
        #[arg(long)]
        note: Option<String>,
    },
    /// 第 5b 步：定义数据结构（JSON 文件：{"name","size","fields":[...]}）
    DefineStruct {
        /// 结构定义 JSON 文件路径
        file: String,
    },
    /// 第 5c 步：定义子工具（JSON 文件：{"tool_name","action":{...},...}）
    DefineTool {
        /// 工具定义 JSON 文件路径
        file: String,
    },
    /// 第 6 步：提交 draft 为正式 profile（全量校验 + 工具自动注册）
    Commit,
    /// 放弃向导（卸会话 + 清 draft/快照）
    Abort,
    /// 当前向导状态
    Status,
}

/// gateway `/api/internal` 直连（estop 同款跨进程模式；attach 注入+握手最长
/// 35s，预算给足 90s）。
async fn gateway_internal(
    home: &std::path::Path,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    let config_path = home.join("config.json");
    let cfg_str = std::fs::read_to_string(&config_path).map_err(|e| {
        anyhow::anyhow!(
            "读取 {} 失败: {e}（--local 启动的 gateway 需 `nemesisbot --local ziwei ...`）",
            config_path.display()
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
        anyhow::anyhow!("Gateway 未运行（找不到 state 文件）。先 `nemesisbot gateway` 启动它。")
    })?;
    if info.web_port <= 0 {
        anyhow::bail!("Gateway state 无效（web_port=0）");
    }
    let base_url = format!("http://{}:{}", info.web_host, info.web_port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()?;
    let resp = client
        .post(format!("{base_url}/api/internal"))
        .header("X-Auth-Token", auth_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Gateway {} 不可达: {e}", base_url))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("内部命令失败（{}）: {text}", status);
    }
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({})))
}

/// Windows tasklist 按镜像名扫 PID（CLI 侧 PID 自动发现；非 Windows 诚实报）。
#[cfg(windows)]
fn find_pids_by_exe(exe: &str) -> Result<Vec<u32>> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {exe}"), "/FO", "CSV", "/NH"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pids = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split("\",").collect();
        // CSV 形如 "dummy-draw.exe","1234","Console",...
        if cols.len() >= 2
            && cols[0].trim_matches('"').eq_ignore_ascii_case(exe)
            && let Ok(pid) = cols[1].trim_matches('"').parse::<u32>()
        {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

#[cfg(not(windows))]
fn find_pids_by_exe(_exe: &str) -> Result<Vec<u32>> {
    anyhow::bail!("紫微系统附加仅支持 Windows（注入/具名管道能力）")
}

/// verify/refresh 的目标 exe 路径解析（与 WSAPI resolve_exe 同口径）：
/// `--exe` 显式优先，缺省 = profile.target.exe_path_hint。
fn resolve_exe(
    exe: Option<&str>,
    profile: &nemesis_ziwei::profile::Profile,
) -> Result<std::path::PathBuf> {
    match exe {
        Some(p) => Ok(std::path::PathBuf::from(p)),
        None => profile
            .target
            .exe_path_hint
            .as_deref()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!("目标 exe 路径未知（--exe 与 profile.target.exe_path_hint 均缺）")
            }),
    }
}

pub async fn run(action: ZiweiCommand, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    // workspace 布局唯一拼接点在 nemesis-path（对齐 sandbox CLI 的
    // `<home>/workspace` 口径）。
    let workspace = nemesis_path::paths::workspace_dir(&home);

    // 运行闸：config ziwei.enabled（默认 false）。
    let cfg = nemesis_config::load_config(&home.join("config.json"))
        .map_err(|e| anyhow::anyhow!("读取配置失败: {e}"))?;
    let zcfg = cfg.ziwei.clone().unwrap_or_default();
    if !zcfg.enabled {
        bail!("{GATE_CLOSED_MSG}");
    }

    let store = ProfileStore::new(&workspace);
    match action {
        ZiweiCommand::List => {
            let profiles = store.list()?;
            if profiles.is_empty() {
                println!("（无 profile）——用 `nemesisbot ziwei setup <exe|pid>` 进入配置期");
                return Ok(());
            }
            println!(
                "{:<18} {:<6} {:<24} {:<10} {:<8} 工具数",
                "ID", "状态", "名称", "目标", "架构"
            );
            for p in profiles {
                println!(
                    "{:<18} {:<6} {:<24} {:<10} {:<8} {}",
                    p.profile_id, p.status, p.name, p.exe_name, p.machine, p.tool_count
                );
            }
        }
        ZiweiCommand::Show { id } => {
            let p = store.load(&id)?;
            println!("profile   : {}（{}）", p.profile_id, p.name);
            println!("状态      : {:?}", p.status);
            println!(
                "目标      : {}（{:?}，校验策略 {:?}）",
                p.target.exe_name, p.target.machine, p.target.verify_policy
            );
            println!("函数      : {} 个", p.functions.len());
            for f in &p.functions {
                println!(
                    "  - {}（{:?} @ {:?}）",
                    f.semantic, f.locate.kind, f.locate.pattern
                );
            }
            println!("结构      : {} 个", p.data_structures.len());
            for s in &p.data_structures {
                println!("  - {}（{} 字节，{} 字段）", s.name, s.size, s.fields.len());
            }
            println!("子工具    : {} 个", p.tools.len());
            for t in &p.tools {
                println!(
                    "  - {}（{:?}，run_policy={:?}）",
                    t.tool_name, t.action.primitive, t.run_policy
                );
            }
            if let Ok(anchors) = store.load_anchors(&id) {
                println!("锚        : {} 条", anchors.anchors.len());
            }
            let notes = store.load_notes(&id).unwrap_or_default();
            if !notes.is_empty() {
                println!("笔记      : {}", notes.lines().next().unwrap_or(""));
            }
        }
        ZiweiCommand::Setup { step } => setup_step(&home, &workspace, step).await?,
        ZiweiCommand::Attach { id, pid } => {
            let pid = match pid {
                Some(p) => p,
                None => {
                    // 按目标 exe 名扫描（Windows tasklist；0 命中/多命中诚实报）
                    let p = store.load(&id)?;
                    let pids = find_pids_by_exe(&p.target.exe_name)?;
                    match pids.len() {
                        0 => bail!(
                            "未发现运行中的 {}（目标未启动或 exe 名不符）",
                            p.target.exe_name
                        ),
                        1 => pids[0],
                        _ => bail!(
                            "发现 {} 个运行中的 {}：{pids:?}——用 --pid 指定",
                            pids.len(),
                            p.target.exe_name
                        ),
                    }
                }
            };
            let resp = gateway_internal(
                &home,
                serde_json::json!({ "cmd": "ziwei_attach", "profile_id": id, "pid": pid }),
            )
            .await?;
            let session = resp.get("session").cloned().unwrap_or_default();
            println!("✓ 已附加（pid={pid}）");
            println!(
                "  profile : {}（{}）",
                session
                    .get("profile_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&id),
                session
                    .get("profile_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            );
            println!(
                "  架构    : {}  状态: {}",
                session.get("arch").and_then(|v| v.as_str()).unwrap_or("?"),
                session.get("state").and_then(|v| v.as_str()).unwrap_or("?")
            );
            println!(
                "  管道    : {}",
                session
                    .get("pipe_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
            );
        }
        ZiweiCommand::Detach { id } => {
            let resp = gateway_internal(
                &home,
                serde_json::json!({ "cmd": "ziwei_detach", "profile_id": id }),
            )
            .await?;
            println!(
                "✓ {}",
                resp.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("已卸载")
            );
        }
        ZiweiCommand::Status => {
            let resp =
                gateway_internal(&home, serde_json::json!({ "cmd": "ziwei_status" })).await?;
            let sessions = resp
                .get("sessions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if sessions.is_empty() {
                println!("（无活动会话）");
                return Ok(());
            }
            println!(
                "{:<18} {:<8} {:<8} {:<22} 附加时间",
                "PROFILE", "状态", "架构", "目标"
            );
            for s in &sessions {
                println!(
                    "{:<18} {:<8} {:<8} {:<22} {}",
                    s.get("profile_id").and_then(|v| v.as_str()).unwrap_or("?"),
                    s.get("state").and_then(|v| v.as_str()).unwrap_or("?"),
                    s.get("arch").and_then(|v| v.as_str()).unwrap_or("?"),
                    s.get("target_exe").and_then(|v| v.as_str()).unwrap_or("?"),
                    s.get("attached_at").and_then(|v| v.as_str()).unwrap_or("?"),
                );
            }
        }
        ZiweiCommand::Invoke { id, tool, args: _ } => {
            // M2：一次性 poll 快照（gateway internal 直连——会话在常驻进程里）。
            // call_fn/hook_capture 由服务端诚实报（随 M4 实装）。
            let resp = gateway_internal(
                &home,
                serde_json::json!({ "cmd": "ziwei_invoke", "profile_id": id, "tool": tool }),
            )
            .await?;
            if resp["status"] == "error" {
                bail!(
                    "{}",
                    resp.get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("未知错误")
                );
            }
            let result = resp.get("result").cloned().unwrap_or_default();
            println!("✓ 快照（{}/{}）", result["profile"], result["tool"]);
            if let Some(data) = result.get("data")
                && let Ok(pretty) = serde_json::to_string_pretty(data)
            {
                println!("{pretty}");
            }
        }
        ZiweiCommand::Verify { id, exe } => {
            // M5 T5.1：验证链本地执行（纯文件面 + profile 存储，不依赖 gateway）。
            let profile = store.load(&id)?;
            let exe_path = resolve_exe(exe.as_deref(), &profile)?;
            let out =
                nemesis_ziwei::verify::verify_profile(&store, &workspace, &profile, &exe_path)
                    .map_err(anyhow::Error::msg)?;
            if let Some(updated) = &out.profile_updated {
                store.save(updated, None)?;
            }
            let r = &out.report;
            println!("✓ 验证完成（{id}）");
            println!(
                "  裁决    : {:?}（hash {}）",
                r.verdict,
                if r.hash_match { "一致" } else { "不一致" }
            );
            if let Some(a) = &r.anchors {
                println!(
                    "  锚投票  : {}/{} 命中（阈值 {}，{}）",
                    a.hit,
                    a.supported,
                    a.threshold,
                    if a.passed { "过" } else { "不过" }
                );
            }
            if let Some(p) = &out.report_path {
                println!("  报告    : {}", p.display());
            }
            for n in &r.notes {
                println!("  注记    : {n}");
            }
            if r.verdict == nemesis_ziwei::verify::Verdict::Stale {
                eprintln!("  下一步  : nemesisbot ziwei refresh {id} --exe <目标exe>");
            }
        }
        ZiweiCommand::Refresh { id, exe } => {
            // M5 T5.3：找回重扫 + 报告（本地执行；授权点在 confirm）。
            let profile = store.load(&id)?;
            let exe_path = resolve_exe(exe.as_deref(), &profile)?;
            let out = nemesis_ziwei::verify::recover_start(&store, &workspace, &id, &exe_path)
                .map_err(anyhow::Error::msg)?;
            println!("✓ 找回扫描完成（{id}）");
            println!(
                "  总判定  : {}",
                out["report"]["verdict"]["verdict"].as_str().unwrap_or("?")
            );
            if let Some(md) = out.get("report_md").and_then(|v| v.as_str()) {
                println!("  报告    : {md}");
            }
            println!("  下一步  : 核对报告后逐条确认——");
            println!(
                "            nemesisbot ziwei refresh-confirm {id} --exe <目标exe> --accept <函数1,函数2|省略=全部命中>"
            );
        }
        ZiweiCommand::RefreshConfirm { id, exe, accept } => {
            // M5 T5.3 后半：用户显式 confirm = 授权凭据。
            let profile = store.load(&id)?;
            let exe_path = resolve_exe(exe.as_deref(), &profile)?;
            let accept_list: Vec<String> = match accept.as_deref() {
                Some(s) if !s.trim().is_empty() => {
                    s.split(',').map(|x| x.trim().to_string()).collect()
                }
                _ => {
                    // 缺省 = 全部锚命中的函数（找回报告里的可恢复集）。
                    let fns = nemesis_ziwei::verify::hit_functions(&store, &id, &exe_path)
                        .map_err(anyhow::Error::msg)?;
                    if fns.is_empty() {
                        bail!("无可接受函数（全部锚 miss——走重新配置向导）");
                    }
                    fns
                }
            };
            let out = nemesis_ziwei::verify::recover_confirm(
                &store,
                &workspace,
                &id,
                &exe_path,
                &accept_list,
            )
            .map_err(anyhow::Error::msg)?;
            println!(
                "✓ 找回确认（{id}）：{}",
                if out["all_recovered"].as_bool().unwrap_or(false) {
                    "全部恢复（profile 回 active）"
                } else {
                    "部分恢复（拒绝/未命中项保持 stale）"
                }
            );
            if let Some(n) = out["regenerated_anchors"].as_u64() {
                println!("  重生锚  : {n}");
            }
            for f in out["per_function"].as_array().unwrap_or(&vec![]) {
                println!(
                    "  - {} : {}",
                    f["function"].as_str().unwrap_or("?"),
                    if f["recovered"].as_bool().unwrap_or(false) {
                        "已恢复"
                    } else {
                        "未恢复"
                    }
                );
            }
        }
        ZiweiCommand::Revoke { id } => {
            // M5 状态机终态：Active/Stale → Revoked；Revoked 不可自动复活
            // （重建或删除后重配）。本地执行（纯 profile 存储面）。
            let mut profile = store.load(&id)?;
            profile
                .status
                .can_transition_to(nemesis_ziwei::profile::ProfileStatus::Revoked)
                .map_err(anyhow::Error::msg)?;
            profile.status = nemesis_ziwei::profile::ProfileStatus::Revoked;
            store.save(&profile, None)?;
            println!("✓ 已吊销 {id}（终态：验证/附加一律拒绝；记录保留可查 `ziwei show {id}`）");
        }
        ZiweiCommand::Remove { id, yes } => {
            // 先确认存在（NotFound 原样透出）。
            let p = store.load(&id)?;
            if !yes {
                eprintln!(
                    "将删除 profile {}（{}）及其全部快照/运行记录。",
                    p.profile_id, p.name
                );
                eprintln!("确认请加 --yes 重新执行。");
                return Ok(());
            }
            store.delete(&id)?;
            println!("已删除 {}", id);
        }
    }
    Ok(())
}

/// 配置期向导步骤执行（全部经 gateway `/api/internal ziwei_setup` 转发；
/// 结果 JSON 缩进打印，可脚本化消费）。
async fn setup_step(
    home: &std::path::Path,
    workspace: &std::path::Path,
    step: SetupStep,
) -> Result<()> {
    let (step_name, data): (&str, serde_json::Value) = match &step {
        SetupStep::SelectTarget { target } => {
            // target：纯数字 = PID；否则按 exe 文件名扫（CLI 侧 tasklist，
            // 多命中列出诚实拒绝——选哪个是用户决策）
            let pid = target.parse::<u32>().ok();
            if let Some(pid) = pid {
                ("select_target", serde_json::json!({ "pid": pid }))
            } else {
                let exe = std::path::Path::new(target)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(target.as_str());
                let pids = find_pids_by_exe(exe)?;
                match pids.len() {
                    0 => anyhow::bail!("未发现运行中的 {exe}（目标未启动？）"),
                    1 => ("select_target", serde_json::json!({ "pid": pids[0] })),
                    n => anyhow::bail!("发现 {n} 个运行中的 {exe}：{pids:?}——用 PID 指定"),
                }
            }
        }
        SetupStep::Snapshot => ("snapshot", serde_json::json!({})),
        SetupStep::Analyze => ("analyze", serde_json::json!({})),
        SetupStep::Diff { wait_secs, hook } => (
            "diff_window",
            match (wait_secs, hook) {
                (Some(w), true) => serde_json::json!({ "wait_secs": w, "hook": true }),
                (Some(w), false) => serde_json::json!({ "wait_secs": w }),
                (None, true) => serde_json::json!({ "hook": true }),
                (None, false) => serde_json::json!({}),
            },
        ),
        SetupStep::Candidates => ("candidates", serde_json::json!({})),
        SetupStep::Annotate {
            rva,
            semantic,
            pattern,
            imm,
            conv,
            note,
        } => {
            let rva_v = parse_rva(rva)?;
            let mut anchors = Vec::new();
            if let Some(p) = pattern {
                anchors.push(serde_json::json!({ "kind": "byte_pattern", "pattern": p }));
            }
            if let Some(v) = imm {
                let val = parse_rva(v)?; // 同样接受 0x…/十进制
                anchors.push(serde_json::json!({ "kind": "imm_const", "value": val }));
            }
            if anchors.is_empty() {
                anyhow::bail!(
                    "标注必须带 ≥1 条锚：--pattern <hex> 或 --imm <value>（候选见 `setup candidates`）"
                );
            }
            let mut body = serde_json::json!({
                "rva": rva_v,
                "semantic": semantic,
                "anchors": anchors,
            });
            if let Some(c) = conv {
                body["conv"] = serde_json::json!(c);
            }
            if let Some(n) = note {
                body["note"] = serde_json::json!(n);
            }
            ("annotate", body)
        }
        SetupStep::DefineStruct { file } => ("define_struct", read_json_file(file, workspace)?),
        SetupStep::DefineTool { file } => ("define_tool", read_json_file(file, workspace)?),
        SetupStep::Commit => ("commit", serde_json::json!({})),
        SetupStep::Abort => ("abort", serde_json::json!({})),
        SetupStep::Status => ("status", serde_json::json!({})),
    };
    let resp = gateway_internal(
        home,
        serde_json::json!({ "cmd": "ziwei_setup", "step": step_name, "data": data }),
    )
    .await?;
    if resp["status"] == "error" {
        bail!(
            "{}",
            resp.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误")
        );
    }
    let result = resp.get("result").cloned().unwrap_or_default();
    println!("✓ setup {step_name}");
    if let Ok(pretty) = serde_json::to_string_pretty(&result) {
        println!("{pretty}");
    }
    Ok(())
}

/// RVA/立即数解析（0x… 十六进制或十进制）。
fn parse_rva(raw: &str) -> Result<u64> {
    let t = raw.trim();
    let v = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .map(|h| u64::from_str_radix(h, 16))
        .unwrap_or_else(|| t.parse::<u64>())?;
    Ok(v)
}

/// 结构/工具定义 JSON 文件读取（相对路径按 workspace 解析）。
fn read_json_file(file: &str, workspace: &std::path::Path) -> Result<serde_json::Value> {
    let path = std::path::Path::new(file);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let bytes = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("读定义文件失败（{}）：{e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("定义文件 JSON 解析失败：{e}"))
}
