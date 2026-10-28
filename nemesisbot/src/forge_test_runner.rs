//! Forge A 线行为回归执行器（自进化升级，2026-10-14）。
//!
//! 实现 [`nemesis_forge::skill_tests::SkillTestRunner`]：对一条用例在隔离
//! 临时 workspace 中构建真实 AgentLoop（与 K1 headless `run` 同一装配链：
//! 安全插件 / tier / spill / turn_guard 全同源），按序执行用例的 turns
//! 脚本，采集 transcript（工具调用 / final 文本 / 错误 / 文件效果 / 预算）。
//!
//! 执行语义（定稿）：多轮 turns 按序拼接为**单任务**提交（detached 临时
//! instance 不跨调用续会话）；`max_llm_turns` 断言（≤N 轮完成）映射
//! DetachedOpts.max_turns = N+2 检测余量（预算=N 断言无牙）。装配细节
//! 镜像 `commands::run`（config 必须存在、credentials 全局路径、安全插件
//! 同一构造）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nemesis_forge::skill_tests::{ExecutionTranscript, SkillTestRunner, TestCase};

/// 进程内行为测试执行器。
pub struct BotSkillTestRunner {
    /// 真实 home（config.json / credentials.yaml 的来源）。
    home: PathBuf,
}

impl BotSkillTestRunner {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }
}

#[async_trait::async_trait]
impl SkillTestRunner for BotSkillTestRunner {
    async fn run_case(
        &self,
        skill_content: &str,
        case: &TestCase,
    ) -> Result<ExecutionTranscript, String> {
        let started = std::time::Instant::now();

        // 1. 隔离临时 home + workspace（用例结束即删）。
        let tmp = tempfile::tempdir().map_err(|e| format!("create temp home: {e}"))?;
        let home = tmp.path().to_path_buf();
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).map_err(|e| format!("create workspace: {e}"))?;

        // 2. config 必须可解析（来自真实 home 的副本——模型/密钥同源）。
        let real_cfg_path = crate::common::config_path(&self.home);
        let cfg =
            nemesis_config::load_config(&real_cfg_path).map_err(|e| format!("load config: {e}"))?;
        let cfg_path = crate::common::config_path(&home);
        let cfg_json =
            serde_json::to_string_pretty(&cfg).map_err(|e| format!("serialize config: {e}"))?;
        std::fs::write(&cfg_path, cfg_json).map_err(|e| format!("write config: {e}"))?;

        // 3. 写入被测 skill（workspace/skills/<name>/SKILL.md——loop 装载源）。
        let skill_name = skill_name_of(skill_content);
        let skill_dir = workspace.join("skills").join(&skill_name);
        std::fs::create_dir_all(&skill_dir).map_err(|e| format!("create skill dir: {e}"))?;
        std::fs::write(skill_dir.join("SKILL.md"), skill_content)
            .map_err(|e| format!("write SKILL.md: {e}"))?;

