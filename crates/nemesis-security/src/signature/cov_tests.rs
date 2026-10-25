// signature.rs 覆盖率补充测试（TrustStore 落盘 save 尾段 + verify_skill 的
// signature base64 解码失败臂）。
//
// 豁免（死防御臂，仅记录不硬凑）：
// - 959（compute_directory_hash 循环内的根级 .signature 跳过 continue）：
//   filter_entry（940-946）已经把根级 .signature 从 walker 剪掉（谓词对
//   非目录且名为 .signature 且 rel.parent 为空 → false → 不产出也不下钻），
//   循环体永远见不到根级 .signature，该 continue 不可达——是过滤器之外的
//   二道防线。嵌套 .signature（sub/.signature）会被刻意哈希（父目录非空
//   → 不过滤也不跳过），由 hash-mismatch 流程测试实际走到。

use super::*;
use base64::Engine;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-sig-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn b64(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.encode(bytes)
}

/// RFC 8032 测试向量 1 的公钥（保证是合法 ed25519 曲线点——from_bytes 对
/// 任意 32 字节可能解压失败，必须用已验证可解压的键才能走到后续分支）。
const VALID_PUBKEY_B64: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";

fn write_signature(dir: &std::path::Path, signature_b64: &str) {
    let sig = serde_json::json!({
        "algorithm": "ed25519",
        "signature": signature_b64,
        "public_key": VALID_PUBKEY_B64,
        "signed_at": "2026-09-25T00:00:00Z",
        "file_count": 1,
        "hash": "00",
    });
    std::fs::write(dir.join(".signature"), serde_json::to_string(&sig).unwrap()).unwrap();
}

