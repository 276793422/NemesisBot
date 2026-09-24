//! exe-sign-tool（**v4**）：可执行文件签名/验证 CLI。
//!
//! v4 架构（Authenticode 对齐）：ECDSA P-256 + X.509 三级证书链（公钥随签名走），
//! **lib 直接验签**（不加载 DLL）。与 verify-loader 区别：本工具用 `nemesis-verify`
//! lib 直接 `verify_bytes`；verify-loader 加载 DLL（C ABI）。
//!
//! 用法：
//! ```sh
//! exe-sign-tool keygen --out keys.json
//! exe-sign-tool split-keys --in keys.json --root-out root.offline.json \
//!     --issuing-out issuing.ci.json [--root-cert-out root_cert.der]
//! exe-sign-tool mint-leaf --issuing issuing.ci.json --days 365 \
//!     --cn "NemesisBot CI <sha>" --out ci-keys.json
//! exe-sign-tool sign --keys keys.json|ci-keys.json --target myapp.exe [--out myapp.signed.exe]
//! exe-sign-tool verify --root-cert certs/root_cert.der --target myapp.exe   # 零私钥
//! exe-sign-tool verify --keys keys.json --target myapp.exe                 # 旧形态
//! ```
//!
//! 密钥分层：keygen → split-keys 把三级私钥拆成（根离线 / 中间 CA 进 Secrets），
//! CI 每次构建 mint-leaf 现铸短期叶签名。分包形态详见 `nemesis_verify::bundle`。

use anyhow::{Result, bail};
use clap::{ArgGroup, Parser, Subcommand};
use nemesis_verify::{bundle, hex_util::hex_encode, keygen::KeyHierarchy, verify};

#[derive(Parser)]
#[command(
    name = "exe-sign-tool",
    version,
    about = "可执行文件签名/验证（v4：ECDSA P-256 + 公钥随签名走 + X.509 证书链）"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 生成密钥体系（root/发行锚/leaf 私钥 + X.509 三级链）到 JSON
    Keygen {
        #[arg(long, default_value = "keys.json")]
        out: String,
    },
    /// 拆分全量密钥包：根（离线保管）/ 中间 CA（进 CI Secrets 的原材料）
    SplitKeys {
        /// 全量 keys.json（拆分前做三级一致性校验）
        #[arg(long = "in")]
        input: String,
        /// 根材料输出（root_sk + root_cert；离线冷存形态）
        #[arg(long)]
        root_out: String,
        /// 中间 CA 材料输出（issuing_sk + issuing_cert + root_cert）
        #[arg(long)]
        issuing_out: String,
        /// 根证书公开部分 DER 输出（进仓库 certs/ 的原材料；可选）
        #[arg(long)]
        root_cert_out: Option<String>,
    },
    /// 从中间 CA 材料现铸叶代码签名证书 → sign-only 密钥包（CI 每次构建；叶即用即弃）
    MintLeaf {
        /// 中间 CA 材料（split-keys 的 --issuing-out 或 CI Secrets 组装的同形态包）
        #[arg(long)]
        issuing: String,
        /// 叶有效期天数（CI 用 365——无时间戳体系，短叶会让旧 release 验签报 Expired）
        #[arg(long, default_value_t = 365)]
        days: u64,
        /// 叶 CN（构建标识，如 "NemesisBot CI <sha>"）
        #[arg(long)]
        cn: String,
        /// 输出 sign-only 包（含叶私钥——CI 落 runner 临时目录，job 结束即湮灭）
        #[arg(long)]
        out: String,
    },
    /// 用 leaf 私钥签目标文件（带三级证书链；--keys 接受全量包或 sign-only 包）
    Sign {
        #[arg(long)]
        keys: String,
        /// 目标文件
        #[arg(long)]
        target: String,
        #[arg(long)]
        out: Option<String>,
    },
    /// 验证目标文件（lib 直接验签；锚二选一：--root-cert 纯公开材料 / --keys 包内根证书）
    #[command(group(
        ArgGroup::new("anchor")
            .required(true)
            .args(&["keys", "root_cert"]),
    ))]
    Verify {
        /// keys 包（取其 root_cert 算锚——不要求任何私钥在场）
        #[arg(long)]
        keys: Option<String>,
        /// 根证书 DER 文件（零私钥验证入口）
        #[arg(long)]
        root_cert: Option<String>,
        /// 目标文件
        #[arg(long)]
        target: String,
        #[arg(long)]
        revocation_url: Option<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Keygen { out } => {
            let h = nemesis_verify::keygen::generate()?;
            // 生成即自检：防「生成即坏」的密钥体系流出到仪式流程下游。
            bundle::validate_full_consistency(&h, now_secs())?;
            println!(
                "root anchor (NEMESIS_BUILD_ROOT_ANCHOR 注入值): {}",
                hex_encode(&h.root_anchor_fingerprint())
            );
            println!(
                "leaf pubkey: {}",
                hex_encode(&nemesis_verify::crypto::public_key_bytes(&h.leaf_vk()))
            );
            h.save(&out)?;
            println!("✓ keys → {}", out);
        }
        Cmd::SplitKeys {
            input,
            root_out,
            issuing_out,
            root_cert_out,
        } => cmd_split_keys(&input, &root_out, &issuing_out, root_cert_out.as_deref())?,
        Cmd::MintLeaf {
            issuing,
            days,
            cn,
            out,
        } => cmd_mint_leaf(&issuing, days, &cn, &out)?,
        Cmd::Sign { keys, target, out } => {
            let mat = bundle::SigningMaterial::load(&keys)?;
            let content = std::fs::read(&target)?;
            // v4 Authenticode（S5-1）：PE → Certificate Table，ELF/raw → v4
            // footer 载体（分派在 sign_content_v4 内，与 verify 同源）。
            let signed =
                verify::sign_content_v4(&content, &mat.leaf_sk, now_secs(), &mat.chain(), None)?;
            let out = out.unwrap_or_else(|| format!("{}.signed", target));
            std::fs::write(&out, signed)?;
            println!("✓ signed → {}", out);
        }
        Cmd::Verify {
            keys,
            root_cert,
            target,
            revocation_url,
        } => {
            if let Some(url) = revocation_url {
                // edition 2024: set_var unsafe
                unsafe {
                    std::env::set_var("NEMESIS_REVOCATION_URL", url);
                }
            }
            let anchor = resolve_verify_anchor(keys.as_deref(), root_cert.as_deref())?;
            let bytes = std::fs::read(&target)?;
            let outcome = verify::verify_bytes(&bytes, &[anchor], now_secs());
            println!("{}", outcome_name(&outcome));
            match outcome {
                verify::VerifyOutcome::Valid { .. } => {}
                _ => std::process::exit(1),
            }
        }
    }
    Ok(())
}

