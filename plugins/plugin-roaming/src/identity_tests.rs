//! 身份面单测：文件路径/格式/权限语义 + 与 iroh 官方实现的数学等价
//! （同 seed 双实现 node_id 一致——插件 ed25519-dalek 手工实现 vs 插件内
//! iroh::SecretKey 官方派生，交叉验证不依赖宿主）。

use super::identity::{Identity, roaming_key_path};

#[test]
fn generate_produces_distinct_ids() {
    let a = Identity::generate();
    let b = Identity::generate();
    assert_ne!(a.node_id_hex(), b.node_id_hex(), "OS 随机源身份必须互异");
    assert_eq!(a.node_id_hex().len(), 64, "node_id = 32B 公钥 hex");
}

#[test]
fn from_seed_roundtrip_is_stable() {
    let seed: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(1));
    let id = Identity::from_seed(&seed);
    assert_eq!(id.seed(), seed, "seed 往返逐字节不变");
    let again = Identity::from_seed(&seed);
    assert_eq!(id.node_id_hex(), again.node_id_hex());
}

#[test]
fn load_or_create_persists_and_reloads() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (id1, created1) = Identity::load_or_create(tmp.path()).expect("首载生成");
    assert!(created1, "无文件 = 新生成");
    let path = roaming_key_path(tmp.path());
    assert!(path.exists(), "roaming.key 落 workspace/cluster/");
    // 文件内容 = hex 32B seed（iroh SecretKey 同格式——宿主若恢复 iroh 面
    // 读写同文件零迁移）。
    let text = std::fs::read_to_string(&path).expect("read");
    let bytes = hex::decode(text.trim()).expect("hex");
    assert_eq!(bytes.len(), 32);
    assert_eq!(hex::encode(&bytes), hex::encode(id1.seed()));
    // 二次加载 = 原身份。
    let (id2, created2) = Identity::load_or_create(tmp.path()).expect("次载复用");
    assert!(!created2, "有文件 = 不重生成");
    assert_eq!(id1.node_id_hex(), id2.node_id_hex());
}

#[test]
fn load_rejects_bad_files_loud() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("roaming.key");
    // 缺文件。
    assert!(Identity::load(&path).is_err());
    // 非 hex。
    std::fs::write(&path, "zzzz").expect("write");
    assert!(Identity::load(&path).is_err());
    // 长度错（16B）。
    std::fs::write(&path, "0123456789abcdef0123456789abcdef").expect("write");
    // .err().expect：身份含密钥材料不实现 Debug，expect_err 不可用。
    let err = Identity::load(&path).err().expect("长度错必须拒");
    assert!(err.contains("32"), "错误注明 seed 长度语义: {err}");
}

#[test]
fn same_seed_matches_iroh_official_identity() {
    // 数学等价：iroh SecretKey = ed25519 seed 32B 包装（官方实现），插件
    // ed25519-dalek SigningKey 手工实现同构——同 seed 双实现 node_id 必须
    // 一致（node_id = 公钥 32B hex；iroh PublicKey::as_bytes 即原始 32B）。
    let seed: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(11).wrapping_add(3));
    let plugin = Identity::from_seed(&seed).node_id_hex();
    let official = hex::encode(iroh::SecretKey::from_bytes(&seed).public().as_bytes());
    assert_eq!(plugin, official, "同 seed 固定向量双实现一致");
    // 随机身份多组交叉（generate 反向取 seed 喂官方派生）。
    for _ in 0..8 {
        let id = Identity::generate();
        let official = hex::encode(iroh::SecretKey::from_bytes(&id.seed()).public().as_bytes());
        assert_eq!(id.node_id_hex(), official, "随机身份双实现一致");
    }
}

#[test]
fn key_file_seed_format_is_iroh_secret_key_roundtrip() {
    // roaming.key 文件内容（hex 32B seed）= iroh SecretKey::to_bytes 同一
    // 材料：seed → 官方 SecretKey → to_bytes 往返逐字节不变（格式互通保证：
    // 未来宿主恢复 iroh 客户端面时同文件零迁移）。
    let id = Identity::generate();
    let sk = iroh::SecretKey::from_bytes(&id.seed());
    assert_eq!(sk.to_bytes(), id.seed(), "iroh SecretKey seed 往返不变");
    // node_id hex 也是合法 iroh EndpointId（真实曲线点，非任意 32B）。
    let bytes: [u8; 32] = hex::decode(id.node_id_hex())
        .expect("hex")
        .try_into()
        .expect("32B");
    iroh::EndpointId::from_bytes(&bytes).expect("插件 node_id 必须是合法 iroh EndpointId");
}