/// TrustStore 带路径：add_key 触发 save 落盘（250-265 尾段），remove 后
/// 重开实例能看到持久化结果。注意 load() 会校验键材料（b64/hex 32 字节
/// ed25519），非法键在重载时被静默跳过——必须用真实公钥。
#[test]
fn trust_store_with_path_persists_across_reopen() {
    let dir = temp_dir("store");
    let store_path = dir.join("nested").join("trust.json");

    {
        let store = TrustStore::new(Some(&store_path));
        store.add_key(VALID_PUBKEY_B64, "sasha", TrustLevel::Verified);
        assert_eq!(store.key_count(), 1);
        assert!(store_path.exists(), "add_key 必须落盘");
    }

    // 重开（从磁盘加载）→ 键仍在；remove_key 再次触发 save。
    let store = TrustStore::new(Some(&store_path));
    assert_eq!(store.key_count(), 1);
    assert!(store.is_trusted(VALID_PUBKEY_B64).1);
    assert!(store.remove_key("sasha"));
    assert_eq!(store.key_count(), 0);

    let raw = std::fs::read_to_string(&store_path).unwrap();
    assert!(!raw.contains("sasha"), "删除后落盘不得残留旧键: {raw}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// .signature 的 signature 字段不是合法 base64 → 解码失败臂
/// （612-621，"invalid signature encoding"）。
#[test]
fn verify_skill_reports_invalid_signature_encoding() {
    let dir = temp_dir("badb64");
    write_signature(&dir, "!!!not-base64!!!");

    let verifier = Verifier::new(Config::default()).unwrap();
    let r = verifier.verify_skill(&dir);
    assert!(!r.valid);
    assert!(
        r.error.contains("invalid signature encoding"),
        "必须报 base64 解码失败: {}",
        r.error
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 非法长度（非 64 字节）的签名 → 长度臂；随后合法 64 字节但错误签名
/// → 走完整目录哈希（根级 .signature 被 filter_entry 剪掉、嵌套的被哈希）
/// → "signature verification failed"。
#[test]
fn verify_skill_length_arm_then_full_hash_flow_with_signature_files() {
    let dir = temp_dir("len");

    // 长度臂：base64 解码成功但只有 10 字节。
    write_signature(&dir, &b64(&[1u8; 10]));
    let verifier = Verifier::new(Config::default()).unwrap();
    let r = verifier.verify_skill(&dir);
    assert!(r.error.contains("invalid signature length"), "{}", r.error);

    // 完整哈希流：目录含内容文件 + 嵌套 .signature（会被计入哈希）。
    std::fs::write(dir.join("payload.txt"), b"cov payload").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join(".signature"), b"nested sig is content").unwrap();
    write_signature(&dir, &b64(&[0u8; 64]));
    let r2 = verifier.verify_skill(&dir);
    assert!(!r2.valid);
    assert_eq!(
        r2.files_verified, 2,
        "实际哈希的文件数（根级 .signature 不算）"
    );
    // 嵌入 hash 字段与实际聚合哈希不符 → 内容篡改臂（在 ed25519 验证之前）。
    assert!(r2.error.contains("hash mismatch"), "{}", r2.error);

    let _ = std::fs::remove_dir_all(&dir);
}

/// 信任库磁盘真相的跨进程可见性（2026-09-30 插件链路测试实锤的缓存陈旧
/// 缺口）：常驻进程在构造 TrustStore 之后，CLI（独立进程）直接写盘新增
/// 信任 / 吊销——读入口（is_trusted/trust_level）必须重读磁盘让两个方向
/// 都即时生效：新增不可见则「先信后装」热信任流断链；吊销不可见则已撤
/// 密钥仍按 trusted 放行，安全语义反向恶化。
#[test]
fn trust_store_refreshes_from_disk_for_running_reader() {
    let dir = temp_dir("refresh");
    let store_path = dir.join("trust.json");

    // 读者进程：先于写入者构造（模拟 gateway 启动时信任库尚不存在）。
    let reader = TrustStore::new(Some(&store_path));
    assert!(
        !reader.is_trusted(VALID_PUBKEY_B64).1,
        "构造期无文件 → 不受信"
    );

    // 写入者进程：独立实例新增信任（= CLI plugin trust）。
    let writer = TrustStore::new(Some(&store_path));
    writer.add_key(VALID_PUBKEY_B64, "cli-ca", TrustLevel::Verified);

    // 读者不重启：新增信任必须可见。
    let (level, trusted) = reader.is_trusted(VALID_PUBKEY_B64);
    assert!(trusted, "CLI 新增信任对运行中读者不可见（缓存陈旧）");
    assert_eq!(level, TrustLevel::Verified);
    assert_eq!(reader.trust_level(VALID_PUBKEY_B64), TrustLevel::Verified);

    // 写入者进程吊销（= CLI plugin trust-revoke）——吊销必须同样即时可见。
    writer.revoke_key("cli-ca").unwrap();
    assert!(
        !reader.is_trusted(VALID_PUBKEY_B64).1,
        "CLI 吊销对运行中读者不可见（缓存陈旧，安全语义反向恶化）"
    );
    assert_eq!(reader.trust_level(VALID_PUBKEY_B64), TrustLevel::Revoked);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// BUG-20261025-38：revoke 持久化失败必须 loud 留痕
//
// 同文件 add_key/remove_key 的文档化不变量：「信任库持久化失败不能静默吞
// 掉：内存态信任与盘上真相分叉……loud 留痕」。revoke 双函数此前 `let _ =
// self.save()` 静默吞——重启后吊销失效=已撤密钥复信（安全语义反向恶化，
// 与 remove 同级）。回归锁：save 失败场景（store 路径父位置放普通文件 →
// create_dir_all 必败，跨平台确定性）断言 ERROR 日志带「吊销未落盘」。
// ---------------------------------------------------------------------------

use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct LogSink38(Arc<Mutex<String>>);

impl std::io::Write for LogSink38 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn revoke_loud_setup(
    dir: &std::path::Path,
) -> (
    TrustStore,
    Arc<Mutex<String>>,
    // subscriber 守卫必须返回给测试体持有：thread-local set_default 的
    // 生命周期 = guard 存活期，setup 内 `let _guard` 会在返回时卸载，
    // 后续 revoke 的 emit 一个都收不到（首版 harness 实测踩中）。
    tracing::subscriber::DefaultGuard,
) {
    // 阻断落盘：把 store 的父目录位置放普通文件。
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let store_path = blocker.join("skill_trust.json");

    let buf = Arc::new(Mutex::new(String::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_writer({
            let buf = buf.clone();
            move || LogSink38(buf.clone())
        })
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    let store = TrustStore::new(Some(&store_path));
    store.add_key(VALID_PUBKEY_B64, "loud-revoke-38", TrustLevel::Verified);
    (store, buf, guard)
}

#[test]
fn test_revoke_key_save_failure_loud_log() {
    let dir = temp_dir("revoke-loud");
    let (store, buf, _guard) = revoke_loud_setup(&dir);

    assert!(store.revoke_key("loud-revoke-38").is_ok());
    let log = buf.lock().unwrap().clone();
    assert!(
        log.contains("信任库持久化失败") && log.contains("的吊销未落盘"),
        "revoke save 失败必须 loud 留痕，实得日志：{log}"
    );
    // add_key 兄弟不变量基线（同场景 loud，钉住口径不回退）。
    assert!(log.contains("仅存内存"), "add_key loud 基线缺失：{log}");
    // 内存态语义不变：吊销在内存生效（盘上分叉由 CLI 盘上真相回读兜底，
    // 层契约只加留痕不翻返回值——与 add/remove 同形态）。
    assert_eq!(store.trust_level(VALID_PUBKEY_B64), TrustLevel::Revoked);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_revoke_key_by_public_key_save_failure_loud_log() {
    let dir = temp_dir("revoke-pk-loud");
    let (store, buf, _guard) = revoke_loud_setup(&dir);

    assert!(store.revoke_key_by_public_key(VALID_PUBKEY_B64).is_ok());
    let log = buf.lock().unwrap().clone();
    assert!(
        log.contains("信任库持久化失败") && log.contains("的吊销未落盘"),
        "revoke_by_public_key save 失败必须 loud 留痕，实得日志：{log}"
    );
    assert_eq!(store.trust_level(VALID_PUBKEY_B64), TrustLevel::Revoked);

    let _ = std::fs::remove_dir_all(&dir);
}
