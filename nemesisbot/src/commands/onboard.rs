//! `onboard default` 初始化序列（12 步）+ 双击直启 auto-init。
//!
//! 从 main.rs 内联实现提取（双击直启 goal 2026-09-17）——CLI 与 auto-init
//! 共用同一份初始化序列，写盘语义按 [`OnboardMode`] 分叉：
//!
//! - [`OnboardMode::Cli`]：`nemesisbot onboard default` 原行为（覆盖语义：
//!   config 已存在时交互确认覆盖、workspace 模板 overwrite、人格文件总是
//!   覆盖——`default/` 是新装机的权威源，re-onboard 用于修复损坏模板）。
//! - [`OnboardMode::Seed`]：gateway 无参/`--local` 直启时 config.json 缺失的
//!   auto-init（种子语义：一切 only-if-absent——用户已有的 workspace/人格/
//!   子系统配置绝不被 clobber；只补齐缺失件）。

use crate::common;
use std::path::Path;

/// 初始化写盘语义（见模块注释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardMode {
    /// CLI `onboard default`：覆盖语义（原行为）。
    Cli,
    /// 双击直启 auto-init：种子语义（everything only-if-absent）。
    Seed,
}

/// `onboard default` 主体。`local` 影响 agents.defaults.workspace 的相对
/// 路径改写（与原 main.rs 内联实现一致）。仅在写主 config 失败时返回 Err
/// （其余步骤维持原 best-effort `let _ =` 行为）。
pub fn onboard_default(home: &Path, local: bool, mode: OnboardMode) -> anyhow::Result<()> {
    use crate::{CONFIG_CLUSTER_DEFAULT, CONFIG_DEFAULT};

    // Platform detection
    let platform = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    };
    println!("  Detected platform: {}", platform);
    println!("  Applying platform-specific security rules...");

    // Create directories
    let _ = std::fs::create_dir_all(home);
    let _ = std::fs::create_dir_all(home.join("workspace"));
    let _ = std::fs::create_dir_all(home.join("workspace").join("config"));
    let _ = std::fs::create_dir_all(common::cluster_dir(home));

    let cfg_path = common::config_path(home);
    let workspace_dir = home.join("workspace");

    // --- Step 0: 用途登记种子（W1 #1 安全图谱批）---
    // agent_charter.json（用途/场景/边界声明）种子落盘（已存在不覆盖
    // ——登记是用户意志，onboard 只补缺席）。purpose 留空引导填写；
    // 编辑面 = 直接改文件 / WSAPI charter.set / Dashboard。
    {
        let charter_path = workspace_dir.join("config").join("agent_charter.json");
        let seed = crate::commands::charter_seed_json();
        write_if_absent(&charter_path, seed, mode, "用途登记 charter");
        if charter_path.exists() {
            println!(
                "  Charter: {} —— 填写用途/场景/边界（read_only=true 建议 plan 模式）",
                charter_path.display()
            );
        }
    }

    // --- Step 1: Main config from embedded default ---
    // Determine whether to write main config.
    // Cli：已存在时交互确认覆盖（原行为）；Seed：已存在直接跳过（种子语义，
    // 本函数只会被「config.json 缺失」的 gateway 调用，此分支是防御）。
    let mut write_main_config = true;
    if cfg_path.exists() {
        match mode {
            OnboardMode::Cli => {
                print!(
                    "  Config already exists at {}, overwrite? (y/N): ",
                    cfg_path.display()
                );
                use std::io::{self as std_io, Write as StdWrite};
                std_io::stdout().flush().ok();
                let mut answer = String::new();
                std_io::stdin().read_line(&mut answer).ok();
                if answer.trim().to_lowercase() != "y" {
                    println!("  Skipping main config (keeping existing).");
                    write_main_config = false;
                }
            }
            OnboardMode::Seed => {
                println!("  Config already exists, keeping existing (seed mode).");
                write_main_config = false;
            }
        }
    }

    if write_main_config {
        // Use compile-time embedded config (always available)
        // [ 死码处置·简化] 原 match 的 Err(_) =>
        // write_fallback_config 分支恒不触发：CONFIG_DEFAULT 是编译期
        // 嵌入常量，from_str 不可能失败（若真失败，onboard 任一测试
        // 第一跑即 panic 暴露）。恒 Ok 路径行为不变；write_fallback_config
        // 已随之注释禁用（见 main.rs 文件底部）。
        let mut cfg = serde_json::from_str::<serde_json::Value>(CONFIG_DEFAULT)
            .expect("embedded CONFIG_DEFAULT must be valid JSON (compile-time constant)");
        {
            // Enable LLM logging
            if let Some(logging) = cfg.get_mut("logging").and_then(|v| v.get_mut("llm"))
                && let Some(obj) = logging.as_object_mut()
            {
                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                obj.insert(
                    "log_dir".to_string(),
                    serde_json::Value::String("logs/request_logs".to_string()),
                );
                obj.insert(
                    "detail_level".to_string(),
                    serde_json::Value::String("full".to_string()),
                );
            }
            println!("  LLM logging enabled");

            // Enable security
            if let Some(security) = cfg.get_mut("security") {
                if let Some(obj) = security.as_object_mut() {
                    obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                }
            } else {
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert("security".to_string(), serde_json::json!({"enabled": true}));
                }
            }
            println!("  Security module enabled");

            // 工作区围栏不动模板默认（true）：安全管线 8 层
            // 不含工作区包围（注入/命令/ABAC/凭据/DLP/SSRF/扫描/审计各自语义），
            // 包围唯一执法点是 agent 侧 WorkspaceBoundary——旧代码在此强制改写
            // false 且注释声称「security module enforces rules」，实测 fresh
            // 装机 `..` 相对路径越界写 ok:true（管线放行、无审批弹卡），开箱即
            // 无文件系统写约束，与模板/部署说明书基线矛盾。需要放开围栏的用户
            // 显式改 config（部署说明书：接受风险条件 = 沙盒 + strict）。
            if let Some(agents) = cfg.get_mut("agents").and_then(|v| v.get_mut("defaults"))
                && let Some(obj) = agents.as_object_mut()
                && local
            {
                obj.insert(
                    "workspace".to_string(),
                    serde_json::Value::String(".nemesisbot/workspace".to_string()),
                );
            }

            // Set web auth token, port, websocket
            if let Some(web) = cfg.pointer_mut("/channels/web")
                && let Some(obj) = web.as_object_mut()
            {
                // SEC-001：引导值与 common::is_bootstrap_web_credential 同源——
                // 新装 = bootstrap 态 + 回环绑定 = 合法初始形态。
                obj.insert(
                    "auth_token".to_string(),
                    serde_json::Value::String(crate::common::BOOTSTRAP_WEB_TOKEN.to_string()),
                );
                obj.insert(
                    "host".to_string(),
                    serde_json::Value::String("127.0.0.1".to_string()),
                );
                obj.insert("port".to_string(), serde_json::Value::Number(49000.into()));
            }
            if let Some(ws) = cfg.pointer_mut("/channels/websocket")
                && let Some(obj) = ws.as_object_mut()
            {
                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
            }

            // 统一原子写入（主 config.json 含引导令牌与安全开关）。
            nemesis_utils::write_file_atomic(
                &cfg_path.to_string_lossy(),
                serde_json::to_string_pretty(&cfg)
                    .unwrap_or_default()
                    .as_bytes(),
                0o600,
            )
            .map_err(anyhow::Error::msg)?;
            println!("  Main config saved to {}", cfg_path.display());
        }
    }

    // --- Step 2: MCP config (embedded) ---
    write_if_absent(
        &common::mcp_config_path(home),
        crate::CONFIG_MCP_DEFAULT,
        mode,
        "MCP config",
    );

    // --- Step 3: Security config (platform-specific, embedded) ---
    let security_content = if cfg!(target_os = "windows") {
        crate::CONFIG_SECURITY_WINDOWS
    } else if cfg!(target_os = "macos") {
        crate::CONFIG_SECURITY_DARWIN
    } else if cfg!(target_os = "linux") {
        crate::CONFIG_SECURITY_LINUX
    } else {
        crate::CONFIG_SECURITY_OTHER
    };
    write_if_absent(
        &common::security_config_path(home),
        security_content,
        mode,
        "Security config",
    );

    // --- Step 4: Cluster config (system params + UDP discovery token) ---
    // config.cluster.json 不再含身份字段（name/role/node_id 等），
    // 身份信息全部由 peers.toml 的 [node] 段承载。
    // [ 死码处置·简化] from_str 对编译期嵌入常量不可能失败
    // （见 Step 1 注）。
    let cluster_cfg_path = common::cluster_config_path(home);
    if !matches!(mode, OnboardMode::Seed) || !cluster_cfg_path.exists() {
        let mut cluster_cfg = serde_json::from_str::<serde_json::Value>(CONFIG_CLUSTER_DEFAULT)
            .expect("embedded CONFIG_CLUSTER_DEFAULT must be valid JSON (compile-time constant)");
        if let Some(obj) = cluster_cfg.as_object_mut() {
            obj.insert(
                "token".to_string(),
                serde_json::Value::String(uuid::Uuid::new_v4().to_string()),
            );
        }
        // 统一原子写入（cluster 配置含发现层 token）。
        let _ = nemesis_utils::write_file_atomic(
            &cluster_cfg_path.to_string_lossy(),
            serde_json::to_string_pretty(&cluster_cfg)
                .unwrap_or_default()
                .as_bytes(),
            0o600,
        );
        println!("  Cluster config created");
    }

    // --- Step 4.5: QUIC relay config (embedded) ---
    // config.quic.json：QUIC 自身全部配置（服务端 relay 段 + 客户端 client
    // 段； 与集群解耦——QUIC 的配置不丢在集群里）。无 token 注入
    // 面（relay.token 用户显式配置），write_if_absent 直接落模板。
    write_if_absent(
        &common::quic_config_path(home),
        crate::CONFIG_QUIC_DEFAULT,
        mode,
        "QUIC relay config",
    );

    // --- Step 5: Cluster peers.toml (本节点身份) ---
    // peers.toml 只含 [node] 段（本节点身份）+ 可选的 [peers.X] 静态条目。
    // 不再含 [cluster] 段（cluster 元数据已下线）。
    {
        let cluster_dir = common::cluster_dir(home);
        let _ = std::fs::create_dir_all(&cluster_dir);
        let peers_path = cluster_dir.join("peers.toml");
        if !matches!(mode, OnboardMode::Seed) || !peers_path.exists() {
            let hostname = std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "node".to_string());
            let node_id = format!("node-{}-{}", hostname.to_lowercase(), uuid::Uuid::new_v4());
            let peers_content = format!(
                "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[node]\nid = \"{}\"\nname = \"Bot {}\"\naddress = \"\"\nrole = \"worker\"\ncategory = \"general\"\ntags = []\ncapabilities = []\n\n# Add peer entries as [peers.Name] tables, e.g.:\n# [peers.MyPeer]\n# address = \"127.0.0.1:11950\"\n# role = \"worker\"\n# category = \"general\"\n",
                node_id, node_id
            );
            // 统一原子写入（peers.toml 是节点身份关键文件）。
            let _ = nemesis_utils::write_file_atomic(
                &peers_path.to_string_lossy(),
                peers_content.as_bytes(),
                0o600,
            );
            println!("  Peers config created");
        }
    }

    // --- Step 6: Skills config (embedded — includes GitHub sources) ---
    write_if_absent(
        &common::skills_config_path(home),
        crate::CONFIG_SKILLS_DEFAULT,
        mode,
        "Skills config",
    );

    // --- Step 7: Scanner config (embedded) ---
    write_if_absent(
        &common::scanner_config_path(home),
        crate::CONFIG_SCANNER_DEFAULT,
        mode,
        "Scanner config",
    );

    // --- Step 7.5: Enhanced Memory config (embedded) ---
    write_if_absent(
        &common::enhanced_memory_config_path(home),
        crate::CONFIG_ENHANCED_MEMORY_DEFAULT,
        mode,
        "Enhanced memory config",
    );

    // --- Step 7.6: Chat config (embedded) ---
    write_if_absent(
        &common::chat_config_path(home),
        crate::CONFIG_CHAT_DEFAULT,
        mode,
        "Chat config",
    );

    // --- Step 7.7: Forge config (embedded) ---
    write_if_absent(
        &common::forge_config_path(home),
        crate::CONFIG_FORGE_DEFAULT,
        mode,
        "Forge config",
    );

    // --- Step 7.8: eval rules (embedded; seeds the assessor's rule
    // file + its readme. Content identical to eval_assessor's
    // include_str — same source file, no second definition) ---
    // 路径走 rules_file_path 单一真相源（手拼 workspace/config
    // 与它是两套逻辑，一处改另一处忘改会漂移——种子到错误位置时
    // 评估器/管理命令都找不到规则）。两种模式都 only-if-absent。
    #[cfg(feature = "eval")]
    {
        let rules_path = crate::eval_assessor::rules_file_path(home);
        if let Some(parent) = rules_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Seed only when absent — user edits survive re-onboard.
        if !rules_path.exists() {
            // 统一原子写入（eval 规则表是安全配置）。
            let _ = nemesis_utils::write_file_atomic(
                &rules_path.to_string_lossy(),
                crate::eval_assessor::DEFAULT_RULES_JSON.as_bytes(),
                0o600,
            );
            println!("  Eval rules config created");
        }
        let readme_path = rules_path
            .parent()
            .map(|p| p.join("eval_rules.readme.md"))
            .unwrap_or_else(|| rules_path.clone());
        if !readme_path.exists() {
            let _ = std::fs::write(
                &readme_path,
                include_str!("../../config/eval_rules.readme.md"),
            );
        }
    }

    // --- Step 8: Extract embedded workspace templates ---
    // Mirrors Go's copyEmbeddedToTarget() — copies all files from
    // embedded `workspace/` directory (skills, scripts, memory, md files).
    // Cli：overwrite（re-onboarding restores corrupted templates，原行为）；
    // Seed：不覆盖（用户已有 workspace 文件保持原样）。
    let extract_result = match mode {
        OnboardMode::Cli => crate::embedded::extract_workspace_templates_overwrite(&workspace_dir),
        OnboardMode::Seed => crate::embedded::extract_workspace_templates(&workspace_dir),
    };
    match extract_result {
        Ok(()) => println!("  Workspace templates extracted"),
        Err(e) => println!(
            "  Warning: failed to extract some workspace templates: {}",
            e
        ),
    }

    // --- Step 9: Install default personality files (embedded) ---
    // Cli：总是覆盖——default/ 是新装机的权威源（mirrors Go's
    // copyDefaultFiles，re-onboard 修复损坏人格）。Seed：only-if-absent
    // （用户已激活人格不被 clobber）。2026-11-16 提取排除后身份三件在
    // Step 8 不再落盘（embedded::EXTRACT_EXCLUDED_ENTRIES），两条路径此处
    // 都天然安装到老贾版——消除「Step 8 空白模板占位 → Seed 存在即跳过
    // 静默截胡」的双真相源分裂。
    // 写失败计数汇总（诚实报告：不再无条件宣称安装成功）。
    let mut persona_failures = 0usize;
    if !install_if_absent(
        &workspace_dir.join("IDENTITY.md"),
        crate::DEFAULT_IDENTITY,
        mode,
    ) {
        persona_failures += 1;
    }
    if !install_if_absent(&workspace_dir.join("SOUL.md"), crate::DEFAULT_SOUL, mode) {
        persona_failures += 1;
    }
    if !install_if_absent(&workspace_dir.join("USER.md"), crate::DEFAULT_USER, mode) {
        persona_failures += 1;
    }
    // Cluster identity — extracted to workspace/cluster/IDENTITY.md.
    let cluster_id_dir = nemesis_path::cluster_dir_in_workspace(&workspace_dir);
    let _ = std::fs::create_dir_all(&cluster_id_dir);
    if !install_if_absent(
        &cluster_id_dir.join("IDENTITY.md"),
        crate::DEFAULT_IDENTITY_CLUSTER,
        mode,
    ) {
        persona_failures += 1;
    }
    if persona_failures == 0 {
        println!(
            "  Default personality files installed (IDENTITY.md, SOUL.md, USER.md, cluster/IDENTITY.md)"
        );
    } else {
        println!("  Warning: {persona_failures} personality file(s) failed to install (see above)");
    }

    // --- Step 9.5: Materialize persona slots (default=贾维斯 / custom=空白) ---
    // 2026-11-16 人格预装语义重构：提取排除后 personas/ 不再来自 Step 8，
    // 改由这里物化两槽（五件套口径见 materialize_persona_store 文档注释）。
    materialize_persona_store(&workspace_dir);

    // --- Step 10: Create additional directories ---
    let _ = std::fs::create_dir_all(workspace_dir.join("logs"));
    let _ = std::fs::create_dir_all(workspace_dir.join("forge"));
    // Workflow subdirs: definitions/ (YAML), templates/ (starter
    // templates), checkpoints/ (resume snapshots), executions/ (JSONL
    // run logs). All four are created up-front so the gateway can
    // rely on them existing without each callsite having to mkdir.
    for sub in ["definitions", "templates", "checkpoints", "executions"] {
        let _ = std::fs::create_dir_all(workspace_dir.join("workflow").join(sub));
    }

    // --- Step 10b: Delete BOOTSTRAP.md from workspace if it exists ---
    // BOOTSTRAP.md is the bootstrap init file; after onboard default the
    // personality is already set up, so it must be removed (mirrors Go).
    let bootstrap = workspace_dir.join("BOOTSTRAP.md");
    if bootstrap.exists() {
        let _ = nemesis_utils::fsutil::remove_file_resilient(&bootstrap);
        println!("  BOOTSTRAP.md removed");
    }

    // --- Step 10c: #16 提示词安全常态化——onboard 后自动评估人格文件
    // （W2 批三，2026-11-20 拍板）。`security.prompt_eval.on_onboard` 闸
    // （缺省 true）；detached fire-and-forget（沙盒内自评，报告落
    // logs/eval/，触发流水落 logs/prompt_eval.log），失败不打扰 onboard
    // 主流程（非 Windows/沙盒未就绪由触发点诚实 SKIP 落日志）。
    {
        let raw_cfg = nemesis_utils::fsutil::read_config_string(common::config_path(home))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        if nemesis_config::resolve_prompt_eval(raw_cfg.as_ref()).on_onboard {
            let mut persona_text = String::new();
            for name in ["IDENTITY.md", "SOUL.md", "USER.md", "AGENT.md"] {
                if let Ok(content) = std::fs::read_to_string(workspace_dir.join(name)) {
                    persona_text.push_str(&content);
                    persona_text.push('\n');
                }
            }
            if persona_text.trim().is_empty() {
                tracing::info!("[Onboard] prompt-eval skipped: no persona files to scan");
            } else {
                match nemesis_utils::prompt_eval::trigger_prompt_eval(home, &persona_text) {
                    nemesis_utils::prompt_eval::PromptEvalTrigger::Spawned => {
                        println!("  Prompt security evaluation started (async, sandbox)");
                    }
                    nemesis_utils::prompt_eval::PromptEvalTrigger::Skipped(reason) => {
                        tracing::info!("[Onboard] prompt-eval skipped: {reason}");
                    }
                }
            }
        }
    }

    // --- Step 11: Web and WebSocket configuration ---
    println!("  Web and WebSocket configuration set");

    println!();
    println!("  Initialization complete!");
    println!();
    match mode {
        OnboardMode::Cli => {
            println!("  Available interfaces:");
            println!("    Web: http://127.0.0.1:49000 (access key: 276793422)");
            println!("    WebSocket: ws://127.0.0.1:49001/ws");
            println!();
            println!("  Next steps:");
            println!(
                "    1. Add your API key: nemesisbot model add --model <vendor/model> --key <key> --default"
            );
            println!("    2. Start gateway:     nemesisbot gateway");
            println!();
            // 信任契约（行动代理 感知层）：初始化时讲清四承诺。
            println!("  Trust contract (action agent):");
            println!("    1. 凭据不泄露 —— API key 等秘密进加密 vault，永不出现在模型上下文/日志");
            println!("    2. 动手前先问 —— 高危操作先弹授权卡，写明影响面与拒绝后果");
            println!("    3. 做错能反悔 —— 聊天说「撤销」回退上一轮（对话+文件一起还原）");
            println!(
                "    4. 出事有账 —— 每个动作带编号凭条，connector receipts / 行动账本随时回查"
            );
            println!();
            println!("  查询: nemesisbot connector list | connector receipts | connector ledger");
            println!("  试试: nemesisbot connector demo   # 30 秒四承诺演示（临时目录，零污染）");
        }
        OnboardMode::Seed => {
            // auto-init：调用方（gateway）随即继续启动，无需再教 CLI 命令。
            tracing::info!(
                "[Onboard] auto-init (seed mode) complete at {} — gateway continuing startup",
                home.display()
            );
        }
    }
    println!();
    if matches!(mode, OnboardMode::Cli) {
        println!("  MCP servers:");
        println!("    Add MCP servers: nemesisbot mcp add -n <name> -c <command>");
        println!("    List MCP servers: nemesisbot mcp list");
    }

    Ok(())
}

