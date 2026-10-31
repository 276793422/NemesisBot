//! Model command - manage LLM models.

use crate::common;
use anyhow::Result;
use chrono::TimeZone;
use std::path::Path;

#[derive(clap::Subcommand)]
pub enum ModelAction {
    /// Add a new model configuration
    Add {
        /// Model name in vendor/model format (e.g., zhipu/glm-4.7).
        /// Optional when --provider is given: the default model of the preset
        /// family is used automatically.
        #[arg(long)]
        model: Option<String>,
        /// Built-in provider family (P9 preset table, e.g. zhipu / groq /
        /// deepseek / ollama). Auto-fills api_base / protocol / default model;
        /// explicit --model/--base/--protocol flags take precedence.
        #[arg(long)]
        provider: Option<String>,
        /// API key for the model
        #[arg(long)]
        key: Option<String>,
        /// Custom base URL
        #[arg(long)]
        base: Option<String>,
        /// Proxy URL for the model
        #[arg(long)]
        proxy: Option<String>,
        /// Authentication method (e.g., "oauth", "token")
        #[arg(long)]
        auth: Option<String>,
        /// Explicit wire protocol (LLM protocol selector): anthropic | openai | responses.
        /// Omit = auto-infer from the provider/ prefix (legacy behavior).
        /// "claude" is accepted as an alias of "anthropic".
        #[arg(long)]
        protocol: Option<String>,
        /// Declared model role (2026-10-13): conversation | utility | classifier.
        /// Omit = conversation. utility/classifier entries are chat-shaped but
        /// rejected from the default conversation slot (--default / WSAPI
        /// models.set_default / gateway assembly).
        #[arg(long)]
        role: Option<String>,
        /// Per-model streaming switch: yes | no（别名 true/false）。
        /// Omit = auto（codex lane 流式、anthropic/openai 兼容 lane 非流式）。
        #[arg(long)]
        stream: Option<String>,
        /// Set as default model
        #[arg(long, default_value_t = false)]
        default: bool,
    },
    /// List configured models
    List {
        /// Show verbose output with all details
        #[arg(short, long, default_value_t = false)]
        verbose: bool,
    },
    /// Remove a model configuration
    Remove {
        /// Model name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
    /// Show default model
    Default,
    /// Set the capability tier for a model (auto/mini/normal/big).
    /// Overrides auto-detection — "user knows best".
    SetTier {
        /// Model name (alias or vendor/model)
        name: String,
        /// One of: auto, mini, normal, big
        tier: String,
    },
    /// Set the parameter size for a model (e.g. 30B, 70B, 120B).
    /// Refines auto tier detection when the alias is opaque.
    SetSize {
        name: String,
        /// Size with optional B suffix, e.g. "30B", "9b", "120"
        size: String,
    },
    /// Set the real model name (for opaque aliases like "astron-code-latest").
    /// Refines auto tier detection.
    SetRealName {
        name: String,
        /// Real model name, e.g. "Qwen3-30B-A3B"
        real_name: String,
    },
    /// per-model 流式开关（2026-10-13 stream 键）：yes/no（别名 true/false）
    /// 强制流式/非流式请求形态；auto 或空串清除回 lane 默认（codex=流式，
    /// anthropic/openai 兼容=非流式）。构造期消费：默认模型需重新设默认
    /// 或重启生效。
    SetStream {
        name: String,
        /// One of: auto | yes | no（别名 true/false）
        stream: String,
    },
    /// per-model 绝对压缩点（2026-10-20 compact_context_tokens）：设 N =
    /// 上下文到 N token 即压缩（不乘比例，解决「1M 模型但 200K 压一次」）；
    /// auto / 0 / 空串清除回默认链（窗口×75%）。超过真实窗口时运行期钳到
    /// 窗口（一次性 warn）。每轮压缩检查前重读配置——下一轮即生效。
    SetCompactContext {
        name: String,
        /// token 数，或 auto / 0 清除
        tokens: String,
    },
    /// H4 (U16 half): set the reasoning-effort tier for a model.
    /// "off" clears it (send nothing); low/medium/high set the tier that
    /// providers translate into their wire format.
    SetEffort {
        name: String,
        /// One of: off | low | medium | high
        effort: String,
    },
    /// Run a capability probe — sends 8 short tasks (7 tool-use + 1 vision
    /// probe) to the model and writes the detected tier (and vision probe
    /// result) to config. Costs ~8 LLM calls. Explicit only.
    Probe { name: String },
    /// U16 (sixth batch): fetch the models.dev catalog (context windows /
    /// max output tokens per model) and cache it at
    /// `<home>/workspace/data/models_catalog.json`. `model add` auto-fills
    /// from the cache.
    CatalogUpdate,
    /// 价目表管理（A2 在线更新：LiteLLM 主源 + 自定义条目 + 离线导入）。
    /// 查表优先级：自定义 > 下载 > 内置（内置 36 模型表离线兜底）。
    Prices {
        #[command(subcommand)]
        action: PricesAction,
    },
}

#[derive(clap::Subcommand)]
pub enum PricesAction {
    /// 显示分层价目表概况（各层条数 + 下载元数据 + 自定义条目明细）。
    List,
    /// 在线拉取最新价目表并整体替换下载层（LiteLLM 主源，ETag 增量；
    /// 失败保留旧表 + 退码非 0）。
    Update {
        /// 镜像地址覆盖（缺省 = LiteLLM 官方 raw 地址）。
        #[arg(long)]
        url: Option<String>,
    },
    /// 从本地文件导入 LiteLLM 原始 JSON（离线环境兜底：外网机器下载后拷入）。
    Import {
        /// LiteLLM model_prices_and_context_window.json 文件路径。
        file: String,
    },
    /// 新增/更新自定义价目条目（最高查表优先级，按模型名幂等）。
    Add {
        /// 模型名（与 `model add` 一致，如 zhipu/glm-4.7；裸名亦可）。
        model: String,
        /// 输入价（USD / 百万 token）。
        #[arg(long)]
        input: f64,
        /// 输出价（USD / 百万 token）。
        #[arg(long)]
        output: f64,
        /// 缓存读价（USD / 百万 token）。
        #[arg(long, default_value_t = 0.0)]
        cache_read: f64,
        /// 缓存写价（USD / 百万 token）。
        #[arg(long, default_value_t = 0.0)]
        cache_creation: f64,
        /// 显示名（可选，缺省用模型名）。
        #[arg(long)]
        display: Option<String>,
        /// Prompt-length 阶梯价（L3，可重复）：`<min_prompt_tokens>:<input>:<output>`
        /// （USD / 百万 token；input_tokens ≥ min 时该档生效，多条命中取
        /// 阈值最大档；缓存价继承基础价——需精细控制走 Dashboard 属性编辑器）。
        #[arg(long = "price-tier")]
        price_tiers: Vec<String>,
    },
    /// 删除自定义条目（只影响自定义层；下载/内置层不动）。
    Remove {
        /// 自定义条目模型名（`prices add` 时的 model）。
        model: String,
    },
}

/// 守卫①生效键（M4 + 2026-10-13 N6）：新 entry 显式键优先，重复 add 回退
/// 旧条目——与 model_list 合并的 `or_insert`（旧键只填缺）语义一致。只看
/// 新 entry 会漏「旧条目是 images-openai/utility，重复 `--default` 仍把
/// 它指到主对话槽」的旁路（两守卫同款洞，合并生效值一并封死）。
fn effective_entry_key(
    entry: &serde_json::Value,
    existing: Option<&serde_json::Value>,
    key: &str,
) -> String {
    entry
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            existing
                .and_then(|e| e.get(key))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

pub async fn run(action: ModelAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);

    match action {
        ModelAction::Add {
            model,
            provider,
            key,
            base,
            proxy,
            auth,
            protocol,
            role,
            stream,
            default,
        } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }

            // P9（能力扩展 WS5）：--provider 家族预设——查内置表自动补
            // api_base / protocol / 默认型号；显式 --model / --base /
            // --protocol 恒优先。未知家族 loud 报错并列出全部可用家族。
            let preset = match provider.as_deref() {
                Some(name) => {
                    Some(nemesis_config::find_provider_preset(name).ok_or_else(|| {
                        anyhow::anyhow!(
                            "Unknown provider family '{}'. Available families:\n  {}",
                            name,
                            nemesis_config::provider_preset_ids().join(", ")
                        )
                    })?)
                }
                None => None,
            };
            let model = match (&model, preset) {
                (Some(m), _) => m.clone(),
                (None, Some(p)) => {
                    if p.default_model.is_empty() {
                        anyhow::bail!(
                            "Provider family '{}' has no built-in default model. \
                             Pass --model explicitly (vendor/model format).",
                            p.id
                        );
                    }
                    let auto = format!("{}/{}", p.id, p.default_model);
                    println!(
                        "  Provider preset '{}': model auto-filled as {}",
                        p.id, auto
                    );
                    auto
                }
                (None, None) => anyhow::bail!(
                    "Either --model or --provider is required.\n\
                     Example: nemesisbot model add --model zhipu/glm-4.7 --key KEY\n\
                     Or:      nemesisbot model add --provider zhipu --key KEY\n\
                     Families: {}",
                    nemesis_config::provider_preset_ids().join(", ")
                ),
            };

            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;

            // Validate model identifier format: must be vendor/model
            if !model.contains('/') {
                anyhow::bail!(
                    "Invalid model identifier '{}'. Expected format: vendor/model\n\
                     Example: openai/gpt-4o, anthropic/claude-sonnet-4",
                    model
                );
            }

            // Parse vendor and model name
            let parts: Vec<&str> = model.splitn(2, '/').collect();
            let model_name_alias = match parts.len() {
                2 => parts[1].to_string(),
                _ => model.clone(),
            };

            // F-I（2026-09-04 四轮盲审）：add 前先查重——重复 add 是「更新」语义，
            // 走下方 or_insert_with 合并；若这里无条件写 model_tier="auto"，
            // 新条目已带该键 → 合并填不进旧值 → 用户 set-tier / probe 写入的
            // tier 被静默重置为 auto（auto→big 默认，小模型会被发全量工具）。
            let already_exists = cfg
                .get("model_list")
                .and_then(|v| v.as_array())
                .is_some_and(|arr| {
                    arr.iter()
                        .any(|m| m.get("model").and_then(|v| v.as_str()) == Some(model.as_str()))
                });

            // Build model entry
            let mut entry = serde_json::json!({
                "model_name": model_name_alias,
                "model": model.clone(),
            });
            if let Some(k) = &key {
                entry["api_key"] = serde_json::Value::String(k.clone());
            }
            if let Some(b) = &base {
                entry["api_base"] = serde_json::Value::String(b.clone());
            } else if let Some(p) = preset {
                // 未给 --base：家族预设端点自动补上（显式 flag 恒优先）。
                entry["api_base"] = serde_json::Value::String(p.api_base.to_string());
            }
            if let Some(p) = &proxy {
                entry["proxy"] = serde_json::Value::String(p.clone());
            }
            if let Some(a) = &auth {
                entry["auth_method"] = serde_json::Value::String(a.clone());
            }
            // LLM 协议选择器（2026-09-11）：显式协议写盘；校验/归一走单一
            // 真相源（claude→anthropic），未知值 loud 报错退出。
            if let Some(p) = &protocol {
                let normalized = nemesis_types::capability::normalize_model_protocol(p)
                    .map_err(anyhow::Error::msg)?;
                entry["protocol"] = serde_json::Value::String(normalized);
            } else if let Some(p) = preset {
                // 表内协议均为归一值（完整性测试钉住），直接写盘即可。
                entry["protocol"] = serde_json::Value::String(p.protocol.to_string());
            }
            // per-model 流式开关（2026-10-13 stream 键）：归一走单一真相源
            // normalize_stream_value（与配置解析同源），非法值 loud 报错
            // （CLI 显式 flag 不做静默忽略——与配置层「未知值落 auto」的分
            // 工是刻意的：flag 是用户当次意志，错了就该说）。
            if let Some(s) = &stream {
                let normalized = nemesis_config::normalize_stream_value(&serde_json::json!(s))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Invalid --stream value '{}'. Use yes | no (aliases: true | false).",
                            s
                        )
                    })?;
                entry["stream"] = serde_json::Value::String(normalized);
            }
            // 集群专业职能框架 M4：图像协议条目唯一消费点 = generate_image 工具
            // （经 tools.image_gen / 唯一 images-openai 条目解析），不能坐上
            // agent 默认对话模型槽。--default 显式请求 = loud 拒绝（守卫①）；
            // 仅一条模型时的自动默认（守卫①b）也跳过图像条目，诚实打印说明。
            // N6（2026-10-13）：守卫判定用**合并生效值**——重复 add 不带
            // --protocol/--role 时新 entry 缺键、旧条目经下方 merge 回填，
            // 只看新 entry 会漏掉「旧条目是 images-openai/utility 仍被
            // --default 指到主对话槽」的旁路（两守卫同款洞，一并修）。
            let existing_entry = cfg
                .get("model_list")
                .and_then(|v| v.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(model.as_str()))
                })
                .cloned();
            // 生效键 = 新 entry 显式键优先，重复 add 时回退旧条目（与下方
            // merge 的 or_insert 语义一致）。
            let entry_protocol = effective_entry_key(&entry, existing_entry.as_ref(), "protocol");
            let is_image_entry = nemesis_types::capability::is_image_protocol(&entry_protocol);
            if default && is_image_entry {
                anyhow::bail!(
                    "protocol 'images-openai' 是图像生成协议，不能设为默认对话模型；\
                     请去掉 --default 重新添加（generate_image 工具会自动解析该条目）"
                );
            }
            // 声明式模型角色（2026-10-13 N6）：显式 --role 写盘；校验/归一走
            // 单一真相源，未知值 loud 报错退出。utility/classifier 是对话形态
            // 条目（probe 照常），但不是默认对话模型槽的合法形态——守卫①
            // 同款 loud 拒绝（agents.small_model 才是这类条目的消费点）。
            if let Some(r) = &role {
                let normalized = nemesis_types::capability::normalize_model_role(r)
                    .map_err(anyhow::Error::msg)?;
                entry["role"] = serde_json::Value::String(normalized);
            }
            let entry_role = effective_entry_key(&entry, existing_entry.as_ref(), "role");
            if default && !nemesis_types::capability::is_conversation_role(&entry_role) {
                anyhow::bail!(
                    "role '{}' 不是默认对话模型槽的合法形态（只有 conversation 可以）；\
                     请去掉 --default 重新添加（杂务消费走 agents.small_model 指向该条目）",
                    entry_role
                );
            }

            // Phase 4a (small-model-tool-robustness): tag with an auto-detect
            // tier. Resolved at runtime from the model name (and any real_name /
            // model_size_b the user adds later). Override with `model set-tier`.
            // F-I：只对**新增**条目写 auto——重复 add 由下方合并保留旧 tier。
            if !already_exists {
                entry["model_tier"] = serde_json::Value::String("auto".to_string());
            }

            // U16 (sixth batch): auto-fill context_window / max_output_tokens
            // from the models.dev catalog cache when the model is a catalog
            // hit and the user didn't set them otherwise (there is no CLI
            // flag for these today — catalog is the only writer besides a
            // manual config edit). Falls back silently when no/empty cache:
            // `model catalog update` populates it.
            {
                let cfg_dir = cfg_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                if let Ok(Some(cat)) = catalog::load_cache(&cfg_dir)
                    && let Some(hit) = catalog::lookup(&cat, &model)
                {
                    entry["context_window"] = serde_json::Value::Number(hit.context_window.into());
                    if let Some(mot) = hit.max_output_tokens {
                        entry["max_output_tokens"] = serde_json::Value::Number(mot.into());
                    }
                    println!(
                        "  Catalog hit (models.dev): context_window={}, max_output_tokens={}",
                        hit.context_window,
                        hit.max_output_tokens
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "(not declared)".to_string())
                    );
                }
            }

            // Add to model list
            if let Some(obj) = cfg.as_object_mut() {
                if let Some(models) = obj.get_mut("model_list") {
                    if let Some(arr) = models.as_array_mut() {
                        // Check for duplicate and warn
                        let existing = arr
                            .iter()
                            .find(|m| m.get("model").and_then(|v| v.as_str()) == Some(&model));
                        if let Some(existing) = existing {
                            let existing_name = existing
                                .get("model_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            println!(
                                "  Warning: Model '{}' already exists (alias: {}), updating...",
                                model, existing_name
                            );
                        }
                        // Remove existing entry with same model name
                        // （2026-09-03 二次回归 model-1：替换时保留旧条目的未类型化
                        // 键——model_tier / vision / vision_probe / max_output_tokens
                        // 等是 probe / set-tier / catalog 写进来的资产，重复 add
                        // 不该静默清掉；新条目显式设置的键以新值为准。
                        // F-I（2026-09-04）：model_tier 现在只对新增条目写入
                        // （见上方 already_exists 门），重复 add 走这里回填旧值。）
                        if let Some(old) = existing
                            && let (Some(new_obj), Some(old_obj)) =
                                (entry.as_object_mut(), old.as_object())
                        {
                            for (k, v) in old_obj {
                                new_obj.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                        arr.retain(|m| m.get("model").and_then(|v| v.as_str()) != Some(&model));
                        arr.push(entry);
                    }
                } else {
                    obj.insert("model_list".to_string(), serde_json::json!([entry]));
                }

                // Set as default if requested
                if default {
                    // Set agents.defaults.llm so get_effective_llm picks it up.
                    // Only set the alias (part after '/') as the default model,
                    // matching Go's behavior. Go does NOT set a top-level default_model field.
                    let alias = model.split('/').next_back().unwrap_or(&model).to_string();
                    let agents = obj.entry("agents").or_insert_with(|| serde_json::json!({}));
                    if let Some(agents_obj) = agents.as_object_mut() {
                        let defaults = agents_obj
                            .entry("defaults")
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(defaults_obj) = defaults.as_object_mut() {
                            defaults_obj
                                .insert("llm".to_string(), serde_json::Value::String(alias));
                        }
                    }
                }

                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
            }

            println!("Model added: {}", model);
            // Phase 4a: print the auto-detected capability tier (advisory).
            {
                let hint = nemesis_types::capability::TierHint {
                    full_model: Some(model.clone()),
                    real_name: None,
                    size_b: None,
                };
                let resolved = nemesis_types::capability::detect_tier(&hint);
                if resolved == nemesis_types::capability::ModelTier::Big {
                    println!(
                        "  → 能力档位：big（全量工具）。若此模型实际是小模型（如 30B 左右），建议：nemesisbot model set-tier {} mini",
                        model_name_alias
                    );
                } else {
                    println!("  → 能力档位：{}（自动检测）", resolved);
                }
            }
            if default {
                println!("Set as default model.");
            } else {
                // Auto-default: if this is the only model and no default is set,
                // automatically make it the default (matches user expectation).
                let model_count = cfg
                    .get("model_list")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let current_default = cfg
                    .get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get("llm"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if model_count == 1 && current_default.is_empty() {
                    if is_image_entry {
                        // 守卫①b：图像协议条目不自动坐默认对话模型槽（唯一消费点
                        // = generate_image 工具），诚实打印而非悄悄写 agents.defaults.llm。
                        println!(
                            "  Note: 'images-openai' 协议条目不自动设为默认对话模型（仅供 generate_image 工具消费）"
                        );
                    } else if !nemesis_types::capability::is_conversation_role(&entry_role) {
                        // 守卫①b（N6）：utility/classifier 条目同样不自动坐槽。
                        println!(
                            "  Note: role '{}' 条目不自动设为默认对话模型（杂务消费走 agents.small_model）",
                            entry_role
                        );
                    } else {
                        // Auto-set as default
                        let alias = model.split('/').next_back().unwrap_or(&model).to_string();
                        if let Some(obj) = cfg.as_object_mut() {
                            let agents =
                                obj.entry("agents").or_insert_with(|| serde_json::json!({}));
                            if let Some(agents_obj) = agents.as_object_mut() {
                                let defaults = agents_obj
                                    .entry("defaults")
                                    .or_insert_with(|| serde_json::json!({}));
                                if let Some(defaults_obj) = defaults.as_object_mut() {
                                    defaults_obj.insert(
                                        "llm".to_string(),
                                        serde_json::Value::String(alias.clone()),
                                    );
                                }
                            }
                            // REL-002：统一原子写入。
                            write_config_atomic(&cfg_path, &cfg)?;
                        }
                        println!(
                            "Auto-set as default model (only model configured): {}",
                            alias
                        );
                    }
                }
            }
        }
        ModelAction::List { verbose } => {
            println!("Configured Models");
            println!("==================");
            if !cfg_path.exists() {
                println!("  No configuration found. Run 'nemesisbot onboard default' first.");
                return Ok(());
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let cfg: serde_json::Value = serde_json::from_str(&data)?;

            // Check agents.defaults.llm first (like Go's GetEffectiveLLM), then fall back to default_model
            let default_model = cfg
                .get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()))
                .unwrap_or("(none)");

            println!("  Default: {}", default_model);
            println!();
            // N1 (devtool-upgrade 阶段 1)：生效 context_window + 来源标记
            // （config 显式 > 价目表 catalog > fallback-128k）。价目表打开
            // 失败 → None，catalog 级静默跳过（fallback 链兜底）。
            let pricing =
                nemesis_data::PricingStore::open(&nemesis_path::workspace_data_dir(&home)).ok();

            if let Some(models) = cfg.get("model_list").and_then(|v| v.as_array()) {
                if models.is_empty() {
                    println!("  No models configured.");
                    println!(
                        "  Add one with: nemesisbot model add --model <vendor/model> --key <key>"
                    );
                } else {
                    for m in models {
                        let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                        let model_name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                        let has_key = m
                            .get("api_key")
                            .and_then(|v| v.as_str())
                            .map(|k| !k.is_empty())
                            .unwrap_or(false);
                        let base = m.get("api_base").and_then(|v| v.as_str());
                        let proxy = m.get("proxy").and_then(|v| v.as_str());
                        let auth_method = m.get("auth_method").and_then(|v| v.as_str());
                        // Match by model_name (alias) or full model identifier
                        let is_default = model == default_model || model_name == default_model;

                        // N1: effective context window + source for THIS alias.
                        let alias_for_window = if model_name.is_empty() {
                            model
                        } else {
                            model_name
                        };
                        let (window, source) = nemesis_agent::r#loop::resolve_context_window_tiered(
                            Some(&cfg),
                            alias_for_window,
                            pricing.as_ref(),
                        );
                        let window_display = window.map(|w| w.to_string()).unwrap_or_else(|| {
                            nemesis_agent::r#loop::FALLBACK_CONTEXT_WINDOW.to_string()
                        });

                        println!("  {} {}", if is_default { "*" } else { " " }, model);
                        println!("    Context window: {} ({})", window_display, source);
                        println!(
                            "    API key: {}",
                            if has_key { "configured" } else { "not set" }
                        );
                        if let Some(b) = base {
                            println!("    Base URL: {}", b);
                        }
                        if verbose {
                            // 有效压缩点 + 来源（2026-10-20 compact_context_tokens；
                            // 未设 = 窗口×75% 默认链，被钳时 clamped_from 带原值）。
                            let trig = nemesis_agent::r#loop::resolve_compact_trigger(
                                Some(&cfg),
                                alias_for_window,
                                pricing.as_ref(),
                                (
                                    nemesis_agent::r#loop::FALLBACK_CONTEXT_WINDOW,
                                    "fallback-128k",
                                ),
                            );
                            match trig.clamped_from {
                                Some(orig) => println!(
                                    "    Compact trigger: {} tokens (user-set {} clamped to window {}, source: {})",
                                    trig.trigger, orig, trig.window, trig.source
                                ),
                                None => println!(
                                    "    Compact trigger: {} tokens (window {}, source: {})",
                                    trig.trigger, trig.window, trig.source
                                ),
                            }
                            // Show masked key as dots
                            if let Some(k) = m.get("api_key").and_then(|v| v.as_str()) {
                                if !k.is_empty() {
                                    println!("    API Key: {}", "\u{2022}".repeat(8));
                                } else {
                                    println!("    API Key: (not set)");
                                }
                            }
                            if let Some(b) = base {
                                println!("    API Base: {}", b);
                            }
                            if let Some(p) = proxy
                                && !p.is_empty()
                            {
                                println!("    Proxy: {}", p);
                            }
                            if let Some(a) = auth_method
                                && !a.is_empty()
                            {
                                println!("    Auth Method: {}", a);
                            }
                        }
                    }
                }
            } else {
                println!("  No models configured.");
            }
        }
        ModelAction::Remove { name, force } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;

            // Check if this model is the current default via agents.defaults.llm
            let default_model = cfg
                .get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Also check top-level default_model for backward compatibility
            let default_model_compat = cfg
                .get("default_model")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let model_list = cfg
                .get("model_list")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let is_default = name == default_model
                || name == default_model_compat
                || model_list.iter().any(|m| {
                    let full_model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                    let alias = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                    (full_model == name || full_model.ends_with(&format!("/{}", name)))
                        && (full_model == default_model
                            || alias == default_model
                            || full_model == default_model_compat
                            || alias == default_model_compat)
                });

            if is_default {
                println!(
                    "  Error: Cannot remove model '{}' - it is the current default.",
                    name
                );
                println!("  Change the default first: nemesisbot agent set llm <other-model>");
                return Ok(());
            }

            // Confirmation prompt
            if !force {
                use std::io::{self, Write};
                print!("Remove model '{}'? (y/N): ", name);
                io::stdout().flush().ok();
                let mut response = String::new();
                io::stdin().read_line(&mut response).ok();
                if response.trim().to_lowercase() != "y" {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let mut found = false;
            if let Some(obj) = cfg.as_object_mut()
                && let Some(models) = obj.get_mut("model_list")
                && let Some(arr) = models.as_array_mut()
            {
                arr.retain(|m| {
                    let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                    if model == name || model.ends_with(&format!("/{}", name)) {
                        found = true;
                        false
                    } else {
                        true
                    }
                });
            }

            if found {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                println!("Model removed: {}", name);
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::Default => {
            if !cfg_path.exists() {
                println!("No configuration found.");
                return Ok(());
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let cfg: serde_json::Value = serde_json::from_str(&data)?;
            // Check agents.defaults.llm first (like Go's GetEffectiveLLM), then fall back to default_model
            let default_model = cfg
                .get("agents")
                .and_then(|a| a.get("defaults"))
                .and_then(|d| d.get("llm"))
                .and_then(|v| v.as_str())
                .or_else(|| cfg.get("default_model").and_then(|v| v.as_str()));
            match default_model {
                Some(m) => println!("Default model: {}", m),
                None => println!("No default model configured."),
            }
        }
        ModelAction::SetTier { name, tier } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            let parsed: nemesis_types::capability::ModelTier =
                serde_json::from_value(serde_json::Value::String(tier.clone())).map_err(|_| {
                    anyhow::anyhow!(
                        "Invalid tier '{}'. Use one of: auto | mini | normal | big",
                        tier
                    )
                })?;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["model_tier"] = serde_json::Value::String(parsed.to_string());
            });
            if updated {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                println!("✓ {} → model_tier={}", name, parsed);
                // tier 热生效：AgentLoop 每轮 LLM 前 check_config_reload()（mtime
                // 检测）自动重解析，切档无需重启（旧文案"需重启"是错的，二次回归
                // model-2 修正；与 set-effort 文案同语义）。
                println!("  (生效于下次 LLM 调用前的 config 重读)");
                // F8：tier 决定「档位工具集」，`agents.hidden_tools` 决定「彻底
                // 隐藏」——两者叠加。若切档后仍看不到某工具，检查是否被隐藏。
                println!(
                    "  提示：工具供给 = tier 档位 ∩ 未隐藏（agents.hidden_tools 可通配隐藏，如 \"mcp_*\"）；详见 TOOLS.md"
                );
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetEffort { name, effort } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            let e = effort.to_lowercase();
            if !matches!(e.as_str(), "off" | "low" | "medium" | "high") {
                anyhow::bail!(
                    "Invalid effort '{}'. Use one of: off | low | medium | high",
                    effort
                );
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            // "off" clears the field (absent = send nothing); a tier writes it.
            let value = if e == "off" {
                serde_json::Value::String(String::new())
            } else {
                serde_json::Value::String(e.clone())
            };
            let updated = update_model_entry(&mut cfg, &name, |en| {
                en["reasoning_effort"] = value.clone();
            });
            if updated {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                if e == "off" {
                    println!("✓ {} → reasoning_effort cleared", name);
                } else {
                    println!("✓ {} → reasoning_effort={}", name, e);
                }
                println!("  (生效于下次 LLM 调用前的 config 重读)");
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetSize { name, size } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            // Accept "30B", "30b", or "30" — normalize to whole billions.
            let size_b = nemesis_types::capability::parse_size_marker(&size)
                .or_else(|| size.trim().parse::<u32>().ok())
                .ok_or_else(|| {
                    anyhow::anyhow!("Invalid size '{}'. Examples: 30B, 9b, 70, 120B", size)
                })?;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let resolved = nemesis_types::capability::tier_from_size_b(size_b);
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["model_size_b"] = serde_json::Value::Number(size_b.into());
            });
            if updated {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                println!(
                    "✓ {} → model_size_b={} (auto 检测将解析为 tier={})",
                    name, size_b, resolved
                );
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetRealName { name, real_name } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["real_name"] = serde_json::Value::String(real_name.clone());
            });
            // Show what the auto-detection resolves to now.
            let hint = nemesis_types::capability::TierHint {
                full_model: None,
                real_name: Some(real_name.clone()),
                size_b: None,
            };
            let resolved = nemesis_types::capability::detect_tier(&hint);
            if updated {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                println!(
                    "✓ {} → real_name=\"{}\" (auto 检测将解析为 tier={})",
                    name, real_name, resolved
                );
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetStream { name, stream } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            // 归一走单一真相源 normalize_stream_value（与 model add --stream /
            // web update_field / 配置解析同源）：auto/空串清除（写空串 =
            // 跟随 lane 默认），yes/no（别名 true/false）归一写盘，未知值
            // loud 拒绝——flag 是用户当次意志，不静默纠偏。
            let normalized = if stream.is_empty() || stream.eq_ignore_ascii_case("auto") {
                String::new()
            } else {
                nemesis_config::normalize_stream_value(&serde_json::json!(stream)).ok_or_else(
                    || {
                        anyhow::anyhow!(
                            "Invalid stream value '{}'. Use yes | no (aliases: true | false), or auto to reset.",
                            stream
                        )
                    },
                )?
            };
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                e["stream"] = serde_json::Value::String(normalized.clone());
            });
            if updated {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                let shown = if normalized.is_empty() {
                    "auto".to_string()
                } else {
                    normalized
                };
                println!(
                    "✓ {} → stream={} (构造期消费：默认模型需重新设默认或重启生效)",
                    name, shown
                );
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::SetCompactContext { name, tokens } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found.");
            }
            // 数字 = 到点即压（不乘比例）；auto / 0 / 空串 = 清除（移键落盘，
            // 回窗口×75% 默认链）。非法值 loud 拒绝——flag 是用户当次意志，
            // 不静默纠偏。生效走每轮压缩检查前的 config fresh-read，下一轮
            // 即生效（无需重启）。
            let trimmed = tokens.trim();
            let value: u64 = if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
                0
            } else {
                trimmed.parse::<u64>().map_err(|_| {
                    anyhow::anyhow!(
                        "Invalid compact_context_tokens '{}'. Use a positive token count, or auto | 0 to clear.",
                        tokens
                    )
                })?
            };
            let cleared = value == 0;
            let data = std::fs::read_to_string(&cfg_path)?;
            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
            let updated = update_model_entry(&mut cfg, &name, |e| {
                if cleared {
                    // 清除 = 移键（写 Null 会留 `"key": null` 字节垃圾；
                    // resolver 对缺席本就按 None 处理）。
                    if let Some(obj) = e.as_object_mut() {
                        obj.remove("compact_context_tokens");
                    }
                } else {
                    e["compact_context_tokens"] = serde_json::Value::Number(value.into());
                }
            });
            if updated {
                // REL-002：统一原子写入。
                write_config_atomic(&cfg_path, &cfg)?;
                if cleared {
                    println!(
                        "✓ {} → compact_context_tokens=auto（清除，回默认链：窗口 × 75%）",
                        name
                    );
                } else {
                    // 展示解析后的有效压缩点（含钳制结果）——超窗口时这里
                    // 直观看到被钳到窗口，而不是等运行期 warn。
                    let pricing =
                        nemesis_data::PricingStore::open(&nemesis_path::workspace_data_dir(&home))
                            .ok();
                    let trig = nemesis_agent::r#loop::resolve_compact_trigger(
                        Some(&cfg),
                        &name,
                        pricing.as_ref(),
                        (
                            nemesis_agent::r#loop::FALLBACK_CONTEXT_WINDOW,
                            "fallback-128k",
                        ),
                    );
                    println!(
                        "✓ {} → compact_context_tokens={}（到点即压，不乘比例；下一轮生效）",
                        name, value
                    );
                    if let Some(orig) = trig.clamped_from {
                        println!(
                            "  ⚠ 设定 {} 超过真实上下文窗口 {}（来源: {}），已钳到窗口；如需更大窗口请设 context_window",
                            orig, trig.window, trig.source
                        );
                    }
                    println!(
                        "  有效压缩点: {} tokens（窗口 {}, 来源: {}）",
                        trig.trigger, trig.window, trig.source
                    );
                }
            } else {
                anyhow::bail!("Model not found: {}", name);
            }
        }
        ModelAction::Probe { name } => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            // 集群专业职能框架 M4 守卫③：探针走对话/工具调用链路，图像协议
            // 条目没有这些形状——诚实拒绝，不烧 8 次注定失败的 LLM 调用。
            {
                let cfg: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)
                        .unwrap_or(serde_json::Value::Null);
                let proto = cfg
                    .get("model_list")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| {
                        arr.iter().find(|m| {
                            m.get("model_name").and_then(|v| v.as_str()) == Some(name.as_str())
                        })
                    })
                    .and_then(|m| m.get("protocol"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if nemesis_types::capability::is_image_protocol(proto) {
                    anyhow::bail!(
                        "'{name}' 是 images-openai 图像协议条目，不支持能力探针\
                         （无对话/工具调用形状；仅供 generate_image 工具消费）"
                    );
                }
            }
            println!(
                "正在对 '{}' 运行能力探针（8 个任务：7 工具 + 1 视觉，约 8 次 LLM 调用，请稍候）...",
                name
            );
            let report = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(run_probe(&home, &name))
            })?;
            // run_probe 返回 (report, wrote)：wrote = tier/vision 写回是否
            // 真正落盘（BUG-20261030-218：报告必须如实反映，不再无条件
            // 宣称"已写入 config.json"）。
            println!("{}", format_probe_report(&name, &report.0, report.1));
            // N1：生效 context_window + 来源标记——fallback 命中自然引导
            // 用户显式配置（catalog 未收录的小众模型手动钉死）。
            let probe_cfg = std::fs::read_to_string(&cfg_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            let probe_pricing =
                nemesis_data::PricingStore::open(&nemesis_path::workspace_data_dir(&home)).ok();
            let (window, source) = nemesis_agent::r#loop::resolve_context_window_tiered(
                probe_cfg.as_ref(),
                &name,
                probe_pricing.as_ref(),
            );
            match window {
                Some(w) => println!("生效上下文窗口: {}（来源: {}）", w, source),
                None => println!(
                    "生效上下文窗口: {}（来源: fallback-128k；小众本地模型建议 config.json 显式配置 context_window）",
                    nemesis_agent::r#loop::FALLBACK_CONTEXT_WINDOW
                ),
            }
        }
        ModelAction::CatalogUpdate => {
            if !cfg_path.exists() {
                anyhow::bail!("Configuration not found. Run 'nemesisbot onboard default' first.");
            }
            println!(
                "正在拉取 models.dev 模型目录（{}，失败自动走 jsDelivr 镜像）...",
                catalog::API_URL
            );
            let cfg_dir = cfg_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            match catalog::fetch_http().await {
                Ok(entries) => {
                    let n = entries.len();
                    catalog::save_cache(&cfg_dir, entries).map_err(|e| {
                        anyhow::anyhow!(
                            "目录写入失败（{}）：{e}",
                            nemesis_path::models_catalog_cache_path(&cfg_dir).display()
                        )
                    })?;
                    println!(
                        "目录已更新：{} 个模型 → {}",
                        n,
                        nemesis_path::models_catalog_cache_path(&cfg_dir).display()
                    );
                    println!(
                        "之后 `model add` 命中的模型会自动填充 context_window / max_output_tokens。"
                    );
                }
                Err(e) => {
                    // Offline/intranet semantics: keep the existing cache,
                    // report loudly, exit non-zero (CLI contract).
                    let cached = catalog::load_cache(&cfg_dir).ok().flatten();
                    match cached {
                        Some(cat) => {
                            println!(
                                "拉取失败（{e}），保留现有缓存：{} 个模型（fetched_at={}）",
                                cat.entries.len(),
                                cat.fetched_at
                            );
                        }
                        None => {
                            anyhow::bail!(
                                "拉取失败且无本地缓存：{e}\n\
                                 内网部署请在外网机器上运行 `model catalog update` 后拷贝 {} 过来。",
                                nemesis_path::models_catalog_cache_path(&cfg_dir).display()
                            );
                        }
                    }
                }
            }
        }
        ModelAction::Prices { action } => {
            run_prices(action, &home).await?;
        }
    }
    Ok(())
}

