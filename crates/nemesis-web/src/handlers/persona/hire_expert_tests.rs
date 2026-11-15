//! 雇佣专家（shop.hire_expert，2026-11-13 批次③；2026-11-15 原文直用改版）
//! 测试：载荷构造（temp workspace 文件级验证，无桥依赖——桥调用面在
//! experts::tests 的 hire 契约测试覆盖）。
//!
//! - 本地 raw.md 原文优先（离线可用）：prompt = 原文正文（剥 frontmatter）
//! - 本地缺席 → 在线拉取（CONTENT_CACHE 注入离线验证）并落盘保存 raw.md
//! - name/description 从原文 frontmatter 解析；name 空 → id 回落
//! - 正文 trim 后空 = 诚实拒绝
//! - slug/hired_from = 人格目录 id；tags 空数组

use super::tests::SHOP_TEST_LOCK;
use super::*;

const RAW_SAMPLE: &str = "---\nname: Code Master\ndescription: writes code\nemoji: 🧑‍💻\n---\n\n# Code Master\n\n## Core Mission\n写好代码。\n";

#[tokio::test]
async fn hire_payload_uses_local_raw_md_over_cache() {
    let _g = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let pdir = ws.join("personas").join("code-master");
    std::fs::create_dir_all(&pdir).unwrap();
    // 本地原文在场：内容缓存注入一份不同内容 → 本地必须胜出（若实现
    // 误走在线臂，name 会变成 OTHER、prompt 变 stale，断言当场暴露）。
    // 复合键 + builtin live 强败（code-master 非 builtin，live raw 探测
    // 404 不该出现在离线测试里）。
    std::fs::write(pdir.join("raw.md"), RAW_SAMPLE).unwrap();
    let _forced = ForcedBuiltinLiveFail::arm();
    CONTENT_CACHE.lock().unwrap().insert(
        "agency/code-master".to_string(),
        "---\nname: OTHER\n---\n\nstale".to_string(),
    );

    let payload = expert_hire_payload(ws.to_str().unwrap(), "code-master")
        .await
        .unwrap();
    assert_eq!(payload["slug"], "code-master");
    assert_eq!(payload["hired_from"], "code-master");
    assert_eq!(
        payload["name"], "Code Master",
        "name 从原文 frontmatter 解析"
    );
    assert_eq!(payload["summary"], "writes code");
    assert_eq!(payload["tags"], serde_json::json!([]));
    // 正文剥离 frontmatter 后 trim：前后空白剥离，正文逐字保留。
    assert_eq!(
        payload["prompt"],
        "# Code Master\n\n## Core Mission\n写好代码。"
    );
    CONTENT_CACHE.lock().unwrap().clear();
}

#[tokio::test]
async fn hire_payload_fetches_and_persists_raw_when_local_missing() {
    let _g = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 未下载（无 personas/code-master 目录）；builtin live 强败（code-master
    // 非 builtin，探测 404 会打真网络——取材链里 builtin live 臂排在 agency
    // 在线臂之前，必须强制离线）；agency 在线臂走内容缓存注入 = 离线。
    let _forced = ForcedBuiltinLiveFail::arm();
    CONTENT_CACHE
        .lock()
        .unwrap()
        .insert("agency/code-master".to_string(), RAW_SAMPLE.to_string());

    let payload = expert_hire_payload(ws.to_str().unwrap(), "code-master")
        .await
        .unwrap();
    assert_eq!(
        payload["prompt"],
        "# Code Master\n\n## Core Mission\n写好代码。"
    );
    // 原文落盘（先存原文再雇佣）：后续雇佣走本地存档，不再依赖上游。
    let saved =
        std::fs::read_to_string(ws.join("personas").join("code-master").join("raw.md")).unwrap();
    assert_eq!(saved, RAW_SAMPLE);
    CONTENT_CACHE.lock().unwrap().clear();
}

#[tokio::test]
async fn hire_payload_rejects_empty_body() {
    // 本地原文只有 frontmatter 无正文（trim 后空）
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let pdir = ws.join("personas").join("bare");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("raw.md"), "---\nname: x\n---\n\n   \n").unwrap();
    let err = expert_hire_payload(ws.to_str().unwrap(), "bare")
        .await
        .unwrap_err();
    assert!(err.contains("原文正文为空"), "实际: {err}");
}

#[test]
fn strip_frontmatter_body_plain_and_unterminated() {
    // 无 frontmatter 原样返回
    assert_eq!(strip_frontmatter_body("plain body"), "plain body");
    // 未闭合（无第二个 ---）原样返回
    assert_eq!(strip_frontmatter_body("---\nname: x\n"), "---\nname: x\n");
    // 正常剥离 + 前导空行吃掉
    assert_eq!(strip_frontmatter_body("---\nname: x\n---\n\nbody"), "body");
}
