//! 诊断复现（2026-08-29）：Dashboard 手动添加的记忆条目没有被自动注入——
//! 请求日志（bin_windows home 的 request_logs/2026-08-29_00-16-40_8af0）里
//! 5 轮请求的 `# Memory Context` 均为 0 次，而配置层全部正确（主开关/强化
//! 记忆/auto_inject/模型文件/插件都在，日志确认 "Vector store initialized"
//! + "memory auto-inject enabled"）。
//!
//! 本测试用与部署**完全相同**的插件 DLL + config_dir（active tier 模型），
//! 独立验证嵌入推理是否成功、查询与记忆条目的余弦分数是否过 0.35 注入阈值。
//!
//! 运行（ONNX Runtime 不能重复 init → 单线程）：
//! ```text
//! cargo test -p nemesis-memory --test embed_repro -- --ignored --nocapture
//! ```

use nemesis_memory::types::VectorConfig;
use nemesis_memory::vector::{cosine_similarity, new_embedding_func};

const PLUGIN: &str = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\plugins\plugin_onnx.dll";
const CONFIG_DIR: &str =
    r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\config";
/// 用户当轮的真实提问（request_logs/2026-08-29_00-16-40_8af0/00.request.md）。
const QUERY: &str = "我记得周六下午我要做点事情，但是我忘了是啥了";
/// 用户手动添加的记忆条目（vector_store.jsonl 中 00:16:14 那条）。
const ENTRY: &str = "周六下午20点，要直播AI写驱动。";

#[test]
#[ignore]
fn deploy_env_embed_and_similarity() {
    let cfg = VectorConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some(PLUGIN.to_string()),
        config_dir: Some(CONFIG_DIR.to_string()),
        host_services: None,
    };

    // 与 gateway 运行时相同的嵌入函数构建路径（插件加载 + 模型解析 + init）。
    let embed = new_embedding_func(&cfg).expect("嵌入函数构建失败（插件/模型层）");

    let q = embed(QUERY).expect("查询文本嵌入失败");
    let d = embed(ENTRY).expect("记忆条目嵌入失败");
    let sim = cosine_similarity(&q, &d);
    println!(
        "dim query={} doc={} cosine={:.4}（注入阈值 0.35）",
        q.len(),
        d.len(),
        sim
    );

    assert_eq!(q.len(), 384, "medium 档应为 384 维");
    assert!(
        sim >= 0.35,
        "相似度 {sim:.4} 低于注入阈值 0.35 —— 即为该对话注入为空的直接原因"
    );
}

/// 端到端复现用户场景：与 gateway 等价的 manager 初始化（同 JSONL、同插件、
/// 同 config_dir），search_auto_inject 用用户真实提问检索。
/// 修复前：store 层 0.7 预过滤 → 0 命中；修复后：0.35 阈值 → 召回。
///
/// 注意：不走 `with_config_dir`——它探测的是「测试进程 exe 旁」的 plugins/，
/// 且探测失败会把 config.enhanced_memory.json 的 enabled 静默写回 false
/// （2026-08-29 实测踩坑）。这里手工初始化，等价且无副作用。
#[test]
#[ignore]
fn deploy_env_manager_search_auto_inject_end_to_end() {
    use nemesis_memory::manager::MemoryManager;
    use nemesis_memory::vector::StoreConfig;
    use std::path::Path;

    let data_dir =
        r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\memory_vector";
    let mgr = MemoryManager::new(&nemesis_memory::manager::Config::new(Path::new(data_dir)));

    let store_config = StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some(PLUGIN.to_string()),
        config_dir: Some(CONFIG_DIR.to_string()),
        max_results: 10,
        // 与 gateway 两处构建一致（with_config_dir 缺省 / dashboard 运行时 init）。
        similarity_threshold: 0.7,
        storage_path: format!("{data_dir}\\vector\\vector_store.jsonl"),
    };
    mgr.init_vector_store(Some(store_config))
        .expect("向量库初始化（插件+模型）");
    mgr.set_vector_enabled(true);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = runtime
        .block_on(mgr.search_auto_inject(QUERY, 5))
        .expect("search_auto_inject");

    println!(
        "search_auto_inject hits={} scores={:?}",
        result.total,
        result.entries.iter().map(|e| e.score).collect::<Vec<_>>()
    );
    assert!(
        result.total >= 1,
        "0.35 阈值下应召回「周六直播」条目（修复前 store 层 0.7 拦截为空）"
    );
    assert!(
        result.entries[0].entry.content.contains("直播"),
        "召回内容应为用户那条记忆"
    );
}