/// 配置文件写盘（按模式分叉）：Cli = 总是写（覆盖，原行为）；Seed =
/// only-if-absent（存在即跳过，静默保留用户文件）。
fn write_if_absent(path: &Path, content: &str, mode: OnboardMode, label: &str) {
    if matches!(mode, OnboardMode::Seed) && path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 统一原子写入（MCP/Skills/Scanner/内存/Chat/Forge 配置共用
    // 本 helper 单点——半截配置文件 = 对应子系统启动失败）。
    // 写失败诚实报 Warning，不再打印假成功（同 Step 8 模板提取的口径）。
    match nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600) {
        Ok(()) => println!("  {} created", label),
        Err(e) => println!("  Warning: failed to write {}: {e}", label),
    }
}

/// 人格文件安装（按模式分叉）：Cli = 总是覆盖；Seed = only-if-absent。
/// 返回 false = 写盘失败（调用方汇总计数，不再无条件宣称安装成功）。
fn install_if_absent(path: &Path, content: &str, mode: OnboardMode) -> bool {
    if matches!(mode, OnboardMode::Seed) && path.exists() {
        return true;
    }
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        println!("  Warning: failed to create {}: {e}", parent.display());
        return false;
    }
    // 原子写（半截 IDENTITY.md = 截断人格，与配置文件同类伤害——
    // 此前横扫遗漏点，裸 std::fs:write 崩溃窗口可落半截人格文件）。
    match nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600) {
        Ok(()) => true,
        Err(e) => {
            println!("  Warning: failed to write {}: {e}", path.display());
            false
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Step 9.5：人格槽物化（2026-11-16 人格预装语义重构）
// ─────────────────────────────────────────────────────────────────────────

/// 系统默认执行面三件（与 workspace 模板同一 include_str 源——改模板两槽
/// 同步；老贾无专属执行面特化，default/custom 共用同一份执行面）。
const SLOT_AGENT_MD: &str = include_str!("../../workspace/AGENT.md");
const SLOT_TOOLS_MD: &str = include_str!("../../workspace/TOOLS.md");
const SLOT_HEARTBEAT_MD: &str = include_str!("../../workspace/HEARTBEAT.md");
/// custom 槽身份两件（workspace 空白模板同源——单一真相源保持：改模板
/// custom 同步；改 default/ 老贾同步）。
const SLOT_IDENTITY_BLANK_MD: &str = include_str!("../../workspace/IDENTITY.md");
const SLOT_SOUL_BLANK_MD: &str = include_str!("../../workspace/SOUL.md");

/// 物化人格槽（onboard Step 9.5）。
///
/// 空/新装机的本地人格底座 = 两槽：`personas/default/`（贾维斯/老贾本体，
/// 默认激活）+ `personas/custom/`（空白定制画布，用户改名编辑后自用）+
/// `_active.json`。内置人格不在此列——必须用户从内置页手动下载才落地
/// （提取排除见 `embedded::EXTRACT_EXCLUDED_ENTRIES`）。
///
/// **五件套口径 = nemesis-web handlers/persona.rs 的 PERSONA_FILES ∪
/// PERSONA_ARCHIVE_FILES 白名单**（IDENTITY/SOUL/AGENT/TOOLS/HEARTBEAT；
/// USER.md 不在白名单 = 跨人格恒定文件，不进 persona 目录，root 由
/// Step 9 落）——激活链（activate_persona_workspace）清根后按白名单恢复
/// 且无系统默认补齐臂，槽内缺哪件切走再切回根上就永久缺哪件，故两槽
/// 必须带全五件。口径变更需与 web 白名单两处同步（防双真相漂移）。
///
/// 写盘**恒 only-if-absent（两种模式一致）**：人格槽是用户可编辑数据面
/// ——default 可能已被用户改过、custom 可能定制中，覆盖即毁用户数据
/// （刻意偏离 Cli=overwrite 惯例；root 身份三件不受影响，仍按 Step 9
/// 模式语义）。文件级 only-if-absent 只作用于**已存在的槽目录内**：
/// 懒初始化归档槽（web ensure_persona_store 的半量白名单产物）在此补齐
/// 缺口件；槽目录本身的创建另受下方闸门约束。
///
/// custom 被用户删除后**不会复活**（槽目录创建闸）：personas/ 整体缺席 =
/// 全新装机，物化两槽；在场 = 存量环境（懒初始化归档 / 用户自行管理），
/// 只对**已存在的槽目录**逐文件补缺口——槽目录缺席 = 用户删除意志，不
/// 重建、不造墓碑；同一闸门天然覆盖「不迁移存量」（老用户 personas/ 无
/// custom 槽，re-onboard 不凭空造出）。删除整个 personas/ 目录 = 显式
/// 重置意志，重初始化照常恢复底座（与删 config.json 重置身份面同口径）。
fn materialize_persona_store(workspace_dir: &Path) {
    let personas_dir = workspace_dir.join("personas");
    // 槽创建闸：personas/ 在场即视为存量环境（不新建任何槽目录）。
    let fresh_install = !personas_dir.exists();

    // default 槽：老贾本体。IDENTITY/SOUL 用 default/ 权威源（与 Step 9
    // root 安装同源——root 是老贾版，default 槽也是老贾版）。
    let default_slot = personas_dir.join("default");
    if fresh_install || default_slot.is_dir() {
        materialize_persona_slot(
            &default_slot,
            &[
                ("IDENTITY.md", crate::DEFAULT_IDENTITY),
                ("SOUL.md", crate::DEFAULT_SOUL),
                ("AGENT.md", SLOT_AGENT_MD),
                ("TOOLS.md", SLOT_TOOLS_MD),
                ("HEARTBEAT.md", SLOT_HEARTBEAT_MD),
            ],
            "贾维斯",
            "😄",
            "默认管家人格：日常对话与任务执行的贴身助手",
        );
    }

    // custom 槽：空白定制画布（同受槽创建闸——存量环境无此槽则不造）。
    let custom_slot = personas_dir.join("custom");
    if fresh_install || custom_slot.is_dir() {
        materialize_persona_slot(
            &custom_slot,
            &[
                ("IDENTITY.md", SLOT_IDENTITY_BLANK_MD),
                ("SOUL.md", SLOT_SOUL_BLANK_MD),
                ("AGENT.md", SLOT_AGENT_MD),
                ("TOOLS.md", SLOT_TOOLS_MD),
                ("HEARTBEAT.md", SLOT_HEARTBEAT_MD),
            ],
            "自定义人格",
            "🧩",
            "空白人格画布：编辑 personas/custom/ 下各文件定制你的人格，启用后生效",
        );
    }

    // 激活标记：默认指向 default（only-if-absent——已有激活状态绝不覆盖）。
    // 写入闸（复审精化）：仅当本重构语义确实接管该 store 时写——
    // fresh_install（两槽刚物化）或任一本重构槽已在场（标记丢失按缺省
    // default 补）。存量老用户 store（只有第三方槽、无 default/custom、
    // 无标记）**不写**：web ensure_persona_store 的懒初始化归档以
    // `_active.json` 在场为闸（persona.rs:1587），提前写入会把闸关死，
    // default 槽将永不创建（老路径靠首次触达人格页从 root 归档自愈）。
    // 不写 = 与旧版行为一致（标记缺席由 ensure-first 纪律兜底：cmd_*
    // 全部先 ensure 再读）。
    let active_path = personas_dir.join("_active.json");
    if !active_path.exists() && (fresh_install || default_slot.is_dir() || custom_slot.is_dir()) {
        let active = serde_json::json!({ "name": "default" });
        if let Err(e) = nemesis_utils::write_file_atomic(
            &active_path.to_string_lossy(),
            serde_json::to_string_pretty(&active)
                .unwrap_or_else(|_| active.to_string())
                .as_bytes(),
            0o600,
        ) {
            println!("  Warning: failed to write personas/_active.json: {e}");
        }
    }
}

/// 单槽物化：五件套 + PERSONA.json，全部 only-if-absent（存在即跳过，
/// 不读不比对——已有文件可能是用户手笔）。PERSONA.json 三字段形态对齐
/// web ensure_persona_store 的懒初始化产物（name/emoji/description；
/// 转换管线 write_persona_files 写的是带 frontmatter 的四字段形态，
/// 两形态 web 侧皆可解析）。
fn materialize_persona_slot(
    slot_dir: &Path,
    files: &[(&str, &str)],
    name: &str,
    emoji: &str,
    description: &str,
) {
    let slot_label = slot_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut written = 0usize;
    let mut failures = 0usize;
    for (file, content) in files {
        let path = slot_dir.join(file);
        if path.exists() {
            continue;
        }
        if install_if_absent(&path, content, OnboardMode::Seed) {
            written += 1;
        } else {
            failures += 1;
        }
    }
    // PERSONA.json 压轴（installed 判定唯一口径同款语义）。
    let persona_json_path = slot_dir.join("PERSONA.json");
    if !persona_json_path.exists() {
        let persona_json = serde_json::json!({
            "name": name,
            "emoji": emoji,
            "description": description,
        });
        match nemesis_utils::write_file_atomic(
            &persona_json_path.to_string_lossy(),
            serde_json::to_string_pretty(&persona_json)
                .unwrap_or_else(|_| persona_json.to_string())
                .as_bytes(),
            0o600,
        ) {
            Ok(()) => written += 1,
            Err(e) => {
                println!("  Warning: failed to write {slot_label}/PERSONA.json: {e}");
                failures += 1;
            }
        }
    }
    if failures > 0 {
        println!(
            "  Warning: persona slot '{slot_label}': {failures} file(s) failed to materialize"
        );
    } else if written > 0 {
        println!("  Persona slot '{slot_label}' materialized ({written} file(s))");
    }
}

// ---------------------------------------------------------------------------
// #5 安全模板（W2，2026-11-20）：`onboard template <场景>`——分级 × 场景
// 生成安全配置预设。
// ---------------------------------------------------------------------------

/// 安全模板场景。v1 四场景；档位映射走 [`SecurityScenario::level_str`]，
/// config 面只写 `security.level` 一个键（档位表是缺省真相源——模板不把
/// 档位表物化进 config，避免两处漂移），差异在打印面讲清楚。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityScenario {
    /// 家用单机：basic（出厂形态）。
    Personal,
    /// 开发机：standard（流控 + 配额 + charter WARN）。
    Dev,
    /// 常驻服务器：strict（+ 执行体隔离 + plan 默认 + 装前扫描强制）。
    Server,
    /// 低资源设备：basic（资源裁剪是构建期 customize 的事，config 与
    /// personal 同面——模板诚实注明，不造假差异）。
    Iot,
}

impl SecurityScenario {
    /// 按名解析（大小写不敏感）。未知名 = None（CLI loud 报错列清单）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "personal" => Some(Self::Personal),
            "dev" => Some(Self::Dev),
            "server" => Some(Self::Server),
            "iot" => Some(Self::Iot),
            _ => None,
        }
    }

    /// 场景对应的安全档位（`security.level` 值）。
    pub fn level_str(self) -> &'static str {
        match self {
            Self::Personal | Self::Iot => "basic",
            Self::Dev => "standard",
            Self::Server => "strict",
        }
    }

    /// 场景说明（apply 时打印——用户看得见这个模板给了什么）。
    pub fn description(self) -> &'static str {
        match self {
            Self::Personal => "家用单机（basic 档：8 层管线 + 聊天面扫描缺省；流控/配额/隔离关）",
            Self::Dev => {
                "开发机（standard 档：basic + 网关流控 + 日/月 token 配额 2M/20M + charter 未登记 WARN）"
            }
            Self::Server => {
                "常驻服务器（strict 档：standard + 执行体隔离开 + 默认 plan 模式 + MCP/技能装前扫描强制）"
            }
            Self::Iot => {
                "低资源设备（basic 档；资源裁剪走构建期 scripts/customize.sh iot——config 面与 personal 相同）"
            }
        }
    }
}

