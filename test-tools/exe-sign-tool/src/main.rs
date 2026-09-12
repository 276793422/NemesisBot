//! exe-sign-tool（**v4**）：可执行文件签名/验证 CLI。
//!
//! v4 架构（Authenticode 对齐）：ECDSA P-256 + X.509 三级证书链（公钥随签名走），
//! **lib 直接验签**（不加载 DLL）。与 verify-loader 区别：本工具用 `nemesis-verify`
//! lib 直接 `verify_bytes`；verify-loader 加载 DLL（C ABI）。
//!
//! 用法：
//! ```sh
//! exe-sign-tool keygen --out keys.json
//! exe-sign-tool sign --keys keys.json myapp.exe [--out myapp.signed.exe]
//! exe-sign-tool verify --keys keys.json myapp.exe [--revocation-url http://127.0.0.1:7878]
//! ```

use anyhow::Result;
use clap::{Parser, Subcommand};
use nemesis_verify::{hex_util::hex_encode, keygen::KeyHierarchy, verify};

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
    /// 用 leaf 私钥签目标文件（带三级证书链）
    Sign {
        #[arg(long)]
        keys: String,
        /// 目标文件
        target: String,
        #[arg(long)]
        out: Option<String>,
    },
    /// 验证目标文件（lib 直接验签；--revocation-url 配则联网查 CRL）
    Verify {
        #[arg(long)]
        keys: String,
        target: String,
        #[arg(long)]
        revocation_url: Option<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Keygen { out } => {
            let h = nemesis_verify::keygen::generate()?;
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
        Cmd::Sign { keys, target, out } => {
            let h = KeyHierarchy::load(&keys)?;
            let content = std::fs::read(&target)?;
            // v4 Authenticode（S5-1）：PE → Certificate Table，ELF/raw → v4
            // footer 载体（分派在 sign_content_v4 内，与 verify 同源）。
            let signed =
                verify::sign_content_v4(&content, &h.leaf_sk, now_secs(), &h.chain(), None)?;
            let out = out.unwrap_or_else(|| format!("{}.signed", target));
            std::fs::write(&out, signed)?;
            println!("✓ signed → {}", out);
        }
        Cmd::Verify {
            keys,
            target,
            revocation_url,
        } => {
            if let Some(url) = revocation_url {
                // edition 2024: set_var unsafe
                unsafe {
                    std::env::set_var("NEMESIS_REVOCATION_URL", url);
                }
            }
            let h = KeyHierarchy::load(&keys)?;
            let bytes = std::fs::read(&target)?;
            let outcome = verify::verify_bytes(&bytes, &[h.root_anchor_fingerprint()], now_secs());
            println!("{}", outcome_name(&outcome));
            match outcome {
                verify::VerifyOutcome::Valid { .. } => {}
                _ => std::process::exit(1),
            }
        }
    }
    Ok(())
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
