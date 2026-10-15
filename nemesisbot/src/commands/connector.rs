//! connector 注册表 CLI（行动代理 P0，2026-10-14；签名信任族 D6.1，2026-10-15）。
//!
//! `nemesisbot connector <action>`：list / receipts / ledger / demo +
//! 签名信任族 sign / keygen / trust / trust-list / trust-revoke。与 WSAPI
//! `connectors.*` / `paperbroker.ledger` 同源——直读 nemesis-connectors
//! 存储（`<workspace>/connectors/` 注册表 + `data/receipts/` 凭条流水 +
//! `data/paper_broker.json` 账本，单点不双写）。执行入口是 agent 的
//! `connector_invoke` 工具（走完整安全管线），CLI 只做只读查询——例外是
//! `demo`：30 秒四承诺演示，全部产物落临时目录，零污染真实 workspace；
//! 以及签名信任族（清单签名/密钥管理，写 detached sig 与共享信任库）。

use anyhow::Result;
use clap::Subcommand;

use crate::common;
use nemesis_connectors::manifest::OpBinding;

mod sources;
#[derive(Debug, Subcommand)]
pub enum ConnectorAction {
    /// List registered connectors (corrupt dirs honestly listed with cause)
    List,
    /// Show recent receipts (newest first)
    Receipts {
        /// Max rows to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Filter by connector id
        #[arg(long)]
        connector: Option<String>,
    },
    /// Show the paper-broker ledger (cash / positions / orders)
    Ledger,
    /// 30-second zero-account zero-network demo of the four trust promises
    Demo,
    /// Sign a connector dir's connector.toml (detached Ed25519 signature)
    Sign {
        /// Connector directory containing connector.toml
        #[arg(value_name = "DIR")]
        dir: std::path::PathBuf,
        /// Ed25519 private key (hex64)
        #[arg(long)]
        key: String,
    },
    /// Generate an Ed25519 keypair for connector signing
    Keygen,
    /// Trust a signer public key (trust store shared with skills)
    Trust {
        /// Signer public key (hex64)
        #[arg(long)]
        key: String,
        /// Signer display name
        #[arg(long)]
        name: String,
        /// Trust level: verified | community
        #[arg(long, default_value = "verified")]
        level: String,
    },
    /// List trusted signer keys
    TrustList,
    /// Revoke a trusted signer key (level -> revoked, entry kept for audit)
    TrustRevoke {
        /// Signer public key (hex64); --key / --name 给其一
        #[arg(long)]
        key: Option<String>,
        /// Signer display name
        #[arg(long)]
        name: Option<String>,
    },
    /// Add a remote connector source (git clone into local cache)
    AddSource {
        /// Git repository URL
        url: String,
        /// Source name (default: derived from repo name)
        #[arg(long)]
        name: Option<String>,
        /// Branch to track (default main)
        #[arg(long)]
        branch: Option<String>,
    },
    /// Remove a remote source registration (local cache kept)
    RemoveSource {
        /// Source name
        name: String,
    },
    /// List registered remote connector sources
    Sources,
    /// Search connectors across cloned remote sources
    Search {
        /// Query tokens (empty = list all)
        query: String,
        /// Limit to one source
        #[arg(long)]
        source: Option<String>,
    },
    /// Install a connector from a remote source into the workspace registry
    Install {
        /// Spec as <source>/<id>
        spec: String,
        /// Allow installing unsigned connectors (explicit trust decision)
        #[arg(long)]
        allow_unsigned: bool,
    },
}