/// 解析 `--price-tier <min>:<input>:<output>` 重复参数（L3）。解析失败
/// 响亮报错（带条目序号与原文），空列表 = 无阶梯（None）。
fn parse_price_tiers(specs: &[String]) -> Result<Option<Vec<nemesis_data::PriceTier>>> {
    if specs.is_empty() {
        return Ok(None);
    }
    let mut tiers = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let parts: Vec<&str> = spec.split(':').map(str::trim).collect();
        if parts.len() != 3 {
            anyhow::bail!(
                "第 {} 条阶梯价格式错误（{spec:?}）：应为 <min_prompt_tokens>:<input>:<output>",
                i + 1
            );
        }
        let min_prompt_tokens: i64 = parts[0].parse().map_err(|_| {
            anyhow::anyhow!(
                "第 {} 条阶梯价阈值非法（{}）：须为非负整数",
                i + 1,
                parts[0]
            )
        })?;
        if min_prompt_tokens < 0 {
            anyhow::bail!("第 {} 条阶梯价阈值不能为负（{}）", i + 1, parts[0]);
        }
        let input_cost_per_million: f64 = parts[1]
            .parse()
            .map_err(|_| anyhow::anyhow!("第 {} 条阶梯输入价非法（{}）", i + 1, parts[1]))?;
        let output_cost_per_million: f64 = parts[2]
            .parse()
            .map_err(|_| anyhow::anyhow!("第 {} 条阶梯输出价非法（{}）", i + 1, parts[2]))?;
        if !(input_cost_per_million.is_finite() && output_cost_per_million.is_finite()) {
            anyhow::bail!("第 {} 条阶梯价须为有限数值", i + 1);
        }
        tiers.push(nemesis_data::PriceTier {
            min_prompt_tokens,
            input_cost_per_million: Some(input_cost_per_million),
            output_cost_per_million: Some(output_cost_per_million),
            cache_read_cost_per_million: None,
            cache_creation_cost_per_million: None,
        });
    }
    Ok(Some(tiers))
}