        // 4. 写 fixtures（路径已在 schema 校验层拒过 `..`/盘符/绝对形态；
        //    这里纵深兜底：再拒绝对路径 + Windows 盘符相对形态——`C:evil`
        //    is_absolute()==false 但 join 会整段换基座写出临时区）。
        for fx in &case.fixtures {
            let rel = Path::new(&fx.path);
            let has_prefix_or_root = matches!(
                rel.components().next(),
                Some(std::path::Component::Prefix(_)) | Some(std::path::Component::RootDir)
            );
            if rel.is_absolute() || has_prefix_or_root {
                return Err(format!("fixture path must be relative: {}", fx.path));
            }
            let dest = workspace.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create fixture dir: {e}"))?;
            }
            std::fs::write(&dest, &fx.content)
                .map_err(|e| format!("write fixture {}: {e}", fx.path))?;
        }

        // 5. 装配 AgentLoop（镜像 commands::run 的 K1 链：安全插件同一构造）。
        let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
        let security_plugin =
            crate::security_setup::build_security_plugin(&home, security_enabled).await;
        let config_store = std::sync::Arc::new(nemesis_config::ConfigStore::from_config(
            cfg.clone(),
            cfg_path,
        ));
        let shared = std::sync::Arc::new(crate::agent_factory::SharedResources {
            home: home.clone(),
            workspace: workspace.clone(),
            config_store,
            security_plugin,
            mcp_enabled: false,
            mcp_config_path: home.join("config.mcp.json"),
            agent_event_tx: None,
            entrance: nemesis_agent::prompt::Entrance::Headless,
            ..Default::default()
        });
        let agent_loop = crate::agent_factory::build_agent_loop(&shared)
            .map_err(|e| format!("build agent loop: {e}"))?;

        // 6. turns 按序拼接为单任务（detached 临时 instance 不跨调用续会话）。
        let task_text = match case.turns.len() {
            0 => return Err("test case has no turns".into()),
            1 => case.turns[0].user.clone(),
            _ => {
                let mut parts = Vec::new();
                for (i, t) in case.turns.iter().enumerate() {
                    parts.push(format!("[第 {} 轮] {}", i + 1, t.user));
                }
                parts.join("\n")
            }
        };

        // 7. 执行。`max_llm_turns` 断言语义 = 用例应在 N 轮内完成；执行
        //    预算取 N+2 检测余量——预算=N 会让 turns_executed ≤ N 结构性
        //    恒真，`turns > N` 断言无牙、检测不到任何超轮（与 replay_once
        //    环回同族，二轮复查根修）。transcript.turns_executed 仍按真实
        //    Message 事件计数（钳位到执行预算），达标用例行为零变化。
        let expect_turns = case.expect.max_llm_turns.unwrap_or(8);
        let max_turns = expect_turns.saturating_add(2);
        let label = format!("forge-test-{}", case.name);
        let events = agent_loop
            .run_detached_events(
                &task_text,
                nemesis_agent::r#loop::DetachedOpts {
                    depth: 0,
                    max_turns,
                    label: Some(&label),
                    ..Default::default()
                },
            )
            .await;

        // 8. 采集 transcript。
        let mut tools: BTreeSet<String> = BTreeSet::new();
        let mut finals = Vec::new();
        let mut errors = Vec::new();
        for ev in &events {
            match ev {
                nemesis_agent::types::AgentEvent::ToolCall(calls) => {
                    for c in calls {
                        tools.insert(c.name.clone());
                    }
                }
                nemesis_agent::types::AgentEvent::Done(final_text) => {
                    finals.push(final_text.clone());
                }
                nemesis_agent::types::AgentEvent::Error(msg) => {
                    errors.push(msg.clone());
                }
                _ => {}
            }
        }

        // 9. 效果面：workspace 内现存文件（相对路径；排除 skill 自身与 fixture
        //    预置——期望的是「新产出」也可与预置同名覆盖，这里如实列全量，
        //    断言按路径比对）。
        let mut files = Vec::new();
        collect_relative_files(&workspace, &workspace, &mut files);
        files.sort();

        // 10. 显式关闭并删除临时 home（TempDir drop 静默忽略删除失败——
        //     Windows AV/索引器瞬时句柄是常态，残留会持续堆积磁盘；close()
        //     把失败诚实上报，仅 warn 不吞执行结果。四路评审 A-F3）。
        if let Err(e) = tmp.close() {
            tracing::warn!(
                "[ForgeTest] temp home cleanup failed ({}): {e}",
                home.display()
            );
        }

        Ok(ExecutionTranscript {
            turns_executed: max_turns.min(
                events
                    .iter()
                    .filter(|e| matches!(e, nemesis_agent::types::AgentEvent::Message(_)))
                    .count() as u32,
            ),
            duration_secs: started.elapsed().as_secs(),
            tools_called: tools.into_iter().collect(),
            finals,
            errors,
            files,
        })
    }
}

