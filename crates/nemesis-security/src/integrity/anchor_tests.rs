//! BUG-6（2026-11-25 远程 home 取证：审计链与被审计物同存储同权限）回归：
//! 链头外置锚语义在此钉死。锚落与链目录**不同故障域**（生产装配给
//! `config/`），链被整链清除/截断后锚仍在 → 回载对账可检测「审计链被清」
//! 痕迹（BUG 文档备注的裁决口径：目标不是防有权限的破坏者，是把「误删/
//! 故障波及」与「蓄意破坏」区分开——留下可检测痕迹）。
//!
//! 钉死的语义：
//! 1. 首启（无链无锚）= 落 genesis 锚（静默）；
//! 2. 每次 append 链尾推进后锚同步（head_hash/total 与链一对一）；
//! 3. 无锚 + 有链 = 按当前链尾重建基线 + WARN（升级部署/锚被删，非异常）；
//! 4. 有锚 + 盘上链尾 ≠ 锚头（含整链被清）= 锚文件**原样保留**（证据不
//!    覆盖）+ CRITICAL 告警，后续 append 把锚重新对齐（自愈，痕迹已留）。

use super::*;

/// 锚 + 链的测试装配：链目录与锚目录刻意放在不同子树（模拟生产
/// `security_chain/` vs `config/` 的故障域分离）。
struct AnchorFixture {
    #[allow(dead_code)]
    tmp: tempfile::TempDir,
    chain_path: PathBuf,
    anchor_path: PathBuf,
}

impl AnchorFixture {
    fn new(tag: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let chain_path = tmp.path().join("security_chain").join("audit_chain.jsonl");
        let anchor_path = tmp
            .path()
            .join("config")
            .join(format!("audit_chain_anchor_{tag}.json"));
        std::fs::create_dir_all(chain_path.parent().unwrap()).expect("chain dir");
        std::fs::create_dir_all(anchor_path.parent().unwrap()).expect("anchor dir");
        Self {
            tmp,
            chain_path,
            anchor_path,
        }
    }

    fn config(&self) -> AuditChainConfig {
        AuditChainConfig {
            enabled: true,
            storage_path: self.chain_path.clone(),
            max_file_size: 50 * 1024 * 1024,
            verify_on_load: false,
            max_events_per_segment: 100_000,
            signing_key: None,
        }
    }

    fn chain(&self) -> AuditChain {
        let chain = AuditChain::new(self.config());
        chain.set_anchor_path(Some(self.anchor_path.clone()));
        chain
    }

    fn read_anchor(&self) -> ChainAnchor {
        let raw = std::fs::read_to_string(&self.anchor_path).expect("anchor readable");
        serde_json::from_str(&raw).expect("anchor parseable")
    }
}

/// 语义 1+2：首启落 genesis 锚；append 链尾推进后锚同步。
#[test]
fn anchor_genesis_then_tracks_appends() {
    let fx = AnchorFixture::new("genesis");
    let chain = fx.chain();
    assert_eq!(chain.load_segments().expect("load"), 0);

    let anchor = fx.read_anchor();
    assert_eq!(anchor.total_events, 0, "首启 genesis 锚 0 事件");
    assert!(
        anchor.head_hash.chars().all(|c| c == '0'),
        "genesis 锚头 = 全零: {}",
        anchor.head_hash
    );

    chain
        .append("exec", "shell", "u", "web", "t1", "deny", "r1")
        .expect("append 1");
    chain
        .append("exec", "shell", "u", "web", "t2", "allow", "r2")
        .expect("append 2");

    let anchor = fx.read_anchor();
    assert_eq!(anchor.total_events, 2, "锚同步事件数");
    assert_eq!(anchor.head_hash, chain.current_hash(), "锚头 = 链尾");
}

/// 语义 3：无锚 + 有链 = 按当前链尾重建基线（升级部署/锚被删形态）。
#[test]
fn anchor_missing_adopts_current_tail() {
    let fx = AnchorFixture::new("adopt");
    let chain = fx.chain();
    chain.load_segments().expect("load");
    chain
        .append("exec", "shell", "u", "web", "t1", "deny", "r1")
        .expect("append 1");
    chain
        .append("exec", "shell", "u", "web", "t2", "allow", "r2")
        .expect("append 2");

    // 锚被删 → 新实例回载 → 重建基线（头 = 盘上链尾）。
    std::fs::remove_file(&fx.anchor_path).expect("anchor removed");
    let chain2 = fx.chain();
    assert_eq!(chain2.load_segments().expect("load"), 2);
    let anchor = fx.read_anchor();
    assert_eq!(anchor.total_events, 2, "重建基线采纳当前链尾");
    assert_eq!(anchor.head_hash, chain2.current_hash());
}