/// A2（2026-08-31）：`model prices` 子命令实现。CLI 直开 DataStore（与
/// gateway 同一路径真相源 `workspace_data_dir`），计价/价目管理复用
/// nemesis-data + nemesis-web 的同步实现，不重复造轮子。
async fn run_prices(action: PricesAction, home: &Path) -> Result<()> {
    let db_path = nemesis_path::workspace_data_dir(home).join("nemesisbot_data.db");
    let ds = nemesis_data::DataStore::open(&db_path)
        .map_err(|e| anyhow::anyhow!("DataStore 打开失败（{}）：{e}", db_path.display()))?;
    let pricing = ds.pricing();

    match action {
        PricesAction::List => {
            let custom = pricing.list_custom();
            let downloaded = pricing.list_downloaded();
            let meta = pricing.meta();
            println!("价目表分层概况（查表优先级：自定义 > 下载 > 内置）：");
            println!("  自定义层: {} 条（最高优先）", custom.len());
            match &downloaded {
                Some(dl) => println!(
                    "  下载层:   {} 条（fetched_at={}，source={}，etag={}）",
                    dl.len(),
                    meta.fetched_at
                        .map(|t| chrono::Local
                            .timestamp_opt(t, 0)
                            .single()
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| t.to_string()))
                        .unwrap_or_else(|| "从未".to_string()),
                    meta.source_url.as_deref().unwrap_or("-"),
                    meta.etag.as_deref().unwrap_or("-"),
                ),
                None => println!(
                    "  下载层:   未下载（`model prices update` 在线拉取，或 `import` 离线导入）"
                ),
            }
            println!(
                "  内置层:   {} 条（离线兜底，{}）",
                nemesis_data::all_pricing().len(),
                nemesis_data::embedded_source()
            );
            if !custom.is_empty() {
                println!("自定义条目：");
                for p in &custom {
                    let tiers = match &p.price_tiers {
                        Some(t) if !t.is_empty() => {
                            let mut parts = Vec::new();
                            for t in t {
                                parts.push(format!(
                                    "≥{}:in={:.4} out={:.4}",
                                    t.min_prompt_tokens,
                                    t.input_cost_per_million.unwrap_or(p.input_cost_per_million),
                                    t.output_cost_per_million
                                        .unwrap_or(p.output_cost_per_million)
                                ));
                            }
                            format!("  阶梯：{}", parts.join(" | "))
                        }
                        _ => String::new(),
                    };
                    println!(
                        "  {:<32} in={:.4} out={:.4} cache_read={:.4} cache_creation={:.4} $/Mtok{}",
                        p.model_id,
                        p.input_cost_per_million,
                        p.output_cost_per_million,
                        p.cache_read_cost_per_million,
                        p.cache_creation_cost_per_million,
                        tiers
                    );
                }
            }
        }
        PricesAction::Update { url } => {
            let shown = match &url {
                Some(u) => u.clone(),
                None => format!("镜像链：{}", nemesis_data::PRICE_MIRROR_URLS.join(" → ")),
            };
            println!("正在拉取价目表（{shown}）...");
            match nemesis_web::pricing_sync::fetch_and_replace(pricing, url.as_deref()).await {
                Ok(r) if r.updated => {
                    println!(
                        "价目表已更新：{} 个模型 → {}",
                        r.entry_count,
                        db_path.parent().unwrap_or(Path::new(".")).display()
                    );
                }
                Ok(r) => println!(
                    "表已是最新（304 NotModified，{} 条，etag={}）",
                    r.entry_count,
                    r.etag.as_deref().unwrap_or("-")
                ),
                Err(e) => anyhow::bail!("{e}"),
            }
        }
        PricesAction::Import { file } => {
            let raw = std::fs::read_to_string(&file)
                .map_err(|e| anyhow::anyhow!("读取 {} 失败：{e}", file))?;
            let entries = nemesis_data::parse_litellm_json(&raw)
                .map_err(|e| anyhow::anyhow!("解析失败：{e}"))?;
            let n = entries.len();
            pricing
                .replace_downloaded(
                    entries,
                    nemesis_data::PricingMeta {
                        etag: None,
                        fetched_at: Some(chrono::Local::now().timestamp()),
                        source_url: Some(format!("manual-import:{}", file)),
                        entry_count: n,
                    },
                )
                .map_err(|e| anyhow::anyhow!("落盘失败：{e}"))?;
            println!("导入完成：{n} 个模型 → 下载层（内置层继续兜底）");
        }
        PricesAction::Add {
            model,
            input,
            output,
            cache_read,
            cache_creation,
            display,
            price_tiers,
        } => {
            let name = model.trim().to_string();
            if name.is_empty() {
                anyhow::bail!("模型名不能为空");
            }
            // BUG-20261030-219：基础价四字段有限性校验——clap f64 解析接受
            // "NaN"/"inf" 字面量，无校验时坏值直接落自定义层污染计价输出
            // （与下方 parse_price_tiers 的 is_finite 校验同函数族，此处对齐）。
            if !(input.is_finite()
                && output.is_finite()
                && cache_read.is_finite()
                && cache_creation.is_finite())
            {
                anyhow::bail!("价格须为有限数值（拒绝 NaN/±inf）");
            }
            let tiers = parse_price_tiers(&price_tiers)?;
            let tier_note = if tiers.is_some() {
                "（含阶梯价）"
            } else {
                ""
            };
            pricing
                .upsert_custom(nemesis_data::ModelPricing {
                    model_id: name.clone(),
                    display_name: display.unwrap_or_else(|| name.clone()),
                    input_cost_per_million: input,
                    output_cost_per_million: output,
                    cache_read_cost_per_million: cache_read,
                    cache_creation_cost_per_million: cache_creation,
                    max_input_tokens: None,
                    max_output_tokens: None,
                    aliases: Vec::new(),
                    price_tiers: tiers,
                })
                .map_err(|e| anyhow::anyhow!("写入自定义条目失败：{e}"))?;
            println!(
                "自定义条目已保存：{name}（in={input} out={output} $/Mtok，最高查表优先级）{tier_note}"
            );
        }
        PricesAction::Remove { model } => {
            let removed = pricing
                .remove_custom(&model)
                .map_err(|e| anyhow::anyhow!("删除失败：{e}"))?;
            if removed {
                println!("已删除自定义条目：{model}");
            } else {
                anyhow::bail!("自定义条目不存在：{model}（`model prices list` 查看）");
            }
        }
    }
    Ok(())
}

