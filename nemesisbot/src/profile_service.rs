//! 用户习惯画像周期任务（自进化升级 B 线，2026-10-14）。
//!
//! 小时级 sweep：统计通道（session_logs 文本规则）+ LLM 反思通道（每 20 次
//! sweep 一次）+ 衰减推进。纪律：仅 enabled；仅向前（enabled_at 之后的
//! 消息）；安全硬边界过滤在 user_profile::upsert_habit / render 内层兜底。
//!
//! 装配：gateway 侧 `ensure_sweep_spawned`（OnceLock 一次守卫，loop 重建
//! 多次调用只 spawn 一份）；LLM caller 每 tick 从 config 现场解析（模型热切
//! 即时生效，无陈旧 Arc）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nemesis_agent::user_profile::{self, UserProfile, stat_candidates, upsert_habit};

static SWEEP_SPAWNED: OnceLock<()> = OnceLock::new();

/// gateway 装配点调用：保证 sweep 任务只 spawn 一份。
pub fn ensure_sweep_spawned(home: PathBuf) {
    if SWEEP_SPAWNED.set(()).is_err() {
        return; // 已 spawn（loop 重建多次调用只生效一次）
    }
    let handle = tokio::runtime::Handle::current();
    tokio::spawn(async move {
        // 间隔每次循环从 config 现读（user_profile.stats_scan_interval_secs，
        // 缺省 3600s）——改配置无需重启。sleep 而非 interval：周期可变。
        let mut sweeps: u64 = 0;
        loop {
            let secs = tokio::task::spawn_blocking({
                let h = home.clone();
                move || load_scan_interval_secs(&h)
            })
            .await
            .unwrap_or(3600);
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            sweeps += 1;
            // sweep_once 内含 LLM 调用（反思通道）——阻塞段放 blocking 线程，
            // LLM 调用经 Handle::block_on 桥回运行时。
            let h = home.clone();
            let s = sweeps;
            let rh = handle.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || sweep_once(&h, s, &rh))
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

/// 单次 sweep：开关关 = no-op。返回本 tick 处理的消息数（诊断用）。
pub fn sweep_once(home: &Path, sweeps: u64, rt: &tokio::runtime::Handle) -> Result<usize, String> {
    let cfg = nemesis_config::load_config(&crate::common::config_path(home))
        .map_err(|e| format!("load config: {e}"))?;
    let Some(up_cfg) = cfg.user_profile.as_ref() else {
        return Ok(0);
    };
    if !up_cfg.enabled {
        return Ok(0);
    }
    let workspace = crate::common::workspace_path(home);
    let mut profile: UserProfile = user_profile::load_profile(&workspace);
    // 1. 增量采集：证据窗 = 滚动 24h（`sweep_window_start_ms`：锚点夹紧 +
    //    水线停机下探）。窗内样本量是统计规则（n≥10 闸）的凑闸前提，小时
    //    级增量批对稀疏用户永远凑不齐——故窗口下限不是水线而是滚动 24h。
    //    已在窗内的消息每 sweep 至多重复 upsert 一次（hits 有界虚增，换
    //    衰减语义正确：last_seen 只反映 24h 内的重现，超窗即进入衰减）。
    let collect_started_ms = chrono::Utc::now().timestamp_millis();
    let Some(window_start) = user_profile::sweep_window_start_ms(&profile, collect_started_ms)
    else {
        return Ok(0); // 从未开启或锚点损坏 = 不采集
    };
    let messages = collect_user_messages_after(&workspace, window_start)?;

    // 2. 统计通道。
    let now = chrono::Local::now().to_rfc3339();
    for candidate in stat_candidates(&messages, &now) {
        upsert_habit(&mut profile, candidate);
    }

    // 3. 反思通道（每 N 次 sweep 一次，config 可调缺省 20；LLM 走 judge
    //    同款回落链，从 config 现场解析）。
    let reflect_every = up_cfg.reflect_every_sweeps_or(20) as u64;
    if sweeps.is_multiple_of(reflect_every)
        && !messages.is_empty()
        && let Some(caller) = build_caller(home, &workspace)?
    {
        match reflect_semantic_habits(
            &caller,
            &messages,
            rt,
            up_cfg.confidence_threshold_or(user_profile::CONFIDENCE_THRESHOLD),
        ) {
            Ok(candidates) => {
                for c in candidates {
                    upsert_habit(&mut profile, c);
                }
            }
            Err(e) => tracing::warn!("[UserProfile] reflect failed: {e}"),
        }
    }

    // 4. 衰减（config 可调天数）+ 截断 + 推进水线 + 落盘（同一次原子写：
    //    中途失败水线不推进，下轮重扫本轮消息——at-least-once 语义）。
    user_profile::decay_sweep_with_days(
        &mut profile,
        chrono::Local::now(),
        up_cfg.decay_days_or(user_profile::DECAY_DAYS),
    );
    user_profile::prune_and_cap_with_max(
        &mut profile,
        up_cfg.max_entries_or(user_profile::MAX_ENTRIES),
    );
    profile.last_sweep_ms = Some(collect_started_ms);
    user_profile::save_profile(&workspace, &profile)?;

    Ok(messages.len())
}

/// 扫 session_logs 全部 `*.jsonl`，收集 `role=="user"` 且时间戳不早于
/// `window_start_ms`（证据窗起点，epoch 毫秒）的消息文本。
fn collect_user_messages_after(
    workspace: &Path,
    window_start_ms: i64,
) -> Result<Vec<String>, String> {
    let dir = workspace.join("logs").join("session_logs");
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read session_logs: {e}"))?;
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.extension().map(|e| e != "jsonl").unwrap_or(true) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in content.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v.get("role").and_then(|r| r.as_str()) != Some("user") {
                continue;
            }
            // 行时间戳 = `timestamp`（RFC3339 字符串）；解析失败视为早于
            // 水线跳过（仅向前语义：无时间锚的数据不参与画像）。
            let ts = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let Some(ts) = ts else { continue };
            if ts.timestamp_millis() < window_start_ms {
                continue;
            }
            if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                out.push(c.to_string());
            }
        }
    }
    Ok(out)
}