pub fn run(action: ConnectorAction, local: bool) -> Result<()> {
    // demo 不碰用户 workspace（临时目录自包含），先于 workspace 解析短路。
    if let ConnectorAction::Demo = action {
        return run_demo();
    }
    let home = common::resolve_home(local);
    let workspace = common::workspace_path(&home);
    match action {
        ConnectorAction::List => {
            let enabled = nemesis_config::load_config(&common::config_path(&home))
                .ok()
                .and_then(|cfg| cfg.connectors)
                .map(|c| c.enabled)
                .unwrap_or(true);
            let loaded = nemesis_connectors::registry::scan_connectors(&workspace);
            println!("connectors: {} (enabled: {})", loaded.len(), enabled);
            for l in &loaded {
                let dir_name = l
                    .dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match &l.connector {
                    Some(c) => {
                        println!(
                            "  {} v{} [{}] {} (dir: {})",
                            c.id,
                            c.version,
                            format!("{:?}", c.kind).to_lowercase(),
                            c.name,
                            dir_name
                        );
                        for op in &c.ops {
                            let binding = match &op.binding {
                                OpBinding::Http { method, path, .. } => format!(
                                    "http {} {}",
                                    format!("{method:?}").to_uppercase(),
                                    path
                                ),
                                OpBinding::Tool { tool } => format!("tool {tool}"),
                                OpBinding::Action { action } => format!("action {action}"),
                            };
                            println!(
                                "    - {:<14} risk={:<8} class={:<8} {}",
                                op.id,
                                op.risk.as_str(),
                                op.class.as_str(),
                                binding
                            );
                        }
                    }
                    None => {
                        println!(
                            "  {dir_name} [corrupt] {}",
                            l.error.clone().unwrap_or_default()
                        );
                    }
                }
            }
        }
        ConnectorAction::Receipts { limit, connector } => {
            let rows =
                nemesis_connectors::receipt::list_receipts(&workspace, limit, connector.as_deref())
                    .map_err(anyhow::Error::msg)?;
            println!("receipts ({}):", rows.len());
            for r in &rows {
                let receipt = r
                    .receipt_value
                    .as_deref()
                    .map(|v| format!(" {}={}", r.receipt_key.as_deref().unwrap_or_default(), v))
                    .unwrap_or_default();
                println!(
                    "  {} {} {} {} {}{}",
                    r.receipt_id, r.ts, r.connector, r.op, r.outcome, receipt
                );
                println!("      {}", r.summary);
            }
        }
        ConnectorAction::Ledger => {
            let l =
                nemesis_connectors::ledger::load_ledger(&workspace).map_err(anyhow::Error::msg)?;
            println!("cash: {:.2}", l.cash);
            println!("positions ({}):", l.positions.len());
            for p in l.positions.values() {
                println!("  {} qty={} avg_cost={:.2}", p.symbol, p.qty, p.avg_cost);
            }
            println!("orders ({}):", l.orders.len());
            for o in &l.orders {
                println!(
                    "  {} {} {} {} qty={} price={:.2} {:?}",
                    o.order_id,
                    o.ts,
                    format!("{:?}", o.side).to_lowercase(),
                    o.symbol,
                    o.qty,
                    o.price,
                    o.status
                );
            }
        }
        // demo 已在函数头短路（不碰用户 workspace）；此臂只为穷尽性。
        ConnectorAction::Demo => unreachable!("handled above"),
        ConnectorAction::Sign { dir, key } => {
            validate_hex64(&key, "私钥")?;
            let public_key = nemesis_connectors::signing::sign_connector_dir(&dir, &key)
                .map_err(anyhow::Error::msg)?;
            let sig_path = dir.join(nemesis_connectors::signing::SIG_FILE);
            println!("已签名：{}", dir.join("connector.toml").display());
            println!("  签名文件 : {}", sig_path.display());
            println!("  公钥(hex64) : {public_key}");
            println!(
                "  下一步      : 消费侧先 `connector trust --key {public_key} --name <签名者名>` \
                 （与 skills 共用信任库），该 connector 的签名才计为 verified。"
            );
        }
        ConnectorAction::Keygen => {
            let pair =
                nemesis_connectors::signing::generate_key_pair().map_err(anyhow::Error::msg)?;
            println!(
                "私钥(hex64，妥善保管，泄露即失去签名身份): {}",
                pair.private_key
            );
            println!(
                "公钥(hex64，分发/入信任库)                : {}",
                pair.public_key
            );
        }
        ConnectorAction::Trust { key, name, level } => {
            validate_hex64(&key, "公钥")?;
            let lvl = match level.as_str() {
                "verified" => nemesis_connectors::signing::TrustLevel::Verified,
                "community" => nemesis_connectors::signing::TrustLevel::Community,
                other => anyhow::bail!("未知信任级别: {other}（可选 verified|community）"),
            };
            let store = nemesis_connectors::signing::workspace_trust_store(&workspace);
            store.add_key(&key, &name, lvl);
            // 盘上真相回读（信任库持久化失败不得静默报成功——mirror plugin trust）。
            let persisted = nemesis_connectors::signing::workspace_trust_store(&workspace)
                .is_trusted(&key)
                .1;
            if persisted {
                println!("已写入信任库：{name}（{level}）");
                println!("  信任库文件 : {}", skill_trust_path(&workspace).display());
                println!("  作用域     : connectors 与 skills 共用此信任库（一把公钥双侧生效）。");
            } else {
                anyhow::bail!(
                    "信任库持久化失败：{name} 未能写入 {}（检查文件权限/磁盘）",
                    skill_trust_path(&workspace).display()
                );
            }
        }
        ConnectorAction::TrustList => {
            let mut keys =
                nemesis_connectors::signing::workspace_trust_store(&workspace).list_keys();
            if keys.is_empty() {
                println!("信任库为空（connector trust --key <hex64> --name <名> 添加）");
                println!("  信任库文件 : {}", skill_trust_path(&workspace).display());
                return Ok(());
            }
            // HashMap 无序输出显式排序（跨平台稳定展示）。
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
            println!(
                "共 {} 条（{}；connectors 与 skills 共用）",
                keys.len(),
                skill_trust_path(&workspace).display()
            );
        }
        ConnectorAction::TrustRevoke { key, name } => {
            let store = nemesis_connectors::signing::workspace_trust_store(&workspace);
            let entry = resolve_trust_entry(&store, &key, &name)?;
            store
                .revoke_key_by_public_key(&entry.public_key)
                .map_err(|e| anyhow::anyhow!("吊销失败: {e}"))?;
            // 盘上真相回读：级别必须已是 revoked（持久化失败不得报成功）。
            match nemesis_connectors::signing::workspace_trust_store(&workspace)
                .get_key(&entry.public_key)
                .map(|k| k.level)
            {
                Some(nemesis_connectors::signing::TrustLevel::Revoked) => {
                    println!("已吊销：{}（指纹 {}）", entry.name, entry.fingerprint);
                    println!(
                        "  效果        : 该公钥签名的 connector 清单验签即拒（篡改防线收回签名权）。"
                    );
                    println!(
                        "  恢复        : 重新 `connector trust --key <hex64> --name <名>` 写入即可（覆盖级别）。"
                    );
                }
                _ => anyhow::bail!(
                    "信任库持久化失败：{} 的吊销未落盘 {}（检查文件权限/磁盘）",
                    entry.name,
                    skill_trust_path(&workspace).display()
                ),
            }
        }
        ConnectorAction::AddSource { url, name, branch } => {
            let s = sources::add_source(&workspace, &url, name.as_deref(), branch.as_deref())
                .map_err(anyhow::Error::msg)?;
            println!("源 `{}` 已注册（{} @ {}）", s.name, s.url, s.branch);
            println!(
                "  缓存目录 : {}",
                sources::cache_dir(&workspace, &s.name).display()
            );
            println!(
                "  下一步   : `connector search <关键词>` 浏览，`connector install {}/<id>` 装配。",
                s.name
            );
        }
        ConnectorAction::RemoveSource { name } => {
            if sources::remove_source(&workspace, &name).map_err(anyhow::Error::msg)? {
                println!(
                    "源 `{name}` 已移除（本地缓存保留：{}——落位过的 connector 不受影响）",
                    sources::cache_dir(&workspace, &name).display()
                );
            } else {
                anyhow::bail!("源 `{name}` 不在注册表（connector sources 查看）");
            }
        }
        ConnectorAction::Sources => {
            let sources = sources::list_sources(&workspace);
            if sources.is_empty() {
                println!(
                    "远程源为空（connector add-source <git-url> 添加）\n  源注册表 : {}",
                    sources::sources_path(&workspace).display()
                );
                return Ok(());
            }
            println!("sources ({}):", sources.len());
            for s in &sources {
                println!("  {:<20} {} @ {}", s.name, s.url, s.branch);
            }
        }
        ConnectorAction::Search { query, source } => {
            let hits = sources::search_sources(&workspace, &query, source.as_deref())
                .map_err(anyhow::Error::msg)?;
            let corrupt = hits.iter().filter(|h| h.error.is_some()).count();
            println!("search \"{}\" ({}):", query, hits.len());
            for h in &hits {
                match &h.connector {
                    Some(c) => println!(
                        "  {}/{} v{} [{}] {}（{} ops）",
                        h.source,
                        c.id,
                        c.version,
                        h.trust.as_str(),
                        c.name,
                        c.ops.len()
                    ),
                    None => println!(
                        "  {}/{} [corrupt] {}",
                        h.source,
                        h.rel,
                        h.error.clone().unwrap_or_default()
                    ),
                }
            }
            if hits.is_empty() {
                println!("  （connector add-source 先加源）");
            } else if corrupt > 0 {
                println!("  ⚠ {corrupt} 条 corrupt（验签失败/清单损坏——install 会拒）");
            }
        }
        ConnectorAction::Install {
            spec,
            allow_unsigned,
        } => {
            let (source_name, connector_id) = spec
                .split_once('/')
                .ok_or_else(|| anyhow::anyhow!("spec 形态应为 <source>/<id>：{spec}"))?;
            let report =
                sources::install_connector(&workspace, source_name, connector_id, allow_unsigned)
                    .map_err(anyhow::Error::msg)?;
            println!(
                "已装配 `{}` → {}（信任: {}）",
                report.id,
                report.target_dir.display(),
                report.trust.as_str()
            );
            println!("  落位文件 : {}", report.copied.join(", "));
            for w in &report.warnings {
                println!("  ⚠ {w}");
            }
            println!(
                "  下一步 : `connector list` 查看（默认停用的 connector 在清单里 enabled = true 打开）。"
            );
        }
    }
    Ok(())
}