/// Find a model entry in `model_list` by alias / full id / `vendor/<name>` suffix
/// and apply a mutation. Returns true if the entry was found and updated.
/// Test-visible delegate for `update_model_entry` (same crate, tests module).
#[cfg(test)]
pub(crate) fn update_model_entry_for_test<F>(cfg: &mut serde_json::Value, name: &str, f: F) -> bool
where
    F: FnOnce(&mut serde_json::Value),
{
    update_model_entry(cfg, name, f)
}

/// REL-002：model CLI raw RMW 写 config.json 的单一原子入口。
/// 序列化失败必须报错退出，不得 unwrap_or_default() 静默写空文件
///（2026-09-03 二次回归 model-4，两处写入点同修）；落盘走
/// `write_file_atomic`（唯一临时名 + sync_all + 失败清理）。
fn write_config_atomic(cfg_path: &Path, cfg: &serde_json::Value) -> anyhow::Result<()> {
    let body = serde_json::to_string_pretty(cfg)
        .map_err(|e| anyhow::anyhow!("序列化 config 失败: {e}"))?;
    nemesis_utils::write_file_atomic(&cfg_path.to_string_lossy(), body.as_bytes(), 0o600)
        .map_err(anyhow::Error::msg)
}

fn update_model_entry<F>(cfg: &mut serde_json::Value, name: &str, f: F) -> bool
where
    F: FnOnce(&mut serde_json::Value),
{
    let arr = match cfg.get_mut("model_list").and_then(|v| v.as_array_mut()) {
        Some(a) => a,
        None => return false,
    };
    for m in arr.iter_mut() {
        let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let alias = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        if full == name || alias == name || full.ends_with(&format!("/{}", name)) {
            f(m);
            return true;
        }
    }
    false
}

