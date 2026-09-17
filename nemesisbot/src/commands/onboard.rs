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
        // [2026-08-27 R9 死码处置·简化] 原 match 的 Err(_) =>
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

            // Disable workspace restriction (security module enforces rules)
            if let Some(agents) = cfg.get_mut("agents").and_then(|v| v.get_mut("defaults"))
                && let Some(obj) = agents.as_object_mut()
            {
                obj.insert(
                    "restrict_to_workspace".to_string(),
                    serde_json::Value::Bool(false),
                );
                if local {
                    obj.insert(
                        "workspace".to_string(),
                        serde_json::Value::String(".nemesisbot/workspace".to_string()),
                    );
                }
            }

            // Set web auth token, port, websocket
            if let Some(web) = cfg.pointer_mut("/channels/web")
                && let Some(obj) = web.as_object_mut()
            {
                obj.insert(
                    "auth_token".to_string(),
                    serde_json::Value::String("276793422".to_string()),
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

            std::fs::write(
                &cfg_path,
                serde_json::to_string_pretty(&cfg).unwrap_or_default(),
            )?;
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
    // [2026-08-27 R9 死码处置·简化] from_str 对编译期嵌入常量不可能失败
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
        let _ = std::fs::write(
            &cluster_cfg_path,
            serde_json::to_string_pretty(&cluster_cfg).unwrap_or_default(),
        );
        println!("  Cluster config created");
    }

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
            let _ = std::fs::write(&peers_path, peers_content);
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
    // AA3：路径走 rules_file_path 单一真相源（手拼 workspace/config
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
            let _ = std::fs::write(&rules_path, crate::eval_assessor::DEFAULT_RULES_JSON);
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
    // copyDefaultFiles，re-onboard 修复损坏人格）。Seed：only-if-absent——
    // 用户装过的人格（persona install/activate）绝不被 auto-init clobber。
    install_if_absent(
        &workspace_dir.join("IDENTITY.md"),
        crate::DEFAULT_IDENTITY,
        mode,
    );
    install_if_absent(&workspace_dir.join("SOUL.md"), crate::DEFAULT_SOUL, mode);
    install_if_absent(&workspace_dir.join("USER.md"), crate::DEFAULT_USER, mode);
    // Cluster identity — extracted to workspace/cluster/IDENTITY.md.
    let cluster_id_dir = nemesis_path::cluster_dir_in_workspace(&workspace_dir);
    let _ = std::fs::create_dir_all(&cluster_id_dir);
    install_if_absent(
        &cluster_id_dir.join("IDENTITY.md"),
        crate::DEFAULT_IDENTITY_CLUSTER,
        mode,
    );
    println!(
        "  Default personality files installed (IDENTITY.md, SOUL.md, USER.md, cluster/IDENTITY.md)"
    );

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
        let _ = std::fs::remove_file(&bootstrap);
        println!("  BOOTSTRAP.md removed");
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
    let _ = std::fs::write(path, content);
    println!("  {} created", label);
}

/// 人格文件安装（按模式分叉）：Cli = 总是覆盖；Seed = only-if-absent。
fn install_if_absent(path: &Path, content: &str, mode: OnboardMode) {
    if matches!(mode, OnboardMode::Seed) && path.exists() {
        return;
    }
    let _ = std::fs::write(path, content);
}

#[cfg(test)]
mod tests;