/// 反思通道：LLM 归纳语义习惯 → 候选条目。`threshold` = 置信度闸
/// （config user_profile.confidence_threshold，缺省 0.7）。
fn reflect_semantic_habits(
    caller: &std::sync::Arc<dyn nemesis_forge::reflector_llm::LLMCaller>,
    messages: &[String],
    rt: &tokio::runtime::Handle,
    threshold: f64,
) -> Result<Vec<user_profile::HabitEntry>, String> {
    let sample: Vec<&String> = messages.iter().rev().take(60).collect();
    let prompt = format!(
        "以下是某用户近期对 AI 助手的指令摘录。归纳该用户**稳定的偏好/习惯**\
         （工作流形态、风格、工具偏好——只限风格与流程姿态，绝不包含权限、审批、安全相关语义）。\n\
         只输出 JSON 数组，每项 {{\"category\": \"workflow|correction|tool_preference|style\", \
         \"content\": \"<一句话习惯陈述>\", \"confidence\": <0.0-1.0>}}。\n\
         无稳定习惯则输出 []。\n\n指令摘录：\n{}",
        sample
            .iter()
            .enumerate()
            .map(|(i, m)| format!("{}. {}", i + 1, m.chars().take(120).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let resp =
        rt.block_on(caller.chat("你是用户行为分析师。只输出 JSON。", &prompt, Some(1024)))?;
    let parsed: serde_json::Value = nemesis_forge::reflector_llm::extract_json(&resp)
        .ok_or("reflect response is not valid JSON")?;
    let arr = parsed
        .as_array()
        .ok_or("reflect response is not a JSON array")?;
    let now = chrono::Local::now().to_rfc3339();
    let mut out = Vec::new();
    for item in arr {
        let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("style");
        let confidence = item
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if content.trim().is_empty() || confidence < threshold {
            continue;
        }
        out.push(user_profile::HabitEntry {
            category: category.to_string(),
            content: content.to_string(),
            confidence,
            hits: 1,
            first_seen: now.clone(),
            last_seen: now.clone(),
            status: user_profile::STATUS_ACTIVE.into(),
            pinned: false,
        });
    }
    Ok(out)
}

/// judge 同款模型回落链解析（forge.test.judge_model → agents.small_model →
/// 主模型；实现单一真相源在 nemesis-web handlers::forge）。解析全败 =
/// Ok(None)（反思通道静默跳过，诚实降级）。
fn build_caller(
    home: &Path,
    workspace: &Path,
) -> Result<Option<std::sync::Arc<dyn nemesis_forge::reflector_llm::LLMCaller>>, String> {
    nemesis_web::handlers::forge::resolve_judge_caller(
        &home.to_string_lossy(),
        &workspace.to_string_lossy(),
    )
}