/// Build a provider for the target model and run the capability probe. Writes
/// the detected tier to config. Must run inside a tokio multi-thread runtime
/// (caller wraps with `block_in_place` + `Handle::block_on`).
async fn run_probe(
    home: &std::path::Path,
    name: &str,
) -> anyhow::Result<(nemesis_agent::probe::ProbeReport, bool)> {
    use std::collections::HashMap;
    use std::sync::Arc;

    let cfg_path = common::config_path(home);
    let cfg = nemesis_config::load_config(&cfg_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;
    let llm_ref = if name.is_empty() {
        nemesis_config::get_effective_llm(Some(&cfg))
    } else {
        name.to_string()
    };
    let resolution = nemesis_config::resolve_model_config(&cfg, &llm_ref)
        .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", llm_ref, e))?;
    let model_name = resolution.model_name.clone();
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        proxy: resolution.proxy.clone(),
        stream_mode: resolution.stream_mode.clone(),
        cache_breakpoints: resolution.cache_breakpoints,
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: home.join("workspace").to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode,
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        account_id: String::new(),
        headers: HashMap::new(),
    };
    let provider = nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> = provider;
    let adapter = nemesis_web::ProviderAdapter::new(provider_arc, model_name.clone());

    let report = nemesis_agent::probe::run(&adapter, &model_name)
        .await
        .map_err(|e| anyhow::anyhow!("Probe failed: {}", e))?;

    // Persist the detected tier.
    let data = std::fs::read_to_string(&cfg_path)?;
    let mut cfg_val: serde_json::Value = serde_json::from_str(&data)?;
    // BUG-20261030-218：写回查找目标 = 实际被探测的 llm_ref，不是原始 name。
    // `probe ""`（探默认模型）时 name 为空串，update_model_entry 的
    // full/alias/suffix 三匹配对空串永不命中 → 8 次 LLM 调用的探针结果
    // 静默丢失且报告谎称已写入。用 llm_ref（name 非空时与 name 相同，
    // 行为不变；name 为空时为生效默认模型别名，命中真实条目）。
    let wrote = update_model_entry(&mut cfg_val, &llm_ref, |e| {
        e["model_tier"] = serde_json::Value::String(report.tier.to_string());
        // T10（多模态 D9）：视觉探针结果——`Some(_)` 才写（None = 传输类
        // 未定，不给模型钉"不支持"；auto 解析序继续走名字关键词/默认放行）。
        if let Some(vp) = report.vision_probe {
            e["vision_probe"] = serde_json::Value::Bool(vp);
        }
    });
    if wrote {
        // REL-002：统一原子写入。
        write_config_atomic(&cfg_path, &cfg_val)?;
    }
    Ok((report, wrote))
}