/// `onboard template <场景>`：同 [`onboard_default`] 12 步初始化，然后把
/// 场景档位写进 config.json 的 `security.level`（merge 语义——只动这一个
/// 键，其余配置原样保留），最后打印场景/档位说明。
pub fn onboard_template(
    home: &Path,
    local: bool,
    scenario: SecurityScenario,
) -> anyhow::Result<()> {
    onboard_default(home, local, OnboardMode::Cli)?;

    let cfg_path = common::config_path(home);
    let mut raw: serde_json::Value = if cfg_path.exists() {
        let text = std::fs::read_to_string(&cfg_path)?;
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("config.json 解析失败（{}）：{e}", cfg_path.display()))?
    } else {
        serde_json::json!({})
    };
    raw["security"]["level"] = serde_json::json!(scenario.level_str());
    nemesis_utils::write_file_atomic(
        &cfg_path.to_string_lossy(),
        serde_json::to_string_pretty(&raw)?.as_bytes(),
        0o600,
    )
    .map_err(|e| anyhow::anyhow!("config.json 写入失败（{}）：{e}", cfg_path.display()))?;

    println!(
        "  Template: {} —— {}",
        scenario_name(scenario),
        scenario.description()
    );
    println!(
        "  security.level = \"{}\" 已写入 {}（显式开关优先于档位表；各键缺席时按档位取缺省）",
        scenario.level_str(),
        cfg_path.display()
    );
    Ok(())
}

/// 场景显示名（打印面用）。
fn scenario_name(s: SecurityScenario) -> &'static str {
    match s {
        SecurityScenario::Personal => "personal",
        SecurityScenario::Dev => "dev",
        SecurityScenario::Server => "server",
        SecurityScenario::Iot => "iot",
    }
}

#[cfg(test)]
mod tests;
