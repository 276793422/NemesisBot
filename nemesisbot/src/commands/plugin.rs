//! Plugin command — WASM 插件 CLI（W4）。
//!
//! CLI 进程是**磁盘 only** 操作者：install/upgrade 走完整九步装配漏斗
//! （CLI 无审批卡 UI → 必须 `--yes` 显式同意；无病毒扫描引擎 → 第④步
//! 诚实跳过并注记），产物落盘（plugins/<slug>/ + lockfile）；enable/
//! disable 直接改写实例配置文件 `config/plugins/<slug>.json`。运行中的
//! gateway 不感知这些变化——重启后经 load_all / apply_enabled_from_file
//! 生效；运行期安装/启停走 Dashboard（WSAPI `plugins.wasm.*`）。
//!
//! 签名（Ed25519，与 trust.rs 验签同源）：manifest 剥根级 `signature`
//! 表后的规范字节做签名；`plugin trust` 把签名者公钥写入 plugin 专用
//! TrustStore（`config/plugin_trust.json`，键 = hex 公钥）。

use anyhow::Result;
use clap::Subcommand;

use crate::common;

#[derive(Subcommand)]
pub enum PluginAction {
    /// 从源目录安装插件（九步装配漏斗）
    Install {
        /// 插件源目录（含 plugin.toml + wasm 载荷）
        source_dir: String,
        /// 放行无签名 manifest（只豁免「无签名」，不豁免「签名无效」）
        #[arg(long)]
        allow_unsigned: bool,
        /// 自动同意安装（CLI 无审批卡 UI；缺省拒绝并提示）
        #[arg(long)]
        yes: bool,
    },
    /// 升级已装插件（重跑安装漏斗；数据目录保留）
    Upgrade {
        /// 插件源目录（含 plugin.toml + wasm 载荷）
        source_dir: String,
        #[arg(long)]
        allow_unsigned: bool,
        #[arg(long)]
        yes: bool,
    },
    /// 卸载插件（注销 + 删载荷 + lockfile 摘除；数据目录保留）
    Uninstall { slug: String },
    /// 启用插件（改实例配置文件；运行中 gateway 重启后生效）
    Enable { slug: String },
    /// 禁用插件（改实例配置文件；运行中 gateway 重启后生效）
    Disable { slug: String },
    /// 列出已安装插件（读 lockfile）
    List,
    /// 查看插件详情（纯磁盘只读：lockfile + 盘上 manifest + 实例配置；
    /// 不构造引擎不装载组件——运行期状态如日志环走 Dashboard/WSAPI `wasm.logs`）
    Info { slug: String },
    /// 签名 plugin.toml（Ed25519；私钥为 32 字节 seed 的 hex64）
    Sign {
        /// manifest 路径（plugin.toml）
        manifest: String,
        /// Ed25519 私钥（hex64）
        #[arg(long)]
        key: String,
    },
    /// 生成新 Ed25519 密钥对（hex 输出；私钥自管，服务器不存）
    Keygen,
    /// 将签名者公钥加入插件信任库（config/plugin_trust.json）
    Trust {
        /// Ed25519 公钥（hex64）
        #[arg(long)]
        key: String,
        /// 签名者名称（展示/摘除用）
        #[arg(long)]
        name: String,
        /// 信任级别：verified | community（缺省 community）
        #[arg(long, default_value = "community")]
        level: String,
    },
    /// 列出插件信任库全部公钥（name/指纹/级别/加入时间）
    TrustList,
    /// 吊销信任库公钥（级别置 revoked，条目保留留痕；--key 或 --name）
    TrustRevoke {
        /// Ed25519 公钥（hex64）
        #[arg(long)]
        key: Option<String>,
        /// 签名者名称
        #[arg(long)]
        name: Option<String>,
    },
    /// 从信任库移除公钥（整条删除，之后可重新 trust；--key 或 --name）
    TrustRemove {
        /// Ed25519 公钥（hex64）
        #[arg(long)]
        key: Option<String>,
        /// 签名者名称
        #[arg(long)]
        name: Option<String>,
    },
}

