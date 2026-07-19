//! exe-sign-tool CLI：`keygen` / `sign` / `verify`（简化版）。
//!
//! 最简流程（密钥默认在 `./keys`）：
//! ```sh
//! exe-sign-tool keygen                 # 生成到 ./keys（keygen 默认 --out keys）
//! exe-sign-tool sign myapp.exe         # 从 ./keys 读 key + sym
//! exe-sign-tool verify myapp.exe       # 从 ./keys 读 pub + sym
//! ```
//! 细粒度 `--key`/`--sym`/`--pub-hex`/`--pub-file` 可选覆盖；`--key-dir` 指定密钥目录。

use clap::{Parser, Subcommand};
use exe_sign_tool::{
    crypto, load_signing_key, sign_executable, verify_executable, CloudClient, RevocationPolicy,
};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "exe-sign-tool",
    version,
    about = "可执行文件签名、验证工具（PE / ELF / Raw）"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 生成 Ed25519 公私钥对 + ChaCha20 对称密钥（SYM_KEY）
    Keygen {
        /// 输出目录（默认 ./keys）
        #[arg(long, default_value = "keys")]
        out: PathBuf,
    },
    /// 给可执行文件签名（原地追加 envelope）
    Sign {
        /// 目标可执行文件（位置参数）
        file: PathBuf,
        /// 密钥目录（从中读 exe_sign.key + exe_sign.sym；默认 ./keys）
        #[arg(long, default_value = "keys")]
        key_dir: PathBuf,
        /// 覆盖：私钥文件（缺省 <key-dir>/exe_sign.key）
        #[arg(long)]
        key: Option<PathBuf>,
        /// 覆盖：对称密钥文件（缺省 <key-dir>/exe_sign.sym，若无则走 NEMESIS_SYM_KEY/默认）
        #[arg(long)]
        sym: Option<PathBuf>,
        /// 签名时间戳（Unix epoch）；缺省取当前时间
        #[arg(long)]
        signed_at: Option<u64>,
        /// 公钥标识（提示用）；缺省 0
        #[arg(long)]
        key_id: Option<u32>,
        /// 发布者（写入 envelope，供按发布者吊销；可选）
        #[arg(long)]
        publisher: Option<String>,
        /// 有效期天数（signed_at + days → expires_at；缺省=无过期）
        #[arg(long)]
        expires_in: Option<u64>,
    },
    /// 验证签名
    Verify {
        /// 目标可执行文件（位置参数）
        file: PathBuf,
        /// 密钥目录（从中读 exe_sign.pub + exe_sign.sym；默认 ./keys）
        #[arg(long, default_value = "keys")]
        key_dir: PathBuf,
        /// 覆盖：hex 公钥
        #[arg(long = "pub-hex")]
        pub_hex: Option<String>,
        /// 覆盖：公钥文件（缺省 <key-dir>/exe_sign.pub）
        #[arg(long = "pub-file")]
        pub_file: Option<PathBuf>,
        /// 覆盖：对称密钥文件（缺省 <key-dir>/exe_sign.sym，若无则走 NEMESIS_SYM_KEY/默认）
        #[arg(long)]
        sym: Option<PathBuf>,
        /// 吊销服务端 URL（如 http://127.0.0.1:7878）；不配则纯本地验证
        #[arg(long)]
        cloud_url: Option<String>,
        /// 吊销根公钥 hex（验云端响应）；--cloud-url 时缺省读 <key-dir>/exe_sign.crpub
        #[arg(long)]
        crpub: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Keygen { out } => keygen(&out),
        Cmd::Sign {
            file,
            key_dir,
            key,
            sym,
            signed_at,
            key_id,
            publisher,
            expires_in,
        } => {
            let sk = resolve_signing_key(key, &key_dir)?;
            let sym_key = resolve_sym(sym, &key_dir)?;
            let signed_at = signed_at.unwrap_or_else(|| chrono::Utc::now().timestamp() as u64);
            let key_id = key_id.unwrap_or(0);
            let expires_at = expires_in.map(|d| signed_at.saturating_add(d * 86400));
            sign_executable(
                &file,
                &sk,
                &sym_key,
                signed_at,
                key_id,
                publisher.as_deref(),
                expires_at,
            )?;
            println!("signed: {}", file.display());
            Ok(())
        }
        Cmd::Verify {
            file,
            key_dir,
            pub_hex,
            pub_file,
            sym,
            cloud_url,
            crpub,
        } => {
            let vk = resolve_verifying_key(pub_hex, pub_file, &key_dir)?;
            let sym_key = resolve_sym(sym, &key_dir)?;
            let cloud = if let Some(url) = cloud_url {
                let crpub_hex = match crpub {
                    Some(h) => h,
                    None => std::fs::read_to_string(key_dir.join("exe_sign.crpub"))?
                        .trim()
                        .to_string(),
                };
                Some(CloudClient::new(&url, &crpub_hex)?)
            } else {
                None
            };
            let bytes = std::fs::read(&file)?;
            let now = chrono::Utc::now().timestamp() as u64;
            let policy = RevocationPolicy::offline(now);
            let status = verify_executable(&bytes, &vk, &sym_key, &policy, cloud.as_ref())?;
            println!(
                "code={:?} cloud={:?} source={:?} signed_at={} detail={}",
                status.code, status.cloud_state, status.source, status.signed_at, status.detail
            );
            if status.is_valid() {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
    }
}

/// 生成密钥：签名密钥对 + 吊销根密钥对 + ChaCha20 SYM_KEY，落盘 6 个文件。
///
/// - `exe_sign.key`/`.pub` — Ed25519 签名密钥对（签 PE/ELF/Raw）
/// - `exe_sign.sym` — ChaCha20 对称密钥（加密 envelope）
/// - `exe_sign.crkey`/`.crpub` — 吊销根密钥对（签 CRL/云端响应，防 MITM）
/// - `exe_sign.meta.json` — 元信息（含 revocation_root_pub）
fn keygen(out: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(out)?;
    let kp = crypto::generate_key_pair(); // 签名密钥对
    let crkp = crypto::generate_key_pair(); // 吊销根密钥对（签 CRL / 云端响应）
    let sym = crypto::generate_sym_key();
    let sym_hex = exe_sign_tool::hex_util::hex_encode(&sym);
    std::fs::write(out.join("exe_sign.key"), &kp.private_key)?;
    std::fs::write(out.join("exe_sign.pub"), &kp.public_key)?;
    std::fs::write(out.join("exe_sign.sym"), &sym_hex)?;
    std::fs::write(out.join("exe_sign.crkey"), &crkp.private_key)?;
    std::fs::write(out.join("exe_sign.crpub"), &crkp.public_key)?;
    let meta = format!(
        "{{\n  \"algorithm\": \"ed25519\",\n  \"scheme\": \"nemesis-bin-sig-v2\",\n  \"enc_algorithm\": \"chacha20-poly1305\",\n  \"created\": \"{}\",\n  \"public_key\": \"{}\",\n  \"revocation_root_pub\": \"{}\"\n}}\n",
        chrono::Local::now().to_rfc3339(),
        kp.public_key,
        crkp.public_key
    );
    std::fs::write(out.join("exe_sign.meta.json"), meta)?;
    println!("generated keypair + sym_key + revocation_root in {}", out.display());
    println!("public_key={}  (签名)", kp.public_key);
    println!(
        "revocation_root_pub={}  (吊销根，签 CRL/响应)",
        crkp.public_key
    );
    println!("sym_key={}  (机密)", sym_hex);
    Ok(())
}

/// 解析私钥：--key 覆盖，缺省 <key-dir>/exe_sign.key。
fn resolve_signing_key(
    key_file: Option<PathBuf>,
    key_dir: &Path,
) -> anyhow::Result<ed25519_dalek::SigningKey> {
    let p = key_file.unwrap_or_else(|| key_dir.join("exe_sign.key"));
    load_signing_key(&p).map_err(|e| anyhow::anyhow!("signing key ({}): {}", p.display(), e))
}

/// 解析公钥：--pub-hex > --pub-file > <key-dir>/exe_sign.pub。
fn resolve_verifying_key(
    pub_hex: Option<String>,
    pub_file: Option<PathBuf>,
    key_dir: &Path,
) -> anyhow::Result<ed25519_dalek::VerifyingKey> {
    if let Some(h) = pub_hex {
        return crypto::verifying_key_from_hex(&h).map_err(|e| anyhow::anyhow!("pub-hex: {}", e));
    }
    let p = pub_file.unwrap_or_else(|| key_dir.join("exe_sign.pub"));
    let hex = std::fs::read_to_string(&p)
        .map_err(|e| anyhow::anyhow!("read public key ({}): {}", p.display(), e))?
        .trim()
        .to_string();
    crypto::verifying_key_from_hex(&hex)
        .map_err(|e| anyhow::anyhow!("public key ({}): {}", p.display(), e))
}

/// 解析对称密钥：--sym > <key-dir>/exe_sign.sym（若存在）> NEMESIS_SYM_KEY/默认。
fn resolve_sym(sym_file: Option<PathBuf>, key_dir: &Path) -> anyhow::Result<[u8; 32]> {
    if let Some(p) = sym_file {
        let hex = std::fs::read_to_string(&p)?.trim().to_string();
        return crypto::sym_key_from_hex(&hex);
    }
    let p = key_dir.join("exe_sign.sym");
    if p.exists() {
        let hex = std::fs::read_to_string(&p)?.trim().to_string();
        crypto::sym_key_from_hex(&hex).map_err(|e| anyhow::anyhow!("sym key ({}): {}", p.display(), e))
    } else {
        Ok(crypto::get_sym_key())
    }
}