/// 语义 4a：整链被清 = 锚原样保留（证据不覆盖）+ 头不一致可检测；
/// 后续 append 把锚重新对齐（自愈）。
#[test]
fn anchor_detects_full_chain_wipe_and_self_heals_on_append() {
    let fx = AnchorFixture::new("wipe");
    let chain = fx.chain();
    chain.load_segments().expect("load");
    chain
        .append("exec", "shell", "u", "web", "t1", "deny", "r1")
        .expect("append 1");
    chain
        .append("exec", "shell", "u", "web", "t2", "allow", "r2")
        .expect("append 2");
    let anchor_before = std::fs::read_to_string(&fx.anchor_path).expect("anchor");

    // 模拟整链被清（BUG-6 现场：logs/ 级删除波及——修复后链在独立目录，
    // 本测试直接删链文件模拟「蓄意清链」的检测面）。
    std::fs::remove_file(&fx.chain_path).expect("chain wiped");

    let chain2 = fx.chain();
    assert_eq!(
        chain2.load_segments().expect("load"),
        0,
        "链被清回载 0 事件"
    );

    // 锚原样保留（字节不变 = 证据不覆盖），与空链 genesis 尾不一致 = 可检测。
    let anchor_after = std::fs::read_to_string(&fx.anchor_path).expect("anchor preserved");
    assert_eq!(
        anchor_before, anchor_after,
        "整链被清后锚文件必须原样保留（证据不覆盖）"
    );
    let anchor = fx.read_anchor();
    assert_eq!(anchor.total_events, 2);
    assert_ne!(
        anchor.head_hash,
        chain2.current_hash(),
        "锚头（旧链尾）≠ 盘上 genesis 尾 = 「链被清」可检测痕迹"
    );

    // 自愈：新链 append 后锚重新对齐。
    chain2
        .append("exec", "shell", "u", "web", "t3", "allow", "r3")
        .expect("append 3");
    let healed = fx.read_anchor();
    assert_eq!(
        healed.head_hash,
        chain2.current_hash(),
        "自愈后锚头 = 新链尾"
    );
    assert_eq!(healed.total_events, 1);
}

/// 语义 4b：链被截断（尾段被剪）= 锚原样保留 + 头不一致可检测。
#[test]
fn anchor_detects_chain_truncation() {
    let fx = AnchorFixture::new("trunc");
    let chain = fx.chain();
    chain.load_segments().expect("load");
    chain
        .append("exec", "shell", "u", "web", "t1", "deny", "r1")
        .expect("append 1");
    chain
        .append("exec", "shell", "u", "web", "t2", "allow", "r2")
        .expect("append 2");

    // 截断到只剩第一行（末事件被剪 = 篡改痕迹形态之一）。
    let content = std::fs::read_to_string(&fx.chain_path).expect("chain readable");
    let first_line = content.lines().next().expect("first line");
    std::fs::write(&fx.chain_path, format!("{first_line}\n")).expect("truncate");

    let chain2 = fx.chain();
    assert_eq!(chain2.load_segments().expect("load"), 1);
    let anchor = fx.read_anchor();
    assert_eq!(anchor.total_events, 2, "锚仍记录截断前状态（证据）");
    assert_ne!(anchor.head_hash, chain2.current_hash(), "锚头 ≠ 截断后链尾");
}

/// 无锚注入（既有构造点）= 锚面整体关闭：append/回载零锚文件副作用。
#[test]
fn anchor_disabled_by_default_has_no_side_effects() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let chain_path = tmp.path().join("audit_chain.jsonl");
    let config = AuditChainConfig {
        enabled: true,
        storage_path: chain_path.clone(),
        max_file_size: 50 * 1024 * 1024,
        verify_on_load: false,
        max_events_per_segment: 100_000,
        signing_key: None,
    };
    let chain = AuditChain::new(config);
    chain.load_segments().expect("load");
    chain
        .append("exec", "shell", "u", "web", "t1", "deny", "r1")
        .expect("append");
    assert!(chain_path.exists(), "链照常落盘");
    assert!(
        !tmp.path().join("config").exists(),
        "无锚注入 = 不产生任何锚文件"
    );
}