fn err_msg(e: nemesis_plugins_wasm::PluginError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn trust_path(home: &std::path::Path) -> std::path::PathBuf {
    common::workspace_path(home)
        .join("config")
        .join("plugin_trust.json")
}

/// 信任吊销面（2026-09-30 插件体系复查 #10）：--key/--name 解析为库内条目。
/// 恰给其一；都给则校验同指一条（防错改同名异钥条目）。
fn resolve_trust_entry(
    store: &nemesis_security::signature::TrustStore,
    key: &Option<String>,
    name: &Option<String>,
) -> Result<nemesis_security::signature::TrustedKey> {
    let not_found = |what: &str| anyhow::anyhow!("{what} 不在信任库（plugin trust list 查看现库）");
    match (key, name) {
        (Some(k), None) => {
            if k.len() != 64 || !k.chars().all(|c| c.is_ascii_hexdigit()) {
                anyhow::bail!("公钥应为 hex64（64 个十六进制字符）");
            }
            store.get_key(k).ok_or_else(|| not_found("公钥"))
        }
        (None, Some(n)) => store.get_key_by_name(n).ok_or_else(|| not_found("签名者")),
        (Some(k), Some(n)) => {
            let entry = store.get_key(k).ok_or_else(|| not_found("公钥"))?;
            if entry.name != *n {
                anyhow::bail!(
                    "--key 与 --name 不指向同一条目：库内该公钥的签名者是「{}」",
                    entry.name
                );
            }
            Ok(entry)
        }
        (None, None) => anyhow::bail!("--key 与 --name 必须提供其一"),
    }
}

/// 变更后盘上真相回读（与 Trust 同模式）：重开信任库文件确认落盘。
fn persisted_trust_store(home: &std::path::Path) -> nemesis_security::signature::TrustStore {
    nemesis_security::signature::TrustStore::new(Some(trust_path(home)))
}

/// CLI 独立 PluginManager 构造（install/upgrade 与 Info 共用；不构造引擎）。
fn cli_manager(
    home: &std::path::Path,
) -> Result<std::sync::Arc<nemesis_plugins_wasm::PluginManager>> {
    let rc = crate::plugin_bridge::read_runtime_config(home);
    let manager = std::sync::Arc::new(
        nemesis_plugins_wasm::PluginManager::new(
            &common::workspace_path(home),
            rc.limits,
            std::sync::Arc::new(crate::plugin_bridge::VaultPluginSecrets::for_home(home)),
        )
        .map_err(err_msg)?,
    );
    Ok(manager)
}

/// CLI 独立装配（不走 gateway 的 build_plugin_stack：无 SecurityPlugin/
/// 审批 manager 可共享——扫描 None 诚实跳过，审批由 `--yes` 承担）。
fn cli_installer(
    home: &std::path::Path,
    yes: bool,
) -> Result<nemesis_plugins_wasm::install::PluginInstaller> {
    let manager = cli_manager(home)?;
    let approver: std::sync::Arc<dyn nemesis_plugins_wasm::install::InstallApprover> = if yes {
        std::sync::Arc::new(nemesis_plugins_wasm::install::AutoApprove)
    } else {
        anyhow::bail!(
            "CLI 安装需要显式 --yes（CLI 进程无审批卡 UI，无法交互确认）。\
             请确认插件来源可信后携带 --yes 重试，或改用 Dashboard 安装（走审批卡）。"
        )
    };
    Ok(nemesis_plugins_wasm::install::PluginInstaller::new(
        manager, approver, None,
    ))
}

/// enable/disable 的磁盘形态：经 PluginManager.set_enabled_file_only 单一
/// 真相源（valid_slug 校验 + lockfile 幽灵配置拒绝 + config_io 互斥 + 原子
/// 写，保留 entries）——CLI 不再裸写实例配置文件（2026-09-29 交付审查 3.2：
/// 与 WSAPI/gateway 共用同一写路径，半写损坏面收敛到一处）。
fn set_enabled_on_disk(home: &std::path::Path, slug: &str, on: bool) -> Result<()> {
    let manager = cli_manager(home)?;
    manager.set_enabled_file_only(slug, on).map_err(err_msg)?;
    Ok(())
}

fn print_install_result(reg: &nemesis_plugins_wasm::RegisteredPlugin) {
    println!("  版本       : {}", reg.manifest.version);
    println!("  kind       : {}", reg.manifest.kind.as_str());
    println!("  信任       : {}", reg.trust.as_str());
    if let Some(meta) = reg.tool_meta.as_ref() {
        println!("  工具       : {}", meta.name);
        if !meta.operation_type.is_empty() {
            println!("  操作声明   : {}", meta.operation_type);
        }
    }
    println!(
        "  注意       : CLI 安装仅落盘（磁盘 only）；运行中的 gateway 需重启后装载。\
         运行期安装请走 Dashboard。"
    );
}

/// 签名：剥根级 signature 表 → 规范字节签名 → 以 toml_edit 回填 signature
/// 表（同引擎插拔保证验签侧 strip 后字节与签名时规范形态一致）。
fn sign_manifest(raw: &str, key_hex: &str) -> Result<(String, String)> {
    let mut doc = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow::anyhow!("plugin.toml 解析失败: {e}"))?;
    doc.remove("signature");
    let canonical = doc.to_string();
    let signature = nemesis_security::signature::sign_content_hex(&canonical, key_hex)
        .map_err(|e| anyhow::anyhow!("签名失败: {e}"))?;
    let public_key = nemesis_security::signature::derive_public_key_hex(key_hex)
        .map_err(|e| anyhow::anyhow!("公钥推导失败: {e}"))?;
    let mut sig = toml_edit::Table::new();
    sig.insert("algorithm", toml_edit::value("ed25519"));
    sig.insert("public-key", toml_edit::value(public_key.clone()));
    sig.insert("signature", toml_edit::value(signature));
    sig.insert(
        "signed-at",
        toml_edit::value(chrono::Local::now().to_rfc3339()),
    );
    doc.insert("signature", toml_edit::Item::Table(sig));
    Ok((doc.to_string(), public_key))
}