/// 拆分全量包 →（根材料, 中间 CA 材料）+ 可选根证书 DER 导出。
fn cmd_split_keys(
    input: &str,
    root_out: &str,
    issuing_out: &str,
    root_cert_out: Option<&str>,
) -> Result<()> {
    let h = KeyHierarchy::load(input)?;
    let (root_mat, issuing_mat) = bundle::split_keys(&h, now_secs())?;
    root_mat.save(root_out)?;
    issuing_mat.save(issuing_out)?;
    if let Some(rc) = root_cert_out {
        std::fs::write(rc, root_mat.root_cert.to_der())?;
    }
    println!("root anchor: {}", hex_encode(&root_mat.root_anchor()));
    println!("✓ 根材料（离线保管） → {root_out}");
    println!("✓ 中间 CA 材料（进 Secrets 原材料） → {issuing_out}");
    if let Some(rc) = root_cert_out {
        println!("✓ 根证书公开部分 → {rc}");
    }
    println!("（输入文件未删除——私钥销毁由你显式执行）");
    Ok(())
}

/// 从中间 CA 材料现铸叶 → sign-only 包落盘。
fn cmd_mint_leaf(issuing_path: &str, days: u64, cn: &str, out: &str) -> Result<()> {
    let iss = bundle::IssuingMaterial::load(issuing_path)?;
    let mat = bundle::mint_leaf(&iss, days, cn, now_secs())?;
    mat.save(out)?;
    println!("root anchor: {}", hex_encode(&mat.root_anchor()));
    println!("✓ sign-only 包 → {out}（leaf CN={cn}, {days}d）");
    Ok(())
}

/// 验证锚解析：`--keys` / `--root-cert` 互斥二选一（clap ArgGroup 已挡，此处兜底诚实拒绝）。
fn resolve_verify_anchor(keys: Option<&str>, root_cert: Option<&str>) -> Result<[u8; 32]> {
    match (keys, root_cert) {
        (Some(k), None) => bundle::load_root_anchor(k),
        (None, Some(rc)) => bundle::root_anchor_from_der_file(rc),
        _ => bail!("--keys 与 --root-cert 必须二选一"),
    }
}

fn outcome_name(o: &verify::VerifyOutcome) -> String {
    use verify::VerifyOutcome;
    match o {
        VerifyOutcome::Valid {
            signed_at,
            key_fp,
            pubkey,
        } => format!(
            "Valid (signed_at={}, key_fp={}, pubkey={})",
            signed_at,
            hex_encode(key_fp),
            hex_encode(pubkey)
        ),
        VerifyOutcome::NoSignature => "NoSignature".into(),
        VerifyOutcome::Tampered(s) => format!("Tampered({})", s),
        VerifyOutcome::SignatureInvalid => "SignatureInvalid".into(),
        VerifyOutcome::Untrusted => "Untrusted".into(),
        VerifyOutcome::Revoked {
            dim, value, reason, ..
        } => {
            format!("Revoked({:?}={}:{})", dim, value, reason)
        }
        VerifyOutcome::Expired(s) => format!("Expired({})", s),
        VerifyOutcome::UnsupportedVersion(v) => format!("UnsupportedVersion({})", v),
        VerifyOutcome::Malformed(s) => format!("Malformed({})", s),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