/// hex64 校验（签名信任族 CLI 入参纪律）。
fn validate_hex64(value: &str, what: &str) -> Result<()> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("{what}应为 hex64（64 个十六进制字符）");
    }
    Ok(())
}

/// connector 签名信任库路径（与 skills 共用的 skill_trust.json）。
fn skill_trust_path(workspace: &std::path::Path) -> std::path::PathBuf {
    workspace.join("config").join("skill_trust.json")
}

/// 信任吊销面：--key/--name 解析为库内条目（mirror plugin 同名助手）。
fn resolve_trust_entry(
    store: &nemesis_connectors::signing::TrustStore,
    key: &Option<String>,
    name: &Option<String>,
) -> Result<nemesis_connectors::signing::TrustedKey> {
    let not_found =
        |what: &str| anyhow::anyhow!("{what} 不在信任库（connector trust-list 查看现库）");
    match (key, name) {
        (Some(k), None) => {
            validate_hex64(k, "公钥")?;
            store.get_key(k).ok_or_else(|| not_found("公钥"))
        }
        (None, Some(n)) => store.get_key_by_name(n).ok_or_else(|| not_found("签名者")),
        (Some(k), Some(n)) => {
            validate_hex64(k, "公钥")?;
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

// ============ 30 秒四承诺演示（收官批 A3，2026-10-15） ============

/// 纸面券商模板原文（include_str! 单一真相源——幕 2/3 用的就是 workspace
/// 里那份模板；模板坏了 demo 同步坏，本身就是清单契约的活演示）。
const PAPER_BROKER_TEMPLATE: &str =
    include_str!("../../workspace/connectors/paper-broker/connector.toml");

/// 幕 1 演示凭据（字节级泄露扫描的针；随临时目录销毁，与任何真实凭据无关）。
const DEMO_SECRET: &str = "demo-secret-7f3a9c1e5b2d4a6f";

/// 四幕机器可读证据（`run_demo` 打印人话；tests 逐字段断言）。
pub struct DemoEvidence {
    /// 幕 1：凭据注入后调用 127.0.0.1:9 的诚实报错（connect refused 原文）。
    pub strict_error: String,
    /// 幕 1：全 workspace 字节级泄露扫描的文件数。
    pub leak_files_scanned: usize,
    /// 幕 1：泄露扫描命中数（承诺 = 0——vault 密文 + 凭条 digest）。
    pub leak_hits: usize,
    /// 幕 2：executed 凭条上的审批引用凭条（承诺 = Some("appr-demo-42")）。
    pub approval_ref: Option<String>,
    /// 幕 2：下单回执业务主键（order_id，承诺 = Some(NB-ORD-1)）。
    pub order_id: Option<String>,
    /// 幕 3：撤单的诚实报错（v1 即时成交无挂单——边界即结果）。
    pub cancel_error: String,
    /// 幕 4：账本订单数（承诺 = 1）。
    pub ledger_orders: usize,
    /// 幕 4：账本现金余额（承诺 = 1_000_000 - 1000 = 999000）。
    pub ledger_cash: f64,
    /// 幕 1 凭据调用是否落了 error 回执（承诺 = true——错误也是账）。
    pub strict_receipt_outcome: Option<String>,
}

fn demo_ctx() -> nemesis_agent::context::RequestContext {
    nemesis_agent::context::RequestContext::new("web", "chat-demo", "user-demo", "session-demo")
}

fn write_demo_connector(ws: &std::path::Path, id: &str, toml_text: &str) -> Result<()> {
    let dir = ws.join("connectors").join(id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("connector.toml"), toml_text)?;
    Ok(())
}

fn demo_walk_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            demo_walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// 幕 1 清单：rest 型 + 必填凭据槽位（header 注入 + Bearer 前缀）。base 指向
/// 127.0.0.1:9（discard 端口，connect refused 必然诚实失败——零外网出网）。
/// 消费者 = 幕 1（security 闸）——同闸。
#[cfg(feature = "security")]
const DEMO_STRICT_MANIFEST: &str = r#"
[connector]
id = "demo-strict"
name = "凭据铁律演示"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:9"

[connector.credentials]
token = { required = true, into = "header:Authorization", prefix = "Bearer " }

[[op]]
id = "ping"
intent = ["连通性探测"]
risk = "low"
class = "network"
http = { method = "GET", path = "/ping" }
"#;

/// 四幕演示核：在给定（临时）workspace 里走完凭据铁律 → 审批引用凭条 →
/// 反向撤销 → 行动账本。独立于打印，tests 直接断言证据。
async fn demo_acts(ws: &std::path::Path) -> Result<DemoEvidence> {
    use crate::connector_tools::ConnectorInvokeTool;
    use nemesis_agent::r#loop::Tool;

    let tool = ConnectorInvokeTool::new(
        ws.to_path_buf(),
        ws.join("config.json"),
        #[cfg(feature = "security")]
        None,
    );

    // ---- 幕 1 · 凭据铁律：vault 存密钥 → 调用 → 泄露扫描 ----
    #[cfg(feature = "security")]
    let strict_error = {
        write_demo_connector(ws, "demo-strict", DEMO_STRICT_MANIFEST)?;
        use nemesis_security::vault::VaultStore;
        let vault_path = nemesis_path::resolve_vault_path_in_workspace(ws);
        let mut store = VaultStore::create(&vault_path, VaultStore::default_mode(), None)
            .map_err(|e| anyhow::anyhow!("创建演示 vault 失败: {e}"))?;
        store
            .set(
                "connector/demo-strict/token",
                DEMO_SECRET,
                "connector",
                "演示凭据",
            )
            .map_err(|e| anyhow::anyhow!("写入演示凭据失败: {e}"))?;
        store
            .save()
            .map_err(|e| anyhow::anyhow!("保存演示 vault 失败: {e}"))?;
        drop(store);
        tool.execute(
            r#"{"connector":"demo-strict","op":"ping","args":{}}"#,
            &demo_ctx(),
        )
        .await
        .expect_err("幕1：127.0.0.1:9 connect refused 必然诚实报错")
    };
    #[cfg(not(feature = "security"))]
    let strict_error = "security feature 未编译——凭据铁律演示跳过（vault 依赖缺席）".to_string();

    // 全 workspace 字节级泄露扫描（含 vault.enc / 凭条 jsonl / 清单 / config）。
    let mut files = Vec::new();
    demo_walk_files(ws, &mut files);
    let leak_files_scanned = files.len();
    let needle = DEMO_SECRET.as_bytes();
    let leak_hits = files
        .iter()
        .filter(|f| {
            std::fs::read(f)
                .map(|b| needle.is_empty() || b.windows(needle.len()).any(|w| w == needle))
                .unwrap_or(false)
        })
        .count();

    // 幕 1 的错误也是账：应有 demo-strict 的 error 回执（security 关时无调用）。
    let strict_receipt_outcome =
        nemesis_connectors::receipt::list_receipts(ws, 50, Some("demo-strict"))
            .ok()
            .and_then(|rs| rs.first().map(|r| r.outcome.clone()));

    // ---- 幕 2 · 审批引用凭条：模板 connector + place_order ----
    write_demo_connector(ws, "paper-broker", PAPER_BROKER_TEMPLATE)?;
    std::fs::write(
        ws.join("config.json"),
        r#"{
  "connectors": {
    "enabled": true,
    "max_ops_per_hour": 100,
    "finance": { "max_order_qty": 100.0, "max_orders_per_day": 5 }
  }
}"#,
    )?;
    let mut ctx = demo_ctx();
    ctx.approval_ref = Some("appr-demo-42".to_string());
    let order_out = tool
        .execute(
            r#"{"connector":"paper-broker","op":"place_order","args":{"side":"buy","symbol":"600519","qty":1.0,"price":1000.0}}"#,
            &ctx,
        )
        .await
        .map_err(|e| anyhow::anyhow!("幕2 下单意外失败: {e}"))?;
    let _ = order_out; // result_text 人话面由打印幕承担；凭条才是证据

    let receipts = nemesis_connectors::receipt::list_receipts(ws, 50, Some("paper-broker"))
        .map_err(anyhow::Error::msg)?;
    let placed = receipts
        .iter()
        .find(|r| r.op == "place_order" && r.outcome == "executed")
        .ok_or_else(|| anyhow::anyhow!("幕2：未找到 place_order 的 executed 凭条"))?;
    let approval_ref = placed.approval_ref.clone();
    let order_id = placed.receipt_value.clone();

    // ---- 幕 3 · 做错能反悔：撤单（v1 即时成交无挂单 = 诚实报错） ----
    let oid = order_id.clone().unwrap_or_default();
    let cancel_error = tool
        .execute(
            &format!(
                r#"{{"connector":"paper-broker","op":"cancel_order","args":{{"order_id":"{oid}"}}}}"#
            ),
            &ctx,
        )
        .await
        .expect_err("幕3：v1 即时成交无挂单，撤单应诚实报错");

    // ---- 幕 4 · 出事有账：账本收尾 ----
    let ledger = nemesis_connectors::ledger::load_ledger(ws).map_err(anyhow::Error::msg)?;

    Ok(DemoEvidence {
        strict_error,
        leak_files_scanned,
        leak_hits,
        approval_ref,
        order_id,
        cancel_error,
        ledger_orders: ledger.orders.len(),
        ledger_cash: ledger.cash,
        strict_receipt_outcome,
    })
}

/// 30 秒四承诺演示入口：零账号 · 零真实资金 · 零外网 · 零污染真实 workspace。
pub fn run_demo() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let ws = tmp.path().to_path_buf();
    println!("════════════════════════════════════════════════════");
    println!(" NemesisBot 行动代理 · 30 秒演示（四承诺）");
    println!(" 零账号 · 零真实资金 · 零外网 · 不动你的真实 workspace");
    println!("════════════════════════════════════════════════════");

    // main 是 #[tokio::main]——从 runtime 内 block_on 会 panic（forge CLI 同款
    // 坑）；block_in_place + Handle 修复。若在 runtime 外调用（tests 直调
    // demo_acts 不走这里）回落自建 current_thread runtime。
    let ev = match tokio::runtime::Handle::try_current() {
        Ok(h) => tokio::task::block_in_place(|| h.block_on(demo_acts(&ws)))?,
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(demo_acts(&ws))?
        }
    };

    println!();
    println!("── 幕 1 · 凭据不泄露 ──────────────────────────────");
    println!("  demo connector 声明必填凭据槽位（header 注入 + Bearer 前缀），");
    println!("  vault 存入演示密钥后调用 http://127.0.0.1:9（discard 端口）：");
    let short: String = ev.strict_error.chars().take(96).collect();
    println!(
        "  ✓ 调用诚实失败：{short}{}",
        if ev.strict_error.chars().count() > 96 {
            "…"
        } else {
            ""
        }
    );
    println!(
        "  ✓ 全 workspace 字节级泄露扫描：{} 个文件，密钥命中 {} 处",
        ev.leak_files_scanned, ev.leak_hits
    );
    match ev.strict_receipt_outcome.as_deref() {
        Some("error") => println!("  ✓ 错误也是账：error 回执已落（凭条编号可交叉印证）"),
        _ => println!("  · error 回执缺席（security feature 未编译时幕 1 跳过）"),
    }
    println!();
    println!("── 幕 2 · 动手前先问 ──────────────────────────────");
    println!("  纸面券商 connector（workspace 模板原文）声明 place_order：");
    println!("  critical/write + confirm=required + finance 限额。演示直调执行核");
    println!("  （审批闸在 agent loop 的 dispatch 层——真实对话里这一步必弹审批卡）：");
    println!(
        "  ✓ 下单执行，凭条审批引用 approval_ref = {:?}",
        ev.approval_ref.as_deref().unwrap_or("<缺>")
    );
    println!(
        "  ✓ 业务主键回填：order_id = {}",
        ev.order_id.as_deref().unwrap_or("<缺>")
    );
    println!();
    println!("── 幕 3 · 做错能反悔 ──────────────────────────────");
    let short3: String = ev.cancel_error.chars().take(96).collect();
    println!("  ✓ 撤单诚实报错：{short3}");
    println!("    （v1 即时成交无挂单——不装成功，边界即结果）");
    println!();
    println!("── 幕 4 · 出事有账 ────────────────────────────────");
    println!(
        "  ✓ 行动账本：现金 {:.2} · 订单 {} 笔",
        ev.ledger_cash, ev.ledger_orders
    );
    println!("    （查询: nemesisbot connector list | connector receipts | connector ledger）");
    println!("    （演示里那份账本在临时目录，与你 workspace 的账本互不相干）");
    println!();
    println!("  演示产物全部在临时目录，进程退出即清空——你的真实 workspace 零改动。");
    println!("════════════════════════════════════════════════════");

    if ev.leak_hits != 0 {
        return Err(anyhow::anyhow!(
            "凭据铁律失守：泄露扫描命中 {} 处（这是演示自身的事故，不是演示目标）",
            ev.leak_hits
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