/// 从 SKILL.md frontmatter 抠 name（无则固定 under-test）。
fn skill_name_of(content: &str) -> String {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return "under-test".into();
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            let n = rest.trim().trim_matches('"').trim_matches('\'');
            if !n.is_empty() {
                // 目录用安全化形式（与 sanitize 纪律一致；短名直用）。
                return nemesis_utils::sanitize::sanitize_path_segment(n);
            }
        }
    }
    "under-test".into()
}

/// 递归收集相对路径（跳过 skills/ 自身与隐藏文件）。符号链接/junction
/// 一律跳过（不跟随不收集，symlink_metadata 判定，R204/BUG-20261029-135
/// 同族）——被测 agent 可在临时工作区造自指链接，跟随版递归挂死 runner；
/// 目标树文件也不得以 `link/` 前缀污染 files_exist 断言面。
fn collect_relative_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        let Ok(meta) = p.symlink_metadata() else {
            continue; // 消失/不可达条目不收集不递归（跟随版旧码会当文件 push）
        };
        if meta.is_symlink() {
            continue;
        }
        let rel = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .to_string();
        if meta.is_dir() {
            if name == "skills" || name.starts_with('.') {
                continue;
            }
            collect_relative_files(root, &p, out);
        } else if !name.starts_with('.') {
            out.push(rel.replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// C 线失败重放执行面（用例管理在 nemesis_forge::replay；执行装配同 K1 链）
// ---------------------------------------------------------------------------

/// 重放执行（confirmed 集）：逐条经进程内 AgentLoop 重跑，回传结果摘要。
pub async fn run_replay_all(
    home: &Path,
    workspace: &Path,
) -> Result<Vec<(String, bool, Vec<String>)>, String> {
    let confirmed = nemesis_forge::replay::confirmed_cases(workspace)?;
    let mut out = Vec::new();
    for case in &confirmed {
        let transcript = replay_once(home, case).await?;
        let result = nemesis_forge::skill_tests::evaluate_case(case, &transcript);
        out.push((case.name.clone(), result.passed, result.failures));
    }
    Ok(out)
}

/// 单次重放执行（镜像 `BotSkillTestRunner::run_case` 的进程内装配；无
/// skill 装载——重放任务不依赖被测 skill）。
async fn replay_once(home: &Path, case: &TestCase) -> Result<ExecutionTranscript, String> {
    let started = std::time::Instant::now();
    let tmp = tempfile::tempdir().map_err(|e| format!("create temp home: {e}"))?;
    let tmp_home = tmp.path().to_path_buf();
    let run_ws = tmp_home.join("workspace");
    std::fs::create_dir_all(&run_ws).map_err(|e| format!("create workspace: {e}"))?;

    let cfg = nemesis_config::load_config(&crate::common::config_path(home))
        .map_err(|e| format!("load config: {e}"))?;
    let cfg_path = crate::common::config_path(&tmp_home);
    let cfg_json = serde_json::to_string_pretty(&cfg).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&cfg_path, cfg_json).map_err(|e| format!("write config: {e}"))?;

    let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    let security_plugin =
        crate::security_setup::build_security_plugin(&tmp_home, security_enabled).await;
    let shared = std::sync::Arc::new(crate::agent_factory::SharedResources {
        home: tmp_home.clone(),
        workspace: run_ws.clone(),
        config_store: std::sync::Arc::new(nemesis_config::ConfigStore::from_config(
            cfg.clone(),
            cfg_path,
        )),
        security_plugin,
        mcp_enabled: false,
        mcp_config_path: tmp_home.join("config.mcp.json"),
        agent_event_tx: None,
        entrance: nemesis_agent::prompt::Entrance::Headless,
        ..Default::default()
    });
    let agent_loop = crate::agent_factory::build_agent_loop(&shared)
        .map_err(|e| format!("build agent loop: {e}"))?;

    // 同 run_case：0-turn 用例在执行前诚实拒绝（join 出空任务文本会让
    // detached loop 空转一轮，transcript 无意义——四路评审 A-F4）。
    if case.turns.is_empty() {
        return Err("replay case has no turns".into());
    }
    let task_text = case
        .turns
        .iter()
        .map(|t| t.user.clone())
        .collect::<Vec<_>>()
        .join("\n");
    // 同 run_case：执行预算 = 期望 +2 检测余量（预算=期望会让
    // max_llm_turns 断言结构性恒真）；transcript.turns_executed 真实计数。
    let expect_turns = case.expect.max_llm_turns.unwrap_or(8);
    let max_turns = expect_turns.saturating_add(2);
    let label = format!("replay-{}", case.name);
    let events = agent_loop
        .run_detached_events(
            &task_text,
            nemesis_agent::r#loop::DetachedOpts {
                depth: 0,
                max_turns,
                label: Some(&label),
                ..Default::default()
            },
        )
        .await;

    let mut tools: BTreeSet<String> = BTreeSet::new();
    let mut finals = Vec::new();
    let mut errors = Vec::new();
    for ev in &events {
        match ev {
            nemesis_agent::types::AgentEvent::ToolCall(calls) => {
                for c in calls {
                    tools.insert(c.name.clone());
                }
            }
            nemesis_agent::types::AgentEvent::Done(f) => finals.push(f.clone()),
            nemesis_agent::types::AgentEvent::Error(m) => errors.push(m.clone()),
            _ => {}
        }
    }
    let mut files = Vec::new();
    collect_relative_files(&run_ws, &run_ws, &mut files);
    files.sort();

    // 同 run_case：显式 close 诚实化临时 home 清理失败（TempDir drop 静默
    // 忽略删除失败，Windows 瞬时句柄残留会持续堆积。四路评审 A-F3）。
    if let Err(e) = tmp.close() {
        tracing::warn!(
            "[ForgeTest] temp home cleanup failed ({}): {e}",
            tmp_home.display()
        );
    }

    Ok(ExecutionTranscript {
        // 与 run_case 同语义：真实执行轮数（Message 事件计数），非预算值——
        // 填预算上限会让 max_llm_turns 断言恒等环回、检测不到截断。
        turns_executed: max_turns.min(
            events
                .iter()
                .filter(|e| matches!(e, nemesis_agent::types::AgentEvent::Message(_)))
                .count() as u32,
        ),
        duration_secs: started.elapsed().as_secs(),
        tools_called: tools.into_iter().collect(),
        finals,
        errors,
        files,
    })
}

/// Stage 2 judge 层杂务模型回落链适配器（2026-10-14 三轮复查：部署闸与
/// CLI/WSAPI 的 judge 口径对齐）。每次 `chat` 现场解析
/// `resolve_judge_caller`（judge_model → agents.small_model → 主模型）——
/// 模型热切即时生效，无陈旧 Arc；全链失败 = 诚实 Err（评估层 fail-closed
/// 记断言失败）。
pub struct ChainJudgeCaller {
    home: PathBuf,
    workspace: PathBuf,
}

impl ChainJudgeCaller {
    pub fn new(home: PathBuf, workspace: PathBuf) -> Self {
        Self { home, workspace }
    }
}

#[async_trait::async_trait]
impl nemesis_forge::reflector_llm::LLMCaller for ChainJudgeCaller {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: Option<i64>,
    ) -> Result<String, String> {
        let caller = nemesis_web::handlers::forge::resolve_judge_caller(
            &self.home.to_string_lossy(),
            &self.workspace.to_string_lossy(),
        )?
        .ok_or_else(|| "judge caller chain resolved to none (all arms failed)".to_string())?;
        caller.chat(system_prompt, user_prompt, max_tokens).await
    }
}