fn format_probe_report(name: &str, r: &nemesis_agent::probe::ProbeReport, wrote: bool) -> String {
    let mut s = format!("能力探针报告: {}\n", name);
    s.push_str(&format!(
        "  format={:.2}  selection={:.2}  schema={:.2}\n",
        r.format_score, r.selection_score, r.schema_score
    ));
    s.push_str("  每个工具得分:\n");
    for (tool, sc) in &r.per_task {
        s.push_str(&format!(
            "    {:<14} format={:.0} selection={:.0} schema={:.0}\n",
            tool, sc.format, sc.selection, sc.schema
        ));
    }
    // BUG-20261030-218：写回结果如实呈现——写失败（config 中找不到条目）
    // 不得谎称"已写入 config.json"。
    if wrote {
        s.push_str(&format!("  → tier={} (已写入 config.json)", r.tier));
    } else {
        s.push_str(&format!(
            "  → tier={} (⚠ 未写入：config.json 中未找到条目「{name}」，探针结果未持久化)",
            r.tier
        ));
    }
    // T10（多模态 D9）：第 8 题视觉探针结果。
    match r.vision_probe {
        Some(true) => s.push_str(
            "\n  vision=支持（第 8 题带图请求成功，已写 vision_probe=true）",
        ),
        Some(false) => s.push_str(
            "\n  vision=不支持（模型拒绝带图请求，已写 vision_probe=false）",
        ),
        None => s.push_str(
            "\n  vision=未定（传输类失败，未写 vision_probe；可在 config.json 用 \"vision\": \"yes\"/\"no\" 手动钉）",
        ),
    }
    s
}

/// U16 (sixth batch): models.dev catalog fetch / cache / lookup.
pub mod catalog;

#[cfg(test)]
mod tests;