/// Run the plugin command.
pub async fn run(local: bool, action: PluginAction) -> Result<()> {
    let home = common::resolve_home(local);
    match action {
        PluginAction::Install {
            source_dir,
            allow_unsigned,
            yes,
        } => {
            let installer = cli_installer(&home, yes)?;
            let reg = installer
                .install(std::path::Path::new(&source_dir), allow_unsigned)
                .await
                .map_err(err_msg)?;
            println!("安装完成：{}", reg.manifest.slug);
            print_install_result(&reg);
        }
        PluginAction::Upgrade {
            source_dir,
            allow_unsigned,
            yes,
        } => {
            let installer = cli_installer(&home, yes)?;
            let reg = installer
                .install(std::path::Path::new(&source_dir), allow_unsigned)
                .await
                .map_err(err_msg)?;
            println!("升级完成（漏斗重跑，数据目录保留）：{}", reg.manifest.slug);
            print_install_result(&reg);
        }
        PluginAction::Uninstall { slug } => {
            let installer = cli_installer(&home, true)?;
            let existed =
                nemesis_plugins_wasm::install::uninstall(installer.manager.as_ref(), &slug)
                    .await
                    .map_err(err_msg)?;
            if existed {
                println!("已卸载 {slug}（载荷删除，数据目录保留；运行中 gateway 需重启生效）");
            } else {
                println!("{slug} 不在盘上（无载荷/注册）");
            }
        }
        PluginAction::Enable { slug } => {
            set_enabled_on_disk(&home, &slug, true)?;
            println!(
                "已启用 {slug}（实例配置落盘；运行中 gateway 重启后生效，运行期启停请走 Dashboard）"
            );
        }
        PluginAction::Disable { slug } => {
            set_enabled_on_disk(&home, &slug, false)?;
            println!(
                "已禁用 {slug}（实例配置落盘；运行中 gateway 重启后生效，运行期启停请走 Dashboard）"
            );
        }
        PluginAction::List => {
            let path = common::workspace_path(&home)
                .join("plugins")
                .join("lockfile.json");
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => {
                    println!("无 lockfile（尚未安装任何插件）：{}", path.display());
                    return Ok(());
                }
            };
            let lf: nemesis_plugins_wasm::install::PluginLockfile = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("lockfile 解析失败: {e}"))?;
            if lf.plugins.is_empty() {
                println!("lockfile 为空（尚未安装任何插件）");
                return Ok(());
            }
            let list_head = format!(
                "{:<24} {:<12} {:<20} {:<16} INSTALLED_AT",
                "SLUG", "VERSION", "TRUST", "SIGNED_BY"
            );
            println!("{list_head}");
            for (slug, e) in &lf.plugins {
                let ts = chrono::DateTime::from_timestamp_millis(e.installed_at_ms as i64)
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| e.installed_at_ms.to_string());
                let signer = if e.signed_by.is_empty() {
                    "-".to_string()
                } else {
                    e.signed_by.clone()
                };
                println!(
                    "{:<24} {:<12} {:<20} {:<16} {}",
                    slug, e.version, e.trust, signer, ts
                );
            }
        }
        PluginAction::Info { slug } => {
            if !nemesis_plugins_wasm::manifest::valid_slug(&slug) {
                anyhow::bail!("非法 slug: {slug}");
            }
            let manager = cli_manager(&home)?;
            // lockfile 条目（安装状态真相源；无条目 = 未安装）
            let lock_path = manager.plugins_dir().join("lockfile.json");
            let lf: nemesis_plugins_wasm::install::PluginLockfile =
                std::fs::read_to_string(&lock_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
            let entry = lf
                .plugins
                .get(&slug)
                .ok_or_else(|| anyhow::anyhow!("插件未安装: {slug}（plugin list 查看已装列表）"))?;
            println!("slug        : {slug}");
            println!("version     : {}", entry.version);
            println!("trust       : {}", entry.trust);
            println!(
                "signed_by   : {}",
                if entry.signed_by.is_empty() {
                    "-".to_string()
                } else {
                    entry.signed_by.clone()
                }
            );
            let payload_dir = manager.plugins_dir().join(&slug);
            println!("payload_dir : {}", payload_dir.display());
            println!("data_dir    : {}", manager.plugin_data_dir(&slug).display());
            // 盘上 manifest（lockfile 在而载荷缺 = 异常残留，诚实注记）
            match nemesis_plugins_wasm::manifest::PluginManifest::load_from_dir(&payload_dir) {
                Ok((m, _)) => {
                    println!("kind        : {}", m.kind.as_str());
                    println!("name        : {}", m.name);
                    println!("min-tier    : {}", m.min_tier);
                    if !m.permissions.egress.is_empty() {
                        println!("egress      : {}", m.permissions.egress.join(", "));
                    }
                    if !m.permissions.x_secret.is_empty() {
                        println!("x-secret    : {}", m.permissions.x_secret.join(", "));
                    }
                    if !m.limits.is_empty() {
                        let kv: Vec<String> =
                            m.limits.iter().map(|(k, v)| format!("{k}={v}")).collect();
                        println!("limits      : {}", kv.join(", "));
                    }
                    if !m.config_schema.is_empty() {
                        println!("config-schema:");
                        for (k, v) in &m.config_schema {
                            println!("  {k}  # {v}");
                        }
                    }
                }
                Err(_) => {
                    println!("manifest    : 载荷缺失（lockfile 有条目但盘上 manifest 不可读）")
                }
            }
            let cfg = manager.get_config(&slug);
            println!("enabled     : {}", cfg.enabled);
            for (k, v) in &cfg.entries {
                println!("  config {k} = {v}");
            }
        }
        PluginAction::Sign { manifest, key } => {
            let raw = std::fs::read_to_string(&manifest)
                .map_err(|e| anyhow::anyhow!("读 {}: {e}", manifest))?;
            let (signed, public_key) = sign_manifest(&raw, &key)?;
            std::fs::write(&manifest, signed.as_bytes())?;
            println!("已签名：{}", manifest);
            println!("  公钥(hex64) : {public_key}");
            println!(
                "  下一步      : 分发公钥前先 `plugin trust --key {public_key} --name <签名者名>` \
                 写入信任库（签名才可获得 trusted/review-recommended 信任态）。"
            );
        }
        PluginAction::Keygen => {
            let pair = nemesis_security::signature::generate_key_pair()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "私钥(hex64，妥善保管，泄露即失去签名身份): {}",
                pair.private_key
            );
            println!(
                "公钥(hex64，分发/入信任库)                : {}",
                pair.public_key
            );
        }
        PluginAction::Trust { key, name, level } => {
            if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
                anyhow::bail!("公钥应为 hex64（64 个十六进制字符）");
            }
            let lvl = match level.as_str() {
                "verified" => nemesis_security::signature::TrustLevel::Verified,
                "community" => nemesis_security::signature::TrustLevel::Community,
                other => anyhow::bail!("未知信任级别: {other}（可选 verified|community）"),
            };
            let verifier =
                nemesis_security::signature::SignatureVerifier::with_persistence(trust_path(&home));
            verifier.trust_store_ref().add_key(&key, &name, lvl);
            // 盘上真相回读（信任库持久化失败不得静默报成功——内存态与磁盘
            // 分叉时重启即丢；2026-09-29 交付审查）。
            let persisted = nemesis_security::signature::TrustStore::new(Some(trust_path(&home)))
                .is_trusted(&key)
                .1;
            if persisted {
                println!("已写入信任库：{name}（{level}）");
                println!("  信任库文件 : {}", trust_path(&home).display());
            } else {
                anyhow::bail!(
                    "信任库持久化失败：{name} 未能写入 {}（检查文件权限/磁盘）",
                    trust_path(&home).display()
                );
            }
        }
        PluginAction::TrustList => {
            let mut keys = persisted_trust_store(&home).list_keys();
            if keys.is_empty() {
                println!("信任库为空（plugin trust --key <hex64> --name <名> 添加）");
                println!("  信任库文件 : {}", trust_path(&home).display());
                return Ok(());
            }
            // read_dir 纪律同款：HashMap 无序输出显式排序（跨平台稳定展示）。
            keys.sort_by(|a, b| a.name.cmp(&b.name).then(a.public_key.cmp(&b.public_key)));
            println!(
                "{:<24} {:<18} {:<12} ADDED_AT             PUBLIC_KEY",
                "NAME", "FINGERPRINT", "LEVEL"
            );
            for k in &keys {
                let fp: String = k.fingerprint.chars().take(16).collect();
                let pk: String = k.public_key.chars().take(16).collect();
                println!(
                    "{:<24} {:<18} {:<12} {:<20} {}…",
                    k.name,
                    format!("{fp}…"),
                    k.level,
                    k.added_at,
                    pk
                );
            }
            println!("共 {} 条（{}）", keys.len(), trust_path(&home).display());
        }
        PluginAction::TrustRevoke { key, name } => {
            let verifier =
                nemesis_security::signature::SignatureVerifier::with_persistence(trust_path(&home));
            let entry = resolve_trust_entry(verifier.trust_store_ref(), &key, &name)?;
            verifier
                .trust_store_ref()
                .revoke_key_by_public_key(&entry.public_key)
                .map_err(|e| anyhow::anyhow!("吊销失败: {e}"))?;
            // 盘上真相回读：级别必须已是 revoked（持久化失败不得报成功）。
            match persisted_trust_store(&home)
                .get_key(&entry.public_key)
                .map(|k| k.level)
            {
                Some(nemesis_security::signature::TrustLevel::Revoked) => {
                    println!("已吊销：{}（指纹 {}）", entry.name, entry.fingerprint);
                    println!(
                        "  效果        : 级别置 revoked、条目保留留痕；该公钥签名的插件装回即拒（blocked）。"
                    );
                    println!(
                        "  恢复        : 重新 `plugin trust --key <hex64> --name <名>` 写入即可（覆盖级别）。"
                    );
                }
                _ => anyhow::bail!(
                    "信任库持久化失败：{} 的吊销未落盘 {}（检查文件权限/磁盘）",
                    entry.name,
                    trust_path(&home).display()
                ),
            }
        }
        PluginAction::TrustRemove { key, name } => {
            let verifier =
                nemesis_security::signature::SignatureVerifier::with_persistence(trust_path(&home));
            let entry = resolve_trust_entry(verifier.trust_store_ref(), &key, &name)?;
            let removed = verifier
                .trust_store_ref()
                .remove_key_by_public_key(&entry.public_key);
            if removed
                && persisted_trust_store(&home)
                    .get_key(&entry.public_key)
                    .is_none()
            {
                println!(
                    "已移除：{}（指纹 {}）——条目整条删除",
                    entry.name, entry.fingerprint
                );
                println!(
                    "  注意        : 该公钥签名的已装插件不受影响；重新信任用 `plugin trust` 写入。"
                );
            } else {
                anyhow::bail!(
                    "信任库持久化失败：{} 的移除未落盘 {}（检查文件权限/磁盘）",
                    entry.name,
                    trust_path(&home).display()
                );
            }
        }
    }
    Ok(())
}
